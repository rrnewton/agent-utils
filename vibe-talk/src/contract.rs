//! The public wire contract: the JSON this server, its browser page, and `vibe-talk-v1` peers
//! exchange.
//!
//! Rust is the authority. Every type here derives `serde` for the bytes that really cross the wire
//! and [`JsonSchema`] for a description of those same bytes, and [`schema_document`] collects the
//! contract's roots into one JSON Schema document. That document is checked in at
//! `contract/vibe-talk.schema.json`; the browser's TypeScript declarations and its runtime
//! validators are generated from it (`make -C vibe-talk contract`), and `tests/contract.rs` fails
//! when the checked-in copy no longer matches these types. So a renamed field, a field made
//! optional, or a new union variant is a reviewable diff in three generated files rather than a
//! property the page silently stops finding.
//!
//! The schema describes what a SENDER emits (`schemars`' serialize contract): a field serde always
//! writes is required, and one it skips when empty is optional. Unknown properties are permitted
//! throughout, so adding a field stays backward compatible; removing one, making one optional,
//! or changing a tag is a protocol change. So is a new value of an enum such as [`TokenScope`],
//! [`LiveDelivery`], or [`TranscriptRole`]: a page loaded before it refuses every answer or frame
//! that carries it. That matters most for [`TranscriptRole`], which peers outside this repository
//! send. A new `vibe-talk-v1` frame TYPE is the exception, because a page ignores a type it does
//! not know.
//!
//! Some contract types live beside the code that builds them — [`crate::model::Message`],
//! [`crate::threads::ThreadSummary`], [`crate::conversation::VoiceSession`] — and are part of the
//! contract because a root here reaches them. The HTTP bodies the page decodes, the live stream's
//! event payloads, and the `vibe-talk-v1` frames are defined here, where the whole public surface
//! can be read in one place.

use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{ChannelId, ChannelInfo, Message, MessageId};

/// A JSON error body.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiErrorBody {
    /// Stable machine-readable code.
    pub error: &'static str,
    /// Human-readable detail. Never contains a secret.
    pub detail: String,
}

/// What the web app needs to know at startup.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ClientConfigResponse {
    /// Source chat service's human-readable name, supplied by the selected backend.
    pub chat_provider_name: String,
    /// Configured channels.
    pub channels: Vec<ChannelInfo>,
    /// ElevenLabs agent id, when the deployment has one. Not a secret: it identifies a public
    /// widget. The API key never leaves the server.
    pub elevenlabs_agent_id: Option<String>,
    /// Selected conversational voice provider, named by its implementation.
    pub conversational_voice: crate::conversation::VoiceDescription,
    /// Selected read-aloud backend and the playback interface the browser should use.
    pub read_aloud: crate::speech::Description,
    /// Server version, so a stale cached page is visible.
    pub version: &'static str,
    /// Whether the operator has allowed an earlier transcript to be replayed into a new call.
    ///
    /// The page needs it BEFORE it has anything to replay: the Settings screen has to describe
    /// what resuming will do, and the control's own note has to say what the next call will be —
    /// both of which are questions asked while there is no conversation to fetch.
    pub replay_enabled: bool,
    /// The Discord account this bridge posts as, when it can be known.
    ///
    /// NOT a secret: a bot's user id is visible on every message it has ever sent. It is here so
    /// the channel view can tell the owner's own words -- posted on his behalf by this bridge --
    /// from everybody else's, on the FIRST render rather than only after he has replied from the
    /// app. `None` when the token is not in the shape ids can be read from; the page then falls
    /// back to learning it the old way.
    pub self_author_id: Option<String>,
    /// The READER's own Discord account, when the operator has said what it is.
    ///
    /// Distinct from `self_author_id`, which is the BRIDGE's account and is read out of the bot
    /// token. This one cannot be derived from anything the server holds — a bot's account has no
    /// relationship to the human reading the channel — so it is absent unless configured. The page
    /// draws both as the owner's own words, because they are: one typed into Discord, one dictated
    /// through this bridge.
    pub owner_author_id: Option<String>,
    /// Seconds between live ingestion ticks, or `0` when live ingestion is OFF.
    ///
    /// The page needs this to tell the truth about its own channel view. With ingestion off the
    /// SSE stream attaches and delivers nothing, which is indistinguishable from a quiet channel;
    /// a page that showed a "live" indicator on that basis would be claiming a freshness it does
    /// not have. Not a secret: it is a property of this deployment, and the caller already holds
    /// a token.
    pub live_poll_seconds: u64,
    /// How changes reach the SSE hub.
    pub live_delivery: LiveDelivery,
    /// Whether the selected provider accepts channel links or references and manages registration.
    pub channel_registration_supported: bool,
    /// Whether the selected provider can list its channels for browsing. `#19 channel-browser`.
    pub channel_discovery_supported: bool,
    /// Whether the selected provider exposes a write-through read cursor.
    pub upstream_read_mark_supported: bool,
    /// Whether the backend supports channel, thread-list, and flattened timelines.
    pub threading_supported: bool,
    /// Whether this server rewrites message bodies for speech before anything says them.
    ///
    /// Here for the same reason as `replay_enabled`: Settings has to be able to describe what this
    /// deployment does, and the page cannot infer this one from any message it holds. An empty
    /// `spoken_content` means "the rewrite changed nothing" just as often as it means "the pass is
    /// off" — deliberately, see [`crate::model::Message::spoken_content`] — so a page guessing from
    /// the data would tell the operator the feature was off every time the channel was plain prose.
    /// They are flipping this switch to compare two runs; guessing is exactly what they cannot do.
    pub speech_prep_enabled: bool,
    /// The scope of the token that asked.
    ///
    /// The caller's own scope, so it discloses nothing it did not already hold. The page needs it
    /// to avoid asking for what this token cannot have: stored conversations are write-only (see
    /// the conversation routes), and a read-scope page that probed them anyway got a 403 and a
    /// console error on every load. `#38 read-token-conversation-probe`.
    pub token_scope: TokenScope,
}

/// A provider-neutral timeline plus this application's channel and read-state metadata.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TimelineResponse {
    /// Configured parent channel, including its local alias and write policy.
    pub channel: ChannelInfo,
    /// View represented by this page.
    pub view: crate::threads::TimelineView,
    /// Page size actually requested.
    pub limit: u16,
    /// Entries on this page, never a channel-wide total.
    pub returned: usize,
    /// Thread and message data, ordered by the backend.
    #[serde(flatten)]
    pub page: crate::threads::TimelinePage,
    /// Messages on this page that the reader has archived locally.
    pub dismissed: Vec<MessageId>,
    /// Channel content remains untrusted data in every view.
    pub untrusted_content_notice: &'static str,
}

/// The scope of the credential that made a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenScope {
    /// May read channel history and summaries.
    Read,
    /// May also post, and may use the write-only conversation routes.
    Write,
}

impl From<crate::auth::Scope> for TokenScope {
    fn from(scope: crate::auth::Scope) -> Self {
        match scope {
            crate::auth::Scope::Read => Self::Read,
            crate::auth::Scope::Write => Self::Write,
        }
    }
}

/// How channel changes reach the live stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LiveDelivery {
    /// Live ingestion is off; the stream attaches and delivers nothing.
    Off,
    /// The server polls the chat provider on an interval.
    Poll,
    /// A provider-neutral adapter pushes changes to the ingest route.
    Push,
}

/// The data of a live stream `message` or `message_update` event.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LiveMessageEvent<'a> {
    /// The message as it now stands. UNTRUSTED: written by whoever is in the channel.
    pub message: &'a Message,
    /// Whether it was classified as history — a replay or a catch-up — rather than news. The page
    /// may render such a message but must not announce it to a live conversation.
    pub replayed: bool,
    /// Whether THIS SERVER posted it; the page must not relay these into a live conversation.
    pub self_posted: bool,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
}

/// The data of a live stream `message_delete` event: only the ids, never the removed text.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LiveDeleteEvent {
    /// Channel the message was removed from.
    pub channel_id: ChannelId,
    /// The message that was removed.
    pub message_id: MessageId,
    /// Whether it was classified as history rather than news.
    pub replayed: bool,
}

/// The data of a live stream `reset` event: the subscriber fell behind the replay tail.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LiveResetEvent {
    /// Events this subscriber missed, or `0` when the count is unknown.
    pub missed: u64,
    /// What the subscriber should do instead of resuming.
    pub detail: &'static str,
}

/// Who spoke a `vibe-talk-v1` transcript line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRole {
    /// The person on the call.
    User,
    /// The voice agent.
    Assistant,
}

/// A JSON control frame a `vibe-talk-v1` server sends. Audio travels separately, as binary frames
/// of 24 kHz, 16-bit little-endian mono PCM.
///
/// A client ignores a `type` it does not know, so a new frame type is backward compatible; a
/// changed field of a known type is not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VibeTalkV1ServerFrame {
    /// The session is ready. Always the first frame.
    SessionStarted {
        /// Whether the agent speaks first. When true, a voice client withholds microphone frames
        /// until the first `turn_complete`. Absent means false.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        greeting: Option<bool>,
        /// Provider session identifier, for connection details only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// Recognised or generated speech text.
    Transcript {
        /// Who said it.
        role: TranscriptRole,
        /// The words, or the current hypothesis of them.
        text: String,
        /// The turn this text belongs to. A frame without one cannot be matched and is treated as
        /// its own turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
        /// False for a streaming hypothesis the provider may still correct. Absent means true, so
        /// a provider that only sends finished text needs no change.
        #[serde(default, rename = "final", skip_serializing_if = "Option::is_none")]
        is_final: Option<bool>,
    },
    /// Every accepted prompt, and every closed audio segment, ends with exactly one of these.
    TurnComplete {
        /// The turn that ended.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
        /// True when an `interrupt` cut the turn short.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupted: Option<bool>,
    },
    /// The provider failed. No `turn_complete` follows, so a client treats this as the end of the
    /// session.
    Error {
        /// What went wrong, in words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Additional detail, when the provider has any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// A JSON control frame a `vibe-talk-v1` client sends. Microphone audio travels separately, as
/// binary PCM at the session's input rate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VibeTalkV1ClientFrame {
    /// Microphone frames follow.
    AudioStart,
    /// The open microphone segment is over; the server answers with `turn_complete`.
    AudioEnd,
    /// One typed turn.
    Prompt {
        /// What the person typed.
        text: String,
    },
    /// Stop the running turn. The server answers with exactly one interrupted `turn_complete`, or
    /// ignores this when nothing is running.
    Interrupt,
    /// End the session.
    Quit,
}

/// Every root of the contract: each value that crosses a boundary on its own, by the name the
/// generated code gives it.
///
/// The order is the document's order, so keep it stable; a type reachable only through another
/// root still appears under `$defs` without being listed here.
fn roots(generator: &mut schemars::SchemaGenerator) -> Vec<schemars::Schema> {
    vec![
        generator.subschema_for::<ApiErrorBody>(),
        generator.subschema_for::<ClientConfigResponse>(),
        generator.subschema_for::<crate::conversation::VoiceSession>(),
        generator.subschema_for::<TimelineResponse>(),
        generator.subschema_for::<LiveMessageEvent<'static>>(),
        generator.subschema_for::<LiveDeleteEvent>(),
        generator.subschema_for::<LiveResetEvent>(),
        generator.subschema_for::<VibeTalkV1ServerFrame>(),
        generator.subschema_for::<VibeTalkV1ClientFrame>(),
        // Not bodies on their own, but rows the page keeps in device storage and reads back
        // across a reload, so they need their own validators.
        generator.subschema_for::<Message>(),
        generator.subschema_for::<crate::threads::ThreadSummary>(),
    ]
}

/// The whole contract as one JSON Schema (draft 2020-12) document.
///
/// Its `anyOf` lists the roots — the values that cross a boundary on their own — and `$defs`
/// holds every named type any of them reaches. Deterministic: `serde_json` keeps object keys
/// sorted, and nothing here depends on iteration order.
#[must_use]
pub fn schema_document() -> serde_json::Value {
    let mut generator = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let any_of = roots(&mut generator);
    let definitions = generator.take_definitions(true);
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://github.com/rrnewton/agent-utils/vibe-talk/contract/vibe-talk.schema.json",
        "title": "vibe-talk wire contract",
        "description": "Generated from vibe-talk/src/contract.rs by `make -C vibe-talk contract`. Do not edit.",
        "anyOf": any_of,
        "$defs": definitions,
    })
}

/// [`schema_document`] exactly as it is checked in: pretty-printed, with a trailing newline.
#[must_use]
pub fn schema_text() -> String {
    let mut text =
        serde_json::to_string_pretty(&schema_document()).expect("a JSON value always serializes");
    text.push('\n');
    text
}
