//! `#208 persistent-cache`: what decides whether the message cache file exists — the
//! `[message_cache]` table, the owner's switch beside the store, and the erases that follow
//! them — exercised through the server's own startup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vibe_talk::cache::lifecycle::{self, Started, Trouble};
use vibe_talk::cache::{file, marker, CacheOptions, MessageCache};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, LogCapture, ManualClock, TempDir};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// 2026-10-01T00:00:00Z.
const T0: i64 = 1_790_812_800_000;

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
