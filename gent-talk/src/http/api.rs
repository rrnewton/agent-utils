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
use crate::model::{ChannelInfo, Message, MessageId};
use crate::retrieval::{self, Resolution};
use crate::state::AppState;
use crate::summary::{self, DigestEntry, DEFAULT_SUMMARY_CHARS};
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

fn require(headers: &HeaderMap, state: &AppState, scope: Scope) -> Result<Scope, ApiError> {
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    Ok(auth::authorize(header, &state.config.auth, scope)?)
}

fn channel<'a>(state: &'a AppState, id: &str) -> Result<&'a ChannelInfo, ApiError> {
    state.channel(id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_channel",
            "that channel is not configured on this server",
        )
    })
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
    let info = channel(&state, &channel_id)?.clone();
    let limit = state.effective_limit(query.limit);
    let messages = state.discord.fetch_recent(&info.id, limit).await?;
    Ok(Json(MessagesResponse {
        channel: info,
        messages,
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
    let info = channel(&state, &channel_id)?.clone();
    let limit = state.effective_limit(query.limit);
    let messages = state.discord.fetch_recent(&info.id, limit).await?;
    let wanted = MessageId(message_id);
    let message = messages
        .into_iter()
        .find(|m| m.id == wanted)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "unknown_message",
                "that message is not in the recent window for this channel",
            )
        })?;
    Ok(Json(MessageResponse {
        channel: info,
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
    let info = channel(&state, &channel_id)?.clone();
    let limit = state.effective_limit(query.limit);
    let messages = state.discord.fetch_recent(&info.id, limit).await?;
    let width = usize::from(query.width.unwrap_or(0));
    let width = if width == 0 {
        DEFAULT_SUMMARY_CHARS
    } else {
        width
    };
    Ok(Json(DigestResponse {
        channel: info,
        entries: summary::digest(&messages, width),
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
    let info = channel(&state, &channel_id)?.clone();
    if request.query.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_query",
            "query must not be empty",
        ));
    }
    let limit = state.effective_limit(request.limit);
    let messages = state.discord.fetch_recent(&info.id, limit).await?;
    let max_alternatives = usize::from(request.max_alternatives.unwrap_or(3)).min(10);
    let resolution = retrieval::resolve(
        state.ranker.as_ref(),
        &messages,
        &request.query,
        max_alternatives,
    );
    Ok(Json(ResolveResponse {
        channel: info,
        query: request.query,
        resolution,
        searched: messages.len(),
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
    let info = channel(&state, &channel_id)?.clone();
    if !info.writable {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "channel_not_writable",
            "this channel is configured read-only",
        ));
    }
    let reply_to = request.reply_to.map(MessageId);
    let posted = state
        .discord
        .post_message(&info.id, &request.text, reply_to.as_ref())
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
    let info = channel(&state, &channel_id)?.clone();
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

/// `GET /app.js`
pub async fn app_js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}

/// `GET /style.css`
pub async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}
