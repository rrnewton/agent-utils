//! `#511 voice-explicit-send-only`, end to end through the real router.
//!
//! The voice agent sends to the chat or the coding agent only when the user explicitly asks it to,
//! whether or not "Ask me before the voice agent sends" is on. These drive the HTTP stack the voice
//! page and a bridge do — the page reporting what it heard, the bridge calling `post_reply` — and
//! the in-memory chat service records what actually went out, so "nothing was posted" is an
//! observation.
//!
//! The first test is the incident: the speech runtime heard the owner ask the voice agent to find
//! something, and the model posted a garbled fragment to a coding agent's channel without asking.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::http::router;
use vibe_talk::testing::{WRITE_CHANNEL, WRITE_TOKEN};

struct Harness {
    router: axum::Router,
    discord: Arc<FakeDiscord>,
}

/// A server with the check on, as shipped. `direct` turns confirmation off, as the owner had it.
async fn harness(direct: bool) -> Harness {
    let text = format!(
        "{}\n[conversation]\nexplicit_send_only = true\n",
        vibe_talk::testing::config_toml()
    );
    let (state, discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let harness = Harness {
        router: router(state),
        discord,
    };
    if direct {
        let (status, body) = send(
            &harness,
            "PUT",
            "/api/v1/voice-settings",
            Some(json!({ "ask_before_send": false })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    harness
}

async fn send(
    harness: &Harness,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("router responds");
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

/// The voice page heard the user say `text` in the turn it calls `key`.
async fn heard(harness: &Harness, key: &str, text: &str) {
    let (status, body) = send(
        harness,
        "POST",
        "/api/v1/voice-heard",
        Some(json!({ "key": key, "text": text })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// The voice page filed a `you` turn in the transcript.
async fn filed(harness: &Harness, text: &str) {
    let (status, body) = send(
        harness,
        "POST",
        "/api/v1/conversations/conv_ask/turns",
        Some(json!({ "speaker": "you", "text": text })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn post_reply(harness: &Harness, arguments: Value) -> String {
    let (status, body) = send(
        harness,
        "POST",
        "/mcp",
        Some(json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "post_reply", "arguments": arguments },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

const QUESTION: &str = "Find the agent that flagged a conflict between Claude dot MD and the \
                        newer agents dot MD that referenced a soft green landing protocol.";
const FRAGMENT: &str = "afflicting guidance and remind on";

#[tokio::test(start_paused = true)]
async fn the_incident_posts_nothing_with_or_without_confirmation() {
    for direct in [true, false] {
        let harness = harness(direct).await;
        filed(&harness, QUESTION).await;
        heard(&harness, "c:9:you", QUESTION).await;
        for request in [None, Some(FRAGMENT), Some(QUESTION)] {
            let mut arguments = json!({ "channel_id": WRITE_CHANNEL, "text": FRAGMENT });
            if let Some(request) = request {
                arguments["user_request"] = json!(request);
            }
            let said = post_reply(&harness, arguments).await;
            assert!(
                said.starts_with("NOT SENT."),
                "direct={direct} {request:?}: {said}"
            );
        }
        assert!(harness.discord.posted().is_empty(), "direct={direct}");
    }
}

#[tokio::test(start_paused = true)]
async fn an_explicit_ask_sends_once() {
    let harness = harness(true).await;
    heard(
        &harness,
        "c:3:you",
        "Tell the coding agent to rebase onto main.",
    )
    .await;
    let said = post_reply(
        &harness,
        json!({
            "channel_id": WRITE_CHANNEL,
            "text": "Please rebase onto main.",
            "user_request": "tell the coding agent to rebase onto main",
        }),
    )
    .await;
    assert!(said.starts_with("SENT"), "{said}");
    assert_eq!(harness.discord.posted().len(), 1);

    // The same call again — a transport's retry — is the repeat it is, and posts nothing.
    let said = post_reply(
        &harness,
        json!({
            "channel_id": WRITE_CHANNEL,
            "text": "Please rebase onto main.",
            "user_request": "tell the coding agent to rebase onto main",
        }),
    )
    .await;
    assert!(said.starts_with("SENT already"), "{said}");
    assert_eq!(harness.discord.posted().len(), 1);

    // Other words on the same ask: the ask is spent.
    let said = post_reply(
        &harness,
        json!({
            "channel_id": WRITE_CHANNEL,
            "text": "And run the tests.",
            "user_request": "tell the coding agent to rebase onto main",
        }),
    )
    .await;
    assert!(said.starts_with("NOT SENT."), "{said}");
    assert!(said.contains("already used"), "{said}");
    assert_eq!(harness.discord.posted().len(), 1);

    // The same words said again are a new ask, and a new license.
    heard(
        &harness,
        "c:6:you",
        "Tell the coding agent to rebase onto main.",
    )
    .await;
    let said = post_reply(
        &harness,
        json!({
            "channel_id": WRITE_CHANNEL,
            "text": "Rebase onto main again, please.",
            "user_request": "tell the coding agent to rebase onto main",
        }),
    )
    .await;
    assert!(said.starts_with("SENT"), "{said}");
    assert_eq!(harness.discord.posted().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_turn_only_filed_in_the_transcript_licenses_nothing() {
    // The page files turns it saved before it was closed, minutes or hours later; an ask in one
    // must not come back to life.
    let harness = harness(true).await;
    filed(&harness, "Tell the coding agent to rebase onto main.").await;
    let said = post_reply(
        &harness,
        json!({
            "channel_id": WRITE_CHANNEL,
            "text": "Please rebase onto main.",
            "user_request": "tell the coding agent to rebase onto main",
        }),
    )
    .await;
    assert!(said.starts_with("NOT SENT."), "{said}");
    assert!(harness.discord.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn proposing_needs_an_ask_too_and_spends_none() {
    let harness = harness(false).await;
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "rebase", "user_request": "rebase" }),
    )
    .await;
    assert!(said.starts_with("NOT SENT."), "{said}");
    let (_, pending) = send(&harness, "GET", "/api/v1/post-proposals", None).await;
    assert!(
        pending["proposal"].is_null(),
        "nothing was proposed: {pending}"
    );

    heard(
        &harness,
        "c:4:you",
        "Send this to the lead team: rebase onto main.",
    )
    .await;
    for text in ["Rebase onto main.", "Please rebase onto main."] {
        let said = post_reply(
            &harness,
            json!({
                "channel_id": WRITE_CHANNEL,
                "text": text,
                "user_request": "send this to the lead team: rebase onto main",
            }),
        )
        .await;
        assert!(
            !said.starts_with("NOT SENT. Nothing was posted or proposed"),
            "{said}"
        );
        let (_, pending) = send(&harness, "GET", "/api/v1/post-proposals", None).await;
        assert!(
            pending["proposal"]["text"]
                .as_str()
                .is_some_and(|t| t.ends_with(text)),
            "proposed, again on the same ask: {pending}"
        );
    }
    assert!(
        harness.discord.posted().is_empty(),
        "a proposal posts nothing"
    );
}

#[tokio::test(start_paused = true)]
async fn the_rest_route_refuses_the_same_way() {
    let harness = harness(true).await;
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/post-proposals",
        Some(json!({ "channel_id": WRITE_CHANNEL, "text": FRAGMENT, "user_request": FRAGMENT })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], json!("send_not_requested"), "{body}");
    assert!(harness.discord.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_tool_requires_the_ask_and_says_why() {
    let harness = harness(true).await;
    let (_, body) = send(
        &harness,
        "POST",
        "/mcp",
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })),
    )
    .await;
    let tool = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "post_reply")
        .expect("post_reply is offered")
        .clone();
    let required = tool["inputSchema"]["required"]
        .as_array()
        .expect("required");
    assert!(required.contains(&json!("user_request")), "{tool}");
    let description = tool["description"].as_str().expect("description");
    assert!(
        description.contains("ONLY when the user explicitly asks"),
        "{description}"
    );
    assert!(
        description.contains("Never call it to pass on what the user said"),
        "{description}"
    );
}

#[tokio::test]
async fn a_heard_report_needs_a_key_and_is_bounded() {
    let harness = harness(true).await;
    let (status, _) = send(
        &harness,
        "POST",
        "/api/v1/voice-heard",
        Some(json!({ "key": " ", "text": "hello" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &harness,
        "POST",
        "/api/v1/voice-heard",
        Some(json!({ "key": "k", "text": "hello", "extra": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(
        &harness,
        "POST",
        "/api/v1/voice-heard",
        Some(json!({ "key": "k", "text": "x".repeat(vibe_talk::send_intent::MAX_BODY_BYTES) })),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
