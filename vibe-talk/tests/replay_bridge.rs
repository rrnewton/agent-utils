//! `#331 resume-on-websocket`: resuming on the `vibe-talk-v1` backend, against the loopback bridge
//! in [`vibe_talk::testing::FakeVoiceBridge`].
//!
//! Two halves, because the protocol has two. The HELLO: a bridge lists `replay` in its
//! `session_started` when it takes the `replay` frame, this server asks it before any call, and
//! client-config offers Resuming only after a yes — a bridge that answered without the word is
//! `bridge_declined`, one that has not answered is `bridge_unconfirmed`, and the ask itself says
//! nothing in the session it opens. The FRAME: `replay` carries the reconstruction this server
//! renders for every backend, within its budget, and the bridge answers it with one `replay_ack`,
//! accepted by a bridge that takes one and refused, with a reason, by one that does not.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt as _, StreamExt as _};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt as _;
use vibe_talk::contract::{VibeTalkV1ClientFrame, VibeTalkV1ServerFrame};
use vibe_talk::http::router;
use vibe_talk::testing::{FakeVoiceBridge, WRITE_TOKEN};
use vibe_talk::voice_bridge::{Hello, ReplaySupport};

/// A server on the websocket backend at `conversation_url`, with replay on or off, and `extra`
/// appended.
fn deployment(conversation_url: &str, replay: bool, extra: &str) -> vibe_talk::state::AppState {
    let toml = format!(
        "{}\n[conversation]\nbackend = \"websocket\"\nwebsocket_url = \"{conversation_url}\"\n{}{extra}",
        vibe_talk::testing::config_toml(),
        if replay { "[replay]\nenabled = true\n" } else { "" },
    );
    vibe_talk::testing::state_with_store_from_toml(&toml).0
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let response = app.clone().oneshot(request).await.expect("router responds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// What client-config says about resuming: `(replay_enabled, replay_unavailable)`.
async fn resuming(app: &axum::Router) -> (bool, Option<String>) {
    let (status, config) = call(app, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    (
        config["replay_enabled"].as_bool().expect("replay_enabled"),
        config
            .get("replay_unavailable")
            .and_then(Value::as_str)
            .map(str::to_owned),
    )
}

/// Wait until every session the bridge has seen has been sent at least one frame, so a check of
/// what the sessions were sent is not a check of how far their tasks have got.
async fn settled(bridge: &FakeVoiceBridge) -> Vec<Vec<Value>> {
    for _ in 0..200 {
        let sessions = bridge.sessions();
        if !sessions.is_empty() && sessions.iter().all(|frames| !frames.is_empty()) {
            return sessions;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "the bridge's sessions never settled: {:?}",
        bridge.sessions()
    );
}

/// A client on the bridge, as the page is one: open, and read the session's first frame.
async fn open_session(
    bridge: &FakeVoiceBridge,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    VibeTalkV1ServerFrame,
) {
    let (mut socket, _) = tokio_tungstenite::connect_async(bridge.url())
        .await
        .expect("the bridge accepts a call");
    let first = next_frame(&mut socket).await;
    (socket, first)
}

async fn next_frame<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> VibeTalkV1ServerFrame
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match socket.next().await.expect("a frame").expect("readable") {
            Message::Text(raw) => {
                return serde_json::from_str(raw.as_ref()).expect("a contract server frame")
            }
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected frame from the bridge: {other:?}"),
        }
    }
}

fn text(frame: &VibeTalkV1ClientFrame) -> Message {
    Message::Text(serde_json::to_string(frame).expect("encodes").into())
}

#[tokio::test]
async fn a_bridge_that_lists_replay_turns_resuming_on_and_the_hello_says_nothing_but_quit() {
    let bridge = FakeVoiceBridge::start(true).await;
    let state = deployment(bridge.url(), true, "");
    let app = router(state.clone());

    // Before the bridge has answered: off, and saying why — not "declined", which it has not.
    assert_eq!(
        resuming(&app).await,
        (false, Some("bridge_unconfirmed".to_owned())),
        "Resuming was offered before the bridge said it takes a recap"
    );

    assert_eq!(
        state.voice_bridge.ask().await,
        Hello::Answered(vec!["replay".to_owned()])
    );
    assert_eq!(state.voice_bridge.replay_support(), ReplaySupport::Accepted);
    assert_eq!(
        resuming(&app).await,
        (true, None),
        "a bridge that listed replay is offered Resuming"
    );

    // Every session the server opened to ask — the one above, and the one client-config started in
    // the background — was told `quit` and nothing else: no audio, no prompt, no recap.
    let sessions = settled(&bridge).await;
    assert!(!sessions.is_empty());
    for frames in &sessions {
        assert_eq!(
            frames,
            &vec![json!({"type": "quit"})],
            "the hello said something in its session"
        );
    }
    assert!(bridge.replays().is_empty(), "the hello sent a recap");
}

#[tokio::test]
async fn a_bridge_that_does_not_list_replay_keeps_resuming_off_and_says_it_declined() {
    let bridge = FakeVoiceBridge::start(false).await;
    let state = deployment(bridge.url(), true, "");
    let app = router(state.clone());

    assert_eq!(state.voice_bridge.ask().await, Hello::Answered(Vec::new()));
    assert_eq!(
        resuming(&app).await,
        (false, Some("bridge_declined".to_owned())),
        "a bridge that did not list replay was offered Resuming, or not said to have declined"
    );

    // The control: the same server against a bridge that takes one says yes, so the no above is
    // the bridge's answer and not this deployment's.
    let taking = FakeVoiceBridge::start(true).await;
    let state = deployment(taking.url(), true, "");
    state.voice_bridge.ask().await;
    assert_eq!(resuming(&router(state)).await, (true, None));
}

#[tokio::test]
async fn a_bridge_that_does_not_answer_is_unconfirmed_and_never_declined() {
    // A port nothing listens on.
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds");
        listener.local_addr().expect("address").port()
    };
    let state = deployment(&format!("ws://127.0.0.1:{closed}/voice"), true, "");
    let Hello::Failed(why) = state.voice_bridge.ask().await else {
        panic!("a bridge that is not there answered");
    };
    assert!(why.contains("refused"), "{why}");
    assert_eq!(
        resuming(&router(state)).await,
        (false, Some("bridge_unconfirmed".to_owned()))
    );

    // A path is the page's own host, which this server cannot dial; the failure names the fix.
    let path = deployment("/ws?mode=voice", true, "");
    let Hello::Failed(why) = path.voice_bridge.ask().await else {
        panic!("a server with no address to dial answered");
    };
    assert!(why.contains("read_aloud.websocket_url"), "{why}");
    assert_eq!(
        resuming(&router(path)).await,
        (false, Some("bridge_unconfirmed".to_owned()))
    );
}

#[tokio::test]
async fn the_servers_own_route_to_the_bridge_is_asked_when_the_page_uses_a_path() {
    let bridge = FakeVoiceBridge::start(true).await;
    let state = deployment(
        "/ws?mode=voice",
        true,
        &format!("[read_aloud]\nwebsocket_url = \"{}\"\n", bridge.url()),
    );
    assert_eq!(state.voice_bridge.url(), Some(bridge.url()));
    assert_eq!(
        state.voice_bridge.ask().await,
        Hello::Answered(vec!["replay".to_owned()])
    );
    assert_eq!(resuming(&router(state)).await, (true, None));
}

#[tokio::test]
async fn with_replay_off_the_bridge_is_never_asked_and_nothing_is_said_unavailable() {
    let bridge = FakeVoiceBridge::start(true).await;
    let app = router(deployment(bridge.url(), false, ""));
    for _ in 0..3 {
        assert_eq!(
            resuming(&app).await,
            (false, None),
            "off on purpose is not unavailable"
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        bridge.sessions().is_empty(),
        "a server with replay off opened a session on the bridge: {:?}",
        bridge.sessions()
    );
}

#[tokio::test]
async fn the_replay_frame_carries_the_servers_budgeted_reconstruction_and_is_acknowledged() {
    let bridge = FakeVoiceBridge::start(true).await;
    let app = router(deployment(bridge.url(), true, "max_turns = 2\n"));
    for (speaker, said) in [
        ("you", "what happened to the arm64 job overnight"),
        ("agent", "the mac runner stalled mid deploy"),
        ("you", "and is it back now"),
    ] {
        let (status, body) = call(
            &app,
            "POST",
            "/api/v1/conversations/conv_earlier/turns",
            Some(json!({"speaker": speaker, "text": said})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    // The reconstruction every backend is handed: the hosted one as a `contextual_update`, a bridge
    // in the frame below. Budgeted: the oldest line did not fit in two turns.
    let (status, payload) = call(
        &app,
        "GET",
        "/api/v1/conversations/conv_earlier/replay",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    assert_eq!(
        (payload["included"].as_u64(), payload["dropped"].as_u64()),
        (Some(2), Some(1))
    );
    let record = payload["text"].as_str().expect("text").to_owned();
    assert!(record.starts_with(vibe_talk::replay::PREAMBLE), "{record}");
    assert!(
        record.contains("INCOMPLETE"),
        "a partial record must say so to the agent: {record}"
    );
    assert!(
        record.contains("and is it back now") && !record.contains("arm64 job"),
        "{record}"
    );

    // The page's side of the call: a session that lists `replay`, then the frame first.
    let (mut socket, started) = open_session(&bridge).await;
    let VibeTalkV1ServerFrame::SessionStarted { capabilities, .. } = started else {
        panic!("the bridge's first frame was not session_started: {started:?}");
    };
    assert_eq!(capabilities, vec!["replay".to_owned()]);
    let frame = VibeTalkV1ClientFrame::Replay {
        text: record.clone(),
        lines: u32::try_from(payload["included"].as_u64().expect("included")).expect("small"),
        dropped: u32::try_from(payload["dropped"].as_u64().expect("dropped")).expect("small"),
        words: u32::try_from(payload["words"].as_u64().expect("words")).expect("small"),
    };
    // On the wire it is exactly what the README documents for bridge authors.
    assert_eq!(
        serde_json::to_value(&frame).expect("encodes"),
        json!({"type": "replay", "text": record, "lines": 2, "dropped": 1, "words": payload["words"]}),
    );
    socket.send(text(&frame)).await.expect("sends the replay");
    assert_eq!(
        next_frame(&mut socket).await,
        VibeTalkV1ServerFrame::ReplayAck {
            accepted: true,
            reason: None
        },
        "a bridge that lists replay acknowledges the record"
    );
    socket
        .send(text(&VibeTalkV1ClientFrame::Quit))
        .await
        .expect("quits");

    assert_eq!(
        bridge.replays(),
        vec![record],
        "the bridge was not handed the record unchanged"
    );
}

#[tokio::test]
async fn a_bridge_that_does_not_take_a_replay_lists_nothing_and_refuses_one_with_a_reason() {
    let bridge = FakeVoiceBridge::start(false).await;
    let (mut socket, started) = open_session(&bridge).await;
    let VibeTalkV1ServerFrame::SessionStarted { capabilities, .. } = started else {
        panic!("the bridge's first frame was not session_started: {started:?}");
    };
    assert!(capabilities.is_empty(), "{capabilities:?}");
    // A client must not send one here; this bridge says so if one does.
    socket
        .send(text(&VibeTalkV1ClientFrame::Replay {
            text: "You are resuming an earlier voice conversation.".into(),
            lines: 1,
            dropped: 0,
            words: 3,
        }))
        .await
        .expect("sends");
    let VibeTalkV1ServerFrame::ReplayAck { accepted, reason } = next_frame(&mut socket).await
    else {
        panic!("a replay was not answered with replay_ack");
    };
    assert!(
        !accepted,
        "a bridge that does not take a replay accepted one"
    );
    assert!(reason.is_some_and(|reason| reason.contains("does not take")));
}
