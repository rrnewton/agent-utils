//! `#553 voice-ws-relay-auth`: the voice socket relayed to a bridge only this server reaches.
//!
//! A real server on loopback in front of a scripted bridge, so the whole path is exercised: the
//! page's ticket minted by its voice-session call, the voice-only token a second server presents,
//! the refusals, what query reaches the bridge, and frames copied both ways.

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use vibe_talk::testing::{READ_TOKEN, WRITE_TOKEN};

const VOICE_TOKEN: &str = "test-voice-token-00000000000";

/// A bridge that reports each connection's request path and query, announces the session, and
/// echoes every text frame back.
// The handshake callback's error type is `http::Response<Option<String>>`, fixed by tungstenite's
// `Callback` trait, so clippy's suggestion (boxing it) is not available.
#[allow(clippy::result_large_err)]
async fn bridge() -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let url = format!("ws://{}/ws", listener.local_addr().expect("address"));
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let seen_tx = seen_tx.clone();
            tokio::spawn(async move {
                let mut asked = String::new();
                let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                     response| {
                        asked = request.uri().to_string();
                        Ok(response)
                    },
                )
                .await
                else {
                    return;
                };
                let _ = seen_tx.send(asked);
                let _ = socket
                    .send(Message::Text(r#"{"type":"session_started"}"#.into()))
                    .await;
                while let Some(Ok(message)) = socket.next().await {
                    if let Message::Text(text) = message {
                        if text.as_str() == "close-me" {
                            // As a bridge ends a session after an error: a code and a reason.
                            let _ = socket
                                .close(Some(CloseFrame {
                                    code: CloseCode::Library(4000),
                                    reason: "bridge said goodbye".into(),
                                }))
                                .await;
                            return;
                        }
                        if socket.send(Message::Text(text)).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    (url, seen)
}

/// This server, relaying to `upstream`, served on loopback. Answers its base `http://` address.
async fn server(upstream: &str) -> String {
    let text = vibe_talk::testing::config_toml().replace(
        &format!("write_token = \"{WRITE_TOKEN}\"\n"),
        &format!("write_token = \"{WRITE_TOKEN}\"\nvoice_token = \"{VOICE_TOKEN}\"\n"),
    ) + &format!(
        "\n[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"/ws?mode=voice&tools=chat\"\n\
         relay_upstream = \"{upstream}\"\n"
    );
    let (state, _chat, _voice) = vibe_talk::testing::state_from_toml(&text);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let address = listener.local_addr().expect("address");
    let app = vibe_talk::http::router(state);
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serves") });
    format!("http://{address}")
}

/// The page's voice-session call, as the page makes it: the socket URL it is handed.
async fn page_socket_url(base: &str) -> String {
    let body: Value = reqwest::Client::new()
        .get(format!("{base}/api/v1/voice-session"))
        .bearer_auth(WRITE_TOKEN)
        .send()
        .await
        .expect("asks")
        .json()
        .await
        .expect("json");
    body["websocket_url"]
        .as_str()
        .expect("a socket URL")
        .to_owned()
}

fn ws(base: &str, path: &str) -> String {
    format!("{}{path}", base.replacen("http://", "ws://", 1))
}

async fn open(
    url: &str,
    token: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    let mut request = url.into_client_request().expect("a request");
    if let Some(token) = token {
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
        );
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

fn refused_with(result: Result<impl Sized, tokio_tungstenite::tungstenite::Error>, status: u16) {
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), status);
        }
        Err(other) => panic!("expected HTTP {status}, got {other}"),
        Ok(_) => panic!("expected HTTP {status}, but the socket opened"),
    }
}

async fn next_text(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("a frame in time")
        {
            Some(Ok(Message::Text(text))) => return text.to_string(),
            Some(Ok(_)) => {}
            other => panic!("the socket ended: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_page_opens_the_relay_once_on_its_ticket_and_keeps_its_tools() {
    let (upstream, mut seen) = bridge().await;
    let base = server(&upstream).await;
    let path = page_socket_url(&base).await;
    assert!(
        path.starts_with("/api/v1/voice-socket?mode=voice&tools=chat&ticket="),
        "{path}"
    );
    let mut socket = open(&ws(&base, &path), None)
        .await
        .expect("the ticket opens it");
    assert_eq!(
        next_text(&mut socket).await,
        r#"{"type":"session_started"}"#
    );
    socket
        .send(Message::Text(r#"{"type":"prompt","text":"hi"}"#.into()))
        .await
        .expect("sends");
    assert_eq!(
        next_text(&mut socket).await,
        r#"{"type":"prompt","text":"hi"}"#
    );
    assert_eq!(
        seen.recv().await.as_deref(),
        Some("/ws?mode=voice&tools=chat"),
        "the bridge gets the page's query, never its ticket"
    );
    // The same ticket again opens nothing.
    refused_with(open(&ws(&base, &path), None).await, 401);
}

#[tokio::test]
async fn a_server_opens_it_with_the_voice_token_but_never_gets_tools() {
    let (upstream, mut seen) = bridge().await;
    let base = server(&upstream).await;
    let url = ws(&base, "/api/v1/voice-socket?mode=voice&tools=chat");
    let mut socket = open(&url, Some(VOICE_TOKEN))
        .await
        .expect("the voice token opens it");
    assert_eq!(
        next_text(&mut socket).await,
        r#"{"type":"session_started"}"#
    );
    assert_eq!(seen.recv().await.as_deref(), Some("/ws?mode=voice"));
}

#[tokio::test]
async fn nothing_else_opens_it() {
    let (upstream, _seen) = bridge().await;
    let base = server(&upstream).await;
    let url = ws(&base, "/api/v1/voice-socket?mode=voice");
    for token in [
        None,
        Some(READ_TOKEN),
        Some(WRITE_TOKEN),
        Some("not-a-token-000000000000"),
    ] {
        refused_with(open(&url, token).await, 401);
    }
    // A guessed ticket is refused, even beside the voice token.
    let guessed = ws(
        &base,
        &format!("/api/v1/voice-socket?ticket={}", "a".repeat(43)),
    );
    refused_with(open(&guessed, Some(VOICE_TOKEN)).await, 401);
}

#[tokio::test]
async fn the_voice_token_opens_no_other_route() {
    let (upstream, _seen) = bridge().await;
    let base = server(&upstream).await;
    let client = reqwest::Client::new();
    for path in [
        "/api/v1/channels",
        "/api/v1/voice-session",
        "/api/v1/voice-agent",
    ] {
        let status = client
            .get(format!("{base}{path}"))
            .bearer_auth(VOICE_TOKEN)
            .send()
            .await
            .expect("asks")
            .status();
        assert_eq!(status.as_u16(), 401, "{path}");
    }
}

#[tokio::test]
async fn a_bridge_that_is_down_is_a_502_not_a_socket_that_drops() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let upstream = format!("ws://{}/ws", closed.local_addr().expect("address"));
    drop(closed);
    let base = server(&upstream).await;
    refused_with(
        open(&ws(&base, "/api/v1/voice-socket"), Some(VOICE_TOKEN)).await,
        502,
    );
}

#[tokio::test]
async fn read_aloud_reaches_a_bridge_through_another_servers_relay() {
    let (upstream, mut seen) = bridge().await;
    let base = server(&upstream).await;
    let dir = vibe_talk::testing::TempDir::new("relay-read-aloud");
    let token_file = dir.path().join("voice-token");
    std::fs::write(&token_file, format!("{VOICE_TOKEN}\n")).expect("writes");
    let credentials = vibe_talk::bridge_dial::Credentials {
        token_file: Some(token_file),
        ca_file: None,
    };
    let said =
        vibe_talk::voice_bridge::hello(&ws(&base, "/api/v1/voice-socket?mode=voice"), &credentials)
            .await;
    assert_eq!(
        said,
        Ok(Vec::new()),
        "the hello got a session through the relay"
    );
    assert_eq!(seen.recv().await.as_deref(), Some("/ws?mode=voice"));
}

#[tokio::test]
async fn a_close_passes_through_with_its_code_and_reason() {
    let capture = vibe_talk::testing::LogCapture::start();
    let (upstream, _seen) = bridge().await;
    let base = server(&upstream).await;
    let url = ws(&base, "/api/v1/voice-socket?mode=voice");
    let mut socket = open(&url, Some(VOICE_TOKEN)).await.expect("opens");
    assert_eq!(
        next_text(&mut socket).await,
        r#"{"type":"session_started"}"#
    );
    socket
        .send(Message::Text("close-me".into()))
        .await
        .expect("sends");
    let closed = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("a frame in time")
        {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => {}
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    let frame = closed.expect("the bridge's close frame, not an empty one");
    assert_eq!(frame.code, CloseCode::Library(4000));
    assert_eq!(frame.reason.as_str(), "bridge said goodbye");
    // `#649 voice-agent-tools` (f): the relay's line says who ended the socket and with what code,
    // and how much each side sent, so a run of reconnects can be told apart.
    let logged = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let text = capture.text();
            if text.contains("voice relay: socket closed") {
                break text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the relay logged the close");
    let line = logged
        .lines()
        .find(|line| line.contains("voice relay: socket closed"))
        .expect("the line");
    for field in [
        "ended=\"bridge_close\"",
        "code=4000",
        "from_client=1",
        "from_bridge=1",
    ] {
        assert!(line.contains(field), "{field}: {line}");
    }
}
