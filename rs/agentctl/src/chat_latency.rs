//! Per-profile chat latency summaries (#801 chat-latency-summaries).
//!
//! Each request is folded once (marked on its own record) into a per-day
//! summary file under `<bridge-state>/latency/`, keyed by the bridge,
//! coordinator and the model/effort/adapter profile tagged at delivery.
//! Summaries are bounded: six stages over fixed log buckets, well under
//! a kilobyte a day. Folding is exact-once live and in backfill because
//! the request record itself carries `folded_at`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Stage names in order.
pub const STAGES: [&str; 6] = [
    "posted_to_admitted",
    "admitted_to_ack",
    "ack_to_delivered",
    "delivered_to_first_reply",
    "posted_to_first_reply",
    "admitted_to_delivered",
];

/// Bucket upper bounds in milliseconds (log-spaced); the last bucket is
/// open-ended. 15 buckets.
pub const EDGES_MS: [u64; 14] = [
    100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 60_000, 150_000, 300_000, 600_000,
    1_500_000, 3_600_000,
];

/// The bucket a duration falls in.
#[must_use]
pub fn bucket_of(millis: u64) -> usize {
    EDGES_MS
        .iter()
        .position(|edge| millis < *edge)
        .unwrap_or(EDGES_MS.len())
}

/// A human bucket range such as `10-25 s`.
#[must_use]
pub fn bucket_label(bucket: usize) -> String {
    let lo = if bucket == 0 { 0 } else { EDGES_MS[bucket - 1] };
    if bucket >= EDGES_MS.len() {
        return format!("{}+", human(lo));
    }
    let hi = EDGES_MS[bucket];
    if lo >= 1_000 && hi <= 60_000 {
        return format!("{}-{} s", lo / 1_000, hi / 1_000);
    }
    format!("{}-{}", human(lo), human(hi))
}

fn human(millis: u64) -> String {
    if millis < 1_000 {
        format!("{millis} ms")
    } else if millis < 60_000 {
        format!("{} s", millis / 1_000)
    } else {
        format!("{} min", millis / 60_000)
    }
}

/// A safe file-name slug for one profile value; the raw value stays in
/// the file, so the slug only needs to be stable and safe.
#[must_use]
pub fn slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.len() > 48 {
        slug[..48].trim_end_matches('-').to_owned()
    } else {
        slug
    };
    if slug.is_empty() {
        "unknown".to_owned()
    } else {
        slug
    }
}

/// One day's summary for one profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    /// Schema marker.
    pub schema: String,
    /// Bridge agent name.
    pub bridge: String,
    /// Coordinator agent name.
    pub coordinator: String,
    /// Raw model value (the file name carries its slug).
    pub model: String,
    /// Raw effort value.
    pub effort: String,
    /// Adapter: `tui` or `headless` (registry record's harness mode).
    pub adapter: String,
    /// ET day, `YYYY-MM-DD`.
    pub day: String,
    /// Per-stage bucket counts.
    pub stages: BTreeMap<String, Vec<u32>>,
    /// Requests folded into this file.
    pub folded: u64,
}

/// The directory a summary lives in: `<state>/latency/<key>/`.
#[must_use]
pub fn summary_dir(
    state: &Path,
    bridge: &str,
    coordinator: &str,
    model: &str,
    effort: &str,
    adapter: &str,
) -> PathBuf {
    // "+" cannot appear in a slug, so key parts cannot collide.
    state.join("latency").join(format!(
        "{}+{}+{}+{}+{}",
        slug(bridge),
        slug(coordinator),
        slug(model),
        slug(effort),
        slug(adapter)
    ))
}

/// Load one summary file, or a fresh empty one.
#[must_use]
pub fn load(
    path: &Path,
    bridge: &str,
    coordinator: &str,
    model: &str,
    effort: &str,
    adapter: &str,
    day: &str,
) -> Summary {
    if let Ok(raw) = std::fs::read(path) {
        if let Ok(summary) = serde_json::from_slice(&raw) {
            return summary;
        }
    }
    Summary {
        schema: "agentctl.chat-latency/1".to_owned(),
        bridge: bridge.to_owned(),
        coordinator: coordinator.to_owned(),
        model: model.to_owned(),
        effort: effort.to_owned(),
        adapter: adapter.to_owned(),
        day: day.to_owned(),
        stages: STAGES
            .iter()
            .map(|stage| ((*stage).to_owned(), vec![0; EDGES_MS.len() + 1]))
            .collect(),
        folded: 0,
    }
}

#[allow(clippy::too_many_arguments)]
/// Fold one request's stage durations (milliseconds, missing = skipped)
/// into its day file. Atomic tmp-plus-rename write.
///
/// # Errors
/// Returns an I/O or serialization error string.
pub fn fold(
    state: &Path,
    bridge: &str,
    coordinator: &str,
    model: &str,
    effort: &str,
    adapter: &str,
    day: &str,
    durations: &[Option<u64>; 6],
) -> Result<(), String> {
    let dir = summary_dir(state, bridge, coordinator, model, effort, adapter);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{day}.json"));
    let mut summary = load(&path, bridge, coordinator, model, effort, adapter, day);
    for (stage, duration) in STAGES.iter().zip(durations.iter()) {
        if let Some(millis) = duration {
            summary.stages.get_mut(*stage).expect("stage")[bucket_of(*millis)] += 1;
        }
    }
    summary.folded += 1;
    // Unique temp name (per process and call) plus fsync before rename.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec(&summary).map_err(|e| e.to_string())?;
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Every summary in the window, for the CLI.
#[must_use]
pub fn read_window(state: &Path, since: &str, until: &str) -> Vec<Summary> {
    let mut out = Vec::new();
    let root = state.join("latency");
    let Ok(dirs) = std::fs::read_dir(&root) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let Some(day) = name.strip_suffix(".json") else {
                continue;
            };
            if day < since || day > until {
                continue;
            }
            if let Ok(raw) = std::fs::read(file.path()) {
                if let Ok(summary) = serde_json::from_slice::<Summary>(&raw) {
                    out.push(summary);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.model, &a.effort, &a.adapter, &a.day).cmp(&(&b.model, &b.effort, &b.adapter, &b.day))
    });
    out
}

/// Merge summaries of one profile across days: stage counts summed.
#[must_use]
pub fn merge(summaries: &[&Summary]) -> BTreeMap<String, Vec<u32>> {
    let mut stages: BTreeMap<String, Vec<u32>> = STAGES
        .iter()
        .map(|stage| ((*stage).to_owned(), vec![0; EDGES_MS.len() + 1]))
        .collect();
    for summary in summaries {
        for (stage, counts) in &summary.stages {
            let target = stages
                .entry(stage.clone())
                .or_insert_with(|| vec![0; EDGES_MS.len() + 1]);
            for (i, count) in counts.iter().enumerate() {
                if i < target.len() {
                    target[i] += count;
                }
            }
        }
    }
    stages
}

/// The bucket holding the `fraction` quantile of a stage, if any.
#[must_use]
pub fn quantile_bucket(counts: &[u32], fraction: f64) -> Option<usize> {
    let total: u64 = counts.iter().map(|c| u64::from(*c)).sum();
    if total == 0 {
        return None;
    }
    let target = (fraction * total as f64).ceil() as u64;
    let mut seen = 0u64;
    for (bucket, count) in counts.iter().enumerate() {
        seen += u64::from(*count);
        if seen >= target {
            return Some(bucket);
        }
    }
    Some(counts.len() - 1)
}

/// The ET calendar date (`YYYY-MM-DD`) of a Unix time (US DST rules).
#[must_use]
pub fn et_date(unix_secs: f64) -> String {
    let est_days = ((unix_secs - 5.0 * 3600.0).div_euclid(86_400.0)) as i64;
    let (year, _, _) = civil(est_days);
    let start = nth_sunday(year, 3, 2);
    let end = nth_sunday(year, 11, 1);
    let offset = if (start..end).contains(&est_days) {
        -4.0
    } else {
        -5.0
    } * 3600.0;
    let (y, m, d) = civil(((unix_secs + offset).div_euclid(86_400.0)) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn nth_sunday(year: i64, month: i64, n: i64) -> i64 {
    let first = days(year, month, 1);
    first + (7 - (first + 4).rem_euclid(7)) % 7 + (n - 1) * 7
}
fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let mp = (m + 9) % 12;
    era * 146_097 + (y - era * 400) * 365 + (y - era * 400) / 4 - (y - era * 400) / 100
        + (153 * mp + 2) / 5
        + d
        - 1
        - 719_468
}
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        m,
        doy - (153 * mp + 2) / 5 + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_slugs_and_quantiles() {
        assert_eq!(bucket_of(99), 0);
        assert_eq!(bucket_of(100), 1);
        assert_eq!(bucket_of(3_600_000), 14);
        assert_eq!(bucket_label(7), "10-25 s");
        assert_eq!(slug("Claude Opus 4.5/high"), "claude-opus-4-5-high");
        assert!(summary_dir(Path::new("/s"), "a-b", "c", "m", "e", "tui")
            .to_string_lossy()
            .contains("a-b+c"));
        assert_eq!(slug(""), "unknown");
        let mut counts = vec![0; 15];
        counts[7] = 10;
        assert_eq!(quantile_bucket(&counts, 0.5), Some(7));
        assert_eq!(quantile_bucket(&counts, 0.9), Some(7));
    }

    #[test]
    fn fold_is_bounded_and_readable() {
        let dir = std::env::temp_dir().join(format!("latency-test-{}", std::process::id()));
        let state = dir.as_path();
        for _ in 0..3 {
            fold(
                state,
                "b",
                "c",
                "m",
                "high",
                "tui",
                "2026-11-03",
                &[Some(12_000), None, None, None, None, None],
            )
            .unwrap();
        }
        let all = read_window(state, "2026-11-01", "2026-11-30");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].folded, 3);
        assert_eq!(all[0].stages["posted_to_admitted"][7], 3);
        let raw =
            std::fs::read(summary_dir(state, "b", "c", "m", "high", "tui").join("2026-11-03.json"))
                .unwrap();
        assert!(raw.len() < 1_024, "{} bytes", raw.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn window_merge_and_json_shape() {
        // The CLI's window merge: one profile's days merged before
        // quantiles, and the --json document built from the same values.
        let dir = std::env::temp_dir().join(format!("latency-merge-{}", std::process::id()));
        let state = dir.as_path();
        for day in ["2026-11-03", "2026-11-04"] {
            fold(
                state,
                "b",
                "c",
                "m",
                "high",
                "tui",
                day,
                &[Some(12_000), None, None, None, None, None],
            )
            .unwrap();
        }
        fold(
            state,
            "b",
            "c",
            "m",
            "high",
            "tui",
            "2026-11-04",
            &[Some(120), None, None, None, None, None],
        )
        .unwrap();
        let all = read_window(state, "2026-11-01", "2026-11-30");
        assert_eq!(all.len(), 2, "both days in the window");
        let merged = merge(&all.iter().collect::<Vec<_>>());
        let counts = &merged["posted_to_admitted"];
        assert_eq!(counts.iter().sum::<u32>(), 3, "days summed");
        assert_eq!(counts[bucket_of(12_000)], 2);
        assert_eq!(counts[bucket_of(120)], 1);
        // Median of [120, 12000, 12000] ms is the 12 s bucket.
        assert_eq!(quantile_bucket(counts, 0.5), Some(bucket_of(12_000)));
        assert_eq!(
            bucket_label(quantile_bucket(counts, 0.5).unwrap()),
            "10-25 s"
        );
        // A narrower window excludes the first day.
        let one = read_window(state, "2026-11-04", "2026-11-04");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].folded, 2);
        // The --json shape: the summary serializes with its raw profile.
        let json = serde_json::to_value(&all[0]).unwrap();
        assert_eq!(json["model"], "m");
        assert_eq!(json["folded"], 1);
        assert_eq!(
            json["stages"]["posted_to_admitted"][bucket_of(12_000)],
            serde_json::json!(1)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
