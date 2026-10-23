//! `#208 persistent-cache`: what decides whether the message cache file exists — the
//! `[message_cache]` table, the owner's switch beside the store, and the erases that follow
//! them — exercised through the server's own startup and its routes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::cache::lifecycle::{self, Started, Trouble};
use vibe_talk::cache::{
    file, marker, CacheOptions, ChanKey, Incoming, MessageCache, Observation, Scope as CacheScope,
};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::state::AppState;
use vibe_talk::testing::{
    self, LogCapture, ManualClock, TempDir, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN,
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MINUTE: i64 = 60_000;
/// 2026-10-01T00:00:00Z.
const T0: i64 = 1_790_812_800_000;
const WAIT: Duration = Duration::from_secs(20);
/// The provider key a single-provider test deployment names its provider by.
const LEGACY: &str = vibe_talk::config::LEGACY_DISCORD_PROVIDER_KEY;

/// The store's file. Its directory is private, as the real store makes it.
fn store_path(dir: &Path) -> PathBuf {
    dir.join("state").join("store.sqlite3")
}

/// The cache file at the default path, beside the store.
fn default_cache(dir: &Path) -> PathBuf {
    file::default_path(&store_path(dir), &file::host_name())
}

fn sidecar(dir: &Path) -> PathBuf {
    marker::sidecar_path(&store_path(dir))
}

/// The test server's configuration, with `table` as its `[message_cache]` body, or none.
fn config_text(dir: &Path, table: Option<&str>) -> String {
    let mut text = format!(
        "{}\n[storage]\npath = \"{}\"\n",
        testing::config_toml(),
        store_path(dir).display()
    );
    if let Some(table) = table {
        text.push_str("\n[message_cache]\n");
        text.push_str(table);
        text.push('\n');
    }
    text
}

/// A server started as `main` starts one: the lifecycle first, then the sink armed with the
/// channels it answers for.
async fn server(dir: &Path, table: Option<&str>, clock: &Arc<ManualClock>) -> (AppState, Started) {
    file::ensure_private_dir(&dir.join("state")).expect("a private store directory");
    let (state, _discord, _store) = testing::state_with_store_from_toml(&config_text(dir, table));
    let started = lifecycle::start_with(
        &state.config,
        &state.message_cache,
        Arc::clone(clock) as Arc<dyn vibe_talk::cache::Clock>,
    )
    .await;
    state.message_cache.arm();
    state.refresh_routes();
    (state, started)
}

/// Stop a server's cache, as a process exiting would: its handle let go, and its lock with it.
fn stop(state: AppState) {
    drop(state.message_cache.uninstall());
    drop(state);
}

/// A cache file made at `path` and closed again.
fn leave_a_cache_at(path: &Path) {
    drop(MessageCache::open(CacheOptions::new(path)).expect("a cache opens"));
    assert!(path.exists());
}

const ON: &str = "enabled = true\nmax_days = 14";

#[tokio::test]
async fn enabled_false_at_startup_erases_registered_and_default_directory_files() {
    let scratch = TempDir::new("cache-switch-config-off");
    let dir = scratch.path();
    file::ensure_private_dir(&dir.join("state")).expect("dir");
    // One the registry names, somewhere else; one in the store's directory the registry does not
    // name (an earlier default path on another host name); and a file in the store's directory
    // named like a cache that is not one of ours.
    let elsewhere = dir.join("elsewhere").join("old.message-cache-h.sqlite3");
    leave_a_cache_at(&elsewhere);
    let unregistered = dir.join("state").join("store.message-cache-older.sqlite3");
    leave_a_cache_at(&unregistered);
    let foreign = dir.join("state").join("notes.message-cache-x.sqlite3");
    std::fs::write(&foreign, b"somebody's notes").expect("write");
    // And another store's cache beside this one: another server's file, which it may only have
    // stopped holding for a deploy.
    let neighbour = dir.join("state").join("neighbour.message-cache-h.sqlite3");
    leave_a_cache_at(&neighbour);
    marker::register(&sidecar(dir), &elsewhere).expect("register");

    let clock = ManualClock::at(T0);
    let (state, started) = server(dir, Some("enabled = false"), &clock).await;
    assert!(
        matches!(started, Started::Erased { bytes } if bytes > 0),
        "{started:?}"
    );
    assert!(!elsewhere.exists(), "a registered cache was left");
    assert!(
        !unregistered.exists(),
        "a cache in the store's directory was left"
    );
    assert_eq!(
        std::fs::read(&foreign).expect("kept"),
        b"somebody's notes",
        "a file that is not a cache was touched"
    );
    assert!(neighbour.exists(), "another store's cache was erased");
    assert!(state.message_cache.cache().is_none());
    assert!(!default_cache(dir).exists(), "a disabled cache was created");
    assert!(
        !marker::load(&sidecar(dir)).expect("load").off,
        "the configuration's off is not the owner's"
    );
}

#[tokio::test]
async fn the_owners_off_recorded_beside_the_store_erases_at_the_next_start_and_opens_nothing() {
    let scratch = TempDir::new("cache-switch-marker");
    let dir = scratch.path();
    let clock = ManualClock::at(T0);
    let (state, started) = server(dir, Some(ON), &clock).await;
    assert_eq!(started, Started::Opened);
    assert!(state.message_cache.cache().is_some_and(|c| c.is_open()));
    assert!(default_cache(dir).exists());
    assert_eq!(
        marker::load(&sidecar(dir)).expect("load").paths,
        vec![default_cache(dir)],
        "the file was not registered"
    );
    stop(state);

    // The marker written, and the process gone before it erased: what a crash between the two
    // leaves.
    marker::mark_off(&sidecar(dir), T0).expect("mark");
    let (state, started) = server(dir, Some(ON), &clock).await;
    assert!(
        matches!(started, Started::Erased { bytes } if bytes > 0),
        "{started:?}"
    );
    assert!(
        !default_cache(dir).exists(),
        "the cache survived the marker"
    );
    assert!(state.message_cache.cache().is_none(), "a cache was opened");
    assert!(marker::load(&sidecar(dir)).expect("load").off);
}

#[tokio::test]
async fn an_absent_table_is_warned_about_at_every_start_and_erased_only_after_seven_days() {
    let scratch = TempDir::new("cache-switch-absent");
    let dir = scratch.path();
    let clock = ManualClock::at(T0);
    let (state, started) = server(dir, Some(ON), &clock).await;
    assert_eq!(started, Started::Opened);
    stop(state);

    for at in [T0, T0 + 3 * DAY_MS, T0 + 7 * DAY_MS - 60_000] {
        clock.set(at);
        let capture = LogCapture::start();
        let (state, started) = server(dir, None, &clock).await;
        assert!(
            matches!(started, Started::Left { bytes } if bytes > 0),
            "{started:?}"
        );
        assert!(default_cache(dir).exists(), "erased before seven days");
        assert!(
            state.message_cache.cache().is_none(),
            "opened without a table"
        );
        let log = capture.text();
        assert!(
            log.contains("WARN") && log.contains("present and unused"),
            "the leftover cache was not warned about:\n{log}"
        );
        stop(state);
    }
    clock.set(T0 + 7 * DAY_MS);
    let (state, started) = server(dir, None, &clock).await;
    assert!(
        matches!(started, Started::Erased { bytes } if bytes > 0),
        "{started:?}"
    );
    assert!(!default_cache(dir).exists(), "kept beyond seven days");
    stop(state);
}

#[tokio::test]
async fn a_server_that_never_had_a_cache_leaves_no_trace_of_one() {
    let scratch = TempDir::new("cache-switch-never");
    let dir = scratch.path();
    let clock = ManualClock::at(T0);
    for table in [None, Some("enabled = false")] {
        let (state, started) = server(dir, table, &clock).await;
        assert!(
            matches!(started, Started::Untouched | Started::Erased { bytes: 0 }),
            "{started:?}"
        );
        assert!(state.message_cache.cache().is_none());
        stop(state);
        let left: Vec<_> = std::fs::read_dir(dir.join("state"))
            .expect("dir")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(left.is_empty(), "{table:?} left {left:?} beside the store");
    }
}

#[tokio::test]
async fn a_cache_whose_path_cannot_be_registered_first_is_not_opened() {
    let scratch = TempDir::new("cache-switch-unregistered");
    let dir = scratch.path();
    file::ensure_private_dir(&dir.join("state")).expect("dir");
    // The sidecar is written through a temporary file beside it; a directory in that place makes
    // the write fail whoever runs the test.
    let mut temporary = sidecar(dir).into_os_string();
    temporary.push(".tmp");
    std::fs::create_dir(&temporary).expect("block the write");
    let clock = ManualClock::at(T0);
    let (state, started) = server(dir, Some(ON), &clock).await;
    assert_eq!(started, Started::Failed(Trouble::SwitchUnwritable));
    assert!(state.message_cache.cache().is_none());
    assert!(
        !default_cache(dir).exists(),
        "a cache file was made that no later \"off\" could find"
    );
    assert_eq!(
        state.message_cache.lifecycle().and_then(|l| l.trouble()),
        Some(Trouble::SwitchUnwritable)
    );
}

/// A message of the read-only channel really written at `at_ms`.
fn message(at_ms: i64, text: &str) -> Message {
    Message {
        edited_timestamp: None,
        mentions: Vec::new(),
        upstream_url: None,
        thread: None,
        id: MessageId::at_time_ms(at_ms),
        channel_id: ChannelId(READ_CHANNEL.to_owned()),
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

/// Offer `count` rows of the read-only channel straight to the cache, as a read of it would, and
/// wait until they are applied.
fn fill(cache: &MessageCache, count: i64, now_ms: i64) {
    let chan = ChanKey::new(LEGACY, READ_CHANNEL);
    let rows = (0..count)
        .map(|n| {
            Incoming::new(
                message(T0 + n * MINUTE, &format!("row {n}")),
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

/// A server with the cache on, its accounts checked, and `count` rows in it.
async fn filled_server(dir: &Path, count: i64) -> (AppState, Arc<ManualClock>) {
    let clock = ManualClock::at(T0 + 60 * MINUTE);
    let (state, started) = server(dir, Some(ON), &clock).await;
    assert_eq!(started, Started::Opened);
    assert!(state.message_cache.identities_verified(WAIT).await);
    let cache = state.message_cache.cache().expect("open");
    fill(&cache, count, clock.now());
    assert_eq!(held(&cache), u64::try_from(count).expect("small"));
    (state, clock)
}

async fn send(
    state: &AppState,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
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

async fn switch(state: &AppState, enabled: bool) -> (StatusCode, Value) {
    send(
        state,
        "PUT",
        "/api/v1/message-cache",
        WRITE_TOKEN,
        Some(json!({ "enabled": enabled })),
    )
    .await
}

async fn erase(state: &AppState) -> (StatusCode, Value) {
    send(state, "DELETE", "/api/v1/message-cache", WRITE_TOKEN, None).await
}

/// Make every write of the sidecar fail, whoever runs the test: it is written through a temporary
/// file beside it, and a directory stands in that file's place. Returns what to remove to undo it.
fn block_the_sidecar(dir: &Path) -> PathBuf {
    let mut temporary = sidecar(dir).into_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::create_dir(&temporary).expect("block the write");
    temporary
}

#[tokio::test]
async fn off_writes_the_marker_before_it_erases_and_a_marker_it_cannot_write_erases_nothing() {
    let scratch = TempDir::new("cache-switch-off");
    let dir = scratch.path();
    let (state, clock) = filled_server(dir, 5).await;
    let cache = state.message_cache.cache().expect("open");

    let blocked = block_the_sidecar(dir);
    let (status, body) = switch(&state, false).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"], "message_cache_switch_unsaved");
    assert!(cache.is_open(), "the file was erased without its marker");
    assert_eq!(held(&cache), 5, "rows went without the marker");
    assert!(state.message_cache.cache().is_some());
    assert!(!marker::load(&sidecar(dir)).expect("load").off);
    std::fs::remove_dir(&blocked).expect("unblock");

    let (status, body) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["available"], false);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["reason"], "switched_off");
    assert_eq!(body["bytes"], 0);
    assert_eq!(body["max_days"], 14);
    assert!(!cache.is_open());
    drop(cache);
    assert!(state.message_cache.cache().is_none());
    assert!(!default_cache(dir).exists(), "the file outlived the switch");
    assert!(marker::load(&sidecar(dir)).expect("load").off);
    stop(state);

    // The switch outlives the process: the next start erases and opens nothing.
    let (state, started) = server(dir, Some(ON), &clock).await;
    assert!(matches!(started, Started::Erased { .. }), "{started:?}");
    assert!(state.message_cache.cache().is_none());
    assert!(!default_cache(dir).exists());
}

#[tokio::test]
async fn on_is_refused_unless_the_configuration_enables_the_cache() {
    let scratch = TempDir::new("cache-switch-refused");
    let dir = scratch.path();
    let clock = ManualClock::at(T0);
    let (state, _) = server(dir, None, &clock).await;
    for (method, body) in [
        ("PUT", Some(json!({ "enabled": true }))),
        ("PUT", Some(json!({ "enabled": false }))),
        ("DELETE", None),
    ] {
        let (status, answer) =
            send(&state, method, "/api/v1/message-cache", WRITE_TOKEN, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method}: {answer}");
        assert_eq!(answer["error"], "message_cache_not_configured");
    }
    stop(state);

    let (state, _) = server(dir, Some("enabled = false"), &clock).await;
    let (status, body) = switch(&state, true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "message_cache_not_enabled");
    assert!(state.message_cache.cache().is_none());
    assert!(!default_cache(dir).exists(), "a disabled cache was opened");
    assert!(
        std::fs::symlink_metadata(sidecar(dir)).is_err(),
        "a refused switch wrote the marker"
    );
    // Off is the configuration's word too, and is answered as it stands.
    let (status, body) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["reason"], "config_disabled");
    // Neither is open to a read token.
    for (method, body) in [("PUT", Some(json!({ "enabled": true }))), ("DELETE", None)] {
        let (status, _) = send(&state, method, "/api/v1/message-cache", READ_TOKEN, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
    }
}

#[tokio::test]
async fn on_after_off_opens_a_fresh_empty_file_and_checks_the_accounts_again() {
    let scratch = TempDir::new("cache-switch-on");
    let dir = scratch.path();
    let (state, _clock) = filled_server(dir, 5).await;
    let (status, body) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = switch(&state, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["available"], true, "{body}");
    assert_eq!(body["enabled"], true);
    assert!(body.get("reason").is_none(), "{body}");
    assert_eq!(body["messages"], 0, "{body}");
    assert!(!marker::load(&sidecar(dir)).expect("load").off);
    let cache = state.message_cache.cache().expect("installed");
    assert!(cache.is_open() && default_cache(dir).exists());
    assert_eq!(held(&cache), 0, "the fresh file holds something");
    // The account table went with the old file: nothing of the provider is filled until it has
    // been asked again, and the answer written to the new one.
    assert!(state.message_cache.identities_verified(WAIT).await);
    let recorded = tokio::task::spawn_blocking(move || {
        assert!(cache.barrier_blocking(WAIT));
        cache.account(LEGACY).served().flatten()
    })
    .await
    .expect("read");
    assert!(recorded.is_some(), "the account was not checked again");
    // On again is nothing new.
    let (status, body) = switch(&state, true).await;
    assert_eq!(
        (status, body["available"].clone()),
        (StatusCode::OK, json!(true))
    );
}

/// The owner turns the cache off and on again while a write-scope read (a long count walk, a
/// poller's seed) is still in flight, holding its fill. The fill holds the old cache weakly, so the
/// old handle and its lock on the file go when the switch lets go of it, and the fresh file opens.
/// What the old fill saw reaches nothing: its stamp was for the file that was erased.
#[tokio::test]
async fn off_then_on_while_a_fill_is_in_flight_still_opens_a_fresh_file() {
    let scratch = TempDir::new("cache-switch-in-flight");
    let dir = scratch.path();
    let (state, _clock) = filled_server(dir, 5).await;
    let in_flight = state
        .message_cache
        .begin_for(
            vibe_talk::auth::Scope::Write,
            &ChannelId(READ_CHANNEL.to_owned()),
        )
        .expect("a fill begins");
    let (status, body) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = switch(&state, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["available"], true,
        "on after off, with a fill in flight: {body}"
    );
    assert!(body.get("reason").is_none(), "{body}");
    assert!(in_flight.cache().is_none(), "the old handle is gone");
    in_flight.offer_walk(&[message(T0 + 30 * 60_000, "seen by the old fill")]);
    let cache = state.message_cache.cache().expect("installed");
    assert!(cache.is_open());
    assert_eq!(held(&cache), 0, "the old fill reached the fresh file");
}

#[tokio::test]
async fn erase_keeps_the_cache_on_with_a_fresh_empty_file_and_erases_nothing_while_it_is_off() {
    let scratch = TempDir::new("cache-switch-erase");
    let dir = scratch.path();
    let (state, _clock) = filled_server(dir, 7).await;
    let before = state.message_cache.cache().expect("open");

    let (status, body) = erase(&state).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["available"], true, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["messages"], 0, "{body}");
    let after = state.message_cache.cache().expect("still behind the sink");
    assert!(Arc::ptr_eq(&before, &after) && after.is_open());
    assert_eq!(held(&after), 0);
    assert!(
        !marker::load(&sidecar(dir)).expect("load").off,
        "erase turned it off"
    );
    assert!(state.message_cache.identities_verified(WAIT).await);
    drop((before, after));

    let (status, _) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = erase(&state).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["available"], false);
    assert_eq!(body["reason"], "switched_off");
    assert!(
        !default_cache(dir).exists(),
        "an erase while off made a file"
    );
}

#[tokio::test]
async fn no_tool_reaches_the_switch_whatever_the_credential() {
    let scratch = TempDir::new("cache-switch-tools");
    let dir = scratch.path();
    let clock = ManualClock::at(T0);
    let (state, _) = server(dir, Some(ON), &clock).await;
    for tool in [
        vibe_talk::send_approval::Approval::Ask,
        vibe_talk::send_approval::Approval::Direct,
    ]
    .into_iter()
    .flat_map(|approval| vibe_talk::mcp::tool_manifest(&[], approval))
    {
        assert!(
            !tool.path.contains("message-cache") && !tool.name.contains("cache"),
            "{} reaches the message cache's switch",
            tool.name
        );
    }
    for token in [READ_TOKEN, WRITE_TOKEN] {
        let (status, body) = send(
            &state,
            "POST",
            "/mcp",
            token,
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let tools = body["result"]["tools"].as_array().expect("tools");
        assert!(!tools.is_empty());
        for tool in tools {
            let name = tool["name"].as_str().expect("name");
            assert!(!name.contains("cache"), "{name} is offered to a model");
        }
    }
    for invented in ["set_message_cache", "erase_message_cache", "message_cache"] {
        let (_, body) = send(
            &state,
            "POST",
            "/mcp",
            WRITE_TOKEN,
            Some(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": invented, "arguments": { "enabled": false } },
            })),
        )
        .await;
        assert!(
            body["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("unknown tool")),
            "{invented} was not refused: {body}"
        );
    }
    assert!(state.message_cache.cache().is_some_and(|c| c.is_open()));
    assert!(!marker::load(&sidecar(dir)).expect("load").off);
}

async fn purge(state: &AppState) -> Value {
    let (status, body) = send(state, "DELETE", "/api/v1/storage", WRITE_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn the_purge_erases_the_file_and_keeps_the_cache_on() {
    let scratch = TempDir::new("cache-switch-purge");
    let dir = scratch.path();
    let (state, _clock) = filled_server(dir, 6).await;
    // A file an earlier configuration left in the store's directory goes too.
    let earlier = dir
        .join("state")
        .join("store.message-cache-earlier.sqlite3");
    leave_a_cache_at(&earlier);
    let before = state.message_cache.cache().expect("open");

    let body = purge(&state).await;
    assert!(
        body["purged"]
            .as_array()
            .is_some_and(|names| names.contains(&json!("message_cache"))),
        "{body}"
    );
    let after = state.message_cache.cache().expect("still behind the sink");
    assert!(
        Arc::ptr_eq(&before, &after) && after.is_open(),
        "the purge turned it off"
    );
    assert_eq!(held(&after), 0, "the purge left rows in the file");
    assert!(!earlier.exists(), "the purge left an earlier cache file");
    assert!(!marker::load(&sidecar(dir)).expect("load").off);
    assert!(state.message_cache.identities_verified(WAIT).await);
    let (status, body) = erase(&state).await;
    assert_eq!(
        (status, body["available"].clone()),
        (StatusCode::OK, json!(true))
    );
}

#[tokio::test]
async fn the_off_switch_survives_the_purge_and_the_restart_after_it() {
    let scratch = TempDir::new("cache-switch-purge-off");
    let dir = scratch.path();
    let (state, clock) = filled_server(dir, 4).await;
    let (status, body) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    purge(&state).await;
    assert!(
        marker::load(&sidecar(dir)).expect("load").off,
        "the purge turned the cache back on"
    );
    assert!(
        state.message_cache.cache().is_none(),
        "the purge opened a cache"
    );
    assert!(!default_cache(dir).exists());
    stop(state);

    let (state, started) = server(dir, Some(ON), &clock).await;
    assert!(matches!(started, Started::Erased { .. }), "{started:?}");
    assert!(state.message_cache.cache().is_none(), "a restart opened it");
    assert!(!default_cache(dir).exists());
}

#[tokio::test]
async fn the_purge_names_every_table_the_store_empties_and_the_message_cache() {
    // Read from the store's own purge, so a table added there without a name here fails.
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/sqlite.rs"))
            .expect("the store's source");
    let body = source
        .split_once("async fn purge_everything(")
        .expect("the store has a purge")
        .1;
    let body = &body[..body.find("\n    }\n").expect("the purge ends")];
    let mut emptied: Vec<String> = body
        .split("DELETE FROM ")
        .skip(1)
        .map(|rest| {
            let table: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            // As a reader of the answer knows them: a pin's revision counter is part of the pins,
            // a read mark's epoch part of the read marks, and an evicted hue part of the hues
            // (`#458 replication-c9-lattice-fixes`).
            match table.as_str() {
                "channel_aliases" => "aliases".to_owned(),
                "pin_revisions" => "pins".to_owned(),
                "read_mark_epochs" => "read_marks".to_owned(),
                "thread_hue_tombstones" => "thread_hues".to_owned(),
                _ => table,
            }
        })
        .collect();
    emptied.push("message_cache".to_owned());
    emptied.sort();
    emptied.dedup();
    assert!(emptied.len() > 10, "{emptied:?}");
    let mut named: Vec<String> = vibe_talk::http::api::PURGED
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    named.sort();
    assert_eq!(
        named, emptied,
        "the purge's answer and what it empties differ"
    );

    let scratch = TempDir::new("cache-switch-purge-names");
    let clock = ManualClock::at(T0);
    let (state, _) = server(scratch.path(), None, &clock).await;
    assert_eq!(
        purge(&state).await["purged"],
        json!(vibe_talk::http::api::PURGED)
    );
}

async fn client_config(state: &AppState, token: &str) -> Value {
    let (status, body) = send(state, "GET", "/api/v1/client-config", token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn client_config_carries_the_status_only_with_a_table_in_numbers_and_never_a_path() {
    let scratch = TempDir::new("cache-switch-status");
    let dir = scratch.path();
    let clock = ManualClock::at(T0);
    let (state, _) = server(dir, None, &clock).await;
    let config = client_config(&state, WRITE_TOKEN).await;
    assert!(config.get("message_cache").is_none(), "{config}");
    stop(state);

    let (state, _clock) = filled_server(dir, 9).await;
    let status = client_config(&state, WRITE_TOKEN).await["message_cache"].clone();
    assert_eq!(status["available"], true, "{status}");
    assert_eq!(status["enabled"], true);
    assert!(status.get("reason").is_none(), "{status}");
    assert_eq!(status["messages"], 9, "{status}");
    assert_eq!(status["channels"], 1);
    assert!(
        status["bytes"].as_u64().is_some_and(|bytes| bytes > 0),
        "{status}"
    );
    assert_eq!(
        (status["max_days"].clone(), status["max_megabytes"].clone()),
        (json!(14), json!(256))
    );
    // Rows with no proof of a whole listing cover nothing.
    assert!(status.get("oldest_covered").is_none(), "{status}");
    // Numbers, booleans and codes: nothing that names a file, a channel or a message.
    let text = status.to_string();
    for leak in [
        dir.to_string_lossy().as_ref(),
        ".sqlite3",
        "/",
        READ_CHANNEL,
    ] {
        assert!(!text.contains(leak), "{leak} is in the status: {text}");
    }
    let fields: Vec<&String> = status.as_object().expect("an object").keys().collect();
    assert!(
        fields.iter().all(|name| [
            "available",
            "enabled",
            "reason",
            "bytes",
            "messages",
            "channels",
            "oldest_covered",
            "max_days",
            "max_megabytes"
        ]
        .contains(&name.as_str())),
        "{fields:?}"
    );
    // The read token, which cannot switch it, is not told.
    let config = client_config(&state, READ_TOKEN).await;
    assert!(config.get("message_cache").is_none(), "{config}");
    // A switch is seen at once, not after the status is measured again.
    let (status, _) = switch(&state, false).await;
    assert_eq!(status, StatusCode::OK);
    let status = client_config(&state, WRITE_TOKEN).await["message_cache"].clone();
    assert_eq!(
        (
            status["available"].clone(),
            status["reason"].clone(),
            status["messages"].clone()
        ),
        (json!(false), json!("switched_off"), json!(0)),
        "{status}"
    );
    stop(state);

    let (state, _) = server(dir, Some("enabled = false"), &clock).await;
    let status = client_config(&state, WRITE_TOKEN).await["message_cache"].clone();
    assert_eq!(
        (status["enabled"].clone(), status["reason"].clone()),
        (json!(false), json!("config_disabled")),
        "{status}"
    );
}
