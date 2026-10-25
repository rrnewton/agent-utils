//! `#384 cache-serve` (of `#208 persistent-cache`, milestone M9): reads answered from the
//! message cache at once, refreshed from the provider in the background.
//!
//! Each test fills a real cache file through the server's own routes with the write token,
//! turns serving on (`Sink::set_serve`, what `[message_cache] serve = true` sets at startup),
//! and then holds the provider's next read at the fake: a provider that has taken its answer
//! and does not hand it back is the slow provider of the owner's reload, without the wait.
//! A served read must answer while that read is still held; a read that may not be served
//! must still be waiting when the hold is released.

mod slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::shadow::ConsumerCounts;
use vibe_talk::cache::sink::Verification;
use vibe_talk::cache::ChanKey;
use vibe_talk::chat::ChatError;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::live::LiveKind;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{
    self, CountingChat, ManualClock, TempDir, TimelineChat, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN,
};

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

#[tokio::test]
async fn a_served_pages_next_full_read_fetches_the_walk_back_cursor() {
    // A served page cannot mint the provider's backward cursor, so it says more exists
    // without one — and the view is then not served again until a caller holds a real page.
    // Otherwise every later load would serve afresh and older history would stay unreachable.
    let d = deployment("cache-serve-cursor").await;
    d.fill_timeline().await;
    d.clock.advance(2 * HOUR);
    d.state.message_cache.set_serve(true);

    let held = d.chat.hold_next_timeline();
    let body = tokio::time::timeout(AT_ONCE, d.spawn_timeline(5))
        .await
        .expect("the reload answered at once")
        .expect("the request ran");
    assert_eq!(body["has_more"], true, "{body}");
    assert!(body["next_before"].is_null(), "no minted cursor: {body}");
    assert_eq!(body["has_threads"], false, "{body}");
    let after = body["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    held.entered
        .await
        .expect("the refresh reached the provider");
    held.release.send(()).expect("the refresh is waiting");
    settled(&d.state).await;

    // The forward cursor a served page hands out is one a delta can continue from.
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

    // The next full read of the view goes upstream, refresh or no: it fetches the page whose
    // cursor the reader walks back on.
    let held = d.chat.hold_next_timeline();
    let mut request = d.spawn_timeline(5);
    assert!(
        tokio::time::timeout(AT_ONCE, &mut request).await.is_err(),
        "the view owed a real page is not served again"
    );
    held.release.send(()).expect("the read is waiting");
    let body = request.await.expect("the request ran");
    assert!(
        body["next_before"].is_string(),
        "the provider's own backward cursor: {body}"
    );
    settled(&d.state).await;
    assert_eq!(counts(&d.state, "timeline_fallback").served, 1);

    // With a real page handed out, serving resumes. Time passes first, as it would: a
    // snapshot holding what the direct read just wrote, at the very instant of the ask,
    // is counted late and never served on.
    d.clock.advance(SECOND);
    let held = d.chat.hold_next_timeline();
    let body = tokio::time::timeout(AT_ONCE, d.spawn_timeline(5))
        .await
        .expect("answered from the cache again")
        .expect("the request ran");
    assert!(body["next_before"].is_null(), "{body}");
    held.release.send(()).expect("the refresh is waiting");
    settled(&d.state).await;
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
    // read behind it — here the next full read, which the served view owes a real page —
    // shows the correction.
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
    settled(&state).await;
    assert_eq!(counts(&state, "timeline_fallback").served, 1);
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
}
