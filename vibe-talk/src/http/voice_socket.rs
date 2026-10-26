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
//!   tools: a `tools` parameter it sends is dropped before the bridge sees it.
//!
//! Nothing else opens it, not even the read or write token: the page always comes with a ticket,
//! and a server has no business holding a browser token to read aloud. Every refusal is the same
//! 401 the rest of `/api/` gives, so the route says nothing about tickets to a caller without one.
//!
//! The rest of the query (`mode`, and `tools` for a page) is forwarded to the bridge in place of
//! `relay_upstream`'s own; with none, `relay_upstream`'s query applies. The bridge is dialed BEFORE
//! the upgrade is accepted, so a bridge that is down is a 502 the caller can read, not a socket
//! that opens and closes at once.

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

/// The bridge URL for this caller: `upstream` with its query replaced by the caller's, less the
/// ticket and, for a server, less `tools`. Parameters are decoded before they are judged and
/// re-encoded after, so an encoded name cannot slip past the filter.
#[must_use]
pub fn upstream_for(upstream: &str, query: Option<&str>, caller: RelayCaller) -> String {
    let (base, own_query) = match upstream.split_once('?') {
        Some((base, own)) => (base, Some(own)),
        None => (upstream, None),
    };
    let kept: Vec<(String, String)> = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .filter(|(name, _)| name != TICKET_PARAMETER)
        .filter(|(name, _)| caller == RelayCaller::Page || name != TOOLS_PARAMETER)
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let forwarded = if kept.is_empty() {
        // Nothing of the caller's own: the configured query, filtered the same way.
        url::form_urlencoded::parse(own_query.unwrap_or("").as_bytes())
            .filter(|(name, _)| caller == RelayCaller::Page || name != TOOLS_PARAMETER)
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect()
    } else {
        kept
    };
    if forwarded.is_empty() {
        return base.to_owned();
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(forwarded)
        .finish();
    format!("{base}?{query}")
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

/// Copy frames both ways until either side closes or fails, then close the other.
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
    let upward = async {
        while let Some(Ok(message)) = client_rx.next().await {
            if forward(&mut bridge_tx, message).await.is_err() {
                break;
            }
        }
        let _ = bridge_tx.close().await;
    };
    let downward = async {
        while let Some(Ok(message)) = bridge_rx.next().await {
            if forward(&mut client_tx, message).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };
    // Either direction ending ends the relay: a page that hung up, or a bridge that closed its side
    // after an error frame.
    tokio::select! {
        () = upward => {},
        () = downward => {},
    }
}

/// Forward one message. Pings and pongs are each side's own business (tungstenite answers a ping
/// itself), and a close ends this direction.
async fn forward<S>(sink: &mut S, message: Message) -> Result<(), ()>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match message {
        Message::Text(_) | Message::Binary(_) => sink.send(message).await.map_err(|_| ()),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(()),
        Message::Close(_) => Err(()),
    }
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
        ] {
            assert_eq!(
                upstream_for("ws://127.0.0.1:9000/ws", Some(query), RelayCaller::Server),
                "ws://127.0.0.1:9000/ws?mode=voice",
                "{query}"
            );
        }
        assert_eq!(
            upstream_for(
                "ws://127.0.0.1:9000/ws?mode=voice&tools=chat",
                None,
                RelayCaller::Server
            ),
            "ws://127.0.0.1:9000/ws?mode=voice",
            "nor from the configured query"
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
