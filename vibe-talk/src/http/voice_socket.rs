//! `GET /api/v1/voice-socket` — the voice socket, relayed to a bridge only this server reaches.
//!
//! `#553 voice-ws-relay-auth`. A deployment-managed voice bridge speaks the public `vibe-talk-v1`
//! protocol and authenticates nobody. Published as it was, through a TLS front door that sent
//! `/ws` straight to it, anyone who could reach the door could talk to the voice agent and its chat
//! tools. With `conversation.relay_upstream` set the bridge stays on loopback and this route stands
//! in front of it: it settles who is asking, then dials the bridge and copies frames both ways.
//!
//! Who may open it, settled before anything else in the request is read ([`RelayCaller`]):
//!
//! * **a page, with a ticket** in the `ticket` query parameter, minted by the page's own
//!   (write-scope) voice-session call. A browser cannot put `Authorization` on a WebSocket, so the
//!   ticket stands in for it, once: see [`crate::voice_tickets`];
//! * **a server, with the voice-only token** (`auth.voice_token`) as `Authorization: Bearer`:
//!   another vibe-talk reading messages aloud through this one. It gets the socket WITHOUT chat
//!   tools: of what it sends only `mode` is forwarded (see [`upstream_for`]).
//!
//! Nothing else opens it, not even the read or write token: the page always comes with a ticket,
//! and a server has no business holding a browser token to read aloud. Every refusal is the same
//! 401 the rest of `/api/` gives, so the route says nothing about tickets to a caller without one.
//!
//! The bridge is dialed BEFORE the upgrade is accepted, so a bridge that is down is a 502 the
//! caller can read, not a socket that opens and closes at once. At most [`MAX_RELAYS`] sockets are
//! relayed at once (503 beyond), and a page that goes silent is noticed: the relay pings it every
//! [`PING_EVERY`] and closes both sides after [`CLIENT_SILENCE`] without a frame, as the bridge's own
//! keepalive would have done had it faced the page directly. Close frames, with their codes and
//! reasons, pass through in both directions.

use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;

use super::api::ApiError;
use crate::auth::{self, AuthError};
use crate::state::AppState;

/// The query parameter a page's ticket travels in.
pub const TICKET_PARAMETER: &str = "ticket";

/// The query parameter that gives the bridge's agent its chat tools; never forwarded for a server.
pub const TOOLS_PARAMETER: &str = "tools";

/// How long the bridge may take to accept the relay's connection.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest frame relayed in either direction, the bridge's own inbound cap.
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// The most sockets relayed at once, across every caller. A bridge for one owner serves a handful
/// of sessions; this bounds what a leaked voice token can open.
pub const MAX_RELAYS: usize = 16;

/// How often the relay pings the client.
pub const PING_EVERY: Duration = Duration::from_secs(20);

/// How long the client may send nothing at all, not even a pong, before both sides are closed.
pub const CLIENT_SILENCE: Duration = Duration::from_secs(45);

/// Sockets being relayed now; one permit each, held for the socket's life.
static RELAYS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_RELAYS);

/// Who opened the socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayCaller {
    /// A page, on a ticket its voice-session call minted.
    Page,
    /// A server, on the voice-only token.
    Server,
}

impl axum::extract::FromRequestParts<AppState> for RelayCaller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // A ticket, when one is sent, decides on its own: a dead one is refused even beside a
        // valid header, so a caller cannot learn which of the two it got wrong.
        if let Some(ticket) = ticket_in(parts.uri.query()) {
            return if state.voice_tickets.redeem(&ticket) {
                Ok(Self::Page)
            } else {
                Err(AuthError::Unauthenticated.into())
            };
        }
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        if auth::is_voice_token(header, &state.config.auth) {
            return Ok(Self::Server);
        }
        Err(AuthError::Unauthenticated.into())
    }
}

/// The `ticket` parameter's value, percent-decoded, when the query has one.
fn ticket_in(query: Option<&str>) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find(|(name, _)| name == TICKET_PARAMETER)
        .map(|(_, value)| value.into_owned())
}

/// The query parameter a server caller may choose: which of the bridge's modes it talks to.
pub const MODE_PARAMETER: &str = "mode";

/// The bridge URL for this caller.
///
/// A page's own query is forwarded, less the ticket; with none, `upstream`'s applies. A page's
/// query came from this server (the voice-session call) to a holder of the write token, so it may
/// ask for tools.
///
/// A server is held to an allowlist: `upstream`'s own query without any tools parameter, with at
/// most its own `mode` in place of the configured one, and nothing else of what it sent. An
/// allowlist rather than a filter, because a bridge's query parser may well read `Tools`,
/// `tools[]` or `tools ` as `tools`; any name that folds to one starting with `tools` is dropped
/// from the configured query too.
#[must_use]
pub fn upstream_for(upstream: &str, query: Option<&str>, caller: RelayCaller) -> String {
    let (base, own_query) = match upstream.split_once('?') {
        Some((base, own)) => (base, own),
        None => (upstream, ""),
    };
    let pairs = |text: &str| -> Vec<(String, String)> {
        url::form_urlencoded::parse(text.as_bytes())
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect()
    };
    let forwarded: Vec<(String, String)> = match caller {
        RelayCaller::Page => {
            let kept: Vec<_> = pairs(query.unwrap_or(""))
                .into_iter()
                .filter(|(name, _)| name != TICKET_PARAMETER)
                .collect();
            if kept.is_empty() {
                pairs(own_query)
            } else {
                kept
            }
        }
        RelayCaller::Server => {
            let mode = pairs(query.unwrap_or(""))
                .into_iter()
                .find(|(name, _)| name == MODE_PARAMETER);
            let mut kept: Vec<_> = pairs(own_query)
                .into_iter()
                .filter(|(name, _)| !names_tools(name))
                .filter(|(name, _)| mode.is_none() || name != MODE_PARAMETER)
                .collect();
            kept.extend(mode);
            kept
        }
    };
    if forwarded.is_empty() {
        return base.to_owned();
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(forwarded)
        .finish();
    format!("{base}?{query}")
}

/// Whether a parameter name could be read as `tools` by a lenient parser: case, brackets, dots,
/// spaces and any other non-alphanumerics aside.
fn names_tools(name: &str) -> bool {
    let folded: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    folded.starts_with(TOOLS_PARAMETER)
}

/// The page's socket URL for a session, with a fresh ticket: this route, carrying
/// `conversation.websocket_url`'s query (its `mode` and `tools`) and the ticket.
#[must_use]
pub fn page_url(websocket_url: Option<&str>, ticket: &str) -> String {
    let own_query = websocket_url
        .and_then(|url| url.split_once('?'))
        .map_or("", |(_, query)| query);
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in url::form_urlencoded::parse(own_query.as_bytes()) {
        if name != TICKET_PARAMETER {
            query.append_pair(&name, &value);
        }
    }
    query.append_pair(TICKET_PARAMETER, ticket);
    format!("/api/v1/voice-socket?{}", query.finish())
}

/// The relay itself.
///
/// # Errors
///
/// 401 for a caller without a live ticket or the voice token (from [`RelayCaller`]); 404
/// `voice_relay_not_configured` when `conversation.relay_upstream` is unset; 400
/// `not_a_websocket_upgrade` for a request that is not one; 502 `voice_relay_unreachable` when the
/// bridge cannot be dialed.
pub async fn relay(
    State(state): State<AppState>,
    caller: RelayCaller,
    mut request: Request,
) -> Result<Response, ApiError> {
    let Some(upstream) = state
        .config
        .conversation
        .relay_upstream
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "voice_relay_not_configured",
            "this server does not relay a voice socket; conversation.relay_upstream is not set",
        ));
    };
    let accept = accept_key(request.headers()).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "not_a_websocket_upgrade",
            "open this route as a WebSocket (GET with Upgrade: websocket, version 13)",
        )
    })?;
    let Ok(permit) = RELAYS.try_acquire() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "voice_relay_busy",
            "this server is relaying as many voice sockets as it allows; try again shortly",
        ));
    };
    let target = upstream_for(upstream, request.uri().query(), caller);
    let bridge = match tokio::time::timeout(
        DIAL_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(target, Some(frame_limits()), false),
    )
    .await
    {
        Ok(Ok((bridge, _))) => bridge,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "voice relay: the bridge refused the connection");
            return Err(unreachable_bridge());
        }
        Err(_) => {
            tracing::warn!("voice relay: the bridge did not accept a connection within 10 seconds");
            return Err(unreachable_bridge());
        }
    };
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => {
                let client = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    hyper_util::rt::TokioIo::new(upgraded),
                    Role::Server,
                    Some(frame_limits()),
                )
                .await;
                tracing::info!(caller = ?caller, "voice relay: socket open");
                pump(client, bridge).await;
                tracing::info!(caller = ?caller, "voice relay: socket closed");
                drop(permit);
            }
            Err(error) => {
                tracing::warn!(error = %error, "voice relay: the client's upgrade did not complete");
            }
        }
    });
    let mut response = StatusCode::SWITCHING_PROTOCOLS.into_response();
    let headers = response.headers_mut();
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
    Ok(response)
}

fn unreachable_bridge() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "voice_relay_unreachable",
        "the voice bridge behind this server did not accept a connection",
    )
}

fn frame_limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
}

/// `Sec-WebSocket-Accept` for a well-formed upgrade request, or `None` when it is not one.
fn accept_key(headers: &HeaderMap) -> Option<HeaderValue> {
    let has = |name: header::HeaderName, token: &str| {
        headers.get_all(name).iter().any(|value| {
            value.to_str().is_ok_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case(token))
            })
        })
    };
    if !has(header::UPGRADE, "websocket") || !has(header::CONNECTION, "upgrade") {
        return None;
    }
    if headers.get(header::SEC_WEBSOCKET_VERSION)?.as_bytes() != b"13" {
        return None;
    }
    let key = headers.get(header::SEC_WEBSOCKET_KEY)?;
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    HeaderValue::from_str(&accept).ok()
}

/// Copy frames both ways until either side closes, fails, or the client goes silent; then close
/// both. A close frame is passed on with its code and reason; pings and pongs are each hop's own
/// (tungstenite answers a ping itself), and the relay keeps its own watch on the client.
async fn pump<C, B>(client: C, bridge: B)
where
    C: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
    B: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
{
    let (mut client_tx, mut client_rx) = client.split();
    let (mut bridge_tx, mut bridge_rx) = bridge.split();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            message = client_rx.next() => {
                let Some(Ok(message)) = message else { break };
                heard = tokio::time::Instant::now();
                match message {
                    Message::Text(_) | Message::Binary(_) => {
                        if bridge_tx.send(message).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(frame) => {
                        let _ = bridge_tx.send(Message::Close(frame)).await;
                        break;
                    }
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
            message = bridge_rx.next() => {
                let Some(Ok(message)) = message else { break };
                match message {
                    Message::Text(_) | Message::Binary(_) => {
                        if client_tx.send(message).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(frame) => {
                        let _ = client_tx.send(Message::Close(frame)).await;
                        break;
                    }
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() >= CLIENT_SILENCE {
                    tracing::info!("voice relay: the client went silent; closing both sides");
                    break;
                }
                if client_tx.send(Message::Ping(bytes::Bytes::new())).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = client_tx.close().await;
    let _ = bridge_tx.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_keeps_its_query_less_the_ticket() {
        assert_eq!(
            upstream_for(
                "ws://127.0.0.1:9000/ws?mode=default",
                Some("mode=voice&tools=chat&ticket=abc"),
                RelayCaller::Page
            ),
            "ws://127.0.0.1:9000/ws?mode=voice&tools=chat"
        );
    }

    #[test]
    fn a_server_never_gets_tools_however_they_are_spelled() {
        for query in [
            "mode=voice&tools=chat",
            "mode=voice&t%6Fols=chat",
            "mode=voice&tools",
            "mode=voice&Tools=chat",
            "mode=voice&tools%5B%5D=chat",
            "mode=voice&tools.0=chat",
            "mode=voice&tools+=chat",
            "mode=voice&other=1",
        ] {
            assert_eq!(
                upstream_for("ws://127.0.0.1:9000/ws", Some(query), RelayCaller::Server),
                "ws://127.0.0.1:9000/ws?mode=voice",
                "{query}"
            );
        }
        assert_eq!(
            upstream_for(
                "ws://127.0.0.1:9000/ws?mode=voice&tools=chat&TOOLS[]=x&lang=en",
                None,
                RelayCaller::Server
            ),
            "ws://127.0.0.1:9000/ws?mode=voice&lang=en",
            "nor from the configured query"
        );
        assert_eq!(
            upstream_for(
                "ws://127.0.0.1:9000/ws?mode=default&lang=en",
                Some("mode=voice&lang=xx"),
                RelayCaller::Server
            ),
            "ws://127.0.0.1:9000/ws?lang=en&mode=voice",
            "only its mode replaces the configured one"
        );
    }

    #[test]
    fn with_no_query_of_its_own_the_caller_gets_the_configured_one() {
        assert_eq!(
            upstream_for(
                "ws://127.0.0.1:9000/ws?mode=voice",
                Some("ticket=abc"),
                RelayCaller::Page
            ),
            "ws://127.0.0.1:9000/ws?mode=voice"
        );
        assert_eq!(
            upstream_for("ws://127.0.0.1:9000/ws", None, RelayCaller::Page),
            "ws://127.0.0.1:9000/ws"
        );
    }

    #[test]
    fn the_page_url_carries_the_configured_query_and_the_ticket() {
        assert_eq!(
            page_url(Some("/ws?mode=voice&tools=chat"), "T0k_-"),
            "/api/v1/voice-socket?mode=voice&tools=chat&ticket=T0k_-"
        );
        assert_eq!(page_url(None, "t"), "/api/v1/voice-socket?ticket=t");
        assert_eq!(
            page_url(Some("wss://example.test/ws?ticket=stale&mode=x"), "t"),
            "/api/v1/voice-socket?mode=x&ticket=t",
            "a ticket in the configured URL is never carried along"
        );
    }

    #[test]
    fn the_ticket_is_read_decoded() {
        assert_eq!(ticket_in(Some("a=1&ticket=x%2Dy")).as_deref(), Some("x-y"));
        assert_eq!(ticket_in(Some("a=1")), None);
        assert_eq!(ticket_in(None), None);
    }

    #[test]
    fn only_a_well_formed_upgrade_is_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("keep-alive, Upgrade"),
        );
        headers.insert(
            header::SEC_WEBSOCKET_VERSION,
            HeaderValue::from_static("13"),
        );
        headers.insert(
            header::SEC_WEBSOCKET_KEY,
            HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="),
        );
        // RFC 6455's own example.
        assert_eq!(
            accept_key(&headers).unwrap(),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        headers.insert(header::SEC_WEBSOCKET_VERSION, HeaderValue::from_static("8"));
        assert!(accept_key(&headers).is_none());
        headers.insert(
            header::SEC_WEBSOCKET_VERSION,
            HeaderValue::from_static("13"),
        );
        headers.remove(header::UPGRADE);
        assert!(accept_key(&headers).is_none());
    }
}
