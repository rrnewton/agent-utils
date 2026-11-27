//! Watched threads: the threads a person wrote to the bridge in, listed by the rescan for two days
//! (#2366 slack-old-thread-reply-late).
//!
//! The rescan finds a reply the event stream did not carry only in a thread it lists, and it
//! lists a thread only when the history listing shows its root with a new reply, or when the
//! stream carried a reply in it in the last 30 minutes. A thread whose root is older than the
//! rescan window has no history row. Once the stream has been quiet in it for half an hour,
//! nothing lists it, so a reply the stream never carries waits until the stream carries some
//! other reply there. On 2026-11-27 a partner member's question in a 38-hour-old thread, posted
//! one minute after the bridge's own reply, reached the agent 5 h 20 min late that way. Bot posts
//! (the bridge's own replies among them) never reach the stream, so they cannot mark a thread
//! active either.
//!
//! So every thread in which an allowed sender's message reached the bridge, by the stream or by
//! the rescan, is watched for `rescan_watch_seconds` (two days by default) after its newest known
//! message. A watched thread whose root the history listing does not show is listed:
//!
//! * on every pass while its newest known message is less than an hour old, then
//! * at most every 10 minutes,
//!
//! within the same host-wide listing budget and per-pass thread cap as every other listing, and
//! from where it was last listed through. A listing raises the thread's newest known message to
//! the newest message it returned, whoever posted it, so the bridge's own reply in a thread puts
//! that thread back on every pass for an hour.
//!
//! The watch lives in `slack-watched-threads.json` beside the plugin's diagnostics log (beside
//! the rescan ledger), not in the subscription: a subscription is dropped and opened again at a
//! stream failure, at a restart, and once a minute while the bridge refuses batches because its
//! request store is full. A watch held only in memory would be forgotten at each of those, and a
//! new subscription would list every watched thread again at once. The file holds identifiers and
//! times only:
//!
//! * `schema`: [`WATCH_SCHEMA`].
//! * `threads`: one entry per watched thread, at most [`MAX_WATCHED`], each with
//!   * `key`: `C…/<root ts>`;
//!   * `activity_micros`: the timestamp of the newest message known in the thread;
//!   * `listed_through_micros`: the newest reply a listing of it has passed (0 before any);
//!   * `listed_at_millis`: when it was last listed (0 before any).
//!
//! Keys this version does not know, in the file or in an entry, are kept when it is rewritten.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

/// The watch file's name, beside the diagnostics log.
pub const WATCH_FILE: &str = "slack-watched-threads.json";
/// The watch file's schema identifier.
pub const WATCH_SCHEMA: &str = "agentctl.slack.watched-threads.v1";
/// Default `rescan_watch_seconds`: two days after a thread's newest known message.
pub const DEFAULT_WATCH: Duration = Duration::from_secs(48 * 60 * 60);
/// The shortest watch `rescan_watch_seconds` accepts, other than 0 (off).
pub const MIN_WATCH_SECONDS: u64 = 60 * 60;
/// The longest watch `rescan_watch_seconds` accepts.
pub const MAX_WATCH_SECONDS: u64 = 7 * 24 * 60 * 60;
/// A watched thread whose newest known message is younger than this is listed on every pass.
pub const BUSY_MILLIS: u64 = 60 * 60 * 1_000;
/// After that, a watched thread is listed at most this often.
pub const QUIET_INTERVAL_MILLIS: u64 = 10 * 60 * 1_000;
/// Threads one watch file holds; past it, the one with the oldest activity is forgotten.
pub const MAX_WATCHED: usize = 1_024;

/// One watched thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watched {
    /// Timestamp of the newest message known in the thread, in Unix microseconds.
    pub activity_micros: u128,
    /// The newest reply a listing of the thread has passed, in Unix microseconds; 0 before any.
    pub listed_through_micros: u128,
    /// When the thread was last listed, in Unix milliseconds; 0 before any.
    pub listed_at_millis: u64,
    unknown: Map<String, Value>,
}

/// The watched threads; see the module documentation.
#[derive(Clone, Debug)]
pub struct Watch {
    path: PathBuf,
    duration: Duration,
    threads: BTreeMap<String, Watched>,
    unknown: Map<String, Value>,
    dirty: bool,
}

fn micros(value: &Value, name: &str) -> Option<u128> {
    value.get(name)?.as_u64().map(u128::from)
}

fn as_json_number(value: u128) -> Value {
    Value::from(u64::try_from(value).unwrap_or(u64::MAX))
}

impl Watch {
    /// The watch file beside `diagnostics_log`, if one is configured.
    #[must_use]
    pub fn path_beside(diagnostics_log: Option<&Path>) -> Option<PathBuf> {
        diagnostics_log
            .and_then(Path::parent)
            .map(|directory| directory.join(WATCH_FILE))
    }

    /// Load the watch at `path`, forgetting threads whose watch has ended. A missing file is an
    /// empty watch; so is an unreadable or invalid one, and the returned note says why.
    #[must_use]
    pub fn load(path: PathBuf, duration: Duration, now_micros: u128) -> (Self, Option<String>) {
        let empty = |path: PathBuf| Self {
            path,
            duration,
            threads: BTreeMap::new(),
            unknown: Map::new(),
            dirty: false,
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (empty(path), None)
            }
            Err(error) => {
                let note = format!(
                    "the watched-thread file could not be read ({error}); starting an empty one"
                );
                return (empty(path), Some(note));
            }
        };
        let parsed = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| match value {
                Value::Object(object) => Some(object),
                _ => None,
            })
            .filter(|object| object.get("schema").and_then(Value::as_str) == Some(WATCH_SCHEMA))
            .and_then(|mut object| {
                let entries = object.remove("threads")?;
                object.remove("schema");
                let mut threads = BTreeMap::new();
                for entry in entries.as_array()? {
                    let mut unknown = entry.as_object()?.clone();
                    let key = unknown.remove("key")?.as_str()?.to_owned();
                    let watched = Watched {
                        activity_micros: micros(entry, "activity_micros")?,
                        listed_through_micros: micros(entry, "listed_through_micros")?,
                        listed_at_millis: entry.get("listed_at_millis")?.as_u64()?,
                        unknown: Map::new(),
                    };
                    for known in [
                        "activity_micros",
                        "listed_through_micros",
                        "listed_at_millis",
                    ] {
                        unknown.remove(known);
                    }
                    threads.insert(key, Watched { unknown, ..watched });
                }
                Some((threads, object))
            });
        match parsed {
            Some((threads, unknown)) => {
                let mut watch = Self {
                    path,
                    duration,
                    threads,
                    unknown,
                    dirty: false,
                };
                watch.prune(now_micros);
                (watch, None)
            }
            None => (
                empty(path),
                Some(
                    "the watched-thread file was not a valid watch; starting an empty one"
                        .to_owned(),
                ),
            ),
        }
    }

    /// The watch file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How long a thread stays watched after its newest known message.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// How many threads are watched.
    #[must_use]
    pub fn len(&self) -> usize {
        self.threads.len()
    }

    /// Whether no thread is watched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    /// The watched thread with this key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Watched> {
        self.threads.get(key)
    }

    /// Whether the watch changed since it was loaded or last saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Watch the thread `key` (`C…/<root ts>`), or raise its newest known message to `at_micros`;
    /// returns whether anything changed.
    pub fn note_activity(&mut self, key: &str, at_micros: u128) -> bool {
        let changed = match self.threads.get_mut(key) {
            Some(watched) if watched.activity_micros >= at_micros => false,
            Some(watched) => {
                watched.activity_micros = at_micros;
                true
            }
            None => {
                self.threads.insert(
                    key.to_owned(),
                    Watched {
                        activity_micros: at_micros,
                        listed_through_micros: 0,
                        listed_at_millis: 0,
                        unknown: Map::new(),
                    },
                );
                while self.threads.len() > MAX_WATCHED {
                    let Some(oldest) = self
                        .threads
                        .iter()
                        .min_by_key(|(_, watched)| watched.activity_micros)
                        .map(|(key, _)| key.clone())
                    else {
                        break;
                    };
                    self.threads.remove(&oldest);
                }
                true
            }
        };
        self.dirty |= changed;
        changed
    }

    /// Record that the thread `key` was listed through `through_micros` at `now_millis`.
    pub fn listed(&mut self, key: &str, through_micros: u128, now_millis: u64) {
        if let Some(watched) = self.threads.get_mut(key) {
            watched.listed_through_micros = watched.listed_through_micros.max(through_micros);
            watched.listed_at_millis = now_millis;
            self.dirty = true;
        }
    }

    /// The watched threads of `channel` a pass at `now_millis` lists, newest activity first, as
    /// (newest known message in microseconds, root ts).
    #[must_use]
    pub fn due(&self, channel: &str, now_millis: u64) -> Vec<(u128, String)> {
        let now_micros = u128::from(now_millis) * 1_000;
        let busy_floor = now_micros.saturating_sub(u128::from(BUSY_MILLIS) * 1_000);
        let watch_floor = now_micros.saturating_sub(self.duration.as_micros());
        let prefix = format!("{channel}/");
        let mut due: Vec<(u128, String)> = self
            .threads
            .iter()
            .filter(|(_, watched)| watched.activity_micros >= watch_floor)
            .filter(|(_, watched)| {
                watched.activity_micros >= busy_floor
                    || now_millis.saturating_sub(watched.listed_at_millis) >= QUIET_INTERVAL_MILLIS
            })
            .filter_map(|(key, watched)| {
                key.strip_prefix(&prefix)
                    .map(|root| (watched.activity_micros, root.to_owned()))
            })
            .collect();
        due.sort_by(|a, b| b.cmp(a));
        due
    }

    /// Forget threads whose watch ended before `now_micros`.
    pub fn prune(&mut self, now_micros: u128) {
        let floor = now_micros.saturating_sub(self.duration.as_micros());
        let before = self.threads.len();
        self.threads
            .retain(|_, watched| watched.activity_micros >= floor);
        self.dirty |= self.threads.len() != before;
    }

    /// Replace the watch file atomically when the watch changed.
    ///
    /// # Errors
    ///
    /// Returns the I/O error; the caller logs it, and the watch stays dirty.
    pub fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut record = self.unknown.clone();
        record.insert("schema".to_owned(), Value::from(WATCH_SCHEMA));
        let threads: Vec<Value> = self
            .threads
            .iter()
            .map(|(key, watched)| {
                let mut entry = watched.unknown.clone();
                entry.insert("key".to_owned(), Value::from(key.as_str()));
                entry.insert(
                    "activity_micros".to_owned(),
                    as_json_number(watched.activity_micros),
                );
                entry.insert(
                    "listed_through_micros".to_owned(),
                    as_json_number(watched.listed_through_micros),
                );
                entry.insert(
                    "listed_at_millis".to_owned(),
                    Value::from(watched.listed_at_millis),
                );
                Value::Object(entry)
            })
            .collect();
        record.insert("threads".to_owned(), Value::Array(threads));
        let bytes =
            serde_json::to_vec_pretty(&Value::Object(record)).map_err(std::io::Error::other)?;
        let mut temporary = self.path.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temporary, &self.path)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MILLIS: u64 = 60 * 60 * 1_000;
    // 2026-11-27T04:50:24Z.
    const NOW: u64 = 1_795_755_024_000;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("slack-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory.join(WATCH_FILE)
    }

    fn at(millis: u64) -> u128 {
        u128::from(millis) * 1_000
    }

    #[test]
    fn a_quiet_thread_is_listed_every_ten_minutes_and_a_busy_one_every_pass() {
        let mut watch = Watch::load(scratch("due"), DEFAULT_WATCH, at(NOW)).0;
        watch.note_activity("C1/100.000001", at(NOW - 3 * HOUR_MILLIS));
        watch.note_activity("C1/200.000002", at(NOW - 60_000));
        watch.note_activity("C2/300.000003", at(NOW - 60_000));
        let roots =
            |due: Vec<(u128, String)>| due.into_iter().map(|(_, root)| root).collect::<Vec<_>>();
        assert_eq!(
            roots(watch.due("C1", NOW)),
            ["200.000002", "100.000001"],
            "never listed: both due, newest activity first, only this channel"
        );
        watch.listed("C1/100.000001", 0, NOW);
        watch.listed("C1/200.000002", 0, NOW);
        assert_eq!(roots(watch.due("C1", NOW + 90_000)), ["200.000002"]);
        assert_eq!(
            roots(watch.due("C1", NOW + QUIET_INTERVAL_MILLIS)),
            ["200.000002", "100.000001"]
        );
        // An hour after its newest message, the busy one is quiet too: once both are listed,
        // neither is due again for ten minutes.
        assert_eq!(roots(watch.due("C1", NOW + HOUR_MILLIS)).len(), 2);
        watch.listed("C1/100.000001", 0, NOW + HOUR_MILLIS);
        watch.listed("C1/200.000002", 0, NOW + HOUR_MILLIS);
        assert_eq!(
            roots(watch.due("C1", NOW + HOUR_MILLIS + 90_000)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_watch_ends_two_days_after_the_newest_message_and_activity_extends_it() {
        let path = scratch("ends");
        let mut watch = Watch::load(path.clone(), DEFAULT_WATCH, at(NOW)).0;
        assert!(watch.note_activity("C1/100.000001", at(NOW)));
        assert!(
            !watch.note_activity("C1/100.000001", at(NOW - 1_000)),
            "older is no news"
        );
        let later = NOW + 47 * HOUR_MILLIS;
        assert!(watch.note_activity("C1/100.000001", at(later)));
        watch.save().unwrap();
        let (reloaded, note) =
            Watch::load(path.clone(), DEFAULT_WATCH, at(later + 47 * HOUR_MILLIS));
        assert!(note.is_none());
        assert_eq!(reloaded.len(), 1);
        let (expired, _) = Watch::load(path.clone(), DEFAULT_WATCH, at(later + 49 * HOUR_MILLIS));
        assert!(expired.is_empty());
        assert!(
            expired.is_dirty(),
            "the expired entry is dropped at the next save"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_file_round_trips_and_keeps_keys_it_does_not_know() {
        let path = scratch("roundtrip");
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema": WATCH_SCHEMA,
                "future": {"kept": true},
                "threads": [{
                    "key": "C1/100.000001",
                    "activity_micros": at(NOW),
                    "listed_through_micros": at(NOW - 5_000),
                    "listed_at_millis": NOW - 1_000,
                    "later_field": 7
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let (mut watch, note) = Watch::load(path.clone(), DEFAULT_WATCH, at(NOW));
        assert!(note.is_none());
        let watched = watch.get("C1/100.000001").unwrap().clone();
        assert_eq!(watched.listed_through_micros, at(NOW - 5_000));
        assert_eq!(watched.listed_at_millis, NOW - 1_000);
        watch.listed("C1/100.000001", at(NOW - 2_000), NOW);
        watch.save().unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future"]["kept"], true);
        assert_eq!(saved["threads"][0]["later_field"], 7);
        assert_eq!(saved["threads"][0]["listed_at_millis"], NOW);
        assert_eq!(
            saved["threads"][0]["listed_through_micros"],
            u64::try_from(at(NOW - 2_000)).unwrap()
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn garbage_an_unknown_schema_or_an_unreadable_file_is_an_empty_watch_with_a_note() {
        let path = scratch("invalid-kinds");
        let cases: [&[u8]; 5] = [
            b"\x00\xff not json at all",
            b"{\"schema\":\"agentctl.slack.watched-threads.v9\",\"threads\":[]}",
            b"{\"schema\":\"agentctl.slack.watched-threads.v1\",\"threads\":{}}",
            b"{\"schema\":\"agentctl.slack.watched-threads.v1\",\"threads\":[{\"key\":\"C1/1.1\"}]}",
            b"[]",
        ];
        for bytes in cases {
            fs::write(&path, bytes).unwrap();
            let (mut watch, note) = Watch::load(path.clone(), DEFAULT_WATCH, at(NOW));
            assert!(watch.is_empty(), "{bytes:?}");
            assert!(
                note.as_deref()
                    .is_some_and(|note| note.contains("not a valid watch; starting an empty one")),
                "{bytes:?}: {note:?}"
            );
            // The next change replaces the invalid file with a valid one.
            assert!(watch.note_activity("C1/100.000001", at(NOW)));
            watch.save().unwrap();
            let (reloaded, note) = Watch::load(path.clone(), DEFAULT_WATCH, at(NOW));
            assert!(note.is_none());
            assert_eq!(reloaded.len(), 1);
        }
        // A path that cannot be read as a file (here a directory) is an empty watch too; saving
        // over it fails with an error the caller logs, and nothing panics.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let (mut watch, note) = Watch::load(path.clone(), DEFAULT_WATCH, at(NOW));
        assert!(watch.is_empty());
        assert!(note.unwrap().contains("could not be read"));
        watch.note_activity("C1/100.000001", at(NOW));
        assert!(watch.save().is_err());
        assert!(
            watch.is_dirty(),
            "a failed save keeps the change for the next one"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_invalid_file_starts_an_empty_watch_with_a_note_and_a_full_one_drops_the_oldest() {
        let path = scratch("invalid");
        fs::write(&path, b"{\"schema\":\"something-else\"}").unwrap();
        let (mut watch, note) = Watch::load(path.clone(), DEFAULT_WATCH, at(NOW));
        assert!(note.unwrap().contains("not a valid watch"));
        for index in 0..=MAX_WATCHED as u64 {
            watch.note_activity(&format!("C1/{index}.000001"), at(NOW - 100_000 + index));
        }
        assert_eq!(watch.len(), MAX_WATCHED);
        assert!(
            watch.get("C1/0.000001").is_none(),
            "the oldest activity was forgotten"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
