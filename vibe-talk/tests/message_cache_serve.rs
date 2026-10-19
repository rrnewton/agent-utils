//! `#384 cache-serve` (of `#208 persistent-cache`, milestone M9): reads answered from the
//! message cache at once, refreshed from the provider in the background.
//!
//! Each test fills a real cache file through the server's own routes with the write token,
//! turns serving on (`Sink::set_serve`, what `[message_cache] serve = true` sets at startup),
//! and then holds the provider's next read at the fake: a provider that has taken its answer
//! and does not hand it back is the slow provider of the owner's reload, without the wait.
//! A served read must answer while that read is still held; a read that may not be served
//! must still be waiting when the hold is released.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::shadow::ConsumerCounts;
use vibe_talk::cache::ChanKey;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::live::LiveKind;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, ManualClock, TempDir, TimelineChat, READ_CHANNEL, WRITE_TOKEN};

/// 2026-08-18T12:00:00Z: where the fixtures' messages are written, a minute apart.
const T0: i64 = 1_787_054_400_000;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const WAIT: Duration = Duration::from_secs(20);
/// A served answer's budget: the brief's one second, against a provider that never answers.
const AT_ONCE: Duration = Duration::from_secs(1);

/// The provider key a single-provider test deployment names its provider by.
const LEGACY: &str = vibe_talk::config::LEGACY_DISCORD_PROVIDER_KEY;

async fn call(state: &AppState, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
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

async fn ok(state: &AppState, uri: &str, token: &str) -> Value {
    let (status, body) = call(state, uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

fn path(channel: &str, rest: &str) -> String {
    format!("/api/v1/channels/{channel}{rest}")
}

/// Wait for every shadow evaluation to be counted, and for the cache's writer.
async fn settled(state: &AppState) {
    assert!(
        state.message_cache.shadow_settled(WAIT).await,
        "every evaluation was counted"
    );
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    }
}

/// What the shadow has counted for `consumer`.
fn counts(state: &AppState, consumer: &str) -> ConsumerCounts {
    state
        .message_cache
        .shadow_counts()
        .into_iter()
        .find(|counts| counts.consumer == consumer)
        .unwrap_or_else(|| panic!("no consumer {consumer}"))
}

/// The texts a timeline or window answer carries, oldest first.
fn texts(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| {
            message["content"]
                .as_str()
                .expect("content is text")
                .to_owned()
        })
        .collect()
}

/// A server over a [`TimelineChat`] whose read channel holds twelve messages a minute apart,
/// with a message cache behind it whose clock reads an hour after the first.
struct Deployment {
    state: AppState,
    fake: Arc<FakeDiscord>,
    chat: Arc<TimelineChat>,
    clock: Arc<ManualClock>,
    ids: Vec<MessageId>,
    _dir: TempDir,
}

async fn deployment(tag: &str) -> Deployment {
    let dir = TempDir::new(tag);
    let (mut state, fake, _voice) = testing::state_from_toml(&testing::config_toml());
    let read = ChannelId(READ_CHANNEL.to_owned());
    let ids = (0..12)
        .map(|n| fake.seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE))
        .collect();
    let chat = Arc::new(TimelineChat::new(fake.clone()));
    state.replace_chat(chat.clone());
    let clock = ManualClock::at(T0 + HOUR);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    Deployment {
        state,
        fake,
        chat,
        clock,
        ids,
        _dir: dir,
    }
}

impl Deployment {
    /// Fill the read channel's `main` from a write-token timeline read of its newest page.
    async fn fill_timeline(&self) {
        ok(
            &self.state,
            &path(READ_CHANNEL, "/timeline?view=main&limit=50"),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
    }

    /// Fill the read channel's `main` from a write-token window read.
    async fn fill_window(&self) {
        ok(
            &self.state,
            &path(READ_CHANNEL, "/messages?limit=50"),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
    }

    /// A write-token timeline read run on its own task, so a test can hold the provider and
    /// still see whether the request answers.
    fn spawn_timeline(&self, limit: u16) -> tokio::task::JoinHandle<Value> {
        let state = self.state.clone();
        tokio::spawn(async move {
            ok(
                &state,
                &path(READ_CHANNEL, &format!("/timeline?view=main&limit={limit}")),
                WRITE_TOKEN,
            )
            .await
        })
    }
}

#[tokio::test]
async fn a_reload_behind_a_provider_that_has_not_answered_shows_the_held_messages_at_once() {
    let d = deployment("cache-serve-reload").await;
    d.fill_timeline().await;
    // Hours later: a timeline page may still be served, as a provisional answer the refresh
    // then checks. Serving on, as `[message_cache] serve = true` sets it at startup.
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    // The provider takes its answer and does not hand it back: the owner's twenty seconds
    // and more, without the wait. The reload must not wait for it.
    let held = d.chat.hold_next_timeline();
    let began = std::time::Instant::now();
    let request = d.spawn_timeline(5);
    let body = tokio::time::timeout(AT_ONCE, request)
        .await
        .expect("the reload answered within a second, from the cache")
        .expect("the request ran");
    let elapsed = began.elapsed();
    eprintln!(
        "cache-serve measurement: reload to first messages behind a held provider: {} ms",
        elapsed.as_millis()
    );
    assert_eq!(
        texts(&body),
        [
            "message 7",
            "message 8",
            "message 9",
            "message 10",
            "message 11"
        ],
        "the newest five messages the cache holds"
    );
    assert!(
        body["as_of"].is_string(),
        "a served page says how far it is complete: {body}"
    );

    // The background refresh is the read being held; let it land and be checked.
    held.entered
        .await
        .expect("the refresh reached the provider");
    held.release.send(()).expect("the refresh is waiting");
    settled(&d.state).await;
    let timeline = counts(&d.state, "timeline_fallback");
    assert_eq!(timeline.served, 1, "{timeline:?}");
    assert!(
        timeline.compared >= 1 && timeline.agreed == timeline.compared,
        "the served answer was checked against upstream, and agreed: {timeline:?}"
    );
}

#[tokio::test]
async fn an_edit_and_a_deletion_the_refresh_finds_correct_what_was_served() {
    let d = deployment("cache-serve-correct").await;
    d.fill_timeline().await;
    // Upstream changes after the fill: message 10 edited, message 9 deleted. The cache has
    // heard of neither.
    d.fake.edit(&d.ids[10], "message 10, corrected");
    d.fake.delete(&d.ids[9]);
    d.clock.advance(HOUR);
    d.state.message_cache.set_serve(true);

    let channel = ChannelId(READ_CHANNEL.to_owned());
    let mut subscription = d.state.live.subscribe(&channel, None);
    while subscription.receiver.try_recv().is_ok() {}

    let held = d.chat.hold_next_timeline();
    let body = tokio::time::timeout(AT_ONCE, d.spawn_timeline(5))
        .await
        .expect("the reload answered at once")
        .expect("the request ran");
    assert_eq!(
        texts(&body),
        [
            "message 7",
            "message 8",
            "message 9",
            "message 10",
            "message 11"
        ],
        "the provisional page is the cache as it stood, edit and deletion unknown to it"
    );

    held.entered
        .await
        .expect("the refresh reached the provider");
    held.release.send(()).expect("the refresh is waiting");

    // The page is put right by live corrections for exactly the rows that were served.
    let (mut updated, mut deleted) = (None::<String>, None::<String>);
    let collect = async {
        while updated.is_none() || deleted.is_none() {
            let live = subscription
                .receiver
                .recv()
                .await
                .expect("a live correction");
            match live.kind {
                LiveKind::Update if live.message.id == d.ids[10] => {
                    updated = Some(live.message.content.clone());
                }
                LiveKind::Delete if live.message.id == d.ids[9] => {
                    deleted = Some(live.message.id.0.clone());
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), collect)
        .await
        .expect("the refresh corrected the served page");
    assert_eq!(updated.as_deref(), Some("message 10, corrected"));
    assert_eq!(deleted.as_deref(), Some(d.ids[9].as_str()));

    settled(&d.state).await;
    let timeline = counts(&d.state, "timeline_fallback");
    assert_eq!(timeline.served, 1, "{timeline:?}");
    assert!(
        timeline.extra_rows >= 1 && timeline.content_differs >= 1,
        "the comparison counted the deletion shown and the text corrected: {timeline:?}"
    );

    // And the next ordinary read is correct in itself. Serve nothing more: what upstream
    // answers now is what the refresh already brought in.
    let body = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=5"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        texts(&body),
        [
            "message 6",
            "message 7",
            "message 8",
            "message 10, corrected",
            "message 11"
        ]
    );
}

#[tokio::test]
async fn a_channel_in_doubt_is_not_served() {
    let d = deployment("cache-serve-doubt").await;
    d.fill_timeline().await;
    // A change for the channel was left out after its rows were seen: nothing held of it is
    // trusted until a later read sees it again, however complete it looks.
    d.clock.advance(SECOND);
    let cache = d.state.message_cache.cache().expect("installed");
    cache.mark_suspect(&ChanKey::new(LEGACY, READ_CHANNEL));
    d.state.message_cache.set_serve(true);

    let held = d.chat.hold_next_timeline();
    let mut request = d.spawn_timeline(5);
    assert!(
        tokio::time::timeout(AT_ONCE, &mut request).await.is_err(),
        "a channel in doubt waits for the provider, as today"
    );
    held.release.send(()).expect("the read is waiting");
    let body = request.await.expect("the request ran");
    assert_eq!(
        texts(&body),
        [
            "message 7",
            "message 8",
            "message 9",
            "message 10",
            "message 11"
        ],
        "answered by the provider, once it answered"
    );
    settled(&d.state).await;
    let timeline = counts(&d.state, "timeline_fallback");
    assert_eq!(timeline.served, 0, "{timeline:?}");
    assert!(timeline.misses.suspect >= 1, "{timeline:?}");
}

#[tokio::test]
async fn a_fresh_window_is_served_and_checked_against_upstream() {
    let d = deployment("cache-serve-window").await;
    d.fill_window().await;
    // Five seconds on: a window's head is still fresh, so the window may be answered as it
    // stands. The refresh behind it is the read held here.
    d.clock.advance(5 * SECOND);
    d.state.message_cache.set_serve(true);

    let held = d.chat.hold_next_recent();
    let state = d.state.clone();
    let request = tokio::spawn(async move {
        ok(
            &state,
            &path(READ_CHANNEL, "/messages?limit=5"),
            WRITE_TOKEN,
        )
        .await
    });
    let body = tokio::time::timeout(AT_ONCE, request)
        .await
        .expect("the window answered within a second, from the cache")
        .expect("the request ran");
    assert_eq!(
        texts(&body),
        [
            "message 7",
            "message 8",
            "message 9",
            "message 10",
            "message 11"
        ]
    );

    held.entered
        .await
        .expect("the refresh reached the provider");
    held.release.send(()).expect("the refresh is waiting");
    settled(&d.state).await;
    let messages = counts(&d.state, "messages");
    assert_eq!(messages.served, 1, "{messages:?}");
    assert!(
        messages.compared >= 1 && messages.agreed >= 1,
        "the served window was checked against upstream: {messages:?}"
    );
}

#[tokio::test]
async fn with_serving_off_a_reload_waits_for_the_provider() {
    // The shipped default, and the measurement's before: serving is not turned on, so a
    // reload behind a provider that has not answered is still waiting a second later.
    let d = deployment("cache-serve-off").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);

    let held = d.chat.hold_next_timeline();
    let began = std::time::Instant::now();
    let mut request = d.spawn_timeline(5);
    assert!(
        tokio::time::timeout(AT_ONCE, &mut request).await.is_err(),
        "with serving off, nothing answers until the provider does"
    );
    eprintln!(
        "cache-serve measurement: reload with serving off still unanswered after {} ms \
         (provider held)",
        began.elapsed().as_millis()
    );
    held.release.send(()).expect("the read is waiting");
    let body = request.await.expect("the request ran");
    assert_eq!(
        texts(&body),
        [
            "message 7",
            "message 8",
            "message 9",
            "message 10",
            "message 11"
        ]
    );
    settled(&d.state).await;
    assert_eq!(counts(&d.state, "timeline_fallback").served, 0);
}
