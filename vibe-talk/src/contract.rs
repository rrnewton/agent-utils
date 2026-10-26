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
    /// What this deployment calls its installed app, `[server] app_name`. `#343 pwa-app-name`.
    ///
    /// The page's title, its sign-in heading and its install manifest carry it already, written by
    /// the server; it is here so the page can use it once signed in. Always sent by this server;
    /// absent from an older one, whose app was always `vibe-talk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// Source chat service's human-readable name, supplied by the selected backend.
    pub chat_provider_name: String,
    /// Configured channels.
    pub channels: Vec<ChannelInfo>,
    /// Channels from the configuration file the owner took off his list, in configuration order.
    /// `#199 removable-config-channels`.
    ///
    /// Settings lists them so each can be shown again; `channels` above already leaves them out.
    /// Sent only to a write-scope caller, because showing one again takes that scope and a
    /// read-scope credential has no use for the names of channels the owner put away. Absent when
    /// none are hidden, and from an older server, which could not hide one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_channels: Vec<ChannelInfo>,
    /// Selected conversational voice provider, named by its implementation.
    pub conversational_voice: crate::conversation::VoiceDescription,
    /// Selected read-aloud backend and the playback interface the browser should use.
    pub read_aloud: crate::speech::Description,
    /// Server version, so a stale cached page is visible.
    pub version: &'static str,
    /// Whether a new call will be handed the earlier transcript: the operator allowed it AND the
    /// voice backend can carry it — the hosted backend always can, and a `vibe-talk-v1` bridge only
    /// once it has said it accepts the `replay` frame (`#331 resume-on-websocket`; see
    /// `replay::offered`).
    ///
    /// The page needs it BEFORE it has anything to replay: the Settings screen has to describe
    /// what resuming will do, and the control's own note has to say what the next call will be —
    /// both of which are questions asked while there is no conversation to fetch.
    pub replay_enabled: bool,
    /// Why replay is off although the operator turned it on, when that is the reason. `#270
    /// voice-transcript-markers`, `#331 resume-on-websocket`.
    ///
    /// Only for a `vibe-talk-v1` bridge, which takes a recap only when it says so: `bridge_declined`
    /// when it answered this server's hello without listing `replay`, `bridge_unconfirmed` when it
    /// has not answered one. The page then draws Resuming's switch disabled, with this reason
    /// beside it, and marks each new conversation "recap unsupported" rather than saying nothing,
    /// which would read as "nothing to recap". Absent otherwise: with replay on and the hosted
    /// backend, and with replay off, which `replay_enabled: false` says alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_unavailable: Option<ReplayUnavailable>,
    /// Whether a new call's recap goes in the voice-agent profile the `vibe-talk-v1` bridge fetches
    /// as its session opens, rather than in a `replay` frame. `#512 voice-recap-private-stack`.
    ///
    /// True only with `replay_enabled`, on a bridge the operator says fetches the profile
    /// (`replay.bridge_fetches_profile`) and that has not said it takes the frame. The page then
    /// arms the recap with `POST /api/v1/voice-recap` just before it opens the call's socket, or a
    /// reconnect's, and draws it as taken only once the server says a fetch served it. Absent
    /// otherwise, and from an older server.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replay_via_profile: bool,
    /// When the voice page takes the owner to be speaking although no transcript says so, and how
    /// long that may hold the page's own messages. `#512 voice-recap-private-stack`. Absent from an
    /// older server, whose page holds nothing for the microphone's level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_hold: Option<SpeechHold>,
    /// Whether this server records the voice page's transcript records: a `sent` row for each
    /// message the voice agent posted, markers for a conversation's start and a draft not sent,
    /// and the tag on a spoken yes, read back with `records=1`, and the send memory read with
    /// `sent_after`. `#270 voice-transcript-markers`. Absent, from an older server, means none of
    /// those may be sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transcript_records_supported: bool,
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
    /// The accounts the configuration names as the coding agents, `coding_agent_user_ids`.
    /// `#323 slack-system-bots`.
    ///
    /// The page draws each as the coding agent. Sent only when there are some; with several
    /// providers, each provider's own are in its [`ProviderDescription`] instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coding_agent_ids: Vec<String>,
    /// Whether only an account named in `coding_agent_ids`, or chosen by the reader in Settings,
    /// is drawn as the coding agent, rather than the one bot in a channel besides this server.
    /// `#323 slack-system-bots`.
    ///
    /// True when the configuration names any coding agent, and for a Slack provider, whose
    /// channels carry system bots (a workspace's welcome bot, integrations) that would otherwise be
    /// drawn as the coding agent, nameless and in its colours. Sent only when true: absent, from
    /// this server or an older one, means the page may guess.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub coding_agents_named_only: bool,
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
    /// Whether the selected provider can clear its own unread state as this server ingests
    /// messages, for a channel the owner turns that on for. `#232 upstream-mark-read`.
    ///
    /// Settings offers the per-channel control only when this is true. Sent only when true:
    /// absent, from this server or an older one, means false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upstream_clear_supported: bool,
    /// Whether the backend supports channel, thread-list, and flattened timelines.
    pub threading_supported: bool,
    /// Whether a reply to a main-channel message is posted in that message's thread, starting one
    /// when it has none, because the provider has no reply pointer of its own (Slack).
    ///
    /// `#252 thread-button-state`. Such a reply already starts the message's thread, so the reply
    /// screen offers no separate "start a new thread" choice when this is true. Sent only when
    /// true: absent, from this server or an older one, means false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replies_in_thread: bool,
    /// Whether a post sent again under the same `idempotency_key` is posted at most once.
    ///
    /// `#195 send-resilience`. The page retries a send whose answer it lost — a timeout, a 502 —
    /// on its own only when this is true, because only then can the retry not post the message
    /// twice. Sent only when true: absent, from this server or an older one, means false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_posts_supported: bool,
    /// Whether this server rewrites message bodies for speech before anything says them.
    ///
    /// Here for the same reason as `replay_enabled`: Settings has to be able to describe what this
    /// deployment does, and the page cannot infer this one from any message it holds. An empty
    /// `spoken_content` means "the rewrite changed nothing" just as often as it means "the pass is
    /// off" — deliberately, see [`crate::model::Message::spoken_content`] — so a page guessing from
    /// the data would tell the operator the feature was off every time the channel was plain prose.
    /// They are flipping this switch to compare two runs; guessing is exactly what they cannot do.
    pub speech_prep_enabled: bool,
    /// Each configured chat provider and what it can do, in configuration order.
    ///
    /// Every channel names its provider by key ([`ChannelInfo::provider`]). The deployment-wide
    /// flags above say whether ANY provider can do something; a page showing one channel uses
    /// that channel's provider entry here instead, so a channel read through a provider without
    /// threads is never offered a thread view because some other provider has one.
    ///
    /// Sent only when there are several providers. With one, and from an older server, it is
    /// absent and the deployment-wide flags are that provider's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderDescription>,
    /// Why message summaries cannot work on this deployment at all, when the server knows.
    ///
    /// Present means the page disables the summaries control and says this instead of letting
    /// every long row fail one request at a time. Absent means summaries may work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summaries_unavailable: Option<String>,
    /// Whether this server names threads and sums them up. `#528 thread-card-markdown-and-cap`.
    ///
    /// The page reads it as "a summary is coming": a thread whose first message is too long for
    /// its card is named from its first reply (`#522 thread-card-max-lines`), so with this the
    /// page draws such a thread in the summary's layout at once — its first message inward, whole,
    /// as the card's first child — and only the card's words change when the summary lands.
    /// Without it the page splits the first message, as it always did. Absent, from an older
    /// server, from one that cannot name threads or whose summariser cannot work, and to a
    /// read-scope caller (whose reads queue no naming), means false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub thread_names_supported: bool,
    /// The scope of the token that asked.
    ///
    /// The caller's own scope, so it discloses nothing it did not already hold. The page needs it
    /// to avoid asking for what this token cannot have: stored conversations are write-only (see
    /// the conversation routes), and a read-scope page that probed them anyway got a 403 and a
    /// console error on every load. `#38 read-token-conversation-probe`.
    pub token_scope: TokenScope,
    /// The rules that decide which messages are read automatically. `#196 auto-read-noise`.
    ///
    /// Here rather than behind a route of its own because the page needs them at startup, for the
    /// Settings editor, and every device and the voice agent share one list. Absent from an older
    /// server — the page then hides the editor — and also when this one cannot read its store, so
    /// a broken store makes the editor disappear rather than show a list that is not in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_rules: Option<NoiseRules>,
    /// "Ask me before the voice agent sends", as Settings shows it. `#264 voice-send-approval`.
    ///
    /// Here so the checkbox is drawn at sign-in, as the noise rules are. Readable at read scope —
    /// it says how the voice agent behaves, which a reader of the channels may know — and changed
    /// only by a write-scope `PUT /api/v1/voice-settings`. Absent from an older server, which
    /// always asked; the page then hides the checkbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_settings: Option<VoiceSettings>,
    /// The owner's own words for the voice agent, as Settings shows them. `#272
    /// voice-agent-instructions`.
    ///
    /// Here so the Instructions editor is drawn at sign-in, as the send setting is. Readable at
    /// read scope — the assembled prompt is served there too, to a bridge holding the read token —
    /// and changed only by a write-scope `PUT /api/v1/voice-prompt`. Absent from an older server;
    /// the page then hides the editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_prompt: Option<VoicePromptSettings>,
    /// The tag this server puts before every message the voice agent posts, as Settings shows it.
    /// `#303 voice-post-prefix`.
    ///
    /// Here so the Sending section's field is drawn at sign-in, as the send setting is. Readable at
    /// read scope — it is in every post the agent makes, which any reader of the channel sees —
    /// and changed only by a write-scope `PUT` or `DELETE /api/v1/voice-post-prefix`. Absent from
    /// an older server, which tags nothing; the page then hides the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_post_prefix: Option<VoicePostPrefix>,
    /// The message cache, as Settings' "Saved messages on this server" shows it. `#208
    /// persistent-cache`.
    ///
    /// Present only when the server's configuration has a `[message_cache]` table, and sent only
    /// to a write-scope caller, the one scope that can switch it: numbers, dates and codes, never
    /// a path. Absent from an older server; the page then hides the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_cache: Option<MessageCacheStatus>,
    /// Whether this server keeps the owner's synced settings at `/api/v1/client-settings`.
    /// `#457 settings-server-side`.
    ///
    /// The page asks for them only when this is true, as it asks for nothing a token or a
    /// server cannot give (`#38`): an older server is never probed for a route it has not
    /// got. Sent only when true: absent means false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub client_settings_supported: bool,
}

/// The tag this server puts before every message the voice agent posts. `#303 voice-post-prefix`.
///
/// The body of `GET`, `PUT` and `DELETE /api/v1/voice-post-prefix`, and `voice_post_prefix` in
/// client-config. The tag counts toward the provider's post-size limit and goes on the first part
/// of a post that has to be cut; posts typed on the page never carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct VoicePostPrefix {
    /// The tag in force: the owner's saved one when `custom`, otherwise `default_prefix`. Empty
    /// means the voice agent's posts carry no tag. Put before the words exactly as it is, so a
    /// trailing space is what separates the two.
    pub prefix: String,
    /// The tag this server ships, which a reset brings back.
    pub default_prefix: String,
    /// Whether `prefix` is the owner's own saved choice — possibly empty, which turns the tag off —
    /// rather than the default because he has saved none.
    pub custom: bool,
    /// The longest tag a save may carry, in characters.
    pub max_chars: u32,
    /// Whether this server can keep a change. False with no store configured, and when the store
    /// cannot be read — the server then tags with the default.
    pub changeable: bool,
}

/// How the voice page holds its own messages while the owner is speaking. `#512
/// voice-recap-private-stack`.
///
/// A relayed chat message, or the page's word on a send card, is said by closing the microphone's
/// segment with `audio_end`; closed under the owner's speech, the speech service took the message
/// and never answered it, and the call was lost. The page judged speech only from transcripts,
/// which that service may drop. So it also reads the microphone's level, and holds while it is at
/// or above `level_dbfs`, or while his transcript grew in the last moments — never longer than
/// `cap_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpeechHold {
    /// The capture buffer's RMS, in dBFS, at or above which the owner is taken to be speaking:
    /// `conversation.speech_hold_dbfs`.
    pub level_dbfs: i32,
    /// The longest one message is held for his speech, in milliseconds; `0`, never held:
    /// `conversation.speech_hold_cap_seconds`.
    pub cap_ms: u32,
}

/// Why replay is off although the operator turned it on. `#270 voice-transcript-markers`, `#331
/// resume-on-websocket`.
///
/// Both are about a `vibe-talk-v1` bridge, the one backend that takes a recap only when it says it
/// does; see [`crate::voice_bridge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplayUnavailable {
    /// The bridge answered this server's hello, and `replay` was not among the frames it said it
    /// accepts.
    BridgeDeclined,
    /// The bridge has not answered a hello: this server has not asked yet, is asking, has no
    /// address it can reach the bridge at, or the bridge did not answer.
    BridgeUnconfirmed,
}

/// The owner's own words for the voice agent, as Settings shows them. `#272
/// voice-agent-instructions`.
///
/// The body of `GET` and `PUT /api/v1/voice-prompt`, and `voice_prompt` in client-config. Both
/// texts join only the scope-neutral part of the prompt: what the agent can do — read, send, or
/// ask first — is added by the server from the caller's token and the send setting, after them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct VoicePromptSettings {
    /// Additional instructions, added to every call. Empty for none.
    pub instructions: String,
    /// The base prompt in force: the owner's edited copy when `system_prompt_edited`, otherwise
    /// the shipped one, so the editor always opens on what the agent is actually given.
    pub system_prompt: String,
    /// Whether `system_prompt` is the owner's edited copy.
    pub system_prompt_edited: bool,
    /// The prompt version shipped when he last saved his copy; present exactly when he has one.
    /// Behind `prompt_version` means the shipped prompt has changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_from_version: Option<u32>,
    /// The version of the prompt this server ships.
    pub prompt_version: u32,
    /// The longest `instructions` a save may carry, in characters.
    pub max_instructions_chars: u32,
    /// The longest `system_prompt` a save may carry, in characters.
    pub max_system_prompt_chars: u32,
    /// Whether this server can keep a change. False with no store configured, and when the store
    /// cannot be read — the agent then gets the shipped prompt.
    pub changeable: bool,
    /// How these words reach this deployment's voice agent.
    pub delivery: PromptDelivery,
}

/// How the owner's words reach the voice agent. `#272 voice-agent-instructions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptDelivery {
    /// A deployment-managed bridge fetches the assembled prompt from `GET /api/v1/voice-agent` as
    /// each call opens, so both texts reach its next call.
    Profile,
    /// The page hands them to a hosted agent as each call opens: the whole assembled prompt when
    /// the agent allows its prompt to be replaced, otherwise the instructions alone, as a note at
    /// the start. The voice session says which ([`SessionPrompt`]).
    Page,
}

/// What the page hands a hosted agent as a call opens, so the call carries the owner's words.
/// `#272 voice-agent-instructions`.
///
/// On every voice session whose provider is one the page configures, whatever the owner has saved.
/// An agent that allows its prompt to be replaced is handed the assembled prompt on every call, so
/// its send rules follow the Sending switch; one that does not is told `not_sent` when nothing of
/// the owner's can reach it, so the page learns that the agent keeps the prompt in its provider's
/// console. Absent for a provider that reads the profile instead (a deployment-managed bridge), and
/// from an older server; the page then sends exactly what it always sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SessionPrompt {
    /// How to hand `text` over.
    pub transport: PromptTransport,
    /// What to send: the whole assembled prompt for `prompt_override` — the shipped one when the
    /// owner saved nothing — the heading and the instructions for `contextual_update`; absent for
    /// `not_sent`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Whether the owner's edited base prompt is in what is sent.
    pub system_prompt_applied: bool,
    /// Whether the owner's additional instructions are in what is sent.
    pub instructions_applied: bool,
    /// What the agent's configuration said about replacing its prompt; absent when it could not
    /// be read or did not say, which is treated as not allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_allowed: Option<bool>,
}

/// How the page hands the owner's words to a hosted agent. `#272 voice-agent-instructions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptTransport {
    /// As the system prompt itself, in the initiation frame's
    /// `conversation_config_override.agent.prompt.prompt`. Only when the agent allows it: an
    /// override it does not allow is believed to end the conversation.
    PromptOverride,
    /// As a `contextual_update` sent right after the initiation frame. The agent keeps its own
    /// prompt; only the instructions reach it.
    ContextualUpdate,
    /// Nothing: the agent does not allow its prompt to be replaced (or its configuration did not
    /// say, or could not be read), and the owner has no instructions saved — he saved nothing, or
    /// edited only the base prompt. The initiation frame is exactly what it always was, and the
    /// agent keeps the prompt in its provider's console.
    NotSent,
}

/// Whether the voice agent must ask before it sends. `#264 voice-send-approval`.
///
/// The body of `GET` and `PUT /api/v1/voice-settings`, and `voice_settings` in client-config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct VoiceSettings {
    /// The value in force: true means `post_reply` proposes and the owner confirms, by a tap or by
    /// a plain yes the voice page hears; false means `post_reply` posts at once. The owner's saved
    /// choice, or the configured default until he makes one.
    pub ask_before_send: bool,
    /// Whether this server can keep a change. False with no store configured, and when the store
    /// cannot be read — the server then asks, whatever the configured default says.
    pub changeable: bool,
}

/// The owner's synced client settings: every setting outside Settings' Screen and keys
/// section, held on this server for all his devices. `#457 settings-server-side`.
///
/// The body of `GET` and `PUT /api/v1/client-settings`, and of the `settings` event on
/// the live streams, where it carries only the entries that changed. A key the owner
/// has never set on any device is absent, which is the shipped default's turn; an entry
/// whose `value` is null says the key was forgotten. `updated_ms` is this server's
/// clock when it took the value — per-key last-writer-wins compares it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientSettings {
    /// The settings held, or the ones that changed, by key.
    pub settings: Vec<crate::client_settings::SyncedSetting>,
}

/// The message cache, as Settings shows it: whether it is on, why it is not, and how much it
/// holds. `#208 persistent-cache`.
///
/// The body `PUT` and `DELETE /api/v1/message-cache` answer with, and `message_cache` in
/// client-config. Numbers, dates and codes only: no path, no channel, nothing of a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct MessageCacheStatus {
    /// Whether a cache file is open, so what write-token and server reads see is being saved.
    pub available: bool,
    /// Whether the cache is switched on: the configuration enables it and the owner has not
    /// turned it off. True while `available` is false means it should be open and is not, and
    /// `reason` says why.
    pub enabled: bool,
    /// Why no cache is open, as a code, when none is: `config_disabled` (the server's
    /// configuration turns it off, and only the operator can turn it on), `switched_off` (the
    /// owner turned it off), or what stopped the file opening — `switch_unreadable`,
    /// `switch_unwritable`, `sqlite_unsupported`, `foreign_file`, `locked`, `filesystem`,
    /// `sqlite` or `not_open`. A string rather than a closed set, so a code added later does not
    /// make an older page refuse client-config; a page reads one it does not know as "not open".
    /// Absent while a cache is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// Bytes the file occupies, its working files included; 0 while none is open.
    pub bytes: u64,
    /// Messages it holds.
    pub messages: u64,
    /// Channels it holds anything of.
    pub channels: u64,
    /// The earliest message time from which some part of a channel is proven to be held whole,
    /// as an RFC 3339 instant in UTC. Absent while nothing is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_covered: Option<String>,
    /// The server's age limit, in days: nothing older is kept.
    pub max_days: u16,
    /// The server's size limit, in megabytes, the working files included.
    pub max_megabytes: u32,
}

/// `PUT /api/v1/message-cache`: the owner's switch. `#208 persistent-cache`.
///
/// An unknown field is ignored, as everywhere in the contract: a page may send one a later server
/// reads without an older one refusing the switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageCacheRequest {
    /// False turns the cache off and erases its file. True turns it on again with a fresh, empty
    /// file, and only where the server's configuration enables it.
    pub enabled: bool,
}

/// The owner's noise rules, as the Settings editor shows them. `#196 auto-read-noise`.
///
/// Also the body `PUT /api/v1/noise-rules` answers with, so the editor redraws from what the
/// server STORED — trimmed and de-duplicated — rather than from what was typed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NoiseRules {
    /// The rules in force, in the owner's order and his own spelling. Until he saves a list this
    /// is the shipped default.
    pub rules: Vec<String>,
    /// One sentence saying how a rule matches, written beside the matcher and quoted by the page
    /// where the rules are edited, so the description cannot drift from what the server does.
    pub matching: &'static str,
}

/// `GET /api/v1/channels/{id}/messages`: a channel's most recent messages, the window the classic
/// page draws.
///
/// `#208 persistent-cache` made it a contract root, appended below, when it gained `as_of`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MessagesResponse {
    /// Channel that was read.
    pub channel: ChannelInfo,
    /// Messages, oldest first.
    pub messages: Vec<Message>,
    /// Whether the fetch came back short, so `messages.len()` counts the CHANNEL rather than the
    /// window this server asked for.
    ///
    /// A client may show a message count only when this is true. Discord gives no message count
    /// for a guild text channel, so a full window means "at least this many" and nothing more.
    pub complete: bool,
    /// Which of `messages` the reader has ARCHIVED, so the view can dim them.
    ///
    /// The ordinary channel view shows archived rows greyed rather than hidden -- hiding is what
    /// the To do filter is for -- and it can only do that if the payload says which they are.
    pub dismissed: Vec<MessageId>,
    /// The instant, RFC 3339, up to which the provider knows `messages` is complete, when it
    /// served a copy it could not bring up to date just now: a bridge reading its Flat view says
    /// so. Absent means now, and from an older server. The same field as
    /// [`crate::threads::TimelinePage::as_of`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
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
    /// `#222 unread-replies`. On the newest page of Main, or a delta of it, in a channel with
    /// threads: every message marked Done from a position on, the replies in Main's threads
    /// included. Absent everywhere else.
    ///
    /// `dismissed` names only the rows on the page, and Main shows a thread's root but none of its
    /// replies, so without this a page in Main could not learn that a reply it holds from All was
    /// marked Done, or put back, on another device. See [`ReplyDismissals`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_dismissals: Option<ReplyDismissals>,
    /// The revision this channel's pins are at. `#206 pin-message`.
    ///
    /// On every read, newest page and delta alike, so a refresh that already happens is what tells
    /// a page another device pinned or unpinned something; the page reads `/pins` again only when
    /// this differs from the revision of the list it holds. Absent when the store cannot say — no
    /// storage configured, or a failing one — and from an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins_revision: Option<i64>,
    /// This channel's bookmark and its revision. `#240 reading-bookmark`.
    ///
    /// On every read, newest page and delta alike, as `pins_revision` is, and for its reason: a
    /// refresh that already happens is how a page learns the owner moved the bookmark on his other
    /// device. ONE small row, so it rides inline rather than as a revision and a second read.
    /// Absent when the store cannot say, and the page keeps what it has; `anchor: null` inside it
    /// is the different answer that no bookmark is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmark: Option<crate::store::Bookmark>,
    /// Channel content remains untrusted data in every view.
    pub untrusted_content_notice: &'static str,
}

/// Every message marked Done at or after one position in a channel. `#222 unread-replies`.
///
/// COMPLETE FROM `from` UP, which is the whole of its use: a message at or after `from` that is
/// not in `messages` is not Done, so a reply put back on another device leaves the list as surely
/// as one marked Done joins it. Below `from` it says nothing. A position is a message id read as a
/// number, which every provider here orders by time, so a reply is never before its thread's root
/// and every reply to a root at or after `from` is covered.
///
/// `from` is the page's oldest row, raised to the oldest mark kept when more than
/// [`crate::ops::MAX_REPLY_DISMISSALS`] marks lie above it: the newest are kept, because unread
/// replies are the recent ones. Ids and no text, as `dismissed` is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ReplyDismissals {
    /// The position the list is complete from: a message id, compared as a number.
    pub from: MessageId,
    /// Every message at or after `from` that is marked Done, newest first.
    pub messages: Vec<MessageId>,
}

/// Whether this server clears one channel's unread state at its chat provider, and what it last
/// did: `GET` and `PUT /api/v1/channels/{id}/upstream-clear`. `#232 upstream-mark-read`.
///
/// Everything here is the server's own record. The provider's cursor is never read back except
/// as the answer to a push, so `pushed` and `last` say what this server did, not what the owner
/// has since read in the provider's own client.
#[derive(Debug, Serialize, JsonSchema)]
pub struct UpstreamClearState {
    /// The channel it is about.
    pub channel: ChannelInfo,
    /// The name of the channel's chat provider, for the words on the control.
    pub provider_name: String,
    /// Whether the channel's provider can do this at all. When false the mode is always off.
    pub supported: bool,
    /// What the owner chose. Off unless set.
    pub mode: crate::store::UpstreamClearMode,
    /// When it was turned on or to a dry run, in milliseconds since the Unix epoch, while it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_at_ms: Option<i64>,
    /// The newest push the provider's cursor is known to cover because of this server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<UpstreamClearAttemptView>,
    /// The provider's cursor before this server first moved it, for a manual undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_cursor: Option<String>,
    /// The most recent attempt of any kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<UpstreamClearAttemptView>,
    /// Failed attempts in a row.
    pub failures: u32,
    /// Whether a message is waiting to be pushed.
    pub pending: bool,
    /// When the next retry is due, in milliseconds since the Unix epoch, after a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at_ms: Option<i64>,
    /// Why the provider cannot do this for this channel, once it has said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// What Settings' "Mark read now" came to: `POST /api/v1/channels/{id}/upstream-clear/now`.
/// `#253 upstream-clear-polish`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct UpstreamClearNowResponse {
    /// What happened to the press.
    pub outcome: UpstreamClearNowOutcome,
    /// The channel's newest message, the one the press asked to push. Absent with `no_message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    /// That message's own instant, in milliseconds since the Unix epoch, when it can be read. An
    /// attempt recorded later whose `message_at_ms` is at or after it answers the press, which is
    /// how a page told `waiting` recognises the answer when it reads the state again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_at_ms: Option<i64>,
    /// The recorded push that answered the press, with `pushed`, `failed` and `skipped`, and with
    /// `unavailable` when the push itself was refused: the pressed message's own, or a newer
    /// message's, which covers it. What the press came to is said from this, never from
    /// `state.last`, which a later push may already have replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<UpstreamClearAttemptView>,
    /// The channel's state after it, as `GET .../upstream-clear` would answer now.
    pub state: UpstreamClearState,
}

/// What happened to a press of "Mark read now". `#253 upstream-clear-polish`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamClearNowOutcome {
    /// The press's push was made and the provider took it: `attempt` says whether its marker
    /// moved, was already there, or, in a dry run, what a real push would have done.
    Pushed,
    /// The press's push was made and failed; `attempt.error` says why. It is retried by itself, at
    /// `state.retry_at_ms`.
    Failed,
    /// The bridge no longer holds the message the push named, so nothing was marked; the next
    /// message that arrives is pushed.
    Skipped,
    /// An earlier push, or an earlier dry run, already covers the newest message. Nothing was sent.
    Covered,
    /// The channel has no message that can be marked.
    NoMessage,
    /// The channel is waiting out a failure, from before the press or of an older push that was
    /// already on its way, and the newest message goes when the retry is due, at
    /// `state.retry_at_ms`. Nothing was sent for the press.
    RetryWaiting,
    /// The push is queued or on its way, and had not been recorded when the server stopped
    /// waiting, at most 45 seconds after the press. The server keeps going without the page.
    Waiting,
    /// The channel's setting changed while the press waited (turned off, set to the other mode,
    /// or every setting erased), or the channel stopped being one this server can clear, so the
    /// push was not made as asked. `state` says what holds now.
    Changed,
    /// The provider refused this channel; `state.unavailable` says why.
    Unavailable,
}

impl UpstreamClearNowOutcome {
    /// The wire name, for log lines.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pushed => "pushed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Covered => "covered",
            Self::NoMessage => "no_message",
            Self::RetryWaiting => "retry_waiting",
            Self::Waiting => "waiting",
            Self::Changed => "changed",
            Self::Unavailable => "unavailable",
        }
    }
}

/// One push, as Settings shows it. `#232 upstream-mark-read`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct UpstreamClearAttemptView {
    /// When it was made, in milliseconds since the Unix epoch.
    pub at_ms: i64,
    /// The newest ingested message it was for.
    pub message_id: MessageId,
    /// That message's instant, in milliseconds since the Unix epoch.
    pub message_at_ms: i64,
    /// What it came to.
    pub result: crate::store::UpstreamClearResult,
    /// Where the provider's cursor was moved to, or would have been, as the provider wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The provider's cursor before it, as the provider wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// Why it failed, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<&crate::store::UpstreamClearAttempt> for UpstreamClearAttemptView {
    fn from(attempt: &crate::store::UpstreamClearAttempt) -> Self {
        Self {
            at_ms: attempt.at_ms,
            message_id: attempt.message.clone(),
            message_at_ms: attempt.message_ms,
            result: attempt.result,
            target: attempt.target.clone(),
            previous: attempt.previous.clone(),
            error: attempt.error.clone(),
        }
    }
}

/// One channel's pins: `GET /api/v1/channels/{id}/pins`. `#206 pin-message`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PinsResponse {
    /// The channel, as configured here.
    pub channel: ChannelInfo,
    /// Every pin, in the order their messages were sent, oldest first, each with the snapshot that
    /// lets the page draw it when its message is not loaded.
    pub pins: Vec<crate::store::Pin>,
    /// The revision this list is at; see [`TimelineResponse::pins_revision`].
    pub revision: i64,
    /// How many pins a channel keeps before the oldest is unpinned to make room.
    pub limit: usize,
    /// The standing statement that these pins are this server's own.
    pub pins_notice: &'static str,
    /// A snapshot is channel content, and untrusted, like every other.
    pub untrusted_content_notice: &'static str,
}

/// What one pin or unpin did: `PUT` and `DELETE /api/v1/channels/{id}/pins/{message_id}`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PinChangeResponse {
    /// The channel, as configured here.
    pub channel: ChannelInfo,
    /// The message the call named.
    pub message_id: MessageId,
    /// Whether it is pinned now.
    pub pinned: bool,
    /// Whether the call changed anything; false when it repeated what was already so.
    pub changed: bool,
    /// The pin exactly as stored — its text cut to the bound — or absent after an unpin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<crate::store::Pin>,
    /// Pins the bound dropped to make room for this one, oldest first. Empty except at the bound,
    /// and the page says so when it is not.
    pub unpinned: Vec<MessageId>,
    /// The channel's pin revision after the call, so the page that made it need not read the list
    /// again to stay current.
    pub revision: i64,
    /// The standing statement that these pins are this server's own.
    pub pins_notice: &'static str,
}

/// One channel's bookmark, read or written: `GET`, `PUT` and `DELETE /api/v1/channels/{id}/bookmark`.
/// `#240 reading-bookmark`.
///
/// A conditional write that lost to a move made elsewhere answers 409 with this same body and an
/// [`ApiErrorBody`]'s two fields beside it, so the page takes where the bookmark is now from the
/// refusal itself.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BookmarkResponse {
    /// The channel, as configured here.
    pub channel: ChannelInfo,
    /// The bookmark as stored after the call.
    pub bookmark: crate::store::Bookmark,
    /// Whether the call changed what is stored: false for a read, a repeat, and a refusal.
    pub changed: bool,
    /// The standing statement that the bookmark is this server's own and marks nothing read.
    pub bookmark_notice: &'static str,
}

/// A post waiting for the owner's confirmation. `#34 voice-chat-write-confirm`.
///
/// Everything the confirmation has to show — the exact text, where it goes, what it answers — and
/// the handle that commits exactly this. The handle is a capability: it is handed only to a
/// write-scope caller of the proposal routes, never to a model, and a commit must restate the
/// channel, text, and reply target it was issued for.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PendingPost {
    /// Correlates this proposal's steps in the access log. Increases with every proposal.
    ///
    /// Within one server process only: a restarted server numbers its proposals from one again.
    /// `instance` says which process numbered this one.
    pub serial: u64,
    /// Which run of this server numbered `serial`: a token minted when the process started, the same
    /// on every answer until it stops. `#270 voice-transcript-markers`. A page that stays open
    /// across a restart sees the serials it has drawn come round again, and tells them apart by this.
    /// Absent from an older server.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance: String,
    /// The single-use confirmation handle.
    pub handle: String,
    /// The channel it would be posted to.
    pub channel_id: ChannelId,
    /// That channel's name as the owner knows it, alias included.
    pub channel_name: String,
    /// The exact text that would be posted.
    pub text: String,
    /// The start of `text` that this server put there: the owner's tag for what the voice agent
    /// posts (`#303 voice-post-prefix`). `text` begins with exactly this. Absent when nothing was
    /// added — tagging is off — and from an older server. It is his own words, not the agent's, so
    /// a client judging whether the agent's words can be confirmed by voice judges the rest.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    /// The message it would reply to, when any.
    pub reply_to: Option<MessageId>,
    /// Who wrote the message it would reply to, when this server holds that message. `#269
    /// approval-card-inline`: the card says "replying to Alex", never an id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_author: Option<String>,
    /// The first few words of the message it would reply to, flattened, when this server holds
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_excerpt: Option<String>,
    /// The name of the thread the message it would reply to is in, when that is a thread this
    /// server has named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_name: Option<String>,
    /// Milliseconds until the handle expires.
    pub expires_in_ms: u64,
}

/// The post waiting for confirmation, or `null` when none is.
///
/// `#270 voice-transcript-markers` adds the post gate's send memory: every message the voice agent
/// posted in the last [`crate::post_gate::SEND_MEMORY_TTL`], however it was approved, published
/// once it settled. A page reads it with `sent_after`, the newest `send_seq` it has seen; without
/// that it gets no sends and waits as it always did.
///
/// The fields this adds are optional in the contract, though this server always sends them, so a
/// page left open across a rollback to an older server still reads the proposal it is shown: a
/// field the contract required would make the page refuse every answer from that server.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PendingPostResponse {
    /// The pending proposal. At most one exists: a new proposal supersedes the old.
    pub proposal: Option<PendingPost>,
    /// Which run of this server answered: see [`PendingPost::instance`]. Its `send_seq` numbers
    /// only go forward within one run; a different run numbers its own, from its clock.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance: String,
    /// The sends that settled after `sent_after`, oldest first. Absent when there are none, and
    /// whenever the request carried no `sent_after`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<VoiceSend>,
    /// The newest `send_seq` this server has handed out: the page's next `sent_after`. Always sent
    /// by this server; absent only from an older one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sends_through: Option<u64>,
    /// What became of the proposals this server still remembers, oldest first. Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settled: Vec<ProposalSettled>,
    /// The server's clock, in milliseconds since the Unix epoch, for placing a send's
    /// `begun_at_ms` in a call. Always sent by this server; absent only from an older one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// One message the voice agent posted, as the send memory publishes it. `#270
/// voice-transcript-markers`.
///
/// The same text and detail a `sent` transcript row records, so a page draws a live card and a
/// restored one with the same code.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct VoiceSend {
    /// The text that went, capped as a transcript row is.
    pub text: String,
    /// Where it went, how it was approved, and what is known to have been posted.
    pub detail: crate::store::SentRecord,
    /// Which run of this server numbered `detail.serial`: see [`PendingPost::instance`]. Not
    /// stored with a transcript row, which names its send by `send_seq` alone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance: String,
}

impl VoiceSend {
    /// The send memory's record of one send, as the run `instance` publishes it.
    #[must_use]
    pub fn published(send: crate::post_gate::VoiceSendRecord, instance: &str) -> Self {
        Self {
            text: send.text,
            detail: send.record,
            instance: instance.to_owned(),
        }
    }
}

/// What became of one proposal. `#270 voice-transcript-markers`.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct ProposalSettled {
    /// The proposal's number.
    pub serial: u64,
    /// What became of it.
    pub fate: crate::post_gate::ProposalFate,
}

/// A confirmed proposal, posted.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CommittedPostResponse {
    /// Which proposal this was.
    pub serial: u64,
    /// The send memory's number for this send, under which the voice page records it. `#270
    /// voice-transcript-markers`. Absent only when the memory was emptied while it was on its way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_seq: Option<u64>,
    /// The FIRST message as the chat service accepted it.
    pub posted: Message,
    /// Every part, in order. One element for a message that fitted.
    pub parts: Vec<Message>,
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

/// One configured chat provider, as the page needs to know it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProviderDescription {
    /// The key channels name this provider by.
    pub key: String,
    /// Human-readable name of the source chat service.
    pub name: String,
    /// Whether this provider accepts pasted links and references when adding a channel.
    pub channel_registration_supported: bool,
    /// Whether this provider can list its channels for browsing.
    pub channel_discovery_supported: bool,
    /// Whether this provider exposes a write-through read cursor.
    pub upstream_read_mark_supported: bool,
    /// Whether this provider can clear its own unread state as this server ingests messages.
    /// `#232 upstream-mark-read`. Sent only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upstream_clear_supported: bool,
    /// Whether this provider supports channel, thread-list, and flattened timelines.
    pub threading_supported: bool,
    /// Whether a reply to a main-channel message is posted in that message's thread, starting one
    /// when it has none, because this provider has no reply pointer of its own (Slack).
    /// `#252 thread-button-state`. Sent only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replies_in_thread: bool,
    /// Whether this provider posts a message sent again under the same key at most once. Sent
    /// only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_posts_supported: bool,
    /// How changes in this provider's channels reach the live stream.
    pub live_delivery: LiveDelivery,
    /// Seconds between this provider's live polls, or `0` when it is not polled.
    pub live_poll_seconds: u64,
    /// The account this server posts as at this provider, when known.
    pub self_author_id: Option<String>,
    /// The owner's own account at this provider, when configured.
    pub owner_author_id: Option<String>,
    /// The accounts the configuration names as this provider's coding agents. `#323
    /// slack-system-bots`. Sent only when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coding_agent_ids: Vec<String>,
    /// Whether only those, or an account the reader chose in Settings, are drawn as the coding
    /// agent in this provider's channels: see [`ClientConfigResponse::coding_agents_named_only`].
    /// Sent only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub coding_agents_named_only: bool,
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
    /// Whether it was published before this stream attached and is re-sent from the replay tail.
    /// Any channel read begun after the stream's response arrived has already seen it. An
    /// adapter's catch-up arriving after the attach is `replayed` but not from the tail: it may be
    /// newer than every read the page has made.
    pub from_tail: bool,
    /// Whether THIS SERVER posted it; the page must not relay these into a live conversation.
    pub self_posted: bool,
    /// The idempotency key of the request this server posted it for, when it had one: the page
    /// that sent it settles its outbox entry by it when the post's own answer was lost. `#468
    /// send-dedupe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<&'a str>,
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
    /// Whether it was published before this stream attached; see [`LiveMessageEvent::from_tail`].
    pub from_tail: bool,
    /// When this server observed the removal, on its clock, in Unix milliseconds: a page read that
    /// began before it never brings the message back. `#473 replication-c7-server-versions`, the
    /// replication model's rule 3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_ms: Option<i64>,
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
        /// The optional client frames this server accepts, by `type`: the bridge's half of the
        /// hello. `#331 resume-on-websocket`.
        ///
        /// `replay` is the only one so far. A client sends an optional frame only to a session
        /// that listed it, because a server that predates one answers it with `error`, which ends
        /// the session. Absent or empty means none; a word a client does not know is ignored.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
    },
    /// The answer to a `replay` frame: exactly one per `replay`, and not a turn, so no
    /// `turn_complete` follows it. `#331 resume-on-websocket`.
    ReplayAck {
        /// True when the record was handed to the agent as context for what it says next; false
        /// when this session will not use it.
        accepted: bool,
        /// Why it was not accepted, in words, when it was not.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
    /// The written record of an earlier conversation, for the agent to carry on from. `#331
    /// resume-on-websocket`, `#46 conversation-replay`.
    ///
    /// Sent at most once per session, only to a session whose `session_started` listed `replay`
    /// in `capabilities`, and as the client's first frame after it, before `audio_start` or any
    /// `prompt`. The server answers with exactly one `replay_ack`. It is context, not a turn: the
    /// agent is not to answer it or read it back, and nothing about it opens or ends a turn.
    Replay {
        /// The whole payload, exactly as `GET /api/v1/conversations/{id}/replay` renders it and
        /// the hosted backend is sent it: a preamble saying what follows, then the record, fenced
        /// as untrusted text. Within the server's `replay.max_chars` and `replay.max_turns`.
        text: String,
        /// How many lines of the earlier conversation the record holds.
        lines: u32,
        /// How many older lines the budget left out; non-zero means the record is partial, and
        /// `text` says so too.
        dropped: u32,
        /// How many words the lines hold, counting only what was said or sent.
        words: u32,
    },
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
        generator.subschema_for::<PendingPostResponse>(),
        generator.subschema_for::<CommittedPostResponse>(),
        generator.subschema_for::<LiveMessageEvent<'static>>(),
        generator.subschema_for::<LiveDeleteEvent>(),
        generator.subschema_for::<LiveResetEvent>(),
        generator.subschema_for::<VibeTalkV1ServerFrame>(),
        generator.subschema_for::<VibeTalkV1ClientFrame>(),
        // Not bodies on their own, but rows the page keeps in device storage and reads back
        // across a reload, so they need their own validators.
        generator.subschema_for::<Message>(),
        generator.subschema_for::<crate::threads::ThreadSummary>(),
        // `#206 pin-message`. Appended, so the roots before them keep their places.
        generator.subschema_for::<PinsResponse>(),
        generator.subschema_for::<PinChangeResponse>(),
        // `#232 upstream-mark-read`. Appended, for the same reason.
        generator.subschema_for::<UpstreamClearState>(),
        // `#240 reading-bookmark`. Appended, for the same reason.
        generator.subschema_for::<BookmarkResponse>(),
        // `#253 upstream-clear-polish`. Appended, for the same reason.
        generator.subschema_for::<UpstreamClearNowResponse>(),
        // `#264 voice-send-approval`. Appended, for the same reason.
        generator.subschema_for::<VoiceSettings>(),
        // `#208 persistent-cache`. Appended, for the same reason.
        generator.subschema_for::<MessagesResponse>(),
        // `#272 voice-agent-instructions`. Appended, for the same reason.
        generator.subschema_for::<VoicePromptSettings>(),
        // `#303 voice-post-prefix`. Appended, for the same reason.
        generator.subschema_for::<VoicePostPrefix>(),
        // `#208 persistent-cache`: the owner's switch and what it answers. Appended, for the same
        // reason.
        generator.subschema_for::<MessageCacheStatus>(),
        generator.subschema_for::<MessageCacheRequest>(),
        // `#457 settings-server-side`. Appended, for the same reason.
        generator.subschema_for::<ClientSettings>(),
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
