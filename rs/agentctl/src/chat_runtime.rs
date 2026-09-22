//! Durable owner for provider-neutral inbound chat subscriptions.
//!
//! This module deliberately stops at the reviewed process-supervision boundary. It owns operator
//! configuration, replay cursors, event deduplication, request admission, and subscription commit
//! ordering. Launching a process plugin is integrated separately so the runtime cannot accidentally
//! grow a second process-group implementation beside the reviewed subscription supervisor.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{CString, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt as UnixFileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chat_subscription::{
    BackendConfiguration, ChannelId, ChatSubscription, CommittableEvent, DeliveryBatch,
    ProviderCursor, SenderId, SubscribeRequest, SubscriptionError, SubscriptionItem,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::agent::{self, DrainOptions, QueueMessageState};
use crate::subagents::{ManagedAgents, ManagedApi};

const STATE_VERSION: u32 = 1;
const MAX_REQUESTS: u64 = 2_048;
const MAX_REQUEST_BYTES: u64 = 128 * 1_024 * 1_024;
const MAX_REQUEST_RECORD_BYTES: usize = 512 * 1_024;
const MAX_PLUGIN_NAME_BYTES: usize = 128;
const MAX_AGENT_NAME_BYTES: usize = 32;
const MAX_AGENT_LABEL_BYTES: usize = 400;
const REPLY_NONCE_BYTES: usize = 16;
const MAX_REPLY_BYTES: usize = 30_000;
const MAX_REPLY_RECORD_BYTES: usize = 64 * 1_024;
const MAX_REPLY_ORDINAL: u32 = 999_999;
const MAX_REQUEST_REPLIES: u32 = 4_096;
const MAX_REQUEST_REPLY_BYTES: u64 = 64 * 1_024 * 1_024;
const MAX_STATE_REPLIES: u64 = 65_536;
const MAX_STATE_REPLY_BYTES: u64 = 1_024 * 1_024 * 1_024;
const MAX_VISIBLE_MARKERS: usize = 4_096;
const MAX_FEEDBACK_AVAILABLE_IDS: usize = 32;
const MAX_OUTBOUND_EXECUTABLE_BYTES: u64 = 64 * 1_024 * 1_024;
const COMMIT_RECEIPT_SLOTS: u64 = 256;
const RETIRED_ROUTE_SLOTS: u64 = 4_096;
const MAX_RETIREMENT_RECORD_BYTES: usize = 256 * 1_024;
const MAX_COMMIT_RECEIPT_BYTES: usize = 64 * 1_024;
const MAX_GAP_DIAGNOSTIC_BYTES: usize = 64 * 1_024;

fn default_ack_reaction() -> Option<String> {
    Some("🤖".to_owned())
}

const fn default_outbound_enabled() -> bool {
    true
}

const fn default_outbound_timeout_millis() -> u64 {
    30_000
}

const fn default_outbound_shutdown_grace_millis() -> u64 {
    2_000
}

/// A durable-runtime failure that never turns an uncertain provider commit into success.
#[derive(Debug)]
pub enum ChatRuntimeError {
    /// Durable coordinator delivery failed.
    Agent(crate::agent::AgentError),
    /// Local state I/O failed.
    Io(io::Error),
    /// Local JSON encoding failed.
    Json(serde_json::Error),
    /// A bounded state or protocol invariant was violated.
    Invalid(String),
    /// The provider-neutral subscription generation failed.
    Subscription(SubscriptionError),
    /// A provider declared an unrecoverable stream gap that must not be acknowledged.
    UnresolvedGap(String),
}

impl ChatRuntimeError {
    fn invalid(detail: impl Into<String>) -> Self {
        Self::Invalid(detail.into())
    }
}

impl fmt::Display for ChatRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(error) => fmt::Display::fmt(error, formatter),
            Self::Io(error) => write!(formatter, "chat state I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "chat state JSON failed: {error}"),
            Self::Invalid(detail) => formatter.write_str(detail),
            Self::Subscription(error) => fmt::Display::fmt(error, formatter),
            Self::UnresolvedGap(detail) => {
                write!(formatter, "provider stream has an unresolved gap: {detail}")
            }
        }
    }
}

impl std::error::Error for ChatRuntimeError {}

impl From<crate::agent::AgentError> for ChatRuntimeError {
    fn from(error: crate::agent::AgentError) -> Self {
        Self::Agent(error)
    }
}

impl From<io::Error> for ChatRuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ChatRuntimeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<SubscriptionError> for ChatRuntimeError {
    fn from(error: SubscriptionError) -> Self {
        Self::Subscription(error)
    }
}

type Result<T> = std::result::Result<T, ChatRuntimeError>;

/// Operator-controlled, non-secret provider configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfigurationDocument {
    /// Provider-owned schema identifier for the non-secret object.
    pub schema: String,
    /// Provider-owned non-secret configuration object.
    pub data: Map<String, Value>,
}

/// Operator-selected one-shot outbound helper configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundCommandConfiguration {
    /// Absolute canonical native helper executable.
    pub executable: PathBuf,
    /// Literal helper arguments; no shell is involved.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Exact inherited environment-variable names; values are never persisted.
    #[serde(default)]
    pub environment: Vec<String>,
    /// End-to-end stdin/stdout deadline in milliseconds.
    #[serde(default = "default_outbound_timeout_millis")]
    pub timeout_millis: u64,
    /// Cooperative process-exit grace before whole-group termination.
    #[serde(default = "default_outbound_shutdown_grace_millis")]
    pub shutdown_grace_millis: u64,
}

impl OutboundCommandConfiguration {
    fn validate(&self) -> Result<()> {
        if !self.executable.is_absolute()
            || self.arguments.len() > 128
            || self
                .arguments
                .iter()
                .any(|value| value.is_empty() || value.contains('\0'))
            || self.environment.len() > 64
            || self
                .environment
                .iter()
                .any(|value| !valid_environment_name(value))
            || self.timeout_millis == 0
            || self.timeout_millis > 30_000
            || self.shutdown_grace_millis > 30_000
        {
            return Err(ChatRuntimeError::invalid(
                "invalid outbound helper path, arguments, environment names, or deadlines",
            ));
        }
        let mut deduplicated = self.environment.clone();
        deduplicated.sort();
        deduplicated.dedup();
        if deduplicated.len() != self.environment.len() {
            return Err(ChatRuntimeError::invalid(
                "outbound helper environment names must be unique",
            ));
        }
        Ok(())
    }

    fn open(&self) -> Result<CommandOutboundTransport> {
        self.validate()?;
        CommandOutboundTransport::new(
            self.executable.clone(),
            self.arguments.iter().map(OsString::from).collect(),
            &self.environment,
            Duration::from_millis(self.timeout_millis),
            Duration::from_millis(self.shutdown_grace_millis),
        )
    }
}

/// Authority and routing configuration retained for one bridge state directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfiguration {
    /// Exact discovered process-plugin name.
    pub subscription_plugin: String,
    /// Provider channel or space authorities.
    pub channel_ids: Vec<String>,
    /// Authenticated provider sender authorities.
    pub allowed_senders: Vec<String>,
    /// Stable named coordinator in the agent registry.
    pub agent_name: String,
    /// Human-readable agent label used in outbound replies.
    pub agent_label: String,
    /// Whether coordinator output is captured and published back to chat.
    #[serde(default = "default_outbound_enabled")]
    pub outbound_enabled: bool,
    /// Optional reaction placed only after durable inbound admission.
    #[serde(default = "default_ack_reaction")]
    pub ack_reaction: Option<String>,
    /// Optional provider-specific non-secret resource configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_configuration: Option<BackendConfigurationDocument>,
    /// Optional bounded request/response adapter for replies and reactions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_command: Option<OutboundCommandConfiguration>,
}

impl BridgeConfiguration {
    fn validate(&self) -> Result<()> {
        validate_slug(
            &self.subscription_plugin,
            "subscription plugin",
            MAX_PLUGIN_NAME_BYTES,
        )?;
        validate_slug(&self.agent_name, "agent name", MAX_AGENT_NAME_BYTES)?;
        validate_single_line(&self.agent_label, "agent label", MAX_AGENT_LABEL_BYTES)?;
        if let Some(reaction) = self.ack_reaction.as_deref() {
            validate_single_line(reaction, "ack reaction", 64)?;
        }
        if let Some(outbound) = &self.outbound_command {
            outbound.validate()?;
        }
        if !self.outbound_enabled
            && (self.ack_reaction.is_some() || self.outbound_command.is_some())
        {
            return Err(ChatRuntimeError::invalid(
                "outbound-disabled chat configuration cannot enable reactions or a reply helper",
            ));
        }
        let _ = self.subscribe_request(None)?;
        Ok(())
    }

    /// Read a strict, bounded, private owner-only configuration document.
    pub fn load_private(path: &Path) -> Result<Self> {
        let configuration: Self = read_document(path, 1 << 20)?;
        configuration.validate()?;
        Ok(configuration)
    }

    /// Validate and pin the configured one-shot outbound helper, when present.
    pub fn outbound_transport(&self) -> Result<Option<CommandOutboundTransport>> {
        self.outbound_command
            .as_ref()
            .map(OutboundCommandConfiguration::open)
            .transpose()
    }

    /// Build and validate one provider-neutral subscription request.
    pub fn subscribe_request(&self, cursor: Option<&str>) -> Result<SubscribeRequest> {
        let channels = self
            .channel_ids
            .iter()
            .map(|value| ChannelId::new(value.clone()).map_err(|error| error.to_string()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(ChatRuntimeError::invalid)?;
        let senders = self
            .allowed_senders
            .iter()
            .map(|value| SenderId::new(value.clone()).map_err(|error| error.to_string()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(ChatRuntimeError::invalid)?;
        let cursor = cursor
            .map(|value| ProviderCursor::new(value.to_owned()))
            .transpose()
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        let request = SubscribeRequest::new(channels, senders, cursor)
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        match self.backend_configuration.as_ref() {
            Some(document) => {
                BackendConfiguration::new(document.schema.clone(), document.data.clone())
                    .map(|configuration| request.with_backend_configuration(configuration))
                    .map_err(|error| ChatRuntimeError::invalid(error.to_string()))
            }
            None => Ok(request),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationEnvelope {
    version: u32,
    config: BridgeConfiguration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    cursor: Option<String>,
    request_count: u64,
    request_bytes: u64,
    committed_batches: u64,
    reconciliation_required: bool,
    #[serde(default)]
    host_batch_sequence: u64,
    #[serde(default)]
    retirement_sequence: u64,
    #[serde(default)]
    retired_route_count: u64,
    reply_count: u64,
    reply_bytes: u64,
    updated_at_millis: u64,
}

impl Checkpoint {
    fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            cursor: None,
            request_count: 0,
            request_bytes: 0,
            committed_batches: 0,
            reconciliation_required: false,
            host_batch_sequence: 0,
            retirement_sequence: 0,
            retired_route_count: 0,
            reply_count: 0,
            reply_bytes: 0,
            updated_at_millis: unix_millis(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommitReceiptPhase {
    Prepared,
    Committed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitReceipt {
    version: u32,
    phase: CommitReceiptPhase,
    host_batch_sequence: u64,
    provider_sequence: u64,
    delivery_id: String,
    cursor: String,
    event_count: u32,
    batch_fingerprint: String,
    prepared_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    committed_at_millis: Option<u64>,
}

impl CommitReceipt {
    fn validate(&self) -> Result<()> {
        if self.version != STATE_VERSION
            || self.host_batch_sequence == 0
            || self.provider_sequence == 0
            || self.delivery_id.is_empty()
            || self.delivery_id.len() > chat_subscription::MAX_TOKEN_BYTES
            || self.cursor.is_empty()
            || self.cursor.len() > chat_subscription::MAX_TOKEN_BYTES
            || self.event_count == 0
            || !valid_key(&self.batch_fingerprint)
            || usize::try_from(self.event_count).unwrap_or(usize::MAX)
                > chat_subscription::MAX_BATCH_EVENTS
            || matches!(self.phase, CommitReceiptPhase::Prepared)
                != self.committed_at_millis.is_none()
        {
            return Err(ChatRuntimeError::invalid(
                "commit receipt is inconsistent or outside protocol bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GapPhase {
    Unresolved,
    Resolved,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GapDiagnostic {
    version: u32,
    phase: GapPhase,
    provider_sequence: u64,
    delivery_id: String,
    proposed_cursor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_cursor: Option<String>,
    reasons: Vec<String>,
    batch_fingerprint: String,
    observed_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_at_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_host_batch_sequence: Option<u64>,
}

impl GapDiagnostic {
    fn validate(&self) -> Result<()> {
        if self.version != STATE_VERSION
            || self.provider_sequence == 0
            || self.delivery_id.is_empty()
            || self.delivery_id.len() > chat_subscription::MAX_TOKEN_BYTES
            || self.proposed_cursor.is_empty()
            || self.proposed_cursor.len() > chat_subscription::MAX_TOKEN_BYTES
            || self.reasons.is_empty()
            || self.reasons.len() > chat_subscription::MAX_BATCH_EVENTS
            || self.reasons.iter().any(|reason| {
                reason.is_empty()
                    || reason.len() > chat_subscription::MAX_GAP_REASON_BYTES
                    || reason.contains('\0')
            })
            || !valid_key(&self.batch_fingerprint)
            || matches!(self.phase, GapPhase::Unresolved)
                != (self.resolved_at_millis.is_none()
                    && self.resolved_host_batch_sequence.is_none())
        {
            return Err(ChatRuntimeError::invalid(
                "gap diagnostic is inconsistent or outside protocol bounds",
            ));
        }
        if let Some(cursor) = self.resume_cursor.as_deref() {
            ProviderCursor::new(cursor.to_owned())
                .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RetirementPhase {
    Prepared,
    Retired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredReplyReceipt {
    ordinal: u32,
    send_request_id: String,
    provider_message_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementRecord {
    version: u32,
    phase: RetirementPhase,
    retirement_sequence: u64,
    request_key: String,
    message_fingerprint: String,
    reply_nonce: String,
    admitted_cursor: String,
    request_bytes: u64,
    reply_bytes: u64,
    reply_count: u32,
    delivery_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ack_reaction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ack_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reaction_already_present: Option<bool>,
    replies: Vec<RetiredReplyReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evicted_request_key: Option<String>,
    prepared_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retired_at_millis: Option<u64>,
}

impl RetirementRecord {
    fn validate(&self) -> Result<()> {
        if self.version != STATE_VERSION
            || self.retirement_sequence == 0
            || !valid_key(&self.request_key)
            || !valid_key(&self.message_fingerprint)
            || !valid_nonce(&self.reply_nonce)
            || self.reply_count != u32::try_from(self.replies.len()).unwrap_or(u32::MAX)
            || self
                .evicted_request_key
                .as_deref()
                .is_some_and(|key| !valid_key(key))
            || matches!(self.phase, RetirementPhase::Prepared) != self.retired_at_millis.is_none()
        {
            return Err(ChatRuntimeError::invalid(
                "retirement journal is inconsistent or outside protocol bounds",
            ));
        }
        match (
            self.ack_reaction.as_deref(),
            self.ack_request_id.as_deref(),
            self.reaction_id.as_deref(),
            self.reaction_already_present,
        ) {
            (None, None, None, None) => {}
            (Some(reaction), Some(request_id), Some(reaction_id), Some(_))
                if !reaction.is_empty()
                    && valid_operation_uuid(request_id)
                    && !reaction_id.is_empty()
                    && reaction_id.len() <= chat_subscription::MAX_RESOURCE_ID_BYTES => {}
            _ => {
                return Err(ChatRuntimeError::invalid(
                    "retirement reaction receipt is incomplete",
                ));
            }
        }
        ProviderCursor::new(self.admitted_cursor.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        if self.replies.iter().enumerate().any(|(index, reply)| {
            reply.ordinal != u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1)
                || !valid_operation_uuid(&reply.send_request_id)
                || reply.provider_message_id.is_empty()
                || reply.provider_message_id.len() > chat_subscription::MAX_RESOURCE_ID_BYTES
        }) {
            return Err(ChatRuntimeError::invalid(
                "retirement reply receipts are inconsistent or outside protocol bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredRouteIndex {
    version: u32,
    retirement_sequence: u64,
    request_key: String,
    message_fingerprint: String,
    reply_nonce: String,
    admitted_cursor: String,
    retired_at_millis: u64,
}

impl RetiredRouteIndex {
    fn validate(&self) -> Result<()> {
        if self.version != STATE_VERSION
            || self.retirement_sequence == 0
            || !valid_key(&self.request_key)
            || !valid_key(&self.message_fingerprint)
            || !valid_nonce(&self.reply_nonce)
        {
            return Err(ChatRuntimeError::invalid(
                "retired route index is inconsistent or outside protocol bounds",
            ));
        }
        ProviderCursor::new(self.admitted_cursor.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestPhase {
    Pending,
    Submitting,
    Delivered,
    DeliveryUncertain,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AckPhase {
    Disabled,
    Pending,
    Sending,
    Acked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMessage {
    channel_id: String,
    message_id: String,
    thread_id: String,
    sender_id: String,
    text: String,
    created_at: String,
    thread_reply: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_payload: Option<ProviderPayloadDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderPayloadDocument {
    schema: String,
    data: Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    version: u32,
    key: String,
    phase: RequestPhase,
    message: SavedMessage,
    reply_nonce: String,
    next_reply_ordinal: u32,
    next_send_ordinal: u32,
    reply_count: u32,
    reply_bytes: u64,
    reply_closed: bool,
    delivery_message_id: String,
    ack_phase: AckPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ack_reaction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ack_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reaction_already_present: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ack_error: Option<String>,
    admitted_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admitted_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_error: Option<String>,
}

impl RequestRecord {
    fn from_message(
        message: &chat_subscription::InboundMessage,
        ack_reaction: Option<&str>,
    ) -> Result<Self> {
        let source = SavedMessage {
            channel_id: message.channel_id().as_str().to_owned(),
            message_id: message.message_id().as_str().to_owned(),
            thread_id: message.thread_id().as_str().to_owned(),
            sender_id: message.sender_id().as_str().to_owned(),
            text: message.text().to_owned(),
            created_at: message.created_at().to_owned(),
            thread_reply: message.is_thread_reply(),
            provider_payload: message
                .provider_payload()
                .map(|payload| ProviderPayloadDocument {
                    schema: payload.schema().to_owned(),
                    data: payload.data().clone(),
                }),
        };
        let key = message_key(&source)?;
        let ack_request_id = ack_reaction.map(|_| random_operation_uuid()).transpose()?;
        Ok(Self {
            version: STATE_VERSION,
            phase: RequestPhase::Pending,
            reply_nonce: random_reply_nonce()?,
            next_reply_ordinal: 1,
            next_send_ordinal: 1,
            reply_count: 0,
            reply_bytes: 0,
            reply_closed: false,
            delivery_message_id: format!("chat-{key}"),
            ack_phase: if ack_reaction.is_some() {
                AckPhase::Pending
            } else {
                AckPhase::Disabled
            },
            ack_reaction: ack_reaction.map(str::to_owned),
            ack_request_id,
            reaction_id: None,
            reaction_already_present: None,
            ack_error: None,
            admitted_at_millis: unix_millis(),
            admitted_cursor: None,
            delivery_error: None,
            key,
            message: source,
        })
    }

    fn validate(&self, path_key: &str) -> Result<()> {
        if self.version != STATE_VERSION {
            return Err(ChatRuntimeError::invalid(format!(
                "request {} has unsupported version {}",
                self.key, self.version
            )));
        }
        if self.key != path_key || self.key != message_key(&self.message)? {
            return Err(ChatRuntimeError::invalid(
                "saved request identity does not match its normalized message",
            ));
        }
        self.message.validate()?;
        if self.next_reply_ordinal == 0
            || self.next_reply_ordinal > MAX_REPLY_ORDINAL.saturating_add(1)
            || self.next_send_ordinal == 0
            || self.next_send_ordinal > self.next_reply_ordinal
            || self.reply_count > MAX_REQUEST_REPLIES
            || self.reply_bytes > MAX_REQUEST_REPLY_BYTES
            || self.next_reply_ordinal != self.reply_count.saturating_add(1)
        {
            return Err(ChatRuntimeError::invalid(
                "saved request reply ordinals are inconsistent or outside the protocol range",
            ));
        }
        if !valid_nonce(&self.reply_nonce) {
            return Err(ChatRuntimeError::invalid(
                "saved request reply nonce is not 22-character base64url",
            ));
        }
        if self.delivery_message_id != format!("chat-{}", self.key) {
            return Err(ChatRuntimeError::invalid(
                "saved request delivery id does not match its request key",
            ));
        }
        if let Some(cursor) = self.admitted_cursor.as_deref() {
            ProviderCursor::new(cursor.to_owned())
                .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        }
        if let Some(error) = self.delivery_error.as_deref() {
            validate_optional_single_line_or_multiline(error, "delivery error", 2_000)?;
        }
        if let Some(error) = self.ack_error.as_deref() {
            validate_optional_single_line_or_multiline(error, "ack error", 2_000)?;
        }
        if let Some(reaction) = self.ack_reaction.as_deref() {
            validate_single_line(reaction, "ack reaction", 64)?;
        }
        if let Some(reaction_id) = self.reaction_id.as_deref() {
            validate_single_line(
                reaction_id,
                "provider reaction id",
                chat_subscription::MAX_RESOURCE_ID_BYTES,
            )?;
        }
        match self.ack_phase {
            AckPhase::Disabled
                if self.ack_reaction.is_none()
                    && self.ack_request_id.is_none()
                    && self.reaction_id.is_none()
                    && self.reaction_already_present.is_none() => {}
            AckPhase::Pending | AckPhase::Sending
                if self
                    .ack_reaction
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                    && self
                        .ack_request_id
                        .as_deref()
                        .is_some_and(valid_operation_uuid)
                    && self.reaction_id.is_none()
                    && self.reaction_already_present.is_none() => {}
            AckPhase::Acked
                if self
                    .ack_reaction
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                    && self
                        .ack_request_id
                        .as_deref()
                        .is_some_and(valid_operation_uuid)
                    && self.reaction_id.is_some()
                    && self.reaction_already_present.is_some() => {}
            _ => {
                return Err(ChatRuntimeError::invalid(
                    "saved request acknowledgement state is inconsistent",
                ));
            }
        }
        Ok(())
    }
}

impl SavedMessage {
    fn validate(&self) -> Result<()> {
        let channel = ChannelId::new(self.channel_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        let message = chat_subscription::MessageId::new(self.message_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        let thread = chat_subscription::ThreadId::new(self.thread_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        let sender = SenderId::new(self.sender_id.clone())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        let normalized = chat_subscription::InboundMessage::new(
            channel,
            message,
            thread,
            sender,
            self.text.clone(),
            self.created_at.clone(),
            self.thread_reply,
        )
        .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
        if let Some(payload) = &self.provider_payload {
            let payload = chat_subscription::ProviderPayload::new(
                payload.schema.clone(),
                payload.data.clone(),
            )
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
            let _ = normalized.with_provider_payload(payload);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplyPhase {
    Pending,
    Sending,
    Sent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyRecord {
    version: u32,
    request_key: String,
    ordinal: u32,
    body: String,
    send_request_id: String,
    reserved_bytes: u64,
    phase: ReplyPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_message_id: Option<String>,
    captured_at_millis: u64,
}

impl ReplyRecord {
    fn new(request_key: &str, ordinal: u32, body: String) -> Result<Self> {
        validate_reply_body(&body)?;
        if !valid_key(request_key) || ordinal == 0 || ordinal > MAX_REPLY_ORDINAL {
            return Err(ChatRuntimeError::invalid(
                "reply identity is outside the protocol range",
            ));
        }
        let send_request_id = random_operation_uuid()?;
        let mut record = Self {
            version: STATE_VERSION,
            request_key: request_key.to_owned(),
            ordinal,
            body,
            send_request_id,
            reserved_bytes: 0,
            phase: ReplyPhase::Pending,
            provider_message_id: None,
            captured_at_millis: unix_millis(),
        };
        let initial = encoded_document_bytes(&record)?;
        let receipt_reservation = chat_subscription::MAX_RESOURCE_ID_BYTES
            .saturating_mul(6)
            .saturating_add(128);
        record.reserved_bytes = u64::try_from(initial.saturating_add(receipt_reservation))
            .map_err(|_| ChatRuntimeError::invalid("reply reservation does not fit u64"))?;
        record.validate(request_key, ordinal)?;
        Ok(record)
    }

    fn validate(&self, request_key: &str, ordinal: u32) -> Result<()> {
        if self.version != STATE_VERSION
            || self.request_key != request_key
            || self.ordinal != ordinal
            || !valid_operation_uuid(&self.send_request_id)
        {
            return Err(ChatRuntimeError::invalid(
                "saved reply identity does not match its artifact path",
            ));
        }
        validate_reply_body(&self.body)?;
        if !valid_operation_uuid(&self.send_request_id) {
            return Err(ChatRuntimeError::invalid(
                "saved reply operation id is not an RFC 4122 UUID",
            ));
        }
        let actual = u64::try_from(encoded_document_bytes(self)?)
            .map_err(|_| ChatRuntimeError::invalid("reply length does not fit u64"))?;
        if actual > self.reserved_bytes
            || self.reserved_bytes > u64::try_from(MAX_REPLY_RECORD_BYTES).unwrap_or(u64::MAX)
        {
            return Err(ChatRuntimeError::invalid(
                "saved reply exceeds its durable byte reservation",
            ));
        }
        if matches!(self.phase, ReplyPhase::Sent) && self.provider_message_id.is_none() {
            return Err(ChatRuntimeError::invalid(
                "sent reply is missing its provider message receipt",
            ));
        }
        if let Some(message_id) = self.provider_message_id.as_deref() {
            validate_single_line(
                message_id,
                "provider reply message id",
                chat_subscription::MAX_RESOURCE_ID_BYTES,
            )?;
        }
        Ok(())
    }
}

/// One outbound provider request. `request_id` remains stable across uncertain retries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplySubmission<'a> {
    /// Exact provider channel or space resource.
    pub channel_id: &'a str,
    /// Exact originating thread resource.
    pub thread_id: &'a str,
    /// Validated nonempty reply body.
    pub body: &'a str,
    /// Stable idempotency key retained across uncertain retries.
    pub request_id: &'a str,
}

/// Request/response transport kept separate from the inbound subscription trait.
pub trait ReplyTransport {
    /// Post or reconcile one reply, returning its immutable provider message identifier.
    fn send(
        &mut self,
        submission: ReplySubmission<'_>,
    ) -> std::result::Result<String, OutboundFailure>;
}

/// One idempotent ensure-reaction request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReactionSubmission<'a> {
    /// Exact provider channel or space resource.
    pub channel_id: &'a str,
    /// Exact durably admitted provider message resource.
    pub message_id: &'a str,
    /// Configured Unicode reaction.
    pub emoji: &'a str,
    /// Stable UUID retained across retries.
    pub request_id: &'a str,
}

/// Provider receipt for an ensured reaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReactionReceipt {
    /// Immutable provider reaction resource.
    pub reaction_id: String,
    /// Whether reconciliation found the reaction already present.
    pub already_present: bool,
}

/// Idempotent outbound reaction transport, independent of inbound subscriptions.
pub trait ReactionTransport {
    /// Ensure the configured reaction is present and return an exact receipt.
    fn ensure_reaction(
        &mut self,
        submission: ReactionSubmission<'_>,
    ) -> std::result::Result<ReactionReceipt, OutboundFailure>;
}

/// Whether a failed outbound operation is proven absent or may have applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundOutcome {
    /// The provider operation was proven not to have applied.
    NotApplied,
    /// The provider outcome could not be proven; only the identical UUID may be retried.
    Unknown,
}

/// Bounded provider or adapter failure for one durable outbound operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundFailure {
    /// Stable machine-readable failure code.
    pub code: String,
    /// Bounded diagnostic without credentials.
    pub detail: String,
    /// Provider application evidence.
    pub outcome: OutboundOutcome,
    /// Whether repeating the identical UUID and fields may succeed.
    pub retryable: bool,
}

impl OutboundFailure {
    fn protocol(detail: impl Into<String>) -> Self {
        Self {
            code: "adapter_protocol".to_owned(),
            detail: bounded_detail(&detail.into(), 2_000),
            outcome: OutboundOutcome::Unknown,
            retryable: true,
        }
    }

    fn not_applied(code: &str, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            detail: bounded_detail(&detail.into(), 2_000),
            outcome: OutboundOutcome::NotApplied,
            retryable,
        }
    }

    fn unknown(code: &str, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            detail: bounded_detail(&detail.into(), 2_000),
            outcome: OutboundOutcome::Unknown,
            retryable,
        }
    }
}

impl fmt::Display for OutboundFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {} (outcome={:?}, retryable={})",
            self.code, self.detail, self.outcome, self.retryable
        )
    }
}

impl std::error::Error for OutboundFailure {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PinnedExecutableIdentity {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    digest: [u8; 32],
}

/// Bounded one-shot NDJSON adapter hosted by the reviewed process supervisor.
///
/// The native executable is opened, hashed, and copied into a sealed in-memory executable at
/// construction. Every operation executes that immutable generation image without reopening or
/// rereading the source path. The command receives one request line followed by EOF and must emit
/// exactly one response line before exiting. Only the explicitly named environment values are
/// captured; credentials never enter protocol frames.
pub struct CommandOutboundTransport {
    executable_image: File,
    executable_path: PathBuf,
    _source_identity: PinnedExecutableIdentity,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    timeout: Duration,
    shutdown_grace: Duration,
    cancellation: Option<OutboundCancellation>,
}

/// Cloneable event-driven cancellation for a running one-shot outbound helper.
#[derive(Clone, Debug)]
pub struct OutboundCancellation {
    descriptor: Arc<File>,
    cancelled: Arc<AtomicBool>,
}

impl OutboundCancellation {
    /// Create one nonblocking event descriptor shared by a service generation.
    pub fn new() -> io::Result<Self> {
        let descriptor = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor: Arc::new(unsafe { File::from_raw_fd(descriptor) }),
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Wake any helper pipe wait and make future launches refuse before spawn.
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        let value = 1_u64.to_ne_bytes();
        let result = unsafe {
            libc::write(
                self.descriptor.as_raw_fd(),
                value.as_ptr().cast(),
                value.len(),
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            debug_assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl CommandOutboundTransport {
    /// Validate and pin one native helper and its non-secret launch configuration.
    ///
    /// `environment_names` is an operator-controlled allowlist. Each named value must already be
    /// present. The helper must be an absolute canonical regular file owned by the current UID,
    /// single-linked, owner-executable, and not group/world writable.
    pub fn new(
        executable_path: PathBuf,
        arguments: Vec<OsString>,
        environment_names: &[String],
        timeout: Duration,
        shutdown_grace: Duration,
    ) -> Result<Self> {
        if !executable_path.is_absolute() || fs::canonicalize(&executable_path)? != executable_path
        {
            return Err(ChatRuntimeError::invalid(
                "outbound helper path must be absolute and canonical",
            ));
        }
        if arguments.len() > 128
            || arguments
                .iter()
                .any(|argument| argument.is_empty() || argument.as_bytes().contains(&0))
        {
            return Err(ChatRuntimeError::invalid(
                "outbound helper accepts at most 128 nonempty NUL-free arguments",
            ));
        }
        if timeout.is_zero()
            || timeout > Duration::from_secs(30)
            || shutdown_grace > Duration::from_secs(30)
        {
            return Err(ChatRuntimeError::invalid(
                "outbound helper timeout must be 1ns-30s and shutdown grace at most 30s",
            ));
        }
        if environment_names.len() > 64 {
            return Err(ChatRuntimeError::invalid(
                "outbound helper environment allowlist exceeds 64 names",
            ));
        }
        let mut seen_environment = BTreeMap::new();
        for name in environment_names {
            if !valid_environment_name(name) || seen_environment.contains_key(name) {
                return Err(ChatRuntimeError::invalid(
                    "outbound helper environment names must be unique shell identifiers",
                ));
            }
            let value = env::var_os(name).ok_or_else(|| {
                ChatRuntimeError::invalid(format!(
                    "outbound helper environment value {name:?} is unavailable"
                ))
            })?;
            seen_environment.insert(name.clone(), value);
        }
        let metadata = fs::symlink_metadata(&executable_path)?;
        if metadata.file_type().is_symlink() {
            return Err(ChatRuntimeError::invalid(
                "outbound helper executable must not be a symlink",
            ));
        }
        let executable = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&executable_path)?;
        let (executable_image, source_identity) = pin_executable_image(&executable, None)?;
        Ok(Self {
            executable_image,
            executable_path,
            _source_identity: source_identity,
            arguments,
            environment: seen_environment
                .into_iter()
                .map(|(name, value)| (OsString::from(name), value))
                .collect(),
            timeout,
            shutdown_grace,
            cancellation: None,
        })
    }

    /// Bind service-generation cancellation before processing any operation.
    pub fn set_cancellation(&mut self, cancellation: OutboundCancellation) {
        self.cancellation = Some(cancellation);
    }

    fn exchange(&self, request: &[u8]) -> std::result::Result<Vec<u8>, OutboundFailure> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(OutboundCancellation::is_cancelled)
        {
            return Err(OutboundFailure::not_applied(
                "helper_cancelled",
                "outbound helper launch was cancelled before spawn",
                true,
            ));
        }
        let deadline = Instant::now().checked_add(self.timeout).ok_or_else(|| {
            OutboundFailure::not_applied(
                "invalid_deadline",
                "outbound helper deadline is unrepresentable",
                false,
            )
        })?;
        let descriptor_path = PathBuf::from(format!(
            "/proc/self/fd/{}",
            self.executable_image.as_raw_fd()
        ));
        let mut command = Command::new(descriptor_path);
        command
            .args(&self.arguments)
            .env_clear()
            .envs(self.environment.iter().cloned())
            .env("AGENTCTL_PROCESS_SUPERVISED", "1");
        if let Some(parent) = self.executable_path.parent() {
            command.current_dir(parent);
        }
        let mut child = chat_subscription_plugin::process::ProcessPluginChild::spawn(command)
            .map_err(|error| {
                OutboundFailure::not_applied("helper_spawn_failed", error.to_string(), true)
            })?;
        if self
            .cancellation
            .as_ref()
            .is_some_and(OutboundCancellation::is_cancelled)
        {
            let cleanup = child.shutdown(Duration::ZERO);
            return Err(OutboundFailure::not_applied(
                "helper_cancelled",
                cleanup_detail(
                    "outbound helper was cancelled before request transfer".to_owned(),
                    cleanup,
                ),
                true,
            ));
        }
        let (mut reader, mut writer) = match child.take_transport() {
            Ok(transport) => transport,
            Err(error) => {
                let cleanup = child.shutdown(Duration::ZERO);
                return Err(OutboundFailure::not_applied(
                    "helper_transport_failed",
                    cleanup_detail(error.to_string(), cleanup),
                    true,
                ));
            }
        };
        if let Err(error) = set_nonblocking(writer.as_raw_fd())
            .and_then(|()| set_nonblocking(reader.as_raw_fd()))
            .and_then(|()| {
                write_nonblocking(&mut writer, request, deadline, self.cancellation.as_ref())
            })
        {
            drop(writer);
            drop(reader);
            let cleanup = child.shutdown(Duration::ZERO);
            return Err(OutboundFailure::unknown(
                "helper_request_unknown",
                cleanup_detail(error.to_string(), cleanup),
                true,
            ));
        }
        drop(writer);
        let output = match read_nonblocking(&mut reader, deadline, self.cancellation.as_ref()) {
            Ok(output) => output,
            Err(error) => {
                drop(reader);
                let cleanup = child.shutdown(Duration::ZERO);
                return Err(OutboundFailure::unknown(
                    "helper_response_unknown",
                    cleanup_detail(error.to_string(), cleanup),
                    true,
                ));
            }
        };
        drop(reader);
        let status = match self.cancellation.as_ref() {
            Some(cancellation) => {
                child.shutdown_cancellable(self.shutdown_grace, cancellation.descriptor.as_raw_fd())
            }
            None => child.shutdown(self.shutdown_grace),
        }
        .map_err(|error| {
            OutboundFailure::unknown("helper_cleanup_unknown", error.to_string(), true)
        })?;
        if self
            .cancellation
            .as_ref()
            .is_some_and(OutboundCancellation::is_cancelled)
        {
            return Err(OutboundFailure::unknown(
                "helper_cancelled",
                "outbound helper was cancelled during final process-group cleanup",
                true,
            ));
        }
        if !status.success() {
            return Err(OutboundFailure::unknown(
                "helper_exit_failed",
                format!("outbound helper exited with {status}"),
                true,
            ));
        }
        exact_response_line(output)
    }
}

impl ReplyTransport for CommandOutboundTransport {
    fn send(
        &mut self,
        submission: ReplySubmission<'_>,
    ) -> std::result::Result<String, OutboundFailure> {
        let request = encode_send_request(&submission)?;
        let response = self.exchange(&request)?;
        decode_send_response(&response, &submission)
    }
}

impl ReactionTransport for CommandOutboundTransport {
    fn ensure_reaction(
        &mut self,
        submission: ReactionSubmission<'_>,
    ) -> std::result::Result<ReactionReceipt, OutboundFailure> {
        let request = encode_reaction_request(&submission)?;
        let response = self.exchange(&request)?;
        decode_reaction_response(&response, &submission)
    }
}

#[derive(Serialize)]
struct SendWireRequest<'a> {
    version: u32,
    id: &'a str,
    action: &'static str,
    channel_id: &'a str,
    thread_id: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct ReactionWireRequest<'a> {
    version: u32,
    id: &'a str,
    action: &'static str,
    channel_id: &'a str,
    message_id: &'a str,
    emoji: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireResponse {
    Success(WireSuccess),
    Failure(WireFailure),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSuccess {
    version: u32,
    id: String,
    action: String,
    ok: bool,
    receipt: WireReceipt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFailure {
    version: u32,
    id: Value,
    action: Value,
    ok: bool,
    error: WireError,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireReceipt {
    Send(SendWireReceipt),
    Reaction(ReactionWireReceipt),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendWireReceipt {
    message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionWireReceipt {
    reaction_id: String,
    already_present: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    code: String,
    detail: String,
    outcome: OutboundOutcome,
    retryable: bool,
}

/// Encode one exact v1 outbound send request as a newline-delimited strict JSON frame.
pub fn encode_send_request(
    submission: &ReplySubmission<'_>,
) -> std::result::Result<Vec<u8>, OutboundFailure> {
    validate_outbound_send(submission)?;
    encode_wire_request(&SendWireRequest {
        version: 1,
        id: submission.request_id,
        action: "send",
        channel_id: submission.channel_id,
        thread_id: submission.thread_id,
        text: submission.body,
    })
}

/// Encode one exact v1 outbound ensure-reaction request as a newline-delimited strict JSON frame.
pub fn encode_reaction_request(
    submission: &ReactionSubmission<'_>,
) -> std::result::Result<Vec<u8>, OutboundFailure> {
    validate_outbound_reaction(submission)?;
    encode_wire_request(&ReactionWireRequest {
        version: 1,
        id: submission.request_id,
        action: "ensure_reaction",
        channel_id: submission.channel_id,
        message_id: submission.message_id,
        emoji: submission.emoji,
    })
}

/// Decode and bind one strict v1 send response to its exact requested authority.
pub fn decode_send_response(
    payload: &[u8],
    submission: &ReplySubmission<'_>,
) -> std::result::Result<String, OutboundFailure> {
    validate_outbound_send(submission)?;
    match decode_wire_response(payload, submission.request_id, "send")? {
        WireReceipt::Send(receipt) => {
            let prefix = format!("{}/messages/", submission.channel_id);
            if !receipt.message_id.starts_with(&prefix)
                || receipt.message_id.len() <= prefix.len()
                || receipt.message_id.len() > chat_subscription::MAX_RESOURCE_ID_BYTES
            {
                return Err(OutboundFailure::protocol(
                    "send receipt names a message outside the requested channel",
                ));
            }
            Ok(receipt.message_id)
        }
        WireReceipt::Reaction(_) => Err(OutboundFailure::protocol(
            "send response contains a reaction receipt",
        )),
    }
}

/// Decode and bind one strict v1 reaction response to its exact requested message.
pub fn decode_reaction_response(
    payload: &[u8],
    submission: &ReactionSubmission<'_>,
) -> std::result::Result<ReactionReceipt, OutboundFailure> {
    validate_outbound_reaction(submission)?;
    match decode_wire_response(payload, submission.request_id, "ensure_reaction")? {
        WireReceipt::Reaction(receipt) => {
            let prefix = format!("{}/reactions/", submission.message_id);
            if !receipt.reaction_id.starts_with(&prefix)
                || receipt.reaction_id.len() <= prefix.len()
                || receipt.reaction_id.len() > chat_subscription::MAX_RESOURCE_ID_BYTES
            {
                return Err(OutboundFailure::protocol(
                    "reaction receipt names a resource outside the requested message",
                ));
            }
            Ok(ReactionReceipt {
                reaction_id: receipt.reaction_id,
                already_present: receipt.already_present,
            })
        }
        WireReceipt::Send(_) => Err(OutboundFailure::protocol(
            "reaction response contains a send receipt",
        )),
    }
}

fn encode_wire_request<T: Serialize>(request: &T) -> std::result::Result<Vec<u8>, OutboundFailure> {
    let mut encoded = serde_json::to_vec(request)
        .map_err(|error| OutboundFailure::protocol(format!("cannot encode request: {error}")))?;
    if encoded.len() >= chat_subscription_plugin::MAX_FRAME_BYTES {
        return Err(OutboundFailure {
            code: "request_too_large".to_owned(),
            detail: "outbound request exceeds the 1 MiB wire limit".to_owned(),
            outcome: OutboundOutcome::NotApplied,
            retryable: false,
        });
    }
    encoded.push(b'\n');
    Ok(encoded)
}

fn decode_wire_response(
    payload: &[u8],
    expected_id: &str,
    expected_action: &str,
) -> std::result::Result<WireReceipt, OutboundFailure> {
    if payload.len() > chat_subscription_plugin::MAX_FRAME_BYTES {
        return Err(OutboundFailure::protocol(
            "outbound adapter response exceeds the 1 MiB wire limit",
        ));
    }
    let response: WireResponse = chat_subscription_plugin::decode_strict_json(payload)
        .map_err(|error| OutboundFailure::protocol(error.to_string()))?;
    match response {
        WireResponse::Success(success) => {
            if success.version != 1
                || !success.ok
                || success.id != expected_id
                || success.action != expected_action
            {
                return Err(OutboundFailure::protocol(
                    "outbound success does not match the requested version, UUID, or action",
                ));
            }
            Ok(success.receipt)
        }
        WireResponse::Failure(failure) => {
            if failure.version != 1 || failure.ok {
                return Err(OutboundFailure::protocol(
                    "outbound failure has an invalid version or success flag",
                ));
            }
            let id = failure.id.as_str();
            let action = failure.action.as_str();
            if id != Some(expected_id) || action != Some(expected_action) {
                // Null id/action is valid only when the helper could not parse an input request.
                // This host supplied a valid request and therefore cannot bind such a failure to
                // the operation whose provider outcome is now unknown.
                return Err(OutboundFailure::protocol(
                    "outbound failure does not identify the exact requested UUID and action",
                ));
            }
            validate_failure_code(&failure.error.code)?;
            if failure.error.detail.is_empty() || failure.error.detail.len() > 2_000 {
                return Err(OutboundFailure::protocol(
                    "outbound failure detail is empty or exceeds 2000 bytes",
                ));
            }
            Err(OutboundFailure {
                code: failure.error.code,
                detail: failure.error.detail,
                outcome: failure.error.outcome,
                retryable: failure.error.retryable,
            })
        }
    }
}

fn validate_outbound_send(
    submission: &ReplySubmission<'_>,
) -> std::result::Result<(), OutboundFailure> {
    validate_operation_id(submission.request_id)?;
    validate_resource(submission.channel_id, "channel")?;
    validate_resource(submission.thread_id, "thread")?;
    if submission.body.trim().is_empty() || submission.body.len() > MAX_REPLY_BYTES {
        return Err(not_applied_invalid(
            "reply body is empty or exceeds 30000 bytes",
        ));
    }
    Ok(())
}

fn validate_outbound_reaction(
    submission: &ReactionSubmission<'_>,
) -> std::result::Result<(), OutboundFailure> {
    validate_operation_id(submission.request_id)?;
    validate_resource(submission.channel_id, "channel")?;
    validate_resource(submission.message_id, "message")?;
    if submission.emoji.is_empty() || submission.emoji.len() > 64 {
        return Err(not_applied_invalid("reaction is empty or exceeds 64 bytes"));
    }
    Ok(())
}

fn validate_operation_id(value: &str) -> std::result::Result<(), OutboundFailure> {
    if valid_operation_uuid(value) {
        Ok(())
    } else {
        Err(not_applied_invalid("operation id is not an RFC 4122 UUID"))
    }
}

fn validate_resource(value: &str, label: &str) -> std::result::Result<(), OutboundFailure> {
    if value.is_empty()
        || value.len() > chat_subscription::MAX_RESOURCE_ID_BYTES
        || value.contains(['\0', '\n', '\r'])
    {
        Err(not_applied_invalid(format!(
            "{label} resource is empty, oversized, or multiline"
        )))
    } else {
        Ok(())
    }
}

fn validate_failure_code(value: &str) -> std::result::Result<(), OutboundFailure> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        Err(OutboundFailure::protocol(
            "outbound failure code is not a bounded lowercase slug",
        ))
    } else {
        Ok(())
    }
}

fn not_applied_invalid(detail: impl Into<String>) -> OutboundFailure {
    OutboundFailure {
        code: "invalid_request".to_owned(),
        detail: bounded_detail(&detail.into(), 2_000),
        outcome: OutboundOutcome::NotApplied,
        retryable: false,
    }
}

fn pin_executable_image(
    file: &File,
    cancellation: Option<&OutboundCancellation>,
) -> io::Result<(File, PinnedExecutableIdentity)> {
    let before = file.metadata()?;
    let uid = fs::metadata("/proc/self")?.uid();
    let mode = before.permissions().mode();
    if !before.is_file()
        || before.uid() != uid
        || before.nlink() != 1
        || mode & 0o100 == 0
        || mode & 0o022 != 0
        || before.len() == 0
        || before.len() > MAX_OUTBOUND_EXECUTABLE_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "outbound helper must be a nonempty, single-linked, owner-executable regular file of at most 64 MiB owned by this UID and not group/world writable",
        ));
    }
    let name = CString::new("agentctl-outbound-helper").expect("static memfd name has no NUL");
    // SAFETY: name is a live NUL-terminated string and the flags request a private close-on-exec
    // descriptor whose contents can be sealed after the bounded copy below.
    let descriptor =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful memfd_create returns one fresh descriptor owned by this function.
    let mut image = unsafe { File::from_raw_fd(descriptor) };
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    let mut buffer = [0_u8; 64 * 1_024];
    loop {
        if cancellation.is_some_and(OutboundCancellation::is_cancelled) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "outbound helper identity check was cancelled",
            ));
        }
        let count = file.read_at(&mut buffer, offset)?;
        if count == 0 {
            break;
        }
        image.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        offset = offset.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        if offset > before.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "outbound helper grew while hashing",
            ));
        }
    }
    let after = file.metadata()?;
    if offset != before.len()
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "outbound helper changed while hashing",
        ));
    }
    // The helper image is executable but immutable for the complete service generation. Sealing
    // removes source-file I/O and hashing from every reply/reaction operation while retaining the
    // exact bytes whose identity was validated above.
    // SAFETY: image owns descriptor and 0500 is a valid regular-file mode.
    if unsafe { libc::fchmod(image.as_raw_fd(), 0o500) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let required_seals =
        libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    // SAFETY: F_ADD_SEALS operates on the live private memfd and required_seals is a valid mask.
    if unsafe { libc::fcntl(image.as_raw_fd(), libc::F_ADD_SEALS, required_seals) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_GET_SEALS has no third argument and returns the active mask for a memfd.
    let observed_seals = unsafe { libc::fcntl(image.as_raw_fd(), libc::F_GET_SEALS) };
    if observed_seals < 0 || observed_seals & required_seals != required_seals {
        return Err(if observed_seals < 0 {
            io::Error::last_os_error()
        } else {
            io::Error::other("outbound helper image did not retain all required seals")
        });
    }
    let identity = PinnedExecutableIdentity {
        device: before.dev(),
        inode: before.ino(),
        size: before.len(),
        mode,
        digest: hasher.finalize().into(),
    };
    Ok((image, identity))
}

fn valid_environment_name(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn set_nonblocking(descriptor: libc::c_int) -> io::Result<()> {
    let flags = loop {
        let result = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
        if result >= 0 {
            break result;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    loop {
        if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn poll_descriptor(
    descriptor: libc::c_int,
    events: i16,
    deadline: Instant,
    cancellation: Option<&OutboundCancellation>,
) -> io::Result<()> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "outbound helper I/O exceeded its deadline",
            ));
        }
        let remaining = deadline.saturating_duration_since(now);
        let milliseconds = remaining
            .as_millis()
            .saturating_add(u128::from(remaining.subsec_nanos() > 0))
            .clamp(1, i32::MAX as u128) as i32;
        let mut polls = [
            libc::pollfd {
                fd: descriptor,
                events,
                revents: 0,
            },
            libc::pollfd {
                fd: cancellation.map_or(-1, |value| value.descriptor.as_raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let result = unsafe {
            libc::poll(
                polls.as_mut_ptr(),
                polls.len() as libc::nfds_t,
                milliseconds,
            )
        };
        if result > 0 {
            if polls[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
                || cancellation.is_some_and(OutboundCancellation::is_cancelled)
            {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "outbound helper I/O was cancelled",
                ));
            }
            if polls[0].revents & libc::POLLNVAL != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "outbound helper pipe became invalid",
                ));
            }
            if polls[0].revents & (events | libc::POLLHUP | libc::POLLERR) != 0 {
                return Ok(());
            }
            continue;
        }
        if result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "outbound helper I/O exceeded its deadline",
            ));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn write_nonblocking(
    file: &mut File,
    bytes: &[u8],
    deadline: Instant,
    cancellation: Option<&OutboundCancellation>,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        match file.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "outbound helper accepted zero request bytes",
                ));
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                poll_descriptor(file.as_raw_fd(), libc::POLLOUT, deadline, cancellation)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_nonblocking(
    file: &mut File,
    deadline: Instant,
    cancellation: Option<&OutboundCancellation>,
) -> io::Result<Vec<u8>> {
    let maximum = chat_subscription_plugin::MAX_FRAME_BYTES.saturating_add(1);
    let mut output = Vec::new();
    let mut buffer = [0_u8; 64 * 1_024];
    loop {
        let remaining = maximum.saturating_sub(output.len());
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "outbound helper response exceeds the 1 MiB wire limit",
            ));
        }
        let read_limit = remaining.min(buffer.len());
        match file.read(&mut buffer[..read_limit]) {
            Ok(0) => return Ok(output),
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                poll_descriptor(file.as_raw_fd(), libc::POLLIN, deadline, cancellation)?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn exact_response_line(mut output: Vec<u8>) -> std::result::Result<Vec<u8>, OutboundFailure> {
    if output.pop() != Some(b'\n')
        || output.is_empty()
        || output.contains(&b'\n')
        || output.len() > chat_subscription_plugin::MAX_FRAME_BYTES
    {
        return Err(OutboundFailure::protocol(
            "outbound helper must emit exactly one nonempty newline-terminated JSON object",
        ));
    }
    Ok(output)
}

fn cleanup_detail(prefix: String, cleanup: io::Result<ExitStatus>) -> String {
    match cleanup {
        Ok(_) => prefix,
        Err(error) => format!("{prefix}; process cleanup also failed: {error}"),
    }
}

/// Durable acknowledgement state after one ensure attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AckResult {
    /// Reactions are disabled by operator configuration.
    Disabled,
    /// The reaction is durably reconciled with its provider receipt.
    Acked(ReactionReceipt),
}

/// Newly captured replies plus unavailable fence identifiers for coordinator feedback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyCapture {
    /// Consecutive reply ordinals durably captured during this snapshot.
    pub ordinals: Vec<u32>,
    /// Unavailable marker identifiers that should be reported to the coordinator.
    pub unknown_ids: Vec<String>,
}

/// One retained pane snapshot captured across all active request nonces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotCapture {
    /// Request key and newly durable ordinals for each completed reply sequence.
    pub replies: Vec<(String, Vec<u32>)>,
    /// First-seen unavailable marker identifiers requiring coordinator feedback.
    pub unknown_ids: Vec<String>,
    /// Active and closed nonce index derived by this same bounded state scan.
    pub(crate) route_entries: Vec<ReplyRouteEntry>,
}

/// One exact active fencing route used to arm a line-local terminal subscription.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyRoute {
    /// Durable request key addressed by this fence.
    pub key: String,
    /// Exact next reply identifier expected for the request.
    pub identifier: String,
}

/// One retained nonce route, including closed routes that must remain stale rather than unknown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyRouteEntry {
    /// Durable request key that originally owned the nonce.
    pub key: String,
    /// Stable random nonce shared by every ordinal for this request.
    pub nonce: String,
    /// Exact next identifier while capture is open; absent after explicit closure.
    pub current_identifier: Option<String>,
}

/// One bounded result of durably admitting an upstream acknowledgement unit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchAdmission {
    /// Newly created durable request keys; replayed duplicates are omitted.
    pub new_request_keys: Vec<String>,
    /// Whether any retained gap still requires request/response reconciliation.
    pub reconciliation_required: bool,
    host_batch_sequence: u64,
    provider_sequence: u64,
    delivery_id: String,
    cursor: String,
    event_count: u32,
    batch_fingerprint: String,
}

/// Result of consuming exactly one item from an ordered subscription.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsumedItem {
    /// Provider liveness advanced without a durable cursor change.
    Heartbeat,
    /// One provider batch was admitted and acknowledged.
    Batch(BatchAdmission),
    /// The provider emitted an explicit graceful end.
    End,
}

/// Durable result of reconciling one request with the coordinator's delivery queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoordinatorDeliveryResult {
    /// The native working transition confirmed prompt delivery.
    Delivered,
    /// The request remains known-safe to retry.
    Pending(String),
    /// The prompt may have crossed the terminal injection barrier.
    Uncertain(String),
    /// The request was already durably confirmed.
    AlreadyDelivered,
}

pub(crate) trait CoordinatorDelivery {
    fn message_state(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> std::result::Result<Option<QueueMessageState>, String>;
    fn submit(
        &self,
        agent_name: &str,
        prompt: &str,
        message_id: &str,
        options: DrainOptions,
    ) -> std::result::Result<(), String>;
    fn drain(&self, agent_name: &str, options: DrainOptions) -> std::result::Result<(), String>;
}

impl<A: ManagedApi + ?Sized> CoordinatorDelivery for ManagedAgents<'_, A> {
    fn message_state(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> std::result::Result<Option<QueueMessageState>, String> {
        ManagedAgents::message_state(self, agent_name, message_id)
            .map_err(|error| error.to_string())
    }

    fn submit(
        &self,
        agent_name: &str,
        prompt: &str,
        message_id: &str,
        options: DrainOptions,
    ) -> std::result::Result<(), String> {
        ManagedAgents::send_identified(self, agent_name, prompt, options, Some(message_id))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn drain(&self, agent_name: &str, options: DrainOptions) -> std::result::Result<(), String> {
        ManagedAgents::drain(self, agent_name, options)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Private, restartable durable state for one coordinator bridge.
#[derive(Clone, Debug)]
pub struct BridgeState {
    root: PathBuf,
    config: BridgeConfiguration,
}

/// Revalidate and pin the configured provider executable without launching it.
///
/// Process containment and protocol connection belong to the reviewed plugin process supervisor;
/// this function returns only the immutable launch plan so the runtime cannot fall back to a
/// second supervisor.
pub fn select_plugin(
    inventory: &crate::plugins::PluginInventory,
    config: &BridgeConfiguration,
) -> Result<crate::plugins::PinnedPluginCommand> {
    config.validate()?;
    let plugin = inventory
        .discovered
        .iter()
        .find(|plugin| plugin.name == config.subscription_plugin)
        .ok_or_else(|| {
            ChatRuntimeError::invalid(format!(
                "subscription plugin {:?} is not safely discovered",
                config.subscription_plugin
            ))
        })?;
    if !plugin.capability.starts_with("chat-subscription.") {
        return Err(ChatRuntimeError::invalid(format!(
            "plugin {:?} does not advertise a chat-subscription capability",
            plugin.name
        )));
    }
    inventory
        .launch_command(&config.subscription_plugin)
        .map_err(|refusal| {
            ChatRuntimeError::invalid(format!(
                "subscription plugin refused ({}): {}",
                refusal.code, refusal.detail
            ))
        })
}

impl BridgeState {
    /// Create a new private bridge state directory.
    pub fn initialize(root: &Path, config: BridgeConfiguration) -> Result<Self> {
        config.validate()?;
        let existed = fs::symlink_metadata(root).is_ok();
        agent::create_private_directory(root, "chat state directory", true, true)?;
        if existed && fs::read_dir(root)?.next().is_some() {
            return Err(ChatRuntimeError::invalid(
                "chat state directory is not empty; open existing state instead",
            ));
        }
        for (directory, label) in [
            (root.join("requests"), "chat request directory"),
            (root.join("replies"), "chat reply directory"),
            (
                root.join("commit-receipts"),
                "chat commit receipt directory",
            ),
            (root.join("tombstones"), "chat tombstone directory"),
            (
                root.join("retirements"),
                "chat retirement journal directory",
            ),
        ] {
            agent::create_private_directory(&directory, label, true, true)?;
        }
        let envelope = ConfigurationEnvelope {
            version: STATE_VERSION,
            config: config.clone(),
        };
        write_document(&root.join("bridge.json"), &envelope)?;
        write_document(&root.join("checkpoint.json"), &Checkpoint::empty())?;
        agent::sync_directory(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            config,
        })
    }

    /// Open and recover one existing private bridge state directory.
    pub fn open(root: &Path) -> Result<Self> {
        agent::validate_private_directory(root, "chat state directory", false)?;
        for (directory, label) in [
            (
                root.join("commit-receipts"),
                "chat commit receipt directory",
            ),
            (root.join("tombstones"), "chat tombstone directory"),
            (
                root.join("retirements"),
                "chat retirement journal directory",
            ),
        ] {
            if fs::symlink_metadata(&directory).is_err() {
                agent::create_private_directory(&directory, label, false, true)?;
            }
        }
        let state = Self::inspect(root)?;
        state.complete_retirements()?;
        state.recover_population()?;
        Ok(state)
    }

    /// Open existing state without recovery writes, for read-only inspection.
    pub fn inspect(root: &Path) -> Result<Self> {
        agent::validate_private_directory(root, "chat state directory", false)?;
        agent::validate_private_directory(&root.join("requests"), "chat request directory", false)?;
        agent::validate_private_directory(&root.join("replies"), "chat reply directory", false)?;
        for (directory, label) in [
            (
                root.join("commit-receipts"),
                "chat commit receipt directory",
            ),
            (root.join("tombstones"), "chat tombstone directory"),
            (
                root.join("retirements"),
                "chat retirement journal directory",
            ),
        ] {
            if fs::symlink_metadata(&directory).is_ok() {
                agent::validate_private_directory(&directory, label, false)?;
            }
        }
        let envelope: ConfigurationEnvelope = read_document(&root.join("bridge.json"), 1 << 20)?;
        if envelope.version != STATE_VERSION {
            return Err(ChatRuntimeError::invalid(format!(
                "chat state has unsupported version {}",
                envelope.version
            )));
        }
        envelope.config.validate()?;
        Ok(Self {
            root: root.to_path_buf(),
            config: envelope.config,
        })
    }

    /// Borrow the immutable authority configuration.
    pub fn config(&self) -> &BridgeConfiguration {
        &self.config
    }

    /// Pin and open the configured one-shot outbound helper, when present.
    pub fn outbound_transport(&self) -> Result<Option<CommandOutboundTransport>> {
        self.config.outbound_transport()
    }

    /// Return the last durable inclusive provider replay cursor.
    pub fn cursor(&self) -> Result<Option<String>> {
        Ok(self.read_checkpoint()?.cursor)
    }

    /// Build the next subscription request from durable replay authority.
    pub fn subscribe_request(&self) -> Result<SubscribeRequest> {
        if let Some(gap) = self.gap_diagnostic()? {
            if gap.phase == GapPhase::Unresolved {
                return Err(ChatRuntimeError::UnresolvedGap(format!(
                    "sequence {} delivery {:?} remains unresolved; automatic reconnect is refused",
                    gap.provider_sequence, gap.delivery_id
                )));
            }
        }
        let checkpoint = self.read_checkpoint()?;
        self.config.subscribe_request(checkpoint.cursor.as_deref())
    }

    /// Acquire the generation-wide owner lease. The descriptor must remain live until shutdown.
    pub fn acquire_runner_lease(&self) -> Result<File> {
        let lock = agent::open_private_lock(&self.root.join(".run.lock"), "chat runner lock")?;
        lock.try_lock_exclusive().map_err(|error| {
            ChatRuntimeError::invalid(format!("another chat runtime owns this state: {error}"))
        })?;
        Ok(lock)
    }

    /// Persist every child event and the cursor without acknowledging the provider.
    pub fn admit_batch(&self, batch: &DeliveryBatch) -> Result<BatchAdmission> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut checkpoint = self.read_checkpoint()?;
        let batch_fingerprint = batch_fingerprint(batch)?;
        if checkpoint.cursor.as_deref() == Some(batch.cursor().as_str())
            && checkpoint.host_batch_sequence > 0
        {
            let previous = self.current_commit_receipt(&checkpoint)?.ok_or_else(|| {
                ChatRuntimeError::invalid(
                    "current inclusive cursor is missing its prepared or committed receipt",
                )
            })?;
            if previous.batch_fingerprint != batch_fingerprint {
                return Err(ChatRuntimeError::invalid(
                    "provider reused the current inclusive cursor for different batch content",
                ));
            }
        }
        let mut gap_reasons = batch
            .events()
            .iter()
            .filter_map(|event| match event {
                CommittableEvent::Gap(gap) => Some(
                    gap.reason()
                        .unwrap_or("provider reported an unspecified gap")
                        .to_owned(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !gap_reasons.is_empty() {
            if batch.events().len() != 1 || gap_reasons.len() != 1 {
                gap_reasons.push(
                    "invalid mixed batch: a reconciliation gap must be the only child event"
                        .to_owned(),
                );
            }
            let diagnostic = GapDiagnostic {
                version: STATE_VERSION,
                phase: GapPhase::Unresolved,
                provider_sequence: batch.sequence().get(),
                delivery_id: batch.delivery_id().as_str().to_owned(),
                proposed_cursor: batch.cursor().as_str().to_owned(),
                resume_cursor: checkpoint.cursor.clone(),
                reasons: gap_reasons,
                batch_fingerprint,
                observed_at_millis: unix_millis(),
                resolved_at_millis: None,
                resolved_host_batch_sequence: None,
            };
            diagnostic.validate()?;
            write_document(&self.root.join("gap.json"), &diagnostic)?;
            checkpoint.reconciliation_required = true;
            checkpoint.updated_at_millis = unix_millis();
            write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
            return Err(ChatRuntimeError::UnresolvedGap(format!(
                "sequence {} delivery {:?} was journaled without advancing cursor or acknowledging",
                batch.sequence().get(),
                batch.delivery_id().as_str()
            )));
        }
        let mut candidate_records = Vec::new();
        let mut candidate_messages = BTreeMap::new();
        let mut request_bytes = 0_u64;

        for event in batch.events() {
            match event {
                CommittableEvent::MessageCreated(message) => {
                    let mut record =
                        RequestRecord::from_message(message, self.config.ack_reaction.as_deref())?;
                    record.admitted_cursor = Some(batch.cursor().as_str().to_owned());
                    if let Some(existing) = candidate_messages.get(&record.key) {
                        if existing != &record.message {
                            return Err(ChatRuntimeError::invalid(
                                "one batch reused a request key for different message content",
                            ));
                        }
                        continue;
                    }
                    let path = self.request_path(&record.key);
                    if fs::symlink_metadata(&path).is_ok() {
                        let saved: RequestRecord = read_document(&path, MAX_REQUEST_RECORD_BYTES)?;
                        saved.validate(&record.key)?;
                        if saved.message != record.message {
                            return Err(ChatRuntimeError::invalid(
                                "an existing request key names different message content",
                            ));
                        }
                        if saved.admitted_cursor.as_deref() == Some(batch.cursor().as_str())
                            && checkpoint.cursor.as_deref() != Some(batch.cursor().as_str())
                        {
                            return Err(ChatRuntimeError::invalid(
                                "provider cursor regressed to an older active request boundary",
                            ));
                        }
                        continue;
                    }
                    if let Some(retired) = self.retired_key(&record.key)? {
                        let fingerprint = saved_message_fingerprint(&record.message)?;
                        if retired.message_fingerprint != fingerprint {
                            return Err(ChatRuntimeError::invalid(
                                "a retired request key names different message content",
                            ));
                        }
                        if retired.admitted_cursor == batch.cursor().as_str()
                            && checkpoint.cursor.as_deref() != Some(batch.cursor().as_str())
                        {
                            return Err(ChatRuntimeError::invalid(
                                "provider cursor regressed to a retired request boundary",
                            ));
                        }
                        continue;
                    }
                    let encoded_bytes_usize = encoded_document_bytes(&record)?;
                    let encoded_bytes = u64::try_from(encoded_bytes_usize).map_err(|_| {
                        ChatRuntimeError::invalid("request record length does not fit u64")
                    })?;
                    if encoded_bytes_usize > MAX_REQUEST_RECORD_BYTES {
                        return Err(ChatRuntimeError::invalid(format!(
                            "request record exceeds {MAX_REQUEST_RECORD_BYTES} bytes"
                        )));
                    }
                    request_bytes = request_bytes.saturating_add(encoded_bytes);
                    candidate_messages.insert(record.key.clone(), record.message.clone());
                    candidate_records.push(record);
                }
                CommittableEvent::Checkpoint => {}
                CommittableEvent::Gap(_) => unreachable!("gaps were rejected before admission"),
            }
        }

        self.retire_for_capacity_locked(
            &mut checkpoint,
            u64::try_from(candidate_records.len()).unwrap_or(u64::MAX),
            request_bytes,
        )?;
        let next_count = checkpoint
            .request_count
            .checked_add(u64::try_from(candidate_records.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| ChatRuntimeError::invalid("request count overflow"))?;
        let next_bytes = checkpoint
            .request_bytes
            .checked_add(request_bytes)
            .ok_or_else(|| ChatRuntimeError::invalid("request byte count overflow"))?;
        if next_count > MAX_REQUESTS || next_bytes > MAX_REQUEST_BYTES {
            return Err(ChatRuntimeError::invalid(
                "chat request population limit reached; no safely retired request can free enough capacity",
            ));
        }
        let new_records = candidate_records
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        for record in &candidate_records {
            write_document(&self.request_path(&record.key), record)?;
        }

        if !new_records.is_empty() {
            agent::sync_directory(&self.root.join("requests"))?;
        }
        let host_batch_sequence = checkpoint
            .host_batch_sequence
            .checked_add(1)
            .ok_or_else(|| ChatRuntimeError::invalid("chat host batch sequence is exhausted"))?;
        let event_count = u32::try_from(batch.events().len())
            .map_err(|_| ChatRuntimeError::invalid("batch event count does not fit u32"))?;
        let receipt = CommitReceipt {
            version: STATE_VERSION,
            phase: CommitReceiptPhase::Prepared,
            host_batch_sequence,
            provider_sequence: batch.sequence().get(),
            delivery_id: batch.delivery_id().as_str().to_owned(),
            cursor: batch.cursor().as_str().to_owned(),
            event_count,
            batch_fingerprint: batch_fingerprint.clone(),
            prepared_at_millis: unix_millis(),
            committed_at_millis: None,
        };
        receipt.validate()?;
        write_document(&self.commit_receipt_path(host_batch_sequence), &receipt)?;
        checkpoint.cursor = Some(batch.cursor().as_str().to_owned());
        checkpoint.request_count = checkpoint
            .request_count
            .saturating_add(u64::try_from(new_records.len()).unwrap_or(u64::MAX));
        checkpoint.request_bytes = checkpoint.request_bytes.saturating_add(request_bytes);
        checkpoint.host_batch_sequence = host_batch_sequence;
        checkpoint.updated_at_millis = unix_millis();
        validate_checkpoint(&checkpoint)?;
        write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
        Ok(BatchAdmission {
            new_request_keys: new_records,
            reconciliation_required: checkpoint.reconciliation_required,
            host_batch_sequence,
            provider_sequence: batch.sequence().get(),
            delivery_id: batch.delivery_id().as_str().to_owned(),
            cursor: batch.cursor().as_str().to_owned(),
            event_count,
            batch_fingerprint,
        })
    }

    fn confirm_batch_commit(&self, admission: &BatchAdmission) -> Result<()> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut checkpoint = self.read_checkpoint()?;
        if checkpoint.host_batch_sequence != admission.host_batch_sequence
            || checkpoint.cursor.as_deref() != Some(admission.cursor.as_str())
        {
            return Err(ChatRuntimeError::invalid(
                "durable checkpoint changed before provider commit confirmation",
            ));
        }
        let path = self.commit_receipt_path(admission.host_batch_sequence);
        let mut receipt: CommitReceipt = read_document(&path, MAX_COMMIT_RECEIPT_BYTES)?;
        receipt.validate()?;
        if receipt.phase != CommitReceiptPhase::Prepared
            || receipt.host_batch_sequence != admission.host_batch_sequence
            || receipt.provider_sequence != admission.provider_sequence
            || receipt.delivery_id != admission.delivery_id
            || receipt.cursor != admission.cursor
            || receipt.event_count != admission.event_count
            || receipt.batch_fingerprint != admission.batch_fingerprint
        {
            return Err(ChatRuntimeError::invalid(
                "prepared commit receipt does not match the exactly committed batch",
            ));
        }
        receipt.phase = CommitReceiptPhase::Committed;
        receipt.committed_at_millis = Some(unix_millis());
        receipt.validate()?;
        write_document(&path, &receipt)?;
        checkpoint.committed_batches = checkpoint.committed_batches.saturating_add(1);
        checkpoint.updated_at_millis = unix_millis();
        write_document(&self.root.join("checkpoint.json"), &checkpoint)
    }

    /// Read bounded local status without contacting a provider or coordinator.
    pub fn status(&self) -> Result<Value> {
        let checkpoint = self.read_checkpoint()?;
        let gap = self.gap_diagnostic()?;
        let unresolved_gap = gap
            .as_ref()
            .is_some_and(|diagnostic| diagnostic.phase == GapPhase::Unresolved);
        let commit_receipt = self.current_commit_receipt(&checkpoint)?;
        let latest_commit_confirmed = commit_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.phase == CommitReceiptPhase::Committed);
        let mut phases = Map::new();
        phases.insert("pending".to_owned(), Value::from(0));
        phases.insert("submitting".to_owned(), Value::from(0));
        phases.insert("delivered".to_owned(), Value::from(0));
        phases.insert("delivery_uncertain".to_owned(), Value::from(0));
        let mut acknowledgements = Map::new();
        acknowledgements.insert("disabled".to_owned(), Value::from(0));
        acknowledgements.insert("pending".to_owned(), Value::from(0));
        acknowledgements.insert("sending".to_owned(), Value::from(0));
        acknowledgements.insert("acked".to_owned(), Value::from(0));
        for (record, _) in self.request_records()? {
            let key = match record.phase {
                RequestPhase::Pending => "pending",
                RequestPhase::Submitting => "submitting",
                RequestPhase::Delivered => "delivered",
                RequestPhase::DeliveryUncertain => "delivery_uncertain",
            };
            let count = phases.get(key).and_then(Value::as_u64).unwrap_or(0);
            phases.insert(key.to_owned(), Value::from(count.saturating_add(1)));
            let ack_key = match record.ack_phase {
                AckPhase::Disabled => "disabled",
                AckPhase::Pending => "pending",
                AckPhase::Sending => "sending",
                AckPhase::Acked => "acked",
            };
            let ack_count = acknowledgements
                .get(ack_key)
                .and_then(Value::as_u64)
                .unwrap_or(0);
            acknowledgements.insert(ack_key.to_owned(), Value::from(ack_count.saturating_add(1)));
        }
        Ok(serde_json::json!({
            "subscription_plugin": self.config.subscription_plugin,
            "backend_configuration_schema": self.config.backend_configuration.as_ref().map(|value| &value.schema),
            "agent_name": self.config.agent_name,
            "agent_label": self.config.agent_label,
            "channel_ids": self.config.channel_ids,
            "outbound_enabled": self.config.outbound_enabled,
            "outbound_command_configured": self.config.outbound_command.is_some(),
            "cursor_present": checkpoint.cursor.is_some(),
            "request_count": checkpoint.request_count,
            "request_bytes": checkpoint.request_bytes,
            "committed_batches": checkpoint.committed_batches,
            "runtime_evidence": {
                "configuration_initialized": true,
                "connected_now": if unresolved_gap { Value::Bool(false) } else { Value::Null },
                "healthy": !unresolved_gap,
                "durable_batch_verified": latest_commit_confirmed && !unresolved_gap,
                "latest_commit_receipt": commit_receipt,
                "basis": if unresolved_gap {
                    "the provider declared a gap; no commit was sent and automatic reconnect is refused"
                } else {
                    "provider-free durable status; use the service manager for current process liveness"
                },
            },
            "reconciliation_required": checkpoint.reconciliation_required,
            "unresolved_gap": gap,
            "reply_count": checkpoint.reply_count,
            "reply_bytes": checkpoint.reply_bytes,
            "retired_route_count": checkpoint.retired_route_count,
            "retirement_sequence": checkpoint.retirement_sequence,
            "phases": phases,
            "acknowledgements": acknowledgements,
        }))
    }

    /// Render one generic provider-independent coordinator prompt and reply protocol.
    pub fn prompt(&self, key: &str) -> Result<String> {
        let record = self.read_request(key)?;
        if !self.config.outbound_enabled {
            return Ok(format!(
                "The user's request arrived through your configured inbound-only chat bridge.\n\
Source: {}\n\
Sender: {}\n\n\
{}\n\n\
Complete this request using your normal instructions and tools. Outbound chat is disabled for this bridge; do not emit CHAT_REPLY fences.",
                record.message.message_id, record.message.sender_id, record.message.text,
            ));
        }
        let reply_id = format!("{}_{}", record.reply_nonce, record.next_reply_ordinal);
        Ok(format!(
            "The user's request arrived through the configured chat bridge.\n\
Source: {}\n\
Sender: {}\n\n\
{}\n\n\
Complete this request using your normal instructions and tools. You may send one or multiple replies, including progress updates. \
Your next reply ID is `{reply_id}`. Compose an opening line from the literal prefix `<CHAT_REPLY_`, that ID, and `>`; \
compose its closing line from `</CHAT_REPLY_`, the same ID, and `>`. Keep both lines standalone and outside code fences. \
Increment the numeric suffix for every later reply. Each consecutive complete block is sent as a separate chat message.",
            record.message.message_id, record.message.sender_id, record.message.text,
        ))
    }

    /// Read pending request keys during startup recovery or an explicit delivery pass.
    pub fn pending_request_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .request_records()?
            .into_iter()
            .filter_map(|(record, _)| {
                matches!(
                    record.phase,
                    RequestPhase::Pending | RequestPhase::Submitting
                )
                .then_some(record.key)
            })
            .collect())
    }

    /// Return every retained request key in deterministic order.
    pub fn request_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .request_records()?
            .into_iter()
            .map(|(record, _)| record.key)
            .collect())
    }

    /// Return requests whose durable reaction operation awaits reconciliation.
    pub fn pending_ack_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .request_records()?
            .into_iter()
            .filter_map(|(record, _)| {
                matches!(record.ack_phase, AckPhase::Pending | AckPhase::Sending)
                    .then_some(record.key)
            })
            .collect())
    }

    /// Return requests with at least one captured reply not durably sent yet.
    pub fn pending_reply_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .request_records()?
            .into_iter()
            .filter_map(|(record, _)| {
                (record.next_send_ordinal < record.next_reply_ordinal).then_some(record.key)
            })
            .collect())
    }

    /// Return the deterministic union of delivery, acknowledgement, and reply work in one scan.
    pub fn pending_work_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .request_records()?
            .into_iter()
            .filter_map(|(record, _)| {
                (matches!(
                    record.phase,
                    RequestPhase::Pending | RequestPhase::Submitting
                ) || matches!(record.ack_phase, AckPhase::Pending | AckPhase::Sending)
                    || record.next_send_ordinal < record.next_reply_ordinal)
                    .then_some(record.key)
            })
            .collect())
    }

    /// Capture complete blocks for every request through one bounded pane-snapshot parse.
    pub fn capture_snapshot(&self, rendered: &str) -> Result<SnapshotCapture> {
        if !self.config.outbound_enabled {
            return Ok(SnapshotCapture {
                replies: Vec::new(),
                unknown_ids: Vec::new(),
                route_entries: Vec::new(),
            });
        }
        let records = self.request_records()?;
        let retired_routes = self.retired_routes()?;
        let mut known_nonces = records
            .iter()
            .map(|(record, _)| record.reply_nonce.clone())
            .collect::<BTreeSet<_>>();
        known_nonces.extend(
            retired_routes
                .iter()
                .map(|retired| retired.reply_nonce.clone()),
        );
        let nonce_to_key = records
            .iter()
            .filter(|(record, _)| !record.reply_closed)
            .map(|(record, _)| (record.reply_nonce.clone(), record.key.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut next_by_nonce = records
            .iter()
            .filter(|(record, _)| !record.reply_closed)
            .map(|(record, _)| (record.reply_nonce.clone(), record.next_reply_ordinal))
            .collect::<BTreeMap<_, _>>();
        let mut scan = scan_reply_blocks_for_nonces(rendered, &known_nonces)?;
        let observed_ids = std::mem::take(&mut scan.observed_ids);
        let mut replies = Vec::new();
        let mut unknown_ids = std::mem::take(&mut scan.unknown_ids);
        for (nonce, blocks) in scan.blocks_by_nonce {
            let Some(key) = nonce_to_key.get(&nonce) else {
                // Closed retained requests stay recognized so a stale terminal marker is a no-op.
                continue;
            };
            let capture = self.capture_scanned_replies(
                key,
                ReplyScan {
                    blocks,
                    unknown_ids: Vec::new(),
                },
            )?;
            if let Some(last) = capture.ordinals.last() {
                next_by_nonce.insert(nonce, last.saturating_add(1));
            }
            if !capture.ordinals.is_empty() {
                replies.push((key.clone(), capture.ordinals));
            }
            for identifier in capture.unknown_ids {
                if unknown_ids.len() < 128 && !unknown_ids.contains(&identifier) {
                    unknown_ids.push(identifier);
                }
            }
        }
        for identifier in observed_ids {
            let unavailable = identifier
                .rsplit_once('_')
                .and_then(|(nonce, _)| {
                    next_by_nonce.get(nonce).and_then(|next| {
                        sequenced_ordinal(&identifier, nonce).map(|ordinal| ordinal >= *next)
                    })
                })
                .unwrap_or(false);
            if unavailable && unknown_ids.len() < 128 && !unknown_ids.contains(&identifier) {
                unknown_ids.push(identifier);
            }
        }
        let mut route_entries = records
            .iter()
            .map(|(record, _)| ReplyRouteEntry {
                key: record.key.clone(),
                nonce: record.reply_nonce.clone(),
                current_identifier: (!record.reply_closed).then(|| {
                    format!(
                        "{}_{}",
                        record.reply_nonce,
                        next_by_nonce
                            .get(&record.reply_nonce)
                            .copied()
                            .unwrap_or(record.next_reply_ordinal)
                    )
                }),
            })
            .collect::<Vec<_>>();
        route_entries.extend(retired_routes.into_iter().map(|retired| ReplyRouteEntry {
            key: retired.request_key,
            nonce: retired.reply_nonce,
            current_identifier: None,
        }));
        Ok(SnapshotCapture {
            replies,
            unknown_ids,
            route_entries,
        })
    }

    /// Return exact currently available reply IDs for coordinator diagnostics and subscriptions.
    pub fn available_reply_ids(&self) -> Result<Vec<String>> {
        if !self.config.outbound_enabled {
            return Ok(Vec::new());
        }
        Ok(self
            .request_records()?
            .into_iter()
            .filter(|(record, _)| !record.reply_closed)
            .map(|(record, _)| format!("{}_{}", record.reply_nonce, record.next_reply_ordinal))
            .collect())
    }

    /// Build the bounded active route cache during startup or explicit recovery.
    pub fn active_reply_routes(&self) -> Result<Vec<ReplyRoute>> {
        if !self.config.outbound_enabled {
            return Ok(Vec::new());
        }
        Ok(self
            .request_records()?
            .into_iter()
            .filter(|(record, _)| !record.reply_closed)
            .map(|(record, _)| ReplyRoute {
                key: record.key,
                identifier: format!("{}_{}", record.reply_nonce, record.next_reply_ordinal),
            })
            .collect())
    }

    /// Build the bounded active-and-closed nonce index during startup or explicit recovery.
    pub fn reply_route_entries(&self) -> Result<Vec<ReplyRouteEntry>> {
        if !self.config.outbound_enabled {
            return Ok(Vec::new());
        }
        let mut entries = self
            .request_records()?
            .into_iter()
            .map(|(record, _)| ReplyRouteEntry {
                key: record.key,
                current_identifier: (!record.reply_closed)
                    .then(|| format!("{}_{}", record.reply_nonce, record.next_reply_ordinal)),
                nonce: record.reply_nonce,
            })
            .collect::<Vec<_>>();
        entries.extend(
            self.retired_routes()?
                .into_iter()
                .map(|retired| ReplyRouteEntry {
                    key: retired.request_key,
                    nonce: retired.reply_nonce,
                    current_identifier: None,
                }),
        );
        Ok(entries)
    }

    /// Read one request directly and return its current active route, if capture remains open.
    pub fn next_reply_route(&self, key: &str) -> Result<Option<ReplyRoute>> {
        if !self.config.outbound_enabled {
            return Ok(None);
        }
        let record = match self.read_request(key) {
            Ok(record) => record,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                if self.retired_key(key)?.is_some() {
                    return Ok(None);
                }
                return Err(ChatRuntimeError::Io(error));
            }
            Err(error) => return Err(error),
        };
        Ok((!record.reply_closed).then(|| ReplyRoute {
            key: record.key,
            identifier: format!("{}_{}", record.reply_nonce, record.next_reply_ordinal),
        }))
    }

    /// Stop capture and retire the request once every durable terminal condition is satisfied.
    pub fn close_replies(&self, key: &str) -> Result<()> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut request = match self.read_request(key) {
            Ok(request) => request,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                if self.retired_key(key)?.is_some() {
                    return Ok(());
                }
                return Err(ChatRuntimeError::Io(error));
            }
            Err(error) => return Err(error),
        };
        let mut checkpoint = self.read_checkpoint()?;
        if !request.reply_closed {
            request.reply_closed = true;
            self.write_request_accounted(&request, &mut checkpoint)?;
        }
        let _ = self.retire_if_eligible_locked(&mut checkpoint, key)?;
        checkpoint.updated_at_millis = unix_millis();
        write_document(&self.root.join("checkpoint.json"), &checkpoint)
    }

    fn retire_for_capacity_locked(
        &self,
        checkpoint: &mut Checkpoint,
        additional_count: u64,
        additional_bytes: u64,
    ) -> Result<()> {
        if checkpoint.request_count.saturating_add(additional_count) <= MAX_REQUESTS
            && checkpoint.request_bytes.saturating_add(additional_bytes) <= MAX_REQUEST_BYTES
        {
            return Ok(());
        }
        let keys = self
            .request_records()?
            .into_iter()
            .map(|(request, _)| request.key)
            .collect::<Vec<_>>();
        for key in keys {
            let _ = self.retire_if_eligible_locked(checkpoint, &key)?;
            if checkpoint.request_count.saturating_add(additional_count) <= MAX_REQUESTS
                && checkpoint.request_bytes.saturating_add(additional_bytes) <= MAX_REQUEST_BYTES
            {
                return Ok(());
            }
        }
        Ok(())
    }

    fn retire_if_eligible_locked(&self, checkpoint: &mut Checkpoint, key: &str) -> Result<bool> {
        let (request, request_bytes): (RequestRecord, u64) =
            read_document_sized(&self.request_path(key), MAX_REQUEST_RECORD_BYTES)?;
        self.validate_request_record(&request, key)?;
        let Some(admitted_cursor) = request.admitted_cursor.as_deref() else {
            return Ok(false);
        };
        if request.phase != RequestPhase::Delivered
            || !matches!(request.ack_phase, AckPhase::Disabled | AckPhase::Acked)
            || !request.reply_closed
            || request.next_send_ordinal != request.next_reply_ordinal
            || checkpoint.cursor.as_deref() == Some(admitted_cursor)
        {
            return Ok(false);
        }
        let mut replies =
            Vec::with_capacity(usize::try_from(request.reply_count).unwrap_or(usize::MAX));
        for ordinal in 1..=request.reply_count {
            let reply = self.read_reply(key, ordinal)?;
            if reply.phase != ReplyPhase::Sent {
                return Ok(false);
            }
            replies.push(RetiredReplyReceipt {
                ordinal,
                send_request_id: reply.send_request_id,
                provider_message_id: reply
                    .provider_message_id
                    .expect("validated sent reply has a provider message id"),
            });
        }
        let retirement_sequence = checkpoint
            .retirement_sequence
            .checked_add(1)
            .ok_or_else(|| ChatRuntimeError::invalid("retirement sequence is exhausted"))?;
        let route_path = self.retired_route_path(retirement_sequence);
        let evicted =
            match read_document::<RetiredRouteIndex>(&route_path, MAX_REQUEST_RECORD_BYTES) {
                Ok(route) => {
                    route.validate()?;
                    Some(route)
                }
                Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
        let prepared_at_millis = unix_millis();
        let mut retirement = RetirementRecord {
            version: STATE_VERSION,
            phase: RetirementPhase::Prepared,
            retirement_sequence,
            request_key: key.to_owned(),
            message_fingerprint: saved_message_fingerprint(&request.message)?,
            reply_nonce: request.reply_nonce.clone(),
            admitted_cursor: admitted_cursor.to_owned(),
            request_bytes,
            reply_bytes: request.reply_bytes,
            reply_count: request.reply_count,
            delivery_message_id: request.delivery_message_id.clone(),
            ack_reaction: request.ack_reaction.clone(),
            ack_request_id: request.ack_request_id.clone(),
            reaction_id: request.reaction_id.clone(),
            reaction_already_present: request.reaction_already_present,
            replies,
            evicted_request_key: evicted
                .as_ref()
                .filter(|route| route.request_key != key)
                .map(|route| route.request_key.clone()),
            prepared_at_millis,
            retired_at_millis: None,
        };
        retirement.validate()?;
        if encoded_document_bytes(&retirement)? > MAX_RETIREMENT_RECORD_BYTES {
            return Err(ChatRuntimeError::invalid(
                "retirement audit record exceeds its bounded artifact size",
            ));
        }
        let retirement_path = self.retirement_path(retirement_sequence);
        write_document(&retirement_path, &retirement)?;
        let index = RetiredRouteIndex {
            version: STATE_VERSION,
            retirement_sequence,
            request_key: key.to_owned(),
            message_fingerprint: retirement.message_fingerprint.clone(),
            reply_nonce: request.reply_nonce,
            admitted_cursor: admitted_cursor.to_owned(),
            retired_at_millis: prepared_at_millis,
        };
        index.validate()?;
        write_document(&self.retired_key_path(key), &index)?;
        write_document(&route_path, &index)?;
        if let Some(evicted) = evicted.filter(|route| route.request_key != key) {
            remove_if_exists(&self.retired_key_path(&evicted.request_key))?;
        }
        for ordinal in 1..=request.reply_count {
            remove_if_exists(&self.reply_path(key, ordinal))?;
        }
        remove_if_exists(&self.request_path(key))?;
        agent::sync_directory(&self.root.join("replies"))?;
        agent::sync_directory(&self.root.join("requests"))?;
        checkpoint.request_count = checkpoint.request_count.checked_sub(1).ok_or_else(|| {
            ChatRuntimeError::invalid("request count underflow during retirement")
        })?;
        checkpoint.request_bytes = checkpoint
            .request_bytes
            .checked_sub(request_bytes)
            .ok_or_else(|| ChatRuntimeError::invalid("request byte underflow during retirement"))?;
        checkpoint.reply_count = checkpoint
            .reply_count
            .checked_sub(u64::from(request.reply_count))
            .ok_or_else(|| ChatRuntimeError::invalid("reply count underflow during retirement"))?;
        checkpoint.reply_bytes = checkpoint
            .reply_bytes
            .checked_sub(request.reply_bytes)
            .ok_or_else(|| ChatRuntimeError::invalid("reply byte underflow during retirement"))?;
        checkpoint.retirement_sequence = retirement_sequence;
        checkpoint.retired_route_count = checkpoint
            .retired_route_count
            .saturating_add(1)
            .min(RETIRED_ROUTE_SLOTS);
        checkpoint.updated_at_millis = unix_millis();
        write_document(&self.root.join("checkpoint.json"), checkpoint)?;
        retirement.phase = RetirementPhase::Retired;
        retirement.retired_at_millis = Some(unix_millis());
        retirement.validate()?;
        write_document(&retirement_path, &retirement)?;
        Ok(true)
    }

    /// Ensure the configured durable-intake reaction is present for one request.
    ///
    /// The operation UUID is persisted with the request before the transport call. A crash or
    /// unknown transport outcome leaves `sending`; the next attempt must reconcile or repeat the
    /// identical ensure operation rather than create a second logical reaction.
    pub fn ensure_ack(
        &self,
        key: &str,
        transport: &mut dyn ReactionTransport,
    ) -> Result<AckResult> {
        let request_snapshot = {
            let state_lock =
                agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
            state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
            let mut request = self.read_request(key)?;
            match request.ack_phase {
                AckPhase::Disabled => return Ok(AckResult::Disabled),
                AckPhase::Acked => {
                    return Ok(AckResult::Acked(ReactionReceipt {
                        reaction_id: request
                            .reaction_id
                            .expect("validated acknowledged request has receipt"),
                        already_present: request
                            .reaction_already_present
                            .expect("validated acknowledged request has reconciliation flag"),
                    }));
                }
                AckPhase::Pending | AckPhase::Sending => {}
            }
            request.ack_phase = AckPhase::Sending;
            request.ack_error = None;
            let mut checkpoint = self.read_checkpoint()?;
            self.write_request_accounted(&request, &mut checkpoint)?;
            checkpoint.updated_at_millis = unix_millis();
            write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
            request
        };

        let receipt = match transport.ensure_reaction(ReactionSubmission {
            channel_id: &request_snapshot.message.channel_id,
            message_id: &request_snapshot.message.message_id,
            emoji: request_snapshot
                .ack_reaction
                .as_deref()
                .expect("validated enabled acknowledgement has reaction"),
            request_id: request_snapshot
                .ack_request_id
                .as_deref()
                .expect("validated enabled acknowledgement has operation id"),
        }) {
            Ok(receipt) => receipt,
            Err(error) => {
                let state_lock =
                    agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
                state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
                let mut request = self.read_request(key)?;
                request.ack_phase = AckPhase::Sending;
                request.ack_error = Some(bounded_detail(&error.to_string(), 2_000));
                let mut checkpoint = self.read_checkpoint()?;
                self.write_request_accounted(&request, &mut checkpoint)?;
                checkpoint.updated_at_millis = unix_millis();
                write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
                return Err(ChatRuntimeError::invalid(format!(
                    "outbound reaction ensure failed: {error}"
                )));
            }
        };
        validate_single_line(
            &receipt.reaction_id,
            "provider reaction id",
            chat_subscription::MAX_RESOURCE_ID_BYTES,
        )?;

        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut request = self.read_request(key)?;
        if request.ack_request_id != request_snapshot.ack_request_id {
            return Err(ChatRuntimeError::invalid(
                "acknowledgement operation identity changed during transport call",
            ));
        }
        request.ack_phase = AckPhase::Acked;
        request.reaction_id = Some(receipt.reaction_id.clone());
        request.reaction_already_present = Some(receipt.already_present);
        request.ack_error = None;
        let mut checkpoint = self.read_checkpoint()?;
        self.write_request_accounted(&request, &mut checkpoint)?;
        let _ = self.retire_if_eligible_locked(&mut checkpoint, key)?;
        checkpoint.updated_at_millis = unix_millis();
        write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
        Ok(AckResult::Acked(receipt))
    }

    /// Capture every complete, consecutive reply block for one request from one retained snapshot.
    /// No directory scan occurs: only the request and its next direct reply paths are touched.
    pub fn capture_replies(&self, key: &str, rendered: &str) -> Result<ReplyCapture> {
        let request = self.read_request(key)?;
        if request.reply_closed {
            return Ok(ReplyCapture {
                ordinals: Vec::new(),
                unknown_ids: Vec::new(),
            });
        }
        let scan = scan_reply_blocks(rendered, &request.reply_nonce)?;
        self.capture_scanned_replies(key, scan)
    }

    fn capture_scanned_replies(&self, key: &str, mut scan: ReplyScan) -> Result<ReplyCapture> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut request = self.read_request(key)?;
        if request.reply_closed {
            return Ok(ReplyCapture {
                ordinals: Vec::new(),
                unknown_ids: Vec::new(),
            });
        }
        let mut expected = request.next_reply_ordinal;
        let mut captured = Vec::new();
        let mut checkpoint = self.read_checkpoint()?;

        loop {
            let mut matching = scan.blocks.iter().filter(|block| block.ordinal == expected);
            let Some(block) = matching.next() else {
                break;
            };
            if matching.next().is_some() {
                return Err(ChatRuntimeError::invalid(format!(
                    "reply ordinal {expected} appears more than once in one capture"
                )));
            }
            if request.reply_count >= MAX_REQUEST_REPLIES {
                return Err(ChatRuntimeError::invalid(
                    "request reply count limit reached",
                ));
            }
            validate_outbound_body(&self.config.agent_label, &block.body)?;
            let reply = ReplyRecord::new(key, expected, block.body.clone())?;
            let path = self.reply_path(key, expected);
            let (bytes, exists) = if fs::symlink_metadata(&path).is_ok() {
                let (saved, _actual_bytes): (ReplyRecord, u64) =
                    read_document_sized(&path, MAX_REPLY_RECORD_BYTES)?;
                saved.validate(key, expected)?;
                if saved.body != reply.body {
                    return Err(ChatRuntimeError::invalid(
                        "existing reply ordinal contains different content",
                    ));
                }
                (saved.reserved_bytes, true)
            } else {
                let encoded_bytes = encoded_document_bytes(&reply)?;
                if encoded_bytes > MAX_REPLY_RECORD_BYTES {
                    return Err(ChatRuntimeError::invalid(format!(
                        "reply record exceeds {MAX_REPLY_RECORD_BYTES} bytes"
                    )));
                }
                (reply.reserved_bytes, false)
            };
            let next_request_bytes = request.reply_bytes.saturating_add(bytes);
            let next_state_count = checkpoint.reply_count.saturating_add(1);
            let next_state_bytes = checkpoint.reply_bytes.saturating_add(bytes);
            if request.reply_count >= MAX_REQUEST_REPLIES
                || next_request_bytes > MAX_REQUEST_REPLY_BYTES
                || next_state_count > MAX_STATE_REPLIES
                || next_state_bytes > MAX_STATE_REPLY_BYTES
            {
                return Err(ChatRuntimeError::invalid(
                    "chat reply population limit reached",
                ));
            }
            if !exists {
                write_document(&path, &reply)?;
                agent::sync_directory(&self.root.join("replies"))?;
            }
            request.reply_count = request.reply_count.saturating_add(1);
            request.reply_bytes = next_request_bytes;
            checkpoint.reply_count = next_state_count;
            checkpoint.reply_bytes = next_state_bytes;
            captured.push(expected);
            expected = expected.saturating_add(1);
            if expected > MAX_REPLY_ORDINAL.saturating_add(1) {
                return Err(ChatRuntimeError::invalid(
                    "reply ordinal space is exhausted",
                ));
            }
        }

        if !captured.is_empty() {
            validate_checkpoint(&checkpoint)?;
            request.next_reply_ordinal = expected;
            request.validate(key)?;
            self.write_request_accounted(&request, &mut checkpoint)?;
            checkpoint.updated_at_millis = unix_millis();
            write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
        }
        for block in &scan.blocks {
            if block.ordinal >= expected {
                let identifier = format!("{}_{}", request.reply_nonce, block.ordinal);
                if scan.unknown_ids.len() < 128 && !scan.unknown_ids.contains(&identifier) {
                    scan.unknown_ids.push(identifier);
                }
            }
        }
        Ok(ReplyCapture {
            ordinals: captured,
            unknown_ids: scan.unknown_ids,
        })
    }

    /// Publish exactly one retained reply in ordinal order.
    ///
    /// A `sending` artifact is retried with the same provider request ID after a crash. The
    /// transport contract must make that request ID idempotent or reconcile it before returning.
    pub fn publish_one(
        &self,
        key: &str,
        transport: &mut dyn ReplyTransport,
    ) -> Result<Option<String>> {
        let (request_snapshot, mut reply) = {
            let state_lock =
                agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
            state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
            let mut request = self.read_request(key)?;
            let original_send_ordinal = request.next_send_ordinal;
            while request.next_send_ordinal < request.next_reply_ordinal {
                let mut reply = self.read_reply(key, request.next_send_ordinal)?;
                if reply.phase != ReplyPhase::Sent {
                    reply.phase = ReplyPhase::Sending;
                    write_document(&self.reply_path(key, reply.ordinal), &reply)?;
                    break;
                }
                request.next_send_ordinal = request.next_send_ordinal.saturating_add(1);
            }
            if request.next_send_ordinal != original_send_ordinal {
                let mut checkpoint = self.read_checkpoint()?;
                self.write_request_accounted(&request, &mut checkpoint)?;
                checkpoint.updated_at_millis = unix_millis();
                write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
            }
            if request.next_send_ordinal >= request.next_reply_ordinal {
                return Ok(None);
            }
            let reply = self.read_reply(key, request.next_send_ordinal)?;
            (request, reply)
        };

        let provider_message_id = transport
            .send(ReplySubmission {
                channel_id: &request_snapshot.message.channel_id,
                thread_id: &request_snapshot.message.thread_id,
                body: &outbound_text(&self.config.agent_label, &reply.body),
                request_id: &reply.send_request_id,
            })
            .map_err(|error| {
                ChatRuntimeError::invalid(format!("outbound chat send failed: {error}"))
            })?;
        validate_single_line(
            &provider_message_id,
            "provider reply message id",
            chat_subscription::MAX_RESOURCE_ID_BYTES,
        )?;

        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut current = self.read_reply(key, reply.ordinal)?;
        if current.body != reply.body || current.send_request_id != reply.send_request_id {
            return Err(ChatRuntimeError::invalid(
                "reply artifact changed during outbound send",
            ));
        }
        current.phase = ReplyPhase::Sent;
        current.provider_message_id = Some(provider_message_id.clone());
        write_document(&self.reply_path(key, current.ordinal), &current)?;
        reply = current;
        let mut request = self.read_request(key)?;
        if request.next_send_ordinal == reply.ordinal {
            request.next_send_ordinal = request.next_send_ordinal.saturating_add(1);
            request.validate(key)?;
            let mut checkpoint = self.read_checkpoint()?;
            self.write_request_accounted(&request, &mut checkpoint)?;
            let _ = self.retire_if_eligible_locked(&mut checkpoint, key)?;
            checkpoint.updated_at_millis = unix_millis();
            write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
        }
        Ok(Some(provider_message_id))
    }

    fn read_request(&self, key: &str) -> Result<RequestRecord> {
        if !valid_key(key) {
            return Err(ChatRuntimeError::invalid(
                "chat request key must be 64 lowercase hexadecimal characters",
            ));
        }
        let record: RequestRecord =
            read_document(&self.request_path(key), MAX_REQUEST_RECORD_BYTES)?;
        self.validate_request_record(&record, key)?;
        Ok(record)
    }

    fn validate_request_record(&self, record: &RequestRecord, path_key: &str) -> Result<()> {
        record.validate(path_key)?;
        if !self.config.channel_ids.contains(&record.message.channel_id)
            || !self
                .config
                .allowed_senders
                .contains(&record.message.sender_id)
        {
            return Err(ChatRuntimeError::invalid(
                "saved request is outside configured channel or sender authority",
            ));
        }
        Ok(())
    }

    fn reply_path(&self, key: &str, ordinal: u32) -> PathBuf {
        self.root
            .join("replies")
            .join(format!("{key}-{ordinal:06}.json"))
    }

    fn read_reply(&self, key: &str, ordinal: u32) -> Result<ReplyRecord> {
        if !valid_key(key) || ordinal == 0 || ordinal > MAX_REPLY_ORDINAL {
            return Err(ChatRuntimeError::invalid("invalid reply artifact identity"));
        }
        let reply: ReplyRecord =
            read_document(&self.reply_path(key, ordinal), MAX_REPLY_RECORD_BYTES)?;
        reply.validate(key, ordinal)?;
        Ok(reply)
    }

    fn set_delivery_phase(
        &self,
        key: &str,
        phase: RequestPhase,
        detail: Option<&str>,
    ) -> Result<RequestRecord> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut record = self.read_request(key)?;
        record.phase = phase;
        record.delivery_error = detail.map(|value| bounded_detail(value, 2_000));
        let mut checkpoint = self.read_checkpoint()?;
        self.write_request_accounted(&record, &mut checkpoint)?;
        if record.phase == RequestPhase::Delivered {
            let _ = self.retire_if_eligible_locked(&mut checkpoint, key)?;
        }
        checkpoint.updated_at_millis = unix_millis();
        write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
        Ok(record)
    }

    fn write_request_accounted(
        &self,
        record: &RequestRecord,
        checkpoint: &mut Checkpoint,
    ) -> Result<()> {
        record.validate(&record.key)?;
        let (_, old_bytes): (RequestRecord, u64) =
            read_document_sized(&self.request_path(&record.key), MAX_REQUEST_RECORD_BYTES)?;
        let new_bytes = u64::try_from(encoded_document_bytes(record)?)
            .map_err(|_| ChatRuntimeError::invalid("request record length does not fit u64"))?;
        if new_bytes > u64::try_from(MAX_REQUEST_RECORD_BYTES).unwrap_or(u64::MAX) {
            return Err(ChatRuntimeError::invalid(format!(
                "request record exceeds {MAX_REQUEST_RECORD_BYTES} bytes"
            )));
        }
        let next_total = checkpoint
            .request_bytes
            .checked_sub(old_bytes)
            .and_then(|bytes| bytes.checked_add(new_bytes))
            .ok_or_else(|| ChatRuntimeError::invalid("request byte accounting underflow"))?;
        if next_total > MAX_REQUEST_BYTES {
            return Err(ChatRuntimeError::invalid(
                "chat request byte population limit reached",
            ));
        }
        write_document(&self.request_path(&record.key), record)?;
        checkpoint.request_bytes = next_total;
        Ok(())
    }

    fn read_checkpoint(&self) -> Result<Checkpoint> {
        let checkpoint: Checkpoint = read_document(&self.root.join("checkpoint.json"), 1 << 20)?;
        validate_checkpoint(&checkpoint)?;
        Ok(checkpoint)
    }

    fn gap_diagnostic(&self) -> Result<Option<GapDiagnostic>> {
        let path = self.root.join("gap.json");
        match read_document(&path, MAX_GAP_DIAGNOSTIC_BYTES) {
            Ok(diagnostic) => {
                let diagnostic: GapDiagnostic = diagnostic;
                diagnostic.validate()?;
                Ok(Some(diagnostic))
            }
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn current_commit_receipt(&self, checkpoint: &Checkpoint) -> Result<Option<CommitReceipt>> {
        if checkpoint.host_batch_sequence == 0 {
            return Ok(None);
        }
        let path = self.commit_receipt_path(checkpoint.host_batch_sequence);
        let receipt: CommitReceipt = match read_document(&path, MAX_COMMIT_RECEIPT_BYTES) {
            Ok(receipt) => receipt,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        receipt.validate()?;
        if receipt.host_batch_sequence != checkpoint.host_batch_sequence
            || checkpoint.cursor.as_deref() != Some(receipt.cursor.as_str())
        {
            return Err(ChatRuntimeError::invalid(
                "latest commit receipt does not match the durable checkpoint",
            ));
        }
        Ok(Some(receipt))
    }

    fn request_path(&self, key: &str) -> PathBuf {
        self.root.join("requests").join(format!("{key}.json"))
    }

    fn commit_receipt_path(&self, host_batch_sequence: u64) -> PathBuf {
        let slot = host_batch_sequence.saturating_sub(1) % COMMIT_RECEIPT_SLOTS;
        self.root
            .join("commit-receipts")
            .join(format!("slot-{slot:03}.json"))
    }

    fn retired_key_path(&self, key: &str) -> PathBuf {
        self.root.join("tombstones").join(format!("key-{key}.json"))
    }

    fn retired_route_path(&self, retirement_sequence: u64) -> PathBuf {
        let slot = retirement_sequence.saturating_sub(1) % RETIRED_ROUTE_SLOTS;
        self.root
            .join("tombstones")
            .join(format!("slot-{slot:04}.json"))
    }

    fn retirement_path(&self, retirement_sequence: u64) -> PathBuf {
        let slot = retirement_sequence.saturating_sub(1) % RETIRED_ROUTE_SLOTS;
        self.root
            .join("retirements")
            .join(format!("slot-{slot:04}.json"))
    }

    fn retired_key(&self, key: &str) -> Result<Option<RetiredRouteIndex>> {
        let path = self.retired_key_path(key);
        let retired: RetiredRouteIndex = match read_document(&path, MAX_REQUEST_RECORD_BYTES) {
            Ok(retired) => retired,
            Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        retired.validate()?;
        if retired.request_key != key {
            return Err(ChatRuntimeError::invalid(
                "retired request guard does not match its key path",
            ));
        }
        Ok(Some(retired))
    }

    fn retired_routes(&self) -> Result<Vec<RetiredRouteIndex>> {
        let checkpoint = self.read_checkpoint()?;
        let count = checkpoint.retired_route_count.min(RETIRED_ROUTE_SLOTS);
        if count == 0 {
            return Ok(Vec::new());
        }
        let first = checkpoint
            .retirement_sequence
            .checked_sub(count.saturating_sub(1))
            .ok_or_else(|| ChatRuntimeError::invalid("retired route sequence underflow"))?;
        let mut routes = Vec::with_capacity(usize::try_from(count).unwrap_or(usize::MAX));
        for sequence in first..=checkpoint.retirement_sequence {
            let route: RetiredRouteIndex =
                read_document(&self.retired_route_path(sequence), MAX_REQUEST_RECORD_BYTES)?;
            route.validate()?;
            if route.retirement_sequence != sequence {
                return Err(ChatRuntimeError::invalid(
                    "retired route ring slot does not match its expected sequence",
                ));
            }
            routes.push(route);
        }
        Ok(routes)
    }

    fn request_records(&self) -> Result<Vec<(RequestRecord, u64)>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(self.root.join("requests"))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| ChatRuntimeError::invalid("request filename is not UTF-8"))?;
            let key = name
                .strip_suffix(".json")
                .filter(|key| valid_key(key))
                .ok_or_else(|| ChatRuntimeError::invalid("unexpected chat request artifact"))?;
            paths.push((entry.path(), key.to_owned()));
            if paths.len() > usize::try_from(MAX_REQUESTS).unwrap_or(usize::MAX) {
                return Err(ChatRuntimeError::invalid(
                    "chat request population exceeds its record cap",
                ));
            }
        }
        paths.sort_by(|left, right| left.1.cmp(&right.1));
        paths
            .into_iter()
            .map(|(path, key)| {
                let (record, bytes): (RequestRecord, u64) =
                    read_document_sized(&path, MAX_REQUEST_RECORD_BYTES)?;
                self.validate_request_record(&record, &key)?;
                Ok((record, bytes))
            })
            .collect()
    }

    fn reply_records(&self) -> Result<Vec<(ReplyRecord, u64)>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(self.root.join("replies"))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| ChatRuntimeError::invalid("reply filename is not UTF-8"))?;
            let stem = name
                .strip_suffix(".json")
                .ok_or_else(|| ChatRuntimeError::invalid("unexpected chat reply artifact"))?;
            let (key, ordinal) = stem
                .rsplit_once('-')
                .ok_or_else(|| ChatRuntimeError::invalid("malformed chat reply artifact name"))?;
            let ordinal = ordinal
                .parse::<u32>()
                .ok()
                .filter(|ordinal| *ordinal > 0 && *ordinal <= MAX_REPLY_ORDINAL)
                .ok_or_else(|| ChatRuntimeError::invalid("invalid chat reply ordinal"))?;
            if !valid_key(key) {
                return Err(ChatRuntimeError::invalid("invalid chat reply request key"));
            }
            paths.push((entry.path(), key.to_owned(), ordinal));
            if paths.len() > usize::try_from(MAX_STATE_REPLIES).unwrap_or(usize::MAX) {
                return Err(ChatRuntimeError::invalid(
                    "chat reply population exceeds its record cap",
                ));
            }
        }
        paths.sort_by(|left, right| (&left.1, left.2).cmp(&(&right.1, right.2)));
        paths
            .into_iter()
            .map(|(path, key, ordinal)| {
                let (record, _actual_bytes): (ReplyRecord, u64) =
                    read_document_sized(&path, MAX_REPLY_RECORD_BYTES)?;
                record.validate(&key, ordinal)?;
                let reserved_bytes = record.reserved_bytes;
                Ok((record, reserved_bytes))
            })
            .collect()
    }

    fn complete_retirements(&self) -> Result<()> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let mut paths = fs::read_dir(self.root.join("retirements"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if paths.len() > usize::try_from(RETIRED_ROUTE_SLOTS).unwrap_or(usize::MAX) {
            return Err(ChatRuntimeError::invalid(
                "retirement journal exceeds its bounded slot count",
            ));
        }
        paths.sort();
        for path in paths {
            let mut retirement: RetirementRecord =
                read_document(&path, MAX_RETIREMENT_RECORD_BYTES)?;
            retirement.validate()?;
            if retirement.phase == RetirementPhase::Retired {
                continue;
            }
            let index = RetiredRouteIndex {
                version: STATE_VERSION,
                retirement_sequence: retirement.retirement_sequence,
                request_key: retirement.request_key.clone(),
                message_fingerprint: retirement.message_fingerprint.clone(),
                reply_nonce: retirement.reply_nonce.clone(),
                admitted_cursor: retirement.admitted_cursor.clone(),
                retired_at_millis: retirement.prepared_at_millis,
            };
            index.validate()?;
            write_document(&self.retired_key_path(&retirement.request_key), &index)?;
            write_document(
                &self.retired_route_path(retirement.retirement_sequence),
                &index,
            )?;
            if let Some(evicted) = retirement.evicted_request_key.as_deref() {
                if evicted != retirement.request_key {
                    remove_if_exists(&self.retired_key_path(evicted))?;
                }
            }
            for ordinal in 1..=retirement.reply_count {
                remove_if_exists(&self.reply_path(&retirement.request_key, ordinal))?;
            }
            remove_if_exists(&self.request_path(&retirement.request_key))?;
            agent::sync_directory(&self.root.join("replies"))?;
            agent::sync_directory(&self.root.join("requests"))?;
            retirement.phase = RetirementPhase::Retired;
            retirement.retired_at_millis = Some(unix_millis());
            retirement.validate()?;
            write_document(&path, &retirement)?;
        }
        let mut route_count = 0_u64;
        let mut latest_sequence = 0_u64;
        for entry in fs::read_dir(self.root.join("tombstones"))? {
            let entry = entry?;
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("slot-") {
                continue;
            }
            let route: RetiredRouteIndex = read_document(&entry.path(), MAX_REQUEST_RECORD_BYTES)?;
            route.validate()?;
            route_count = route_count.saturating_add(1);
            latest_sequence = latest_sequence.max(route.retirement_sequence);
        }
        let mut checkpoint = self.read_checkpoint()?;
        checkpoint.retirement_sequence = checkpoint.retirement_sequence.max(latest_sequence);
        checkpoint.retired_route_count = route_count.min(RETIRED_ROUTE_SLOTS);
        checkpoint.updated_at_millis = unix_millis();
        write_document(&self.root.join("checkpoint.json"), &checkpoint)
    }

    /// One explicit startup pass repairs counters after a crash between request creation and the
    /// cursor write. The provider hot loop thereafter uses the checkpoint and direct key lookups.
    fn recover_population(&self) -> Result<()> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        state_lock.lock_exclusive().map_err(ChatRuntimeError::Io)?;
        let records = self.request_records()?;
        let count = u64::try_from(records.len())
            .map_err(|_| ChatRuntimeError::invalid("request count does not fit u64"))?;
        let bytes = records
            .iter()
            .try_fold(0_u64, |total, (_, bytes)| total.checked_add(*bytes))
            .ok_or_else(|| ChatRuntimeError::invalid("request byte count overflow"))?;
        if count > MAX_REQUESTS || bytes > MAX_REQUEST_BYTES {
            return Err(ChatRuntimeError::invalid(
                "chat request population exceeds its durable cap",
            ));
        }
        let mut checkpoint = self.read_checkpoint()?;
        let mut checkpoint_changed = false;
        if checkpoint.request_count != count || checkpoint.request_bytes != bytes {
            checkpoint.request_count = count;
            checkpoint.request_bytes = bytes;
            checkpoint_changed = true;
        }
        let replies = self.reply_records()?;
        let reply_count = u64::try_from(replies.len())
            .map_err(|_| ChatRuntimeError::invalid("reply count does not fit u64"))?;
        let reply_bytes = replies
            .iter()
            .try_fold(0_u64, |total, (_, bytes)| total.checked_add(*bytes))
            .ok_or_else(|| ChatRuntimeError::invalid("reply byte count overflow"))?;
        if reply_count > MAX_STATE_REPLIES || reply_bytes > MAX_STATE_REPLY_BYTES {
            return Err(ChatRuntimeError::invalid(
                "chat reply population exceeds its durable cap",
            ));
        }
        let mut by_request: BTreeMap<String, Vec<(ReplyRecord, u64)>> = records
            .iter()
            .map(|(request, _)| (request.key.clone(), Vec::new()))
            .collect();
        for (reply, bytes) in replies {
            by_request
                .get_mut(&reply.request_key)
                .ok_or_else(|| ChatRuntimeError::invalid("retained reply names a missing request"))?
                .push((reply, bytes));
        }
        for (key, values) in by_request {
            if values.len() > usize::try_from(MAX_REQUEST_REPLIES).unwrap_or(usize::MAX) {
                return Err(ChatRuntimeError::invalid(
                    "request reply population exceeds its record cap",
                ));
            }
            let mut request = self.read_request(&key)?;
            let mut expected = 1_u32;
            let mut first_unsent = None;
            let mut bytes = 0_u64;
            for (reply, size) in &values {
                if reply.ordinal != expected {
                    return Err(ChatRuntimeError::invalid(
                        "retained reply ordinals are not contiguous",
                    ));
                }
                if reply.phase != ReplyPhase::Sent && first_unsent.is_none() {
                    first_unsent = Some(reply.ordinal);
                }
                if first_unsent.is_some() && reply.phase == ReplyPhase::Sent {
                    return Err(ChatRuntimeError::invalid(
                        "a sent reply follows an unsent ordinal",
                    ));
                }
                bytes = bytes.saturating_add(*size);
                expected = expected.saturating_add(1);
            }
            if bytes > MAX_REQUEST_REPLY_BYTES {
                return Err(ChatRuntimeError::invalid(
                    "request reply population exceeds its byte cap",
                ));
            }
            let original = (
                request.reply_count,
                request.reply_bytes,
                request.next_reply_ordinal,
                request.next_send_ordinal,
            );
            request.reply_count = u32::try_from(values.len())
                .map_err(|_| ChatRuntimeError::invalid("request reply count does not fit u32"))?;
            request.reply_bytes = bytes;
            request.next_reply_ordinal = expected;
            request.next_send_ordinal = first_unsent.unwrap_or(expected);
            request.validate(&key)?;
            let repaired = (
                request.reply_count,
                request.reply_bytes,
                request.next_reply_ordinal,
                request.next_send_ordinal,
            );
            if repaired != original {
                write_document(&self.request_path(&key), &request)?;
            }
        }
        let repaired_requests = self.request_records()?;
        let repaired_count = u64::try_from(repaired_requests.len())
            .map_err(|_| ChatRuntimeError::invalid("request count does not fit u64"))?;
        let repaired_bytes = repaired_requests
            .iter()
            .try_fold(0_u64, |total, (_, bytes)| total.checked_add(*bytes))
            .ok_or_else(|| ChatRuntimeError::invalid("request byte count overflow"))?;
        if repaired_count > MAX_REQUESTS || repaired_bytes > MAX_REQUEST_BYTES {
            return Err(ChatRuntimeError::invalid(
                "repaired request population exceeds its durable cap",
            ));
        }
        if checkpoint.request_count != repaired_count || checkpoint.request_bytes != repaired_bytes
        {
            checkpoint.request_count = repaired_count;
            checkpoint.request_bytes = repaired_bytes;
            checkpoint_changed = true;
        }
        if checkpoint.reply_count != reply_count || checkpoint.reply_bytes != reply_bytes {
            checkpoint.reply_count = reply_count;
            checkpoint.reply_bytes = reply_bytes;
            checkpoint_changed = true;
        }
        validate_checkpoint(&checkpoint)?;
        if checkpoint_changed {
            checkpoint.updated_at_millis = unix_millis();
            write_document(&self.root.join("checkpoint.json"), &checkpoint)?;
        }
        Ok(())
    }
}

/// Deliver or reconcile one admitted request without ever replaying uncertain terminal input.
pub fn deliver_request<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    key: &str,
    options: DrainOptions,
) -> Result<CoordinatorDeliveryResult> {
    deliver_request_with(state, manager, key, options)
}

/// Inject one deduplicated coordinator diagnostic for unavailable reply fence identifiers.
pub fn deliver_fence_feedback<A: ManagedApi + ?Sized>(
    state: &BridgeState,
    manager: &ManagedAgents<'_, A>,
    unknown_ids: &[String],
    options: DrainOptions,
) -> Result<CoordinatorDeliveryResult> {
    deliver_fence_feedback_with(state, manager, unknown_ids, options)
}

pub(crate) fn deliver_fence_feedback_with(
    state: &BridgeState,
    delivery: &dyn CoordinatorDelivery,
    unknown_ids: &[String],
    options: DrainOptions,
) -> Result<CoordinatorDeliveryResult> {
    if unknown_ids.is_empty() {
        return Ok(CoordinatorDeliveryResult::AlreadyDelivered);
    }
    if unknown_ids.len() > 128
        || unknown_ids
            .iter()
            .any(|identifier| identifier.is_empty() || identifier.len() > 256)
    {
        return Err(ChatRuntimeError::invalid(
            "unavailable reply marker diagnostics exceed their bounded population",
        ));
    }
    let available = state.available_reply_ids()?;
    let mut unavailable = unknown_ids.to_vec();
    unavailable.sort();
    unavailable.dedup();
    let identity = serde_json::to_vec(&serde_json::json!({
        "unavailable": unavailable,
        "available": available,
    }))?;
    let message_id = format!("chat-feedback-{:x}", Sha256::digest(identity));
    let prompt = format!(
        "Chat reply routing error: your output referenced unavailable reply ID(s): {}. \
The currently available reply ID(s) are: {}. Emit a complete reply block using one exact available ID.",
        unavailable.join(", "),
        format_available_reply_ids(&available)
    );
    let agent_name = &state.config.agent_name;
    let initial = delivery
        .message_state(agent_name, &message_id)
        .map_err(ChatRuntimeError::invalid)?;
    let operation = match initial {
        Some(QueueMessageState::Processed) => {
            return Ok(CoordinatorDeliveryResult::AlreadyDelivered)
        }
        Some(QueueMessageState::Inflight | QueueMessageState::Failed) => {
            return Ok(CoordinatorDeliveryResult::Uncertain(
                "fence feedback may already have reached the coordinator".to_owned(),
            ));
        }
        Some(QueueMessageState::Pending) => delivery.drain(agent_name, options),
        None => delivery.submit(agent_name, &prompt, &message_id, options),
    };
    if operation.is_ok() && initial.is_none() {
        return Ok(CoordinatorDeliveryResult::Delivered);
    }
    let detail = operation.err().map(|error| error.to_string());
    match delivery
        .message_state(agent_name, &message_id)
        .map_err(ChatRuntimeError::invalid)?
    {
        Some(QueueMessageState::Processed) => Ok(CoordinatorDeliveryResult::Delivered),
        Some(QueueMessageState::Inflight | QueueMessageState::Failed) => {
            Ok(CoordinatorDeliveryResult::Uncertain(detail.unwrap_or_else(
                || "fence feedback may already have reached the coordinator".to_owned(),
            )))
        }
        Some(QueueMessageState::Pending) | None => Ok(CoordinatorDeliveryResult::Pending(
            detail.unwrap_or_else(|| "fence feedback remains pending".to_owned()),
        )),
    }
}

fn format_available_reply_ids(available: &[String]) -> String {
    if available.is_empty() {
        return "<none>".to_owned();
    }
    let mut displayed = available
        .iter()
        .take(MAX_FEEDBACK_AVAILABLE_IDS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if available.len() > MAX_FEEDBACK_AVAILABLE_IDS {
        displayed.push_str(&format!(
            " (and {} more; inspect chat status for the complete set)",
            available.len() - MAX_FEEDBACK_AVAILABLE_IDS
        ));
    }
    displayed
}

pub(crate) fn deliver_request_with(
    state: &BridgeState,
    delivery: &dyn CoordinatorDelivery,
    key: &str,
    options: DrainOptions,
) -> Result<CoordinatorDeliveryResult> {
    let mut record = state.read_request(key)?;
    match record.phase {
        RequestPhase::Delivered => return Ok(CoordinatorDeliveryResult::AlreadyDelivered),
        RequestPhase::DeliveryUncertain => {
            return Ok(CoordinatorDeliveryResult::Uncertain(
                record
                    .delivery_error
                    .unwrap_or_else(|| "prior coordinator delivery is uncertain".to_owned()),
            ));
        }
        RequestPhase::Pending => {
            record = state.set_delivery_phase(key, RequestPhase::Submitting, None)?;
        }
        RequestPhase::Submitting => {}
    }

    let agent_name = &state.config.agent_name;
    let message_id = &record.delivery_message_id;
    let observed = match delivery.message_state(agent_name, message_id) {
        Ok(observed) => observed,
        Err(error) => {
            state.set_delivery_phase(key, RequestPhase::Submitting, Some(&error))?;
            return Ok(CoordinatorDeliveryResult::Pending(error));
        }
    };

    let operation_error = match observed {
        Some(QueueMessageState::Processed) => None,
        Some(QueueMessageState::Inflight | QueueMessageState::Failed) => {
            let detail = "coordinator queue reports a possibly submitted request";
            state.set_delivery_phase(key, RequestPhase::DeliveryUncertain, Some(detail))?;
            return Ok(CoordinatorDeliveryResult::Uncertain(detail.to_owned()));
        }
        Some(QueueMessageState::Pending) => delivery.drain(agent_name, options).err(),
        None => delivery
            .submit(agent_name, &state.prompt(key)?, message_id, options)
            .err(),
    };

    if operation_error.is_none() && observed != Some(QueueMessageState::Pending) {
        state.set_delivery_phase(key, RequestPhase::Delivered, None)?;
        return Ok(CoordinatorDeliveryResult::Delivered);
    }

    match delivery.message_state(agent_name, message_id) {
        Ok(Some(QueueMessageState::Processed)) => {
            state.set_delivery_phase(key, RequestPhase::Delivered, None)?;
            Ok(CoordinatorDeliveryResult::Delivered)
        }
        Ok(Some(QueueMessageState::Inflight | QueueMessageState::Failed)) => {
            let detail = operation_error.unwrap_or_else(|| {
                "coordinator queue reports a possibly submitted request".to_owned()
            });
            state.set_delivery_phase(key, RequestPhase::DeliveryUncertain, Some(&detail))?;
            Ok(CoordinatorDeliveryResult::Uncertain(detail))
        }
        Ok(Some(QueueMessageState::Pending) | None) => {
            let detail = operation_error
                .unwrap_or_else(|| "coordinator is not ready; request remains pending".to_owned());
            state.set_delivery_phase(key, RequestPhase::Pending, Some(&detail))?;
            Ok(CoordinatorDeliveryResult::Pending(detail))
        }
        Err(error) => {
            let detail = operation_error.map_or(error.clone(), |operation| {
                format!("{operation}; recovery probe failed: {error}")
            });
            state.set_delivery_phase(key, RequestPhase::Submitting, Some(&detail))?;
            Ok(CoordinatorDeliveryResult::Pending(detail))
        }
    }
}

/// Receive, durably admit, and acknowledge one ordered stream item.
pub fn consume_one(
    subscription: &mut ChatSubscription,
    state: &BridgeState,
) -> Result<ConsumedItem> {
    match subscription.next_item()? {
        None => Ok(ConsumedItem::End),
        Some(SubscriptionItem::Heartbeat(_)) => Ok(ConsumedItem::Heartbeat),
        Some(SubscriptionItem::Batch(batch)) => {
            let admission = state.admit_batch(&batch)?;
            subscription.commit_durable(&batch)?;
            state.confirm_batch_commit(&admission)?;
            Ok(ConsumedItem::Batch(admission))
        }
    }
}

fn validate_checkpoint(checkpoint: &Checkpoint) -> Result<()> {
    if checkpoint.version != STATE_VERSION {
        return Err(ChatRuntimeError::invalid(format!(
            "chat checkpoint has unsupported version {}",
            checkpoint.version
        )));
    }
    if checkpoint.request_count > MAX_REQUESTS
        || checkpoint.request_bytes > MAX_REQUEST_BYTES
        || checkpoint.reply_count > MAX_STATE_REPLIES
        || checkpoint.reply_bytes > MAX_STATE_REPLY_BYTES
        || checkpoint.retired_route_count > RETIRED_ROUTE_SLOTS
    {
        return Err(ChatRuntimeError::invalid(
            "chat checkpoint exceeds a retained population cap",
        ));
    }
    if let Some(cursor) = checkpoint.cursor.as_deref() {
        ProviderCursor::new(cursor.to_owned())
            .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
    }
    Ok(())
}

fn validate_slug(value: &str, label: &str, maximum: usize) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > maximum
        || !bytes[0].is_ascii_lowercase()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(ChatRuntimeError::invalid(format!(
            "{label} must be a lowercase slug of 1-{maximum} bytes"
        )));
    }
    Ok(())
}

fn validate_single_line(value: &str, label: &str, maximum: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > maximum || value.contains(['\n', '\r', '\0']) {
        return Err(ChatRuntimeError::invalid(format!(
            "{label} must be one nonempty line of at most {maximum} bytes"
        )));
    }
    Ok(())
}

fn validate_optional_single_line_or_multiline(
    value: &str,
    label: &str,
    maximum: usize,
) -> Result<()> {
    if value.len() > maximum
        || value.contains('\0')
        || value.chars().any(invalid_rendered_character)
    {
        return Err(ChatRuntimeError::invalid(format!(
            "{label} must contain at most {maximum} rendered UTF-8 bytes"
        )));
    }
    Ok(())
}

fn message_key(message: &SavedMessage) -> Result<String> {
    let identity = serde_json::to_vec(&serde_json::json!({
        "channel_id": message.channel_id,
        "message_id": message.message_id,
    }))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn saved_message_fingerprint(message: &SavedMessage) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(message)?)
    ))
}

fn batch_fingerprint(batch: &DeliveryBatch) -> Result<String> {
    let events = batch
        .events()
        .iter()
        .map(|event| match event {
            CommittableEvent::MessageCreated(message) => serde_json::json!({
                "kind": "message_created",
                "channel_id": message.channel_id().as_str(),
                "message_id": message.message_id().as_str(),
                "thread_id": message.thread_id().as_str(),
                "sender_id": message.sender_id().as_str(),
                "text": message.text(),
                "created_at": message.created_at(),
                "thread_reply": message.is_thread_reply(),
                "provider_payload": message.provider_payload().map(|payload| serde_json::json!({
                    "schema": payload.schema(),
                    "data": payload.data(),
                })),
            }),
            CommittableEvent::Checkpoint => serde_json::json!({"kind": "checkpoint"}),
            CommittableEvent::Gap(gap) => serde_json::json!({
                "kind": "gap",
                "reason": gap.reason(),
            }),
        })
        .collect::<Vec<_>>();
    let identity = serde_json::to_vec(&serde_json::json!({
        "cursor": batch.cursor().as_str(),
        "events": events,
    }))?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

fn valid_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_reply_body(body: &str) -> Result<()> {
    if body.trim().is_empty() || body.len() > MAX_REPLY_BYTES {
        return Err(ChatRuntimeError::invalid(format!(
            "chat reply body must be nonempty and at most {MAX_REPLY_BYTES} UTF-8 bytes"
        )));
    }
    if body.chars().any(invalid_rendered_character) {
        return Err(ChatRuntimeError::invalid(
            "chat reply body contains terminal control characters",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScannedReply {
    ordinal: u32,
    body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReplyScan {
    blocks: Vec<ScannedReply>,
    unknown_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MultiReplyScan {
    blocks_by_nonce: BTreeMap<String, Vec<ScannedReply>>,
    observed_ids: Vec<String>,
    unknown_ids: Vec<String>,
}

#[derive(Clone, Debug)]
struct ActiveReply {
    protocol: &'static str,
    identifier: String,
    nonce: String,
    ordinal: u32,
    opening_margin: String,
    body: Vec<String>,
}

fn scan_reply_blocks(rendered: &str, expected_nonce: &str) -> Result<ReplyScan> {
    let expected_nonces = BTreeSet::from([expected_nonce.to_owned()]);
    let mut scan = scan_reply_blocks_for_nonces(rendered, &expected_nonces)?;
    Ok(ReplyScan {
        blocks: scan
            .blocks_by_nonce
            .remove(expected_nonce)
            .unwrap_or_default(),
        unknown_ids: scan.unknown_ids,
    })
}

fn scan_reply_blocks_for_nonces(
    rendered: &str,
    expected_nonces: &BTreeSet<String>,
) -> Result<MultiReplyScan> {
    if expected_nonces.iter().any(|nonce| !valid_nonce(nonce)) {
        return Err(ChatRuntimeError::invalid(
            "expected reply nonce is not 22-character base64url",
        ));
    }
    if rendered.chars().any(invalid_rendered_character) {
        return Err(ChatRuntimeError::invalid(
            "reply capture contains terminal control characters",
        ));
    }
    let normalized = rendered.replace("\r\n", "\n");
    let mut blocks_by_nonce = BTreeMap::<String, Vec<ScannedReply>>::new();
    let mut observed_ids = Vec::new();
    let mut unknown_ids = Vec::new();
    let mut seen_unknown_ids = BTreeSet::new();
    let mut seen_identifiers = BTreeSet::new();
    let mut active: Option<ActiveReply> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut prompt_margin: Option<usize> = None;

    for line in normalized.split('\n') {
        let (undecorated, margin, decorated) = undecorate(line);
        let stripped = line.trim_start_matches([' ', '\t']);
        if active.is_none() {
            if stripped.starts_with("› ") || stripped.starts_with("❯ ") {
                prompt_margin = Some(line.len() - stripped.len() + 2);
                continue;
            }
            if let Some(expected_margin) = prompt_margin {
                if stripped.is_empty() || line.len() - stripped.len() >= expected_margin {
                    continue;
                }
                prompt_margin = None;
            }
        }

        if active.is_none()
            && decorated
            && parse_marker(&undecorated).is_some_and(|marker| {
                !marker.closing
                    && recognized_reply_marker(&marker.identifier, expected_nonces).is_some()
            })
        {
            // Native assistant bullets delimit items. An unrelated unmatched Markdown fence
            // from an older retained item must not hide the fresh expected reply marker.
            fence = None;
        }
        if let Some((fence_character, fence_length)) = fence {
            if closing_fence(&undecorated, fence_character, fence_length) {
                fence = None;
            }
            if let Some(active) = active.as_mut() {
                active.body.push(line.to_owned());
            }
            continue;
        }

        if let Some(opened) = opening_fence(&undecorated) {
            fence = Some(opened);
            if let Some(active) = active.as_mut() {
                active.body.push(line.to_owned());
            }
            continue;
        }

        let Some(marker) = parse_marker(&undecorated) else {
            if let Some(active) = active.as_mut() {
                active.body.push(line.to_owned());
            }
            continue;
        };
        if seen_identifiers.insert(marker.identifier.clone()) {
            if seen_identifiers.len() > MAX_VISIBLE_MARKERS {
                return Err(ChatRuntimeError::invalid(format!(
                    "reply capture exceeds {MAX_VISIBLE_MARKERS} distinct visible markers"
                )));
            }
            observed_ids.push(marker.identifier.clone());
        }
        let expected = recognized_reply_marker(&marker.identifier, expected_nonces);
        if expected.is_none()
            && unknown_ids.len() < 128
            && seen_unknown_ids.insert(marker.identifier.clone())
        {
            unknown_ids.push(bounded_detail(&marker.identifier, 256));
        }

        match active.as_mut() {
            None if !marker.closing => {
                if let Some((nonce, ordinal)) = expected {
                    let nonce = nonce.to_owned();
                    active = Some(ActiveReply {
                        protocol: marker.protocol,
                        identifier: marker.identifier,
                        nonce,
                        ordinal,
                        opening_margin: margin,
                        body: Vec::new(),
                    });
                }
            }
            None => {}
            Some(_) if !marker.closing => {
                return Err(ChatRuntimeError::invalid(
                    "nested chat reply markers are ambiguous",
                ));
            }
            Some(opened)
                if opened.identifier != marker.identifier || opened.protocol != marker.protocol =>
            {
                return Err(ChatRuntimeError::invalid(
                    "chat reply closing marker does not match its opening marker",
                ));
            }
            Some(_) => {
                let opened = active.take().expect("matched active reply");
                blocks_by_nonce
                    .entry(opened.nonce)
                    .or_default()
                    .push(ScannedReply {
                        ordinal: opened.ordinal,
                        body: reply_body(opened.body, &opened.opening_margin, &margin)?,
                    });
            }
        }
    }
    Ok(MultiReplyScan {
        blocks_by_nonce,
        observed_ids,
        unknown_ids,
    })
}

fn recognized_reply_marker<'a>(
    identifier: &'a str,
    expected_nonces: &BTreeSet<String>,
) -> Option<(&'a str, u32)> {
    let (nonce, _) = identifier.rsplit_once('_')?;
    expected_nonces
        .contains(nonce)
        .then(|| sequenced_ordinal(identifier, nonce))
        .flatten()
        .map(|ordinal| (nonce, ordinal))
}

#[derive(Clone, Debug)]
struct Marker {
    protocol: &'static str,
    closing: bool,
    identifier: String,
}

fn parse_marker(line: &str) -> Option<Marker> {
    let (closing, body) = line.strip_prefix("</").map_or_else(
        || line.strip_prefix('<').map(|body| (false, body)),
        |body| Some((true, body)),
    )?;
    let body = body.strip_suffix('>')?;
    let (protocol, identifier) = if let Some(identifier) = body.strip_prefix("CHAT_REPLY_") {
        ("CHAT", identifier)
    } else {
        ("GCHAT", body.strip_prefix("GCHAT_REPLY_")?)
    };
    if identifier
        .contains(|character: char| character.is_whitespace() || matches!(character, '<' | '>'))
    {
        return None;
    }
    Some(Marker {
        protocol,
        closing,
        identifier: identifier.to_owned(),
    })
}

fn sequenced_ordinal(identifier: &str, expected_nonce: &str) -> Option<u32> {
    let suffix = identifier.strip_prefix(expected_nonce)?.strip_prefix('_')?;
    if suffix.is_empty()
        || suffix.len() > 6
        || suffix.starts_with('0')
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    suffix
        .parse::<u32>()
        .ok()
        .filter(|ordinal| *ordinal <= MAX_REPLY_ORDINAL)
}

fn undecorate(line: &str) -> (String, String, bool) {
    let stripped = line.trim_start_matches([' ', '\t']);
    let mut margin = line[..line.len() - stripped.len()].to_owned();
    let (stripped, decorated) = ["• ", "⏺ "]
        .into_iter()
        .find_map(|prefix| stripped.strip_prefix(prefix).map(|value| (value, true)))
        .unwrap_or((stripped, false));
    if decorated {
        margin.push_str("  ");
    }
    let extra = stripped.len() - stripped.trim_start_matches([' ', '\t']).len();
    margin.push_str(&stripped[..extra]);
    (
        stripped[extra..].trim_end_matches([' ', '\t']).to_owned(),
        margin,
        decorated,
    )
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let character = line.chars().next()?;
    if !matches!(character, '`' | '~') {
        return None;
    }
    let length = line.chars().take_while(|value| *value == character).count();
    if length < 3 {
        return None;
    }
    let suffix = &line[length..];
    if character == '`' && suffix.contains('`') {
        return None;
    }
    Some((character, length))
}

fn closing_fence(line: &str, character: char, minimum: usize) -> bool {
    let length = line.chars().take_while(|value| *value == character).count();
    length >= minimum && line[length..].trim().is_empty()
}

fn reply_body(lines: Vec<String>, opening_margin: &str, closing_margin: &str) -> Result<String> {
    let mut margin_length = opening_margin.len();
    for line in std::iter::once(closing_margin).chain(
        lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(String::as_str),
    ) {
        let common = opening_margin
            .bytes()
            .zip(line.bytes())
            .take(margin_length)
            .take_while(|(left, right)| left == right)
            .count();
        margin_length = common;
        if margin_length == 0 {
            break;
        }
    }
    let margin = &opening_margin[..margin_length];
    let body = lines
        .into_iter()
        .map(|line| line.strip_prefix(margin).unwrap_or(&line).to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    validate_reply_body(&body)?;
    Ok(body)
}

fn invalid_rendered_character(character: char) -> bool {
    (character < ' ' && !matches!(character, '\t' | '\n' | '\r'))
        || ('\u{7f}'..='\u{9f}').contains(&character)
}

fn random_reply_nonce() -> Result<String> {
    Ok(base64url_nonce(random_bytes()?))
}

pub(crate) fn random_operation_uuid() -> Result<String> {
    let mut bytes = random_bytes()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ))
}

fn valid_operation_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            14 => byte == b'4',
            19 => matches!(byte, b'8' | b'9' | b'a' | b'b'),
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
}

fn random_bytes() -> Result<[u8; REPLY_NONCE_BYTES]> {
    let mut bytes = [0_u8; REPLY_NONCE_BYTES];
    let mut random = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open("/dev/urandom")?;
    random.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn base64url_nonce(bytes: [u8; REPLY_NONCE_BYTES]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(22);
    for chunk in bytes.chunks_exact(3) {
        output.push(char::from(ALPHABET[usize::from(chunk[0] >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from(((chunk[0] & 3) << 4) | (chunk[1] >> 4))],
        ));
        output.push(char::from(
            ALPHABET[usize::from(((chunk[1] & 15) << 2) | (chunk[2] >> 6))],
        ));
        output.push(char::from(ALPHABET[usize::from(chunk[2] & 63)]));
    }
    let tail = bytes[15];
    output.push(char::from(ALPHABET[usize::from(tail >> 2)]));
    output.push(char::from(ALPHABET[usize::from((tail & 3) << 4)]));
    output
}

fn valid_nonce(value: &str) -> bool {
    value.len() == 22
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn bounded_detail(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut boundary = maximum;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn outbound_text(agent_label: &str, body: &str) -> String {
    let prefix = format!("[{agent_label}]");
    if body.starts_with(&prefix) {
        body.to_owned()
    } else {
        format!("{prefix} {body}")
    }
}

fn validate_outbound_body(agent_label: &str, body: &str) -> Result<()> {
    let encoded = outbound_text(agent_label, body);
    if encoded.len() > MAX_REPLY_BYTES {
        return Err(ChatRuntimeError::invalid(format!(
            "agent-labelled chat reply exceeds {MAX_REPLY_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn write_document<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    agent::atomic_json(path, &serde_json::to_value(value)?)?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ChatRuntimeError::Io(error)),
    }
}

fn encoded_document_bytes<T: Serialize>(value: &T) -> Result<usize> {
    serde_json::to_vec_pretty(value)?
        .len()
        .checked_add(1)
        .ok_or_else(|| ChatRuntimeError::invalid("encoded document length overflow"))
}

fn read_document<T: for<'de> Deserialize<'de>>(path: &Path, maximum: usize) -> Result<T> {
    read_document_sized(path, maximum).map(|(value, _)| value)
}

fn read_document_sized<T: for<'de> Deserialize<'de>>(
    path: &Path,
    maximum: usize,
) -> Result<(T, u64)> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ChatRuntimeError::invalid(format!(
            "chat artifact is not a private owner-only regular file: {}",
            path.display()
        )));
    }
    let maximum_u64 = u64::try_from(maximum).unwrap_or(u64::MAX);
    if metadata.len() > maximum_u64 {
        return Err(ChatRuntimeError::invalid(format!(
            "chat artifact exceeds {maximum_u64} bytes: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(maximum)
            .min(maximum),
    );
    Read::by_ref(&mut file)
        .take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err(ChatRuntimeError::invalid(format!(
            "chat artifact changed while it was read: {}",
            path.display()
        )));
    }
    let document = chat_subscription_plugin::decode_strict_json(&bytes)
        .map_err(|error| ChatRuntimeError::invalid(error.to_string()))?;
    Ok((document, metadata.len()))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::num::NonZeroU16;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::sync::{Arc, Mutex};

    use chat_subscription::{
        BackendCapabilities, BackendFailure, CancellationError, ChatSubscriptionBackend,
        ChatSubscriptionCancellation, ChatSubscriptionDriver, DeliveryId, EventKind, EventSequence,
        InboundMessage, MessageId, ProviderPayload, ReconciliationGap, ReplaySupport,
        SubscriptionItem, ThreadId,
    };

    use super::*;

    struct Driver {
        items: VecDeque<SubscriptionItem>,
        acknowledged: Arc<Mutex<Vec<String>>>,
        next_calls: Arc<AtomicU64>,
        fail_ack: bool,
        sabotage_receipt_directory: Option<PathBuf>,
    }

    struct NoopCancellation;

    impl ChatSubscriptionCancellation for NoopCancellation {
        fn cancel(&self) -> std::result::Result<(), CancellationError> {
            Ok(())
        }
    }

    fn noop_cancellation() -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(NoopCancellation)
    }

    impl ChatSubscriptionDriver for Driver {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            noop_cancellation()
        }

        fn next_item(&mut self) -> std::result::Result<Option<SubscriptionItem>, BackendFailure> {
            self.next_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(self.items.pop_front())
        }

        fn acknowledge(
            &mut self,
            delivery_id: &DeliveryId,
        ) -> std::result::Result<(), BackendFailure> {
            if self.fail_ack {
                return Err(BackendFailure::new(
                    "commit_outcome_unknown",
                    "fixture withheld exact Committed confirmation",
                    true,
                )
                .expect("valid fixture failure"));
            }
            if let Some(directory) = self.sabotage_receipt_directory.take() {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
                    .expect("make receipt directory unwritable");
            }
            self.acknowledged
                .lock()
                .expect("acknowledgement lock")
                .push(delivery_id.as_str().to_owned());
            Ok(())
        }
    }

    struct Backend {
        items: Option<VecDeque<SubscriptionItem>>,
        acknowledged: Arc<Mutex<Vec<String>>>,
        next_calls: Arc<AtomicU64>,
        fail_ack: bool,
        sabotage_receipt_directory: Option<PathBuf>,
    }

    #[derive(Default)]
    struct FakeDelivery {
        queue_state: Mutex<Option<QueueMessageState>>,
        submitted_prompts: Mutex<Vec<String>>,
        drains: Mutex<u64>,
        submit_error: Mutex<Option<String>>,
    }

    impl CoordinatorDelivery for FakeDelivery {
        fn message_state(
            &self,
            _agent_name: &str,
            _message_id: &str,
        ) -> std::result::Result<Option<QueueMessageState>, String> {
            Ok(*self.queue_state.lock().expect("queue state lock"))
        }

        fn submit(
            &self,
            _agent_name: &str,
            prompt: &str,
            _message_id: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            self.submitted_prompts
                .lock()
                .expect("prompt lock")
                .push(prompt.to_owned());
            if let Some(error) = self.submit_error.lock().expect("error lock").take() {
                return Err(error);
            }
            *self.queue_state.lock().expect("queue state lock") =
                Some(QueueMessageState::Processed);
            Ok(())
        }

        fn drain(
            &self,
            _agent_name: &str,
            _options: DrainOptions,
        ) -> std::result::Result<(), String> {
            *self.drains.lock().expect("drain lock") += 1;
            *self.queue_state.lock().expect("queue state lock") =
                Some(QueueMessageState::Processed);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeReplyTransport {
        submissions: Vec<(String, String, String, String)>,
        fail_once: bool,
    }

    impl ReplyTransport for FakeReplyTransport {
        fn send(
            &mut self,
            submission: ReplySubmission<'_>,
        ) -> std::result::Result<String, OutboundFailure> {
            self.submissions.push((
                submission.channel_id.to_owned(),
                submission.thread_id.to_owned(),
                submission.body.to_owned(),
                submission.request_id.to_owned(),
            ));
            if std::mem::take(&mut self.fail_once) {
                return Err(OutboundFailure {
                    code: "fixture_unknown".to_owned(),
                    detail: "uncertain provider response".to_owned(),
                    outcome: OutboundOutcome::Unknown,
                    retryable: true,
                });
            }
            Ok(format!("messages/reply-{}", self.submissions.len()))
        }
    }

    #[derive(Default)]
    struct FakeReactionTransport {
        submissions: Vec<(String, String, String, String)>,
        fail_once: bool,
    }

    impl ReactionTransport for FakeReactionTransport {
        fn ensure_reaction(
            &mut self,
            submission: ReactionSubmission<'_>,
        ) -> std::result::Result<ReactionReceipt, OutboundFailure> {
            self.submissions.push((
                submission.channel_id.to_owned(),
                submission.message_id.to_owned(),
                submission.emoji.to_owned(),
                submission.request_id.to_owned(),
            ));
            if std::mem::take(&mut self.fail_once) {
                return Err(OutboundFailure {
                    code: "fixture_unknown".to_owned(),
                    detail: "provider outcome unknown".to_owned(),
                    outcome: OutboundOutcome::Unknown,
                    retryable: true,
                });
            }
            Ok(ReactionReceipt {
                reaction_id: "spaces/example/messages/one/reactions/robot".to_owned(),
                already_present: self.submissions.len() > 1,
            })
        }
    }

    impl ChatSubscriptionBackend for Backend {
        fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
            noop_cancellation()
        }

        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::new(
                "fixture",
                ReplaySupport::Cursor,
                true,
                NonZeroU16::new(1).expect("one"),
                vec![
                    EventKind::MessageCreated,
                    EventKind::Checkpoint,
                    EventKind::Gap,
                    EventKind::Heartbeat,
                ],
            )
            .expect("fixture capabilities")
        }

        fn subscribe(
            &mut self,
            _request: &SubscribeRequest,
        ) -> std::result::Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            Ok(Box::new(Driver {
                items: self.items.take().expect("single fixture subscription"),
                acknowledged: Arc::clone(&self.acknowledged),
                next_calls: Arc::clone(&self.next_calls),
                fail_ack: self.fail_ack,
                sabotage_receipt_directory: self.sabotage_receipt_directory.take(),
            }))
        }
    }

    fn temporary(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-chat-runtime-{name}-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        fs::create_dir(&path).expect("temporary directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("private temporary");
        path
    }

    fn copied_shell(root: &Path) -> PathBuf {
        let source = fs::canonicalize("/bin/sh").expect("canonical shell");
        let destination = root.join("outbound-helper");
        fs::copy(source, &destination).expect("copy native helper fixture");
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))
            .expect("set helper mode");
        destination
    }

    fn config() -> BridgeConfiguration {
        BridgeConfiguration {
            subscription_plugin: "fixture".to_owned(),
            channel_ids: vec!["spaces/example".to_owned()],
            allowed_senders: vec!["users/owner".to_owned()],
            agent_name: "coordinator".to_owned(),
            agent_label: "codex coordinator".to_owned(),
            outbound_enabled: true,
            ack_reaction: Some("🤖".to_owned()),
            backend_configuration: None,
            outbound_command: None,
        }
    }

    fn config_without_reaction() -> BridgeConfiguration {
        let mut configuration = config();
        configuration.ack_reaction = None;
        configuration
    }

    fn state_with_old_request(
        name: &str,
        configuration: BridgeConfiguration,
    ) -> (BridgeState, String, PathBuf) {
        let root = temporary(name);
        let state = BridgeState::initialize(&root, configuration).expect("initialize state");
        let key = state
            .admit_batch(&indexed_delivery(1, 1))
            .expect("first admission")
            .new_request_keys
            .into_iter()
            .next()
            .expect("first key");
        state
            .admit_batch(&indexed_delivery(2, 2))
            .expect("advance cursor");
        (state, key, root)
    }

    fn attempt_retirement(state: &BridgeState, key: &str) -> bool {
        let lock = agent::open_private_lock(&state.root.join(".state.lock"), "test state lock")
            .expect("open state lock");
        lock.lock_exclusive().expect("lock state");
        let mut checkpoint = state.read_checkpoint().expect("checkpoint");
        state
            .retire_if_eligible_locked(&mut checkpoint, key)
            .expect("retirement attempt")
    }

    fn delivery(sequence: u64, cursor: &str, receipt: &str) -> DeliveryBatch {
        let mut payload = Map::new();
        payload.insert("name".to_owned(), Value::String("messages/one".to_owned()));
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new("spaces/example/messages/one").expect("message"),
            ThreadId::new("spaces/example/threads/one").expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            "run the tests",
            "2026-09-21T12:00:00Z",
            false,
        )
        .expect("normalized message")
        .with_provider_payload(
            ProviderPayload::new("fixture.message.v1", payload).expect("provider payload"),
        );
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(receipt).expect("receipt"),
            vec![
                CommittableEvent::Checkpoint,
                CommittableEvent::message_created(message),
            ],
        )
        .expect("delivery")
    }

    fn indexed_delivery(sequence: u64, index: u64) -> DeliveryBatch {
        indexed_delivery_at(
            sequence,
            index,
            &format!("cursor-{index}"),
            &format!("request {index}"),
        )
    }

    fn indexed_delivery_at(sequence: u64, index: u64, cursor: &str, text: &str) -> DeliveryBatch {
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new(format!("spaces/example/messages/{index}")).expect("message"),
            ThreadId::new(format!("spaces/example/threads/{index}")).expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            text,
            "2026-09-21T12:00:00Z",
            false,
        )
        .expect("normalized message");
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(format!("delivery-{sequence}-{index}")).expect("delivery"),
            vec![CommittableEvent::message_created(message)],
        )
        .expect("indexed delivery")
    }

    fn maximum_message_delivery(sequence: u64, cursor: &str, receipt: &str) -> DeliveryBatch {
        let events = (0..chat_subscription::MAX_BATCH_EVENTS)
            .map(|index| {
                let message = InboundMessage::new(
                    ChannelId::new("spaces/example").expect("channel"),
                    MessageId::new(format!("spaces/example/messages/{index:03}")).expect("message"),
                    ThreadId::new("spaces/example/threads/one").expect("thread"),
                    SenderId::new("users/owner").expect("sender"),
                    format!("request {index}"),
                    "2026-09-21T12:00:00Z",
                    false,
                )
                .expect("normalized message")
                .with_provider_payload(
                    ProviderPayload::new(
                        "fixture.message.v1",
                        Map::from_iter([("index".to_owned(), Value::from(index as u64))]),
                    )
                    .expect("provider payload"),
                );
                CommittableEvent::message_created(message)
            })
            .collect::<Vec<_>>();
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(receipt).expect("receipt"),
            events,
        )
        .expect("maximum message delivery")
    }

    fn gap_delivery(sequence: u64, cursor: &str, receipt: &str) -> DeliveryBatch {
        DeliveryBatch::new(
            EventSequence::new(sequence).expect("sequence"),
            ProviderCursor::new(cursor).expect("cursor"),
            DeliveryId::new(receipt).expect("receipt"),
            vec![CommittableEvent::Gap(
                ReconciliationGap::new(Some("fixture history was truncated".to_owned()))
                    .expect("gap"),
            )],
        )
        .expect("gap delivery")
    }

    #[test]
    fn nonce_encoding_is_canonical_unpadded_base64url() {
        assert_eq!(base64url_nonce([0; 16]), "AAAAAAAAAAAAAAAAAAAAAA");
        assert!(valid_nonce("AAAAAAAAAAAAAAAAAAAAAA"));
        assert!(!valid_nonce("AAAAAAAAAAAAAAAAAAAAAA="));
    }

    #[test]
    fn durable_admission_precedes_upstream_acknowledgement_and_deduplicates_replay() {
        let root = temporary("commit-order");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let acknowledged = Arc::new(Mutex::new(Vec::new()));
        let mut backend = Backend {
            items: Some(VecDeque::from([SubscriptionItem::Batch(delivery(
                1,
                "cursor-1",
                "receipt-1",
            ))])),
            acknowledged: Arc::clone(&acknowledged),
            next_calls: Arc::new(AtomicU64::new(0)),
            fail_ack: false,
            sabotage_receipt_directory: None,
        };
        let request = state.subscribe_request().expect("request");
        let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribe");

        let ConsumedItem::Batch(admission) =
            consume_one(&mut subscription, &state).expect("consume")
        else {
            panic!("expected batch");
        };
        assert_eq!(admission.new_request_keys.len(), 1);
        assert_eq!(&*acknowledged.lock().expect("ack lock"), &["receipt-1"]);
        assert_eq!(state.cursor().expect("cursor").as_deref(), Some("cursor-1"));
        let status = state.status().expect("status");
        assert_eq!(status["request_count"], 1);
        assert_eq!(
            status["runtime_evidence"]["latest_commit_receipt"]["phase"],
            "committed"
        );
        assert_eq!(status["runtime_evidence"]["durable_batch_verified"], true);

        let reopened = BridgeState::open(&root).expect("reopen state");
        let replay = delivery(1, "cursor-1", "receipt-replay");
        let admission = reopened.admit_batch(&replay).expect("deduplicate replay");
        assert!(admission.new_request_keys.is_empty());
        let status = reopened.status().expect("status");
        assert_eq!(status["request_count"], 1);
        assert_eq!(
            status["runtime_evidence"]["latest_commit_receipt"]["phase"],
            "prepared"
        );
        assert_eq!(status["runtime_evidence"]["durable_batch_verified"], false);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unresolved_gap_is_journaled_before_refusal_and_never_acknowledged_or_reconnected() {
        let root = temporary("gap-fail-closed");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let acknowledged = Arc::new(Mutex::new(Vec::new()));
        let next_calls = Arc::new(AtomicU64::new(0));
        let mut initial_backend = Backend {
            items: Some(VecDeque::from([SubscriptionItem::Batch(delivery(
                1,
                "cursor-safe",
                "delivery-safe",
            ))])),
            acknowledged: Arc::clone(&acknowledged),
            next_calls: Arc::clone(&next_calls),
            fail_ack: false,
            sabotage_receipt_directory: None,
        };
        let request = state.subscribe_request().expect("initial request");
        let mut initial =
            ChatSubscription::open(&mut initial_backend, &request).expect("initial subscribe");
        consume_one(&mut initial, &state).expect("commit safe boundary");
        drop(initial);

        let later = delivery(2, "cursor-later", "delivery-later");
        let mut gap_backend = Backend {
            items: Some(VecDeque::from([
                SubscriptionItem::Batch(gap_delivery(1, "cursor-gap", "delivery-gap")),
                SubscriptionItem::Batch(later),
            ])),
            acknowledged: Arc::clone(&acknowledged),
            next_calls: Arc::clone(&next_calls),
            fail_ack: false,
            sabotage_receipt_directory: None,
        };
        let request = state.subscribe_request().expect("safe-cursor request");
        assert_eq!(
            request.resume_from().map(ProviderCursor::as_str),
            Some("cursor-safe")
        );
        let mut subscription =
            ChatSubscription::open(&mut gap_backend, &request).expect("gap subscribe");
        let error = consume_one(&mut subscription, &state).expect_err("gap must fail closed");
        assert!(matches!(error, ChatRuntimeError::UnresolvedGap(_)));
        assert_eq!(next_calls.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(&*acknowledged.lock().expect("ack lock"), &["delivery-safe"]);
        assert_eq!(
            state.cursor().expect("cursor").as_deref(),
            Some("cursor-safe")
        );
        let status = state.status().expect("degraded status");
        assert_eq!(status["runtime_evidence"]["connected_now"], false);
        assert_eq!(status["runtime_evidence"]["healthy"], false);
        assert_eq!(status["runtime_evidence"]["durable_batch_verified"], false);
        assert_eq!(status["unresolved_gap"]["phase"], "unresolved");
        assert!(state.subscribe_request().is_err());
        let reopened = BridgeState::open(&root).expect("inspect degraded state");
        assert!(matches!(
            reopened.subscribe_request(),
            Err(ChatRuntimeError::UnresolvedGap(_))
        ));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn missing_exact_commit_confirmation_retains_prepared_receipt_and_stops_receive() {
        let root = temporary("commit-confirmation-missing");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let acknowledged = Arc::new(Mutex::new(Vec::new()));
        let next_calls = Arc::new(AtomicU64::new(0));
        let mut backend = Backend {
            items: Some(VecDeque::from([
                SubscriptionItem::Batch(delivery(1, "cursor-1", "delivery-1")),
                SubscriptionItem::Batch(delivery(2, "cursor-2", "delivery-2")),
            ])),
            acknowledged,
            next_calls: Arc::clone(&next_calls),
            fail_ack: true,
            sabotage_receipt_directory: None,
        };
        let request = state.subscribe_request().expect("request");
        let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribe");
        assert!(consume_one(&mut subscription, &state).is_err());
        assert_eq!(next_calls.load(AtomicOrdering::SeqCst), 1);
        let status = state.status().expect("status");
        assert_eq!(
            status["runtime_evidence"]["latest_commit_receipt"]["phase"],
            "prepared"
        );
        assert_eq!(status["runtime_evidence"]["durable_batch_verified"], false);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn receipt_write_failure_after_exact_commit_stops_before_second_receive() {
        let root = temporary("commit-receipt-write-failure");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let acknowledged = Arc::new(Mutex::new(Vec::new()));
        let next_calls = Arc::new(AtomicU64::new(0));
        let receipt_directory = root.join("commit-receipts");
        let mut backend = Backend {
            items: Some(VecDeque::from([
                SubscriptionItem::Batch(delivery(1, "cursor-1", "delivery-1")),
                SubscriptionItem::Batch(delivery(2, "cursor-2", "delivery-2")),
            ])),
            acknowledged: Arc::clone(&acknowledged),
            next_calls: Arc::clone(&next_calls),
            fail_ack: false,
            sabotage_receipt_directory: Some(receipt_directory.clone()),
        };
        let request = state.subscribe_request().expect("request");
        let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribe");
        assert!(consume_one(&mut subscription, &state).is_err());
        assert_eq!(next_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(&*acknowledged.lock().expect("ack lock"), &["delivery-1"]);
        fs::set_permissions(&receipt_directory, fs::Permissions::from_mode(0o700))
            .expect("restore receipt directory");
        let status = state.status().expect("status");
        assert_eq!(
            status["runtime_evidence"]["latest_commit_receipt"]["phase"],
            "prepared"
        );
        assert_eq!(status["runtime_evidence"]["durable_batch_verified"], false);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn maximum_message_batch_remains_restartable_after_provider_acknowledgement() {
        let root = temporary("maximum-active-routes");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let acknowledged = Arc::new(Mutex::new(Vec::new()));
        let checkpoint = DeliveryBatch::new(
            EventSequence::new(1).expect("sequence"),
            ProviderCursor::new("cursor-boundary").expect("cursor"),
            DeliveryId::new("receipt-boundary").expect("receipt"),
            vec![CommittableEvent::Checkpoint],
        )
        .expect("checkpoint delivery");
        let maximum = maximum_message_delivery(2, "cursor-maximum", "receipt-maximum");
        let mut backend = Backend {
            items: Some(VecDeque::from([
                SubscriptionItem::Batch(checkpoint),
                SubscriptionItem::Batch(maximum),
            ])),
            acknowledged: Arc::clone(&acknowledged),
            next_calls: Arc::new(AtomicU64::new(0)),
            fail_ack: false,
            sabotage_receipt_directory: None,
        };
        let request = state.subscribe_request().expect("request");
        let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribe");
        assert!(matches!(
            consume_one(&mut subscription, &state).expect("consume boundary"),
            ConsumedItem::Batch(_)
        ));
        let ConsumedItem::Batch(admission) =
            consume_one(&mut subscription, &state).expect("consume maximum batch")
        else {
            panic!("expected maximum batch");
        };
        assert_eq!(admission.new_request_keys.len(), 256);
        assert_eq!(
            &*acknowledged.lock().expect("ack lock"),
            &["receipt-boundary", "receipt-maximum"]
        );
        assert_eq!(
            state.active_reply_routes().expect("active routes").len(),
            256
        );
        drop(subscription);

        let recovered = BridgeState::open(&root).expect("restart state");
        assert_eq!(
            recovered
                .active_reply_routes()
                .expect("recovered routes")
                .len(),
            256
        );
        assert_eq!(
            recovered
                .available_reply_ids()
                .expect("available ids")
                .len(),
            256
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn completed_explicitly_closed_requests_churn_beyond_population_cap_without_replay() {
        let root = temporary("retirement-churn");
        let mut configuration = config();
        configuration.outbound_enabled = false;
        configuration.ack_reaction = None;
        let state = BridgeState::initialize(&root, configuration).expect("initialize state");
        let delivery = FakeDelivery::default();
        let mut admitted = Vec::new();
        for index in 1..=MAX_REQUESTS.saturating_add(2) {
            let batch = indexed_delivery(index, index);
            let key = state
                .admit_batch(&batch)
                .expect("admit through retirement churn")
                .new_request_keys
                .into_iter()
                .next()
                .expect("new request key");
            *delivery.queue_state.lock().expect("queue state") = None;
            assert_eq!(
                deliver_request_with(&state, &delivery, &key, DrainOptions::default())
                    .expect("deliver request"),
                CoordinatorDeliveryResult::Delivered
            );
            state.close_replies(&key).expect("explicit close");
            admitted.push((index, key));
        }
        let status = state.status().expect("status");
        assert!(status["request_count"].as_u64().expect("count") <= MAX_REQUESTS);
        assert!(status["retired_route_count"].as_u64().expect("retired") > 0);
        let (retired_index, retired_key) = admitted
            .iter()
            .find(|(_, key)| state.retired_key(key).expect("retired guard").is_some())
            .expect("at least one request was retired");
        let retired = state
            .retired_key(retired_key)
            .expect("retired guard")
            .expect("retired index");
        let stale = state
            .capture_snapshot(&format!(
                "<CHAT_REPLY_{}_1>\nstale\n</CHAT_REPLY_{}_1>",
                retired.reply_nonce, retired.reply_nonce
            ))
            .expect("retired marker is recognized");
        assert!(stale.replies.is_empty());
        assert!(stale.unknown_ids.is_empty());
        let cursor_before = state.cursor().expect("cursor before replay");
        let regression = indexed_delivery(*retired_index, *retired_index);
        assert!(state
            .admit_batch(&regression)
            .expect_err("old cursor replay must fail closed")
            .to_string()
            .contains("regressed"));
        assert_eq!(state.cursor().expect("cursor after refusal"), cursor_before);

        let replay = indexed_delivery_at(
            MAX_REQUESTS.saturating_add(3),
            *retired_index,
            "cursor-replay-inclusive",
            &format!("request {retired_index}"),
        );
        assert!(state
            .admit_batch(&replay)
            .expect("deduplicate recent replay at a forward cursor")
            .new_request_keys
            .is_empty());
        assert!(state.retired_key(retired_key).expect("guard").is_some());
        let mismatch = indexed_delivery_at(
            MAX_REQUESTS.saturating_add(4),
            *retired_index,
            "cursor-mismatched-replay",
            "different content for the same provider message",
        );
        assert!(state.admit_batch(&mismatch).is_err());
        let reopened = BridgeState::open(&root).expect("restart after churn");
        assert!(reopened.retired_key(retired_key).expect("guard").is_some());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retirement_refuses_every_nonterminal_delivery_ack_route_and_reply_state() {
        for (name, phase) in [
            ("retire-pending", RequestPhase::Pending),
            ("retire-submitting", RequestPhase::Submitting),
            ("retire-uncertain", RequestPhase::DeliveryUncertain),
        ] {
            let (state, key, root) = state_with_old_request(name, config_without_reaction());
            state
                .set_delivery_phase(&key, phase, Some("fixture nonterminal state"))
                .expect("set phase");
            state.close_replies(&key).expect("close route");
            assert!(!attempt_retirement(&state, &key), "{name}");
            fs::remove_dir_all(root).expect("cleanup");
        }

        for (name, ack_phase) in [
            ("retire-ack-pending", AckPhase::Pending),
            ("retire-ack-sending", AckPhase::Sending),
        ] {
            let (state, key, root) = state_with_old_request(name, config());
            state
                .set_delivery_phase(&key, RequestPhase::Delivered, None)
                .expect("mark delivered");
            let lock = agent::open_private_lock(&state.root.join(".state.lock"), "test state lock")
                .expect("open state lock");
            lock.lock_exclusive().expect("lock state");
            let mut request = state.read_request(&key).expect("request");
            request.ack_phase = ack_phase;
            let mut checkpoint = state.read_checkpoint().expect("checkpoint");
            state
                .write_request_accounted(&request, &mut checkpoint)
                .expect("persist ack phase");
            write_document(&state.root.join("checkpoint.json"), &checkpoint)
                .expect("persist checkpoint");
            drop(lock);
            state.close_replies(&key).expect("close route");
            assert!(!attempt_retirement(&state, &key), "{name}");
            fs::remove_dir_all(root).expect("cleanup");
        }

        let (state, key, root) =
            state_with_old_request("retire-open-route", config_without_reaction());
        state
            .set_delivery_phase(&key, RequestPhase::Delivered, None)
            .expect("mark delivered");
        assert!(!attempt_retirement(&state, &key));
        fs::remove_dir_all(root).expect("cleanup");

        let (state, key, root) =
            state_with_old_request("retire-unsent-reply", config_without_reaction());
        state
            .set_delivery_phase(&key, RequestPhase::Delivered, None)
            .expect("mark delivered");
        let route = state
            .next_reply_route(&key)
            .expect("route")
            .expect("open route");
        state
            .capture_replies(
                &key,
                &format!(
                    "<CHAT_REPLY_{}>\nunsent\n</CHAT_REPLY_{}>",
                    route.identifier, route.identifier
                ),
            )
            .expect("capture unsent reply");
        state.close_replies(&key).expect("close route");
        assert!(!attempt_retirement(&state, &key));
        let mut sending = state.read_reply(&key, 1).expect("pending reply");
        sending.phase = ReplyPhase::Sending;
        write_document(&state.reply_path(&key, 1), &sending).expect("persist sending reply");
        assert!(!attempt_retirement(&state, &key));
        fs::remove_dir_all(root).expect("cleanup");

        let root = temporary("retire-current-boundary");
        let state = BridgeState::initialize(&root, config_without_reaction()).expect("state");
        let key = state
            .admit_batch(&indexed_delivery(1, 1))
            .expect("admit")
            .new_request_keys
            .remove(0);
        state
            .set_delivery_phase(&key, RequestPhase::Delivered, None)
            .expect("mark delivered");
        state.close_replies(&key).expect("close route");
        assert!(!attempt_retirement(&state, &key));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn restart_completes_retirement_after_each_delete_crash_boundary_exactly_once() {
        for (boundary, restore_request, restore_reply) in [
            ("after-journal", true, true),
            ("mid-reply-delete", true, false),
            ("after-request-delete", false, false),
        ] {
            let (state, key, root) = state_with_old_request(
                &format!("retirement-crash-{boundary}"),
                config_without_reaction(),
            );
            state
                .set_delivery_phase(&key, RequestPhase::Delivered, None)
                .expect("mark delivered");
            let route = state
                .next_reply_route(&key)
                .expect("route")
                .expect("open route");
            state
                .capture_replies(
                    &key,
                    &format!(
                        "<CHAT_REPLY_{}>\naudit reply\n</CHAT_REPLY_{}>",
                        route.identifier, route.identifier
                    ),
                )
                .expect("capture reply");
            let mut transport = FakeReplyTransport::default();
            state
                .publish_one(&key, &mut transport)
                .expect("publish reply")
                .expect("provider receipt");
            let request = state.read_request(&key).expect("request before retirement");
            let reply = state.read_reply(&key, 1).expect("reply before retirement");
            state.close_replies(&key).expect("retire request");
            assert!(state.retired_key(&key).expect("retired key").is_some());
            let retirement_path = state.retirement_path(1);
            let mut retirement: RetirementRecord =
                read_document(&retirement_path, MAX_RETIREMENT_RECORD_BYTES)
                    .expect("retirement record");
            assert_eq!(retirement.phase, RetirementPhase::Retired);
            assert_eq!(retirement.replies[0].send_request_id, reply.send_request_id);
            assert_eq!(
                retirement.replies[0].provider_message_id,
                reply.provider_message_id.clone().expect("reply receipt")
            );
            retirement.phase = RetirementPhase::Prepared;
            retirement.retired_at_millis = None;
            retirement.validate().expect("prepared journal");
            write_document(&retirement_path, &retirement).expect("plant crash journal");
            if restore_request {
                write_document(&state.request_path(&key), &request).expect("restore request");
            }
            if restore_reply {
                write_document(&state.reply_path(&key, 1), &reply).expect("restore reply");
            }

            let reopened = BridgeState::open(&root).expect("complete retirement on restart");
            assert!(!reopened.request_path(&key).exists());
            assert!(!reopened.reply_path(&key, 1).exists());
            let completed: RetirementRecord =
                read_document(&retirement_path, MAX_RETIREMENT_RECORD_BYTES)
                    .expect("completed retirement");
            assert_eq!(completed.phase, RetirementPhase::Retired);
            assert!(reopened.retired_key(&key).expect("retired guard").is_some());
            assert_eq!(reopened.status().expect("status")["request_count"], 1);
            assert_eq!(
                reopened
                    .admit_batch(&indexed_delivery(3, 100))
                    .expect("admit after recovery")
                    .new_request_keys
                    .len(),
                1
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn recovery_snapshot_routes_across_full_active_batch_without_route_cap() {
        let root = temporary("maximum-snapshot-routes");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&maximum_message_delivery(
                1,
                "cursor-maximum",
                "receipt-maximum",
            ))
            .expect("admit maximum batch");
        assert_eq!(admission.new_request_keys.len(), 256);
        let routes = state.active_reply_routes().expect("active routes");
        let selected = [&routes[0], &routes[255]];
        let rendered = selected
            .iter()
            .enumerate()
            .map(|(index, route)| {
                format!(
                    "<CHAT_REPLY_{}>\nreply {index}\n</CHAT_REPLY_{}>",
                    route.identifier, route.identifier
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let capture = state
            .capture_snapshot(&rendered)
            .expect("capture maximum-route snapshot");
        assert_eq!(capture.replies.len(), 2);
        assert!(capture.unknown_ids.is_empty());
        assert_eq!(state.read_checkpoint().expect("checkpoint").reply_count, 2);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn startup_repairs_population_after_precheckpoint_crash() {
        let root = temporary("repair");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let batch = delivery(1, "cursor-1", "receipt-1");
        let CommittableEvent::MessageCreated(message) = &batch.events()[1] else {
            panic!("message fixture");
        };
        let record = RequestRecord::from_message(message, Some("🤖")).expect("record");
        write_document(&state.request_path(&record.key), &record).expect("orphan request");
        assert_eq!(
            state.read_checkpoint().expect("checkpoint").request_count,
            0
        );

        let recovered = BridgeState::open(&root).expect("recover state");
        let checkpoint = recovered.read_checkpoint().expect("checkpoint");
        assert_eq!(checkpoint.request_count, 1);
        assert!(checkpoint.request_bytes > 0);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn runner_lease_is_exclusive() {
        let root = temporary("lease");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let lease = state.acquire_runner_lease().expect("first lease");
        let error = state
            .acquire_runner_lease()
            .expect_err("second lease refused");
        assert!(error.to_string().contains("another chat runtime owns"));
        drop(lease);
        state.acquire_runner_lease().expect("lease released");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn delivery_uses_stable_id_and_generic_multi_reply_fence() {
        let root = temporary("deliver");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let target = FakeDelivery::default();
        assert_eq!(
            deliver_request_with(&state, &target, key, DrainOptions::default())
                .expect("deliver request"),
            CoordinatorDeliveryResult::Delivered
        );
        let prompts = target.submitted_prompts.lock().expect("prompt lock");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("one or multiple replies"));
        assert!(prompts[0].contains("<CHAT_REPLY_"));
        assert!(!prompts[0].contains("GCHAT_REPLY"));
        assert_eq!(
            state.read_request(key).expect("request").phase,
            RequestPhase::Delivered
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn submitting_recovery_never_replays_failed_queue_artifact() {
        let root = temporary("uncertain");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        state
            .set_delivery_phase(key, RequestPhase::Submitting, None)
            .expect("record intent");
        let target = FakeDelivery::default();
        *target.queue_state.lock().expect("queue state lock") = Some(QueueMessageState::Failed);
        assert!(matches!(
            deliver_request_with(&state, &target, key, DrainOptions::default())
                .expect("reconcile request"),
            CoordinatorDeliveryResult::Uncertain(_)
        ));
        assert!(target
            .submitted_prompts
            .lock()
            .expect("prompt lock")
            .is_empty());
        assert_eq!(
            state.read_request(key).expect("request").phase,
            RequestPhase::DeliveryUncertain
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retained_pending_queue_is_drained_without_duplicate_submit() {
        let root = temporary("pending");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let target = FakeDelivery::default();
        *target.queue_state.lock().expect("queue state lock") = Some(QueueMessageState::Pending);
        assert_eq!(
            deliver_request_with(&state, &target, key, DrainOptions::default())
                .expect("drain request"),
            CoordinatorDeliveryResult::Delivered
        );
        assert_eq!(*target.drains.lock().expect("drain lock"), 1);
        assert!(target
            .submitted_prompts
            .lock()
            .expect("prompt lock")
            .is_empty());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn capture_ignores_fenced_examples_and_retains_consecutive_multi_replies() {
        let root = temporary("capture");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let nonce = state.read_request(key).expect("request").reply_nonce;
        let rendered = format!(
            "```text\n<CHAT_REPLY_{nonce}_1>\nignored\n</CHAT_REPLY_{nonce}_1>\n```\n\
<CHAT_REPLY_not-available>\nignored unknown\n</CHAT_REPLY_not-available>\n\
<CHAT_REPLY_{nonce}_1>\nfirst reply\n</CHAT_REPLY_{nonce}_1>\n\
<CHAT_REPLY_{nonce}_2>\nsecond reply\n</CHAT_REPLY_{nonce}_2>"
        );
        let captured = state
            .capture_replies(key, &rendered)
            .expect("capture replies");
        assert_eq!(captured.ordinals, vec![1, 2]);
        assert_eq!(captured.unknown_ids, vec!["not-available"]);
        assert_eq!(
            state.read_reply(key, 1).expect("first reply").body,
            "first reply"
        );
        assert_eq!(
            state.read_reply(key, 2).expect("second reply").body,
            "second reply"
        );
        assert_eq!(state.read_checkpoint().expect("checkpoint").reply_count, 2);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn one_snapshot_parse_routes_blocks_for_many_active_nonces() {
        let first = "AAAAAAAAAAAAAAAAAAAAAA";
        let second = "BBBBBBBBBBBBBBBBBBBBBB";
        let expected = BTreeSet::from([first.to_owned(), second.to_owned()]);
        let rendered = format!(
            "```text\n<CHAT_REPLY_{first}_1>\nignored\n</CHAT_REPLY_{first}_1>\n```\n\
<CHAT_REPLY_{first}_1>\nfirst\n</CHAT_REPLY_{first}_1>\n\
<CHAT_REPLY_{second}_1>\nsecond\n</CHAT_REPLY_{second}_1>\n\
<CHAT_REPLY_unknown_1>\nunknown\n</CHAT_REPLY_unknown_1>"
        );
        let scan = scan_reply_blocks_for_nonces(&rendered, &expected).expect("scan once");
        assert_eq!(scan.blocks_by_nonce.len(), 2);
        assert_eq!(scan.blocks_by_nonce[first][0].body, "first");
        assert_eq!(scan.blocks_by_nonce[second][0].body, "second");
        assert_eq!(scan.observed_ids.len(), 3);
        assert_eq!(scan.unknown_ids, vec!["unknown_1"]);
    }

    #[test]
    fn capture_reserves_agent_label_bytes_before_durable_outbox_admission() {
        let root = temporary("labelled-reply-bound");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let nonce = state.read_request(key).expect("request").reply_nonce;
        let oversized = "x".repeat(MAX_REPLY_BYTES);
        let error = state
            .capture_replies(
                key,
                &format!("<CHAT_REPLY_{nonce}_1>\n{oversized}\n</CHAT_REPLY_{nonce}_1>"),
            )
            .expect_err("label prefix must count toward transport bound");
        assert!(error.to_string().contains("agent-labelled chat reply"));
        assert_eq!(
            state.read_request(key).expect("request").next_reply_ordinal,
            1
        );

        let prefix = "[codex coordinator] ";
        let maximum = "y".repeat(MAX_REPLY_BYTES - prefix.len());
        assert_eq!(
            outbound_text("codex coordinator", &maximum).len(),
            MAX_REPLY_BYTES
        );
        assert_eq!(
            state
                .capture_replies(
                    key,
                    &format!("<CHAT_REPLY_{nonce}_1>\n{maximum}\n</CHAT_REPLY_{nonce}_1>"),
                )
                .expect("maximum labelled reply")
                .ordinals,
            vec![1]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn fence_feedback_bounds_display_without_changing_full_identity_set() {
        let available = (0..(MAX_FEEDBACK_AVAILABLE_IDS + 3))
            .map(|index| format!("reply-{index}"))
            .collect::<Vec<_>>();
        let displayed = format_available_reply_ids(&available);
        assert!(displayed.contains("reply-0"));
        assert!(displayed.contains("reply-31"));
        assert!(!displayed.contains("reply-32"));
        assert!(displayed.contains("and 3 more"));
    }

    #[test]
    fn outbound_retries_use_one_stable_request_id_and_advance_in_order() {
        let root = temporary("outbound");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let nonce = state.read_request(key).expect("request").reply_nonce;
        state
            .capture_replies(
                key,
                &format!("<CHAT_REPLY_{nonce}_1>\nanswer\n</CHAT_REPLY_{nonce}_1>"),
            )
            .expect("capture reply");
        let mut transport = FakeReplyTransport {
            fail_once: true,
            ..FakeReplyTransport::default()
        };
        assert!(state.publish_one(key, &mut transport).is_err());
        assert_eq!(
            state.read_reply(key, 1).expect("sending reply").phase,
            ReplyPhase::Sending
        );
        assert_eq!(
            state
                .publish_one(key, &mut transport)
                .expect("retry send")
                .as_deref(),
            Some("messages/reply-2")
        );
        assert_eq!(transport.submissions.len(), 2);
        assert_eq!(transport.submissions[0].3, transport.submissions[1].3);
        assert_eq!(transport.submissions[0].2, "[codex coordinator] answer");
        assert_eq!(
            state
                .publish_one(key, &mut transport)
                .expect("outbox empty"),
            None
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn inbound_only_mode_disables_reaction_work_and_reply_fences() {
        let root = temporary("inbound-only");
        let mut configuration = config();
        configuration.outbound_enabled = false;
        configuration.ack_reaction = None;
        let state = BridgeState::initialize(&root, configuration).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let prompt = state.prompt(key).expect("render inbound-only prompt");
        assert!(prompt.contains("inbound-only chat bridge"));
        assert!(prompt.contains("do not emit CHAT_REPLY fences"));
        assert!(state.available_reply_ids().expect("reply ids").is_empty());
        assert!(state
            .pending_ack_keys()
            .expect("pending acknowledgements")
            .is_empty());
        assert_eq!(
            state
                .capture_snapshot("<CHAT_REPLY_unavailable_1>\nno\n</CHAT_REPLY_unavailable_1>")
                .expect("disabled capture"),
            SnapshotCapture {
                replies: Vec::new(),
                unknown_ids: Vec::new(),
                route_entries: Vec::new(),
            }
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn inbound_only_configuration_refuses_outbound_side_effects() {
        let mut configuration = config();
        configuration.outbound_enabled = false;
        assert!(configuration.validate().is_err());
    }

    #[test]
    fn startup_repairs_reply_counters_and_send_pointer() {
        let root = temporary("reply-repair");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let reply =
            ReplyRecord::new(key, 1, "orphaned before summary update".to_owned()).expect("reply");
        write_document(&state.reply_path(key, 1), &reply).expect("orphan reply");

        let recovered = BridgeState::open(&root).expect("recover state");
        let request = recovered.read_request(key).expect("request");
        assert_eq!(request.reply_count, 1);
        assert_eq!(request.next_reply_ordinal, 2);
        assert_eq!(request.next_send_ordinal, 1);
        assert_eq!(
            recovered.read_checkpoint().expect("checkpoint").reply_count,
            1
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn acknowledgement_is_after_admission_and_retries_one_stable_uuid() {
        let root = temporary("ack");
        let state = BridgeState::initialize(&root, config()).expect("initialize state");
        let admission = state
            .admit_batch(&delivery(1, "cursor-1", "receipt-1"))
            .expect("admit request");
        let key = &admission.new_request_keys[0];
        let mut transport = FakeReactionTransport {
            fail_once: true,
            ..FakeReactionTransport::default()
        };
        assert!(state.ensure_ack(key, &mut transport).is_err());
        assert_eq!(
            state.read_request(key).expect("request").ack_phase,
            AckPhase::Sending
        );
        assert_eq!(
            state
                .ensure_ack(key, &mut transport)
                .expect("retry acknowledgement"),
            AckResult::Acked(ReactionReceipt {
                reaction_id: "spaces/example/messages/one/reactions/robot".to_owned(),
                already_present: true,
            })
        );
        assert_eq!(transport.submissions.len(), 2);
        assert_eq!(transport.submissions[0].3, transport.submissions[1].3);
        assert!(valid_operation_uuid(&transport.submissions[0].3));
        assert_eq!(
            state
                .ensure_ack(key, &mut transport)
                .expect("already acknowledged"),
            AckResult::Acked(ReactionReceipt {
                reaction_id: "spaces/example/messages/one/reactions/robot".to_owned(),
                already_present: true,
            })
        );
        assert_eq!(transport.submissions.len(), 2);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn outbound_wire_binds_success_to_exact_uuid_action_and_authority() {
        let send = ReplySubmission {
            channel_id: "spaces/example",
            thread_id: "spaces/example/threads/one",
            body: "hello",
            request_id: "123e4567-e89b-42d3-a456-426614174000",
        };
        let encoded = encode_send_request(&send).expect("encode send");
        assert!(encoded.ends_with(b"\n"));
        let document: Value = serde_json::from_slice(&encoded).expect("request JSON");
        assert_eq!(
            document,
            serde_json::json!({
                "version": 1,
                "id": send.request_id,
                "action": "send",
                "channel_id": send.channel_id,
                "thread_id": send.thread_id,
                "text": send.body,
            })
        );
        let response = br#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":true,"receipt":{"message_id":"spaces/example/messages/reply"}}"#;
        assert_eq!(
            decode_send_response(response, &send).expect("decode send"),
            "spaces/example/messages/reply"
        );
        let outside = br#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":true,"receipt":{"message_id":"spaces/other/messages/reply"}}"#;
        assert_eq!(
            decode_send_response(outside, &send)
                .expect_err("outside authority")
                .outcome,
            OutboundOutcome::Unknown
        );

        let reaction = ReactionSubmission {
            channel_id: "spaces/example",
            message_id: "spaces/example/messages/source",
            emoji: "🤖",
            request_id: "123e4567-e89b-42d3-a456-426614174001",
        };
        let reaction_response = br#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174001","action":"ensure_reaction","ok":true,"receipt":{"reaction_id":"spaces/example/messages/source/reactions/robot","already_present":true}}"#;
        assert_eq!(
            decode_reaction_response(reaction_response, &reaction).expect("decode reaction"),
            ReactionReceipt {
                reaction_id: "spaces/example/messages/source/reactions/robot".to_owned(),
                already_present: true,
            }
        );
    }

    #[test]
    fn outbound_wire_preserves_provider_failure_evidence_and_rejects_unbound_frames() {
        let send = ReplySubmission {
            channel_id: "spaces/example",
            thread_id: "spaces/example/threads/one",
            body: "hello",
            request_id: "123e4567-e89b-42d3-a456-426614174000",
        };
        let failure = br#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":false,"error":{"code":"quota","detail":"try later","outcome":"not_applied","retryable":true}}"#;
        assert_eq!(
            decode_send_response(failure, &send).expect_err("provider failure"),
            OutboundFailure {
                code: "quota".to_owned(),
                detail: "try later".to_owned(),
                outcome: OutboundOutcome::NotApplied,
                retryable: true,
            }
        );
        let parser_failure = br#"{"version":1,"id":null,"action":null,"ok":false,"error":{"code":"invalid_json","detail":"bad input","outcome":"not_applied","retryable":false}}"#;
        assert_eq!(
            decode_send_response(parser_failure, &send)
                .expect_err("unbound parser failure")
                .outcome,
            OutboundOutcome::Unknown
        );
        let duplicate = br#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":true,"ok":false,"receipt":{"message_id":"spaces/example/messages/reply"}}"#;
        assert_eq!(
            decode_send_response(duplicate, &send)
                .expect_err("duplicate key")
                .outcome,
            OutboundOutcome::Unknown
        );
    }

    #[test]
    fn command_outbound_uses_reviewed_supervisor_and_exact_one_line_protocol() {
        let root = temporary("command-outbound");
        let helper = copied_shell(&root);
        let response = r#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":true,"receipt":{"message_id":"spaces/example/messages/reply"}}"#;
        let script = format!("IFS= read -r request || exit 2; printf '%s\\n' '{response}'");
        let mut transport = CommandOutboundTransport::new(
            helper,
            vec![OsString::from("-c"), OsString::from(script)],
            &[],
            Duration::from_secs(2),
            Duration::from_millis(50),
        )
        .expect("pin helper");
        assert_eq!(
            transport
                .send(ReplySubmission {
                    channel_id: "spaces/example",
                    thread_id: "spaces/example/threads/one",
                    body: "hello",
                    request_id: "123e4567-e89b-42d3-a456-426614174000",
                })
                .expect("send through helper"),
            "spaces/example/messages/reply"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn command_outbound_uses_sealed_generation_image_after_source_removal() {
        let root = temporary("command-identity");
        let helper = copied_shell(&root);
        let response = r#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":true,"receipt":{"message_id":"spaces/example/messages/pinned"}}"#;
        let script = format!("IFS= read -r request || exit 2; printf '%s\\n' '{response}'");
        let mut transport = CommandOutboundTransport::new(
            helper.clone(),
            vec![OsString::from("-c"), OsString::from(script)],
            &[],
            Duration::from_secs(2),
            Duration::ZERO,
        )
        .expect("pin helper");
        fs::remove_file(&helper).expect("remove source after generation pin");
        assert_eq!(
            transport
                .send(ReplySubmission {
                    channel_id: "spaces/example",
                    thread_id: "spaces/example/threads/one",
                    body: "hello",
                    request_id: "123e4567-e89b-42d3-a456-426614174000",
                })
                .expect("execute sealed generation image"),
            "spaces/example/messages/pinned"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn command_outbound_refuses_executable_above_the_hashing_bound() {
        let root = temporary("command-size");
        let helper = copied_shell(&root);
        fs::OpenOptions::new()
            .write(true)
            .open(&helper)
            .expect("open helper")
            .set_len(MAX_OUTBOUND_EXECUTABLE_BYTES + 1)
            .expect("extend helper fixture");
        let error = CommandOutboundTransport::new(
            helper,
            Vec::new(),
            &[],
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .err()
        .expect("oversized helper refused");
        assert!(error.to_string().contains("at most 64 MiB"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn command_outbound_cancellation_interrupts_blocked_helper_and_descendants() {
        let root = temporary("command-cancellation");
        let helper = copied_shell(&root);
        let mut transport = CommandOutboundTransport::new(
            helper,
            vec![
                OsString::from("-c"),
                OsString::from("IFS= read -r request; /bin/sleep 30"),
            ],
            &[],
            Duration::from_secs(30),
            Duration::ZERO,
        )
        .expect("pin helper");
        let cancellation = OutboundCancellation::new().expect("create cancellation");
        transport.set_cancellation(cancellation.clone());
        let started = Instant::now();
        let worker = std::thread::spawn(move || transport.exchange(b"{}\n"));
        std::thread::sleep(Duration::from_millis(50));
        cancellation.cancel();
        let error = worker
            .join()
            .expect("join helper operation")
            .expect_err("cancel helper operation");
        assert_eq!(error.outcome, OutboundOutcome::Unknown);
        assert!(error.detail.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(5));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn command_outbound_cancellation_interrupts_shutdown_after_response_eof() {
        let root = temporary("command-cancel-after-eof");
        let helper = copied_shell(&root);
        let ready = root.join("stdout-closed");
        let response = r#"{"version":1,"id":"123e4567-e89b-42d3-a456-426614174000","action":"send","ok":true,"receipt":{"message_id":"spaces/example/messages/late"}}"#;
        let script = format!(
            "IFS= read -r request; printf '%s\\n' '{response}'; exec 1>&-; : > '{}'; /bin/sleep 30",
            ready.display()
        );
        let mut transport = CommandOutboundTransport::new(
            helper,
            vec![OsString::from("-c"), OsString::from(script)],
            &[],
            Duration::from_secs(30),
            Duration::from_secs(30),
        )
        .expect("pin helper");
        let cancellation = OutboundCancellation::new().expect("create cancellation");
        transport.set_cancellation(cancellation.clone());
        let started = Instant::now();
        let worker = std::thread::spawn(move || transport.exchange(b"{}\n"));
        while !ready.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.exists(), "helper closed stdout before cancellation");
        cancellation.cancel();
        let error = worker
            .join()
            .expect("join helper operation")
            .expect_err("post-EOF cancellation is not success");
        assert_eq!(error.code, "helper_cancelled");
        assert_eq!(error.outcome, OutboundOutcome::Unknown);
        assert!(started.elapsed() < Duration::from_secs(5));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn command_outbound_deadline_kills_descendants_without_detaching() {
        let root = temporary("command-timeout");
        let helper = copied_shell(&root);
        let transport = CommandOutboundTransport::new(
            helper,
            vec![
                OsString::from("-c"),
                OsString::from("IFS= read -r request; /bin/sleep 10"),
            ],
            &[],
            Duration::from_millis(50),
            Duration::ZERO,
        )
        .expect("pin helper");
        let started = Instant::now();
        let error = transport
            .exchange(b"{}\n")
            .expect_err("blocked helper times out");
        assert_eq!(error.outcome, OutboundOutcome::Unknown);
        assert!(started.elapsed() < Duration::from_secs(5));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
