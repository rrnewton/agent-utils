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
        // The rest of the ReadScope surface (second cold review).
        ("GET", "/api/v1/channels".into(), None),
        ("GET", "/api/v1/agent-tools".into(), None),
        ("GET", "/api/v1/inbox".into(), None),
        ("GET", "/api/v1/voice-agent".into(), None),
        ("GET", "/api/v1/voice-post-prefix".into(), None),
        ("GET", "/api/v1/documents".into(), None),
        ("GET", "/api/v1/documents/search?q=cats".into(), None),
        ("GET", "/api/v1/documents/some-key".into(), None),
        ("GET", "/api/v1/transcripts".into(), None),
        (
            "GET",
            format!("/api/v1/transcripts/{c}/1795263665.619359"),
            None,
        ),
        ("GET", format!("/api/v1/channels/{c}/recap"), None),
        (
            "GET",
            format!("/api/v1/channels/{c}/messages/123/summary"),
            None,
        ),
        ("GET", format!("/api/v1/channels/{c}/pins"), None),
        ("GET", format!("/api/v1/channels/{c}/thread-pins"), None),
        ("GET", format!("/api/v1/channels/{c}/bookmark"), None),
        ("GET", "/api/v1/labels".into(), None),
        // Suffix variants: paths that merely look like the timeline
        // route (B1). Each must be refused as a route, before any
        // handler or provider call.
        (
            "GET",
            format!("/api/v1/channels/{c}/messages/timeline"),
            None,
        ),
    ]
}

/// Write-scope routes with the test credential: refused by scope, as
/// every mutating route is. A representative sweep.
fn write_routes() -> Vec<(&'static str, String, Option<Value>)> {
    let c = READ_CHANNEL;
    vec![
        (
            "POST",
            format!("/api/v1/channels/{c}/reply"),
            Some(json!({"content": "x"})),
        ),
        (
            "POST",
            format!("/api/v1/channels/{c}/dismiss"),
            Some(json!({"messages": []})),
        ),
        (
            "POST",
            format!("/api/v1/channels/{c}/read"),
            Some(json!({"message_id": "1"})),
        ),
        (
            "PUT",
            "/api/v1/client-settings".into(),
            Some(json!({"settings": []})),
        ),
        ("POST", "/api/v1/channels".into(), Some(json!({"id": "9"}))),
        (
            "PUT",
            format!("/api/v1/channels/{c}/alias"),
            Some(json!({"alias": "x"})),
        ),
        (
            "DELETE",
            format!("/api/v1/channels/{c}/pins/timeline"),
            None,
        ),
    ]
}

#[tokio::test]
async fn the_test_credential_is_refused_on_write_routes_too() {
    let (state, discord) = state();
    let spent = discord.fetch_count();
    for (method, uri, body) in write_routes() {
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

/// A served newest page for the test credential (second cold review):
/// it answers from the cache while the provider's next read is held,
/// it reaches the provider zero times, and it leaves the refresh claim
/// free for the owner's own correction.
#[tokio::test]
async fn a_served_page_for_the_test_credential_takes_no_claim_and_spawns_no_refresh() {
    use std::sync::Arc;
    use std::time::Duration;
    use vibe_talk::cache::RefreshKey;
    use vibe_talk::model::ChannelId;
    use vibe_talk::testing::{ManualClock, TempDir, TimelineChat};
    use vibe_talk::threads::TimelineView;

    const T0: i64 = 1_787_054_400_000;
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;

    let dir = TempDir::new("test-read-served");
    let toml = testing::config_toml().replace(
        &format!("write_token = \"{WRITE_TOKEN}\""),
        &format!("write_token = \"{WRITE_TOKEN}\"\ntest_token = \"{TEST_TOKEN}\""),
    );
    let (mut state, fake, _voice) = testing::state_from_toml(&toml);
    let read = ChannelId(READ_CHANNEL.to_owned());
    for n in 0..12 {
        fake.seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE);
    }
    let chat = Arc::new(TimelineChat::new(fake.clone()));
    state.replace_chat(chat.clone());
    let clock = ManualClock::at(T0 + HOUR);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let (status, answer) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        state
            .message_cache
            .shadow_settled(Duration::from_secs(20))
            .await,
        "every evaluation was counted"
    );
    if let Some(cache) = state.message_cache.cache() {
        assert!(
            cache.quiesce_blocking(Duration::from_secs(20)),
            "the writer caught up"
        );
    }
    clock.advance(2 * HOUR);
    state.message_cache.set_serve(true);

    let _hold = chat.hold_next_timeline();
    let spent = fake.fetch_count();
    let (status, answer) = tokio::time::timeout(
        Duration::from_secs(10),
        call(
            &state,
            "GET",
            &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=5"),
            TEST_TOKEN,
            None,
        ),
    )
    .await
    .expect("a served page answers while the provider is held");
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["messages"].as_array().map(Vec::len), Some(5));
    assert_eq!(
        fake.fetch_count(),
        spent,
        "the served read reached the provider"
    );
    let key = RefreshKey::timeline(read, TimelineView::Main, None, 5);
    assert!(
        state.message_cache.claim_refresh(key).is_some(),
        "the test credential took the owner's refresh claim"
    );
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
