//! `#208 persistent-cache`, milestone `cache-fill`: the shadow, which measures what the message
//! cache would have served while every read still goes upstream.
//!
//! Each test fills a real cache file through the server's own routes with the write token, moves
//! the cache's hand-set clock, and asks again: with the read token when only the verdict matters,
//! since a read-scope request writes nothing and so can never race its own evaluation; and with
//! the write token, its upstream read held until the evaluation beside it has read the cache, when
//! what the request's own write does next matters too.

mod slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::shadow::ConsumerCounts;
use vibe_talk::cache::{ChanKey, Incoming, Observation, Offered, Read, Scope as CacheScope};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{
    self, ManualClock, TempDir, TimelineChat, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN,
};

/// 2026-08-18T12:00:00Z: where the fixtures' messages are written, a minute apart.
const T0: i64 = 1_787_054_400_000;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const WAIT: Duration = Duration::from_secs(20);

/// The provider key a single-provider test deployment names its provider by.
const LEGACY: &str = vibe_talk::config::LEGACY_DISCORD_PROVIDER_KEY;

/// The credential that makes a live producer feed every channel.
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";

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

/// Ask, and insist on an answer: the shadow never fails a request.
async fn ok(state: &AppState, uri: &str, token: &str) -> Value {
    let (status, body) = call(state, uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

fn path(channel: &str, rest: &str) -> String {
    format!("/api/v1/channels/{channel}{rest}")
}

/// Wait for every evaluation to be counted, and for the cache's writer.
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

/// A server reading recent windows and `/timeline` through a [`TimelineChat`] (Discord's own API,
/// by the test configuration), from `text`, with twelve messages a minute apart in the read
/// channel and a cache behind it whose clock reads an hour after the first.
struct Deployment {
    state: AppState,
    fake: Arc<FakeDiscord>,
    chat: Arc<TimelineChat>,
    clock: Arc<ManualClock>,
    ids: Vec<MessageId>,
    _dir: TempDir,
}

async fn deployment(tag: &str, text: &str) -> Deployment {
    let dir = TempDir::new(tag);
    let (mut state, fake, _voice) = testing::state_from_toml(text);
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
    /// A write-token read of the read channel's newest `limit` messages, its upstream read held
    /// until the evaluation beside it has read the cache, so that it reads what the cache held
    /// before this read's own write; then that write. Returns what the request answered.
    async fn held_write(&self, limit: u16) -> Value {
        let held = self.chat.hold_next_recent();
        let request = {
            let state = self.state.clone();
            tokio::spawn(async move {
                ok(
                    &state,
                    &path(READ_CHANNEL, &format!("/messages?limit={limit}")),
                    WRITE_TOKEN,
                )
                .await
            })
        };
        held.entered.await.expect("the read reached the provider");
        assert!(
            self.state.message_cache.shadow_read(WAIT).await,
            "the evaluation read the cache"
        );
        held.release.send(()).expect("the read is waiting");
        let body = request.await.expect("the request ran");
        settled(&self.state).await;
        body
    }

    /// Fill the read channel's `main` with a write-token read of its newest window: every
    /// message, a head at the newest, and coverage up to the head's unsettled band.
    async fn fill(&self) {
        self.held_write(50).await;
        let cache = self.state.message_cache.cache().expect("installed");
        let chan = ChanKey::new(LEGACY, READ_CHANNEL);
        assert!(
            !cache
                .coverage(&chan, &CacheScope::Main)
                .served()
                .expect("coverage")
                .is_empty(),
            "the window was filled and stamped"
        );
    }

    /// A read-token window of the newest `limit` messages, counted once it is.
    async fn read_window(&self, limit: u16) {
        ok(
            &self.state,
            &path(READ_CHANNEL, &format!("/messages?limit={limit}")),
            READ_TOKEN,
        )
        .await;
        settled(&self.state).await;
    }

    fn messages(&self) -> ConsumerCounts {
        counts(&self.state, "messages")
    }
}

#[tokio::test]
async fn a_covered_head_listed_moments_ago_would_serve_and_agrees_with_upstream() {
    let d = deployment("cache-shadow-serve", &testing::config_toml()).await;
    d.fill().await;
    let filled = d.messages();
    assert_eq!(
        (filled.asked, filled.misses.gap, filled.would_serve),
        (1, 1, 0),
        "the read that filled it found nothing held, not even its own write: {filled:?}"
    );

    d.clock.advance(5 * SECOND);
    d.read_window(5).await;
    let after = d.messages();
    assert_eq!(after.asked, 2, "{after:?}");
    assert_eq!(after.would_serve, 1, "{after:?}");
    assert_eq!(
        (after.compared, after.agreed),
        (1, 1),
        "and what it would have served is what upstream answered: {after:?}"
    );
    assert_eq!(
        (after.extra_rows, after.missing_rows, after.content_differs),
        (0, 0, 0)
    );

    // Each caller of the window is counted under its own name.
    ok(&d.state, &path(READ_CHANNEL, "/digest?limit=5"), READ_TOKEN).await;
    settled(&d.state).await;
    let digest = counts(&d.state, "digest");
    assert_eq!((digest.asked, digest.would_serve), (1, 1), "{digest:?}");
    assert_eq!(d.messages().asked, 2, "not counted twice");
}

#[tokio::test]
async fn a_head_older_than_fifteen_seconds_is_stale() {
    let d = deployment("cache-shadow-stale", &testing::config_toml()).await;
    d.fill().await;
    d.clock.advance(15 * SECOND);
    d.read_window(5).await;
    assert_eq!(d.messages().would_serve, 1, "fifteen seconds is fresh");
    d.clock.advance(1);
    d.read_window(5).await;
    let counted = d.messages();
    assert_eq!(counted.misses.stale_head, 1, "{counted:?}");
    assert_eq!(counted.would_serve, 1);
    assert_eq!(counted.compared, 1, "a miss is not compared");
}

#[tokio::test]
async fn a_live_fed_channels_head_is_stale_only_past_a_minute() {
    let text = format!(
        "{}\n[ingest]\ntoken = \"{INGEST_TOKEN}\"\n",
        testing::config_toml()
    );
    let d = deployment("cache-shadow-stale-fed", &text).await;
    d.fill().await;
    d.clock.advance(16 * SECOND);
    d.read_window(5).await;
    assert_eq!(
        d.messages().would_serve,
        1,
        "a fed channel's head is fresh at sixteen seconds"
    );
    d.clock.advance(44 * SECOND);
    d.read_window(5).await;
    assert_eq!(d.messages().would_serve, 2, "and at a minute");
    d.clock.advance(1);
    d.read_window(5).await;
    let counted = d.messages();
    assert_eq!(counted.misses.stale_head, 1, "{counted:?}");
}

#[tokio::test]
async fn a_gap_a_suspect_channel_and_a_blocked_one_each_miss() {
    let d = deployment("cache-shadow-misses", &testing::config_toml()).await;
    d.fill().await;
    d.clock.advance(SECOND);
    let cache = d.state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);

    // Wider than the channel: a window of fewer messages than asked is the whole channel only
    // from its start, which no coverage reaches.
    d.read_window(20).await;
    assert_eq!(d.messages().misses.gap, 2, "{:?}", d.messages());
    // A channel never read has no head at all.
    ok(
        &d.state,
        &path(WRITE_CHANNEL, "/messages?limit=5"),
        READ_TOKEN,
    )
    .await;
    settled(&d.state).await;
    assert_eq!(d.messages().misses.gap, 3);

    // An offer too big for the queue is dropped, and its channel's head is no longer trusted.
    let huge = Message {
        edited_timestamp: None,
        mentions: Vec::new(),
        upstream_url: None,
        thread: None,
        id: MessageId::at_time_ms(T0 + 30 * SECOND),
        channel_id: ChannelId(READ_CHANNEL.to_owned()),
        author: "Ada".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: vibe_talk::clock::iso_from_ms(T0 + 30 * SECOND),
        spoken_time: String::new(),
        reply_to: None,
        content: "x".repeat(17 * 1024 * 1024),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
    };
    let offered = cache.offer(
        chan.clone(),
        cache.stamp(&chan),
        Observation::Rows {
            rows: vec![Incoming::new(huge, CacheScope::Main)],
            scope: None,
            observed_ms: d.clock.now(),
        },
    );
    assert_eq!(offered, Offered::Dropped);
    assert!(cache.is_suspect(&chan));
    d.read_window(5).await;
    assert_eq!(d.messages().misses.suspect, 1, "{:?}", d.messages());

    // A newest read begun since the drop trusts the head again, though its own evaluation, made
    // before its write, still found the channel suspect; access in doubt blocks it.
    d.clock.advance(SECOND);
    d.fill().await;
    assert_eq!(d.messages().misses.suspect, 2, "{:?}", d.messages());
    assert!(!cache.is_suspect(&chan));
    d.clock.advance(SECOND);
    cache.block_channel(&chan);
    d.read_window(5).await;
    assert_eq!(d.messages().misses.blocked, 1, "{:?}", d.messages());
    cache.unblock(&chan, None);
    d.read_window(5).await;
    let counted = d.messages();
    assert_eq!(counted.would_serve, 1, "{counted:?}");
    assert_eq!(
        counted.asked,
        counted.would_serve
            + counted.misses.gap
            + counted.misses.suspect
            + counted.misses.blocked
            + counted.late,
        "every evaluation is counted once: {counted:?}"
    );
}

#[tokio::test]
async fn with_the_cache_off_an_ask_is_counted_off_and_without_a_table_nothing_is_counted() {
    // The configuration turns the cache off: nothing is open, and every ask is a miss, `off`.
    let text = format!(
        "{}\n[message_cache]\nenabled = false\n",
        testing::config_toml()
    );
    let (mut state, fake, _voice) = testing::state_from_toml(&text);
    fake.seed(&ChannelId(READ_CHANNEL.to_owned()), "Ada", "hello");
    state.replace_chat(Arc::new(TimelineChat::new(fake.clone())));
    vibe_talk::cache::lifecycle::start(&state.config, &state.message_cache).await;
    state.message_cache.arm();
    state.refresh_routes();
    assert!(state.message_cache.cache().is_none());
    ok(&state, &path(READ_CHANNEL, "/messages"), READ_TOKEN).await;
    ok(&state, &path(READ_CHANNEL, "/count"), READ_TOKEN).await;
    ok(
        &state,
        &path(READ_CHANNEL, "/timeline?view=main"),
        READ_TOKEN,
    )
    .await;
    // The thread list is never answered from the cache, so it is not a miss of one.
    ok(
        &state,
        &path(READ_CHANNEL, "/timeline?view=threads"),
        READ_TOKEN,
    )
    .await;
    settled(&state).await;
    let messages = counts(&state, "messages");
    assert_eq!(
        (messages.asked, messages.misses.off),
        (1, 1),
        "{messages:?}"
    );
    assert_eq!(counts(&state, "count").misses.off, 1);
    let timeline = counts(&state, "timeline_fallback");
    assert_eq!(
        (timeline.asked, timeline.misses.off),
        (1, 1),
        "{timeline:?}"
    );

    // No table at all, the default: nothing is evaluated and nothing counted.
    let (state, fake, _voice) = testing::state_from_toml(&testing::config_toml());
    fake.seed(&ChannelId(READ_CHANNEL.to_owned()), "Ada", "hello");
    vibe_talk::cache::lifecycle::start(&state.config, &state.message_cache).await;
    state.message_cache.arm();
    state.refresh_routes();
    ok(&state, &path(READ_CHANNEL, "/messages"), READ_TOKEN).await;
    ok(&state, &path(READ_CHANNEL, "/messages"), WRITE_TOKEN).await;
    settled(&state).await;
    assert!(
        state
            .message_cache
            .shadow_counts()
            .iter()
            .all(|counts| counts
                == &ConsumerCounts {
                    consumer: counts.consumer,
                    ..ConsumerCounts::default()
                }),
        "{:?}",
        state.message_cache.shadow_counts()
    );
}

#[tokio::test]
async fn an_upstream_deleted_row_the_cache_still_holds_counts_extra_and_the_write_through_then_tombstones_it(
) {
    let d = deployment("cache-shadow-extra", &testing::config_toml()).await;
    d.fill().await;
    let cache = d.state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);
    let deleted = d.ids[5].clone();
    d.fake.delete(&deleted);
    d.clock.advance(5 * SECOND);
    assert_eq!(cache.tombstoned(&chan, &deleted), Read::Served(false));

    // A write-token window: the evaluation reads the cache before this read writes to it.
    let body = d.held_write(10).await;
    let returned: Vec<&str> = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(!returned.contains(&deleted.as_str()), "upstream's answer");

    let counted = d.messages();
    assert_eq!(counted.would_serve, 1, "{counted:?}");
    assert_eq!(counted.compared, 1, "{counted:?}");
    assert_eq!(
        (counted.extra_rows, counted.missing_rows, counted.agreed),
        (1, 0, 0),
        "served, the cache would have brought the deleted message back: {counted:?}"
    );
    // The same read, written through, settles it.
    assert_eq!(
        cache.tombstoned(&chan, &deleted),
        Read::Served(true),
        "the write-through tombstoned it"
    );
    assert_eq!(
        cache.message(&chan, &deleted),
        Read::Served(None),
        "and it is no longer held"
    );
}

const SLACK: &str = "slack";
const SLACK_CH: &str = "C0000MAIN1";

fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1_000
}

#[tokio::test]
async fn slacks_message_by_id_beyond_two_hours_needs_an_existence_check() {
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
    let dir = TempDir::new("cache-shadow-slack");
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

    // A write-token Main page: Slack proves the channel's start, so its oldest messages are
    // covered, revalidated as of the read.
    ok(
        &state,
        &path(SLACK_CH, "/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    settled(&state).await;
    let id = vibe_talk::slack::ids::message_id_from_ts(&slack_fake::ts(60, 0)).expect("an id");
    let by_id = path(SLACK_CH, &format!("/messages/{}", id.as_str()));

    clock.advance(HOUR);
    ok(&state, &by_id, READ_TOKEN).await;
    settled(&state).await;
    let counted = counts(&state, "message_by_id");
    assert_eq!(
        (counted.asked, counted.would_serve, counted.agreed),
        (1, 1, 1),
        "revalidated an hour ago: served as it stands: {counted:?}"
    );

    // Slack's feed carries no deletions: past two hours the message may be gone, and serving it
    // would take one call upstream to say it is not.
    clock.advance(HOUR + SECOND);
    ok(&state, &by_id, READ_TOKEN).await;
    settled(&state).await;
    let counted = counts(&state, "message_by_id");
    assert_eq!(counted.asked, 2, "{counted:?}");
    assert_eq!(counted.needs_existence_check, 1, "{counted:?}");
    assert_eq!(counted.would_serve, 1, "{counted:?}");
}

#[tokio::test]
async fn a_summary_that_reads_upstream_is_shadowed_for_its_message_and_counted_as_a_summary() {
    let d = deployment("cache-shadow-summary", &testing::config_toml()).await;
    d.fill().await;
    d.clock.advance(5 * SECOND);
    // Not in the server's memory any more, so the summary reads the channel again.
    d.state.read_ahead.clear();
    let target = d.ids[4].clone();
    // Whatever the summariser makes of it: the read is what is shadowed.
    let _ = vibe_talk::ops::summarize_message(
        &d.state,
        vibe_talk::auth::Scope::Read,
        READ_CHANNEL,
        target.as_str(),
        &[],
        Some(10),
    )
    .await;
    settled(&d.state).await;
    let summary = counts(&d.state, "summary");
    assert_eq!(summary.asked, 1, "{summary:?}");
    assert_eq!(
        (summary.would_serve, summary.agreed),
        (1, 1),
        "held, covered and revalidated moments ago, as upstream has it: {summary:?}"
    );
    assert_eq!(d.messages().asked, 1, "only the fill: {:?}", d.messages());
}

#[tokio::test]
async fn a_page_is_never_served_from_a_claim_whose_rows_the_week_took() {
    let d = deployment("cache-shadow-week", &testing::config_toml()).await;
    d.fill().await;
    let cache = d.state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);
    // Eight days on, nothing having read the channel again: the week takes its rows.
    d.clock.advance(8 * 24 * HOUR);
    cache.maintain(vibe_talk::cache::Maintenance::Hourly);
    settled(&d.state).await;
    assert_eq!(
        cache.message(&chan, &d.ids[2]),
        Read::Served(None),
        "the week took the row"
    );
    let iso = vibe_talk::clock::iso_from_ms;
    ok(
        &d.state,
        &path(
            READ_CHANNEL,
            &format!("/page?since={}&until={}", iso(T0), iso(T0 + 5 * MINUTE)),
        ),
        READ_TOKEN,
    )
    .await;
    settled(&d.state).await;
    let page = counts(&d.state, "page");
    assert_eq!(
        (page.would_serve, page.missing_rows),
        (0, 0),
        "a page served from a claim whose rows are gone: {page:?}, claims {:?}",
        cache.coverage(&chan, &CacheScope::Main)
    );
    assert_eq!(page.misses.gap, 1, "{page:?}");
}

/// An hour after the window was read whole, a message in it is edited upstream (Discord's feed
/// carries no edits), and a write-token read of the newest three makes the head fresh again. A
/// read-token window of ten still rests on the claim from an hour ago: it is stale, not served
/// with the old text. The three the short read re-read are fresh.
#[tokio::test]
async fn a_short_read_of_the_newest_end_never_makes_an_hour_old_window_fresh() {
    let d = deployment("cache-shadow-short-edit", &testing::config_toml()).await;
    d.fill().await;
    d.clock.advance(HOUR);
    d.fake.edit(&d.ids[3], "message 3, edited an hour later");
    d.held_write(3).await;
    let before = d.messages();
    d.clock.advance(5 * SECOND);
    d.read_window(10).await;
    let counted = d.messages();
    assert_eq!(
        (
            counted.would_serve - before.would_serve,
            counted.content_differs
        ),
        (0, 0),
        "a window an hour old counted as fresh, and its text differs from upstream: {counted:?}"
    );
    assert_eq!(
        counted.misses.stale_head - before.misses.stale_head,
        1,
        "{counted:?}"
    );
    d.read_window(3).await;
    let counted = d.messages();
    assert_eq!(
        (counted.would_serve - before.would_serve, counted.agreed),
        (1, 1),
        "what the short read re-read is fresh: {counted:?}"
    );
}

/// The same with a deletion: a message deleted upstream an hour after the window was read whole
/// is never counted as one the cache would serve.
#[tokio::test]
async fn a_short_read_of_the_newest_end_never_makes_a_deleted_message_servable() {
    let d = deployment("cache-shadow-short-delete", &testing::config_toml()).await;
    d.fill().await;
    d.clock.advance(HOUR);
    d.fake.delete(&d.ids[4]);
    d.held_write(3).await;
    let before = d.messages();
    d.clock.advance(5 * SECOND);
    d.read_window(10).await;
    let counted = d.messages();
    assert_eq!(
        (counted.would_serve - before.would_serve, counted.extra_rows),
        (0, 0),
        "a message deleted upstream counted as servable: {counted:?}"
    );
}

/// Only part of a stretch is read again within the week: three days on, a write-token page re-reads
/// messages 5 to 8. Five days later the week takes the rows of the part that was not, and takes
/// that part's claim with them; the part read again stands, rows and claim.
#[tokio::test]
async fn a_partly_revalidated_stretch_keeps_only_the_claim_that_was_read_again() {
    let d = deployment("cache-shadow-week-part", &testing::config_toml()).await;
    d.fill().await;
    let iso = vibe_talk::clock::iso_from_ms;
    let page = |since: i64, until: i64| {
        path(
            READ_CHANNEL,
            &format!("/page?since={}&until={}", iso(since), iso(until)),
        )
    };
    d.clock.advance(3 * 24 * HOUR);
    ok(
        &d.state,
        &page(T0 + 5 * MINUTE, T0 + 9 * MINUTE),
        WRITE_TOKEN,
    )
    .await;
    settled(&d.state).await;
    d.clock.advance(5 * 24 * HOUR);
    let cache = d.state.message_cache.cache().expect("installed");
    cache.maintain(vibe_talk::cache::Maintenance::Hourly);
    settled(&d.state).await;
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);
    let claims = cache.coverage(&chan, &CacheScope::Main);
    let before = counts(&d.state, "page");
    ok(&d.state, &page(T0, T0 + 4 * MINUTE), READ_TOKEN).await;
    settled(&d.state).await;
    let after = counts(&d.state, "page");
    assert_eq!(
        (
            after.would_serve - before.would_serve,
            after.missing_rows - before.missing_rows
        ),
        (0, 0),
        "the week took rows 0 to 4 but a claim still served them: {after:?}; claims {claims:?}"
    );
    ok(
        &d.state,
        &page(T0 + 5 * MINUTE, T0 + 8 * MINUTE),
        READ_TOKEN,
    )
    .await;
    settled(&d.state).await;
    let last = counts(&d.state, "page");
    assert_eq!(
        (
            last.would_serve - after.would_serve,
            last.agreed - after.agreed
        ),
        (1, 1),
        "the stretch read again: {last:?}; claims {claims:?}"
    );
}

/// A count, not a window. An hour after the window was read whole, message 4 is deleted upstream,
/// and a short write-token read of the newest three makes the head fresh. A read-token count from
/// the first message still rests on the hour-old claim, and is not one the cache would serve.
#[tokio::test]
async fn a_short_newest_read_never_makes_an_hour_old_count_fresh() {
    let d = deployment("cache-shadow-count-hour", &testing::config_toml()).await;
    d.fill().await;
    d.clock.advance(HOUR);
    d.fake.delete(&d.ids[4]);
    d.held_write(3).await;
    d.clock.advance(5 * SECOND);
    let iso = vibe_talk::clock::iso_from_ms;
    ok(
        &d.state,
        &path(READ_CHANNEL, &format!("/count?since={}", iso(T0))),
        READ_TOKEN,
    )
    .await;
    settled(&d.state).await;
    let count = counts(&d.state, "count");
    assert_eq!(
        (count.would_serve, count.extra_rows),
        (0, 0),
        "an hour-old count counted as fresh, with a deleted message in it: {count:?}"
    );
}

/// Message 10 lies in the head's unsettled band. It is deleted (Discord's feed carries no
/// deletions), and a write-token read of the whole window five seconds later, the latest read of
/// the newest end, does not list it; inside the band its absence is no proof yet, so it stays in
/// the file. What the latest read did not list is vouched for by nothing: a read-token window of
/// five does not serve it, and neither does a count, which reads the band as a window does.
#[tokio::test]
async fn a_band_row_the_latest_read_did_not_list_is_never_served_in_a_window_nor_counted() {
    let d = deployment("cache-shadow-band-count", &testing::config_toml()).await;
    d.fill().await;
    d.fake.delete(&d.ids[10]);
    d.clock.advance(5 * SECOND);
    d.held_write(50).await;
    let cache = d.state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);
    assert!(
        matches!(cache.message(&chan, &d.ids[10]), Read::Served(Some(_))),
        "inside the band an absence proves nothing yet"
    );
    d.clock.advance(5 * SECOND);
    let before = d.messages();
    d.read_window(5).await;
    let after = d.messages();
    assert_eq!(
        (
            after.would_serve - before.would_serve,
            after.extra_rows - before.extra_rows
        ),
        (0, 0),
        "a window: {after:?}"
    );
    let iso = vibe_talk::clock::iso_from_ms;
    let body = ok(
        &d.state,
        &path(READ_CHANNEL, &format!("/count?since={}", iso(T0))),
        READ_TOKEN,
    )
    .await;
    settled(&d.state).await;
    let count = counts(&d.state, "count");
    assert_eq!(
        (count.would_serve, count.extra_rows),
        (0, 0),
        "upstream answered {body}; the count shadow: {count:?}"
    );
    assert_eq!(count.misses.gap, 1, "{count:?}");
}
