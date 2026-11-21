//! `#2060 vibe-talk-realistic-testing` Part B: the read-only test
//! credential is deny-by-default.
//!
//! The state's provider is the answering FakeDiscord, which counts every
//! fetch: a request that fell through to the provider would be answered
//! (200) and counted. So a 403 with an unchanged fetch count proves the
//! refusal happened before any upstream call, rather than merely
//! asserting the status.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::http::router;
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};

const TEST_TOKEN: &str = "test-credential-token-0000000";

fn state() -> (
    vibe_talk::state::AppState,
    std::sync::Arc<vibe_talk::discord::fake::FakeDiscord>,
) {
    let toml = testing::config_toml().replace(
        &format!("write_token = \"{WRITE_TOKEN}\""),
        &format!("write_token = \"{WRITE_TOKEN}\"\ntest_token = \"{TEST_TOKEN}\""),
    );
    let (state, discord, _elevenlabs) = testing::state_from_toml(&toml);
    (state, discord)
}

async fn call(
    state: &vibe_talk::state::AppState,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    builder = builder.header("authorization", format!("Bearer {token}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = router(state.clone())
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

/// Every read route the allowlist does NOT name, with a body where the
/// route takes one. A new read route belongs here until the allowlist
/// names it, which is the deny-by-default property in test form.
fn non_allowlisted() -> Vec<(&'static str, String, Option<Value>)> {
    let c = READ_CHANNEL;
    vec![
        ("GET", format!("/api/v1/channels/{c}/messages"), None),
        ("GET", format!("/api/v1/channels/{c}/messages/123"), None),
        ("GET", format!("/api/v1/channels/{c}/page"), None),
        ("GET", format!("/api/v1/channels/{c}/count"), None),
        ("GET", format!("/api/v1/channels/{c}/digest"), None),
        ("GET", "/api/v1/activity".into(), None),
        ("GET", "/api/v1/diagnostics".into(), None),
        ("GET", "/api/v1/read-aloud/voices".into(), None),
        ("GET", "/api/v1/search?q=cats".into(), None),
        ("GET", format!("/api/v1/channels/{c}/search?q=cats"), None),
        ("GET", "/api/v1/client-settings".into(), None),
        ("GET", "/api/v1/voice-settings".into(), None),
        ("GET", "/api/v1/voice-prompt".into(), None),
        ("GET", format!("/api/v1/channels/{c}/upstream-clear"), None),
        ("GET", format!("/api/v1/channels/{c}/history"), None),
        ("GET", format!("/api/v1/channels/{c}/stream"), None),
        ("GET", "/api/v1/live/stream".into(), None),
        (
            "POST",
            format!("/api/v1/channels/{c}/resolve"),
            Some(json!({"query": "cats"})),
        ),
    ]
}

#[tokio::test]
async fn the_test_credential_is_refused_off_the_allowlist_without_upstream_calls() {
    let (state, discord) = state();
    let spent = discord.fetch_count();
    for (method, uri, body) in non_allowlisted() {
        let (status, answer) = call(&state, method, &uri, TEST_TOKEN, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {answer}");
        assert_eq!(
            discord.fetch_count(),
            spent,
            "{method} {uri} reached the provider"
        );
    }
}

#[tokio::test]
async fn the_allowlisted_routes_answer_and_the_timeline_refuses_rather_than_fetch() {
    let (state, discord) = state();
    let spent = discord.fetch_count();
    let (status, config) = call(&state, "GET", "/api/v1/client-config", TEST_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["test_read_only"], json!(true));
    // No cache is installed in this state, so the timeline cannot be
    // served: it must refuse (test_read_cache_only), never fetch.
    let (status, answer) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline"),
        TEST_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["error"], json!("test_read_cache_only"));
    assert_eq!(
        discord.fetch_count(),
        spent,
        "the timeline fetched upstream"
    );
}

#[tokio::test]
async fn speech_and_mcp_refuse_the_test_credential_distinctly() {
    let (state, _discord) = state();
    let (status, answer) = call(
        &state,
        "POST",
        &format!("/api/v1/channels/{READ_CHANNEL}/speech/prepare"),
        TEST_TOKEN,
        Some(json!({"ids": ["123"]})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["error"], json!("test_read_speech_refused"));
    let (status, _answer) = call(
        &state,
        "POST",
        "/mcp",
        TEST_TOKEN,
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_read_token_keeps_its_routes() {
    let (state, _discord) = state();
    let (status, answer) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/messages"),
        READ_TOKEN,
        None,
    )
    .await;
    assert_ne!(answer["error"], json!("test_read_route_refused"));
    assert_ne!(status, StatusCode::FORBIDDEN, "{answer}");
}
