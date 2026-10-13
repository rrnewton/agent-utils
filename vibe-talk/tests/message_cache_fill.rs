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
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{self, ManualClock, TempDir, READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};

/// 2026-08-18T12:00:00Z: where the fixtures' messages are written.
const T0: i64 = 1_787_054_400_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const WAIT: Duration = Duration::from_secs(20);

/// The provider key a single-provider test deployment names its provider by.
const LEGACY: &str = vibe_talk::config::LEGACY_DISCORD_PROVIDER_KEY;

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

[[channels]]
id = "{WRITE_CHANNEL}"
label = "lead team"
writable = true
provider = "{key}"
"#,
        read = testing::READ_TOKEN,
    );
    let (mut state, fake, _voice) = testing::state_from_toml(&text);
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
    (state, fake)
}

#[tokio::test]
async fn a_channel_the_owner_hides_is_swept_out_of_the_cache() {
    let dir = TempDir::new("cache-sweep-hidden");
    let clock = ManualClock::at(T0 + HOUR);
    let (state, _fake, _store) = testing::state_with_store();
    let state = testing::with_message_cache(state, dir.path(), clock.clone());
    let cache = state.message_cache.cache().expect("installed");
    offer_rows(&cache, LEGACY, READ_CHANNEL, 3, clock.now());
    offer_rows(&cache, LEGACY, WRITE_CHANNEL, 4, clock.now());
    assert_eq!(held(&cache), 7);

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
}

#[tokio::test]
async fn a_channel_taken_out_of_the_configuration_is_swept_at_the_next_start() {
    let dir = TempDir::new("cache-sweep-config");
    let clock = ManualClock::at(T0 + HOUR);
    {
        let (state, _fake) = testing::state();
        let state = testing::with_message_cache(state, dir.path(), clock.clone());
        let cache = state.message_cache.cache().expect("installed");
        offer_rows(&cache, LEGACY, READ_CHANNEL, 3, clock.now());
        offer_rows(&cache, LEGACY, WRITE_CHANNEL, 4, clock.now());
        assert_eq!(held(&cache), 7);
        close(&state, cache);
    }
    let (state, _fake, _voice) = testing::state_from_toml(&config_without_the_write_channel());
    let state = testing::with_message_cache(state, dir.path(), clock.clone());
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(
        held(&cache),
        3,
        "the channel the file no longer names is gone"
    );
}

#[tokio::test]
async fn a_renamed_provider_key_leaves_nothing_behind_under_the_old_one() {
    let dir = TempDir::new("cache-sweep-rename");
    let clock = ManualClock::at(T0 + HOUR);
    {
        let (state, _fake) = keyed_deployment("first");
        let state = testing::with_message_cache(state, dir.path(), clock.clone());
        let cache = state.message_cache.cache().expect("installed");
        offer_rows(&cache, "first", READ_CHANNEL, 3, clock.now());
        offer_rows(&cache, "first", WRITE_CHANNEL, 4, clock.now());
        assert_eq!(held(&cache), 7);
        close(&state, cache);
    }
    let (state, _fake) = keyed_deployment("second");
    let state = testing::with_message_cache(state, dir.path(), clock.clone());
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(
        held(&cache),
        0,
        "the same channels under a provider key the configuration no longer has are gone"
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
    {
        let (state, fake) = testing::state_with(store.clone());
        fake.register_channel(&added);
        let cache = testing::install_message_cache(&state, dir.path(), clock.clone());
        let (from_store, hidden) = state.restore_channel_lists().await;
        assert_eq!(from_store.expect("added channels"), 1);
        assert_eq!(hidden.expect("hidden channels"), 0);
        assert!(state.message_cache.is_armed());
        offer_rows(&cache, LEGACY, added.as_str(), 5, clock.now());
        offer_rows(&cache, LEGACY, READ_CHANNEL, 3, clock.now());
        assert_eq!(held(&cache), 8);
        close(&state, cache);
    }

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
    assert!(
        state
            .message_cache
            .begin_for(vibe_talk::auth::Scope::Write, &id(READ_CHANNEL))
            .is_none(),
        "and nothing is filled"
    );
}
