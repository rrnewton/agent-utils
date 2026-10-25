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

// --- `#512 voice-recap-private-stack`: the recap in the profile the bridge fetches --------------
//
// The deployment-managed bridge lists no `replay`, but fetches `GET /api/v1/voice-agent` with the
// write token as each session opens and uses `prompt.text` verbatim. These run this server on a real
// loopback port and a fake bridge that does exactly that, so the recap is followed from the page's
// `POST /api/v1/voice-recap` to the prompt the bridge would hand its model.

/// A server listening on loopback, on the websocket backend, with replay on, the operator's word
/// that the bridge fetches the profile (unless `fetches` is false), and a fake bridge that fetches
/// it with `bridge_token` before every `session_started`. `extra` is appended to `[replay]`.
async fn served_with_fetching_bridge(
    takes_replay: bool,
    fetches: bool,
    bridge_token: &str,
    extra: &str,
) -> (vibe_talk::state::AppState, axum::Router, FakeVoiceBridge) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port for this server");
    let profile_url = format!(
        "http://{}/api/v1/voice-agent",
        listener.local_addr().expect("address")
    );
    let bridge = FakeVoiceBridge::fetching_profile(takes_replay, &profile_url, bridge_token).await;
    let state = deployment(
        bridge.url(),
        true,
        &format!("bridge_fetches_profile = {fetches}\n{extra}"),
    );
    let app = router(state.clone());
    let serving = app.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, serving).await;
    });
    (state, app, bridge)
}

async fn record(app: &axum::Router, conversation: &str, turns: &[(&str, &str)]) {
    for (speaker, said) in turns {
        let (status, body) = call(
            app,
            "POST",
            &format!("/api/v1/conversations/{conversation}/turns"),
            Some(json!({"speaker": speaker, "text": said})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

/// Open a session as the page does and quit it once it has started: the bridge has fetched its
/// profile by then.
async fn session_as_the_page(bridge: &FakeVoiceBridge) {
    let (mut socket, started) = open_session(bridge).await;
    assert!(
        matches!(started, VibeTalkV1ServerFrame::SessionStarted { .. }),
        "{started:?}"
    );
    socket
        .send(text(&VibeTalkV1ClientFrame::Quit))
        .await
        .expect("quits");
}

fn prompt_text(prompt: &Value) -> &str {
    prompt["text"].as_str().expect("prompt.text")
}

#[tokio::test]
async fn a_recap_armed_by_the_page_rides_once_in_the_profile_the_bridge_fetches_and_never_in_the_hellos(
) {
    let (state, app, bridge) = served_with_fetching_bridge(
        false,
        true,
        WRITE_TOKEN,
        "max_turns = 2\n[read_aloud]\nbackend = \"conversation\"\n",
    )
    .await;
    record(
        &app,
        "conv_earlier",
        &[
            ("you", "what happened to the arm64 job overnight"),
            ("agent", "the mac runner stalled mid deploy"),
            ("you", "and is it back now"),
        ],
    )
    .await;

    // The hello first, as at startup: it answers without `replay`, and client-config offers Resuming
    // through the profile, with no reason it is unavailable.
    assert_eq!(state.voice_bridge.ask().await, Hello::Answered(Vec::new()));
    let (status, config) = call(&app, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["replay_enabled"], json!(true), "{config}");
    assert_eq!(config["replay_via_profile"], json!(true), "{config}");
    assert!(config.get("replay_unavailable").is_none(), "{config}");
    let shipped = prompt_text(&bridge.prompts()[0]).to_owned();

    // The page arms the recap: the replay route's own record and budget, without its words.
    let (status, armed) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_earlier"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{armed}");
    assert_eq!(armed["armed"], json!(true), "{armed}");
    assert_eq!(
        (armed["included"].as_u64(), armed["dropped"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(
        armed.get("text").is_none(),
        "the page was handed the record's words: {armed}"
    );
    let id = armed["id"].as_u64().expect("an id");
    let (_, replay) = call(
        &app,
        "GET",
        "/api/v1/conversations/conv_earlier/replay",
        None,
    )
    .await;
    let record_text = replay["text"].as_str().expect("text").to_owned();
    assert_eq!(armed["words"], replay["words"]);

    // The hello opens a session too, and the bridge fetches the profile for it: withheld.
    state.voice_bridge.ask().await;
    let hello_prompt = bridge.prompts().last().cloned().expect("the hello's fetch");
    assert_eq!(
        prompt_text(&hello_prompt),
        shipped,
        "the hello's session was handed the recap"
    );
    let (_, after_hello) = call(&app, "GET", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(after_hello["state"], json!("armed"), "{after_hello}");

    // So does read-aloud on this backend: its session is the server's own too, and withheld.
    state
        .speech
        .warm_up()
        .await
        .expect("a read-aloud session starts");
    assert_eq!(
        bridge.prompts().len(),
        3,
        "read-aloud's session fetched no profile"
    );
    assert_eq!(
        prompt_text(&bridge.prompts()[2]),
        shipped,
        "read-aloud's session was handed the recap"
    );
    let (_, after_read) = call(&app, "GET", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(after_read["state"], json!("armed"), "{after_read}");

    // Each of those was owed ONE fetch, now spent: the page's session, opened at once, is served.
    // Its profile carries the record, fenced, before the capability section, and the fingerprint
    // is of the text served.
    session_as_the_page(&bridge).await;
    let served = bridge.prompts().last().cloned().expect("the call's fetch");
    let served_text = prompt_text(&served);
    assert!(
        served_text.contains(&record_text),
        "the record was not in the prompt: {served_text}"
    );
    assert!(served_text.contains("and is it back now") && !served_text.contains("arm64 job"));
    let section = vibe_talk::voice_agent::SEND_SECTION;
    let direct = vibe_talk::voice_agent::SEND_DIRECT_SECTION;
    assert!(
        served_text.ends_with(section) || served_text.ends_with(direct),
        "the capability section is not the last thing the agent reads"
    );
    assert!(served_text.find(&record_text) < served_text.rfind("You can also send"));
    assert_eq!(
        served["fingerprint"],
        json!(vibe_talk::voice_agent::fingerprint(served_text))
    );
    assert_eq!(served["recap"]["id"], json!(id));
    assert_eq!(served["recap"]["lines"], json!(2));
    assert_eq!(served["recap"]["dropped"], json!(1));

    // The page's answer, and its last word, say it was served.
    let (_, now) = call(&app, "GET", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(now["state"], json!("served"), "{now}");
    let (_, settled) = call(&app, "DELETE", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(settled["state"], json!("served"), "{settled}");

    // Once: the next session, a reconnect say, is handed the shipped prompt again.
    session_as_the_page(&bridge).await;
    let next = bridge.prompts().last().cloned().expect("the next fetch");
    assert_eq!(prompt_text(&next), shipped, "a recap was served twice");
    assert!(next.get("recap").is_none());
    assert!(
        bridge.replays().is_empty(),
        "a replay frame went to a bridge that lists none"
    );
}

#[tokio::test]
async fn a_recap_is_never_served_to_a_read_token_nor_after_it_was_withdrawn_or_forgotten() {
    // A bridge holding the read token: it may not read a transcript, so it is never handed one.
    let (_state, app, bridge) =
        served_with_fetching_bridge(false, true, vibe_talk::testing::READ_TOKEN, "").await;
    record(&app, "conv_a", &[("you", "the deploy is green")]).await;
    let (_, armed) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_a"})),
    )
    .await;
    let id = armed["id"].as_u64().expect("an id");
    session_as_the_page(&bridge).await;
    assert!(!prompt_text(&bridge.prompts()[0]).contains("the deploy is green"));
    let (_, settled) = call(&app, "DELETE", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(settled["state"], json!("withdrawn"), "{settled}");

    // Withdrawn by the page before any fetch: the next session starts fresh.
    let (_state, app, bridge) = served_with_fetching_bridge(false, true, WRITE_TOKEN, "").await;
    record(&app, "conv_b", &[("you", "the deploy is green")]).await;
    let (_, armed) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_b"})),
    )
    .await;
    let id = armed["id"].as_u64().expect("an id");
    let (_, settled) = call(&app, "DELETE", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(settled["state"], json!("withdrawn"));
    session_as_the_page(&bridge).await;
    assert!(!prompt_text(&bridge.prompts()[0]).contains("the deploy is green"));

    // Forgetting the conversations drops a recap still waiting.
    let (_, armed) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_b"})),
    )
    .await;
    let id = armed["id"].as_u64().expect("an id");
    let (status, _) = call(&app, "DELETE", "/api/v1/conversations", None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, gone) = call(&app, "GET", &format!("/api/v1/voice-recap/{id}"), None).await;
    assert_eq!(gone["state"], json!("unknown"), "{gone}");
    session_as_the_page(&bridge).await;
    assert!(!prompt_text(&bridge.prompts()[1]).contains("the deploy is green"));
}

#[tokio::test]
async fn nothing_is_armed_from_an_empty_record_nor_where_the_profile_is_not_the_way() {
    let (_state, app, _bridge) = served_with_fetching_bridge(false, true, WRITE_TOKEN, "").await;
    // Only the page's own note: nothing said aloud to recap.
    record(&app, "conv_notes", &[("note", "the call ended")]).await;
    let (status, empty) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_notes"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["armed"], json!(false));
    assert!(empty.get("id").is_none(), "{empty}");

    // The operator has not said the bridge fetches the profile: Resuming stays unavailable, and
    // arming is refused rather than quietly kept for a fetch that never comes.
    let (state, app, _bridge) = served_with_fetching_bridge(false, false, WRITE_TOKEN, "").await;
    state.voice_bridge.ask().await;
    assert_eq!(
        resuming(&app).await,
        (false, Some("bridge_declined".to_owned()))
    );
    let (status, refused) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_a"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], json!("recap_unavailable"));

    // A bridge that takes the frame is sent the frame: the profile is not the way there.
    let (state, app, _bridge) = served_with_fetching_bridge(true, true, WRITE_TOKEN, "").await;
    state.voice_bridge.ask().await;
    let (_, config) = call(&app, "GET", "/api/v1/client-config", None).await;
    assert_eq!(config["replay_enabled"], json!(true));
    assert!(config.get("replay_via_profile").is_none(), "{config}");
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/voice-recap",
        Some(json!({"conversation_id": "conv_a"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}
