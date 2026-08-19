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
use crate::model::{ChannelInfo, Message};
use crate::ops::{self, OpError};
use crate::retrieval::Resolution;
use crate::state::AppState;
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
            OpError::EmptyQuery => StatusCode::BAD_REQUEST,
            OpError::ChannelNotWritable => StatusCode::FORBIDDEN,
            OpError::Discord(inner) => return Self::from(inner),
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
    /// Configured channels.
    pub channels: Vec<ChannelInfo>,
}

/// `GET /api/v1/channels`
pub async fn list_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ChannelsResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    Ok(Json(ChannelsResponse {
        channels: state.config.channels.clone(),
    }))
}

/// `GET /api/v1/agent-tools` — the voice agent's tool manifest.
pub async fn agent_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    let manifest = crate::mcp::tool_manifest(&state.config.channels);
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
}

/// `GET /api/v1/client-config`
pub async fn client_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ClientConfigResponse>, ApiError> {
    require(&headers, &state, Scope::Read)?;
    Ok(Json(ClientConfigResponse {
        channels: state.config.channels.clone(),
        elevenlabs_agent_id: state.config.elevenlabs.agent_id.clone(),
        version: env!("CARGO_PKG_VERSION"),
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
    Ok(Json(MessagesResponse {
        complete: window.is_whole_channel(),
        channel: window.channel,
        messages: window.messages,
        untrusted_content_notice: untrusted::NOTICE,
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
    use super::APP_JS;

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
}
