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

    /// Fill the read channel's `main` from a write-token timeline read of its newest `limit`
    /// only: the file then proves nothing below them, and a step back from a served page walks the
    /// provider (`#650 cache-first-older-reads`).
    async fn fill_newest(&self, limit: u16) {
        ok(
            &self.state,
            &path(READ_CHANNEL, &format!("/timeline?view=main&limit={limit}")),
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

/// Wait until no step back of `channel`'s `view` is being read upstream behind an answer from the
/// cache (`#650 cache-first-older-reads`), and for the cache's writer.
async fn stepped_behind(state: &AppState, channel: &str, view: TimelineView) {
    let key = || RefreshKey::timeline_backward(ChannelId(channel.to_owned()), view, None);
    tokio::time::timeout(WAIT, async {
        while state.message_cache.claim_refresh(key()).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the step behind finished");
    settled(state).await;
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

    // After those serves, the first one's cursor still steps back. `#650 cache-first-older-reads`:
    // the file proves the five messages below the served page, so they are the answer, at once,
    // with the provider held, and this server's cursor again for the next step.
    let held = d.chat.hold_next_timeline();
    let older = tokio::time::timeout(AT_ONCE, d.step_back(5, &cursor))
        .await
        .expect("the step back answered at once, from the cache");
    // The provider's step, read behind it: its claims were listed two hours ago.
    held.entered
        .await
        .expect("the step is read behind the answer");
    held.release.send(()).expect("the read behind is waiting");
    stepped_behind(&d.state, READ_CHANNEL, TimelineView::Main).await;
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
    let next = served_cursor(&older);
    // Below them the file holds two messages, and its claims stop at the age limit, so older ones
    // may be missing from it: that step walks the provider back to them, which says nothing is
    // older.
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
    d.fill_newest(5).await;
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
    d.fill_newest(5).await;
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

    // Its own cursor steps back past the provider's newest page into Slack's history. (`#650
    // cache-first-older-reads`: the copy proves nothing between the fill's settled stretch and
    // this page, so the step is the provider's.)
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

    // A catch-up read, which the cache never serves: it used to be a 502. Untagged, so not one
    // of the page's reads the cache answers first (`#622 cache-first-reads`).
    let capture = vibe_talk::testing::LogCapture::start();
    let body = ok(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=50&after={after}"),
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

    // Two catch-up reads of one view at once behind a provider that does not answer: one goes
    // upstream, the other waits for it, and both are answered from the cache when the wait runs
    // out. Reads that place live messages, which the cache does not answer first (`#622
    // cache-first-reads`): they ask the provider what this server does not know.
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
    let (first, second) = (read("place"), read("reply-place"));
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
) -> (AppState, slack_fake::Fake, Arc<ManualClock>, TempDir) {
    slack_threads_deployment_with(tag, live_poll, "").await
}

/// [`slack_threads_deployment`] with `table` added to its configuration.
async fn slack_threads_deployment_with(
    tag: &str,
    live_poll: bool,
    table: &str,
) -> (AppState, slack_fake::Fake, Arc<ManualClock>, TempDir) {
    slack_threads_deployment_opening(tag, live_poll, table, &[60, 180]).await
}

/// [`slack_threads_deployment_with`], reading only the threads rooted at `opened` (seconds into the
/// fixture) after Main: the others are known by their roots and reply counts alone, as a thread
/// live ingestion or Main brought is until someone opens it.
async fn slack_threads_deployment_opening(
    tag: &str,
    live_poll: bool,
    table: &str,
    opened: &[i64],
) -> (AppState, slack_fake::Fake, Arc<ManualClock>, TempDir) {
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
{table}
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
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    for &root in opened {
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
    (state, fake, clock, dir)
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
    let (state, fake, _clock, _dir) = slack_threads_deployment("side-rate-limited", false).await;
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
    // `#622 cache-first-reads`: answered from the cache first, the read upstream behind it.
    assert!(body["refresh_failed"].is_string(), "{body}");
    assert!(body["next_before"].is_null(), "{body}");
    quiet(&state, &threads_key(50)).await;
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
    quiet(&state, &threads_key(50)).await;
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
    quiet(&state, &newest_key(SLACK_CH, TimelineView::Flat, 50)).await;
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
async fn a_live_fed_channel_s_side_thread_list_spends_no_slack_call_between_upstream_reads() {
    let (state, fake, clock, _dir) = slack_threads_deployment("side-live-fed", true).await;
    state.message_cache.set_serve(true);
    let list = |extra: &str| {
        path(
            SLACK_CH,
            &format!("/timeline?view=threads&limit=1&reason=side{extra}"),
        )
    };

    // The first thread list is the cache's (`#622 cache-first-reads`), with the list read upstream
    // behind it; the next is that list: one card, and the way back to the other.
    let history = fake.calls("conversations.history");
    let body = ok(&state, &list(""), WRITE_TOKEN).await;
    assert!(body["refresh_failed"].is_string(), "{body}");
    upstream_list(&state).await;
    let body = ok(&state, &list(""), WRITE_TOKEN).await;
    assert_eq!(thread_ids(&body), [slack_fake::ts(180, 0)], "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert_eq!(body["has_more"], true, "{body}");
    let way_back = body["next_before"].as_str().expect("a way back").to_owned();
    assert!(
        fake.calls("conversations.history") > history,
        "read upstream"
    );
    settled(&state).await;

    // A reply arrives in the other thread after that read, and reaches the cache (here through a
    // read of its thread; live ingestion brings it the same way).
    clock.advance(200 * SECOND);
    fake.with(|fixture| {
        slack_fake::add_reply(
            fixture,
            SLACK_CH,
            &slack_fake::ts(60, 0),
            &slack_fake::ts(3_700, 0),
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

    // The next thread lists, newest and catch-up, are the cache's: no Slack call, no failure, the
    // thread that just moved is the latest, and the upstream list's own way back still stands.
    let (history, replies) = (
        fake.calls("conversations.history"),
        fake.calls("conversations.replies"),
    );
    let body = ok(&state, &list(""), WRITE_TOKEN).await;
    assert_eq!(
        thread_ids(&body),
        [slack_fake::ts(180, 0), slack_fake::ts(60, 0)],
        "{body}"
    );
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert_eq!(body["has_more"], true, "{body}");
    assert_eq!(body["next_before"], way_back.as_str(), "{body}");
    let after = body["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    let body = ok(
        &state,
        &list(&format!("&after={}", url_encode(&after))),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(thread_ids(&body).len(), 2, "{body}");
    assert_eq!(fake.calls("conversations.history"), history);
    assert_eq!(fake.calls("conversations.replies"), replies);

    // "Older threads…" reads upstream, on that way back.
    let body = ok(
        &state,
        &path(
            SLACK_CH,
            &format!(
                "/timeline?view=threads&limit=1&reason=threads&before={}",
                url_encode(&way_back)
            ),
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(thread_ids(&body), [slack_fake::ts(60, 0)], "{body}");

    // All's counts read is not the thread list: it reads All upstream, for the replies the counts
    // say the page lacks, behind an answer from the cache (`#650 cache-first-older-reads`); the
    // page's next counts read takes what that read brought.
    let history = fake.calls("conversations.history");
    let counts_read = path(SLACK_CH, "/timeline?view=flat&limit=50&reason=counts");
    let body = ok(&state, &counts_read, WRITE_TOKEN).await;
    assert!(body["refresh_failed"].is_string(), "{body}");
    landed(&state, &newest_key(SLACK_CH, TimelineView::Flat, 50), None).await;
    assert!(fake.calls("conversations.history") > history);
    let body = ok(&state, &counts_read, WRITE_TOKEN).await;
    assert!(body.get("refresh_failed").is_none(), "{body}");
}

#[tokio::test]
async fn with_serving_off_a_side_read_still_waits_out_a_short_rate_limit() {
    // Nothing would stand in for it, so it is not hurried: it waits, as every read did.
    let (state, fake, _clock, _dir) = slack_threads_deployment("side-serve-off", false).await;
    fake.with(|fixture| {
        fixture.rate_limit_once = Some(("conversations.history".to_owned(), 1));
    });
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
        fake.calls("conversations.history") >= history + 2,
        "sent again after the Retry-After"
    );
}

// --- `#617 cold-load-first-paint` ---------------------------------------------------------------

#[tokio::test]
async fn a_live_fed_side_read_past_the_interval_is_answered_at_once_and_refreshes_behind() {
    // `#617 cold-load-first-paint`: every fifth minute the side read went upstream itself and
    // waited the whole fallback for a scan slower than it.
    let (state, fake, _clock, _dir) = slack_threads_deployment("side-refresh-behind", true).await;
    state.message_cache.set_serve(true);
    let list = path(SLACK_CH, "/timeline?view=threads&limit=50&reason=side");
    ok(&state, &list, WRITE_TOKEN).await;
    upstream_list(&state).await;
    settled(&state).await;
    state
        .message_cache
        .age_upstream_reads(vibe_talk::ops::LIVE_FED_UPSTREAM_EVERY + Duration::from_secs(1));
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let history = fake.calls("conversations.history");
    let began = std::time::Instant::now();
    let body = ok(&state, &list, WRITE_TOKEN).await;
    assert!(began.elapsed() < AT_ONCE, "{:?}", began.elapsed());
    assert_eq!(thread_ids(&body).len(), 2, "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
    // The read behind it, once, however many side reads come meanwhile.
    ok(&state, &list, WRITE_TOKEN).await;
    tokio::time::timeout(WAIT, async {
        while fake.calls("conversations.history") == history {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("an upstream read of the list began behind the answer");
    fake.with(|fixture| fixture.delay = None);
    tokio::time::timeout(WAIT, async {
        while state
            .message_cache
            .upstream_read_within(
                &RefreshKey::view(ChannelId(SLACK_CH.to_owned()), TimelineView::Threads, None),
                vibe_talk::ops::LIVE_FED_UPSTREAM_EVERY,
            )
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the read behind it landed and was remembered");
    assert_eq!(
        fake.calls("conversations.history"),
        history + 1,
        "one read behind both answers"
    );
}

// --- `#622 cache-first-reads`: a page's read answered from the cache first ---------------------

/// The newest thread list key of the Slack channel at `limit`.
fn threads_key(limit: u16) -> RefreshKey {
    newest_key(SLACK_CH, TimelineView::Threads, limit)
}

/// Wait until no upstream read under `key` is running.
async fn quiet(state: &AppState, key: &RefreshKey) {
    tokio::time::timeout(WAIT, async {
        while state.message_cache.refresh_running(key) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the read behind the answer ended");
}

/// Wait until the Slack channel's thread list has been read upstream in full.
async fn upstream_list(state: &AppState) {
    tokio::time::timeout(WAIT, async {
        while state
            .message_cache
            .upstream_read_within(
                &RefreshKey::view(ChannelId(SLACK_CH.to_owned()), TimelineView::Threads, None),
                Duration::from_secs(60),
            )
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the thread list was read upstream behind the answer");
}

/// The newest page key every read of `view` of `channel` at `limit` shares upstream.
fn newest_key(channel: &str, view: TimelineView, limit: u16) -> RefreshKey {
    RefreshKey::timeline(ChannelId(channel.to_owned()), view, None, limit)
}

/// Wait until the read begun behind an answer from the cache has landed for the next read.
async fn landed(state: &AppState, key: &RefreshKey, after: Option<&str>) {
    tokio::time::timeout(WAIT, async {
        while !state.landed.holds(key, after) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the read behind the answer landed");
}

#[tokio::test]
async fn slack_s_all_the_cache_holds_is_answered_at_once_and_the_read_behind_it_lands_for_the_next()
{
    let (state, fake, _clock, _dir) = slack_threads_deployment("cache-first-all", true).await;
    state.message_cache.set_serve(true);
    // Every Slack call now takes three seconds: an answer within one never waited on one.
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let history = fake.calls("conversations.history");
    let capture = vibe_talk::testing::LogCapture::start();
    let began = std::time::Instant::now();
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=enter"),
        WRITE_TOKEN,
    )
    .await;
    assert!(began.elapsed() < AT_ONCE, "{:?}", began.elapsed());
    let said = texts(&body);
    for text in [
        "message 0",
        "first reply",
        "second reply",
        "only reply",
        "message 5",
    ] {
        assert!(said.iter().any(|said| said == text), "{text}: {body}");
    }
    assert!(
        body["refresh_failed"].is_string(),
        "the page lets it answer for nothing it lacks, and reads again soon: {body}"
    );
    assert!(
        body["as_of"].is_string(),
        "dated by when the copy was last brought up to date: {body}"
    );
    assert!(
        capture.text().contains(" served_from=\"cache-first\""),
        "{}",
        capture.text()
    );

    // The read behind it lands, and the page's next read is answered with it: the provider's own
    // page, which proves what it lacks, at no second read.
    landed(&state, &newest_key(SLACK_CH, TimelineView::Flat, 50), None).await;
    let calls = fake.calls("conversations.history");
    assert_eq!(calls, history + 1, "one history read behind the answer");
    let body = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=poll"),
        WRITE_TOKEN,
    )
    .await;
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert!(body["settled_below_ms"].is_number(), "{body}");
    assert_eq!(texts(&body).len(), said.len(), "{body}");
    assert_eq!(fake.calls("conversations.history"), calls);
    assert!(
        capture.text().contains(" served_from=\"landed\""),
        "{}",
        capture.text()
    );
}

#[tokio::test]
async fn a_page_entering_a_view_the_cache_holds_never_waits_on_the_provider() {
    let d = deployment("cache-first-enter").await;
    forward_cursor(&d).await;
    // Too old for the serve rule: it used to wait FIRST_PAINT_WAIT on a provider that does not
    // answer.
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let reads = d.chat.timeline_reads();
    let held = d.chat.hold_next_timeline();
    let enter = path(READ_CHANNEL, "/timeline?view=main&limit=50&reason=enter");
    let body = tokio::time::timeout(AT_ONCE, ok(&d.state, &enter, WRITE_TOKEN))
        .await
        .expect("answered from the cache at once");
    assert_eq!(texts(&body).len(), 12, "{body}");
    assert!(body["refresh_failed"].is_string(), "{body}");
    assert!(
        body["as_of"].is_string(),
        "a channel nothing feeds says how old its copy is: {body}"
    );
    held.entered
        .await
        .expect("the read behind it reached the provider");
    // Another read while that one is out: from the cache, and no second read upstream.
    let body = tokio::time::timeout(AT_ONCE, ok(&d.state, &enter, WRITE_TOKEN))
        .await
        .expect("answered from the cache at once");
    assert!(body["refresh_failed"].is_string(), "{body}");
    assert_eq!(d.chat.timeline_reads(), reads + 1);
    held.release.send(()).expect("the read is still out");
    landed(
        &d.state,
        &newest_key(READ_CHANNEL, TimelineView::Main, 50),
        None,
    )
    .await;
    let body = ok(&d.state, &enter, WRITE_TOKEN).await;
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert!(body.get("as_of").is_none(), "{body}");
    assert_eq!(d.chat.timeline_reads(), reads + 1, "one read for all three");
}

#[tokio::test]
async fn a_catch_up_read_the_cache_holds_is_answered_at_once_and_its_cursor_lands_once() {
    let d = deployment("cache-first-forward").await;
    let after = forward_cursor(&d).await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let reads = d.chat.timeline_reads();
    let held = d.chat.hold_next_timeline();
    let poll = path(
        READ_CHANNEL,
        &format!("/timeline?view=main&limit=50&after={after}&reason=visible"),
    );
    let body = tokio::time::timeout(AT_ONCE, ok(&d.state, &poll, WRITE_TOKEN))
        .await
        .expect("answered from the cache at once");
    assert_eq!(texts(&body).len(), 12, "{body}");
    assert!(body["refresh_failed"].is_string(), "{body}");
    held.entered
        .await
        .expect("the read behind it reached the provider");
    held.release.send(()).expect("the read is still out");
    landed(
        &d.state,
        &RefreshKey::timeline_forward(ChannelId(READ_CHANNEL.to_owned()), TimelineView::Main, None),
        Some(&after),
    )
    .await;
    // The same cursor again (the page moved nothing for an answer from the cache): the delta.
    let body = ok(&d.state, &poll, WRITE_TOKEN).await;
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert!(body["delta"].is_object(), "{body}");
    assert_eq!(d.chat.timeline_reads(), reads + 1);
}

#[tokio::test]
async fn a_view_the_cache_has_never_seen_goes_to_the_provider_first() {
    let d = deployment("cache-first-miss").await;
    d.state.message_cache.set_serve(true);
    let reads = d.chat.timeline_reads();
    let held = d.chat.hold_next_timeline();
    let capture = vibe_talk::testing::LogCapture::start();
    let state = d.state.clone();
    let request = tokio::spawn(async move {
        ok(
            &state,
            &path(READ_CHANNEL, "/timeline?view=main&limit=50&reason=enter"),
            WRITE_TOKEN,
        )
        .await
    });
    held.entered.await.expect("the read went upstream");
    tokio::time::sleep(vibe_talk::ops::FIRST_PAINT_WAIT + Duration::from_millis(500)).await;
    assert!(
        !request.is_finished(),
        "nothing to answer with: the read waits for the provider"
    );
    held.release.send(()).expect("the read is still out");
    let body = request.await.expect("the request ran");
    assert_eq!(texts(&body).len(), 12, "{body}");
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert_eq!(d.chat.timeline_reads(), reads + 1);
    assert!(
        capture.text().contains(" served_from=\"upstream\""),
        "{}",
        capture.text()
    );
}

// --- `#688 vt-timeline-latency`: no page read the cache can answer waits on Slack ---------------

#[tokio::test]
async fn a_thread_nobody_has_opened_is_answered_from_its_root_at_once_under_a_rate_limit() {
    // Main was read, so the cache holds the root of the thread at 180 s and its reply count, but
    // nobody opened the thread: its scope holds nothing, and no read of it has landed.
    let (state, fake, _clock, _dir) =
        slack_threads_deployment_opening("unopened-thread", true, "", &[60]).await;
    state.message_cache.set_serve(true);
    rate_limit(&fake, "conversations.history");
    rate_limit(&fake, "conversations.replies");
    let capture = vibe_talk::testing::LogCapture::start();
    let thread = path(
        SLACK_CH,
        &format!(
            "/timeline?view=thread&thread_id={}&limit=50&reason=visible",
            slack_fake::ts(180, 0)
        ),
    );
    // It went to Slack first and waited out the thirty-second Retry-After on the request, then
    // answered 502: 33-58 s for a new thread on the live instance, and the page gave up.
    let body = tokio::time::timeout(AT_ONCE, ok(&state, &thread, WRITE_TOKEN))
        .await
        .expect("answered from the cache at once, not after Slack's rate limit");
    assert_eq!(
        texts(&body),
        ["message 3"],
        "the root, as Main holds it: {body}"
    );
    assert!(
        body["refresh_failed"].is_string(),
        "the page lets it answer for nothing it lacks, and reads again soon: {body}"
    );
    assert!(
        body["as_of"].is_string(),
        "dated by what it holds, as no read of the thread ever landed: {body}"
    );
    assert!(
        capture.text().contains(" served_from=\"cache-first\""),
        "{}",
        capture.text()
    );
    assert!(
        !capture.text().contains("server error"),
        "{}",
        capture.text()
    );
}

#[tokio::test]
async fn no_page_read_of_a_view_the_cache_holds_waits_on_slack() {
    let (state, fake, _clock, _dir) =
        slack_threads_deployment_opening("no-page-read-waits", true, "", &[60]).await;
    state.message_cache.set_serve(true);
    // Slack refuses every read, for thirty seconds at a time, and is slow to say so.
    rate_limit(&fake, "conversations.history");
    rate_limit(&fake, "conversations.replies");
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let capture = vibe_talk::testing::LogCapture::start();
    let views = [
        "view=main".to_owned(),
        "view=flat".to_owned(),
        "view=threads".to_owned(),
        format!("view=thread&thread_id={}", slack_fake::ts(60, 0)),
        format!("view=thread&thread_id={}", slack_fake::ts(180, 0)),
    ];
    // Every reason a page reads a view's newest end or catches up with, except those that exist to
    // ask Slack what the server does not know (`place`, `reply-place`, and "Older threads…").
    let reasons = [
        "enter", "poll", "live", "mutation", "visible", "reset", "replay", "pull", "expired",
        "older", "search", "counts", "side",
    ];
    for view in &views {
        for reason in reasons {
            let uri = path(
                SLACK_CH,
                &format!("/timeline?{view}&limit=50&reason={reason}"),
            );
            let began = std::time::Instant::now();
            let (status, body) = call(&state, &uri, WRITE_TOKEN).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
            assert!(
                began.elapsed() < AT_ONCE,
                "{uri} waited on Slack: {:?}",
                began.elapsed()
            );
            let held = body["messages"].as_array().map_or(0, Vec::len)
                + body["threads"].as_array().map_or(0, Vec::len);
            assert!(held > 0, "{uri} answered with what the cache holds: {body}");
        }
    }
    assert!(
        !capture.text().contains("server error"),
        "{}",
        capture.text()
    );
}

#[tokio::test]
async fn placing_a_live_message_while_slack_is_limiting_is_answered_from_the_cache_at_once() {
    let (state, fake, _clock, _dir) = slack_threads_deployment("place-limited", true).await;
    state.message_cache.set_serve(true);
    let all = path(SLACK_CH, "/timeline?view=flat&limit=50&reason=place");
    // Slack answering: placement reads it first, as ever, and is answered by it.
    let body = ok(&state, &all, WRITE_TOKEN).await;
    assert!(body.get("refresh_failed").is_none(), "{body}");

    // Slack starts refusing reads, and has just refused one.
    rate_limit(&fake, "conversations.history");
    rate_limit(&fake, "conversations.replies");
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=50&reason=counts"),
        WRITE_TOKEN,
    )
    .await;
    quiet(&state, &newest_key(SLACK_CH, TimelineView::Flat, 50)).await;
    let capture = vibe_talk::testing::LogCapture::start();
    // It used to wait out the whole stand-in wait (ten seconds) on a Slack that was refusing it.
    let body = tokio::time::timeout(AT_ONCE, ok(&state, &all, WRITE_TOKEN))
        .await
        .expect("answered from the cache at once while Slack is limiting");
    assert!(
        texts(&body).contains(&"only reply".to_owned()),
        "All from the cache, replies included: {body}"
    );
    assert!(body["refresh_failed"].is_string(), "{body}");
    // Stood in for at once: as limited, or as failed when the read behind gave up at the first
    // rate limit before the answer was put together.
    assert!(
        capture
            .text()
            .contains("read_reason=\"place\" read_view=\"flat\" read_cursor=\"newest\" served_from=\"stand-in\""),
        "{}",
        capture.text()
    );
}

/// A server voice that answers at once.
struct QuickSpeech;

#[async_trait::async_trait]
impl vibe_talk::speech::SpeechProvider for QuickSpeech {
    fn describe(&self) -> vibe_talk::speech::Description {
        vibe_talk::speech::Description {
            backend: "quick-test",
            label: "Quick test voice".to_owned(),
            playback: vibe_talk::speech::Playback::Audio,
            local_only: false,
            chain: vec![],
        }
    }

    async fn speak(
        &self,
        text: &str,
        _speed: Option<f64>,
    ) -> Result<vibe_talk::speech::Speech, vibe_talk::speech::SpeechError> {
        Ok(vibe_talk::speech::Speech {
            audio: text.as_bytes().to_vec(),
            content_type: "audio/test".to_owned(),
        })
    }
}

async fn post(state: &AppState, uri: &str, body: &Value) -> (StatusCode, Value) {
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

#[tokio::test]
async fn read_aloud_of_a_message_the_cache_holds_fetches_nothing_and_a_miss_still_fetches() {
    let (mut state, fake, _clock, _dir) =
        slack_threads_deployment("cache-first-speech", true).await;
    state.speech = Arc::new(QuickSpeech);
    let root = slack_fake::ts(60, 0);
    let thread = ok(
        &state,
        &path(
            SLACK_CH,
            &format!("/timeline?view=thread&thread_id={root}&limit=50"),
        ),
        WRITE_TOKEN,
    )
    .await;
    let ids: Vec<String> = thread["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["id"].as_str().expect("an id").to_owned())
        .collect();
    assert_eq!(ids.len(), 3, "{thread}");
    settled(&state).await;
    state.message_cache.set_serve(true);
    // Long past the page's own read: nothing in the read-ahead memory.
    state.read_ahead.clear();
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let calls = || fake.calls("conversations.replies") + fake.calls("conversations.history");
    let before = calls();
    let capture = vibe_talk::testing::LogCapture::start();
    let prepare = path(SLACK_CH, "/speech/prepare");
    let began = std::time::Instant::now();
    let (status, body) = post(
        &state,
        &prepare,
        &serde_json::json!({ "thread_id": root, "ids": [ids[0], ids[2]] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(began.elapsed() < AT_ONCE, "{:?}", began.elapsed());
    let prepared: Vec<&str> = body["prepared"]
        .as_array()
        .expect("prepared")
        .iter()
        .map(|entry| entry["message_id"].as_str().expect("an id"))
        .collect();
    assert_eq!(prepared, [ids[0].as_str(), ids[2].as_str()], "{body}");
    assert_eq!(calls(), before, "no Slack call");
    let text = capture.text();
    assert!(
        text.contains("cache_hits=2") && text.contains("fetches=0"),
        "{text}"
    );
    assert!(text.contains(" served_from=\"held\""), "{text}");

    // A message the cache does not hold is fetched, as before.
    fake.with(|fixture| {
        slack_fake::add_reply(fixture, SLACK_CH, &root, &slack_fake::ts(75, 0), "unseen");
    });
    let missing = vibe_talk::slack::ids::message_id_from_ts(&slack_fake::ts(75, 0))
        .expect("an id")
        .0;
    let (status, body) = post(
        &state,
        &prepare,
        &serde_json::json!({ "thread_id": root, "ids": [missing] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(calls() > before, "the miss went to Slack");
    let text = capture.text();
    assert!(
        text.contains("cache_hits=0") && text.contains("fetches=1"),
        "{text}"
    );
    assert!(text.contains(" served_from=\"fetched\""), "{text}");
}

#[tokio::test]
async fn an_expired_cursor_behind_an_answer_from_the_cache_is_the_next_read_s_answer() {
    let d = deployment("cache-first-expired").await;
    let after = forward_cursor(&d).await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    d.chat
        .fail_timelines(Some(ChatError::CursorExpired("too old".to_owned())));
    let poll = path(
        READ_CHANNEL,
        &format!("/timeline?view=main&limit=50&after={after}&reason=poll"),
    );
    let body = ok(&d.state, &poll, WRITE_TOKEN).await;
    assert!(body["refresh_failed"].is_string(), "{body}");
    landed(
        &d.state,
        &RefreshKey::timeline_forward(ChannelId(READ_CHANNEL.to_owned()), TimelineView::Main, None),
        Some(&after),
    )
    .await;
    // The page's next read from the same cursor learns it expired, and reads the newest page.
    let (status, body) = call(&d.state, &poll, WRITE_TOKEN).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["error"], "cursor_expired", "{body}");
}

#[tokio::test]
async fn an_answer_from_the_cache_is_dated_by_the_last_upstream_read_that_answered() {
    let d = deployment("cache-first-dated").await;
    let after = forward_cursor(&d).await;
    let listed = T0 + HOUR;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let poll = |after: &str| {
        path(
            READ_CHANNEL,
            &format!("/timeline?view=main&limit=50&after={after}&reason=poll"),
        )
    };
    let forward =
        RefreshKey::timeline_forward(ChannelId(READ_CHANNEL.to_owned()), TimelineView::Main, None);
    let as_of = |body: &Value| {
        vibe_talk::clock::instant_ms(body["as_of"].as_str().expect("dated")).expect("an instant")
    };

    // The provider is failing: every answer is the cache's, dated by its last listing, however
    // long that goes on, so the page says it could not refresh.
    d.chat.fail_timelines(Some(rate_limited()));
    for _ in 0..2 {
        let body = ok(&d.state, &poll(&after), WRITE_TOKEN).await;
        assert!(body["refresh_failed"].is_string(), "{body}");
        assert_eq!(as_of(&body), listed, "{body}");
        quiet(&d.state, &forward).await;
    }

    // It answers again: the catch-up behind lands for the next read from that cursor, and the
    // read after it, from the cache again, is as new as that catch-up, which listed nothing.
    d.chat.fail_timelines(None);
    ok(&d.state, &poll(&after), WRITE_TOKEN).await;
    landed(&d.state, &forward, Some(&after)).await;
    let body = ok(&d.state, &poll(&after), WRITE_TOKEN).await;
    assert!(body.get("refresh_failed").is_none(), "{body}");
    let next = body["next_after"].as_str().expect("a cursor").to_owned();
    let body = ok(&d.state, &poll(&url_encode(&next)), WRITE_TOKEN).await;
    assert!(body["refresh_failed"].is_string(), "{body}");
    assert_eq!(as_of(&body), T0 + 3 * HOUR, "{body}");
}

/// A voice catch-up of the Slack channel, as `recent_activity` asks for one over the last `hours`
/// counted back from the fixtures' clock.
async fn slack_catch_up(state: &AppState, hours: u64) -> vibe_talk::ops::Activity {
    vibe_talk::ops::activity(
        state,
        vibe_talk::auth::Scope::Write,
        vibe_talk::ops::ActivityRequest {
            hours: Some(hours),
            per_channel_limit: None,
            channel: None,
            now_ms: slack_ms(3_600),
            unread_only: false,
            channels: &[],
        },
    )
    .await
    .expect("a catch-up answers")
}

/// `#649 voice-agent-tools`. The owner's catch-up of 2026-10-28: after a restart this server held
/// no window of any channel in memory, Slack was rate-limiting history and replies, every channel
/// missed its eight-second read, and the answer said there were no messages in any channel. With
/// the message cache holding the channel, the catch-up answers from it at once, replies and all,
/// says it is the cache's copy and how old, and reads Slack behind the answer; once that read is
/// rate-limited, the next catch-up does not start it again.
#[tokio::test]
async fn a_catch_up_with_nothing_held_answers_from_the_cache_at_once_under_a_rate_limit() {
    let (state, fake, _clock, _dir) = slack_threads_deployment("catch-up-cache-first", false).await;
    state.message_cache.set_serve(true);
    rate_limit(&fake, "conversations.history");
    rate_limit(&fake, "conversations.replies");
    let channel = ChannelId(SLACK_CH.to_owned());
    assert!(!state.live.recent().holds(&channel), "nothing in memory");

    let began = std::time::Instant::now();
    let activity = slack_catch_up(&state, 12).await;
    let took = began.elapsed();
    eprintln!(
        "voice-agent-tools measurement: recent_activity from the cache, nothing held, Slack \
         rate-limited: {} ms",
        took.as_millis()
    );
    // Measured at 1 ms on its own; the bound is the suite's "at once", which a loaded parallel run
    // still meets, rather than the 300 ms target, which only an idle host can be held to.
    assert!(took < AT_ONCE, "{took:?}");
    let part = &activity.channels[0];
    assert!(
        matches!(
            part.currency,
            vibe_talk::ops::Currency::Cached { age, .. } if age < Duration::from_secs(3_600)
        ),
        "the cache's copy, dated by its own clock: {:?}",
        part.currency
    );
    assert!(part.copied_at.is_some(), "says what time the copy is as of");
    let read = part.read.as_ref().expect("answered");
    let said: Vec<&str> = read.messages.iter().map(|m| m.content.as_str()).collect();
    for text in ["message 0", "first reply", "only reply", "message 5"] {
        assert!(said.contains(&text), "{text}: {said:?}");
    }
    let sources = activity.sources();
    assert_eq!((sources.cached, sources.unread), (1, 0), "{sources:?}");
    assert_eq!(sources.served_from(), "cache");

    // The read behind the answer meets the rate limit; the next catch-up names the failure and
    // starts no read of its own while the backoff runs.
    tokio::time::timeout(WAIT, async {
        while state.live.recent().snapshot(&channel).failure.is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the read behind the answer failed on the rate limit");
    assert!(state.live.recent().backing_off(&channel));
    let history = fake.calls("conversations.history");
    let activity = slack_catch_up(&state, 12).await;
    assert!(
        matches!(
            activity.channels[0].currency,
            vibe_talk::ops::Currency::Cached {
                failed: Some(_),
                ..
            }
        ),
        "{:?}",
        activity.channels[0].currency
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fake.calls("conversations.history"),
        history,
        "no read started while Slack is refusing them"
    );
}

/// One MCP tool call as the voice agent makes it with the write token, and its text.
async fn agent_tool(state: &AppState, name: &str, arguments: Value) -> String {
    let outcome = vibe_talk::mcp::protocol::dispatch(
        state,
        vibe_talk::auth::Scope::Write,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        }),
    )
    .await;
    let vibe_talk::mcp::protocol::Outcome::Reply(response) = outcome else {
        panic!("{name}: no reply");
    };
    let result = response.result.as_ref().expect("a result");
    assert_eq!(result["isError"], false, "{name}: {result}");
    result["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned()
}

/// `#649 voice-agent-tools`. Every read tool the voice agent has, answered from the message cache
/// while Slack is rate-limiting every read and slow besides: read_message, digest_channel,
/// find_message, read_page (the newest page, and a page since an instant the cache reaches past)
/// and count_messages since that instant. Each answers within a second with what the cache holds,
/// and its access line says it came from the cache.
#[tokio::test]
async fn every_voice_agent_read_tool_answers_from_the_cache_while_slack_is_refusing_reads() {
    let (state, fake, _clock, _dir) = slack_threads_deployment("tools-cache-first", false).await;
    state.message_cache.set_serve(true);
    for method in [
        "conversations.history",
        "conversations.replies",
        "conversations.info",
    ] {
        rate_limit(&fake, method);
    }
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let capture = vibe_talk::testing::LogCapture::start();
    let since = vibe_talk::clock::iso_from_ms(slack_ms(30));
    // The id read_message is handed, as the agent would have it: from a line it read.
    let digest = agent_tool(
        &state,
        "digest_channel",
        serde_json::json!({ "channel_id": SLACK_CH }),
    )
    .await;
    let fourth = digest
        .lines()
        .find(|line| line.ends_with("] message 4"))
        .and_then(|line| line.strip_prefix('['))
        .and_then(|line| line.split(' ').next())
        .expect("the digest lists message 4")
        .to_owned();
    for (tool, arguments, expect) in [
        (
            "read_message",
            serde_json::json!({ "channel_id": SLACK_CH, "message_id": fourth }),
            "message 4",
        ),
        (
            "digest_channel",
            serde_json::json!({ "channel_id": SLACK_CH }),
            "message 5",
        ),
        (
            "find_message",
            serde_json::json!({ "channel_id": SLACK_CH, "query": "message 3" }),
            "message 3",
        ),
        (
            "read_page",
            serde_json::json!({ "channel_id": SLACK_CH }),
            "message 5",
        ),
        (
            "read_page",
            serde_json::json!({ "channel_id": SLACK_CH, "since": since }),
            "message 1",
        ),
        (
            "count_messages",
            serde_json::json!({ "channel_id": SLACK_CH, "since": since }),
            "holds AT LEAST 5 messages",
        ),
    ] {
        let began = std::time::Instant::now();
        let text = agent_tool(&state, tool, arguments).await;
        let took = began.elapsed();
        eprintln!(
            "voice-agent-tools measurement: {tool} from the cache under a rate limit: {} ms",
            took.as_millis()
        );
        assert!(took < AT_ONCE, "{tool}: {took:?}");
        assert!(text.contains(expect), "{tool}: {text}");
    }
    let page = agent_tool(
        &state,
        "read_page",
        serde_json::json!({ "channel_id": SLACK_CH }),
    )
    .await;
    assert!(page.contains("From this server's copy, as of "), "{page}");
    let log = capture.text();
    assert!(
        log.matches(" served_from=\"cache-first\"").count() >= 6,
        "{log}"
    );
}

// --- `#650 cache-first-older-reads` ---------------------------------------------------------------

/// A table that keeps every day (`#595 cache-full-history`): with no age limit the copy's claims
/// reach a channel's start, and say nothing is older. The walk itself is not started in a test.
const FULL_HISTORY: &str = "\n[message_cache]\nbackfill = \"full\"\n";

/// Read Slack's Main upstream again past the unsettled band of the deployment's fill, with serving
/// still off, so the copy's claims reach its newest messages: a page's oldest message then lies in
/// the claims, as one fifty messages down does in a live channel.
async fn settle_band(state: &AppState, clock: &ManualClock) {
    clock.advance(11 * MINUTE);
    ok(
        state,
        &path(SLACK_CH, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(state).await;
}

/// The `served_from` words of the access lines `capture` holds for `channel`'s timeline, in order.
fn served_from(capture: &vibe_talk::testing::LogCapture, channel: &str) -> Vec<String> {
    capture
        .access_lines()
        .into_iter()
        .filter(|line| line.contains(&format!("/channels/{channel}/timeline")))
        .filter_map(|line| {
            line.split(" served_from=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_owned)
        })
        .collect()
}

#[tokio::test]
async fn slack_s_all_held_from_its_start_says_there_is_nothing_older_so_the_page_never_steps_back()
{
    // The loop the issue found: a page at the top of a view the server's copy holds whole, from
    // its start, was told there was more, stepped back after every poll, and each step walked
    // Slack to learn there was nothing. The copy now says so itself.
    let (state, fake, clock, _dir) =
        slack_threads_deployment_with("older-all-start", false, FULL_HISTORY).await;
    settle_band(&state, &clock).await;
    state.message_cache.set_serve(true);
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    for reason in ["poll", "older", "search"] {
        let began = std::time::Instant::now();
        let body = ok(
            &state,
            &path(
                SLACK_CH,
                &format!("/timeline?view=flat&limit=50&reason={reason}"),
            ),
            WRITE_TOKEN,
        )
        .await;
        assert!(began.elapsed() < AT_ONCE, "{reason}: {:?}", began.elapsed());
        assert!(body["refresh_failed"].is_string(), "{reason}: {body}");
        assert_eq!(body["has_more"], false, "{reason}: {body}");
        assert!(body["next_before"].is_null(), "{reason}: {body}");
    }
}

#[tokio::test]
async fn slack_s_all_steps_back_from_the_servers_copy_without_waiting_on_slack() {
    // Every newest page of All comes from the copy, with this server's cursor; every step back
    // from it walked Slack from its newest page, 2.6 s a step and once 21 s. The copy holds the
    // whole history, so the steps come from it, to the start.
    let (state, fake, clock, _dir) =
        slack_threads_deployment_with("older-all-steps", false, FULL_HISTORY).await;
    settle_band(&state, &clock).await;
    state.message_cache.set_serve(true);
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let capture = vibe_talk::testing::LogCapture::start();
    let newest = ok(
        &state,
        &path(SLACK_CH, "/timeline?view=flat&limit=2&reason=poll"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(texts(&newest), ["message 4", "message 5"], "{newest}");
    assert_eq!(newest["has_more"], true, "{newest}");
    let mut cursor = served_cursor(&newest);
    let mut steps = Vec::new();
    loop {
        let began = std::time::Instant::now();
        let older = ok(
            &state,
            &path(
                SLACK_CH,
                &format!(
                    "/timeline?view=flat&limit=2&reason=older&before={}",
                    url_encode(&cursor)
                ),
            ),
            WRITE_TOKEN,
        )
        .await;
        assert!(began.elapsed() < AT_ONCE, "{:?}", began.elapsed());
        // Its replies are the newest the copy holds of each thread, as All's stand-in's are: the
        // page takes nothing out for lacking one.
        assert!(older["refresh_failed"].is_string(), "{older}");
        assert!(older["next_after"].is_null(), "{older}");
        steps.push(texts(&older));
        if older["has_more"] == false {
            break;
        }
        cursor = served_cursor(&older);
        assert!(
            steps.len() < 5,
            "the steps never reached the start: {steps:?}"
        );
    }
    assert_eq!(
        steps,
        [
            vec!["message 2", "message 3", "only reply"],
            vec!["message 0", "message 1", "first reply", "second reply"],
        ],
    );
    let words = served_from(&capture, SLACK_CH);
    assert_eq!(
        words.first().map(String::as_str),
        Some("cache-first"),
        "{words:?}"
    );
    assert!(
        words[1..].iter().all(|word| word == "cache-older"),
        "{words:?}"
    );
}

#[tokio::test]
async fn a_step_back_the_copy_cannot_prove_still_reads_the_provider() {
    // The copy holds only the newest five, so nothing below them is proven: the step walks the
    // provider as before, and its answer carries the provider's own cursor.
    let d = deployment("older-unproven").await;
    d.fill_newest(5).await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let body = d.served_reload(5).await;
    let cursor = served_cursor(&body);
    let capture = vibe_talk::testing::LogCapture::start();
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
    assert!(
        older["next_before"]
            .as_str()
            .is_some_and(|next| !next.starts_with(vibe_talk::timeline_served::PREFIX)),
        "{older}"
    );
    assert_eq!(served_from(&capture, READ_CHANNEL), ["upstream"]);
}

#[tokio::test]
async fn all_s_counts_read_never_waits_on_slack_and_the_next_one_takes_the_read_behind_it() {
    // `read_reason=counts` waited 8.6 s on Slack. Answered from the copy, it says so, and the read
    // made behind it is what the page's next counts read takes: the provider's own page, which
    // ends the page's asking.
    let (state, fake, _clock, _dir) = slack_threads_deployment("older-counts", false).await;
    state.message_cache.set_serve(true);
    fake.with(|fixture| fixture.delay = Some(Duration::from_secs(3)));
    let counts_read = path(SLACK_CH, "/timeline?view=flat&limit=50&reason=counts");
    let began = std::time::Instant::now();
    let body = ok(&state, &counts_read, WRITE_TOKEN).await;
    assert!(began.elapsed() < AT_ONCE, "{:?}", began.elapsed());
    assert!(body["refresh_failed"].is_string(), "{body}");
    landed(&state, &newest_key(SLACK_CH, TimelineView::Flat, 50), None).await;
    let calls = fake.calls("conversations.history");
    let capture = vibe_talk::testing::LogCapture::start();
    let body = ok(&state, &counts_read, WRITE_TOKEN).await;
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert_eq!(fake.calls("conversations.history"), calls);
    assert_eq!(served_from(&capture, SLACK_CH), ["landed"]);
}

#[tokio::test]
async fn a_step_back_over_a_drop_the_writer_has_not_recorded_reads_the_provider() {
    // Something seen of the channel was not offered after the copy's claims were listed: it may
    // have been an edit or a deletion in the stretch, so the copy does not answer for it, even
    // before the writer has written the drop down.
    let d = deployment("older-dropped").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);
    let body = d.served_reload(5).await;
    let cursor = served_cursor(&body);
    d.state
        .message_cache
        .cache()
        .expect("a cache")
        .mark_suspect(&ChanKey::new(LEGACY.to_owned(), READ_CHANNEL));
    let capture = vibe_talk::testing::LogCapture::start();
    let older = d.step_back(5, &cursor).await;
    assert_eq!(texts(&older).len(), 5, "{older}");
    assert_eq!(served_from(&capture, READ_CHANNEL), ["upstream"]);
}
