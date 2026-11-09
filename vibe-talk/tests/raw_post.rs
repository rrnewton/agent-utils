//! `#1136 raw-message-bridge`: a post asked raw is filed in Slack as the words as typed plus
//! one `[raw]` marker line, with `"raw": true` in the same operation-key metadata payload, so
//! the chat bridge can type it into the bridged agent verbatim. An ordinary post is byte for
//! byte what it was, and a raw text over the cap is refused before anything is sent.
//!
//! Against the loopback Slack fake, as `send_log.rs` runs.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, WRITE_TOKEN};

const TEAM: &str = "C0000MAIN1";

async fn call(state: &AppState, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
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

async fn slack(fake: &Fake) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"

[[channels]]
id = "{TEAM}"
label = "team"
writable = true
"#,
        read = testing::READ_TOKEN,
        base = fake.base,
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

fn fixture() -> FakeState {
    let mut state = FakeState::default();
    state.history.insert(
        TEAM.to_owned(),
        vec![profiled(
            &ts(10, 0),
            "UADA00001",
            "Ada",
            "is the build green?",
        )],
    );
    state
}

fn reply() -> String {
    format!("/api/v1/channels/{TEAM}/reply")
}

#[tokio::test]
async fn a_raw_post_carries_the_marker_in_words_and_metadata_and_a_plain_one_is_unchanged() {
    let fake = Fake::start(fixture(), 50).await;
    let state = slack(&fake).await;
    let parent = vibe_talk::slack::message_id_from_ts(&ts(10, 0))
        .expect("an id")
        .0;

    // Raw, keyed, answering a message: every carrier at once.
    let (status, answered) = call(
        &state,
        &reply(),
        json!({
            "text": "/help",
            "reply_to": parent,
            "raw": true,
            "idempotency_key": "raw-1136-a",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let requests = fake.requests("chat.postMessage");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].get("text").map(String::as_str),
        Some("/help\n[raw]"),
        "the words as typed plus exactly one marker line"
    );
    let metadata: Value =
        serde_json::from_str(requests[0].get("metadata").expect("metadata sent")).expect("JSON");
    assert_eq!(metadata["event_type"], "vibe_talk_op_key", "{metadata}");
    assert_eq!(metadata["event_payload"]["raw"], true, "{metadata}");
    assert!(
        metadata["event_payload"]["op_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("raw-1136-a")),
        "{metadata}"
    );
    assert_eq!(metadata["event_payload"]["reply_to"], parent, "{metadata}");
    let fields = metadata.as_object().expect("metadata is an object");
    assert_eq!(fields.len(), 2, "{metadata}");
    assert!(metadata.to_string().len() < 8 * 1024, "{metadata}");

    // The same words unmarked (the flag absent, as every older caller sends them): no marker
    // line, and an unkeyed ordinary post still files no metadata at all.
    let (status, answered) = call(&state, &reply(), json!({ "text": "/help" })).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let requests = fake.requests("chat.postMessage");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].get("text").map(String::as_str),
        Some("/help"),
        "an ordinary post's words are unchanged"
    );
    assert!(
        !requests[1].contains_key("metadata"),
        "an ordinary unkeyed post files no metadata: {:?}",
        requests[1]
    );

    // Explicitly false is the ordinary post too.
    let (status, answered) = call(
        &state,
        &reply(),
        json!({ "text": "plain words", "raw": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let requests = fake.requests("chat.postMessage");
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[2].get("text").map(String::as_str),
        Some("plain words")
    );
    assert!(!requests[2].contains_key("metadata"));
}

#[tokio::test]
async fn a_raw_post_over_the_cap_is_refused_and_nothing_is_sent() {
    let fake = Fake::start(fixture(), 50).await;
    let state = slack(&fake).await;
    let over = "x".repeat(vibe_talk::ops::RAW_POST_MAX_CHARS + 1);
    let (status, answered) = call(&state, &reply(), json!({ "text": over, "raw": true })).await;
    assert_ne!(status, StatusCode::OK, "{answered}");
    assert!(
        answered.to_string().contains("4000"),
        "the refusal names the cap: {answered}"
    );
    assert_eq!(
        fake.calls("chat.postMessage"),
        0,
        "an over-cap raw post sent nothing"
    );

    // Just under the cap it posts whole, as one message holding the words and the marker
    // line. (This bridge's own per-message byte limit is also 4,000, marker line included,
    // so the last few characters under the character cap are refused for size instead: a
    // raw post is never cut in two.)
    let under = "y".repeat(vibe_talk::ops::RAW_POST_MAX_CHARS - "\n[raw]".len());
    let (status, answered) = call(&state, &reply(), json!({ "text": under, "raw": true })).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let requests = fake.requests("chat.postMessage");
    assert_eq!(requests.len(), 1);
    let text = requests[0].get("text").expect("text sent");
    assert_eq!(text.len(), vibe_talk::ops::RAW_POST_MAX_CHARS);
    assert!(text.ends_with("\n[raw]"));
}

#[tokio::test]
async fn a_raw_posts_key_answers_its_repeat_from_memory() {
    // A keyed raw post retried under its key is answered with the post it made, marker line
    // and all, and nothing is posted twice: the key names the words in the channel, as for
    // every post (a distinct outbox entry mints a distinct key).
    let fake = Fake::start(fixture(), 50).await;
    let state = slack(&fake).await;
    let (status, answered) = call(
        &state,
        &reply(),
        json!({ "text": "/help", "raw": true, "idempotency_key": "raw-1136-key" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let (again_status, again) = call(
        &state,
        &reply(),
        json!({ "text": "/help", "raw": true, "idempotency_key": "raw-1136-key" }),
    )
    .await;
    assert_eq!(again_status, StatusCode::OK, "{again}");
    assert_eq!(again["posted"]["id"], answered["posted"]["id"], "{again}");
    assert_eq!(
        fake.calls("chat.postMessage"),
        1,
        "the repeat posted nothing"
    );
}
