//! `#384 cache-serve` (of `#208 persistent-cache`, milestone M9): reads answered from the
//! message cache at once, refreshed from the provider in the background.
//!
//! Each test fills a real cache file through the server's own routes with the write token,
//! turns serving on (`Sink::set_serve`, what `[message_cache] serve = true` sets at startup),
//! and then holds the provider's next read at the fake: a provider that has taken its answer
//! and does not hand it back is the slow provider of the owner's reload, without the wait.
//! A served read must answer while that read is still held; a read that may not be served
//! must still be waiting when the hold is released.

use crate::slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::shadow::ConsumerCounts;
use vibe_talk::cache::sink::Verification;
use vibe_talk::cache::{ChanKey, RefreshKey};
use vibe_talk::chat::ChatError;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::live::LiveKind;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{
    self, CountingChat, ManualClock, TempDir, TimelineChat, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN,
};
use vibe_talk::threads::TimelineView;

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
        self.spawn_timeline_as(limit, WRITE_TOKEN)
    }

    fn spawn_timeline_as(&self, limit: u16, token: &str) -> tokio::task::JoinHandle<Value> {
        let state = self.state.clone();
        let token = token.to_owned();
        tokio::spawn(async move {
            ok(
                &state,
                &path(READ_CHANNEL, &format!("/timeline?view=main&limit={limit}")),
                &token,
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
    // `#502 cache-serve-coverage`: the reader's wait is timed as answered from the cache, and
    // the refresh behind it, which no reader waited for, is not timed at all: the one read timed
    // upstream is the fill's, before serving was on.
    let latency = timeline.latency;
    assert_eq!(
        (latency.cache.n, latency.upstream.n, latency.would.n),
        (1, 1, 0),
        "{latency:?}"
    );
    assert!(
        latency.cache.p50_us <= u64::try_from(elapsed.as_micros()).expect("small"),
        "timed from the ask to the cache's answer, within the request: {latency:?} {elapsed:?}"
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
    let mut event_ids: Vec<String> = Vec::new();
    let collect = async {
        while updated.is_none() || deleted.is_none() {
            let live = subscription
                .receiver
                .recv()
                .await
                .expect("a live correction");
            event_ids.push(live.event_id.clone());
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
    // A subscriber resumes after the LAST event of the id it names, so a correction may never
    // carry another event's id — its message's create id above all.
    assert!(
        event_ids.iter().all(|event_id| event_id.contains("~c")),
        "correction event ids are their own: {event_ids:?}"
    );
    let mut sorted = event_ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), event_ids.len(), "and no two share one");
    let resumed = d
        .state
        .live
        .subscribe(&channel, Some(&MessageId(event_ids[0].clone())));
    assert!(
        resumed
            .replay
            .iter()
            .any(|live| live.event_id == event_ids[1]),
        "resuming after the first correction replays the second: {:?}",
        resumed
            .replay
            .iter()
            .map(|live| &live.event_id)
            .collect::<Vec<_>>()
    );

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
    let timeline = counts(&d.state, "timeline_fallback");
    assert_eq!(timeline.served, 0);
    // `#502 cache-serve-coverage`: the reader waited for upstream, held past a second (the other
    // read timed upstream is the fill's); the cache would have had its answer far sooner, and
    // says how soon.
    let latency = timeline.latency;
    assert_eq!(
        (latency.cache.n, latency.upstream.n, latency.would.n),
        (0, 2, 1),
        "{latency:?}"
    );
    assert!(
        // Within the histogram's resolution, an eighth of a power of two.
        latency.upstream.p99_us >= u64::try_from(AT_ONCE.as_micros()).expect("small") * 7 / 8
            && latency.would.p99_us < latency.upstream.p99_us,
        "{latency:?}"
    );
}

#[tokio::test]
async fn a_read_token_reload_is_never_served() {
    // Serving is the write scope's: a served read's refresh fills under the caller's grant,
    // and the read token has none. Its reads are shadowed, as before, and answered upstream.
    let d = deployment("cache-serve-read-scope").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    let held = d.chat.hold_next_timeline();
    let mut request = d.spawn_timeline_as(5, READ_TOKEN);
    assert!(
        tokio::time::timeout(AT_ONCE, &mut request).await.is_err(),
        "a read-token reload waits for the provider"
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

impl Deployment {
    /// One reload served from the cache while the provider is held, its refresh then released
    /// and settled: the served body.
    async fn served_reload(&self, limit: u16) -> Value {
        let held = self.chat.hold_next_timeline();
        let body = tokio::time::timeout(AT_ONCE, self.spawn_timeline(limit))
            .await
            .expect("the reload answered at once, from the cache")
            .expect("the request ran");
        held.entered
            .await
            .expect("the refresh reached the provider");
        held.release.send(()).expect("the refresh is waiting");
        settled(&self.state).await;
        self.refreshed(limit).await;
        body
    }

    /// [`refreshed`] for the read channel.
    async fn refreshed(&self, limit: u16) {
        refreshed(&self.state, READ_CHANNEL, limit).await;
    }

    /// A write-token step back in `main` from `cursor`.
    async fn step_back(&self, limit: u16, cursor: &str) -> Value {
        let body = ok(
            &self.state,
            &path(
                READ_CHANNEL,
                &format!(
                    "/timeline?view=main&limit={limit}&before={}",
                    url_encode(cursor)
                ),
            ),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
        body
    }
}

/// Wait until no refresh of `channel`'s newest `main` page of `limit` is running: its read
/// offered, its corrections published and its claim given up, so the next serve starts a refresh
/// of its own and reads what this one wrote.
async fn refreshed(state: &AppState, channel: &str, limit: u16) {
    let key = || {
        RefreshKey::timeline(
            ChannelId(channel.to_owned()),
            TimelineView::Main,
            None,
            limit,
        )
    };
    tokio::time::timeout(WAIT, async {
        while state.message_cache.claim_refresh(key()).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the refresh finished");
    settled(state).await;
}

/// The backward cursor a served page carries, which only this server mints.
fn served_cursor(body: &Value) -> String {
    let cursor = body["next_before"]
        .as_str()
        .unwrap_or_else(|| panic!("a served page carries a backward cursor: {body}"))
        .to_owned();
    assert!(
        cursor.starts_with(vibe_talk::timeline_served::PREFIX),
        "the server's own cursor, not a provider's: {body}"
    );
    cursor
}

#[tokio::test]
async fn back_to_back_reloads_are_each_served_and_a_step_back_still_reaches_older_history() {
    // `#510 cache-serve-every-read`: a served page used to leave its view owing the provider's
    // own page, so every other reload went upstream. Now each reload the rule allows is served,
    // and the served page's own cursor walks back into the provider's history.
    let d = deployment("cache-serve-cursor").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    let mut cursor = String::new();
    for round in 1..=3_u64 {
        // Time passes between reloads, as it would: a snapshot holding what the refresh just
        // wrote, at the very instant of the ask, is counted late and never served on.
        d.clock.advance(SECOND);
        let body = d.served_reload(5).await;
        assert_eq!(
            texts(&body),
            [
                "message 7",
                "message 8",
                "message 9",
                "message 10",
                "message 11"
            ],
            "reload {round}"
        );
        assert_eq!(body["has_more"], true, "{body}");
        assert_eq!(body["has_threads"], false, "{body}");
        cursor = served_cursor(&body);
        assert_eq!(
            counts(&d.state, "timeline_fallback").served,
            round,
            "reload {round} was served"
        );
    }

    // The forward cursor a served page hands out is one a delta can continue from.
    d.clock.advance(SECOND);
    let held = d.chat.hold_next_timeline();
    let body = tokio::time::timeout(AT_ONCE, d.spawn_timeline(5))
        .await
        .expect("served")
        .expect("the request ran");
    held.release.send(()).expect("the refresh is waiting");
    d.refreshed(5).await;
    let after = body["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    let delta = ok(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=5&after={}", url_encode(&after)),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert!(delta["delta"].is_object(), "a delta answer: {delta}");
    settled(&d.state).await;

    // After those serves, the first one's cursor still steps back: the provider's own newest
    // page holds the served rows and nothing older, so the step goes on by its cursor and
    // answers the five older messages, with the provider's own cursor for the next.
    let older = d.step_back(5, &cursor).await;
    assert_eq!(
        texts(&older),
        [
            "message 2",
            "message 3",
            "message 4",
            "message 5",
            "message 6"
        ],
        "{older}"
    );
    assert_eq!(older["has_more"], true, "{older}");
    assert!(
        older["next_after"].is_null(),
        "a step back says nothing of the newest end"
    );
    let next = older["next_before"]
        .as_str()
        .expect("the provider's own backward cursor")
        .to_owned();
    assert!(
        !next.starts_with(vibe_talk::timeline_served::PREFIX),
        "every step after the first is the provider's: {older}"
    );
    let oldest = d.step_back(5, &next).await;
    assert_eq!(texts(&oldest), ["message 0", "message 1"], "{oldest}");
    assert_eq!(oldest["has_more"], false, "{oldest}");

    // And the step back held nothing back either: the next reload is served too.
    d.clock.advance(SECOND);
    let body = d.served_reload(5).await;
    served_cursor(&body);
    assert_eq!(counts(&d.state, "timeline_fallback").served, 5);
}

#[tokio::test]
async fn a_step_back_from_a_served_page_walks_past_what_arrived_since() {
    // More messages arrived after the serve than one page holds: the provider's newest page no
    // longer reaches the served rows, and the step walks back on its cursors until it does,
    // answering only what lies below the oldest message served.
    let d = deployment("cache-serve-walk").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let body = d.served_reload(5).await;
    let cursor = served_cursor(&body);

    let read = ChannelId(READ_CHANNEL.to_owned());
    for n in 12..19 {
        d.fake
            .seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE);
    }
    let older = d.step_back(5, &cursor).await;
    assert_eq!(
        texts(&older),
        ["message 4", "message 5", "message 6"],
        "only what lies below the served page: {older}"
    );
    assert_eq!(older["has_more"], true, "{older}");
    let next = older["next_before"]
        .as_str()
        .expect("the provider's own backward cursor")
        .to_owned();
    let below = d.step_back(5, &next).await;
    assert_eq!(
        texts(&below),
        ["message 0", "message 1", "message 2", "message 3"],
        "{below}"
    );
}

#[tokio::test]
async fn a_walk_that_spends_its_pages_resumes_and_never_hands_back_a_newer_message() {
    // Twenty-five messages arrived after a page of five was served: four provider pages hold
    // none of what lies below it. The step answers nothing yet, with a served cursor that goes
    // on from where it stopped, not the provider's bare one, whose pages would carry the
    // twenty-five newer messages up above the ones on the page.
    let d = deployment("cache-serve-walk-cap").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let body = d.served_reload(5).await;
    let cursor = served_cursor(&body);

    let read = ChannelId(READ_CHANNEL.to_owned());
    for n in 12..37 {
        d.fake
            .seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE);
    }
    let first = d.step_back(5, &cursor).await;
    assert!(texts(&first).is_empty(), "nothing below yet: {first}");
    assert_eq!(first["has_more"], true, "{first}");
    let resumed = served_cursor(&first);
    assert_ne!(resumed, cursor, "it goes on from where the walk stopped");

    let older = d.step_back(5, &resumed).await;
    assert_eq!(
        texts(&older),
        [
            "message 2",
            "message 3",
            "message 4",
            "message 5",
            "message 6"
        ],
        "only what lies below the served page: {older}"
    );
    let next = older["next_before"]
        .as_str()
        .expect("the provider's own backward cursor")
        .to_owned();
    assert!(
        !next.starts_with(vibe_talk::timeline_served::PREFIX),
        "{older}"
    );
    assert_eq!(
        texts(&d.step_back(5, &next).await),
        ["message 0", "message 1"]
    );
}

#[tokio::test]
async fn a_reload_while_the_last_serves_refresh_runs_is_served_and_shares_that_refresh() {
    // Every served page is refreshed, to compare and correct it. While one refresh is still
    // reading the provider, a reload of the same page is served too, and starts no second read:
    // the running refresh corrects the page by live event, which reaches every reader of the
    // channel. `#611 timeline-read-failures`: it used to go upstream beside the refresh, and a
    // provider slow enough to hold the refresh failed it.
    let d = deployment("cache-serve-overlap").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    let held = d.chat.hold_next_timeline();
    let body = tokio::time::timeout(AT_ONCE, d.spawn_timeline(5))
        .await
        .expect("served at once")
        .expect("the request ran");
    served_cursor(&body);
    held.entered
        .await
        .expect("the refresh reached the provider");
    let reads = d.chat.timeline_reads();

    d.clock.advance(SECOND);
    let again = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=5"),
        WRITE_TOKEN,
    )
    .await;
    served_cursor(&again);
    assert_eq!(d.chat.timeline_reads(), reads, "no second read upstream");
    held.release.send(()).expect("the refresh is waiting");
    d.refreshed(5).await;
    let timeline = counts(&d.state, "timeline_fallback");
    assert_eq!(timeline.served, 2, "{timeline:?}");

    // With that refresh done, the next reload is served again, with a refresh of its own.
    d.clock.advance(SECOND);
    served_cursor(&d.served_reload(5).await);
    assert_eq!(counts(&d.state, "timeline_fallback").served, 3);
}

#[tokio::test]
async fn a_served_cursor_is_refused_for_any_other_view() {
    let d = deployment("cache-serve-cursor-bound").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let body = d.served_reload(5).await;
    let cursor = served_cursor(&body);
    let (status, refused) = call(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=flat&limit=5&before={}", url_encode(&cursor)),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
}

#[tokio::test]
async fn a_second_serve_in_a_row_is_corrected_by_its_own_refresh() {
    // Serving back to back keeps every correction: what changed upstream after the first
    // serve's refresh is shown by the second, provisionally, and put right by its refresh.
    let d = deployment("cache-serve-correct-again").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    d.served_reload(5).await;

    d.fake.edit(&d.ids[10], "message 10, corrected");
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let mut subscription = d.state.live.subscribe(&channel, None);
    while subscription.receiver.try_recv().is_ok() {}
    d.clock.advance(SECOND);
    let body = d.served_reload(5).await;
    assert_eq!(
        texts(&body)[3],
        "message 10",
        "the provisional page is the cache as it stood"
    );
    // Only the row that changed is corrected: a row merely seen again by a refresh is no
    // change, though its served copy carries a version the held copy keeps beside it.
    let corrected = async {
        let mut seen = Vec::new();
        loop {
            let live = subscription
                .receiver
                .recv()
                .await
                .expect("a live correction");
            seen.push((
                live.kind,
                live.message.id.clone(),
                live.message.content.clone(),
            ));
            if live.kind == LiveKind::Update && live.message.id == d.ids[10] {
                return seen;
            }
        }
    };
    let seen = tokio::time::timeout(Duration::from_secs(10), corrected)
        .await
        .expect("the second serve's refresh corrected it");
    assert_eq!(
        seen,
        [(
            LiveKind::Update,
            d.ids[10].clone(),
            "message 10, corrected".to_owned()
        )]
    );
    assert_eq!(counts(&d.state, "timeline_fallback").served, 2);
}

/// Percent-encode a cursor for a query string: enough for the envelope's own characters.
fn url_encode(raw: &str) -> String {
    raw.chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => vec![c],
            _ => format!("%{:02X}", u32::from(c)).chars().collect(),
        })
        .collect()
}

#[tokio::test]
async fn a_flat_page_off_a_bridge_is_never_served() {
    // Only a bridge's Flat list is a scope the cache can vouch for whole; Discord's own Flat
    // view is never answered from it, however fresh its Main scope is.
    let d = deployment("cache-serve-flat").await;
    d.fill_timeline().await;
    d.state.message_cache.set_serve(true);

    let held = d.chat.hold_next_timeline();
    let state = d.state.clone();
    let mut request = tokio::spawn(async move {
        ok(
            &state,
            &path(READ_CHANNEL, "/timeline?view=flat&limit=5"),
            WRITE_TOKEN,
        )
        .await
    });
    assert!(
        tokio::time::timeout(AT_ONCE, &mut request).await.is_err(),
        "a Flat page here waits for the provider"
    );
    held.release.send(()).expect("the read is waiting");
    request.await.expect("the request ran");
    settled(&d.state).await;
    assert_eq!(counts(&d.state, "timeline_fallback").served, 0);
}

#[tokio::test]
async fn a_provider_whose_account_is_unchecked_is_not_served() {
    // Rows read as one account must never be handed back as another's: between a credential
    // swap and the new account being checked against the file, every read goes upstream,
    // however well the cache would otherwise serve it.
    let dir = TempDir::new("cache-serve-unverified");
    let (mut state, fake, _voice) = testing::state_from_toml(&testing::config_toml());
    let read = ChannelId(READ_CHANNEL.to_owned());
    for n in 0..12 {
        fake.seed_at(&read, "Ada", &format!("message {n}"), T0 + n * MINUTE);
    }
    let chat = Arc::new(TimelineChat::new(fake));
    let counting = Arc::new(CountingChat::new(chat.clone()));
    state.replace_chat(counting.clone());
    let clock = ManualClock::at(T0 + HOUR);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    ok(
        &state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(&state).await;
    clock.advance(2 * HOUR);
    state.message_cache.set_serve(true);

    // The provider stops answering which account it is, and the check starts over.
    for _ in 0..8 {
        counting.fail_next(
            "identity",
            None,
            ChatError::Transport("the identity read is down".to_owned()),
        );
    }
    state.message_cache.reverify();
    assert_ne!(
        state.message_cache.verification(LEGACY),
        Verification::Verified,
        "unchecked again from this start of the check"
    );

    let held = chat.hold_next_timeline();
    let request_state = state.clone();
    let mut request = tokio::spawn(async move {
        ok(
            &request_state,
            &path(READ_CHANNEL, "/timeline?view=main&limit=5"),
            WRITE_TOKEN,
        )
        .await
    });
    assert!(
        tokio::time::timeout(AT_ONCE, &mut request).await.is_err(),
        "unchecked, the reload waits for the provider"
    );
    held.release.send(()).expect("the read is waiting");
    request.await.expect("the request ran");
    settled(&state).await;
    assert_eq!(counts(&state, "timeline_fallback").served, 0);
}

const SLACK: &str = "slack";
const SLACK_CH: &str = "C0000MAIN1";

fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1_000
}

/// A Slack channel of six messages a minute apart, filled into a cache read by `clock`.
async fn slack_deployment(tag: &str) -> (AppState, slack_fake::Fake, Arc<ManualClock>, TempDir) {
    let mut fixture = slack_fake::FakeState::default();
    fixture.history.insert(
        SLACK_CH.to_owned(),
        (0..6)
            .map(|n| {
                slack_fake::profiled(
                    &slack_fake::ts(n * 60, 0),
                    "UADA00001",
                    "Ada",
                    &format!("message {n}"),
                )
            })
            .collect(),
    );
    let fake = slack_fake::Fake::start(fixture, 10).await;
    let dir = TempDir::new(tag);
    let clock = ManualClock::at(slack_ms(3_600));
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{SLACK}"
kind = "slack"
api_base = "{base}"
token = "{token}"

[[channels]]
id = "{SLACK_CH}"
label = "team"
writable = true
"#,
        base = fake.base,
        token = slack_fake::TOKEN,
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(&state).await;
    (state, fake, clock, dir)
}

/// Rewrite one held message's text upstream, in the fake's history: the cache hears nothing.
fn slack_edit(fake: &slack_fake::Fake, at_seconds: i64, text: &str) {
    let mut state = fake.state.lock().expect("the fake's state");
    let history = state.history.get_mut(SLACK_CH).expect("the channel");
    let message = history
        .iter_mut()
        .find(|message| message["ts"] == slack_fake::ts(at_seconds, 0))
        .expect("the message");
    message["text"] = Value::String(text.to_owned());
}

#[tokio::test]
async fn a_slack_window_is_never_served_and_a_slack_main_page_is() {
    // Requirement (c) of #297 cache-coverage-races: a Slack window upstream carries thread
    // replies the cache keeps in their threads alone, so it is not served at all in this
    // build, while a Main timeline page is the conversation's own history and may be.
    let (state, fake, clock, _dir) = slack_deployment("cache-serve-slack").await;
    state.message_cache.set_serve(true);

    // The window, five seconds after the fill — fresh enough to serve on any other shape.
    // An edit upstream since the fill settles which answer came back.
    clock.advance(5 * SECOND);
    slack_edit(&fake, 300, "message 5, edited upstream");
    let body = ok(&state, &path(SLACK_CH, "/messages?limit=2"), WRITE_TOKEN).await;
    assert_eq!(
        texts(&body),
        ["message 4", "message 5, edited upstream"],
        "the window was the provider's own answer"
    );
    settled(&state).await;
    assert_eq!(counts(&state, "messages").served, 0);

    // The Main page, against an edit the cache has not heard of: provisional first, and the
    // refresh behind it brings the correction, which the next reload, served again, shows.
    clock.advance(HOUR);
    slack_edit(&fake, 240, "message 4, edited upstream");
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=main&limit=2"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        texts(&body),
        ["message 4", "message 5, edited upstream"],
        "the Main page as the cache holds it, before its refresh lands"
    );
    refreshed(&state, SLACK_CH, 2).await;
    assert_eq!(counts(&state, "timeline_fallback").served, 1);
    clock.advance(SECOND);
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=main&limit=2"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        texts(&body),
        ["message 4, edited upstream", "message 5, edited upstream"]
    );
    settled(&state).await;
    assert_eq!(
        counts(&state, "timeline_fallback").served,
        2,
        "`#510 cache-serve-every-read`: served again, back to back"
    );

    // Its own cursor steps back past the provider's newest page into Slack's history.
    let cursor = served_cursor(&body);
    let older = ok(
        &state,
        &path(
            SLACK_CH,
            &format!("/timeline?view=main&limit=2&before={}", url_encode(&cursor)),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(texts(&older), ["message 2", "message 3"], "{older}");
    assert_eq!(older["has_more"], true, "{older}");
    assert!(
        older["next_before"]
            .as_str()
            .is_some_and(|next| !next.starts_with(vibe_talk::timeline_served::PREFIX)),
        "Slack's own cursor for the next step: {older}"
    );
}

// --- `#605 server-side-search`: the server's own copy, searched ---------------------------------

fn search_path(channel: &str, query: &str) -> String {
    path(channel, &format!("/search?q={}&tz=UTC", url_encode(query)))
}

fn hit_texts(body: &Value) -> Vec<String> {
    body["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|m| m["content"].as_str().expect("content").to_owned())
        .collect()
}

#[tokio::test]
async fn a_search_of_the_servers_copy_finds_what_it_holds_newest_first_and_pages_on() {
    let d = deployment("cache-search-hits").await;
    d.fill_timeline().await;
    d.state.message_cache.set_serve(true);

    let body = ok(
        &d.state,
        &search_path(READ_CHANNEL, "MESSAGE 1"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        hit_texts(&body),
        ["message 11", "message 10", "message 1"],
        "case-insensitive substrings, newest first: {body}"
    );
    assert_eq!(body["upstream"], "none");
    let scope = &body["scopes"][0];
    assert_eq!(scope["state"], "searched", "{scope}");
    assert_eq!(scope["channel"]["id"], READ_CHANNEL);
    assert_eq!(scope["examined"], 12, "{scope}");
    assert!(
        scope["resume_below_ms"].is_null(),
        "the whole copy was searched"
    );
    assert!(body["resume_below_ms"].is_null());
    let timeline = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    let read = timeline["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["id"] == body["hits"][0]["id"])
        .expect("the timeline holds the hit");
    for field in ["content_html", "spoken_time", "spoken_content", "noise"] {
        assert_eq!(
            body["hits"][0][field], read[field],
            "{field}: rendered as a timeline's messages are"
        );
    }
    assert!(
        body["hits"][0]["observed_ms"].is_i64(),
        "a copy says when it was read"
    );

    // The language: OR, a phrase, and a negation.
    let body = ok(
        &d.state,
        &search_path(READ_CHANNEL, "\"message 2\" OR \"message 3\" -ada"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        hit_texts(&body),
        Vec::<String>::new(),
        "every author is Ada"
    );
    let body = ok(
        &d.state,
        &search_path(READ_CHANNEL, "\"message 2\" OR \"message 3\""),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(hit_texts(&body), ["message 3", "message 2"]);

    // A limit cuts on a whole millisecond and says where to resume; the next answer, below it,
    // goes on with nothing skipped and nothing repeated.
    let first = ok(
        &d.state,
        &format!("{}&limit=2", search_path(READ_CHANNEL, "message")),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(hit_texts(&first), ["message 11", "message 10"]);
    let resume = first["resume_below_ms"].as_i64().expect("more to search");
    let next = ok(
        &d.state,
        &format!(
            "{}&limit=2&below={resume}",
            search_path(READ_CHANNEL, "message")
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(hit_texts(&next), ["message 9", "message 8"]);
}

#[tokio::test]
async fn a_search_of_the_servers_copy_keeps_the_serving_rules_and_says_which_refused() {
    let d = deployment("cache-search-gates").await;
    d.fill_timeline().await;
    let reason = |body: &Value| body["scopes"][0]["reason"].as_str().map(str::to_owned);

    // Serving off: the operator has not let the server answer from its copy.
    let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
    assert_eq!(reason(&body).as_deref(), Some("serve_off"), "{body}");
    assert_eq!(body["scopes"][0]["state"], "not_searched");
    assert!(hit_texts(&body).is_empty());

    d.state.message_cache.set_serve(true);
    // The read token is never answered from the copy.
    let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), READ_TOKEN).await;
    assert_eq!(reason(&body).as_deref(), Some("read_scope"), "{body}");
    assert!(hit_texts(&body).is_empty());

    // A channel in doubt is not searched.
    let cache = d.state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);
    cache.block_channel(&chan);
    let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
    assert_eq!(reason(&body).as_deref(), Some("in_doubt"), "{body}");
    assert!(hit_texts(&body).is_empty());
    cache.unblock(&chan, None);
    let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
    assert_eq!(
        hit_texts(&body).len(),
        12,
        "searched again once the doubt is gone"
    );

    // An offer for the channel dropped: what was seen before it may be an edit or a deletion
    // behind, so it is withheld until a read sees it again.
    d.clock.advance(SECOND);
    cache.mark_suspect(&chan);
    let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
    assert!(hit_texts(&body).is_empty(), "{body}");
    assert_eq!(body["scopes"][0]["withheld"], 12, "{body}");
    d.clock.advance(SECOND);
    d.fill_timeline().await;
    let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
    assert_eq!(
        hit_texts(&body).len(),
        12,
        "a read since the drop vouches for them again: {body}"
    );

    // An account not checked against the file: nothing of it is handed out.
    d.state.message_cache.reverify();
    if d.state.message_cache.verification(LEGACY) != Verification::Verified {
        let body = ok(&d.state, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
        assert_eq!(reason(&body).as_deref(), Some("unverified"), "{body}");
    }

    // What is not a search, and what is not a channel here.
    let (status, _) = call(&d.state, &search_path(READ_CHANNEL, "  "), WRITE_TOKEN).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a query with no term");
    let (status, _) = call(&d.state, &search_path("9999999999", "message"), WRITE_TOKEN).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No cache at all.
    let (bare, _fake, _voice) = testing::state_from_toml(&testing::config_toml());
    let body = ok(&bare, &search_path(READ_CHANNEL, "message"), WRITE_TOKEN).await;
    assert_eq!(reason(&body).as_deref(), Some("no_cache"), "{body}");
}

#[tokio::test]
async fn all_channels_searches_every_channel_the_server_answers_for() {
    let d = deployment("cache-search-all").await;
    d.fill_timeline().await;
    d.state.message_cache.set_serve(true);
    let body = ok(&d.state, "/api/v1/search?q=message%205&tz=UTC", WRITE_TOKEN).await;
    assert_eq!(hit_texts(&body), ["message 5"], "{body}");
    assert_eq!(body["hits"][0]["channel_id"], READ_CHANNEL);
    let scopes = body["scopes"].as_array().expect("scopes");
    assert!(
        scopes.len() >= 2,
        "every configured channel has a scope: {body}"
    );
    assert!(scopes
        .iter()
        .any(|scope| scope["channel"]["id"] == READ_CHANNEL && scope["state"] == "searched"));
}

#[tokio::test]
async fn a_slack_search_finds_replies_in_their_threads_and_withholds_a_thread_in_doubt() {
    let (state, fake, clock, _dir) = slack_deployment("cache-search-slack").await;
    // Message 2 becomes the root of a thread with two replies, read in the thread view.
    let root = slack_fake::ts(120, 0);
    {
        let mut fixture = fake.state.lock().expect("the fake's state");
        let mut message = fixture.history[SLACK_CH][2].clone();
        let replies = [
            (slack_fake::ts(130, 0), "a heron by the river".to_owned()),
            (slack_fake::ts(140, 0), "and a kingfisher".to_owned()),
        ];
        slack_fake::thread(&mut fixture, SLACK_CH, &mut message, &replies);
        fixture.history.get_mut(SLACK_CH).expect("the channel")[2] = message;
    }
    clock.advance(SECOND);
    ok(
        &state,
        &path(
            SLACK_CH,
            &format!("/timeline?view=thread&thread_id={root}&limit=50"),
        ),
        WRITE_TOKEN,
    )
    .await;
    settled(&state).await;
    state.message_cache.set_serve(true);

    let body = ok(&state, &search_path(SLACK_CH, "HERON"), WRITE_TOKEN).await;
    assert_eq!(hit_texts(&body), ["a heron by the river"], "{body}");
    assert_eq!(body["hits"][0]["thread"]["id"], root.as_str());
    let scope = &body["scopes"][0];
    assert_eq!(
        scope["top_level_only"], true,
        "replies are not covered by Main's claims"
    );
    assert_eq!(scope["withheld"], 0);

    // The thread in doubt: its replies are not handed out, and the scope says how many.
    let cache = state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(SLACK, SLACK_CH);
    let thread = vibe_talk::cache::Scope::Thread(root.clone());
    cache.block_scope(&chan, &thread);
    let body = ok(&state, &search_path(SLACK_CH, "heron"), WRITE_TOKEN).await;
    assert!(hit_texts(&body).is_empty(), "{body}");
    assert_eq!(body["scopes"][0]["withheld"], 1, "{body}");
    assert_eq!(body["scopes"][0]["state"], "searched");
    cache.unblock(&chan, Some(&thread));
}

// --- `#611 timeline-read-failures`: the cache stands in for a provider that fails or is slow ----

/// A rate limit that did not clear in its budget, as the Slack client reports one.
fn rate_limited() -> ChatError {
    ChatError::RateLimited(vibe_talk::chat::RateLimitExhausted {
        provider: "slack",
        route: "conversations.history".to_owned(),
        attempts: 5,
        waited: Duration::from_secs(5),
        budget: Duration::from_secs(30),
        retry_after: Duration::from_secs(1),
        global: false,
    })
}

/// The forward cursor a newest read of the read channel's Main hands back.
async fn forward_cursor(d: &Deployment) -> String {
    let body = ok(
        &d.state,
        &path(READ_CHANNEL, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(&d.state).await;
    body["next_after"]
        .as_str()
        .expect("a newest page hands back a forward cursor")
        .to_owned()
}

#[tokio::test]
async fn a_read_the_provider_fails_is_answered_from_the_cache_and_says_it_could_not_refresh() {
    let d = deployment("stand-in-failed").await;
    let after = forward_cursor(&d).await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    d.chat.fail_timelines(Some(rate_limited()));
    let reads = d.chat.timeline_reads();

    // A poll's catch-up read, which the cache never serves: it used to be a 502.
    let capture = vibe_talk::testing::LogCapture::start();
    let body = ok(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=50&after={after}&reason=poll"),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(d.chat.timeline_reads(), reads + 1, "one read went upstream");
    assert_eq!(
        texts(&body).len(),
        12,
        "every message the cache holds of Main: {body}"
    );
    assert!(
        body.get("delta").is_none(),
        "a newest page, not a delta: {body}"
    );
    assert!(
        body["refresh_failed"]
            .as_str()
            .is_some_and(|why| why.contains("limiting")),
        "the page is told the provider could not be read, and why: {body}"
    );
    assert!(body["as_of"].is_string(), "and how old the copy is: {body}");
    assert!(
        body.get("settled_below_ms").is_none(),
        "a stand-in proves nothing by what it lacks: {body}"
    );
    assert!(
        body["next_after"].is_string() && body["next_before"].is_string(),
        "it can be continued either way: {body}"
    );
    let text = capture.text();
    assert!(
        text.contains("timeline_stand_in") && text.contains("upstream=\"failed\""),
        "the stand-in is logged: {text}"
    );
    assert!(
        !text.contains("server error"),
        "no 5xx was answered: {text}"
    );
}

#[tokio::test]
async fn a_read_the_cache_cannot_answer_fails_and_the_log_says_why() {
    let d = deployment("stand-in-off").await;
    let after = forward_cursor(&d).await;
    // Serving off: the cache answers nothing, so the failure is the answer, said at WARN.
    d.chat.fail_timelines(Some(rate_limited()));
    let capture = vibe_talk::testing::LogCapture::start();
    let (status, body) = call(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=50&after={after}&reason=side"),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"], "chat_error", "{body}");
    let text = capture.text();
    let line = text
        .lines()
        .find(|line| line.contains("server error"))
        .unwrap_or_else(|| panic!("one server-error line: {text}"));
    assert_eq!(
        text.matches("server error").count(),
        1,
        "once per request: {text}"
    );
    for wanted in [
        "WARN",
        "status=502",
        "read_reason=\"side\"",
        "error=\"chat_error\"",
        "conversations.history",
        "upstream_operation=\"fetch_timeline\"",
        "upstream_outcome=\"rate_limited\"",
    ] {
        assert!(line.contains(wanted), "{wanted} in: {line}");
    }
}

#[tokio::test]
async fn an_access_answer_is_never_hidden_behind_the_cache() {
    let d = deployment("stand-in-forbidden").await;
    let after = forward_cursor(&d).await;
    d.state.message_cache.set_serve(true);
    d.chat.fail_timelines(Some(ChatError::Status {
        status: 403,
        body: "conversations.history answered not_in_channel".to_owned(),
    }));
    let (status, body) = call(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=50&after={after}"),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
}

#[tokio::test]
async fn a_slow_provider_is_answered_from_the_cache_one_read_at_a_time_and_its_read_runs_on() {
    let d = deployment("stand-in-slow").await;
    let after = forward_cursor(&d).await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let reads = d.chat.timeline_reads();

    // Two catch-up reads of one view at once, a poll and another tab's, behind a provider that
    // does not answer: one goes upstream, the other waits for it, and both are answered from the
    // cache when the wait runs out.
    let held = d.chat.hold_next_timeline();
    let read = |reason: &'static str| {
        let state = d.state.clone();
        let after = after.clone();
        tokio::spawn(async move {
            ok(
                &state,
                &path(
                    READ_CHANNEL,
                    &format!("/timeline?view=main&limit=50&after={after}&reason={reason}"),
                ),
                WRITE_TOKEN,
            )
            .await
        })
    };
    let began = std::time::Instant::now();
    let (first, second) = (read("poll"), read("live"));
    let wait = vibe_talk::ops::stand_in_after(&d.state.config);
    for request in [first, second] {
        let body = tokio::time::timeout(wait + Duration::from_secs(5), request)
            .await
            .expect("answered once the stand-in wait ran out, not when the provider did")
            .expect("the request ran");
        assert!(
            body["refresh_failed"].is_string(),
            "answered from the cache: {body}"
        );
        assert_eq!(texts(&body).len(), 12, "{body}");
    }
    assert!(
        began.elapsed() >= wait,
        "the provider was given its time first"
    );
    assert_eq!(
        d.chat.timeline_reads(),
        reads + 1,
        "one read upstream, not one per reader"
    );

    // The read upstream was not abandoned: released, it lands.
    held.entered.await.expect("the read reached the provider");
    held.release.send(()).expect("the read is still waiting");
    settled(&d.state).await;
    assert_eq!(d.chat.timeline_reads(), reads + 1);
}

// --- `#614 timeline-read-failures`: side and counts reads under Slack's rate limits -------------

/// A Slack channel of six messages a minute apart, the second and fourth the roots of threads
/// (two replies and one), filled into a cache read by `clock` through a read of Main and of each
/// thread. `live_poll` makes the channel live-fed (its provider is polled; no poller runs here).
async fn slack_threads_deployment(
    tag: &str,
    live_poll: bool,
) -> (AppState, slack_fake::Fake, TempDir) {
    let mut fixture = slack_fake::FakeState::default();
    let mut history: Vec<Value> = (0..6)
        .map(|n| {
            slack_fake::profiled(
                &slack_fake::ts(n * 60, 0),
                "UADA00001",
                "Ada",
                &format!("message {n}"),
            )
        })
        .collect();
    slack_fake::thread(
        &mut fixture,
        SLACK_CH,
        &mut history[1],
        &[
            (slack_fake::ts(70, 0), "first reply".to_owned()),
            (slack_fake::ts(80, 0), "second reply".to_owned()),
        ],
    );
    slack_fake::thread(
        &mut fixture,
        SLACK_CH,
        &mut history[3],
        &[(slack_fake::ts(200, 0), "only reply".to_owned())],
    );
    fixture.history.insert(SLACK_CH.to_owned(), history);
    let fake = slack_fake::Fake::start(fixture, 10).await;
    let dir = TempDir::new(tag);
    let clock = ManualClock::at(slack_ms(3_600));
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{SLACK}"
kind = "slack"
api_base = "{base}"
token = "{token}"
{poll}

[[channels]]
id = "{SLACK_CH}"
label = "team"
writable = true
"#,
        base = fake.base,
        token = slack_fake::TOKEN,
        poll = if live_poll {
            "live_poll_seconds = 30"
        } else {
            ""
        },
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let state = testing::with_message_cache(state, dir.path(), clock).await;
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    for root in [60, 180] {
        ok(
            &state,
            &path(
                SLACK_CH,
                &format!(
                    "/timeline?view=thread&thread_id={}&limit=50",
                    slack_fake::ts(root, 0)
                ),
            ),
            WRITE_TOKEN,
        )
        .await;
    }
    settled(&state).await;
    (state, fake, dir)
}

/// The thread ids a thread list answer carries, oldest activity first.
fn thread_ids(body: &Value) -> Vec<String> {
    body["threads"]
        .as_array()
        .unwrap_or_else(|| panic!("a thread list: {body}"))
        .iter()
        .map(|thread| thread["id"].as_str().expect("an id").to_owned())
        .collect()
}

/// Every request of `method` from now on answered 429 with a thirty-second `Retry-After`.
fn rate_limit(fake: &slack_fake::Fake, method: &str) {
    fake.with(|state| {
        state
            .rate_limit_many
            .insert(method.to_owned(), (30, usize::MAX));
    });
}

#[tokio::test]
async fn a_side_read_under_a_rate_limited_history_is_answered_from_the_cache_at_once() {
    let (state, fake, _dir) = slack_threads_deployment("side-rate-limited", false).await;
    state.message_cache.set_serve(true);
    rate_limit(&fake, "conversations.history");
    rate_limit(&fake, "conversations.replies");
    let history = fake.calls("conversations.history");
    let capture = vibe_talk::testing::LogCapture::start();

    // The right column's thread list: it used to wait out Slack's thirty-second budget and answer
    // 502 after 16-47 s, since the cache would not stand in for a thread list at all.
    let began = std::time::Instant::now();
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=threads&limit=50&reason=side"),
        WRITE_TOKEN,
    )
    .await;
    assert!(
        began.elapsed() < AT_ONCE * 5,
        "answered at the first rate limit, not after the budget: {:?}",
        began.elapsed()
    );
    assert_eq!(
        thread_ids(&body),
        [slack_fake::ts(60, 0), slack_fake::ts(180, 0)],
        "both threads, the latest activity last: {body}"
    );
    let first = &body["threads"][1];
    assert_eq!(first["reply_count"], 1, "{body}");
    assert_eq!(first["root"]["content"], "message 3", "{body}");
    assert_eq!(first["title"], "message 3", "{body}");
    assert!(
        body["refresh_failed"]
            .as_str()
            .is_some_and(|why| why.contains("limiting")),
        "{body}"
    );
    assert!(body["next_before"].is_null(), "{body}");
    assert_eq!(
        fake.calls("conversations.history"),
        history + 1,
        "one request, answered 429, and no retry"
    );

    // Again, with Slack's Retry-After still running: nothing is sent into it.
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=threads&limit=50&reason=side"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(thread_ids(&body).len(), 2, "{body}");
    assert_eq!(fake.calls("conversations.history"), history + 1);

    // The side column's thread, and All's counts read, the same way.
    let replies = fake.calls("conversations.replies");
    let began = std::time::Instant::now();
    let body = ok(
        &state,
        &path(
            SLACK_CH,
            &format!(
                "/timeline?view=thread&thread_id={}&limit=50&reason=side",
                slack_fake::ts(60, 0)
            ),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        texts(&body),
        ["message 1", "first reply", "second reply"],
        "{body}"
    );
    assert!(body["refresh_failed"].is_string(), "{body}");
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=counts"),
        WRITE_TOKEN,
    )
    .await;
    assert!(
        texts(&body).contains(&"only reply".to_owned()),
        "All from the cache, replies included: {body}"
    );
    assert!(body["refresh_failed"].is_string(), "{body}");
    assert!(began.elapsed() < AT_ONCE * 5, "{:?}", began.elapsed());
    assert!(
        fake.calls("conversations.replies") <= replies + 1,
        "at most the one request Slack refused"
    );
    let text = capture.text();
    assert!(
        !text.contains("server error"),
        "no 5xx was answered: {text}"
    );
}

#[tokio::test]
async fn a_side_read_the_cache_cannot_answer_fails_fast_under_a_rate_limit() {
    // No thread in the channel: nothing to put a thread list together from.
    let (state, fake, _clock, _dir) = slack_deployment("side-nothing-held").await;
    state.message_cache.set_serve(true);
    rate_limit(&fake, "conversations.history");
    let began = std::time::Instant::now();
    let (status, body) = call(
        &state,
        &path(SLACK_CH, "/timeline?view=threads&limit=50&reason=side"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        began.elapsed() < AT_ONCE * 5,
        "failed at the first rate limit: {:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn a_side_read_the_cache_cannot_answer_fails_once_the_wait_has_passed() {
    // All's counts read of a provider whose All the cache never puts together, behind a provider
    // that does not answer: it fails when the stand-in wait runs out, not when the provider
    // answers, and its read runs on.
    let d = deployment("side-slow-nothing").await;
    d.state.message_cache.set_serve(true);
    let held = d.chat.hold_next_timeline();
    let wait = vibe_talk::ops::stand_in_after(&d.state.config);
    let began = std::time::Instant::now();
    let (status, body) = tokio::time::timeout(
        wait + Duration::from_secs(5),
        call(
            &d.state,
            &path(READ_CHANNEL, "/timeline?view=flat&limit=50&reason=counts"),
            WRITE_TOKEN,
        ),
    )
    .await
    .expect("answered once the wait ran out");
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(began.elapsed() >= wait, "the provider was given its time");
    held.entered.await.expect("the read reached the provider");
    held.release.send(()).expect("the read is still waiting");
}

#[tokio::test]
async fn a_live_fed_channel_s_side_and_counts_reads_spend_no_slack_calls_between_upstream_reads() {
    let (state, fake, _dir) = slack_threads_deployment("side-live-fed", true).await;
    state.message_cache.set_serve(true);

    // The first thread list goes upstream, and answers.
    let history = fake.calls("conversations.history");
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=threads&limit=50&reason=side"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(thread_ids(&body).len(), 2, "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert!(
        fake.calls("conversations.history") > history,
        "read upstream"
    );
    settled(&state).await;

    // A reply arrives, and reaches the cache (here through a read of its thread; live ingestion
    // brings it the same way).
    fake.with(|fixture| {
        slack_fake::add_reply(
            fixture,
            SLACK_CH,
            &slack_fake::ts(60, 0),
            &slack_fake::ts(400, 0),
            "news",
        );
    });
    ok(
        &state,
        &path(
            SLACK_CH,
            &format!(
                "/timeline?view=thread&thread_id={}&limit=50",
                slack_fake::ts(60, 0)
            ),
        ),
        WRITE_TOKEN,
    )
    .await;
    settled(&state).await;

    // The next thread lists, newest and catch-up, are the cache's: no Slack call, no failure,
    // and the thread that just moved is now the latest.
    let (history, replies) = (
        fake.calls("conversations.history"),
        fake.calls("conversations.replies"),
    );
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=threads&limit=50&reason=side"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(
        thread_ids(&body),
        [slack_fake::ts(180, 0), slack_fake::ts(60, 0)],
        "{body}"
    );
    assert!(body.get("refresh_failed").is_none(), "{body}");
    let after = body["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    let body = ok(
        &state,
        &path(
            SLACK_CH,
            &format!(
                "/timeline?view=threads&limit=50&reason=side&after={}",
                url_encode(&after)
            ),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(thread_ids(&body).len(), 2, "{body}");
    assert_eq!(fake.calls("conversations.history"), history);
    assert_eq!(fake.calls("conversations.replies"), replies);

    // All's counts read: the first goes upstream, the next is the cache's.
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=counts"),
        WRITE_TOKEN,
    )
    .await;
    settled(&state).await;
    let (history, replies) = (
        fake.calls("conversations.history"),
        fake.calls("conversations.replies"),
    );
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=counts"),
        WRITE_TOKEN,
    )
    .await;
    assert!(texts(&body).contains(&"news".to_owned()), "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert!(body.get("settled_below_ms").is_none(), "{body}");
    assert_eq!(fake.calls("conversations.history"), history);
    assert_eq!(fake.calls("conversations.replies"), replies);

    // A person's read of All still goes upstream.
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=enter"),
        WRITE_TOKEN,
    )
    .await;
    assert!(fake.calls("conversations.history") > history);
}
