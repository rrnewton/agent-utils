//! Private log of WTF lookups (`#2232 vibe-talk-wtf-lookup-log-and-replay`).
//!
//! Every `POST …/glossary/lookup` appends one JSON line describing what the
//! lookup did: the selection and where it was pressed, the head and qualifier
//! it searched for, each search with its hit count, the candidates handed to the
//! model (ids, never message text), the answer or the kind of miss, and timings.
//! `vibe-talk wtf-replay` ([`crate::wtf_replay`]) reads it back to re-run misses
//! against a local server, so an engine change is measured by its hit rate.
//!
//! The log is deployment state, never repository content: it lives in its own
//! subdirectory of the state directory ([`LOG_SUBDIR`]), one file per UTC day,
//! the directory `0700` and every file `0600`. It is bounded twice: a day file
//! stops growing at [`DAY_CAP_BYTES`] (later records that day are dropped and
//! counted, with one warning), and day files older than [`KEEP_DAYS`] are
//! deleted when a new day's file is first written. A write is one `stat` and one
//! appended line under a process lock held only for that append; pruning lists
//! one small directory outside the lock. A failed append warns and never fails
//! the lookup. Text fields pass through the deployment's secret redaction before
//! they get here; no request header, token or cookie is ever recorded.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Subdirectory under the state directory holding the day files.
pub const LOG_SUBDIR: &str = "wtf-lookups";
/// Day file name prefix: `lookups-YYYY-MM-DD.jsonl`.
pub const FILE_PREFIX: &str = "lookups-";
/// Day file name suffix.
pub const FILE_SUFFIX: &str = ".jsonl";
/// The most bytes one day file holds.
pub const DAY_CAP_BYTES: u64 = 2 * 1024 * 1024;
/// Day files kept, counting today.
pub const KEEP_DAYS: i64 = 30;
/// Engine tag written into every record, so runs of two engines compare.
pub const ENGINE: &str = "wtf-engine-2";
/// Record shape version.
pub const RECORD_VERSION: u32 = 1;
/// The longest selection, context, term or definition kept, in characters.
pub const MAX_TEXT_CHARS: usize = 500;
/// The most searches one record lists.
pub const MAX_QUERIES: usize = 24;
/// The most candidates one record lists.
pub const MAX_CANDIDATES: usize = 12;

/// One search the lookup ran.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedQuery {
    /// Human form of the search (the spaced window, or the link key).
    pub q: String,
    /// `head`, `qualifier`, `full`, `window`, `word` or `link`.
    pub kind: String,
    /// Hits the search returned before filtering.
    pub hits: usize,
    /// New candidates it added after filtering and dedupe.
    #[serde(default)]
    pub kept: usize,
    /// Milliseconds the search took.
    #[serde(default)]
    pub ms: u64,
}

/// One candidate handed to the model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedCandidate {
    /// Message id.
    pub id: String,
    /// Channel id.
    pub channel: String,
    /// `thread`, `channel_recent`, `link` or `global`.
    pub bucket: String,
    /// Definitional-fit score.
    pub score: i32,
    /// Whether it names the head on word boundaries.
    #[serde(default)]
    pub head_match: bool,
}

/// Per-bucket counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketCounts {
    /// Whether the selection's own thread was read.
    #[serde(default)]
    pub thread_searched: bool,
    /// Candidates from the thread.
    #[serde(default)]
    pub thread: usize,
    /// Messages of the channel's recent history scanned.
    #[serde(default)]
    pub channel_recent_scanned: usize,
    /// Candidates from the channel's recent history.
    #[serde(default)]
    pub channel_recent: usize,
    /// Link keys followed.
    #[serde(default)]
    pub link_keys: usize,
    /// Candidates from link keys.
    #[serde(default)]
    pub link: usize,
    /// Candidates from global windows.
    #[serde(default)]
    pub global: usize,
    /// Already-fetched documents found.
    #[serde(default)]
    pub docs: usize,
}

/// Phase timings in milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timings {
    /// Message retrieval (thread, channel history, links, global windows).
    #[serde(default)]
    pub retrieval: u64,
    /// Document search.
    #[serde(default)]
    pub docs: u64,
    /// The model step.
    #[serde(default)]
    pub model: u64,
    /// The whole lookup.
    #[serde(default)]
    pub total: u64,
}

/// The source an answer rests on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedSource {
    /// `message` or `document`.
    pub kind: String,
    /// Message id or document key.
    pub id: String,
    /// The message's channel (empty for a document).
    #[serde(default)]
    pub channel: String,
}

/// One lookup, as logged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookupRecord {
    /// Record shape version ([`RECORD_VERSION`]).
    pub v: u32,
    /// UTC instant, RFC 3339.
    pub ts: String,
    /// Engine tag ([`ENGINE`]).
    pub engine: String,
    /// Channel the lookup was pressed in.
    pub channel: String,
    /// The message the selection stands in, when the page sent it.
    #[serde(default)]
    pub message_id: Option<String>,
    /// The selection (redacted, capped).
    pub selected: String,
    /// The containing line (redacted, capped).
    #[serde(default)]
    pub context: String,
    /// Head noun phrase searched for.
    #[serde(default)]
    pub head: String,
    /// Qualifier, when the selection had one.
    #[serde(default)]
    pub qualifier: Option<String>,
    /// `retrieval` or `full` for an operator probe; absent for a reader's press.
    #[serde(default)]
    pub probe: Option<String>,
    /// `reviewed_hit`, `learned_hit`, `found`, `evidence`, `miss_no_evidence`,
    /// `miss_model_declined`, `miss_unlinked_source` or `error`.
    pub outcome: String,
    /// The term answered.
    #[serde(default)]
    pub term: Option<String>,
    /// The definition answered (redacted, capped).
    #[serde(default)]
    pub definition: Option<String>,
    /// The source the answer rests on.
    #[serde(default)]
    pub source: Option<LoggedSource>,
    /// Per-bucket counts.
    #[serde(default)]
    pub buckets: BucketCounts,
    /// The searches run, in order.
    #[serde(default)]
    pub queries: Vec<LoggedQuery>,
    /// The candidates handed to the model, in evidence order.
    #[serde(default)]
    pub candidates: Vec<LoggedCandidate>,
    /// Document keys found.
    #[serde(default)]
    pub docs: Vec<String>,
    /// Why searching stopped early (`strong`, `query_budget`, `time_budget`).
    #[serde(default)]
    pub stopped: Option<String>,
    /// Model that answered.
    #[serde(default)]
    pub model: Option<String>,
    /// Error code for an `error` outcome (a short code, never upstream text).
    #[serde(default)]
    pub error: Option<String>,
    /// Phase timings.
    #[serde(default)]
    pub ms: Timings,
}

impl LookupRecord {
    /// Whether the record is a hit: a definition was answered, or (for a
    /// retrieval probe) strong evidence was held.
    #[must_use]
    pub fn is_hit(&self) -> bool {
        matches!(
            self.outcome.as_str(),
            "reviewed_hit" | "learned_hit" | "found" | "evidence"
        )
    }

    /// Bound every list and text field (applied before writing).
    pub fn bound(&mut self) {
        let cap = |s: &mut String| {
            if s.chars().count() > MAX_TEXT_CHARS {
                *s = s.chars().take(MAX_TEXT_CHARS).collect();
            }
        };
        cap(&mut self.selected);
        cap(&mut self.context);
        cap(&mut self.channel);
        if let Some(term) = self.term.as_mut() {
            cap(term);
        }
        if let Some(definition) = self.definition.as_mut() {
            cap(definition);
        }
        self.queries.truncate(MAX_QUERIES);
        for query in &mut self.queries {
            cap(&mut query.q);
        }
        self.candidates.truncate(MAX_CANDIDATES);
        self.docs.truncate(MAX_CANDIDATES);
    }
}

/// What an operator probe returns: the reader's answer plus what the lookup
/// did. Only `?probe=` requests get it, on a server started with
/// `[summaries] wtf_probe = true`; the page never sees it.
#[derive(Debug, Serialize)]
pub struct ProbeAnswer {
    /// The answer a reader would get (for a retrieval probe, `state` is
    /// `evidence` or `not_found` and no model was called).
    #[serde(flatten)]
    pub answer: crate::contract::GlossaryLookupResponse,
    /// The lookup's record, exactly as logged.
    pub diagnostics: LookupRecord,
}

/// Directory holding the day files for a state directory.
#[must_use]
pub fn log_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(LOG_SUBDIR)
}

/// State directory from the store path: the store file's parent.
#[must_use]
pub fn state_dir_of(storage_path: Option<&Path>) -> Option<PathBuf> {
    storage_path.and_then(|p| p.parent()).map(Path::to_path_buf)
}

/// Day file name for a UTC date `YYYY-MM-DD`.
#[must_use]
pub fn day_file_name(day: &str) -> String {
    format!("{FILE_PREFIX}{day}{FILE_SUFFIX}")
}

/// The UTC date of a timestamp, `YYYY-MM-DD`.
#[must_use]
pub fn utc_day(at: jiff::Timestamp) -> String {
    at.to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%Y-%m-%d")
        .to_string()
}

/// The date a day file name stands for, when it is one.
#[must_use]
pub fn day_of_file_name(name: &str) -> Option<jiff::civil::Date> {
    let day = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    day.parse::<jiff::civil::Date>().ok()
}

/// What one append did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appended {
    /// The line was written.
    Written,
    /// The day file is full; the record was dropped. `first` is true for the
    /// first drop of the day (the one that warns).
    Dropped {
        /// First drop of this day.
        first: bool,
    },
}

/// Serialises appends inside one process. Held only for a stat and one write.
static WRITE_LOCK: Mutex<Option<String>> = Mutex::new(None);

/// Append one record to the day file for `at`, creating the directory
/// (`0700`) and the file (`0600`) as needed, refusing to grow a day file past
/// `day_cap`. When this append created a new day's file, day files older than
/// `keep_days` are pruned afterwards, outside the lock.
///
/// # Errors
///
/// [`std::io::Error`] when the directory or file cannot be written.
pub fn append(
    dir: &Path,
    record: &LookupRecord,
    at: jiff::Timestamp,
    day_cap: u64,
    keep_days: i64,
) -> std::io::Result<Appended> {
    let mut bounded = record.clone();
    bounded.bound();
    let mut line = serde_json::to_string(&bounded).map_err(std::io::Error::other)?;
    line.push('\n');
    let day = utc_day(at);
    let path = dir.join(day_file_name(&day));
    let created_day_file = {
        let mut dropped_day = WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !dir.is_dir() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        let existing = std::fs::metadata(&path).ok().map(|m| m.len());
        if existing.is_some_and(|len| len + line.len() as u64 > day_cap) {
            let first = dropped_day.as_deref() != Some(day.as_str());
            *dropped_day = Some(day);
            return Ok(Appended::Dropped { first });
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(line.as_bytes())?;
        existing.is_none()
    };
    if created_day_file {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        prune(dir, at, keep_days);
    }
    Ok(Appended::Written)
}

/// Delete day files older than `keep_days` (today counts as one). Lists one
/// directory that holds at most about `keep_days` files; never errors.
pub fn prune(dir: &Path, at: jiff::Timestamp, keep_days: i64) {
    let today = at.to_zoned(jiff::tz::TimeZone::UTC).date();
    let Ok(oldest_kept) = today.checked_sub(jiff::Span::new().days(keep_days.max(1) - 1)) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(day) = name.to_str().and_then(day_of_file_name) else {
            continue;
        };
        if day < oldest_kept {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Day files under `dir`, oldest first.
#[must_use]
pub fn day_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(jiff::civil::Date, PathBuf)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let day = entry.file_name().to_str().and_then(day_of_file_name)?;
                    Some((day, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files.into_iter().map(|(_, path)| path).collect()
}

/// Read records from a day file, the log directory, or the state directory
/// holding it, oldest first. Malformed lines are skipped, never an error.
#[must_use]
pub fn read_records(path: &Path) -> Vec<LookupRecord> {
    let files = if path.is_file() {
        vec![path.to_path_buf()]
    } else if path.join(LOG_SUBDIR).is_dir() {
        day_files(&path.join(LOG_SUBDIR))
    } else {
        day_files(path)
    };
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        out.extend(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<LookupRecord>(line).ok()),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().expect("timestamp")
    }

    fn record(selected: &str, outcome: &str) -> LookupRecord {
        LookupRecord {
            v: RECORD_VERSION,
            ts: "2026-09-20T10:00:00Z".to_owned(),
            engine: ENGINE.to_owned(),
            channel: "c1".to_owned(),
            selected: selected.to_owned(),
            outcome: outcome.to_owned(),
            ..LookupRecord::default()
        }
    }

    #[test]
    fn appends_are_private_one_file_per_day_and_read_back_in_order() {
        let tmp = TempDir::new("append");
        let dir = log_dir(tmp.path());
        let day1 = at("2026-09-20T23:59:00Z");
        let day2 = at("2026-09-21T00:01:00Z");
        assert_eq!(
            append(
                &dir,
                &record("alpha", "found"),
                day1,
                DAY_CAP_BYTES,
                KEEP_DAYS
            )
            .unwrap(),
            Appended::Written
        );
        append(
            &dir,
            &record("beta", "miss_no_evidence"),
            day2,
            DAY_CAP_BYTES,
            KEEP_DAYS,
        )
        .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        let files = day_files(&dir);
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(files[0].ends_with("lookups-2026-09-20.jsonl"));
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }
        let back = read_records(tmp.path());
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].selected, "alpha");
        assert!(back[0].is_hit());
        assert!(!back[1].is_hit());
        assert_eq!(read_records(&dir).len(), 2);
        assert_eq!(read_records(&files[1]).len(), 1);
    }

    #[test]
    fn a_full_day_drops_records_and_says_so_once() {
        let tmp = TempDir::new("cap");
        let dir = log_dir(tmp.path());
        let now = at("2026-09-20T10:00:00Z");
        let line_len = serde_json::to_string(&record("alpha", "found"))
            .unwrap()
            .len() as u64
            + 1;
        let cap = line_len * 2;
        for _ in 0..2 {
            assert_eq!(
                append(&dir, &record("alpha", "found"), now, cap, KEEP_DAYS).unwrap(),
                Appended::Written
            );
        }
        let third = append(&dir, &record("alpha", "found"), now, cap, KEEP_DAYS).unwrap();
        let fourth = append(&dir, &record("alpha", "found"), now, cap, KEEP_DAYS).unwrap();
        assert!(matches!(third, Appended::Dropped { .. }), "{third:?}");
        assert_eq!(fourth, Appended::Dropped { first: false });
        assert_eq!(read_records(&dir).len(), 2);
        let size = std::fs::metadata(dir.join(day_file_name("2026-09-20")))
            .unwrap()
            .len();
        assert!(size <= cap);
    }

    #[test]
    fn a_new_day_prunes_files_past_the_age_cap() {
        let tmp = TempDir::new("prune");
        let dir = log_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        for day in ["2026-08-01", "2026-09-10", "2026-09-11"] {
            std::fs::write(dir.join(day_file_name(day)), "{}\n").unwrap();
        }
        std::fs::write(dir.join("unrelated.txt"), "keep").unwrap();
        append(
            &dir,
            &record("alpha", "found"),
            at("2026-09-20T10:00:00Z"),
            DAY_CAP_BYTES,
            10,
        )
        .unwrap();
        let names: Vec<String> = day_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["lookups-2026-09-11.jsonl", "lookups-2026-09-20.jsonl"]
        );
        assert!(dir.join("unrelated.txt").exists());
    }

    #[test]
    fn records_are_bounded_before_writing() {
        let mut long = record(&"x".repeat(2_000), "found");
        long.queries = (0..40)
            .map(|i| LoggedQuery {
                q: format!("q{i}"),
                ..LoggedQuery::default()
            })
            .collect();
        long.bound();
        assert_eq!(long.selected.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(long.queries.len(), MAX_QUERIES);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let tmp = TempDir::new("malformed");
        let dir = log_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&record("alpha", "found")).unwrap();
        std::fs::write(
            dir.join(day_file_name("2026-09-20")),
            format!("not json\n{good}\n\n"),
        )
        .unwrap();
        assert_eq!(read_records(&dir).len(), 1);
    }
}
