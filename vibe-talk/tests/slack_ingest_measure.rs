//! `#452 slack-live-ingest` and `#453 cache-backfill`, measured against the loopback Slack fake:
//! the backfill's reads and its yielding, the shadow's servable rate for the newest timeline page
//! with and without the relist and the backfill, and how long a message takes from the stream to
//! the live hub by a content-free hint and by a pushed record.
//!
//! The functional tests run in the ordinary suite. The two measurements print their figures and
//! are `#[ignore]`d, because they take seconds of wall-clock time and their numbers, not a pass,
//! are the point: `cargo test --test slack_ingest_measure -- --ignored --nocapture`.

mod slack_fake;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::model::ChannelId;
use vibe_talk::providers::ChatRouter;
use vibe_talk::relist::{Backfill, BackfillStep, RelistOptions};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, ManualClock, TempDir, READ_TOKEN, WRITE_TOKEN};

const CH: &str = "C0000MAIN1";
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";
const LIMIT: u16 = 50;
const WAIT: Duration = Duration::from_secs(20);

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

fn wall_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs(),
    )
    .expect("a sane clock")
}

/// `count` messages a minute apart, the newest `newest_ago` seconds ago.
fn history(count: i64, newest_ago: i64) -> FakeState {
    let mut state = FakeState::default();
    let base = wall_secs() - newest_ago - (count - 1) * 60;
    let list: Vec<Value> = (0..count)
        .map(|i| {
            profiled(
                &format!("{}.000000", base + i * 60),
                "UADA00001",
                "Ada",
                &format!("message {i}"),
            )
        })
        .collect();
    state.history.insert(CH.to_owned(), list);
    state
}

fn config(fake: &Fake, cache_table: &str, poll_seconds: u64) -> String {
    format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[ingest]
token = "{INGEST_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
live_poll_seconds = {poll_seconds}

[[channels]]
id = "{CH}"
label = "team"
writable = true
{cache_table}
"#,
        base = fake.base,
    )
}

fn deployment(fake: &Fake, cache_table: &str, poll_seconds: u64) -> AppState {
    let (mut state, _discord, _store) =
        testing::state_with_store_from_toml(&config(fake, cache_table, poll_seconds));
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

async fn cached(fake: &Fake, tag: &str, table: &str) -> (AppState, Arc<ManualClock>, TempDir) {
    let dir = TempDir::new(tag);
    let clock = ManualClock::at(wall_secs() * 1_000);
    let state = deployment(fake, table, 3600);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    (state, clock, dir)
}

fn held(state: &AppState) -> u64 {
    let cache = state.message_cache.cache().expect("a cache");
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.counts().served().expect("counts").messages
}

async fn request(
    state: &AppState,
    method: &str,
    uri: &str,
    token: &str,
    body: &Value,
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let body = if method == "POST" {
        builder = builder.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = vibe_talk::http::router(state.clone())
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("the router answers");
    let status = response.status();
    let _ = response.into_body().collect().await;
    status
}

fn record(seq: u64, ts: &str, text: &str) -> Value {
    json!({
        "type": "message.created",
        "iris_seq_id": seq,
        "payload": {"container_id": CH, "slack_message": {
            "channel_id": CH, "message_ts": ts, "slack_user_id": "UADA00001", "text": text}},
    })
}

/// Add one message to the fake now, and return its `ts`.
fn arrive(fake: &Fake, n: usize, micros: i64) -> String {
    let ts = format!("{}.{micros:06}", wall_secs());
    fake.with(|fake| {
        fake.history.get_mut(CH).expect("history").push(profiled(
            &ts,
            "UADA00001",
            "Ada",
            &format!("arrival {n}"),
        ));
    });
    ts
}

#[tokio::test]
async fn the_backfill_walks_older_pages_into_the_cache_and_off_makes_no_call() {
    let fake = Fake::start(history(120, 60), 20).await;
    let (state, _clock, _dir) = cached(
        &fake,
        "backfill-walk",
        "\n[message_cache]\nbackfill = \"adaptive\"\n",
    )
    .await;
    let before = fake.calls("conversations.history");
    let mut backfill = Backfill::new();
    let mut reads = 0;
    loop {
        match backfill.step(&state, LIMIT, Duration::ZERO).await {
            BackfillStep::Read => reads += 1,
            BackfillStep::Idle => break,
            other => panic!("unexpected step {other:?}"),
        }
        assert!(reads <= vibe_talk::relist::BACKFILL_PAGES, "the walk ends");
    }
    // Pages of 50 rows (each read through as many 20-row bridge calls as it takes): three hold
    // all 120 messages, and the third says nothing is older.
    assert_eq!(reads, 3);
    assert!(fake.calls("conversations.history") - before >= reads);
    assert_eq!(held(&state), 120);
    assert_eq!(
        backfill.step(&state, LIMIT, Duration::ZERO).await,
        BackfillStep::Idle
    );
    backfill.rewalk();
    assert_eq!(
        backfill.step(&state, LIMIT, Duration::ZERO).await,
        BackfillStep::Read
    );

    // Off: no call at all.
    let off_fake = Fake::start(history(10, 60), 20).await;
    let (off, _clock, _dir) = cached(
        &off_fake,
        "backfill-off",
        "\n[message_cache]\nbackfill = \"off\"\n",
    )
    .await;
    let calls = off_fake.calls("conversations.history");
    assert_eq!(
        Backfill::new().step(&off, LIMIT, Duration::ZERO).await,
        BackfillStep::Off
    );
    assert_eq!(off_fake.calls("conversations.history"), calls);
}

#[tokio::test]
async fn the_backfill_yields_while_anything_else_reads_the_provider() {
    let mut fixture = history(10, 60);
    fixture.delay = Some(Duration::from_millis(600));
    let fake = Fake::start(fixture, 20).await;
    let (state, _clock, _dir) = cached(
        &fake,
        "backfill-yield",
        "\n[message_cache]\nbackfill = \"adaptive\"\n",
    )
    .await;
    // A page's read is in flight: the backfill does not add its own.
    let reader = {
        let state = state.clone();
        tokio::spawn(async move {
            request(
                &state,
                "GET",
                &format!("/api/v1/channels/{CH}/timeline?view=main&limit=50"),
                WRITE_TOKEN,
                &Value::Null,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut backfill = Backfill::new();
    assert_eq!(
        backfill.step(&state, LIMIT, Duration::from_secs(2)).await,
        BackfillStep::Busy
    );
    assert_eq!(reader.await.expect("joined"), StatusCode::OK);
    // Just after it, still busy: the provider has not been idle for the margin yet.
    assert_eq!(
        backfill.step(&state, LIMIT, Duration::from_secs(2)).await,
        BackfillStep::Busy
    );
    assert_eq!(
        backfill.step(&state, LIMIT, Duration::ZERO).await,
        BackfillStep::Read
    );
}

/// The shadow's count for the newest timeline page.
fn timeline_counts(state: &AppState) -> vibe_talk::cache::shadow::ConsumerCounts {
    state
        .message_cache
        .shadow_counts()
        .into_iter()
        .find(|counts| counts.consumer == "timeline_fallback")
        .expect("the timeline consumer")
}

async fn settled(state: &AppState) {
    assert!(state.message_cache.shadow_settled(WAIT).await);
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(WAIT));
    }
}

/// What a page's own write-scope read of the newest Main page writes to the cache, without the
/// shadow evaluation beside it: the fill every reader's read makes.
async fn page_fill(state: &AppState) {
    let request = vibe_talk::threads::TimelineRequest {
        limit: LIMIT,
        ..vibe_talk::threads::TimelineRequest::default()
    };
    let fill = state
        .message_cache
        .begin_for(vibe_talk::auth::Scope::Write, &channel());
    let page =
        vibe_talk::chat::ChatClient::fetch_timeline(state.providers.as_ref(), &channel(), &request)
            .await
            .expect("the page reads");
    if let Some(fill) = fill {
        fill.offer_timeline(
            &request,
            vibe_talk::cache::observe::TimelineRead::Newest,
            &page,
        );
    }
}

/// One workload: `rounds` times, a message arrives (pushed by the stream) and then a reader asks
/// for the newest Main page, whose verdict the shadow counts (asked with the read token, so the
/// ask's own read writes nothing it could race); the reader's own write-scope read then fills the
/// cache, as a page's does. With `backfill`, the backfill walks first; with `relist`, the relist
/// runs between each arrival and the ask, as it does whenever its interval is shorter than the
/// reader's.
async fn servable(
    tag: &str,
    backfill: bool,
    relist: bool,
    rounds: usize,
) -> vibe_talk::cache::shadow::ConsumerCounts {
    let fake = Fake::start(history(200, 3_600), 100).await;
    let (state, clock, _dir) =
        cached(&fake, tag, "\n[message_cache]\nbackfill = \"adaptive\"\n").await;
    let mut cursor = None;
    vibe_talk::live::poll_once(
        state.chat.as_ref(),
        &state.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("the seed");
    page_fill(&state).await;
    if backfill {
        let mut walk = Backfill::new();
        while walk.step(&state, LIMIT, Duration::ZERO).await == BackfillStep::Read {}
    }
    let timeline = format!("/api/v1/channels/{CH}/timeline?view=main&limit=50");
    for round in 0..rounds {
        clock.advance(30_000);
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let ts = arrive(&fake, round, 100 + i64::try_from(round).expect("small"));
        let status = request(
            &state,
            "POST",
            "/api/v1/live/tail",
            INGEST_TOKEN,
            &record(
                u64::try_from(round).expect("small") + 1,
                &ts,
                &format!("arrival {round}"),
            ),
        )
        .await;
        assert!(status.is_success(), "{status}");
        clock.advance(1_000);
        if relist {
            vibe_talk::relist::relist_once(
                &state,
                None,
                RelistOptions {
                    limit: LIMIT,
                    threads: 4,
                    interval: Duration::from_secs(60),
                    settle: Duration::ZERO,
                },
                wall_secs() * 1_000,
            )
            .await;
        }
        settled(&state).await;
        // The cache's clock is the test's: the ask comes a second after what was written for it,
        // as it would by any real clock.
        clock.advance(1_000);
        assert_eq!(
            request(&state, "GET", &timeline, READ_TOKEN, &Value::Null).await,
            StatusCode::OK
        );
        settled(&state).await;
        page_fill(&state).await;
        settled(&state).await;
    }
    timeline_counts(&state)
}

#[tokio::test]
#[ignore = "a measurement: run with --ignored --nocapture"]
async fn measure_the_servable_rate_of_the_newest_timeline_page() {
    let rounds = 10;
    let mut rates = Vec::new();
    for (name, backfill, relist) in [
        ("fill-only", false, false),
        ("backfill", true, false),
        ("relist", false, true),
        ("relist+backfill", true, true),
    ] {
        let counts = servable(&format!("measure-{name}"), backfill, relist, rounds).await;
        println!(
            "timeline_fallback {name:>16}: {}/{} servable, {} agreed; misses {:?}",
            counts.would_serve, counts.asked, counts.agreed, counts.misses
        );
        rates.push(counts.would_serve);
    }
    assert!(
        rates[3] > rates[0],
        "the relist and the backfill make more reads servable"
    );
}

/// How long one message takes from its stream record reaching vibe-talk to the live hub, by
/// hint (the poller reads the channel through the bridge) and by pushed record.
async fn arrival_to_hub(
    push: bool,
    delay: Duration,
    messages: usize,
    gap: Duration,
) -> Vec<Duration> {
    let mut fixture = history(5, 120);
    fixture.delay = Some(delay);
    let fake = Fake::start(fixture, 100).await;
    let state = deployment(&fake, "", 3600);
    let poller = tokio::spawn(vibe_talk::live::poll_forever(
        state.clone(),
        None,
        Arc::clone(&state.providers.entries()[0].client),
        LIMIT,
        Duration::from_secs(3600),
    ));
    // Let the poller seed before anything arrives.
    let deadline = Instant::now() + WAIT;
    while state.live.seeded_from(&channel()).is_none() {
        assert!(Instant::now() < deadline, "the poller seeded");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut subscription = state.live.subscribe(&channel(), None);
    let mut latencies = Vec::new();
    for n in 0..messages {
        let ts = arrive(&fake, n, 500 + i64::try_from(n).expect("small"));
        let started = Instant::now();
        let status = if push {
            request(
                &state,
                "POST",
                "/api/v1/live/tail",
                INGEST_TOKEN,
                &record(
                    u64::try_from(n).expect("small") + 1,
                    &ts,
                    &format!("arrival {n}"),
                ),
            )
            .await
        } else {
            request(
                &state,
                "POST",
                "/api/v1/live/hints",
                INGEST_TOKEN,
                &json!({"channel_id": CH}),
            )
            .await
        };
        assert!(status.is_success(), "{status}");
        let id = vibe_talk::slack::message_id_from_ts(&ts).expect("an id");
        loop {
            let live = tokio::time::timeout(WAIT, subscription.receiver.recv())
                .await
                .expect("published in time")
                .expect("the hub is open");
            if live.message.id == id {
                break;
            }
        }
        latencies.push(started.elapsed());
        tokio::time::sleep(gap).await;
    }
    poller.abort();
    latencies
}

fn summary(latencies: &[Duration]) -> String {
    let mut ms: Vec<u128> = latencies.iter().map(Duration::as_millis).collect();
    ms.sort_unstable();
    format!(
        "n={} min={}ms median={}ms max={}ms",
        ms.len(),
        ms[0],
        ms[ms.len() / 2],
        ms[ms.len() - 1]
    )
}

#[tokio::test]
#[ignore = "a measurement: run with --ignored --nocapture"]
async fn measure_arrival_to_visible_by_hint_and_by_pushed_record() {
    for delay_ms in [0_u64, 300, 1_500] {
        let delay = Duration::from_millis(delay_ms);
        for gap_ms in [6_000_u64, 1_000] {
            let gap = Duration::from_millis(gap_ms);
            let hinted = arrival_to_hub(false, delay, 4, gap).await;
            let pushed = arrival_to_hub(true, delay, 4, gap).await;
            println!(
                "bridge read {delay_ms:>5}ms, arrivals {gap_ms:>5}ms apart: hint {} | pushed {}",
                summary(&hinted),
                summary(&pushed)
            );
        }
    }
}
