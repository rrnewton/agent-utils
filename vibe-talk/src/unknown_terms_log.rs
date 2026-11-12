//! Debug log of Discuss unknown terms (`discuss-unknown-terms-log`).
//!
//! Behind `[summaries] catchup_unknown_terms_log` (default off), each Discuss
//! run appends one JSONL record under the state directory in its own
//! subdirectory, so the recurring terms can be counted offline. The report in
//! this module lists the most frequent terms over N days. Cache-only: this
//! module reads local files and never calls a provider.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Subdirectory under the state directory holding the log.
pub const LOG_SUBDIR: &str = "unknown-terms";
/// Log file name inside [`LOG_SUBDIR`].
pub const LOG_FILE: &str = "unknown-terms.jsonl";
/// Rotated backup kept beside the live file.
pub const LOG_BACKUP: &str = "unknown-terms.jsonl.1";
/// Default total cap per file; rotation keeps live plus one backup.
pub const DEFAULT_MAX_BYTES: u64 = 1_048_576;

/// One Discuss run's unknown terms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnknownTermsRecord {
    /// UTC instant as RFC 3339 (from `jiff::Timestamp::to_string`).
    pub time: String,
    /// Discuss scope: channel id, or `channel/thread:<id>`.
    pub scope: String,
    /// Model that answered.
    pub model: String,
    /// Terms as parsed by `catchup::split_unknown_terms`.
    pub unknown_terms: Vec<String>,
    /// `unknown_terms.len()`, stored so a reader need not recount.
    pub unknown_term_count: usize,
    /// Warm-up fingerprint from `catchup::load_warmup`.
    pub warmup_fingerprint: u64,
}

/// Directory holding the log for a state directory.
#[must_use]
pub fn log_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(LOG_SUBDIR)
}

/// Live log path for a state directory.
#[must_use]
pub fn log_path(state_dir: &Path) -> PathBuf {
    log_dir(state_dir).join(LOG_FILE)
}

/// State directory from the store path: the store file's parent.
#[must_use]
pub fn state_dir_of(storage_path: Option<&Path>) -> Option<PathBuf> {
    storage_path.and_then(|p| p.parent()).map(Path::to_path_buf)
}

/// Serialise one record as a single JSON line (no newline).
#[must_use]
pub fn record_line(record: &UnknownTermsRecord) -> String {
    serde_json::to_string(record).unwrap_or_else(|_| "{}".to_owned())
}

/// Append one record, rotating to the backup when the cap would be exceeded.
///
/// Best-effort by contract: callers log and continue on error; a Discuss
/// answer never fails because the debug log did.
pub fn append_record(
    path: &Path,
    record: &UnknownTermsRecord,
    max_bytes: u64,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = record_line(record);
    line.push('\n');
    let line_len = line.len() as u64;
    if max_bytes > 0 {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > 0 && meta.len() + line_len > max_bytes {
                let backup = path.with_file_name(LOG_BACKUP);
                let _ = std::fs::remove_file(&backup);
                std::fs::rename(path, &backup)?;
            }
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

/// Read records from a log file and its rotated backup (either may be absent).
/// Malformed lines are skipped, never an error.
#[must_use]
pub fn read_records(path: &Path) -> Vec<UnknownTermsRecord> {
    let mut out = Vec::new();
    let backup = path.with_file_name(LOG_BACKUP);
    for candidate in [backup, path.to_path_buf()] {
        let Ok(text) = std::fs::read_to_string(&candidate) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<UnknownTermsRecord>(line) {
                out.push(record);
            }
        }
    }
    out
}

/// Resolve a report target: a log file, the state dir, or a log dir.
#[must_use]
pub fn resolve_log_path(target: &Path) -> PathBuf {
    if target.is_file() {
        return target.to_path_buf();
    }
    let nested = target.join(LOG_SUBDIR).join(LOG_FILE);
    if nested.exists() {
        return nested;
    }
    let direct = target.join(LOG_FILE);
    if direct.exists() {
        return direct;
    }
    nested
}

/// One term's tally for the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TermTally {
    /// The term as reported.
    pub term: String,
    /// Runs reporting it in the window.
    pub count: usize,
    /// Earliest `time` seen (lexicographic = chronological for RFC 3339 UTC).
    pub first_seen: String,
    /// Latest `time` seen.
    pub last_seen: String,
}

/// Aggregate records at or after `cutoff` (RFC 3339 string, inclusive).
///
/// Pure and clock-free: the caller computes the cutoff. Sorted by count
/// descending, then term ascending, so output is deterministic.
#[must_use]
pub fn aggregate(records: &[UnknownTermsRecord], cutoff: Option<&str>) -> Vec<TermTally> {
    let mut map: BTreeMap<String, TermTally> = BTreeMap::new();
    for record in records {
        if let Some(cut) = cutoff {
            if record.time.as_str() < cut {
                continue;
            }
        }
        // Count a repeated term once per run, matching split_unknown_terms dedupe.
        let mut seen = std::collections::HashSet::new();
        for term in &record.unknown_terms {
            if !seen.insert(term.clone()) {
                continue;
            }
            let entry = map.entry(term.clone()).or_insert_with(|| TermTally {
                term: term.clone(),
                count: 0,
                first_seen: record.time.clone(),
                last_seen: record.time.clone(),
            });
            entry.count += 1;
            if record.time < entry.first_seen {
                entry.first_seen = record.time.clone();
            }
            if record.time > entry.last_seen {
                entry.last_seen = record.time.clone();
            }
        }
    }
    let mut tallies: Vec<TermTally> = map.into_values().collect();
    tallies.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.term.cmp(&b.term)));
    tallies
}

/// Cutoff RFC 3339 string `days` before `now` (parsed as `jiff::Timestamp`).
#[must_use]
pub fn cutoff_for_days(now: &str, days: u32) -> Option<String> {
    let instant: jiff::Timestamp = now.parse().ok()?;
    let ms = instant
        .as_millisecond()
        .checked_sub(i64::from(days) * 86_400_000)?;
    jiff::Timestamp::from_millisecond(ms)
        .ok()
        .map(|t| t.to_string())
}

/// Render the plain-text report.
#[must_use]
pub fn format_report(tallies: &[TermTally], days: u32) -> String {
    let mut out = format!(
        "unknown terms over last {days} day(s): {} distinct\n",
        tallies.len()
    );
    out.push_str("count\tfirst_seen\tlast_seen\tterm\n");
    for tally in tallies {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            tally.count, tally.first_seen, tally.last_seen, tally.term
        ));
    }
    out
}

/// Parsed `unknown-terms-report` options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportOptions {
    /// Window in days.
    pub days: u32,
    /// Log file, state dir, or log dir.
    pub target: PathBuf,
    /// Emit JSON instead of text.
    pub json: bool,
}

/// Parse `vibe-talk unknown-terms-report` arguments. `Ok(None)` = help shown.
pub fn parse_report_args(
    args: impl Iterator<Item = String>,
) -> anyhow::Result<Option<ReportOptions>> {
    let mut days: u32 = 7;
    let mut json = false;
    let mut target: Option<PathBuf> = None;
    let mut it = args.peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!(
                    "usage: vibe-talk unknown-terms-report [--days N] [--json] <path>\n\
                     \n\
                     List the most frequent Discuss unknown terms over the last N days\n\
                     (default 7). <path> is the unknown-terms.jsonl file, its directory,\n\
                     or the vibe-talk state directory. The rotated .1 backup is read too.\n"
                );
                return Ok(None);
            }
            "--days" => {
                let Some(value) = it.next() else {
                    anyhow::bail!("--days needs a value");
                };
                days = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--days must be a number"))?;
            }
            "--json" => json = true,
            _ if arg.starts_with("--days=") => {
                days = arg["--days=".len()..]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--days must be a number"))?;
            }
            _ if arg.starts_with('-') => anyhow::bail!("unknown option {arg}"),
            _ => {
                if target.is_some() {
                    anyhow::bail!("unknown-terms-report takes one path");
                }
                target = Some(PathBuf::from(arg));
            }
        }
    }
    let Some(target) = target else {
        anyhow::bail!("usage: vibe-talk unknown-terms-report [--days N] [--json] <path>");
    };
    Ok(Some(ReportOptions { days, target, json }))
}

/// Run the report against an explicit `now` (kept a parameter for tests).
#[must_use]
pub fn run_report(options: &ReportOptions, now: &str) -> String {
    let path = resolve_log_path(&options.target);
    let records = read_records(&path);
    let cutoff = cutoff_for_days(now, options.days);
    let tallies = aggregate(&records, cutoff.as_deref());
    if options.json {
        serde_json::to_string_pretty(&tallies).unwrap_or_else(|_| "[]".to_owned())
    } else {
        format_report(&tallies, options.days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn record(time: &str, terms: &[&str]) -> UnknownTermsRecord {
        UnknownTermsRecord {
            time: time.to_owned(),
            scope: "C1".to_owned(),
            model: "test-model".to_owned(),
            unknown_terms: terms.iter().map(|t| (*t).to_owned()).collect(),
            unknown_term_count: terms.len(),
            warmup_fingerprint: 42,
        }
    }

    #[test]
    fn append_writes_jsonl_in_subdir_and_reads_back() {
        let dir = TempDir::new("utlog-append");
        let path = log_path(dir.path());
        assert!(
            path.ends_with("unknown-terms/unknown-terms.jsonl"),
            "{path:?}"
        );
        append_record(
            &path,
            &record("2026-11-10T00:00:00Z", &["wave Q"]),
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        append_record(
            &path,
            &record("2026-11-11T00:00:00Z", &["step 3A"]),
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        // Every field survives the round trip.
        let first: UnknownTermsRecord = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first.scope, "C1");
        assert_eq!(first.model, "test-model");
        assert_eq!(first.unknown_term_count, 1);
        assert_eq!(first.warmup_fingerprint, 42);
        assert_eq!(read_records(&path).len(), 2);
    }

    #[test]
    fn rotation_keeps_one_backup_and_bounds_size() {
        let dir = TempDir::new("utlog-rotate");
        let path = log_path(dir.path());
        // Tiny cap: each later append rotates.
        for i in 0..6 {
            let time = format!("2026-11-1{i}T00:00:00Z");
            append_record(&path, &record(&time, &["term-with-some-length"]), 120).unwrap();
        }
        let backup = path.with_file_name(LOG_BACKUP);
        assert!(backup.exists(), "backup must exist after rotation");
        let live = std::fs::metadata(&path).unwrap().len();
        assert!(live <= 200, "live file stays near the cap, got {live}");
        // Report still sees terms from live plus backup.
        assert!(!read_records(&path).is_empty());
    }

    #[test]
    fn aggregate_counts_first_last_and_window() {
        let records = vec![
            record("2026-11-01T00:00:00Z", &["old", "wave Q"]),
            record("2026-11-10T00:00:00Z", &["wave Q", "step 3A"]),
            record("2026-11-12T00:00:00Z", &["wave Q"]),
        ];
        let all = aggregate(&records, None);
        assert_eq!(all[0].term, "wave Q");
        assert_eq!(all[0].count, 3);
        assert_eq!(all[0].first_seen, "2026-11-01T00:00:00Z");
        assert_eq!(all[0].last_seen, "2026-11-12T00:00:00Z");
        // Window excludes the Nov 1 run: `old` disappears, counts drop.
        let recent = aggregate(&records, Some("2026-11-05T00:00:00Z"));
        assert!(recent.iter().all(|t| t.term != "old"), "{recent:?}");
        assert_eq!(recent.iter().find(|t| t.term == "wave Q").unwrap().count, 2);
        let text = format_report(&recent, 7);
        assert!(text.contains("wave Q"), "{text}");
    }

    #[test]
    fn cutoff_arithmetic_is_exact() {
        assert_eq!(
            cutoff_for_days("2026-11-12T00:00:00Z", 7).as_deref(),
            Some("2026-11-05T00:00:00Z")
        );
        assert_eq!(
            cutoff_for_days("2026-11-12T12:00:00Z", 0).as_deref(),
            Some("2026-11-12T12:00:00Z")
        );
    }

    #[test]
    fn report_runs_end_to_end_without_a_clock() {
        let dir = TempDir::new("utlog-report");
        let path = log_path(dir.path());
        append_record(
            &path,
            &record("2026-11-11T00:00:00Z", &["wave Q"]),
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        append_record(
            &path,
            &record("2026-11-12T00:00:00Z", &["wave Q", "step 3A"]),
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        let options = ReportOptions {
            days: 7,
            target: dir.path().to_path_buf(),
            json: false,
        };
        let out = run_report(&options, "2026-11-12T00:00:00Z");
        assert!(out.contains("wave Q"), "{out}");
        assert!(out.contains("step 3A"), "{out}");
    }
}
