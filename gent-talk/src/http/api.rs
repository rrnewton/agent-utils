//! Request handlers.
//!
//! Every handler that touches a channel goes through the same three steps, in this order:
//! authorize the caller, resolve the channel against the configured allowlist, then act. Doing
//! them in that order means an unauthenticated caller cannot use error messages to discover which
//! channel snowflakes are configured.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthError, Scope};
use crate::discord::DiscordError;
use crate::elevenlabs::{SignedUrl, SignedUrlError};
use crate::model::{ChannelInfo, Message, MessageId};
use crate::ops::{self, OpError};
use crate::retrieval::Resolution;
use crate::state::AppState;
use crate::store::{ConversationId, ConversationSummary, ReadMark, Speaker, StoreError, Turn};
use crate::summary::DigestEntry;
use crate::untrusted;

/// A JSON error body.
#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    /// Stable machine-readable code.
    pub error: &'static str,
    /// Human-readable detail. Never contains a secret.
    pub detail: String,
}

/// An error that can be returned to a caller.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    detail: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status,
            code,
            detail: detail.into(),
        }
    }

    /// A request this server understood and will not carry out as written.
    ///
    /// One code, `bad_request`, because a client branches on the code and there is nothing useful
    /// to branch on here: the detail says what to send instead, and every use of this is a shape
    /// the caller can fix by reading it.
    fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", detail)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: self.code,
                detail: self.detail,
            }),
        )
            .into_response()
    }
}

impl From<AuthError> for ApiError {
    fn from(value: AuthError) -> Self {
        match value {
            AuthError::Unauthenticated => Self::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                value.to_string(),
            ),
            AuthError::Forbidden => {
                Self::new(StatusCode::FORBIDDEN, "forbidden", value.to_string())
            }
        }
    }
}

impl From<DiscordError> for ApiError {
    fn from(value: DiscordError) -> Self {
        match value {
            DiscordError::Refused(detail) => Self::new(StatusCode::BAD_REQUEST, "refused", detail),
            other => Self::new(StatusCode::BAD_GATEWAY, "discord_error", other.to_string()),
        }
    }
}

impl From<SignedUrlError> for ApiError {
    fn from(value: SignedUrlError) -> Self {
        let code = value.code();
        let status = match value {
            // Not the caller's fault and not fixable by retrying: the operator has to set a value
            // and restart. 503 says "this server is not able to do this right now", which is the
            // truth, and it is distinguishable from ElevenLabs having refused us.
            SignedUrlError::NotConfigured(_) => StatusCode::SERVICE_UNAVAILABLE,
            // An upstream failure, reported as one. Not flattened into a 500, and never into a
            // 200 with a URL that would fail later inside the browser.
            SignedUrlError::Transport(_)
            | SignedUrlError::Status { .. }
            | SignedUrlError::Shape(_) => StatusCode::BAD_GATEWAY,
        };
        // `Display` for every variant is already redacted at construction; see
        // `SignedUrlError::from_response`.
        Self::new(status, code, value.to_string())
    }
}

impl From<OpError> for ApiError {
    fn from(value: OpError) -> Self {
        let code = value.code();
        let status = match value {
            OpError::UnknownChannel | OpError::UnknownMessage => StatusCode::NOT_FOUND,
            OpError::EmptyQuery | OpError::InvalidCursor | OpError::InvalidRange => {
                StatusCode::BAD_REQUEST
            }
            OpError::ChannelNotWritable => StatusCode::FORBIDDEN,
            OpError::Discord(inner) => return Self::from(inner),
            OpError::Store(inner) => return Self::from(inner),
            OpError::Summarizer(inner) => {
                let code = inner.code();
                // 503 for both: a summariser that is not configured and one that cannot be
                // reached are equally "this server cannot do this right now", and neither is the
                // caller's fault. The CODE tells them apart, which is what a client branches on.
                return Self::new(StatusCode::SERVICE_UNAVAILABLE, code, inner.to_string());
            }
        };
        Self::new(status, code, value.to_string())
    }
}

impl From<StoreError> for ApiError {
    fn from(value: StoreError) -> Self {
        let code = value.code();
        let status = match value {
            // Not the caller's fault and not fixable by retrying: the operator has to configure a
            // path and restart. Same reasoning, and the same status, as a missing ElevenLabs key.
            StoreError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            StoreError::NotFound => StatusCode::NOT_FOUND,
            StoreError::BadId(_) => StatusCode::BAD_REQUEST,
            // 413 rather than 400: the request was well formed, it is the amount that is refused,
            // and the client's correct response is to stop appending rather than to retry.
            StoreError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            StoreError::Backend(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, code, value.to_string())
    }
}

fn require(headers: &HeaderMap, state: &AppState, scope: Scope) -> Result<Scope, ApiError> {
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    Ok(auth::authorize(header, &state.config.auth, scope)?)
}

/// `GET /healthz` — liveness only. Deliberately says nothing about the configuration.
pub async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "gent-talk",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Fallback for unrouted paths.
pub async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such route")
}

/// Channel listing.
#[derive(Debug, Serialize)]
pub struct ChannelsResponse {
    /// Configured channels, each carrying the operator's local alias when he has set one.
    pub channels: Vec<ChannelInfo>,
}

/// `GET /api/v1/channels`
pub async fn list_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ChannelsResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    Ok(Json(ChannelsResponse {
        channels: ops::channels(&state).await,
    }))
}

/// The name the operator wants to call a channel. `#39 channel-alias`.
#[derive(Debug, Deserialize)]
pub struct SetAliasRequest {
    /// What to call it. Trimmed and length-checked by [`crate::store::validate_alias`]; blank is
    /// refused rather than treated as a clear, because DELETE is the clear.
    pub alias: String,
}

/// What this server is now calling a channel, and what it is called elsewhere.
#[derive(Debug, Serialize)]
pub struct AliasResponse {
    /// The channel, with `alias` set or cleared as the call asked.
    pub channel: ChannelInfo,
    /// Said on every alias answer for the same reason [`crate::store::INBOX_NOTICE`] is said on
    /// every inbox answer: this is the place a person expects the name to have changed in Discord.
    pub alias_notice: &'static str,
}

/// The standing statement that a channel alias is local and ours.
pub const ALIAS_NOTICE: &str = "A channel alias is gent-talk's own name for the channel. It is \
                                stored on this server only: Discord is not told, the channel is \
                                not renamed there, and nobody outside this deployment sees it. \
                                Clearing it puts the configured label back.";

/// `PUT /api/v1/channels/{channel_id}/alias`
///
/// WRITE scope, like every other durable write here. **The scope is not what keeps the agent
/// out**, and saying so would be a comfortable overstatement: a hosted voice agent is routinely
/// given the write token, because `post_reply` is a write tool it is meant to use. What keeps a
/// model from renaming the channels it is also reporting on is that there is NO TOOL for it —
/// [`crate::mcp::tool_manifest`] offers none, and [`crate::mcp::protocol::dispatch`] refuses any
/// name it cannot find there. The model is handed the alias and cannot choose it.
pub async fn set_alias(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<SetAliasRequest>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let channel = ops::set_channel_alias(&state, &channel_id, &request.alias).await?;
    Ok(no_store(Json(AliasResponse {
        channel,
        alias_notice: ALIAS_NOTICE,
    })))
}

/// `DELETE /api/v1/channels/{channel_id}/alias` — go back to the configured label.
pub async fn clear_alias(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let channel = ops::clear_channel_alias(&state, &channel_id).await?;
    Ok(no_store(Json(AliasResponse {
        channel,
        alias_notice: ALIAS_NOTICE,
    })))
}

/// `GET /api/v1/agent-tools` — the voice agent's tool manifest.
pub async fn agent_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let manifest = crate::mcp::tool_manifest(&ops::channels(&state).await);
    Ok(Json(serde_json::json!({ "tools": manifest })))
}

/// What the web app needs to know at startup.
#[derive(Debug, Serialize)]
pub struct ClientConfigResponse {
    /// Configured channels.
    pub channels: Vec<ChannelInfo>,
    /// ElevenLabs agent id, when the deployment has one. Not a secret: it identifies a public
    /// widget. The API key never leaves the server.
    pub elevenlabs_agent_id: Option<String>,
    /// Server version, so a stale cached page is visible.
    pub version: &'static str,
    /// Whether the operator has allowed an earlier transcript to be replayed into a new call.
    ///
    /// The page needs it BEFORE it has anything to replay: the Settings screen has to describe
    /// what resuming will do, and the control's own note has to say what the next call will be —
    /// both of which are questions asked while there is no conversation to fetch.
    pub replay_enabled: bool,
    /// Seconds between live ingestion ticks, or `0` when live ingestion is OFF.
    ///
    /// The page needs this to tell the truth about its own channel view. With ingestion off the
    /// SSE stream attaches and delivers nothing, which is indistinguishable from a quiet channel;
    /// a page that showed a "live" indicator on that basis would be claiming a freshness it does
    /// not have. Not a secret: it is a property of this deployment, and the caller already holds
    /// a token.
    pub live_poll_seconds: u64,
}

/// `GET /api/v1/client-config`
pub async fn client_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ClientConfigResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    Ok(Json(ClientConfigResponse {
        channels: ops::channels(&state).await,
        elevenlabs_agent_id: state.config.elevenlabs.agent_id.clone(),
        version: env!("CARGO_PKG_VERSION"),
        live_poll_seconds: state.config.discord.live_poll_seconds,
        replay_enabled: state.config.replay.enabled,
    }))
}

/// `GET /api/v1/signed-url` — mint a short-lived signed conversation URL for the voice agent.
///
/// Two things about this route are deliberate.
///
/// **It requires the WRITE scope**, even though it reads nothing. What it hands back is a working
/// conversation with the agent, and that agent is configured with a credential of its own: if it
/// holds the write token, then anyone holding a signed URL can ask it to post in the owner's name.
/// The gate on this route therefore has to be at least as strong as the strongest thing the
/// conversation can do. An unauthenticated version of this route would be strictly worse than the
/// public `talk-to` link that enabling authentication just closed.
///
/// **The answer is `no-store`.** The minted URL is itself a bearer credential for the next fifteen
/// minutes; it must not sit in a proxy or a browser cache.
pub async fn signed_url(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let minted: SignedUrl = state
        .elevenlabs
        .signed_url(&state.config.elevenlabs)
        .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(minted)).into_response())
}

/// Query parameters for the read endpoints.
#[derive(Debug, Default, Deserialize)]
pub struct LimitQuery {
    /// How many recent messages to consider.
    pub limit: Option<u16>,
    /// Width of a digest line, in characters.
    pub width: Option<u16>,
}

/// Full-text scrollback.
#[derive(Debug, Serialize)]
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
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/channels/{channel_id}/messages`
pub async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<MessagesResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let window = ops::messages(&state, &channel_id, query.limit).await?;
    let dismissed = ops::dismissed_within(&state, &window.channel.id, &window.messages).await?;
    Ok(Json(MessagesResponse {
        complete: window.is_whole_channel(),
        channel: window.channel,
        messages: window.messages,
        dismissed,
        untrusted_content_notice: untrusted::NOTICE,
    }))
}

/// Query parameters for one step of a walk.
#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    /// How many messages this step should return.
    pub limit: Option<u16>,
    /// Step back from this message id, exclusive.
    pub before: Option<String>,
    /// Start of a time span, inclusive, ISO-8601.
    pub since: Option<String>,
    /// End of a time span, exclusive, ISO-8601. Requires `since`.
    pub until: Option<String>,
}

/// One step of a walk, saying plainly that it is one.
#[derive(Debug, Serialize)]
pub struct PageResponse {
    /// Channel that was read.
    pub channel: ChannelInfo,
    /// Messages, oldest first.
    pub messages: Vec<Message>,
    /// How many this step returned. Never a channel total.
    pub returned: usize,
    /// The page size that applied.
    pub limit: u16,
    /// Whether messages exist beyond this page in the direction of travel.
    pub has_more: bool,
    /// Hand this back as `before` to take the next step back. Absent when there is nothing more.
    pub next_before: Option<String>,
    /// Hand this back as `since` to take the next step of a range walk.
    pub next_since: Option<String>,
    /// Which of `messages` the reader has ARCHIVED, so the view can dim them.
    ///
    /// The ordinary channel view shows archived rows greyed rather than hidden -- hiding is what
    /// the To do filter is for -- and it can only do that if the payload says which they are.
    pub dismissed: Vec<MessageId>,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/channels/{channel_id}/page`
pub async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PageResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let step = ops::page(
        &state,
        &channel_id,
        ops::PageRequest {
            limit: query.limit,
            before: query.before.as_deref(),
            since: query.since.as_deref(),
            until: query.until.as_deref(),
        },
    )
    .await?;
    let dismissed = ops::dismissed_within(&state, &step.channel.id, &step.messages).await?;
    Ok(Json(PageResponse {
        returned: step.returned(),
        channel: step.channel,
        messages: step.messages,
        dismissed,
        limit: step.limit,
        has_more: step.has_more,
        next_before: step.next_before.map(|id| id.0),
        next_since: step.next_since,
        untrusted_content_notice: untrusted::NOTICE,
    }))
}

/// Query parameters for a bounded count.
#[derive(Debug, Default, Deserialize)]
pub struct CountQuery {
    /// Count only messages at or after this ISO-8601 instant.
    pub since: Option<String>,
    /// Stop after this many. Clamped by `discord.max_count_scan`.
    pub cap: Option<u32>,
}

/// A bounded count, with the honesty flag attached.
#[derive(Debug, Serialize)]
pub struct CountResponse {
    /// Channel that was counted.
    pub channel: ChannelInfo,
    /// How many messages were seen.
    pub counted: usize,
    /// When true, `counted` is a LOWER BOUND: the cap stopped the walk before the channel ran out.
    pub at_least: bool,
    /// The ceiling that applied.
    pub cap: u32,
    /// Oldest message the walk reached.
    pub oldest_seen: Option<String>,
    /// Newest message the walk started from.
    pub newest_seen: Option<String>,
}

/// `GET /api/v1/channels/{channel_id}/count`
pub async fn count(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Query(query): Query<CountQuery>,
) -> Result<Json<CountResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let tally = ops::count(&state, &channel_id, query.since.as_deref(), query.cap).await?;
    Ok(Json(CountResponse {
        channel: tally.channel,
        counted: tally.counted,
        at_least: tally.at_least,
        cap: tally.cap,
        oldest_seen: tally.oldest_seen.map(|id| id.0),
        newest_seen: tally.newest_seen.map(|id| id.0),
    }))
}

/// One message, in full.
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    /// Channel that was read.
    pub channel: ChannelInfo,
    /// The message.
    pub message: Message,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/channels/{channel_id}/messages/{message_id}`
///
/// The lookup is within the recent window this server fetches, so a very old message answers 404
/// rather than silently returning something else.
pub async fn message_by_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((channel_id, message_id)): Path<(String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<MessageResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let (channel, message) =
        ops::message_by_id(&state, &channel_id, &message_id, query.limit).await?;
    Ok(Json(MessageResponse {
        channel,
        message,
        untrusted_content_notice: untrusted::NOTICE,
    }))
}

/// A speakable channel digest.
#[derive(Debug, Serialize)]
pub struct DigestResponse {
    /// Channel that was read.
    pub channel: ChannelInfo,
    /// One line per message, oldest first.
    pub entries: Vec<DigestEntry>,
    /// Whether the fetch came back short, so `entries.len()` counts the CHANNEL rather than the
    /// window. See [`MessagesResponse::complete`].
    pub complete: bool,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/channels/{channel_id}/digest`
pub async fn digest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<DigestResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let (channel, entries, complete) =
        ops::digest(&state, &channel_id, query.limit, query.width).await?;
    Ok(Json(DigestResponse {
        channel,
        entries,
        complete,
        untrusted_content_notice: untrusted::NOTICE,
    }))
}

/// A semantic-random-access request: describe the message you want.
#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    /// The description, in the speaker's own words.
    pub query: String,
    /// How many recent messages to search.
    pub limit: Option<u16>,
    /// How many runners-up to return.
    pub max_alternatives: Option<u16>,
}

/// The answer to a resolve request.
#[derive(Debug, Serialize)]
pub struct ResolveResponse {
    /// Channel that was searched.
    pub channel: ChannelInfo,
    /// What was asked for.
    pub query: String,
    /// The match, its runners-up, and whether it was close.
    #[serde(flatten)]
    pub resolution: Resolution,
    /// How many messages were searched, so a caller can say "I only looked at the last 20".
    pub searched: usize,
    /// Standing reminder that the content is third-party text.
    pub untrusted_content_notice: &'static str,
}

/// `POST /api/v1/channels/{channel_id}/resolve` — semantic random access.
pub async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<ResolveRequest>,
) -> Result<Json<ResolveResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let (channel, resolution, searched) = ops::resolve(
        &state,
        &channel_id,
        &request.query,
        request.limit,
        request.max_alternatives,
    )
    .await?;
    Ok(Json(ResolveResponse {
        channel,
        query: request.query,
        resolution,
        searched,
        untrusted_content_notice: untrusted::NOTICE,
    }))
}

/// A request to post into a channel.
#[derive(Debug, Deserialize)]
pub struct ReplyRequest {
    /// The text to post.
    pub text: String,
    /// Message being replied to, when any.
    pub reply_to: Option<String>,
}

/// The result of posting.
#[derive(Debug, Serialize)]
pub struct ReplyResponse {
    /// The message as Discord accepted it.
    pub posted: Message,
}

/// `POST /api/v1/channels/{channel_id}/reply` — the only route that speaks in the owner's name.
pub async fn reply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<ReplyRequest>,
) -> Result<Json<ReplyResponse>, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let (_channel, posted) = ops::reply(
        &state,
        &channel_id,
        &request.text,
        request.reply_to.as_deref(),
    )
    .await?;
    Ok(Json(ReplyResponse { posted }))
}

/// A slow-path question for the coding agents.
#[derive(Debug, Deserialize)]
pub struct AskRequest {
    /// The question.
    pub question: String,
}

/// `POST /api/v1/channels/{channel_id}/ask` — the slow-path seam. Not implemented in v0.
pub async fn ask(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<AskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let info = state
        .channel(&channel_id)
        .cloned()
        .ok_or(OpError::UnknownChannel)?;
    match state.agent.ask(&info.id, &request.question).await {
        Ok(answer) => Ok(Json(serde_json::json!({ "answer": answer }))),
        Err(error) => Err(ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "agent_backend_unavailable",
            error.to_string(),
        )),
    }
}

/// `GET /api/v1/channels/{channel_id}/stream` — Server-Sent Events for one channel.
///
/// Read scope, and the same allowlist as every other channel route: see [`ops::watch`] for why the
/// gate is there rather than here. A channel outside the configuration answers `404
/// unknown_channel`, never a 200 that streams nothing.
///
/// **`Last-Event-ID` is honoured.** A browser reconnecting sends back the last `id:` it saw and
/// gets only what came after it, out of the hub's bounded replay tail. That is what "reconnection
/// must not duplicate or drop messages" means in practice, and the two failure directions are
/// treated differently on purpose: a repeat is de-duplicated by id on the page, and a subscriber
/// that fell too far behind gets one `event: reset` and the stream ENDS, so the page re-reads
/// through `/messages` rather than silently skipping the gap.
///
/// **Every event says whether it is `replayed`.** An attach with no `Last-Event-ID` gets the whole
/// tail, and on the wire a replayed message is otherwise indistinguishable from one that arrived a
/// moment ago — so a page could not avoid announcing two hundred old messages to a live, billed
/// conversation as news. See [`crate::live::events`].
///
/// **`Cache-Control: no-store`** because the body is channel text belonging to one credential, and
/// **`X-Accel-Buffering: no`** because an nginx-shaped reverse proxy in front of this will
/// otherwise buffer the response and deliver nothing until it is large enough — which presents as
/// a stream that works locally and is dead behind the tunnel.
///
/// One more thing a reader of the access log needs: **this route leaves exactly one line, at
/// attach, with `millis=0`**. [`crate::http::access_layer::log_requests`] returns as soon as the
/// status is known, and for a streaming body that is before a single event has gone out — so a
/// stream held open for an hour still logs zero milliseconds. A test in `tests/logging.rs` pins
/// that rather than leaving it to be misread as an instant request.
pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| crate::model::MessageId(value.to_owned()));
    let (_channel, subscription) = ops::watch(&state, &channel_id, after.as_ref())?;
    let body = axum::response::sse::Sse::new(crate::live::events(subscription)).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    );
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (
                axum::http::HeaderName::from_static("x-accel-buffering"),
                "no",
            ),
        ],
        body,
    )
        .into_response())
}

const INDEX_HTML: &str = include_str!("../../web/index.html");
const VOICE_HTML: &str = include_str!("../../web/voice.html");
const VOICE_JS: &str = include_str!("../../web/voice.js");
const VOICE_CSS: &str = include_str!("../../web/voice.css");
const APP_JS: &str = include_str!("../../web/app.js");
const STYLE_CSS: &str = include_str!("../../web/style.css");

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            // The app is a single page with no third-party anything; say so.
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// `GET /` — the phone web app.
pub async fn index_html() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML)
}

/// `GET /voice` — the minimal page that starts an authenticated conversation.
pub async fn voice_html() -> Response {
    asset("text/html; charset=utf-8", VOICE_HTML)
}

/// `GET /voice.js`
pub async fn voice_js() -> Response {
    asset("text/javascript; charset=utf-8", VOICE_JS)
}

/// `GET /voice.css`
///
/// Held apart from `style.css` on purpose: it turns the document into a fixed application frame
/// (`100dvh`, no page scroll), and `/` is an ordinary scrolling page that must not inherit that.
pub async fn voice_css() -> Response {
    asset("text/css; charset=utf-8", VOICE_CSS)
}

/// `GET /app.js`
pub async fn app_js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}

/// `GET /style.css`
pub async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

#[cfg(test)]
mod tests {
    use super::{APP_JS, VOICE_JS};

    #[test]
    fn the_voice_page_pluralizes_and_prefers_the_operators_own_clock() {
        // Both defects were fixed in web/app.js first and never carried across, and this guard
        // only covered app.js — so the page the owner actually uses kept printing "message(s)"
        // and UTC while the suite stayed green. Guarding one of two assets that render the same
        // two fields is not a guard; it is a coin flip about which file someone edits next.
        assert!(
            !VOICE_JS.contains("(s)"),
            "web/voice.js still renders a parenthesised plural"
        );
        assert!(
            VOICE_JS.contains("function channelSummary("),
            "the count helper is what keeps the fetch window from being read as a channel total"
        );
        assert!(
            VOICE_JS.contains("complete !== true"),
            "a server too old to send `complete` must be treated as unknown, not as complete"
        );
        assert!(
            VOICE_JS.contains("message.spoken_time ||"),
            "the channel view must prefer the zone-converted time the server already computed"
        );
    }

    /// The text of one top-level function in a served asset, up to the next one.
    ///
    /// Crude on purpose: it is enough to ask "does THIS function call that one", which is the
    /// only question the guard below has, and it needs no parser to answer it.
    ///
    /// `async function` counts as the next one. It did not used to, and the consequence was that
    /// a body ran on through every `async` declaration after it until the next synchronous one —
    /// so an assertion about one function was quietly an assertion about three. web/app.js is
    /// mostly `async`, which is where that first mattered.
    fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
        let start = source
            .find(signature)
            .unwrap_or_else(|| panic!("the asset no longer defines `{signature}`"));
        let rest = &source[start + signature.len()..];
        let end = ["\nfunction ", "\nasync function "]
            .iter()
            .filter_map(|marker| rest.find(marker))
            .min()
            .unwrap_or(rest.len());
        &rest[..end]
    }

    #[test]
    fn both_message_lists_on_the_voice_page_fold_long_messages_the_same_way() {
        // `#47 scrollback-stability`. The page renders two message lists — the voice transcript
        // and the channel view — and the whole point of the folding work is that they behave
        // IDENTICALLY, because they sit one switch apart and a reader moves between them without
        // thinking about it. That is not a property of a shared helper existing; it is a property
        // of both call sites using it. A fix landed on one list and not the other is the exact
        // defect this file's other guard was written for, and it cost a round trip that time.
        assert!(
            VOICE_JS.contains("function foldable("),
            "the folding idiom must be ONE function, not a behaviour each list implements"
        );
        for renderer in ["function line(", "function discordNode("] {
            assert!(
                function_body(VOICE_JS, renderer).contains("foldable("),
                "`{renderer}` in web/voice.js no longer folds long messages, so the two message \
                 lists on /voice disagree about what a long message looks like"
            );
        }
        // And the anchoring that goes with it: an arrival must be conditional, never the old
        // unconditional jump.
        assert!(
            VOICE_JS.contains("function followIfPinned("),
            "an arrival must follow the newest line only when the reader was already there"
        );
        // `function seam(` used to be in this list. `#63 status-line-placement` split BUILDING a
        // seam from PLACING one — the channel's summary is a seam at the head of its own list, and
        // placing it at the end of the transcript would be wrong — so the thing that appends to
        // the transcript, and therefore the thing that has to decide whether to follow, is
        // `transcriptSeam`. The property asserted is unchanged; the name of the function that has
        // to satisfy it moved.
        for renderer in ["function line(", "function transcriptSeam("] {
            let body = function_body(VOICE_JS, renderer);
            assert!(
                body.contains("followIfPinned("),
                "`{renderer}` in web/voice.js appends without deciding whether to follow"
            );
            assert!(
                !body.contains("scrollToNewest("),
                "`{renderer}` in web/voice.js scrolls on every arrival again, which is the \
                 defect: a reader looking at older messages gets thrown to the bottom"
            );
        }
    }

    #[test]
    fn typed_input_on_the_voice_page_has_exactly_one_send_path() {
        // `#43 typed-input`. The same argument as the folding guard above, one issue later. Three
        // things now put a client event on the conversation socket — the composer's Send, the
        // Enter key, and the presence ping while somebody is typing — and `#59 text-entry-button`
        // and `#60 canned-prompt-buttons` add more. Each one that grows its own
        // `socket.send(JSON.stringify(...))` is a place the guards can drift: the readyState
        // check, the visible refusal when there is no call, the local echo of the turn and the
        // de-duplication of the vendor's echo all live in these two functions, and a second copy
        // gets some subset of them.
        //
        // So: one writer, and the composer's callers go through it.
        assert!(
            VOICE_JS.contains("function sendClientEvent("),
            "the one place a JSON client event reaches the conversation socket is gone"
        );
        for caller in ["function sendUserMessage(", "function noteComposing("] {
            assert!(
                function_body(VOICE_JS, caller).contains("sendClientEvent("),
                "`{caller}` in web/voice.js writes to the socket by some other route, so the \
                 readyState guard and the visible refusal are no longer on every send"
            );
        }
        // The audio path is the ONE deliberate exception, and it is documented as one in
        // web/voice.js: it is called every 4096 samples, holds the socket in a closure and does
        // its own readyState check. Pinned so that "the exception" cannot quietly become "the
        // rule" by a second hot-path send appearing beside it.
        assert!(
            function_body(VOICE_JS, "function startCapture(").contains("user_audio_chunk"),
            "the microphone frames no longer go out from startCapture"
        );
        assert_eq!(
            VOICE_JS.matches("user_audio_chunk:").count(),
            1,
            "web/voice.js writes microphone frames from more than one place"
        );
        // A typed turn is rendered locally because the vendor may never echo it back, which makes
        // suppressing the echo — if there is one — the other half of the same decision. Losing
        // either half puts the sentence on screen twice or not at all.
        let typed = function_body(VOICE_JS, "function sendUserMessage(");
        assert!(
            typed.contains("line(\"you\""),
            "a typed turn is no longer rendered when it is sent, so it exists only if the vendor \
             chooses to echo it"
        );
        assert!(
            VOICE_JS.contains("function isEchoOfTyped("),
            "nothing suppresses the vendor's echo of a typed turn, so it can render twice"
        );
    }

    #[test]
    fn the_phone_app_pluralizes_and_never_prints_the_placeholder_form() {
        // `#62 message-count-accuracy`. `message(s)` is the shape of the defect: it is what a
        // renderer writes when it has not decided whether it knows the number. The bytes asserted
        // on here are exactly the bytes `GET /app.js` serves.
        assert!(
            !APP_JS.contains("(s)"),
            "web/app.js still renders a parenthesised plural"
        );
        assert!(
            APP_JS.contains("function messageCount("),
            "the count helper is what keeps the two call sites honest and identical"
        );
        assert!(
            APP_JS.contains("complete !== true"),
            "a server too old to send `complete` must be treated as unknown, not as complete"
        );
    }

    #[test]
    fn the_phone_app_shows_the_time_the_server_already_converted() {
        // `#52 operator-timezone`. The phone and the voice agent must say the same thing about
        // when a message arrived; the only way to guarantee that is for both to render the one
        // string the server computed, rather than each slicing the ISO value their own way.
        assert!(
            APP_JS.contains("message.spoken_time ||"),
            "web/app.js must prefer the server-converted time, with the ISO slice as fallback"
        );
        assert!(
            APP_JS.contains("spoken_time: entry.spoken_time"),
            "a digest entry carries the converted time too; dropping it here would make the \
             digest list silently fall back to UTC while the scrollback shows local time"
        );
    }

    /// Every place either served page names a channel, and the one function that decides the name.
    ///
    /// The list is the point. `#52 operator-timezone` and `#62 message-count-accuracy` were each
    /// fixed on one of these two files and carried across afterwards, at the cost of a round trip
    /// both times, and `#39 channel-alias` walked into it a third time: the alias reached
    /// /voice and the voice agent while `/` went on printing the configured label, so one
    /// deployment showed two names for one channel.
    const NAMES_A_CHANNEL: [(&str, &str, &[&str]); 2] = [
        (
            "web/app.js",
            APP_JS,
            // Both pickers are filled by one function; the header is written on both read paths.
            &[
                "function fillChannelSelects(",
                "function loadDigest(",
                "function loadScrollback(",
            ],
        ),
        (
            "web/voice.js",
            VOICE_JS,
            // Both pickers again, the head of the channel on each of its three writers, and the
            // head of the to-do list.
            &[
                "function fillChannelSelect(",
                "function loadDiscord(",
                "function loadOlder(",
                "function restateChannelSeam(",
                "function todoSummary(",
            ],
        ),
    ];

    #[test]
    fn both_served_pages_call_a_channel_what_the_operator_called_it() {
        // `#39 channel-alias`. `ChannelInfo::display_name` is the rule on this side of the wire;
        // `channelName` is the same rule in the browser, and it exists twice because two served
        // assets with no build step between them cannot share a module. That is exactly the
        // arrangement the guards above were written for, so it gets the same treatment: the RULE
        // is asserted over the bytes of BOTH files, and so is every call site of it.
        for (name, source, renderers) in NAMES_A_CHANNEL {
            let rule = function_body(source, "function channelName(");
            assert!(
                rule.contains("channel.alias"),
                "{name} defines channelName without consulting the operator's own name for the \
                 channel"
            );
            assert!(
                rule.contains("channel.label"),
                "{name} defines channelName without the configured label as its fallback, so \
                 clearing an alias would leave the channel with no name at all"
            );
            for renderer in renderers {
                let body = function_body(source, renderer);
                assert!(
                    body.contains("channelName("),
                    "`{renderer}` in {name} names a channel without going through channelName, \
                     so this deployment can show two different names for one channel"
                );
                assert!(
                    !body.contains(".label"),
                    "`{renderer}` in {name} reads the configured label directly again, which is \
                     the defect: the alias is ignored wherever that is done"
                );
            }
        }
    }

    #[test]
    fn no_served_page_grows_a_second_opinion_about_a_channels_name() {
        // The other half, and the half that keeps the list above honest: a call site added later
        // is not covered by a list written today. `.label` is the shape of the defect, so it is
        // allowed in exactly two places — the fallback inside `channelName`, and the alias editor
        // on /voice, which names the configured label deliberately so the owner can see what
        // clearing would go back to.
        assert_eq!(
            APP_JS.matches(".label").count(),
            1,
            "web/app.js reads a channel's configured label somewhere other than the fallback \
             inside channelName"
        );
        assert_eq!(
            VOICE_JS.matches(".label").count(),
            3,
            "web/voice.js reads a channel's configured label somewhere other than the fallback \
             inside channelName and the two sentences the alias editor shows"
        );
        assert_eq!(
            function_body(VOICE_JS, "function renderAliasEditor(")
                .matches(".label")
                .count(),
            2,
            "the alias editor on /voice must be where the other two readings of the configured \
             label are; if they moved, this count is guarding the wrong thing"
        );
    }
}

// --- durable state: the /voice transcript, and this server's own inbox ------------------------
//
// These five conversation routes are the only handlers on this server that do not consult
// `ops::allowed`, because a conversation is not a channel: it is the owner's own session with
// the agent, held on this page, and there is no allowlist for it. What takes the place of that
// gate is `ConversationId::parse`, which every one of them runs first.
//
// ALL OF THEM REQUIRE THE WRITE SCOPE, including the reads. A transcript is the owner's own
// speech plus whatever channel text the agent read aloud to him, which is strictly more sensitive
// than a digest of a channel he already allowlisted — and /voice, the only thing that uses these,
// already holds the write token. A read-scope token deliberately gets 403 here, and a test
// asserts that rather than assuming it.
//
// NONE OF THEM IS AN MCP TOOL, and a test in tests/mcp.rs holds that line. The model does not
// need the owner's transcript to answer a question about a channel, and text that reaches a model
// is text that leaves this machine.

/// A stored conversation listing.
#[derive(Debug, Serialize)]
pub struct ConversationsResponse {
    /// Conversations, most recently active first.
    pub conversations: Vec<ConversationSummary>,
    /// Standing reminder that a transcript quotes third-party channel text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/conversations`
pub async fn list_conversations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let conversations = state.store.conversations().await?;
    Ok(no_store(Json(ConversationsResponse {
        conversations,
        untrusted_content_notice: untrusted::NOTICE,
    })))
}

/// One stored conversation, in full.
#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    /// The conversation's id.
    pub id: ConversationId,
    /// Its turns, oldest first.
    pub turns: Vec<Turn>,
    /// Standing reminder that a transcript quotes third-party channel text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/conversations/{conversation_id}`
pub async fn conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let id = ConversationId::parse(&conversation_id)?;
    let turns = state.store.turns(&id).await?;
    Ok(no_store(Json(ConversationResponse {
        id,
        turns,
        untrusted_content_notice: untrusted::NOTICE,
    })))
}

/// A reconstruction of an earlier conversation, and everything needed to describe it honestly.
#[derive(Debug, Serialize)]
pub struct ReplayResponse {
    /// The payload to send on the new socket. EMPTY when there is nothing to resume from.
    pub text: String,
    /// How many turns are in it.
    pub included: usize,
    /// How many older turns were left out. Non-zero means the interface must say "in part".
    pub dropped: usize,
    /// Whether the budget, rather than the transcript running out, is what stopped it.
    pub truncated: bool,
    /// The budget that produced it.
    pub policy: crate::replay::ReplayPolicy,
    /// How the page should hand `text` to the vendor.
    pub transport: crate::replay::Transport,
    /// Whether the operator has turned resuming on at all.
    ///
    /// Reported rather than answered with a 404, because "off" and "nothing to resume from" are
    /// different things the page has to say differently. A route that refused would collapse them.
    pub enabled: bool,
    /// Standing reminder that a transcript quotes third-party channel text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/conversations/{conversation_id}/replay`
///
/// **Write scope, like every other conversation route**, and for the same reason: what comes back
/// is the transcript itself, rendered. If a read token could fetch this it could fetch the
/// transcript, and the block comment above says why it may not.
///
/// Server-side rather than in the page, for two reasons. The budget policy is then testable in
/// Rust and shared with the billed harness that is the only thing able to say whether the vendor
/// honours the payload at all; and the transcript already lives here, so building it in the
/// browser would mean shipping the whole record to the page in order to throw most of it away.
///
/// **Disabled is a 200, not a refusal.** `enabled: false` with an empty `text` is a state the page
/// renders ("the agent starts fresh"); a 404 would be indistinguishable from a conversation that
/// was never stored, and the page would say the wrong one of those two things.
pub async fn replay(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let id = ConversationId::parse(&conversation_id)?;
    let settings = &state.config.replay;
    // Not fetched at all when resuming is off. The point of the setting is that prior conversation
    // content is not re-sent to a vendor, and a server that assembled the payload anyway and let
    // the page decide would have already read the whole transcript to do it.
    let built = if settings.enabled {
        let turns = state.store.turns(&id).await?;
        crate::replay::build(&turns, &settings.policy)
    } else {
        crate::replay::build(&[], &settings.policy)
    };
    Ok(no_store(Json(ReplayResponse {
        text: built.text,
        included: built.included,
        dropped: built.dropped,
        truncated: built.truncated,
        policy: built.policy,
        transport: settings.transport,
        enabled: settings.enabled,
        untrusted_content_notice: untrusted::NOTICE,
    })))
}

/// One turn to record.
#[derive(Debug, Deserialize)]
pub struct AppendTurnRequest {
    /// Who said it.
    pub speaker: Speaker,
    /// What was said.
    pub text: String,
}

/// What the store did with it.
#[derive(Debug, Serialize)]
pub struct AppendTurnResponse {
    /// The conversation it went into.
    pub id: ConversationId,
    /// The turn as stored, including the SERVER's timestamp — the page must render this rather
    /// than its own clock, or a restored transcript is stamped with the moment it was reloaded.
    pub turn: Turn,
}

/// `POST /api/v1/conversations/{conversation_id}/turns`
pub async fn append_turn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
    Json(request): Json<AppendTurnRequest>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let id = ConversationId::parse(&conversation_id)?;
    let turn = Turn::now(request.speaker, request.text);
    state.store.append_turn(&id, &turn).await?;
    Ok(no_store(Json(AppendTurnResponse { id, turn })))
}

/// `DELETE /api/v1/conversations/{conversation_id}`
pub async fn forget_conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let id = ConversationId::parse(&conversation_id)?;
    state.store.forget_conversation(&id).await?;
    Ok(no_store(Json(serde_json::json!({ "forgotten": 1 }))))
}

/// `DELETE /api/v1/conversations`
pub async fn forget_conversations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let forgotten = state.store.forget_all_conversations().await?;
    Ok(no_store(Json(
        serde_json::json!({ "forgotten": forgotten }),
    )))
}

/// `DELETE /api/v1/storage` — erase EVERYTHING this server holds.
///
/// The operator's erase, and the only complete one there is over HTTP.
/// `DELETE /api/v1/conversations` clears transcripts and deliberately leaves the read marks
/// alone, because the two are different records and a control that quietly does more than it
/// says is the failure this project is written against. This route says it does all of it: the
/// transcripts, this server's read marks, the cached summaries — which are the one thing on
/// disk written by third parties — and the per-message inbox overlay `#50 todo-view` adds.
///
/// Write scope, obviously. It exists because [`crate::store::StateStore::purge_everything`] was
/// otherwise trait surface with three implementations and no caller: an erase nobody can invoke
/// is not an erase, it is a claim in a document.
pub async fn purge_storage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    state.store.purge_everything().await?;
    Ok(no_store(Json(serde_json::json!({
        "purged": ["conversations", "read_marks", "summaries", "dismissals"],
        "detail": "every durable record this server holds has been erased; the store is still \
                   open and usable",
    }))))
}

/// This server's own inbox state.
#[derive(Debug, Serialize)]
pub struct InboxResponse {
    /// Every configured channel, marked or not.
    pub channels: Vec<ops::InboxEntry>,
    /// Said on every inbox answer, not buried in a document: this read state is not Discord's,
    /// in either direction. See [`crate::store::INBOX_NOTICE`].
    pub read_state_notice: &'static str,
}

/// `GET /api/v1/inbox`
pub async fn inbox(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let channels = ops::inbox(&state).await?;
    Ok(no_store(Json(InboxResponse {
        channels,
        read_state_notice: crate::store::INBOX_NOTICE,
    })))
}

/// How far to move a read mark.
#[derive(Debug, Deserialize)]
pub struct MarkReadRequest {
    /// The newest message the owner has been shown.
    pub message_id: String,
}

/// The mark after the move.
#[derive(Debug, Serialize)]
pub struct MarkReadResponse {
    /// The channel it is about.
    pub channel: ChannelInfo,
    /// The mark as it now stands. Not necessarily what was asked for: a mark never moves
    /// backwards, so a stale client is told where the mark really is.
    pub mark: ReadMark,
    /// See [`crate::store::INBOX_NOTICE`]. Repeated on the mutating route too, because this is
    /// exactly where someone expects the Discord badge to clear.
    pub read_state_notice: &'static str,
}

/// `POST /api/v1/channels/{channel_id}/read`
pub async fn mark_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<MarkReadRequest>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let (channel, mark) = ops::mark_read(
        &state,
        &channel_id,
        &crate::model::MessageId(request.message_id),
    )
    .await?;
    Ok(no_store(Json(MarkReadResponse {
        channel,
        mark,
        read_state_notice: crate::store::INBOX_NOTICE,
    })))
}

/// `DELETE /api/v1/channels/{channel_id}/read`
pub async fn forget_read_mark(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let channel = ops::forget_read_mark(&state, &channel_id).await?;
    Ok(no_store(Json(serde_json::json!({
        "channel": channel,
        "read_state_notice": crate::store::INBOX_NOTICE,
    }))))
}

/// The messages in one channel that have not been dealt with here.
#[derive(Debug, Serialize)]
pub struct TodoResponse {
    /// The to-do list itself: the channel, what is left, and how big the window was.
    #[serde(flatten)]
    pub todo: ops::TodoView,
    /// See [`crate::store::INBOX_NOTICE`]. On the LISTING as well as on the mutations, because
    /// this is the screen where "unread" means something to a Discord user and this is not that
    /// thing.
    pub read_state_notice: &'static str,
    /// The messages are somebody else's words, exactly as on every other read.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/channels/{channel_id}/todo`
pub async fn todo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let todo = ops::todo(&state, &channel_id, query.limit).await?;
    Ok(no_store(Json(TodoResponse {
        todo,
        read_state_notice: crate::store::INBOX_NOTICE,
        untrusted_content_notice: untrusted::NOTICE,
    })))
}

/// Which messages to mark as dealt with, or put back.
///
/// Exactly one of the two, and supplying both is a refusal rather than a precedence rule: a
/// caller that sent both meant one of them, and guessing which would be wrong half the time in a
/// way that quietly clears a backlog.
#[derive(Debug, Default, Deserialize)]
pub struct DismissRequest {
    /// The messages named one by one.
    #[serde(default)]
    pub messages: Vec<String>,
    /// Or: everything in the window at or before this one. The boundary is INCLUDED.
    #[serde(default)]
    pub through: Option<String>,
    /// The window the caller READ the list with, so `through` cannot clear more than was shown.
    ///
    /// The same number sent to `GET /todo?limit=`. Without it a client that paged three messages
    /// and gave up on them would clear the default fifty, because the boundary would be resolved
    /// against a window it never displayed — clearing work the owner never saw, which is the worst
    /// failure this view has. Ignored when `messages` names the ids one by one: there the window is
    /// only an existence check, so narrowing it could only refuse an id the caller really did see.
    #[serde(default)]
    pub limit: Option<u16>,
}

/// What a dismissal or a restoration actually did.
#[derive(Debug, Serialize)]
pub struct InboxChangeResponse {
    /// The channel and the exact set that changed.
    #[serde(flatten)]
    pub change: ops::InboxChange,
    /// How many messages changed state. The list beside it is what an undo needs; this is what a
    /// confirmation says out loud.
    pub count: usize,
    /// See [`crate::store::INBOX_NOTICE`]. Repeated on the mutating routes, because this is
    /// exactly where someone expects the Discord badge to move.
    pub read_state_notice: &'static str,
}

fn answered(change: ops::InboxChange) -> Response {
    let count = change.messages.len();
    no_store(Json(InboxChangeResponse {
        change,
        count,
        read_state_notice: crate::store::INBOX_NOTICE,
    }))
}

/// Why a dismissal that names both ways of choosing, or neither, is refused.
///
/// Held as constants so the match below stays one line per case and reads as the table of four it
/// is. Guessing between the two would be wrong half the time, and the wrong half CLEARS A BACKLOG.
const NAMES_BOTH_WAYS: &str = "send either `messages` or `through`, not both: they are different \
                               acts and guessing which one you meant would clear a backlog by \
                               accident";
const NAMES_NEITHER: &str = "send `messages` or `through`; an empty request would report success \
                             for having done nothing";

/// `POST /api/v1/channels/{channel_id}/dismiss`
pub async fn dismiss(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<DismissRequest>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    let change = match (request.through, request.messages.is_empty()) {
        (Some(_), false) => return Err(ApiError::bad_request(NAMES_BOTH_WAYS)),
        (Some(through), true) => {
            // WITH the window the caller read the list with. See `DismissRequest::limit`.
            let boundary = crate::model::MessageId(through);
            ops::declare_bankruptcy(&state, &channel_id, &boundary, request.limit).await?
        }
        (None, false) => {
            let wanted: Vec<crate::model::MessageId> = request
                .messages
                .into_iter()
                .map(crate::model::MessageId)
                .collect();
            ops::dismiss(&state, &channel_id, &wanted).await?
        }
        (None, true) => return Err(ApiError::bad_request(NAMES_NEITHER)),
    };
    Ok(answered(change))
}

/// `POST /api/v1/channels/{channel_id}/restore`
pub async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
    Json(request): Json<DismissRequest>,
) -> Result<Response, ApiError> {
    require(&headers, &state, Scope::Write)?;
    if request.messages.is_empty() {
        return Err(ApiError::bad_request(
            "send the `messages` an earlier dismissal reported; restoring nothing is not an undo",
        ));
    }
    let wanted: Vec<crate::model::MessageId> = request
        .messages
        .into_iter()
        .map(crate::model::MessageId)
        .collect();
    Ok(answered(ops::restore(&state, &channel_id, &wanted).await?))
}

/// One message, summarised.
#[derive(Debug, Serialize)]
pub struct MessageSummaryResponse {
    /// The channel it came from.
    pub channel: ChannelInfo,
    /// The message it is about.
    pub message_id: String,
    /// Whether it was below the threshold, served from the cache, or generated now. A page shows
    /// the original for `below_threshold`; a shortened copy of something already short would be
    /// a claim that work was done.
    #[serde(flatten)]
    pub outcome: ops::SummaryOutcome,
    /// Which backend produced it, said out loud so a page can never imply a model summary it did
    /// not get.
    pub backend: &'static str,
    /// The cache key's policy component. Changes the moment any setting that decides what a
    /// summary says changes.
    pub version: String,
    /// The length below which nothing is summarised, so a client can avoid asking at all.
    pub threshold_chars: usize,
    /// Standing reminder that a summary of third-party text is still third-party text.
    pub untrusted_content_notice: &'static str,
}

/// `GET /api/v1/channels/{channel_id}/messages/{message_id}/summary`
pub async fn message_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((channel_id, message_id)): Path<(String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<MessageSummaryResponse>, ApiError> {
    // The granted scope, not the required one: a read token may be served FROM the cache but
    // never writes to it. See `ops::summarize_message`.
    let caller = require(&headers, &state, Scope::Read)?;
    let (channel, outcome) =
        ops::summarize_message(&state, caller, &channel_id, &message_id, query.limit).await?;
    Ok(Json(MessageSummaryResponse {
        channel,
        message_id,
        outcome,
        backend: state.summarizer.describe(),
        version: state.summary_version.to_string(),
        threshold_chars: state.config.summaries.threshold_chars,
        untrusted_content_notice: untrusted::NOTICE,
    }))
}

/// Answer with `Cache-Control: no-store`.
///
/// Every durable-state answer is the owner's own speech or his reading position. Neither belongs
/// in a proxy cache or in a browser's back-forward cache, and the transcript routes are the first
/// on this server whose body is private to one person rather than merely credentialed.
fn no_store<T: IntoResponse>(body: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], body).into_response()
}
