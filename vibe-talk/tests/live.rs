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
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::http::router;
use vibe_talk::live::LiveHub;
use vibe_talk::model::ChannelId;
use vibe_talk::state::AppState;
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

struct Harness {
    router: axum::Router,
    discord: Arc<FakeDiscord>,
    live: Arc<LiveHub>,
    state: AppState,
}

fn harness() -> Harness {
    let (state, discord) = vibe_talk::testing::state();
    let live = Arc::clone(&state.live);
    Harness {
        router: router(state.clone()),
        discord,
        live,
        state,
    }
}

async fn add_channel(harness: &Harness, id: &str, label: &str) -> StatusCode {
    harness
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/channels")
                .header("authorization", format!("Bearer {WRITE_TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "id": id, "label": label }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("router responds")
        .status()
}

async fn wait_for_fetches(discord: &FakeDiscord, minimum: usize) {
    for _ in 0..100 {
        if discord.fetch_count() >= minimum {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "the poller made {} fetches, fewer than the expected {minimum}",
        discord.fetch_count()
    );
}

/// Ingest whatever the fake currently holds for `channel`, exactly as a poll tick would.
///
/// This is the real [`vibe_talk::live::poll_once`], not a shortcut around it, so the seeding rule
/// applies here too: the first call publishes nothing.
async fn tick(harness: &Harness, channel: &ChannelId, cursor: &mut Option<u64>) -> usize {
    vibe_talk::live::poll_once(
        harness.discord.as_ref(),
        harness.live.as_ref(),
        channel,
        50,
        cursor,
    )
    .await
    .expect("the fake reads")
    .published
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

/// Read frames until the stream ENDS, or fail by timing out.
///
/// The counterpart of [`read_events`], and the only way to assert that a stream stopped: reading
/// for a fixed number of events cannot tell "there are no more" from "the next one is late".
async fn read_to_end(body: &mut Body) -> String {
    let mut text = String::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .unwrap_or_else(|_| {
                panic!("the stream was still open after five seconds; so far: {text:?}")
            });
        let Some(frame) = frame else {
            return text;
        };
        let frame = frame.expect("the frame is readable");
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_channel_added_while_the_poller_is_running_publishes_new_messages() {
    let harness = harness();
    let interval = Duration::from_secs(10);
    let poller = tokio::spawn(vibe_talk::live::poll_forever(
        harness.state.clone(),
        None,
        harness.state.chat.clone(),
        50,
        interval,
    ));
    wait_for_fetches(harness.discord.as_ref(), 2).await;

    let fresh = ChannelId("3333333333333333333".to_owned());
    harness
        .discord
        .seed(&fresh, "codex-eng", "present when the channel was added");
    assert_eq!(
        add_channel(&harness, fresh.as_str(), "second team").await,
        StatusCode::OK
    );
    let after_add_probe = harness.discord.fetch_count();

    tokio::time::advance(interval).await;
    wait_for_fetches(harness.discord.as_ref(), after_add_probe + 3).await;
    let mut subscription = harness.live.subscribe(&fresh, None);
    assert!(
        subscription.replay.is_empty(),
        "the first poll of a newly added channel must seed its cursor, not publish its history"
    );

    harness
        .discord
        .seed(&fresh, "codex-eng", "arrived after the channel was added");
    let before_live_tick = harness.discord.fetch_count();
    tokio::time::advance(interval).await;
    wait_for_fetches(harness.discord.as_ref(), before_live_tick + 3).await;
    let published = subscription.receiver.try_recv().expect(
        "the running poller did not publish a message from the channel added through the API",
    );
    assert_eq!(
        published.message.content,
        "arrived after the channel was added"
    );
    poller.abort();
}

#[tokio::test(start_paused = true)]
async fn a_channel_restored_at_startup_is_polled_and_publishes_new_messages() {
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    let fresh = ChannelId("4444444444444444444".to_owned());
    store
        .add_channel(&fresh, "restored team", false, None, 1)
        .await
        .expect("store added channel");
    assert_eq!(
        state
            .restore_added_channels()
            .await
            .expect("restore channels"),
        1
    );
    discord.seed(&fresh, "codex-eng", "present at startup");

    let interval = Duration::from_secs(10);
    let poller = tokio::spawn(vibe_talk::live::poll_forever(
        state.clone(),
        None,
        state.chat.clone(),
        50,
        interval,
    ));
    wait_for_fetches(discord.as_ref(), 3).await;
    let mut subscription = state.live.subscribe(&fresh, None);
    assert!(
        subscription.replay.is_empty(),
        "the startup poll must seed the restored channel instead of replaying old history"
    );

    discord.seed(&fresh, "codex-eng", "arrived after restart");
    let before_live_tick = discord.fetch_count();
    tokio::time::advance(interval).await;
    wait_for_fetches(discord.as_ref(), before_live_tick + 3).await;
    let published = subscription
        .receiver
        .try_recv()
        .expect("the poller did not publish a message from the restored channel");
    assert_eq!(published.message.content, "arrived after restart");
    poller.abort();
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
async fn a_replayed_message_is_labelled_as_replayed_and_a_live_one_is_not() {
    // The burst this exists to stop: a page that attaches with no `Last-Event-ID` — a fresh
    // sign-in, a channel change, the reconnect after an `event: reset` — is handed the whole tail,
    // up to two hundred messages. They belong on screen. They must NOT be announced to a live,
    // billed conversation as "a message was just posted", and the page cannot tell them from a
    // live arrival unless this server says which is which.
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;
    harness
        .discord
        .seed(&channel, "a", "said before you got here");
    assert_eq!(tick(&harness, &channel, &mut cursor).await, 1);

    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    let mut body = response.into_body();
    let tail = read_events(&mut body, 1).await;
    let held = payloads(&tail).pop().expect("the tail event");
    assert_eq!(held["message"]["content"], "said before you got here");
    assert_eq!(
        held["replayed"], true,
        "a message out of the replay tail must say so, or the page relays stale channel text into \
         a paid conversation as news: {tail:?}"
    );
    assert_eq!(
        held["from_tail"], true,
        "the page skips a read only for what every read after the attach has seen: {tail:?}"
    );

    harness
        .discord
        .seed(&channel, "a", "and this just happened");
    assert_eq!(tick(&harness, &channel, &mut cursor).await, 1);
    let live = read_events(&mut body, 1).await;
    let arrived = payloads(&live).pop().expect("the live event");
    assert_eq!(arrived["message"]["content"], "and this just happened");
    assert_eq!(
        arrived["replayed"], false,
        "and a message that really did just arrive must NOT be marked replayed, or the feature \
         relays nothing at all: {live:?}"
    );
    assert_eq!(arrived["from_tail"], false, "{live:?}");
}

#[tokio::test]
async fn a_subscriber_that_falls_behind_gets_one_reset_and_the_stream_ends() {
    // The lag path, which had no server-side test at all: the page branches on the event NAME, and
    // the whole argument for `reset` over a short resume is that the stream STOPS so the page has
    // to re-read. Both halves are pinned here — a stream that carried on after the reset would
    // leave a page reading a feed with a hole in it and no way to know.
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;

    // Attached and then never read, which is exactly what a stalled browser looks like.
    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();

    let overflow = vibe_talk::live::BROADCAST_CAPACITY + 8;
    for n in 0..overflow {
        harness.discord.seed(&channel, "a", &format!("m{n}"));
    }
    // Two ticks, because one tick is bounded by `MAX_PAGES_PER_TICK`; between them everything
    // seeded is published, and the subscriber has read none of it.
    let mut published = tick(&harness, &channel, &mut cursor).await;
    while published < overflow {
        let more = tick(&harness, &channel, &mut cursor).await;
        assert!(more > 0, "the walk stalled with {published} of {overflow}");
        published += more;
    }

    let text = read_to_end(&mut body).await;
    assert!(
        text.contains("event: reset"),
        "a subscriber this far behind must be told to re-read, by the event name the page branches \
         on: {text:?}"
    );
    assert!(
        !text.contains(&format!("m{}", overflow - 1)),
        "and the reset must be the LAST thing on the stream — carrying on afterwards resumes short \
         over a gap the page cannot see: {text:?}"
    );
    // `read_to_end` returning at all is the other half: the body ENDED rather than parking.
    let after_reset = text
        .split("event: reset")
        .nth(1)
        .expect("the reset is in there");
    assert!(
        !after_reset.contains("event: message"),
        "nothing may follow the reset: {after_reset:?}"
    );
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
    let (state, discord) = vibe_talk::testing::state();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let hub: &Arc<LiveHub> = &state.live;
    let mut first = hub.subscribe(&channel, None);
    let mut second = hub.subscribe(&channel, None);
    let mut cursor = None;
    vibe_talk::live::poll_once(discord.as_ref(), hub.as_ref(), &channel, 50, &mut cursor)
        .await
        .expect("seeds");
    discord.seed(&channel, "a", "to both of you");
    vibe_talk::live::poll_once(discord.as_ref(), hub.as_ref(), &channel, 50, &mut cursor)
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

// --- the stream of every channel: `GET /api/v1/live/stream` ----------------------------------------
//
// `#337 voice-relay-silent`. A message arrived in a channel the owner watches during a call, and the
// call never heard of it: the page followed only the channel on screen's stream. A page in a call
// reads this one as well, which carries every listed channel.

/// The `id:` of every event in `text`, in order.
fn event_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .map(str::to_owned)
        .collect()
}

/// `(channel, content, replayed)` for every message event in `text`.
fn arrivals(text: &str) -> Vec<(String, String, bool)> {
    payloads(text)
        .iter()
        .map(|p| {
            (
                p["message"]["channel_id"].as_str().unwrap_or("").to_owned(),
                p["message"]["content"].as_str().unwrap_or("").to_owned(),
                p["replayed"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_stream_of_every_channel_carries_each_listed_channel_from_attach_and_none_of_its_past()
{
    let harness = harness();
    let read = ChannelId(READ_CHANNEL.to_owned());
    let write = ChannelId(WRITE_CHANNEL.to_owned());
    let (mut read_cursor, mut write_cursor) = (None, None);
    tick(&harness, &read, &mut read_cursor).await;
    tick(&harness, &write, &mut write_cursor).await;
    harness.discord.seed(&read, "a", "said before the call");
    assert_eq!(tick(&harness, &read, &mut read_cursor).await, 1);

    let response = open(&harness, "/api/v1/live/stream", Some(READ_TOKEN), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    // First, where it starts: an `id` and nothing else.
    let start = read_events(&mut body, 1).await;
    assert_eq!(event_ids(&start).len(), 1, "{start:?}");
    assert!(
        payloads(&start).is_empty(),
        "a fresh attach replayed history: {start:?}"
    );

    harness.discord.seed(&write, "b", "in the other channel");
    assert_eq!(tick(&harness, &write, &mut write_cursor).await, 1);
    harness.discord.seed(&read, "c", "in the first channel");
    assert_eq!(tick(&harness, &read, &mut read_cursor).await, 1);
    let text = read_events(&mut body, 2).await;
    assert_eq!(
        arrivals(&text),
        vec![
            (
                WRITE_CHANNEL.to_owned(),
                "in the other channel".to_owned(),
                false
            ),
            (
                READ_CHANNEL.to_owned(),
                "in the first channel".to_owned(),
                false
            ),
        ],
        "{text:?}"
    );
    assert!(!text.contains("said before the call"), "{text:?}");
    let payload = payloads(&text).pop().expect("an event");
    assert!(
        payload["untrusted_content_notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("DATA")),
        "the payload is a channel stream's, notice and all: {payload}"
    );
    // Each event's id is the stream's own cursor, not the channel's message id, and every one is
    // distinct, so one `Last-Event-ID` can say how far a page got in all of them.
    let ids = event_ids(&text);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert!(
        ids.iter().all(|id| id.contains('.')),
        "ids are cursors: {ids:?}"
    );
}

#[tokio::test]
async fn a_channel_stream_carries_only_its_own_channel_which_is_why_the_stream_of_every_channel_exists(
) {
    // The negative control for the test above: the stream a page in a call used to follow alone
    // never carries a message from another channel.
    let harness = harness();
    let read = ChannelId(READ_CHANNEL.to_owned());
    let write = ChannelId(WRITE_CHANNEL.to_owned());
    let (mut read_cursor, mut write_cursor) = (None, None);
    tick(&harness, &read, &mut read_cursor).await;
    tick(&harness, &write, &mut write_cursor).await;
    let response = open(
        &harness,
        &format!("/api/v1/channels/{READ_CHANNEL}/stream"),
        Some(READ_TOKEN),
        None,
    )
    .await;
    let mut body = response.into_body();
    harness.discord.seed(&write, "b", "in the other channel");
    tick(&harness, &write, &mut write_cursor).await;
    harness.discord.seed(&read, "c", "in this channel");
    tick(&harness, &read, &mut read_cursor).await;
    let text = read_events(&mut body, 1).await;
    assert_eq!(
        arrivals(&text),
        vec![(READ_CHANNEL.to_owned(), "in this channel".to_owned(), false)],
        "{text:?}"
    );
}

#[tokio::test]
async fn the_stream_of_every_channel_resumes_from_its_cursor_with_exactly_what_was_missed() {
    let harness = harness();
    let read = ChannelId(READ_CHANNEL.to_owned());
    let write = ChannelId(WRITE_CHANNEL.to_owned());
    let (mut read_cursor, mut write_cursor) = (None, None);
    tick(&harness, &read, &mut read_cursor).await;
    tick(&harness, &write, &mut write_cursor).await;

    let response = open(&harness, "/api/v1/live/stream", Some(READ_TOKEN), None).await;
    let mut body = response.into_body();
    let start = read_events(&mut body, 1).await;
    let cursor = event_ids(&start).pop().expect("the starting cursor");
    drop(body);

    // While the page was away: one message in each channel.
    harness
        .discord
        .seed(&write, "b", "missed in the other channel");
    tick(&harness, &write, &mut write_cursor).await;
    harness
        .discord
        .seed(&read, "c", "missed in the first channel");
    tick(&harness, &read, &mut read_cursor).await;

    let response = open(
        &harness,
        "/api/v1/live/stream",
        Some(READ_TOKEN),
        Some(&cursor),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let text = read_events(&mut body, 2).await;
    assert_eq!(
        arrivals(&text),
        vec![
            (
                WRITE_CHANNEL.to_owned(),
                "missed in the other channel".to_owned(),
                true
            ),
            (
                READ_CHANNEL.to_owned(),
                "missed in the first channel".to_owned(),
                true
            ),
        ],
        "a resumed attach replays exactly what followed its cursor, marked as replayed: {text:?}"
    );
    assert!(
        payloads(&text).iter().all(|p| p["from_tail"] == true),
        "{text:?}"
    );
    // Resumed from the newest cursor there is nothing to replay: the next event is the next arrival.
    let newest = event_ids(&text).pop().expect("a cursor");
    drop(body);
    let response = open(
        &harness,
        "/api/v1/live/stream",
        Some(READ_TOKEN),
        Some(&newest),
    )
    .await;
    let mut body = response.into_body();
    harness.discord.seed(&read, "d", "after the second attach");
    tick(&harness, &read, &mut read_cursor).await;
    let text = read_events(&mut body, 1).await;
    let messages = arrivals(&text);
    assert_eq!(
        messages.last(),
        Some(&(
            READ_CHANNEL.to_owned(),
            "after the second attach".to_owned(),
            false
        )),
        "{text:?}"
    );
    assert!(
        !text.contains("missed in"),
        "what was already delivered came again: {text:?}"
    );
}

#[tokio::test]
async fn a_cursor_the_stream_of_every_channel_cannot_resume_from_gets_one_reset_and_the_stream_ends(
) {
    let harness = harness();
    let read = ChannelId(READ_CHANNEL.to_owned());
    let mut read_cursor = None;
    tick(&harness, &read, &mut read_cursor).await;

    // Another process's cursor: a server restarted under the page.
    let response = open(
        &harness,
        "/api/v1/live/stream",
        Some(READ_TOKEN),
        Some("0.0"),
    )
    .await;
    let text = read_to_end(&mut response.into_body()).await;
    assert!(text.contains("event: reset"), "{text:?}");

    // One of this process's, whose successors have fallen out of the tail.
    let response = open(&harness, "/api/v1/live/stream", Some(READ_TOKEN), None).await;
    let mut body = response.into_body();
    let cursor = event_ids(&read_events(&mut body, 1).await)
        .pop()
        .expect("the starting cursor");
    drop(body);
    let overflow = vibe_talk::live::ALL_TAIL + 2;
    for n in 0..overflow {
        harness.discord.seed(&read, "a", &format!("m{n}"));
    }
    let mut published = 0;
    while published < overflow {
        let more = tick(&harness, &read, &mut read_cursor).await;
        assert!(more > 0, "the walk stalled with {published} of {overflow}");
        published += more;
    }
    let response = open(
        &harness,
        "/api/v1/live/stream",
        Some(READ_TOKEN),
        Some(&cursor),
    )
    .await;
    let text = read_to_end(&mut response.into_body()).await;
    assert!(
        text.contains("event: reset"),
        "a gap the tail no longer covers must be said, not replayed short: {text:?}"
    );
    assert!(
        !text.contains("event: message"),
        "nothing may come with the reset: {text:?}"
    );
}

#[tokio::test]
async fn a_channel_off_the_list_is_not_carried_by_the_stream_of_every_channel() {
    let harness = harness();
    let read = ChannelId(READ_CHANNEL.to_owned());
    let write = ChannelId(WRITE_CHANNEL.to_owned());
    let (mut read_cursor, mut write_cursor) = (None, None);
    tick(&harness, &read, &mut read_cursor).await;
    tick(&harness, &write, &mut write_cursor).await;
    let removed = harness
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/channels/{WRITE_CHANNEL}"))
                .header("authorization", format!("Bearer {WRITE_TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router responds");
    assert!(
        removed.status().is_success(),
        "taking the channel off the list: {}",
        removed.status()
    );

    let response = open(&harness, "/api/v1/live/stream", Some(READ_TOKEN), None).await;
    let mut body = response.into_body();
    read_events(&mut body, 1).await;
    harness
        .discord
        .seed(&write, "b", "in the channel off the list");
    tick(&harness, &write, &mut write_cursor).await;
    harness.discord.seed(&read, "c", "in the listed channel");
    tick(&harness, &read, &mut read_cursor).await;
    // Two events: a bare cursor for the channel off the list, then the listed channel's message.
    let text = read_events(&mut body, 2).await;
    assert_eq!(
        arrivals(&text),
        vec![(
            READ_CHANNEL.to_owned(),
            "in the listed channel".to_owned(),
            false
        )],
        "{text:?}"
    );
    assert_eq!(event_ids(&text).len(), 2, "{text:?}");
}

#[tokio::test]
async fn the_stream_of_every_channel_needs_a_token_and_marks_what_this_server_posted() {
    let harness = harness();
    let response = open(&harness, "/api/v1/live/stream", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let write = ChannelId(WRITE_CHANNEL.to_owned());
    let mut cursor = None;
    tick(&harness, &write, &mut cursor).await;
    let response = open(&harness, "/api/v1/live/stream", Some(READ_TOKEN), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|v| v.to_str().ok()),
        Some("no")
    );
    let mut body = response.into_body();
    read_events(&mut body, 1).await;
    let posted = harness
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/channels/{WRITE_CHANNEL}/reply"))
                .header("authorization", format!("Bearer {WRITE_TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"text": "landed it"}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("router responds");
    assert_eq!(posted.status(), StatusCode::OK);
    tick(&harness, &write, &mut cursor).await;
    let text = read_events(&mut body, 1).await;
    let payload = payloads(&text).pop().expect("the posted message");
    assert_eq!(payload["message"]["content"], "landed it");
    assert_eq!(
        payload["self_posted"], true,
        "the page must not relay the agent's own post back to it: {text:?}"
    );
}

// --- `#378 live-reactions` -----------------------------------------------------------------

fn thumbs(count: u32) -> Vec<vibe_talk::model::Reaction> {
    vec![vibe_talk::model::Reaction {
        emoji: "👍".to_owned(),
        custom: false,
        custom_id: None,
        count,
        owner_reacted: false,
    }]
}

/// A reaction added in the native app to a message already read reaches an attached page as a
/// `message_update` on the next poll tick of the watched channel — the owner's thumbs-up case.
/// The elapsed time from the native change to the frame is the latency this task reports: with
/// fakes it is one tick's read, so in production it is bounded by the poll interval (or by the
/// hint wake-up where a bridge sends one).
#[tokio::test]
async fn a_reaction_added_after_the_read_arrives_as_an_update() {
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    // No reactions at all when seeded: the owner's case is the FIRST reaction on a message.
    // The other message keeps a set throughout, so the refresh read always proves it reports
    // reactions — the gate under which an absent field may read as a removal below.
    let reporter = harness.discord.seed(&channel, "codex-eng", "looking");
    harness.discord.set_reactions(&reporter, Some(thumbs(1)));
    let id = harness.discord.seed(&channel, "codex-eng", "ship it");
    let mut cursor = None;
    assert_eq!(
        tick(&harness, &channel, &mut cursor).await,
        0,
        "seed publishes nothing"
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

    let changed_at = std::time::Instant::now();
    harness.discord.set_reactions(&id, Some(thumbs(1)));
    tick(&harness, &channel, &mut cursor).await;

    let text = read_events(&mut body, 1).await;
    let latency = changed_at.elapsed();
    assert!(
        text.contains("event: message_update"),
        "a reaction-only change must ride the stream as an update: {text:?}"
    );
    let payload = payloads(&text).pop().expect("one parsed event");
    assert_eq!(
        payload["message"]["reactions"][0]["emoji"], "👍",
        "{payload}"
    );
    assert_eq!(payload["message"]["reactions"][0]["count"], 1, "{payload}");
    assert!(
        latency < Duration::from_secs(5),
        "native reaction to stream frame took {latency:?} with fakes (one tick expected)"
    );
    eprintln!("live-reactions latency (fake, one tick): {latency:?}");

    // Removing the last reaction is an update too, with an empty list — not silence. On the
    // wire the field disappears entirely, which a read of a provider that reported a set before
    // reads as "none left".
    harness.discord.set_reactions(&id, None);
    tick(&harness, &channel, &mut cursor).await;
    let text = read_events(&mut body, 1).await;
    assert!(text.contains("event: message_update"), "{text:?}");
    let payload = payloads(&text).pop().expect("one parsed event");
    assert_eq!(
        payload["message"]["reactions"],
        serde_json::json!([]),
        "{payload}"
    );
}

/// A reaction pushed by an adapter as an update reaches the stream and the held window, so a
/// page that re-reads after the update is never served the older reaction set.
#[tokio::test]
async fn a_pushed_reaction_update_reaches_the_stream() {
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let id = harness.discord.seed(&channel, "codex-eng", "ship it");
    harness.discord.set_reactions(&id, Some(Vec::new()));
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;

    let mut subscription = harness.live.subscribe(&channel, None);
    let mut message = harness.discord.message(&id).expect("seeded");
    message.reactions = Some(thumbs(2));
    assert!(harness.live.publish_ingested(
        "reaction-1".to_owned(),
        vibe_talk::live::LiveKind::Update,
        false,
        &channel,
        message,
    ));
    let live = tokio::time::timeout(Duration::from_secs(5), subscription.receiver.recv())
        .await
        .expect("the update arrives")
        .expect("the channel is open");
    assert_eq!(live.kind, vibe_talk::live::LiveKind::Update);
    assert_eq!(live.message.reactions, Some(thumbs(2)));
}

/// The refresh window is never smaller than the page a reader has open: a reaction on a message
/// forty rows back — on screen, past any smaller window — still arrives.
#[tokio::test]
async fn a_reaction_deep_in_the_open_page_arrives() {
    let harness = harness();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let target = harness
        .discord
        .seed(&channel, "codex-eng", "deep in the page");
    for i in 0..44 {
        harness
            .discord
            .seed(&channel, "codex-eng", &format!("filler {i}"));
    }
    let mut cursor = None;
    tick(&harness, &channel, &mut cursor).await;
    let mut subscription = harness.live.subscribe(&channel, None);
    harness.discord.set_reactions(&target, Some(thumbs(3)));
    tick(&harness, &channel, &mut cursor).await;
    let live = tokio::time::timeout(Duration::from_secs(5), subscription.receiver.recv())
        .await
        .expect("the update arrives")
        .expect("the channel is open");
    assert_eq!(live.kind, vibe_talk::live::LiveKind::Update);
    assert_eq!(live.message.id, target);
    assert_eq!(live.message.reactions, Some(thumbs(3)));
}
