//! `#621 send-double-check`: two-phase answer and outcome delivery.

use crate::slack_fake;

use std::sync::Arc;
use std::time::Duration;

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

async fn slack(fake: &Fake, max_post_chars: usize) -> AppState {
    let text = format!("\n[auth]\nread_token = \"{read}\"\nwrite_token = \"{WRITE_TOKEN}\"\n\n[[providers]]\nkey = \"slack\"\nkind = \"slack\"\napi_base = \"{base}\"\ntoken = \"{TOKEN}\"\nmax_post_chars = {max_post_chars}\n\n[[channels]]\nid = \"{TEAM}\"\nlabel = \"team\"\nwritable = true\n", read = testing::READ_TOKEN, base = fake.base);
    let (mut state, _d, _s) = testing::state_with_store_from_toml(&text);
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

#[tokio::test]
async fn accepted_answers_at_once_then_outcome_delivers_slow_post() {
    let mut fx = fixture();
    fx.post_delay = Some(Duration::from_millis(600));
    let fake = Fake::start(fx, 20).await;
    let state = slack(&fake, 4000).await;
    let body = json!({"text": "slow hello", "idempotency_key": "tick-slow-1"});
    let started = std::time::Instant::now();
    let (status, accepted) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply?ack=accepted"),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["state"], "accepted");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "accept must not wait for upstream: {:?}",
        started.elapsed()
    );
    assert!(accepted["server_ms"].as_u64().is_some());
    let (status, settled) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply/outcome?wait=20"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert!(settled["posted"]["id"].is_string(), "{settled}");
    assert!(
        settled["timing"]["server_ms"].as_u64().is_some(),
        "{settled}"
    );
    assert!(
        settled["timing"]["upstream_ms"].as_u64().unwrap_or(0) >= 500,
        "{settled}"
    );
}

#[tokio::test]
async fn accepted_failed_post_settles_as_unknown_or_error_not_green() {
    let mut fx = fixture();
    fx.fail.insert(
        "chat.postMessage".to_owned(),
        "channel_not_found".to_owned(),
    );
    let fake = Fake::start(fx, 20).await;
    let state = slack(&fake, 4000).await;
    let body = json!({"text": "will fail", "idempotency_key": "tick-fail-1"});
    let (status, _) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply?ack=accepted"),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // Failed with nothing posted is forgotten: outcome is 404, never a 200.
    let (status, _) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply/outcome?wait=20"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn accepted_partial_post_answers_207_with_timing() {
    let mut fx = fixture();
    fx.fail_posts = vec![2];
    let fake = Fake::start(fx, 20).await;
    let state = slack(&fake, 500).await;
    let text = "word ".repeat(240);
    let body = json!({"text": text, "idempotency_key": "tick-part-1"});
    let (status, _) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply?ack=accepted"),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, settled) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply/outcome?wait=20"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{settled}");
    assert_eq!(settled["error"], "partially_posted");
}

#[tokio::test]
async fn blocking_reply_still_answers_200_without_ack() {
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 4000).await;
    let (status, body) = call(
        &state,
        &format!("/api/v1/channels/{TEAM}/reply"),
        json!({"text": "plain", "idempotency_key": "tick-plain-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["posted"]["id"].is_string());
}
