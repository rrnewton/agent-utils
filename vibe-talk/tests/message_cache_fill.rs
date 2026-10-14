//! `#208 persistent-cache`, milestone `cache-fill`: what reaches the message cache, and what is
//! swept out of it.
//!
//! Every test opens a real cache file in its own directory, behind the server's sink, with a
//! hand-set clock placed near its fixture's times: the cache refuses rows older than its age limit
//! by its own clock, and judges which of two reads is newer by the times it was given.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::{ChanKey, Incoming, MessageCache, Observation, Scope as CacheScope};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::state::AppState;
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::{StateStore as _, SummaryKey};
use vibe_talk::testing::{self, ManualClock, TempDir, READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};

/// 2026-08-18T12:00:00Z: where the fixtures' messages are written.
const T0: i64 = 1_787_054_400_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const WAIT: Duration = Duration::from_secs(20);

/// The provider key a single-provider test deployment names its provider by.
const LEGACY: &str = vibe_talk::config::LEGACY_DISCORD_PROVIDER_KEY;

/// The credential an adapter pushes live changes with.
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";

fn id(channel: &str) -> ChannelId {
    ChannelId(channel.to_owned())
}

/// A message in `channel` really written at `at_ms`, its id and timestamp agreeing.
fn message(channel: &str, at_ms: i64, text: &str) -> Message {
    Message {
        edited_timestamp: None,
        mentions: Vec::new(),
        thread: None,
        id: MessageId::at_time_ms(at_ms),
        channel_id: id(channel),
        author: "Ada".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: vibe_talk::clock::iso_from_ms(at_ms),
        spoken_time: String::new(),
        reply_to: None,
        content: text.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
    }
}

/// Offer `count` rows of `channel`, a minute apart, straight to the cache, as a read of it would,
/// and wait until they are applied.
fn offer_rows(cache: &MessageCache, provider: &str, channel: &str, count: i64, now_ms: i64) {
    let chan = ChanKey::new(provider, channel);
    let rows = (0..count)
        .map(|n| {
            Incoming::new(
                message(channel, T0 + n * MINUTE, &format!("row {n}")),
                CacheScope::Main,
            )
        })
        .collect();
    cache.offer(
        chan.clone(),
        cache.stamp(&chan),
        Observation::Rows {
            rows,
            scope: None,
            observed_ms: now_ms,
        },
    );
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
}

/// Every message the file holds, once the writer has finished what it was asked.
fn held(cache: &MessageCache) -> u64 {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.counts().served().expect("counts").messages
}

/// Close the cache behind `state`, so the next server can open the same file.
fn close(state: &AppState, cache: Arc<MessageCache>) {
    assert!(cache.quiesce_blocking(WAIT));
    drop(cache);
    drop(state.message_cache.uninstall());
}

async fn call(state: &AppState, method: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
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

/// The test configuration with the writable channel taken out of it.
fn config_without_the_write_channel() -> String {
    let text = testing::config_toml();
    let block =
        format!("[[channels]]\nid = \"{WRITE_CHANNEL}\"\nlabel = \"lead team\"\nwritable = true\n");
    assert!(text.contains(&block), "the fixture still names the channel");
    text.replace(&block, "")
}

/// A deployment of one Discord-protocol provider under `key`, routed through a router that really
/// names it so, and the fake behind it.
fn keyed_deployment(key: &str) -> (AppState, Arc<vibe_talk::discord::fake::FakeDiscord>) {
    let (state, fake, _store) = keyed_deployment_of(key, true);
    (state, fake)
}

/// [`keyed_deployment`], with the writable channel on its list only when `write_channel`, and its
/// store.
fn keyed_deployment_of(
    key: &str,
    write_channel: bool,
) -> (
    AppState,
    Arc<vibe_talk::discord::fake::FakeDiscord>,
    Arc<FakeStore>,
) {
    let write = if write_channel {
        format!(
            "[[channels]]\nid = \"{WRITE_CHANNEL}\"\nlabel = \"lead team\"\nwritable = true\n\
             provider = \"{key}\"\n"
        )
    } else {
        String::new()
    };
    let text = format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{key}"
kind = "discord"
token = "test-bot-token"

[[channels]]
id = "{READ_CHANNEL}"
label = "build noise"
provider = "{key}"

{write}"#,
        read = testing::READ_TOKEN,
    );
    let (mut state, fake, store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::new(
        vec![ProviderEntry {
            key: key.to_owned(),
            namespace: state.config.providers[0].namespace(),
            client: fake.clone(),
            live_poll_seconds: 0,
        }],
        None,
    );
    state.replace_providers(Arc::new(router));
    (state, fake, store)
}

// ---- the store's summaries ----------------------------------------------------------------------

/// The key of a summary of `text`, said by `message` in `channel`, under `state`'s policy.
fn summary(state: &AppState, channel: &str, message: &str, text: &str) -> SummaryKey {
    SummaryKey {
        channel: id(channel),
        message: MessageId(message.to_owned()),
        content_hash: vibe_talk::summarize::content_hash(text),
        version: state.summary_version.to_string(),
    }
}

/// The key of a thread's display name in `channel`: kept whatever happens to the channel's
/// summaries.
fn thread_name(channel: &str) -> SummaryKey {
    SummaryKey {
        channel: id(channel),
        message: MessageId("thread-name:1000".to_owned()),
        content_hash: 0,
        version: vibe_talk::store::KEPT_VERSION.to_owned(),
    }
}

async fn file_summaries(store: &FakeStore, keys: &[&SummaryKey]) {
    for key in keys {
        store
            .cache_summary(key, "a summary", None)
            .await
            .expect("filed");
    }
}

async fn kept(store: &FakeStore, key: &SummaryKey) -> bool {
    store.cached_summary(key).await.expect("read").is_some()
}

/// Wait for the cache behind `state`, if any, and then for the task that keeps the store's
/// summaries in step with it.
async fn settled(state: &AppState) {
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    }
    assert!(
        state.message_cache.orphans_settled(WAIT).await,
        "the summaries task caught up"
    );
}

#[tokio::test]
async fn a_channel_the_owner_hides_is_swept_out_of_the_cache() {
    let dir = TempDir::new("cache-sweep-hidden");
    let clock = ManualClock::at(T0 + HOUR);
    let (state, _fake, store) = testing::state_with_store();
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let cache = state.message_cache.cache().expect("installed");
    offer_rows(&cache, LEGACY, READ_CHANNEL, 3, clock.now());
    offer_rows(&cache, LEGACY, WRITE_CHANNEL, 4, clock.now());
    assert_eq!(held(&cache), 7);
    let listed = summary(&state, READ_CHANNEL, "1000000000000000001", "row 1");
    let hidden = summary(&state, WRITE_CHANNEL, "1000000000000000002", "row 2");
    let name = thread_name(WRITE_CHANNEL);
    file_summaries(&store, &[&listed, &hidden, &name]).await;

    let (status, body) = call(
        &state,
        "DELETE",
        &format!("/api/v1/channels/{WRITE_CHANNEL}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        held(&cache),
        3,
        "only the channel still on the list is held"
    );
    settled(&state).await;
    assert!(!kept(&store, &hidden).await, "its summaries go with it");
    assert!(kept(&store, &name).await, "its thread names stay");
    assert!(kept(&store, &listed).await);
}

#[tokio::test]
async fn a_channel_taken_out_of_the_configuration_is_swept_at_the_next_start() {
    let dir = TempDir::new("cache-sweep-config");
    let clock = ManualClock::at(T0 + HOUR);
    {
        let (state, _fake) = testing::state();
        let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
        let cache = state.message_cache.cache().expect("installed");
        offer_rows(&cache, LEGACY, READ_CHANNEL, 3, clock.now());
        offer_rows(&cache, LEGACY, WRITE_CHANNEL, 4, clock.now());
        assert_eq!(held(&cache), 7);
        close(&state, cache);
    }
    let (state, _fake, store) =
        testing::state_with_store_from_toml(&config_without_the_write_channel());
    let listed = summary(&state, READ_CHANNEL, "1000000000000000001", "row 1");
    let removed = summary(&state, WRITE_CHANNEL, "1000000000000000002", "row 2");
    let name = thread_name(WRITE_CHANNEL);
    file_summaries(&store, &[&listed, &removed, &name]).await;
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(
        held(&cache),
        3,
        "the channel the file no longer names is gone"
    );
    settled(&state).await;
    assert!(!kept(&store, &removed).await, "and so are its summaries");
    assert!(kept(&store, &name).await, "but not its thread names");
    assert!(kept(&store, &listed).await);
}

#[tokio::test]
async fn a_renamed_provider_key_leaves_nothing_behind_under_the_old_one() {
    let dir = TempDir::new("cache-sweep-rename");
    let clock = ManualClock::at(T0 + HOUR);
    {
        let (state, _fake) = keyed_deployment("first");
        let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
        let cache = state.message_cache.cache().expect("installed");
        offer_rows(&cache, "first", READ_CHANNEL, 3, clock.now());
        offer_rows(&cache, "first", WRITE_CHANNEL, 4, clock.now());
        assert_eq!(held(&cache), 7);
        close(&state, cache);
    }
    let (state, _fake, store) = keyed_deployment_of("second", true);
    let read = summary(&state, READ_CHANNEL, "1000000000000000001", "row 1");
    let write = summary(&state, WRITE_CHANNEL, "1000000000000000002", "row 2");
    file_summaries(&store, &[&read, &write]).await;
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(
        held(&cache),
        0,
        "the same channels under a provider key the configuration no longer has are gone"
    );
    settled(&state).await;
    assert!(
        kept(&store, &read).await && kept(&store, &write).await,
        "the store keys summaries by channel alone, and both channels are still listed"
    );
}

#[tokio::test]
async fn nothing_is_swept_or_filled_when_the_store_did_not_give_back_the_added_channels() {
    let dir = TempDir::new("cache-sweep-guard");
    let clock = ManualClock::at(T0 + HOUR);
    let store = Arc::new(FakeStore::new());
    let added = id("5555555555");
    store
        .add_channel(&added, "added in the app", false, None, 1)
        .await
        .expect("stored");
    let added_summary = {
        let (state, fake) = testing::state_with(store.clone());
        fake.register_channel(&added);
        let added_summary = summary(&state, added.as_str(), "1000000000000000001", "row 1");
        file_summaries(&store, &[&added_summary]).await;
        let cache = testing::install_message_cache(&state, dir.path(), clock.clone());
        let (from_store, hidden) = state.restore_channel_lists().await;
        assert_eq!(from_store.expect("added channels"), 1);
        assert_eq!(hidden.expect("hidden channels"), 0);
        assert!(state.message_cache.is_armed());
        offer_rows(&cache, LEGACY, added.as_str(), 5, clock.now());
        offer_rows(&cache, LEGACY, READ_CHANNEL, 3, clock.now());
        assert_eq!(held(&cache), 8);
        close(&state, cache);
        added_summary
    };

    // The next start, at which the store fails to answer for the channels added in the app.
    let (state, _fake) = testing::state_with(store.clone());
    let cache = testing::install_message_cache(&state, dir.path(), clock.clone());
    store.fail_next("the store is briefly unreadable");
    let (from_store, hidden) = state.restore_channel_lists().await;
    assert!(from_store.is_err());
    assert!(hidden.is_ok());
    assert!(!state.message_cache.is_armed());
    assert!(
        state.channel(added.as_str()).is_none(),
        "this start's list is short of the added channel"
    );
    // A later change of routes sweeps nothing either: the sink stays unarmed.
    state.refresh_routes();
    assert_eq!(held(&cache), 8, "nothing was swept");
    assert_eq!(cache.applied_seq(), 0, "nothing was asked of the writer");
    settled(&state).await;
    assert!(
        kept(&store, &added_summary).await,
        "nor were the added channel's summaries"
    );
    assert!(
        state
            .message_cache
            .begin_for(vibe_talk::auth::Scope::Write, &id(READ_CHANNEL))
            .is_none_or(|fill| !fill.writes_rows()),
        "and nothing is filled but deletions"
    );
}

// ---- write-through at the read sites -----------------------------------------------------------

mod slack_fake;

use vibe_talk::cache::coverage::{Interval, Source, SETTLE_MS};
use vibe_talk::cache::Read;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::testing::TimelineChat;

const DAY: i64 = 24 * HOUR;
/// The cache's default age limit, in milliseconds.
const MAX_AGE: i64 = 90 * DAY;

/// A deployment reading its provider's timelines through [`TimelineChat`], as Discord's own API,
/// or as a bridge when `bridge`, with `count` messages in the read-only channel a minute apart
/// from [`T0`], and a cache behind its sink read by `clock`.
struct Deployment {
    state: AppState,
    fake: Arc<FakeDiscord>,
    chat: Arc<TimelineChat>,
    store: Arc<FakeStore>,
    cache: Arc<MessageCache>,
    ids: Vec<MessageId>,
    _dir: TempDir,
}

/// What a [`Deployment`] reads, before any cache is put behind it.
struct Upstream {
    state: AppState,
    fake: Arc<FakeDiscord>,
    chat: Arc<TimelineChat>,
    store: Arc<FakeStore>,
    ids: Vec<MessageId>,
}

fn upstream(bridge: bool, count: i64) -> Upstream {
    let text = if bridge {
        testing::config_toml().replace(
            "[discord]",
            "[discord]\nthread_api = \"bridge\"\napi_base = \"http://127.0.0.1:9/api\"",
        )
    } else {
        testing::config_toml()
    };
    // An adapter may push changes for the channels, as the bridge's does.
    let text = format!("{text}\n[ingest]\ntoken = \"{INGEST_TOKEN}\"\n");
    let (mut state, fake, store) = testing::state_with_store_from_toml(&text);
    let ids = (0..count)
        .map(|n| {
            fake.seed_at(
                &id(READ_CHANNEL),
                "Ada",
                &format!("message {n}"),
                T0 + n * MINUTE,
            )
        })
        .collect();
    let chat = Arc::new(TimelineChat::new(fake.clone()));
    state.replace_chat(chat.clone());
    Upstream {
        state,
        fake,
        chat,
        store,
        ids,
    }
}

async fn deployment(bridge: bool, count: i64, clock: &Arc<ManualClock>) -> Deployment {
    let dir = TempDir::new("cache-fill");
    let Upstream {
        state,
        fake,
        chat,
        store,
        ids,
    } = upstream(bridge, count);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let cache = state.message_cache.cache().expect("installed");
    assert!(cache.quiesce_blocking(WAIT), "the first sweep landed");
    Deployment {
        state,
        fake,
        chat,
        store,
        cache,
        ids,
        _dir: dir,
    }
}

fn read_chan() -> ChanKey {
    ChanKey::new(LEGACY, READ_CHANNEL)
}

fn coverage(cache: &MessageCache, chan: &ChanKey, scope: &CacheScope) -> Vec<Interval> {
    assert!(cache.quiesce_blocking(WAIT));
    cache.coverage(chan, scope).served().expect("coverage")
}

fn holds(cache: &MessageCache, chan: &ChanKey, message: &MessageId) -> bool {
    assert!(cache.quiesce_blocking(WAIT));
    cache
        .message(chan, message)
        .served()
        .expect("a read")
        .is_some()
}

fn tombstoned(cache: &MessageCache, chan: &ChanKey, message: &MessageId) -> bool {
    assert!(cache.quiesce_blocking(WAIT));
    cache.tombstoned(chan, message).served().expect("a read")
}

async fn get(state: &AppState, uri: &str) -> Value {
    let (status, body) = call(state, "GET", uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

#[tokio::test]
async fn a_write_token_timeline_of_discord_main_fills_rows_and_coverage() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(false, 10, &clock).await;
    get(
        &d.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
    )
    .await;
    assert_eq!(held(&d.cache), 10);
    let newest = T0 + 9 * MINUTE;
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main),
        [Interval {
            // The page said there is nothing older: from the age limit, as far back as the cache
            // keeps anything.
            t0_ms: clock.now() - MAX_AGE,
            // The newest two minutes are not settled yet.
            t1_ms: newest - SETTLE_MS,
            listed_ms: clock.now(),
            full_ms: Some(clock.now()),
            complete: true,
            source: Source::Read,
        }]
    );
    let head = d
        .cache
        .head(&read_chan(), &CacheScope::Main)
        .served()
        .flatten()
        .expect("a head");
    assert_eq!(head.head_ms, newest);
    assert_eq!(head.head_listed_ms, clock.now());
}

#[tokio::test]
async fn a_write_token_timeline_of_a_bridge_flat_page_is_listed_as_of_when_the_bridge_says() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 10, &clock).await;
    let as_of = clock.now() - 10 * MINUTE;
    d.chat.set_as_of(Some(vibe_talk::clock::iso_from_ms(as_of)));
    get(
        &d.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=flat&limit=50"),
    )
    .await;
    assert_eq!(held(&d.cache), 10);
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Flat),
        [Interval {
            // A bridge's `has_more` is no proof of the channel's start: from its oldest row.
            t0_ms: T0,
            t1_ms: T0 + 9 * MINUTE - SETTLE_MS,
            // The earlier of the read's start and the bridge's `as_of`.
            listed_ms: as_of,
            full_ms: Some(as_of),
            complete: true,
            source: Source::Read,
        }]
    );
    assert_eq!(
        d.cache.memberships(&read_chan(), &d.ids[0]),
        Read::Served(vec![CacheScope::Flat]),
        "every bridge row is home in flat"
    );
    assert!(coverage(&d.cache, &read_chan(), &CacheScope::Main).is_empty());
}

const SLACK_CH: &str = "C0000MAIN1";
const SLACK: &str = "slack";

fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1_000
}

fn slack_id(ts: &str) -> MessageId {
    vibe_talk::slack::message_id_from_ts(ts).expect("an encodable ts")
}

fn slack_chan() -> ChanKey {
    ChanKey::new(SLACK, SLACK_CH)
}

/// A Slack deployment against `fake`, with a cache in `dir` read by `clock`.
async fn slack_deployment(
    fake: &slack_fake::Fake,
    dir: &TempDir,
    clock: &Arc<ManualClock>,
) -> AppState {
    slack_deployment_of(fake, dir, clock, SLACK_CH).await
}

/// [`slack_deployment`], answering for `channel` alone.
async fn slack_deployment_of(
    fake: &slack_fake::Fake,
    dir: &TempDir,
    clock: &Arc<ManualClock>,
    channel: &str,
) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{SLACK}"
kind = "slack"
api_base = "{base}"
token = "{token}"

[[channels]]
id = "{channel}"
label = "team"
writable = true
"#,
        read = testing::READ_TOKEN,
        base = fake.base,
        token = slack_fake::TOKEN,
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    assert!(state
        .message_cache
        .cache()
        .expect("installed")
        .quiesce_blocking(WAIT));
    state
}

#[tokio::test]
async fn a_write_token_timeline_of_slack_main_that_reached_the_start_claims_it_but_never_complete()
{
    let mut fixture = slack_fake::FakeState::default();
    let list = (0..6)
        .map(|n| {
            slack_fake::profiled(
                &slack_fake::ts(n * 60, 0),
                "UADA00001",
                "Ada",
                &format!("message {n}"),
            )
        })
        .collect();
    fixture.history.insert(SLACK_CH.to_owned(), list);
    let fake = slack_fake::Fake::start(fixture, 4).await;
    let dir = TempDir::new("cache-fill-slack");
    let clock = ManualClock::at(slack_ms(3_600));
    let state = slack_deployment(&fake, &dir, &clock).await;
    get(
        &state,
        &format!("/api/v1/channels/{SLACK_CH}/timeline?view=main&limit=50"),
    )
    .await;
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(held(&cache), 6);
    assert_eq!(
        coverage(&cache, &slack_chan(), &CacheScope::Main),
        [Interval {
            t0_ms: clock.now() - MAX_AGE,
            t1_ms: slack_ms(300) - SETTLE_MS,
            listed_ms: clock.now(),
            full_ms: Some(clock.now()),
            // The client omits a message that has no author, so no Slack listing is complete.
            complete: false,
            source: Source::Read,
        }]
    );
}

#[tokio::test]
async fn a_newest_slack_window_with_an_authorless_row_never_claims_the_channels_start() {
    let mut fixture = slack_fake::FakeState::default();
    let authorless = serde_json::json!({
        "type": "message", "ts": slack_fake::ts(0, 0), "text": "nobody wrote this",
    });
    fixture.history.insert(
        SLACK_CH.to_owned(),
        vec![
            authorless,
            slack_fake::profiled(&slack_fake::ts(200, 0), "UADA00001", "Ada", "first"),
            slack_fake::profiled(&slack_fake::ts(400, 0), "UADA00001", "Ada", "second"),
        ],
    );
    let fake = slack_fake::Fake::start(fixture, 10).await;
    let dir = TempDir::new("cache-fill-slack-window");
    let clock = ManualClock::at(slack_ms(3_600));
    let state = slack_deployment(&fake, &dir, &clock).await;
    // Twenty asked, two returned: shorter than asked is still no proof of the channel's start.
    let window =
        vibe_talk::ops::messages(&state, vibe_talk::auth::Scope::Write, SLACK_CH, Some(20))
            .await
            .expect("a window");
    assert_eq!(window.messages.len(), 2);
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(held(&cache), 2, "the authorless row is not a message");
    assert_eq!(
        coverage(&cache, &slack_chan(), &CacheScope::Main),
        [Interval {
            t0_ms: slack_ms(200),
            t1_ms: slack_ms(400) - SETTLE_MS,
            listed_ms: clock.now(),
            full_ms: Some(clock.now()),
            complete: false,
            source: Source::Read,
        }]
    );
}

#[tokio::test]
async fn a_long_slack_thread_whose_read_narrows_never_claims_its_start_nor_erases_its_older_replies(
) {
    // The thread read as a view of its channel, and as a channel narrowed to that one thread.
    for narrowed in [false, true] {
        let mut fixture = slack_fake::FakeState::default();
        let root_ts = slack_fake::ts(0, 0);
        let mut root = slack_fake::profiled(&root_ts, "UADA00001", "Ada", "topic");
        let reply = |j: i64| (slack_fake::ts(100 + j * 10, 0), format!("reply {j}"));
        let replies: Vec<(String, String)> = (0..30).map(reply).collect();
        slack_fake::thread(&mut fixture, SLACK_CH, &mut root, &replies);
        fixture.history.insert(SLACK_CH.to_owned(), vec![root]);
        // Four rows a page: the client reads a thread through a window of ten pages.
        let fake = slack_fake::Fake::start(fixture, 4).await;
        let dir = TempDir::new("cache-fill-slack-long-thread");
        let clock = ManualClock::at(slack_ms(3_600));
        let (channel, uri, scope) = if narrowed {
            let channel = format!("{SLACK_CH}~{root_ts}");
            let uri = format!("/api/v1/channels/{channel}/timeline?view=main&limit=50");
            (channel, uri, CacheScope::Main)
        } else {
            let uri = format!(
                "/api/v1/channels/{SLACK_CH}/timeline?view=thread&thread_id={root_ts}&limit=50"
            );
            (
                SLACK_CH.to_owned(),
                uri,
                CacheScope::Thread(root_ts.clone()),
            )
        };
        let state = slack_deployment_of(&fake, &dir, &clock, &channel).await;
        let cache = state.message_cache.cache().expect("installed");
        let chan = ChanKey::new(SLACK, &channel);

        // Read whole, inside the window: the root and its thirty replies.
        let first = get(&state, &uri).await;
        assert_eq!(first["messages"].as_array().expect("messages").len(), 31);
        assert_eq!(held(&cache), 31);

        // The thread grows past the window, and is read again.
        fake.with(|state| {
            let list = state
                .replies
                .get_mut(&(SLACK_CH.to_owned(), root_ts.clone()))
                .expect("the thread");
            for j in 30..60 {
                let (ts, text) = reply(j);
                list.push(serde_json::json!({"type": "message", "ts": ts, "thread_ts": root_ts,
                    "user": "UREPLIER1", "text": text, "user_profile": {"display_name": "Replier"}}));
            }
        });
        clock.advance(15 * MINUTE);
        let second = get(&state, &uri).await;
        let returned = second["messages"].as_array().expect("messages").len();
        // What the client does with a thread longer than its window: one page, gap-free from the
        // newest end, shorter than asked, saying there is no more.
        assert!(
            returned < 50 && second["has_more"] == false,
            "narrowed {narrowed}: {returned} rows, has_more {}",
            second["has_more"]
        );

        for (ts, _) in &replies {
            let id = slack_id(ts);
            assert!(
                !tombstoned(&cache, &chan, &id),
                "narrowed {narrowed}: {ts} is still upstream"
            );
            assert!(holds(&cache, &chan, &id), "narrowed {narrowed}: {ts}");
        }
        assert!(holds(&cache, &chan, &slack_id(&root_ts)), "the root");
        assert_eq!(held(&cache), 31 + u64::try_from(returned).expect("small"));
        let claimed = coverage(&cache, &chan, &scope);
        assert!(
            !claimed.is_empty(),
            "narrowed {narrowed}: the reads claimed their spans"
        );
        assert!(
            claimed.iter().all(|interval| interval.t0_ms >= slack_ms(0)),
            "narrowed {narrowed}: nothing claims the thread from before its root: {claimed:?}"
        );
    }
}

#[tokio::test]
async fn a_bounded_slack_flat_page_never_tombstones_a_reply_nor_claims_a_thread() {
    // Eleven threads. The busiest has sixty replies, written last, so Flat expands it and carries
    // only its newest fifty; Flat says it was bounded.
    let mut fixture = slack_fake::FakeState::default();
    let mut history = Vec::new();
    let mut threads = Vec::new();
    for k in 0..11_i64 {
        let root_ts = slack_fake::ts(k * 100, 0);
        let mut root = slack_fake::profiled(&root_ts, "UADA00001", "Ada", &format!("topic {k}"));
        let replies: Vec<(String, String)> = if k == 0 {
            (0..60)
                .map(|j| (slack_fake::ts(2_000 + j, 0), format!("busy reply {j}")))
                .collect()
        } else {
            vec![(slack_fake::ts(k * 100 + 1, 0), format!("reply to {k}"))]
        };
        slack_fake::thread(&mut fixture, SLACK_CH, &mut root, &replies);
        history.push(root);
        threads.push((root_ts, replies));
    }
    fixture.history.insert(SLACK_CH.to_owned(), history);
    let fake = slack_fake::Fake::start(fixture, 20).await;
    let dir = TempDir::new("cache-fill-slack-flat");
    let clock = ManualClock::at(slack_ms(3_600));
    let state = slack_deployment(&fake, &dir, &clock).await;
    let cache = state.message_cache.cache().expect("installed");

    // The busy thread read whole: its sixty replies, and a claim over the thread.
    let (busy, busy_replies) = &threads[0];
    get(
        &state,
        &format!("/api/v1/channels/{SLACK_CH}/timeline?view=thread&thread_id={busy}&limit=100"),
    )
    .await;
    let busy_scope = CacheScope::Thread(busy.clone());
    let claimed = coverage(&cache, &slack_chan(), &busy_scope);
    assert_eq!(claimed.len(), 1, "the thread read claims the thread");
    for (reply, _) in busy_replies {
        assert!(holds(&cache, &slack_chan(), &slack_id(reply)));
    }

    // Later, the bounded Flat page.
    clock.advance(15 * MINUTE);
    let flat = get(
        &state,
        &format!("/api/v1/channels/{SLACK_CH}/timeline?view=flat&limit=50"),
    )
    .await;
    assert!(
        flat["notice"].is_string(),
        "Flat said it was bounded: {flat}"
    );
    for (reply, _) in busy_replies {
        let reply = slack_id(reply);
        assert!(holds(&cache, &slack_chan(), &reply), "{reply:?} was erased");
        assert!(!tombstoned(&cache, &slack_chan(), &reply));
    }
    assert_eq!(
        coverage(&cache, &slack_chan(), &busy_scope),
        claimed,
        "a Flat page claims nothing about a thread"
    );
    for (root, _) in &threads[1..] {
        assert!(
            coverage(&cache, &slack_chan(), &CacheScope::Thread(root.clone())).is_empty(),
            "no claim over thread {root}"
        );
    }
    assert!(
        coverage(&cache, &slack_chan(), &CacheScope::Main).is_empty(),
        "a Flat page with threads on it is not taken as Main"
    );
}

#[tokio::test]
async fn an_empty_page_claims_nothing_and_a_walk_from_since_claims_its_span() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(false, 0, &clock).await;
    for n in 10..=20 {
        d.fake.seed_at(
            &id(READ_CHANNEL),
            "Ada",
            &format!("message {n}"),
            T0 + n * MINUTE,
        );
    }
    let iso = vibe_talk::clock::iso_from_ms;
    let applied = d.cache.applied_seq();
    let empty = get(
        &d.state,
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/page?since={}&until={}",
            iso(T0),
            iso(T0 + 5 * MINUTE)
        ),
    )
    .await;
    assert_eq!(empty["messages"], serde_json::json!([]), "{empty}");
    assert_eq!(d.cache.applied_seq(), applied, "nothing was offered");
    assert!(coverage(&d.cache, &read_chan(), &CacheScope::Main).is_empty());

    let since = T0 + 9 * MINUTE;
    let walked = get(
        &d.state,
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/page?since={}&until={}",
            iso(since),
            iso(T0 + HOUR)
        ),
    )
    .await;
    assert_eq!(walked["has_more"], false, "{walked}");
    assert_eq!(held(&d.cache), 11);
    let newest = T0 + 20 * MINUTE;
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main),
        [Interval {
            t0_ms: since,
            // The walk proved [since, min(until, newest + 1)); the writer keeps its claim out of
            // the newest two minutes, which no read of the newest end has settled yet.
            t1_ms: (newest + 1).min(newest - SETTLE_MS),
            listed_ms: clock.now(),
            full_ms: Some(clock.now()),
            complete: true,
            source: Source::Read,
        }]
    );
}

#[tokio::test]
async fn a_before_page_gives_rows_only_and_a_delta_never_claims_but_a_complete_one_deletes() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 5, &clock).await;
    let flat =
        |extra: &str| format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=flat&limit=50{extra}");

    let before = get(&d.state, &flat(&format!("&before={}", d.ids[3].as_str()))).await;
    assert_eq!(before["messages"].as_array().expect("messages").len(), 3);
    assert_eq!(held(&d.cache), 3, "the rows of a step back are kept");
    assert!(
        coverage(&d.cache, &read_chan(), &CacheScope::Flat).is_empty(),
        "and claim nothing"
    );

    clock.advance(MINUTE);
    let newest = get(&d.state, &flat("")).await;
    let cursor = newest["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned();
    let claimed = coverage(&d.cache, &read_chan(), &CacheScope::Flat);
    assert_eq!(claimed.len(), 1);

    // Upstream, one message is deleted and one written; the bridge's delta says both.
    let gone = d.ids[1].clone();
    d.fake.delete(&gone);
    let written = d
        .fake
        .seed_at(&id(READ_CHANNEL), "Ada", "written since", T0 + 5 * MINUTE);
    d.chat.set_delta(vibe_talk::threads::TimelineDelta {
        more: false,
        complete: true,
        deleted: vec![gone.clone()],
        removed_threads: Vec::new(),
    });
    clock.advance(MINUTE);
    let delta = get(&d.state, &flat(&format!("&after={cursor}"))).await;
    assert!(delta["delta"].is_object(), "{delta}");
    assert!(holds(&d.cache, &read_chan(), &written));
    assert!(
        tombstoned(&d.cache, &read_chan(), &gone),
        "the delta's deletion is applied"
    );
    assert!(!holds(&d.cache, &read_chan(), &gone));
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Flat),
        claimed,
        "a delta claims nothing"
    );
}

/// A fresh deployment with a thread in the read-only channel: a root and two replies to it, after
/// eight plain messages.
async fn deployment_with_a_thread(
    clock: &Arc<ManualClock>,
) -> (Deployment, MessageId, Vec<MessageId>) {
    let d = deployment(false, 8, clock).await;
    let root = d.ids[2].clone();
    let replies = (0..2)
        .map(|n| {
            let reply = d.fake.seed_at(
                &id(READ_CHANNEL),
                "Ada",
                &format!("reply {n}"),
                T0 + (10 + n) * MINUTE,
            );
            d.fake.set_reply_to(&reply, &root);
            reply
        })
        .collect();
    (d, root, replies)
}

#[tokio::test]
async fn every_read_site_fills_under_a_write_grant() {
    use vibe_talk::auth::Scope;
    let clock = ManualClock::at(T0 + HOUR);

    // `/count`: the walk's own pages, newest first.
    let d = deployment(false, 10, &clock).await;
    get(&d.state, &format!("/api/v1/channels/{READ_CHANNEL}/count")).await;
    assert_eq!(held(&d.cache), 10);
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main)[0].source,
        Source::Walk
    );

    // `/resolve`: the window it searched.
    let d = deployment(false, 10, &clock).await;
    vibe_talk::ops::resolve(
        &d.state,
        Scope::Write,
        READ_CHANNEL,
        "message 3",
        None,
        None,
    )
    .await
    .expect("resolved");
    assert_eq!(held(&d.cache), 10);

    // `/page`, the newest page: the newest stretch.
    let d = deployment(false, 10, &clock).await;
    get(
        &d.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/page?limit=5"),
    )
    .await;
    assert_eq!(held(&d.cache), 6, "the page and the one more it asked for");

    // A message read by id from a thread: that row, listed in its thread.
    let (d, root, replies) = deployment_with_a_thread(&clock).await;
    let uri = format!(
        "/api/v1/channels/{READ_CHANNEL}/messages/{}?thread_id={}",
        replies[1].as_str(),
        root.as_str()
    );
    get(&d.state, &uri).await;
    assert_eq!(held(&d.cache), 1);
    assert_eq!(
        d.cache.memberships(&read_chan(), &replies[1]),
        Read::Served(vec![CacheScope::Thread(root.as_str().to_owned())])
    );

    // A catch-up's refresh of a channel's window: the grant of the caller that claimed it.
    let d = deployment(false, 10, &clock).await;
    vibe_talk::ops::activity(
        &d.state,
        Scope::Write,
        vibe_talk::ops::ActivityRequest::default(),
    )
    .await
    .expect("activity");
    assert_eq!(held(&d.cache), 10);

    // `/messages` itself.
    let d = deployment(false, 10, &clock).await;
    get(
        &d.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/messages"),
    )
    .await;
    assert_eq!(held(&d.cache), 10);
}

#[tokio::test]
async fn an_unarmed_cache_is_filled_by_nothing() {
    let clock = ManualClock::at(T0 + HOUR);
    let dir = TempDir::new("cache-fill-unarmed");
    let (mut state, fake, _store) = testing::state_with_store();
    for n in 0..5 {
        fake.seed_at(&id(READ_CHANNEL), "Ada", &format!("m {n}"), T0 + n * MINUTE);
    }
    state.replace_chat(Arc::new(TimelineChat::new(fake.clone())));
    let cache = testing::install_message_cache(&state, dir.path(), clock.clone());
    state.refresh_routes();
    for uri in [
        format!("/api/v1/channels/{READ_CHANNEL}/messages"),
        format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main"),
        format!("/api/v1/channels/{READ_CHANNEL}/page"),
        format!("/api/v1/channels/{READ_CHANNEL}/count"),
    ] {
        get(&state, &uri).await;
    }
    assert!(cache.quiesce_blocking(WAIT));
    // Each read was left out, and only that it was reached the writer: a time, for a channel the
    // file holds nothing of, so nothing at all was written.
    assert!(
        cache.last_drop_ms(&read_chan()).is_some(),
        "the reads were left out"
    );
    let counts = cache.counts().served().expect("counts");
    assert_eq!(
        (counts.messages, counts.channels, counts.intervals),
        (0, 0, 0),
        "nothing was written"
    );
    assert_eq!(cache.stats().rows_written, 0);
    assert_eq!(held(&cache), 0);
}

#[tokio::test]
async fn a_deletion_pushed_while_the_sink_is_unarmed_still_takes_the_message_out_of_the_file() {
    let clock = ManualClock::at(T0 + HOUR);
    let dir = TempDir::new("cache-fill-unarmed-deletion");
    let ids = {
        let u = upstream(true, 5);
        let state = testing::with_message_cache(u.state, dir.path(), clock.clone()).await;
        get(&state, &flat_page()).await;
        let cache = state.message_cache.cache().expect("installed");
        assert_eq!(held(&cache), 5);
        close(&state, cache);
        u.ids
    };

    // A start whose channel list could not be put together: the sink is never armed.
    let u = upstream(true, 5);
    let cache = testing::install_message_cache(&u.state, dir.path(), clock.clone());
    u.state.refresh_routes();
    assert!(!u.state.message_cache.is_armed());
    clock.advance(MINUTE);
    let publish = |kind, message: Message| {
        assert!(u.state.live.publish_ingested(
            format!("e-{}", message.id.as_str()),
            kind,
            false,
            &id(READ_CHANNEL),
            message,
        ));
    };
    // A deletion of a message the file holds takes it out; one of a message it never held writes
    // nothing; an edit is not written.
    let gone = u.fake.message(&ids[1]).expect("seeded");
    publish(vibe_talk::live::LiveKind::Delete, gone);
    publish(
        vibe_talk::live::LiveKind::Delete,
        message(READ_CHANNEL, T0 + 30 * MINUTE, "never held"),
    );
    publish(
        vibe_talk::live::LiveKind::Update,
        said(&u.fake, &ids[3], "edited upstream"),
    );
    assert!(cache.quiesce_blocking(WAIT));
    let counts = cache.counts().served().expect("counts");
    assert_eq!((counts.messages, counts.tombstones), (4, 1));
    close(&u.state, cache);

    // The next start is armed: the deleted message is not there to serve, and the edit was left
    // for a read to bring.
    let u = upstream(true, 5);
    let state = testing::with_message_cache(u.state, dir.path(), clock.clone()).await;
    let cache = state.message_cache.cache().expect("installed");
    assert!(tombstoned(&cache, &read_chan(), &ids[1]));
    assert!(!holds(&cache, &read_chan(), &ids[1]));
    assert_eq!(
        cached(&cache, &read_chan(), &ids[3]).message.content,
        "message 3"
    );
}

// ---- changes from the live feed, and posts ------------------------------------------------------

use vibe_talk::testing::HeldRead;

fn flat_page() -> String {
    format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=flat&limit=50")
}

/// Push one change through the adapter route, as a bridge's adapter does.
async fn ingest(state: &AppState, event: Value) -> StatusCode {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/live/events")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {INGEST_TOKEN}"))
        .body(Body::from(event.to_string()))
        .expect("request");
    vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers")
        .status()
}

fn pushed(event_id: &str, kind: &str, message: &Message) -> Value {
    serde_json::json!({
        "event_id": event_id,
        "historical": false,
        "kind": kind,
        "message": message,
    })
}

fn pushed_deletion(event_id: &str, message: &MessageId) -> Value {
    serde_json::json!({
        "event_id": event_id,
        "historical": false,
        "kind": "delete",
        "channel_id": READ_CHANNEL,
        "message_id": message,
    })
}

/// `message` as the fake holds it, with `content` instead of its own.
fn saying(d: &Deployment, message: &MessageId, content: &str) -> Message {
    said(&d.fake, message, content)
}

fn said(fake: &FakeDiscord, message: &MessageId, content: &str) -> Message {
    Message {
        content: content.to_owned(),
        ..fake.message(message).expect("the fake holds it")
    }
}

fn cached(
    cache: &MessageCache,
    chan: &ChanKey,
    message: &MessageId,
) -> vibe_talk::cache::CachedMessage {
    assert!(cache.quiesce_blocking(WAIT));
    cache
        .message(chan, message)
        .served()
        .flatten()
        .expect("the cache holds it")
}

#[tokio::test]
async fn a_pushed_edit_and_a_pushed_deletion_reach_the_cache() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 5, &clock).await;
    get(&d.state, &flat_page()).await;
    assert_eq!(held(&d.cache), 5);

    clock.advance(MINUTE);
    let (edited, gone) = (d.ids[3].clone(), d.ids[1].clone());
    let edit = saying(&d, &edited, "message 3, edited upstream");
    assert_eq!(
        ingest(&d.state, pushed("e1", "update", &edit)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        ingest(&d.state, pushed_deletion("e2", &gone)).await,
        StatusCode::ACCEPTED
    );
    let now = cached(&d.cache, &read_chan(), &edited);
    assert_eq!(now.message.content, "message 3, edited upstream");
    assert_eq!(now.observed_ms, clock.now(), "observed when it was pushed");
    assert!(tombstoned(&d.cache, &read_chan(), &gone));
    assert!(!holds(&d.cache, &read_chan(), &gone));
    assert_eq!(held(&d.cache), 4);
    // Neither claimed anything: the coverage is the read's.
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Flat)[0].listed_ms,
        T0 + HOUR
    );
}

#[tokio::test]
async fn a_deletion_pushed_while_a_read_is_in_flight_is_not_undone_by_that_read() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 5, &clock).await;
    get(&d.state, &flat_page()).await;
    let gone = d.ids[2].clone();
    assert!(holds(&d.cache, &read_chan(), &gone));

    // A read begins, and takes the channel as it stands, the message still in it...
    clock.advance(MINUTE);
    let HeldRead { entered, release } = d.chat.hold_next_timeline();
    let reader = d.state.clone();
    let read = tokio::spawn(async move { get(&reader, &flat_page()).await });
    entered.await.expect("the read reached the provider");
    // ...and while it is on its way back, the deletion is pushed.
    clock.advance(MINUTE);
    assert_eq!(
        ingest(&d.state, pushed_deletion("e1", &gone)).await,
        StatusCode::ACCEPTED
    );
    assert!(tombstoned(&d.cache, &read_chan(), &gone));
    // Time passes before the read hands its answer back, as it does on a real server: a fill that
    // took its time when the read returned, rather than when it began, would now be later than
    // the deletion, and bring the message back.
    clock.advance(MINUTE);
    drop(release);
    let page = read.await.expect("the read finished");
    assert!(
        page["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .any(|m| m["id"] == gone.as_str()),
        "the read's answer still carried the message: {page}"
    );
    assert!(
        tombstoned(&d.cache, &read_chan(), &gone),
        "a read that began before the deletion does not bring the message back"
    );
    assert!(!holds(&d.cache, &read_chan(), &gone));
}

#[tokio::test]
async fn a_stale_bridge_snapshot_does_not_revert_a_later_push() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 5, &clock).await;
    get(&d.state, &flat_page()).await;

    clock.advance(10 * MINUTE);
    let (edited, gone) = (d.ids[3].clone(), d.ids[1].clone());
    let pushed_at = clock.now();
    let edit = saying(&d, &edited, "message 3, edited upstream");
    assert_eq!(
        ingest(&d.state, pushed("e1", "update", &edit)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        ingest(&d.state, pushed_deletion("e2", &gone)).await,
        StatusCode::ACCEPTED
    );

    // A read that starts after both pushes, answered from a copy the bridge took before them.
    clock.advance(MINUTE);
    d.chat
        .set_as_of(Some(vibe_talk::clock::iso_from_ms(pushed_at - 5 * MINUTE)));
    let page = get(&d.state, &flat_page()).await;
    assert_eq!(page["messages"].as_array().expect("messages").len(), 5);
    assert_eq!(
        cached(&d.cache, &read_chan(), &edited).message.content,
        "message 3, edited upstream",
        "the older copy's text does not come back"
    );
    assert!(
        tombstoned(&d.cache, &read_chan(), &gone),
        "nor does the deleted message"
    );
    assert!(!holds(&d.cache, &read_chan(), &gone));
}

/// A bridge configured with `recent_view = "flat"`, as the real client is: its recent read answers
/// from the copy of the channel the bridge keeps, dated by `as_of`, and `fetch_recent` is that same
/// read with the date dropped. A test can freeze the copy as it stood.
struct CopyKeepingBridge {
    inner: Arc<TimelineChat>,
    copy: std::sync::Mutex<Option<(Vec<Message>, String)>>,
}

#[async_trait::async_trait]
impl vibe_talk::chat::ChatClient for CopyKeepingBridge {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn supports_threading(&self) -> bool {
        true
    }

    async fn identity(&self) -> Result<vibe_talk::chat::ChatIdentity, vibe_talk::chat::ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, vibe_talk::chat::ChatError> {
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, vibe_talk::chat::ChatError> {
        Ok(self.fetch_recent_dated(channel, limit).await?.messages)
    }

    async fn fetch_recent_dated(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<vibe_talk::chat::RecentRead, vibe_talk::chat::ChatError> {
        let copy = self.copy.lock().expect("copy").clone();
        match copy {
            Some((messages, as_of)) => Ok(vibe_talk::chat::RecentRead {
                messages,
                as_of: Some(as_of),
                exhausted: false,
                notice: None,
            }),
            None => self.inner.fetch_recent_dated(channel, limit).await,
        }
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &vibe_talk::threads::TimelineRequest,
    ) -> Result<vibe_talk::threads::TimelinePage, vibe_talk::chat::ChatError> {
        self.inner.fetch_timeline(channel, request).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, vibe_talk::chat::ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

#[tokio::test]
async fn a_recent_read_from_a_stale_bridge_copy_does_not_revert_a_later_push() {
    // Through `find_message` (`ops::resolve`), and through the live poller's first read.
    for via_poller in [false, true] {
        let clock = ManualClock::at(T0 + HOUR);
        let text = testing::config_toml().replace(
            "[discord]",
            "[discord]\nthread_api = \"bridge\"\napi_base = \"http://127.0.0.1:9/api\"\n\
             recent_view = \"flat\"",
        );
        let text = format!("{text}\n[ingest]\ntoken = \"{INGEST_TOKEN}\"\n");
        let (mut state, fake, _store) = testing::state_with_store_from_toml(&text);
        let ids: Vec<MessageId> = (0..5)
            .map(|n| {
                fake.seed_at(
                    &id(READ_CHANNEL),
                    "Ada",
                    &format!("message {n}"),
                    T0 + n * MINUTE,
                )
            })
            .collect();
        let bridge = Arc::new(CopyKeepingBridge {
            inner: Arc::new(TimelineChat::new(fake.clone())),
            copy: std::sync::Mutex::new(None),
        });
        state.replace_chat(bridge.clone());
        let dir = TempDir::new("cache-fill-stale-copy");
        let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
        let cache = state.message_cache.cache().expect("installed");
        get(&state, &flat_page()).await;
        assert_eq!(held(&cache), 5);

        // The bridge takes its copy now; then an edit and a deletion are pushed.
        let copy = vibe_talk::chat::ChatClient::fetch_recent(fake.as_ref(), &id(READ_CHANNEL), 50)
            .await
            .expect("read");
        let copied_at = clock.now();
        clock.advance(10 * MINUTE);
        let (edited, gone) = (ids[3].clone(), ids[1].clone());
        let edit = said(&fake, &edited, "message 3, edited upstream");
        assert_eq!(
            ingest(&state, pushed("e1", "update", &edit)).await,
            StatusCode::ACCEPTED
        );
        assert_eq!(
            ingest(&state, pushed_deletion("e2", &gone)).await,
            StatusCode::ACCEPTED
        );

        // A write-scope read that starts after both pushes, answered from the older copy.
        clock.advance(MINUTE);
        *bridge.copy.lock().expect("copy") = Some((copy, vibe_talk::clock::iso_from_ms(copied_at)));
        if via_poller {
            let mut cursor = None;
            poll_once(
                state.chat.as_ref(),
                &state.live,
                &id(READ_CHANNEL),
                50,
                &mut cursor,
            )
            .await
            .expect("seeded");
        } else {
            vibe_talk::ops::resolve(
                &state,
                vibe_talk::auth::Scope::Write,
                READ_CHANNEL,
                "message",
                None,
                None,
            )
            .await
            .expect("resolved");
        }
        assert_eq!(
            cached(&cache, &read_chan(), &edited).message.content,
            "message 3, edited upstream",
            "via the poller {via_poller}: the older copy's text does not come back"
        );
        assert!(
            tombstoned(&cache, &read_chan(), &gone),
            "via the poller {via_poller}: nor does the deleted message"
        );
        assert!(!holds(&cache, &read_chan(), &gone));
    }
}

#[tokio::test]
async fn a_pushed_creation_is_a_possible_snippet_until_a_read_and_a_pushed_snippet_never_overwrites_one(
) {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 5, &clock).await;

    // Written upstream with a long body; the live event carries only the start of it.
    let full = "a long body that the live event cut short";
    let created = d
        .fake
        .seed_at(&id(READ_CHANNEL), "Ada", full, T0 + 5 * MINUTE);
    let snippet = saying(&d, &created, "a long body…");
    assert_eq!(
        ingest(&d.state, pushed("e1", "create", &snippet)).await,
        StatusCode::ACCEPTED
    );
    let first = cached(&d.cache, &read_chan(), &created);
    assert_eq!(first.message.content, "a long body…");
    assert!(first.content_partial, "a pushed creation may be a snippet");

    // A read lists it: its text replaces the snippet, even though it started no later.
    get(&d.state, &flat_page()).await;
    let listed = cached(&d.cache, &read_chan(), &created);
    assert_eq!(listed.message.content, full);
    assert!(!listed.content_partial);

    // A pushed edit that is only the start of the text a read brought changes nothing.
    clock.advance(MINUTE);
    let cut = saying(&d, &d.ids[2], "message…");
    assert_eq!(
        ingest(&d.state, pushed("e2", "update", &cut)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        cached(&d.cache, &read_chan(), &d.ids[2]).message.content,
        "message 2"
    );
}

#[tokio::test]
async fn a_message_this_server_posts_is_written_to_the_cache() {
    // The fake mints the first message posted to it the id below, from 2022: the cache's clock is
    // put an hour after it, so the post is inside the cache's age limit.
    let posted_at = MessageId("1000000000000000001".to_owned())
        .created_at_ms()
        .expect("a snowflake");
    let clock = ManualClock::at(posted_at + HOUR);
    let d = deployment(false, 0, &clock).await;
    let (_channel, first, parts) =
        vibe_talk::ops::reply(&d.state, WRITE_CHANNEL, "posted by this server", None)
            .await
            .expect("posted");
    assert_eq!(parts.len(), 1);
    assert_eq!(first.id.as_str(), "1000000000000000001");
    let chan = ChanKey::new(LEGACY, WRITE_CHANNEL);
    let post = cached(&d.cache, &chan, &first.id);
    assert_eq!(post.message.content, "posted by this server");
    assert!(
        !post.content_partial,
        "what a provider answers a post with is the whole message"
    );
    assert_eq!(
        d.cache.memberships(&chan, &first.id),
        Read::Served(vec![CacheScope::Main])
    );
    // A post proves nothing about the rest of the channel.
    assert!(coverage(&d.cache, &chan, &CacheScope::Main).is_empty());
}

#[tokio::test]
async fn a_pushed_change_landing_after_a_pushed_deletion_never_brings_the_message_back() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 5, &clock).await;
    get(&d.state, &flat_page()).await;
    let gone = d.ids[2].clone();
    let stale = d.fake.message(&gone).expect("held");
    clock.advance(MINUTE);
    d.fake.delete(&gone);
    assert_eq!(
        ingest(&d.state, pushed_deletion("e1", &gone)).await,
        StatusCode::ACCEPTED
    );
    assert!(tombstoned(&d.cache, &read_chan(), &gone));

    // Later: an adapter's retry of an earlier edit, and a creation that raced the deletion.
    clock.advance(1_000);
    for (event, kind) in [("e0-retried", "update"), ("e0-raced", "create")] {
        assert_eq!(
            ingest(&d.state, pushed(event, kind, &stale)).await,
            StatusCode::ACCEPTED
        );
        assert!(
            tombstoned(&d.cache, &read_chan(), &gone),
            "a pushed {kind} does not bring a deleted message back"
        );
        assert!(!holds(&d.cache, &read_chan(), &gone));
    }
}

// ---- the live poller ----------------------------------------------------------------------------

use vibe_talk::live::{poll_once, Tick};

/// The provider as the live poller reads it: the next walk forward (a read `after` a cursor) takes
/// its answer from the channel as it stands, then waits until the test lets it go.
struct HeldWalk {
    fake: Arc<FakeDiscord>,
    gate: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl HeldWalk {
    fn holding(fake: &Arc<FakeDiscord>) -> (Arc<Self>, HeldRead) {
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let walk = Arc::new(Self {
            fake: fake.clone(),
            gate: std::sync::Mutex::new(Some((entered_tx, release_rx))),
        });
        (walk, HeldRead { entered, release })
    }
}

#[async_trait::async_trait]
impl vibe_talk::chat::ChatClient for HeldWalk {
    fn provider_name(&self) -> &str {
        vibe_talk::chat::ChatClient::provider_name(self.fake.as_ref())
    }

    async fn identity(&self) -> Result<vibe_talk::chat::ChatIdentity, vibe_talk::chat::ChatError> {
        vibe_talk::chat::ChatClient::identity(self.fake.as_ref()).await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, vibe_talk::chat::ChatError> {
        let answer = vibe_talk::chat::ChatClient::fetch_page(
            self.fake.as_ref(),
            channel,
            limit,
            before,
            after,
        )
        .await;
        let gate = if after.is_some() {
            self.gate.lock().expect("gate").take()
        } else {
            None
        };
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            let _ = release.await;
        }
        answer
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, vibe_talk::chat::ChatError> {
        vibe_talk::chat::ChatClient::post_message(self.fake.as_ref(), channel, content, reply_to)
            .await
    }
}

#[tokio::test]
async fn a_pollers_publish_never_undoes_an_edit_or_a_deletion_learned_while_its_read_was_out() {
    // An edit a later read saw, on Discord's own API; a deletion pushed, on a bridge.
    for bridge in [false, true] {
        let clock = ManualClock::at(T0 + HOUR);
        let d = deployment(bridge, 5, &clock).await;
        let fresh = d
            .fake
            .seed_at(&id(READ_CHANNEL), "Ada", "tpyo", T0 + 30 * MINUTE);
        let (walk, HeldRead { entered, release }) = HeldWalk::holding(&d.fake);
        let mut cursor = Some(d.ids[4].numeric().expect("a snowflake"));
        let hub = Arc::clone(&d.state.live);
        let poll = tokio::spawn(async move {
            poll_once(walk.as_ref(), &hub, &id(READ_CHANNEL), 50, &mut cursor)
                .await
                .map(|tick| tick.published)
        });
        entered.await.expect("the walk reached the provider");

        // While the walk's page is on its way back, the cache learns the message changed.
        clock.advance(MINUTE);
        if bridge {
            d.fake.delete(&fresh);
            assert_eq!(
                ingest(&d.state, pushed_deletion("e1", &fresh)).await,
                StatusCode::ACCEPTED
            );
            assert!(tombstoned(&d.cache, &read_chan(), &fresh));
        } else {
            d.fake.edit(&fresh, "typo fixed");
            get(
                &d.state,
                &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
            )
            .await;
            assert_eq!(
                cached(&d.cache, &read_chan(), &fresh).message.content,
                "typo fixed"
            );
        }

        // The walk comes back later still, and publishes the message as it read it.
        clock.advance(MINUTE);
        drop(release);
        assert_eq!(poll.await.expect("joined").expect("polled"), 1);
        if bridge {
            assert!(
                tombstoned(&d.cache, &read_chan(), &fresh),
                "the deletion stands"
            );
            assert!(!holds(&d.cache, &read_chan(), &fresh));
        } else {
            assert_eq!(
                cached(&d.cache, &read_chan(), &fresh).message.content,
                "typo fixed",
                "the edit stands"
            );
        }
    }
}

#[tokio::test]
async fn the_poller_claims_what_it_walked_only_when_its_page_bound_did_not_stop_it() {
    let clock = ManualClock::at(T0 + HOUR);
    // Discord's own API, ten messages a minute apart from T0.
    let d = deployment(false, 10, &clock).await;
    let channel = id(READ_CHANNEL);
    let mut cursor = None;

    // The seeding read: fifty asked, ten returned, and still no word on where the channel starts.
    let seeded = poll_once(d.fake.as_ref(), &d.state.live, &channel, 50, &mut cursor)
        .await
        .expect("seeded");
    assert_eq!(seeded, Tick::default(), "the seed publishes nothing");
    assert_eq!(held(&d.cache), 10);
    let seed = Interval {
        t0_ms: T0,
        t1_ms: T0 + 9 * MINUTE - SETTLE_MS,
        listed_ms: clock.now(),
        full_ms: Some(clock.now()),
        complete: true,
        source: Source::Read,
    };
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main),
        [seed],
        "from its oldest row, never from the channel's start"
    );

    // Three written since; one page, shorter than asked: the walk ran out of messages.
    for n in 10..13 {
        d.fake
            .seed_at(&channel, "Ada", &format!("message {n}"), T0 + n * MINUTE);
    }
    clock.advance(MINUTE);
    let walked_at = clock.now();
    let tick = poll_once(d.fake.as_ref(), &d.state.live, &channel, 5, &mut cursor)
        .await
        .expect("polled");
    assert_eq!(
        tick,
        Tick {
            published: 3,
            backlog: false,
            replies_behind: false,
        }
    );
    assert_eq!(held(&d.cache), 13);
    let walk = Interval {
        // After the cursor, which was the newest message the seed saw.
        t0_ms: T0 + 9 * MINUTE + 1,
        t1_ms: T0 + 12 * MINUTE - SETTLE_MS,
        listed_ms: walked_at,
        full_ms: Some(walked_at),
        complete: true,
        source: Source::Poll,
    };
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main),
        [seed, walk]
    );
    let head = d
        .cache
        .head(&read_chan(), &CacheScope::Main)
        .served()
        .flatten()
        .expect("a head");
    assert_eq!(
        head.head_ms,
        T0 + 12 * MINUTE,
        "the walk reached the newest end"
    );
    // What the walk published reached the cache as pushes too; the walk's listing confirmed them.
    assert!(
        !cached(
            &d.cache,
            &read_chan(),
            &MessageId::at_time_ms(T0 + 11 * MINUTE)
        )
        .content_partial
    );

    // Twenty more, and pages of two: the page bound stops the walk after eight.
    for n in 13..33 {
        d.fake
            .seed_at(&channel, "Ada", &format!("message {n}"), T0 + n * MINUTE);
    }
    clock.advance(MINUTE);
    let tick = poll_once(d.fake.as_ref(), &d.state.live, &channel, 2, &mut cursor)
        .await
        .expect("polled");
    assert_eq!(
        tick,
        Tick {
            published: 8,
            backlog: true,
            replies_behind: false,
        }
    );
    assert_eq!(held(&d.cache), 21, "what it read is kept");
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main),
        [seed, walk],
        "a walk its page bound stopped claims nothing"
    );

    // The next tick runs out of messages again, and claims what it walked.
    clock.advance(MINUTE);
    let caught_up_at = clock.now();
    let tick = poll_once(d.fake.as_ref(), &d.state.live, &channel, 50, &mut cursor)
        .await
        .expect("polled");
    assert_eq!(
        tick,
        Tick {
            published: 12,
            backlog: false,
            replies_behind: false,
        }
    );
    assert_eq!(
        coverage(&d.cache, &read_chan(), &CacheScope::Main),
        [
            seed,
            walk,
            Interval {
                t0_ms: T0 + 20 * MINUTE + 1,
                t1_ms: T0 + 32 * MINUTE - SETTLE_MS,
                listed_ms: caught_up_at,
                full_ms: Some(caught_up_at),
                complete: true,
                source: Source::Poll,
            }
        ]
    );
}

// ---- the store's summaries follow what the cache learns -----------------------------------------

/// The summaries of a test of edits and deletions, all of messages in the read-only channel.
struct Filed {
    /// The edited message's summary of its old text, of its new text, and of its old text under
    /// an older policy.
    edited_old: SummaryKey,
    edited_new: SummaryKey,
    edited_older_policy: SummaryKey,
    /// The thread sentence of the edited message's thread: filed under a key of its own.
    sentence: SummaryKey,
    /// Two summaries of the message deleted by a push, and one of the message whose absence from
    /// a later read deletes it.
    deleted: [SummaryKey; 2],
    absent: SummaryKey,
    /// A message nothing happens to.
    untouched: SummaryKey,
}

async fn file_for(state: &AppState, store: &FakeStore, ids: &[MessageId]) -> Filed {
    let key = |n: usize, text: &str| summary(state, READ_CHANNEL, ids[n].as_str(), text);
    let filed = Filed {
        edited_old: key(3, "message 3"),
        edited_new: key(3, "message 3, edited upstream"),
        edited_older_policy: SummaryKey {
            version: "v2-an-older-policy".to_owned(),
            ..key(3, "message 3")
        },
        sentence: SummaryKey {
            message: MessageId(format!("thread-sentence:{}", ids[3].as_str())),
            content_hash: 0,
            ..key(3, "")
        },
        deleted: [key(1, "message 1"), key(1, "message 1, as it was once")],
        absent: key(4, "message 4"),
        untouched: key(5, "message 5"),
    };
    file_summaries(
        store,
        &[
            &filed.edited_old,
            &filed.edited_new,
            &filed.edited_older_policy,
            &filed.sentence,
            &filed.deleted[0],
            &filed.deleted[1],
            &filed.absent,
            &filed.untouched,
        ],
    )
    .await;
    filed
}

/// Upstream, message 3 is edited, message 1 deleted (and the adapter pushes both), and message 4
/// deleted silently; then the channel is read again, late enough to settle what it lacks.
async fn edit_and_delete(
    state: &AppState,
    fake: &FakeDiscord,
    ids: &[MessageId],
    clock: &ManualClock,
) {
    clock.advance(MINUTE);
    fake.edit(&ids[3], "message 3, edited upstream");
    let edit = said(fake, &ids[3], "message 3, edited upstream");
    assert_eq!(
        ingest(state, pushed("e1", "update", &edit)).await,
        StatusCode::ACCEPTED
    );
    fake.delete(&ids[1]);
    assert_eq!(
        ingest(state, pushed_deletion("e2", &ids[1])).await,
        StatusCode::ACCEPTED
    );
    fake.delete(&ids[4]);
    clock.advance(15 * MINUTE);
    get(state, &flat_page()).await;
}

#[tokio::test]
async fn an_edit_or_a_deletion_forgets_only_that_messages_summaries_of_other_text() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 10, &clock).await;
    get(&d.state, &flat_page()).await;
    let filed = file_for(&d.state, &d.store, &d.ids).await;

    edit_and_delete(&d.state, &d.fake, &d.ids, &clock).await;
    assert!(
        tombstoned(&d.cache, &read_chan(), &d.ids[4]),
        "the read saw it gone"
    );
    settled(&d.state).await;

    assert!(
        !kept(&d.store, &filed.edited_old).await,
        "a summary of words it no longer says"
    );
    assert!(
        kept(&d.store, &filed.edited_new).await,
        "a summary of what it says now"
    );
    assert!(
        kept(&d.store, &filed.edited_older_policy).await,
        "another policy's entries are the startup sweep's"
    );
    assert!(
        kept(&d.store, &filed.sentence).await,
        "a thread sentence is not the message's"
    );
    for key in &filed.deleted {
        assert!(
            !kept(&d.store, key).await,
            "every summary of a deleted message"
        );
    }
    assert!(
        !kept(&d.store, &filed.absent).await,
        "deleted, as a read found"
    );
    assert!(kept(&d.store, &filed.untouched).await);
}

#[tokio::test]
async fn with_the_cache_off_no_summary_is_deleted() {
    let clock = ManualClock::at(T0 + HOUR);
    let up = upstream(true, 10);
    assert!(
        up.state.message_cache.cache().is_none(),
        "no cache, as a server ships"
    );
    get(&up.state, &flat_page()).await;
    let filed = file_for(&up.state, &up.store, &up.ids).await;

    edit_and_delete(&up.state, &up.fake, &up.ids, &clock).await;
    settled(&up.state).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    for key in [
        &filed.edited_old,
        &filed.edited_new,
        &filed.edited_older_policy,
        &filed.sentence,
        &filed.deleted[0],
        &filed.deleted[1],
        &filed.absent,
        &filed.untouched,
    ] {
        assert!(kept(&up.store, key).await, "{key:?}");
    }
}

#[tokio::test]
async fn the_sweep_forgets_a_channel_off_the_list_but_not_one_still_listed_under_another_key() {
    let clock = ManualClock::at(T0 + HOUR);
    for cache_on in [true, false] {
        let dir = TempDir::new("cache-sweep-summaries");
        {
            let (state, _fake) = keyed_deployment("first");
            let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
            let cache = state.message_cache.cache().expect("installed");
            offer_rows(&cache, "first", READ_CHANNEL, 3, clock.now());
            offer_rows(&cache, "first", WRITE_CHANNEL, 4, clock.now());
            close(&state, cache);
        }
        // The next start: the provider key renamed, and the writable channel gone from the file.
        let (state, _fake, store) = keyed_deployment_of("second", false);
        let renamed = summary(&state, READ_CHANNEL, "1000000000000000001", "row 1");
        let removed = summary(&state, WRITE_CHANNEL, "1000000000000000002", "row 2");
        let name = thread_name(WRITE_CHANNEL);
        file_summaries(&store, &[&renamed, &removed, &name]).await;
        let state = if cache_on {
            testing::with_message_cache(state, dir.path(), clock.clone()).await
        } else {
            // A server with the cache off starts as `main` does, and opens none.
            let (added, hidden) = state.restore_channel_lists().await;
            assert!(added.is_ok() && hidden.is_ok());
            state.refresh_routes();
            state
        };
        settled(&state).await;
        assert!(
            kept(&store, &renamed).await,
            "cache {cache_on}: the same channel, still listed under the new key"
        );
        assert_eq!(
            kept(&store, &removed).await,
            !cache_on,
            "cache {cache_on}: a channel off the list is forgotten only by the cache's sweep"
        );
        assert!(
            kept(&store, &name).await,
            "cache {cache_on}: thread names stay"
        );
        if let Some(cache) = state.message_cache.cache() {
            assert_eq!(held(&cache), 0);
        }
    }
}

// ---- a read that began before a deletion, landing after a read that began after it -------------

/// A request with `token`, for the reads a test makes only to see what the shadow counts.
async fn call_as(state: &AppState, uri: &str, token: &str) -> StatusCode {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers")
        .status()
}

/// What the shadow has counted for one consumer, once every evaluation is counted.
async fn shadow_of(state: &AppState, consumer: &str) -> vibe_talk::cache::shadow::ConsumerCounts {
    assert!(state.message_cache.shadow_settled(WAIT).await);
    state
        .message_cache
        .shadow_counts()
        .into_iter()
        .find(|counts| counts.consumer == consumer)
        .expect("a consumer the shadow counts")
}

/// Whether some complete claim of the read channel's `main` holds the instant `at_ms`.
fn proven(cache: &MessageCache, at_ms: i64) -> bool {
    coverage(cache, &read_chan(), &CacheScope::Main)
        .iter()
        .any(|claim| claim.complete && claim.t0_ms <= at_ms && at_ms < claim.t1_ms)
}

/// On a cache that never held the message (a fresh one, as after an erase), a `/messages` read that
/// began before an older message was deleted lands after a `/timeline` read that began after the
/// deletion and proved that stretch of the channel without it. The earlier read must not put the
/// message back: its absence from the later listing is a deletion, whether or not the file held it.
#[tokio::test]
async fn a_read_that_began_before_a_deletion_never_puts_back_a_message_a_later_listing_proved_gone()
{
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(false, 20, &clock).await;
    let HeldRead { entered, release } = d.chat.hold_next_recent();
    let slow = {
        let state = d.state.clone();
        tokio::spawn(async move {
            call(
                &state,
                "GET",
                &format!("/api/v1/channels/{READ_CHANNEL}/messages?limit=50"),
            )
            .await
            .0
        })
    };
    entered.await.expect("the slow read took its answer");

    clock.advance(MINUTE);
    let gone = d.ids[5].clone();
    d.fake.delete(&gone);
    get(
        &d.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
    )
    .await;
    assert!(!holds(&d.cache, &read_chan(), &gone));
    assert!(
        proven(&d.cache, T0 + 5 * MINUTE),
        "the later read proved the stretch"
    );

    clock.advance(MINUTE);
    drop(release);
    assert_eq!(slow.await.expect("joined"), StatusCode::OK);
    assert!(
        !holds(&d.cache, &read_chan(), &gone),
        "a message deleted before the later read began is in the file again"
    );
    assert!(
        tombstoned(&d.cache, &read_chan(), &gone),
        "its absence is a deletion"
    );
    assert!(
        holds(&d.cache, &read_chan(), &d.ids[6]),
        "the rest of the slow read landed"
    );
}

/// The same race through the live poller: its walk forward takes a burst that includes a message,
/// the message is deleted and a write-scope read lists the channel without it, and only then does
/// the walk publish what it read. The cache must not hold the deleted message, nor count it as one
/// it would serve by id.
#[tokio::test]
async fn a_pollers_publish_never_inserts_a_message_a_later_listing_proved_gone() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(false, 5, &clock).await;
    let mut cursor = None;
    poll_once(
        d.state.chat.as_ref(),
        &d.state.live,
        &id(READ_CHANNEL),
        50,
        &mut cursor,
    )
    .await
    .expect("the first read seeds the cursor");

    // A burst: one message that will be deleted, then more, past the unsettled band.
    let doomed = d
        .fake
        .seed_at(&id(READ_CHANNEL), "Ada", "doomed words", T0 + 5 * MINUTE);
    for n in 6..20 {
        d.fake.seed_at(
            &id(READ_CHANNEL),
            "Ada",
            &format!("message {n}"),
            T0 + n * MINUTE,
        );
    }
    clock.advance(MINUTE);
    let (walk, HeldRead { entered, release }) = HeldWalk::holding(&d.fake);
    let hub = Arc::clone(&d.state.live);
    let poll = tokio::spawn(async move {
        poll_once(walk.as_ref(), &hub, &id(READ_CHANNEL), 50, &mut cursor)
            .await
            .map(|tick| tick.published)
    });
    entered.await.expect("the walk took its page");

    // While the walk's page is on its way back, the message is deleted and a read lists without it.
    clock.advance(MINUTE);
    d.fake.delete(&doomed);
    get(
        &d.state,
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
    )
    .await;
    assert!(!holds(&d.cache, &read_chan(), &doomed));
    assert!(
        proven(&d.cache, T0 + 5 * MINUTE),
        "the read proved the stretch"
    );

    clock.advance(MINUTE);
    drop(release);
    assert_eq!(poll.await.expect("joined").expect("polled"), 15);
    assert!(
        !holds(&d.cache, &read_chan(), &doomed),
        "the poller's publish put a message a later listing proved gone into the file"
    );

    // Upstream no longer has it, and the cache must not count it as one it would serve.
    clock.advance(5_000);
    let status = call_as(
        &d.state,
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/messages/{}",
            doomed.as_str()
        ),
        testing::READ_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let counted = shadow_of(&d.state, "message_by_id").await;
    assert_eq!(
        (counted.would_serve, counted.extra_rows),
        (0, 0),
        "{counted:?}"
    );
}

/// The newest page of the read channel's `main`, and what the shadow counts of it, at read scope.
async fn fallback_of(state: &AppState) -> vibe_talk::cache::shadow::ConsumerCounts {
    let status = call_as(
        state,
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=10"),
        testing::READ_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    shadow_of(state, "timeline_fallback").await
}

fn head_of(cache: &MessageCache) -> vibe_talk::cache::coverage::Head {
    assert!(cache.quiesce_blocking(WAIT));
    cache
        .head(&read_chan(), &CacheScope::Main)
        .served()
        .expect("a read")
        .expect("a head")
}

/// The most ordinary deletion there is: a message deleted moments after it was posted, so that it
/// lies in the unsettled band of the read that began after the deletion (or above that read's
/// newest message), where a listing proves nothing. A read that saw it began before the deletion,
/// and lands after that listing. The message may be held again, but the head stays where the later
/// read left it, the message is never one the cache would serve, and the listing that settles its
/// stretch takes it out.
#[tokio::test]
async fn a_message_deleted_moments_after_it_was_posted_is_never_served_when_an_earlier_read_lands_late(
) {
    for (label, at) in [
        ("in the band", T0 + 18 * MINUTE + 30_000),
        ("above the head", T0 + 19 * MINUTE + 30_000),
    ] {
        let clock = ManualClock::at(T0 + HOUR);
        let d = deployment(false, 20, &clock).await;
        let oops = d
            .fake
            .seed_at(&id(READ_CHANNEL), "Ada", "oops, wrong channel", at);
        let HeldRead { entered, release } = d.chat.hold_next_recent();
        let slow = {
            let state = d.state.clone();
            tokio::spawn(async move {
                call(
                    &state,
                    "GET",
                    &format!("/api/v1/channels/{READ_CHANNEL}/messages?limit=50"),
                )
                .await
                .0
            })
        };
        entered.await.expect("the slow read took its answer");

        clock.advance(MINUTE);
        d.fake.delete(&oops);
        get(
            &d.state,
            &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
        )
        .await;
        assert!(!holds(&d.cache, &read_chan(), &oops), "{label}");
        let head = head_of(&d.cache);

        clock.advance(1_000);
        drop(release);
        assert_eq!(slow.await.expect("joined"), StatusCode::OK);
        assert_eq!(
            head_of(&d.cache),
            head,
            "{label}: the earlier read moved the head to a message the later one did not list"
        );

        clock.advance(1_000);
        let fallback = fallback_of(&d.state).await;
        assert_eq!(
            (fallback.would_serve, fallback.extra_rows),
            (0, 0),
            "{label}: {fallback:?}"
        );
    }
}

/// The same with no race at all: a write-scope listing holds a just-posted message; it is deleted;
/// a listing that began after the deletion lacks it, and its absence inside the band proves
/// nothing yet. The message stays in the file until that stretch is settled, and is never one the
/// cache would serve; a listing ten minutes on settles the band and takes it out.
#[tokio::test]
async fn a_message_deleted_moments_after_it_was_posted_is_never_served_from_the_band() {
    for (label, at) in [
        ("in the band", T0 + 18 * MINUTE + 30_000),
        ("above the head", T0 + 19 * MINUTE + 30_000),
    ] {
        let clock = ManualClock::at(T0 + HOUR);
        let d = deployment(false, 20, &clock).await;
        let oops = d
            .fake
            .seed_at(&id(READ_CHANNEL), "Ada", "oops, wrong channel", at);
        let main = format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50");
        get(&d.state, &main).await;
        assert!(holds(&d.cache, &read_chan(), &oops), "{label}");
        clock.advance(MINUTE);
        d.fake.delete(&oops);
        get(&d.state, &main).await;
        assert!(
            holds(&d.cache, &read_chan(), &oops),
            "{label}: inside the band an absence proves nothing yet"
        );
        clock.advance(1_000);
        let fallback = fallback_of(&d.state).await;
        assert_eq!(
            (fallback.would_serve, fallback.extra_rows),
            (0, 0),
            "{label}: {fallback:?}"
        );
        // Ten minutes on, the band settles, and the listing that settles it takes the message out.
        clock.advance(10 * MINUTE);
        get(&d.state, &main).await;
        assert!(
            tombstoned(&d.cache, &read_chan(), &oops),
            "{label}: the listing that settled its stretch took it out"
        );
        clock.advance(1_000);
        let fallback = fallback_of(&d.state).await;
        assert_eq!(
            (fallback.would_serve, fallback.extra_rows),
            (1, 0),
            "{label}: what the settled listing holds is served, and the message is not: {fallback:?}"
        );
    }
}

// ---- a change left out while the account is checked again ---------------------------------------

/// A bridge whose identity answer the test can hold, to stand for a provider slow to say which
/// account it is.
struct SlowIdentity {
    inner: Arc<TimelineChat>,
    open: tokio::sync::watch::Sender<bool>,
}

#[async_trait::async_trait]
impl vibe_talk::chat::ChatClient for SlowIdentity {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn supports_threading(&self) -> bool {
        true
    }

    async fn identity(&self) -> Result<vibe_talk::chat::ChatIdentity, vibe_talk::chat::ChatError> {
        let mut open = self.open.subscribe();
        let _ = open.wait_for(|open| *open).await;
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, vibe_talk::chat::ChatError> {
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, vibe_talk::chat::ChatError> {
        self.inner.fetch_recent(channel, limit).await
    }

    async fn fetch_recent_dated(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<vibe_talk::chat::RecentRead, vibe_talk::chat::ChatError> {
        self.inner.fetch_recent_dated(channel, limit).await
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &vibe_talk::threads::TimelineRequest,
    ) -> Result<vibe_talk::threads::TimelinePage, vibe_talk::chat::ChatError> {
        self.inner.fetch_timeline(channel, request).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, vibe_talk::chat::ChatError> {
        self.inner.post_message(channel, content, reply_to).await
    }
}

/// A live-fed bridge: the account is checked again and its answer is slow, so an adapter's edit of
/// an older message is left out. Once the account answers, the page's first look, a two-row Flat
/// page, re-reads only the newest messages: the cache must still not vouch for the older one, whose
/// text in the file is the text from before the edit.
#[tokio::test]
async fn a_short_flat_page_never_vouches_for_an_older_message_whose_pushed_edit_was_left_out() {
    let clock = ManualClock::at(T0 + HOUR);
    let Upstream {
        mut state,
        fake,
        ids,
        ..
    } = upstream(true, 8);
    let slow = Arc::new(SlowIdentity {
        inner: Arc::new(TimelineChat::new(fake.clone())),
        open: tokio::sync::watch::Sender::new(true),
    });
    state.replace_chat(slow.clone());
    let dir = TempDir::new("cache-left-out-edit");
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let cache = state.message_cache.cache().expect("installed");
    get(&state, &flat_page()).await;
    assert_eq!(held(&cache), 8);

    slow.open.send_replace(false);
    state.message_cache.reverify();
    clock.advance(MINUTE);
    let edit = said(&fake, &ids[1], "message 1, edited upstream");
    fake.edit(&ids[1], "message 1, edited upstream");
    assert_eq!(
        ingest(&state, pushed("left-out", "update", &edit)).await,
        StatusCode::ACCEPTED
    );
    assert!(cache.quiesce_blocking(WAIT));
    assert!(cache.is_suspect(&read_chan()), "the edit was left out");
    slow.open.send_replace(true);
    assert!(state.message_cache.identities_verified(WAIT).await);

    clock.advance(MINUTE);
    get(
        &state,
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=flat&limit=2"),
    )
    .await;
    assert_eq!(
        cached(&cache, &read_chan(), &ids[1]).message.content,
        "message 1",
        "the short page did not reach it"
    );
    assert!(
        cache.is_suspect(&read_chan()),
        "nothing re-read the older message"
    );
    let (status, _) = call(
        &state,
        "GET",
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/messages/{}",
            ids[1].as_str()
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let counted = shadow_of(&state, "message_by_id").await;
    assert_eq!(
        (counted.would_serve, counted.misses.suspect),
        (0, 1),
        "{counted:?}"
    );

    // A read of the whole channel that began after the edit was left out takes it in.
    clock.advance(MINUTE);
    get(&state, &flat_page()).await;
    assert_eq!(
        cached(&cache, &read_chan(), &ids[1]).message.content,
        "message 1, edited upstream"
    );
    assert!(!cache.is_suspect(&read_chan()));
}

// ---- the live feed vouches only for what it has carried -----------------------------------------

/// A live-fed bridge, filled, then stopped. While no server runs, a message is deleted upstream:
/// no push reaches anything, and nothing records a drop. The next start, past the bridge's
/// twenty-four hours, asks for the message by id: the live feed that carries deletions has carried
/// none of that downtime's, so it vouches for nothing seen before this start.
#[tokio::test]
async fn a_deletion_made_while_the_server_was_down_is_never_vouched_for() {
    let clock = ManualClock::at(T0 + HOUR);
    let dir = TempDir::new("cache-fill-downtime");
    {
        let Upstream { state, .. } = upstream(true, 8);
        let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
        get(&state, &flat_page()).await;
        let cache = state.message_cache.cache().expect("installed");
        assert_eq!(held(&cache), 8);
        close(&state, cache);
    }
    clock.advance(25 * HOUR);
    let Upstream {
        state, fake, ids, ..
    } = upstream(true, 8);
    fake.delete(&ids[2]);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    clock.advance(MINUTE);
    let by_id =
        |id: &MessageId| format!("/api/v1/channels/{READ_CHANNEL}/messages/{}", id.as_str());
    let status = call_as(&state, &by_id(&ids[2]), testing::READ_TOKEN).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let counted = shadow_of(&state, "message_by_id").await;
    assert_eq!(
        (counted.would_serve, counted.extra_rows),
        (0, 0),
        "a message deleted while the server was down is vouched for: {counted:?}"
    );
    assert_eq!(counted.misses.stale_head, 1, "{counted:?}");

    // A read of the whole channel since this start revalidates what it lists, and from then on
    // the feed carries every deletion: a message it lists is vouched for, past the read's own
    // twenty-four hours too.
    get(&state, &flat_page()).await;
    clock.advance(25 * HOUR);
    let status = call_as(&state, &by_id(&ids[3]), testing::READ_TOKEN).await;
    assert_eq!(status, StatusCode::OK);
    let counted = shadow_of(&state, "message_by_id").await;
    assert_eq!(
        (counted.would_serve, counted.agreed),
        (1, 1),
        "seen since this start, on a live feed that carries deletions: {counted:?}"
    );
}

// ---- a write that fails ------------------------------------------------------------------------

/// A live-fed bridge. An adapter's edit of an older message is queued, but its write fails: here
/// another connection holds the file's write lock past the writer's busy timeout, as a full disk,
/// an I/O error or an operator's own session would make any write fail. The edit is then left out
/// of the file exactly as a full queue's drop would leave it, so the channel is suspect at once and
/// the cache never vouches for the old text; a read that began afterwards takes the edit in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pushed_edit_whose_write_failed_is_never_vouched_for() {
    let clock = ManualClock::at(T0 + HOUR);
    let d = deployment(true, 8, &clock).await;
    get(&d.state, &flat_page()).await;
    assert_eq!(held(&d.cache), 8);
    assert!(!d.cache.is_suspect(&read_chan()));

    let path = d.cache.path().to_owned();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let locker = std::thread::spawn(move || {
        let connection = rusqlite::Connection::open(&path).expect("open");
        connection.execute_batch("BEGIN IMMEDIATE").expect("lock");
        locked_tx.send(()).expect("tell");
        let _ = release_rx.recv();
        connection.execute_batch("ROLLBACK").expect("unlock");
    });
    locked_rx.recv().expect("locked");
    clock.advance(MINUTE);
    let edit = said(&d.fake, &d.ids[1], "message 1, edited upstream");
    d.fake.edit(&d.ids[1], "message 1, edited upstream");
    assert_eq!(
        ingest(&d.state, pushed("write-fails", "update", &edit)).await,
        StatusCode::ACCEPTED
    );
    // The batch, and the edit's write alone, each wait out the busy timeout, and fail.
    let failing = std::time::Instant::now();
    while d.cache.stats().ops_failed == 0 {
        assert!(failing.elapsed() < WAIT, "the write never failed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        d.cache.last_drop_ms(&read_chan()).is_some(),
        "the failed write is a drop, in memory at once"
    );
    release_tx.send(()).expect("release");
    locker.join().expect("locker");
    assert!(d.cache.quiesce_blocking(WAIT));
    assert_eq!(
        cached(&d.cache, &read_chan(), &d.ids[1]).message.content,
        "message 1",
        "the edit never reached the file"
    );
    assert!(d.cache.is_suspect(&read_chan()));

    clock.advance(1_000);
    let status = call_as(
        &d.state,
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/messages/{}",
            d.ids[1].as_str()
        ),
        testing::READ_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let counted = shadow_of(&d.state, "message_by_id").await;
    assert_eq!(
        (
            counted.would_serve,
            counted.content_differs,
            counted.misses.suspect
        ),
        (0, 0, 1),
        "the old text vouched for after its edit's write failed: {counted:?}"
    );

    // A read of the whole channel that began afterwards takes the edit in, and lifts the doubt.
    clock.advance(MINUTE);
    get(&d.state, &flat_page()).await;
    assert_eq!(
        cached(&d.cache, &read_chan(), &d.ids[1]).message.content,
        "message 1, edited upstream"
    );
    assert!(!d.cache.is_suspect(&read_chan()));
}
