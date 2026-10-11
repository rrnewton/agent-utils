//! `#232 upstream-mark-read`: clearing a channel's unread state at its chat provider, through the
//! real router, store and ingest hub, against the in-memory provider.
//!
//! The loop's own timing (one push per burst, never backwards, retry waits) is tested beside it in
//! `src/upstream_clear.rs`; this file is about the setting: who may see and change it, that it is
//! kept by the server, that a channel whose provider cannot do it is refused, and that a message
//! ingested for an enabled channel reaches the provider.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::model::ChannelId;
use vibe_talk::state::AppState;
use vibe_talk::store::{StateStore as _, UpstreamClearMode};
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::upstream_clear::Clearer;

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
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

fn path(channel: &str) -> String {
    format!("/api/v1/channels/{channel}/upstream-clear")
}

#[tokio::test]
async fn it_is_off_by_default_and_not_offered_by_a_provider_that_cannot() {
    let (state, _discord, _store) = vibe_talk::testing::state_with_store();
    let (status, config) = call(&state, "GET", "/api/v1/client-config", READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        config.get("upstream_clear_supported").is_none(),
        "advertised by a provider that cannot: {config}"
    );
    let (status, view) = call(&state, "GET", &path(WRITE_CHANNEL), READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["mode"], "off");
    assert_eq!(view["supported"], false);
    let (status, refused) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        WRITE_TOKEN,
        Some(json!({ "mode": "on" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "refused");
    assert!(
        state
            .store
            .upstream_clear_settings()
            .await
            .expect("read")
            .is_empty(),
        "a refused setting was stored"
    );
}

#[tokio::test]
async fn the_owner_turns_it_on_the_server_keeps_it_and_a_read_token_cannot_change_it() {
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    discord.enable_upstream_clear();
    let (_, config) = call(&state, "GET", "/api/v1/client-config", READ_TOKEN, None).await;
    assert_eq!(config["upstream_clear_supported"], true);

    let (status, _) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        READ_TOKEN,
        Some(json!({ "mode": "on" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, view) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        WRITE_TOKEN,
        Some(json!({ "mode": "dry_run" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["mode"], "dry_run");
    assert_eq!(view["supported"], true);
    assert_eq!(view["provider_name"], "Discord");
    assert!(view["enabled_at_ms"].as_i64().is_some(), "{view}");
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    assert_eq!(
        state.live.upstream_clear().mode(&channel),
        UpstreamClearMode::DryRun,
        "the running loop was not told"
    );
    let stored = store.upstream_clear_settings().await.expect("read");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].mode, UpstreamClearMode::DryRun);

    // Seen at read scope, as the server keeps it.
    let (status, view) = call(&state, "GET", &path(WRITE_CHANNEL), READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["mode"], "dry_run");
    // The other channel is untouched.
    let (_, other) = call(&state, "GET", &path(READ_CHANNEL), READ_TOKEN, None).await;
    assert_eq!(other["mode"], "off");

    let (status, view) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        WRITE_TOKEN,
        Some(json!({ "mode": "off" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["mode"], "off");
    assert!(view.get("enabled_at_ms").is_none(), "{view}");
    assert_eq!(
        state.live.upstream_clear().mode(&channel),
        UpstreamClearMode::Off
    );
}

#[tokio::test]
async fn a_bad_request_an_unknown_channel_and_a_server_without_a_store_are_refused() {
    let (state, discord, _store) = vibe_talk::testing::state_with_store();
    discord.enable_upstream_clear();
    for body in [
        json!({ "mode": "sometimes" }),
        json!({}),
        json!({ "mode": "on", "everything": true }),
    ] {
        let (status, _) = call(
            &state,
            "PUT",
            &path(WRITE_CHANNEL),
            WRITE_TOKEN,
            Some(body.clone()),
        )
        .await;
        assert!(status.is_client_error(), "{body} was accepted: {status}");
    }
    let (status, _) = call(
        &state,
        "PUT",
        &path("9999999999"),
        WRITE_TOKEN,
        Some(json!({ "mode": "on" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (state, discord) =
        vibe_talk::testing::state_with(Arc::new(vibe_talk::store::disabled::DisabledStore));
    discord.enable_upstream_clear();
    let (status, body) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        WRITE_TOKEN,
        Some(json!({ "mode": "on" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "storage_not_configured");
    let (status, _) = call(&state, "GET", &path(WRITE_CHANNEL), READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn clear_now_pushes_the_newest_message_and_settings_reports_it() {
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    discord.enable_upstream_clear();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    discord.seed(&channel, "agent", "older");
    let newest = discord.seed(&channel, "agent", "newest");
    let (status, view) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        WRITE_TOKEN,
        Some(json!({ "mode": "on", "clear_now": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["pending"], true, "{view}");
    assert!(
        discord.upstream_clears().is_empty(),
        "the request pushed inline instead of leaving it to the loop"
    );

    let clearer = Clearer::for_state(&state);
    assert!(clearer.push_due().await, "clear-now was not due at once");
    assert_eq!(
        discord.upstream_clears(),
        vec![(channel.clone(), newest.clone(), false)]
    );

    let (_, view) = call(&state, "GET", &path(WRITE_CHANNEL), READ_TOKEN, None).await;
    assert_eq!(view["pending"], false);
    assert_eq!(view["pushed"]["message_id"], newest.as_str());
    assert_eq!(view["pushed"]["result"], "advanced");
    assert_eq!(view["last"]["result"], "advanced");
    assert_eq!(view["failures"], 0);
    assert_eq!(
        store.upstream_clear_settings().await.expect("read")[0]
            .pushed
            .as_ref()
            .map(|pushed| pushed.message.clone()),
        Some(newest)
    );
}

#[tokio::test(start_paused = true)]
async fn a_message_ingested_for_an_enabled_channel_reaches_the_provider_and_others_do_not() {
    let (state, discord, _store) = vibe_talk::testing::state_with_store();
    discord.enable_upstream_clear();
    let (status, _) = call(
        &state,
        "PUT",
        &path(WRITE_CHANNEL),
        WRITE_TOKEN,
        Some(json!({ "mode": "on" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let enabled = ChannelId(WRITE_CHANNEL.to_owned());
    let other = ChannelId(READ_CHANNEL.to_owned());
    let in_enabled = discord.seed(&enabled, "agent", "build green");
    let in_other = discord.seed(&other, "person", "lunch?");
    // The hub is where both producers land; publishing is what the poller and the adapter do.
    state
        .live
        .publish(&enabled, discord.message(&in_enabled).expect("seeded"));
    state
        .live
        .publish(&other, discord.message(&in_other).expect("seeded"));
    assert!(state.live.upstream_clear().status(&enabled).pending);
    assert!(!state.live.upstream_clear().status(&other).pending);
    let clearer = Clearer::for_state(&state);
    assert!(
        !clearer.push_due().await,
        "pushed before the channel went quiet"
    );
    tokio::time::advance(vibe_talk::upstream_clear::DEFAULT_QUIET).await;
    assert!(
        clearer.push_due().await,
        "not pushed once the channel went quiet"
    );
    assert!(!clearer.push_due().await, "pushed twice");
    assert_eq!(
        discord.upstream_clears(),
        vec![(enabled.clone(), in_enabled, false)]
    );
    assert_eq!(discord.upstream_cursor(&other), None);
}
