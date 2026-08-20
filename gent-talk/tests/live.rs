//! End-to-end tests of the live stream: `GET /api/v1/channels/{id}/stream`.
//!
//! Driven through the real router, exactly as `tests/api.rs` drives everything else, so the
//! bearer check, the channel allowlist, the SSE framing and the headers are the ones a browser
//! would meet.
//!
//! **One harness detail is load-bearing and cost an hour the first time.** `BodyExt::collect()`
//! HANGS on an endless body: an SSE response never ends, so "read the whole body and assert on it"
//! is a test that never finishes and reports nothing. Frames are read INCREMENTALLY here, inside a
//! `tokio::time::timeout`, and the timeout is what turns "the page never got the message" into a
//! failure instead of a hung suite.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gent_talk::discord::fake::FakeDiscord;
use gent_talk::http::router;
use gent_talk::live::LiveHub;
use gent_talk::model::ChannelId;
use gent_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

struct Harness {
    router: axum::Router,
    discord: Arc<FakeDiscord>,
    live: Arc<LiveHub>,
}

fn harness() -> Harness {
    let (state, discord) = gent_talk::testing::state();
    let live = Arc::clone(&state.live);
    Harness {
        router: router(state.clone()),
        discord,
        live,
    }
}

/// Ingest whatever the fake currently holds for `channel`, exactly as a poll tick would.
///
/// This is the real [`gent_talk::live::poll_once`], not a shortcut around it, so the seeding rule
/// applies here too: the first call publishes nothing.
async fn tick(harness: &Harness, channel: &ChannelId, cursor: &mut Option<u64>) -> usize {
    gent_talk::live::poll_once(
        harness.discord.as_ref(),
        harness.live.as_ref(),
        channel,
        50,
        cursor,
    )
    .await
    .expect("the fake reads")
}

async fn open(
    harness: &Harness,
    uri: &str,
    token: Option<&str>,
    last_event_id: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(id) = last_event_id {
        builder = builder.header("last-event-id", id);
    }
    harness
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("router responds")
}

/// Read frames until `want` complete SSE events have arrived, or fail by timing out.
///
/// An SSE event ends at a blank line, so counting `\n\n` counts events.
async fn read_events(body: &mut Body, want: usize) -> String {
    let mut text = String::new();
    while text.matches("\n\n").count() < want {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .unwrap_or_else(|_| {
                panic!("the stream produced nothing within five seconds; so far: {text:?}")
            })
            .expect("the stream ended before it delivered anything")
            .expect("the frame is readable");
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    text
}

/// The `data:` payloads of every complete event in `text`, parsed.
fn payloads(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|json| serde_json::from_str(json.trim()).ok())
        .collect()
}

#[tokio::test]
async fn an_unauthenticated_caller_cannot_open_a_stream() {
    let harness = harness();
    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_channel_outside_the_allowlist_is_refused_rather_than_streamed_empty() {
    // The failure this exists for: a stream is the one route where "no such channel" and "nothing
    // is happening in that channel" look identical from the client, forever. So it must refuse.
    let harness = harness();
    let response = open(
        &harness,
        "/api/v1/channels/9999999999/stream",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("an error body is finite")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(payload["error"], "unknown_channel");
}

#[tokio::test]
async fn a_message_published_after_attach_reaches_an_attached_subscriber() {
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    harness.discord.seed(&channel, "codex-eng", "already here");
    let mut cursor = None;
    assert_eq!(
        tick(&harness, &channel, &mut cursor).await,
        0,
        "the seeding tick must publish nothing"
    );

    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();

    harness
        .discord
        .seed(&channel, "claude-integ", "the runner came back");
    assert_eq!(tick(&harness, &channel, &mut cursor).await, 1);

    let text = read_events(&mut body, 1).await;
    assert!(
        text.contains("event: message"),
        "every event must name itself so a page can branch on the type: {text:?}"
    );
    assert!(
        text.contains("id: 1"),
        "every event must carry its message id, or Last-Event-ID has nothing to send back: \
         {text:?}"
    );
    let payload = payloads(&text).pop().expect("one parsed event");
    assert_eq!(payload["message"]["content"], "the runner came back");
    assert_eq!(payload["self_posted"], false);
    assert!(
        payload["untrusted_content_notice"]
            .as_str()
            .expect("the notice rides along")
            .contains("DATA"),
        "a pushed message is third-party text exactly as a fetched one is: {payload}"
    );
    assert_eq!(
        payload["message"]["content"].as_str(),
        Some("the runner came back")
    );
    assert!(
        !text.contains("already here"),
        "the history that predates the subscriber must not be delivered as news: {text:?}"
    );
}

#[tokio::test]
async fn last_event_id_replays_only_what_came_after_it() {
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;

    harness.discord.seed(&channel, "a", "one");
    let second = harness.discord.seed(&channel, "a", "two");
    harness.discord.seed(&channel, "a", "three");
    assert_eq!(tick(&harness, &channel, &mut cursor).await, 3);

    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        Some(second.as_str()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let text = read_events(&mut body, 1).await;
    let seen: Vec<String> = payloads(&text)
        .iter()
        .filter_map(|p| p["message"]["content"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        seen,
        vec!["three".to_owned()],
        "resuming must deliver strictly what came AFTER the id the page last saw: {text:?}"
    );
}

#[tokio::test]
async fn a_reconnect_with_no_last_event_id_gets_the_whole_replay_tail() {
    // The control for the test above: without it, an implementation that replayed nothing at all
    // would satisfy "only what came after".
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;
    harness.discord.seed(&channel, "a", "one");
    harness.discord.seed(&channel, "a", "two");
    tick(&harness, &channel, &mut cursor).await;

    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    let mut body = response.into_body();
    let text = read_events(&mut body, 2).await;
    let seen: Vec<String> = payloads(&text)
        .iter()
        .filter_map(|p| p["message"]["content"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(seen, vec!["one".to_owned(), "two".to_owned()]);
}

#[tokio::test]
async fn the_stream_is_not_cached_and_is_not_buffered_by_a_reverse_proxy() {
    let harness = harness();
    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "no-store"
    );
    assert_eq!(
        headers
            .get("x-accel-buffering")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "no",
        "without this an nginx-shaped proxy buffers the whole response and the stream is dead \
         behind the tunnel while working perfectly on localhost"
    );
    assert!(headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
}

#[tokio::test]
async fn a_message_this_server_posted_is_marked_so_the_page_does_not_relay_it_back() {
    // The feedback loop, end to end: `ops::reply` posts as the bot, the poller then sees that very
    // message, and relaying it into the live conversation would have the agent answering itself.
    let harness = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;

    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/channels/{WRITE_CHANNEL}/reply"))
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"text": "landed it"}).to_string(),
        ))
        .expect("request");
    let posted = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(posted.status(), StatusCode::OK);

    harness.discord.seed(&channel, "someone-else", "nice");
    assert_eq!(tick(&harness, &channel, &mut cursor).await, 2);

    let response = open(
        &harness,
        &format!("/api/v1/channels/{WRITE_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    let mut body = response.into_body();
    let text = read_events(&mut body, 2).await;
    let flags: Vec<(String, bool)> = payloads(&text)
        .iter()
        .map(|p| {
            (
                p["message"]["content"].as_str().unwrap_or("").to_owned(),
                p["self_posted"].as_bool().unwrap_or(false),
            )
        })
        .collect();
    assert_eq!(
        flags,
        vec![("landed it".to_owned(), true), ("nice".to_owned(), false),],
        "only the message this server posted may be flagged, and it must still be DELIVERED — the \
         channel view shows it; it is the RELAY to the agent the page suppresses: {text:?}"
    );
}

#[tokio::test]
async fn the_hub_is_reachable_from_the_shared_state_so_a_second_reader_sees_the_same_feed() {
    // Two subscribers, one publish. A hub that handed each caller its own channel would pass every
    // other test here and deliver a message to exactly one of two open phones.
    let (state, discord) = gent_talk::testing::state();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let hub: &Arc<LiveHub> = &state.live;
    let mut first = hub.subscribe(&channel, None);
    let mut second = hub.subscribe(&channel, None);
    let mut cursor = None;
    gent_talk::live::poll_once(discord.as_ref(), hub.as_ref(), &channel, 50, &mut cursor)
        .await
        .expect("seeds");
    discord.seed(&channel, "a", "to both of you");
    gent_talk::live::poll_once(discord.as_ref(), hub.as_ref(), &channel, 50, &mut cursor)
        .await
        .expect("polls");
    assert_eq!(
        first.receiver.try_recv().expect("first").message.content,
        "to both of you"
    );
    assert_eq!(
        second.receiver.try_recv().expect("second").message.content,
        "to both of you"
    );
}
