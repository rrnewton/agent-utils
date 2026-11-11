//! Per-profile chat latency summaries (#801 chat-latency-summaries) and their stamps, jams,
//! archive and weekly export (#897 chat-latency-stamps).
//!
//! Each request is folded once (marked on its own record) into a per-day
//! summary file under `<bridge-state>/latency/`, keyed by the bridge,
//! coordinator and the stamps it carried at delivery: model, effort,
//! adapter, harness, launch profile, role, protocol, and whether it was
//! admitted inside a jam. Summaries are bounded: ten stages over fixed
//! log buckets, well under a kilobyte a day. Folding is exact-once live and
//! in backfill because the request record itself carries `folded_at`.
//!
//! Beside them, in directories of their own: `latency-records/<day>.jsonl`
//! keeps one row of timings and stamps (no message text) per folded request,
//! `latency-exports/<week>.json` the weekly export, and `latency-jams.jsonl`
//! the jam windows. Everything is pruned after [`RETENTION_DAYS`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Stage names in order. The last two split `delivered_to_first_reply` where the target's
/// transcript is followed (#897 chat-latency-stamps): until its first tool call (thinking, or
/// waiting for its turn), and from then until the first reply (its tools and what follows).
pub const STAGES: [&str; 10] = [
    "posted_to_admitted",
    "admitted_to_ack",
    "ack_to_delivered",
    "delivered_to_first_reply",
    "posted_to_first_reply",
    "admitted_to_delivered",
    "delivered_to_first_tool",
    "first_tool_to_first_reply",
    // #1313: headless ingestion splits at queue acceptance, so it can
    // be compared with a terminal target's admitted -> typed.
    "admitted_to_accepted",
    "accepted_to_delivered",
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

/// The value a stamp gets when its source does not say (#897): never a guess.
pub const UNKNOWN: &str = "unknown";

fn unknown() -> String {
    UNKNOWN.to_owned()
}

/// What one request is folded under: the bridge, the agent it served, and the stamps that
/// agent's request carried at delivery (#801, #897 chat-latency-stamps).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldKey {
    /// Bridge name (its state directory's name).
    pub bridge: String,
    /// The agent the request was delivered to.
    pub coordinator: String,
    /// Model, as the agent record or its launch profile names it.
    pub model: String,
    /// Effective reasoning effort.
    pub effort: String,
    /// `tui` or `headless`.
    pub adapter: String,
    /// Harness (`claude`, `codex`, `muse`, `agentcloud`, `unknown` ...).
    pub harness: String,
    /// Launch profile name.
    pub profile: String,
    /// Role: `coordinator`, `liaison`, `coordinator-hands`, `worker`, or as labelled.
    pub role: String,
    /// Protocol: `self-tooling`, `hands-delegating`, or as labelled.
    pub protocol: String,
    /// Whether the request was admitted inside a jam window known when it was folded.
    pub jammed: bool,
}

impl FoldKey {
    /// A key with every stamp unknown, as a request folded before #897 has.
    #[must_use]
    pub fn unstamped(bridge: &str, coordinator: &str) -> Self {
        Self {
            bridge: bridge.to_owned(),
            coordinator: coordinator.to_owned(),
            model: unknown(),
            effort: unknown(),
            adapter: unknown(),
            harness: unknown(),
            profile: unknown(),
            role: unknown(),
            protocol: unknown(),
            jammed: false,
        }
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
    /// Harness (#897); `unknown` in a summary written before it was stamped.
    #[serde(default = "unknown")]
    pub harness: String,
    /// Launch profile name (#897).
    #[serde(default = "unknown")]
    pub profile: String,
    /// Role (#897).
    #[serde(default = "unknown")]
    pub role: String,
    /// Protocol (#897).
    #[serde(default = "unknown")]
    pub protocol: String,
    /// Whether these requests were admitted inside a jam window (#897).
    #[serde(default)]
    pub jammed: bool,
    /// ET day, `YYYY-MM-DD`.
    pub day: String,
    /// Per-stage bucket counts.
    pub stages: BTreeMap<String, Vec<u32>>,
    /// Requests folded into this file.
    pub folded: u64,
}

/// The directory a summary lives in: `<state>/latency/<key>/`. A key whose #897 stamps are all
/// unknown keeps the original `bridge+coordinator+model+effort+adapter` name; any other adds a
/// digest of the stamps, so the name stays short whatever they hold.
#[must_use]
pub fn summary_dir(state: &Path, key: &FoldKey) -> PathBuf {
    // "+" cannot appear in a slug, so key parts cannot collide.
    let mut name = format!(
        "{}+{}+{}+{}+{}",
        slug(&key.bridge),
        slug(&key.coordinator),
        slug(&key.model),
        slug(&key.effort),
        slug(&key.adapter)
    );
    let stamps = [&key.harness, &key.profile, &key.role, &key.protocol];
    if key.jammed || stamps.iter().any(|stamp| stamp.as_str() != UNKNOWN) {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for stamp in stamps {
            digest.update(stamp.as_bytes());
            digest.update([0]);
        }
        digest.update([u8::from(key.jammed)]);
        let digest = digest.finalize();
        name.push('+');
        for byte in &digest[..8] {
            name.push_str(&format!("{byte:02x}"));
        }
    }
    state.join("latency").join(name)
}

/// Load one summary file, or a fresh empty one.
#[must_use]
pub fn load(path: &Path, key: &FoldKey, day: &str) -> Summary {
    if let Ok(raw) = std::fs::read(path) {
        if let Ok(summary) = serde_json::from_slice(&raw) {
            return summary;
        }
    }
    Summary {
        schema: "agentctl.chat-latency/1".to_owned(),
        bridge: key.bridge.clone(),
        coordinator: key.coordinator.clone(),
        model: key.model.clone(),
        effort: key.effort.clone(),
        adapter: key.adapter.clone(),
        harness: key.harness.clone(),
        profile: key.profile.clone(),
        role: key.role.clone(),
        protocol: key.protocol.clone(),
        jammed: key.jammed,
        day: day.to_owned(),
        stages: STAGES
            .iter()
            .map(|stage| ((*stage).to_owned(), vec![0; EDGES_MS.len() + 1]))
            .collect(),
        folded: 0,
    }
}

/// Write `bytes` to `path` through a unique temporary file, synced before the rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Fold one request's stage durations (milliseconds, missing = skipped)
/// into its day file. Atomic tmp-plus-rename write.
///
/// # Errors
/// Returns an I/O or serialization error string.
pub fn fold(
    state: &Path,
    key: &FoldKey,
    day: &str,
    durations: &[Option<u64>; STAGES.len()],
) -> Result<(), String> {
    let dir = summary_dir(state, key);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{day}.json"));
    let mut summary = load(&path, key, day);
    for (stage, duration) in STAGES.iter().zip(durations.iter()) {
        if let Some(millis) = duration {
            summary
                .stages
                .entry((*stage).to_owned())
                .or_insert_with(|| vec![0; EDGES_MS.len() + 1])[bucket_of(*millis)] += 1;
        }
    }
    summary.folded += 1;
    let bytes = serde_json::to_vec(&summary).map_err(|e| e.to_string())?;
    write_atomic(&path, &bytes)
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

/// The dimensions `chat latency --by` groups by (#897). `profile` keeps its #801 meaning, the
/// model, effort and adapter together; `launch-profile` is the launch profile's name.
pub const DIMENSIONS: [&str; 9] = [
    "profile",
    "model",
    "effort",
    "adapter",
    "harness",
    "launch-profile",
    "role",
    "protocol",
    "coordinator",
];

/// The dimensions named by `by`, a comma-separated list of [`DIMENSIONS`], with `profile`
/// expanded to `model`, `effort` and `adapter`.
///
/// # Errors
/// A name that is not a dimension, or an empty list.
pub fn parse_dimensions(by: &str) -> Result<Vec<&'static str>, String> {
    let mut out: Vec<&'static str> = Vec::new();
    for name in by.split(',').map(str::trim).filter(|name| !name.is_empty()) {
        let expanded: &[&'static str] = match name {
            "profile" => &["model", "effort", "adapter"],
            other => match DIMENSIONS.iter().find(|known| **known == other) {
                Some(known) => std::slice::from_ref(known),
                None => {
                    return Err(format!(
                        "--by takes a comma-separated list of {}; {other:?} is not one",
                        DIMENSIONS.join(", ")
                    ))
                }
            },
        };
        for dimension in expanded {
            if !out.contains(dimension) {
                out.push(dimension);
            }
        }
    }
    if out.is_empty() {
        return Err(format!(
            "--by needs one or more of {}",
            DIMENSIONS.join(", ")
        ));
    }
    Ok(out)
}

/// A summary's value for one dimension from [`parse_dimensions`].
#[must_use]
pub fn dimension<'a>(summary: &'a Summary, name: &str) -> &'a str {
    match name {
        "model" => &summary.model,
        "effort" => &summary.effort,
        "adapter" => &summary.adapter,
        "harness" => &summary.harness,
        "launch-profile" => &summary.profile,
        "role" => &summary.role,
        "protocol" => &summary.protocol,
        "coordinator" => &summary.coordinator,
        _ => UNKNOWN,
    }
}

/// `summaries` grouped by the values of `dimensions`, leaving out jammed ones when
/// `exclude_jams`.
#[must_use]
pub fn group<'a>(
    summaries: &'a [Summary],
    dimensions: &[&str],
    exclude_jams: bool,
) -> BTreeMap<Vec<String>, Vec<&'a Summary>> {
    let mut groups: BTreeMap<Vec<String>, Vec<&Summary>> = BTreeMap::new();
    for summary in summaries {
        if exclude_jams && summary.jammed {
            continue;
        }
        let values = dimensions
            .iter()
            .map(|name| dimension(summary, name).to_owned())
            .collect();
        groups.entry(values).or_default().push(summary);
    }
    groups
}

/// The stages of a group of summaries as JSON: count, median and p90 buckets, and counts.
#[must_use]
pub fn stages_json(group: &[&Summary]) -> serde_json::Map<String, serde_json::Value> {
    merge(group)
        .iter()
        .map(|(stage, counts)| {
            (
                stage.clone(),
                serde_json::json!({
                    "n": counts.iter().sum::<u32>(),
                    "median": quantile_bucket(counts, 0.5).map(bucket_label),
                    "p90": quantile_bucket(counts, 0.9).map(bucket_label),
                    "counts": counts,
                }),
            )
        })
        .collect()
}

/// How long jam windows, stamped records, exports and day summaries are kept (#897).
pub const RETENTION_DAYS: i64 = 90;
/// The jam windows, one JSON object per line.
pub const JAMS_FILE: &str = "latency-jams.jsonl";
/// One `<ET day>.jsonl` of stamped request rows per day.
pub const RECORDS_DIR: &str = "latency-records";
/// One `<ISO week>.json` export per week.
pub const EXPORTS_DIR: &str = "latency-exports";
/// The causes a jam window is given. A delivery that waits while the agent works is queueing,
/// the latency being measured, and never a jam.
pub const JAM_CAUSES: [&str; 8] = [
    "composer-blocked",
    "pane-lock",
    "queue-held",
    "refused-retry",
    "target-down",
    "bridge-restart",
    "provider-gap",
    "other",
];
/// A blocked delivery that lasted at least this long, from when it was first blocked, marks a
/// jam window.
pub const JAM_AFTER_MILLIS: u64 = 5 * 60 * 1_000;

/// A period in which the bridge could not deliver (#897): requests admitted inside one are left
/// out by `chat latency --exclude-jams`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jam {
    /// Start, Unix milliseconds.
    pub start_millis: u64,
    /// End, Unix milliseconds.
    pub end_millis: u64,
    /// One of [`JAM_CAUSES`].
    pub cause: String,
    /// `bridge` (marked on a slow delivery) or `operator` (`chat jam`).
    pub source: String,
    /// The request whose delivery marked it, for a bridge mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// What happened, for an operator mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Append one line to `path`, creating it private (0600).
fn append_line(path: &Path, line: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Record a jam window.
///
/// # Errors
/// An unknown cause, an end before the start, or an I/O error.
pub fn append_jam(state: &Path, jam: &Jam) -> Result<(), String> {
    if !JAM_CAUSES.contains(&jam.cause.as_str()) {
        return Err(format!(
            "jam cause {:?} is not one of {}",
            jam.cause,
            JAM_CAUSES.join(", ")
        ));
    }
    if jam.end_millis < jam.start_millis {
        return Err("a jam window cannot end before it starts".to_owned());
    }
    let line = serde_json::to_string(jam).map_err(|e| e.to_string())?;
    append_line(&state.join(JAMS_FILE), &line)
}

/// Every readable jam window; a line that does not parse is skipped.
#[must_use]
pub fn read_jams(state: &Path) -> Vec<Jam> {
    std::fs::read_to_string(state.join(JAMS_FILE))
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `at_millis` falls inside any of `jams`.
#[must_use]
pub fn in_jam(jams: &[Jam], at_millis: u64) -> bool {
    jams.iter()
        .any(|jam| (jam.start_millis..=jam.end_millis).contains(&at_millis))
}

/// The jam cause a delivery error names, when it says the delivery was blocked: a stranded paste
/// or a draft in the composer, a wait for the pane's lock, a held or paused queue, a refusal that
/// is retried, or a target that is down. `None` for anything else, above all an agent that is
/// busy, which is queueing behind its work rather than a jam.
#[must_use]
pub fn jam_cause(error: &str) -> Option<&'static str> {
    let error = error.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| error.contains(word));
    if has(&[
        "did not become idle",
        "running turn",
        "last status=working",
        "busy",
    ]) {
        None
    } else if has(&[
        "stranded",
        "composer already holds",
        "draft",
        "menu or dialog",
    ]) {
        Some("composer-blocked")
    } else if error.match_indices("lock").any(|(at, _)| {
        // "lock" as a word of its own, not inside "blocked" or "clock".
        !error[..at].ends_with(|c: char| c.is_ascii_alphabetic())
    }) {
        Some("pane-lock")
    } else if has(&["paused", "on hold", "held by", "hold "]) {
        Some("queue-held")
    } else if has(&[
        "wrapper is down",
        "not running",
        "is stopped",
        "no live",
        "dead",
    ]) {
        Some("target-down")
    } else if has(&["refus"]) {
        Some("refused-retry")
    } else {
        None
    }
}

/// Append one stamped request row (timings and stamps, no text) to its day's archive.
///
/// # Errors
/// An I/O or serialization error.
pub fn append_record(state: &Path, day: &str, row: &serde_json::Value) -> Result<(), String> {
    let dir = state.join(RECORDS_DIR);
    create_private_dir(&dir)?;
    let line = serde_json::to_string(row).map_err(|e| e.to_string())?;
    append_line(&dir.join(format!("{day}.jsonl")), &line)
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))
}

/// The day number (days since 1970-01-01) of `YYYY-MM-DD`.
#[must_use]
pub fn day_number(day: &str) -> Option<i64> {
    let mut parts = day.splitn(3, '-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let date: i64 = parts.next()?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&date)).then(|| days(year, month, date))
}

/// `YYYY-MM-DD` of a day number.
#[must_use]
pub fn day_string(number: i64) -> String {
    let (y, m, d) = civil(number);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The ISO week holding day number `number`: its Monday's day number and its label, such as
/// `2026-W45`.
#[must_use]
pub fn iso_week(number: i64) -> (i64, String) {
    // 1970-01-01 was a Thursday.
    let monday = number - (number + 3).rem_euclid(7);
    let thursday = monday + 3;
    let (year, _, _) = civil(thursday);
    let week = (thursday - days(year, 1, 1)) / 7 + 1;
    (monday, format!("{year:04}-W{week:02}"))
}

/// Write the export of the ISO week starting on day number `monday`: every summary of its days,
/// grouped by every stamp and by whether it was jammed. Timings only.
///
/// # Errors
/// An I/O or serialization error.
pub fn export_week(state: &Path, monday: i64, now_millis: u64) -> Result<PathBuf, String> {
    let (monday, label) = iso_week(monday);
    let (from, until) = (day_string(monday), day_string(monday + 6));
    let summaries = read_window(state, &from, &until);
    let mut dimensions = DIMENSIONS[1..].to_vec();
    dimensions.retain(|name| *name != "coordinator");
    let mut groups: BTreeMap<(Vec<String>, bool), Vec<&Summary>> = BTreeMap::new();
    for summary in &summaries {
        let values = dimensions
            .iter()
            .map(|name| dimension(summary, name).to_owned())
            .collect();
        groups
            .entry((values, summary.jammed))
            .or_default()
            .push(summary);
    }
    let groups: Vec<serde_json::Value> = groups
        .iter()
        .map(|((values, jammed), group)| {
            let mut row = serde_json::Map::new();
            for (name, value) in dimensions.iter().zip(values) {
                row.insert((*name).to_owned(), serde_json::json!(value));
            }
            row.insert("jammed".to_owned(), serde_json::json!(jammed));
            row.insert(
                "folded".to_owned(),
                serde_json::json!(group.iter().map(|s| s.folded).sum::<u64>()),
            );
            row.insert(
                "stages".to_owned(),
                serde_json::Value::Object(stages_json(group)),
            );
            serde_json::Value::Object(row)
        })
        .collect();
    let document = serde_json::json!({
        "schema": "agentctl-chat-latency-export/v1",
        "week": label,
        "from": from,
        "until": until,
        "generated_at_millis": now_millis,
        "bucket_edges_millis": EDGES_MS,
        "groups": groups,
    });
    let dir = state.join(EXPORTS_DIR);
    create_private_dir(&dir)?;
    let path = dir.join(format!("{label}.json"));
    let bytes = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
    write_atomic(&path, &bytes)?;
    Ok(path)
}

/// Remove what is older than [`RETENTION_DAYS`] before ET day `today`: day summaries, stamped
/// record days, exports whose week ended before then, and jam windows that ended before then.
/// Returns how many files were removed and jam windows dropped.
#[must_use]
pub fn prune(state: &Path, today: &str) -> usize {
    let Some(cutoff) = day_number(today).map(|today| today - RETENTION_DAYS) else {
        return 0;
    };
    let old_day = |name: &str, suffix: &str| {
        name.strip_suffix(suffix)
            .and_then(day_number)
            .is_some_and(|day| day < cutoff)
    };
    let mut removed = 0;
    let mut remove = |path: &Path| {
        if std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    };
    if let Ok(dirs) = std::fs::read_dir(state.join("latency")) {
        for dir in dirs.flatten() {
            let Ok(files) = std::fs::read_dir(dir.path()) else {
                continue;
            };
            for file in files.flatten() {
                if old_day(&file.file_name().to_string_lossy(), ".json") {
                    remove(&file.path());
                }
            }
            let _ = std::fs::remove_dir(dir.path());
        }
    }
    if let Ok(files) = std::fs::read_dir(state.join(RECORDS_DIR)) {
        for file in files.flatten() {
            if old_day(&file.file_name().to_string_lossy(), ".jsonl") {
                remove(&file.path());
            }
        }
    }
    if let Ok(files) = std::fs::read_dir(state.join(EXPORTS_DIR)) {
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let ended = name
                .strip_suffix(".json")
                .and_then(|label| {
                    let (year, week) = label.split_once("-W")?;
                    let (year, week): (i64, i64) = (year.parse().ok()?, week.parse().ok()?);
                    let (first_monday, _) = iso_week(days(year, 1, 4));
                    Some(first_monday + (week - 1) * 7 + 6)
                })
                .is_some_and(|last| last < cutoff);
            if ended {
                remove(&file.path());
            }
        }
    }
    let jams = read_jams(state);
    let cutoff_millis = u64::try_from(cutoff.max(0)).unwrap_or(0) * 86_400_000;
    let kept: Vec<&Jam> = jams
        .iter()
        .filter(|jam| jam.end_millis >= cutoff_millis)
        .collect();
    if kept.len() < jams.len() {
        let text: String = kept
            .iter()
            .filter_map(|jam| serde_json::to_string(jam).ok())
            .map(|line| line + "\n")
            .collect();
        if write_atomic(&state.join(JAMS_FILE), text.as_bytes()).is_ok() {
            removed += jams.len() - kept.len();
        }
    }
    removed
}

/// The weekly upkeep the service runs (#897): once the export of the last full ISO week before
/// `now_millis` is missing, write it, then prune. Returns the export written, if any.
///
/// # Errors
/// The export cannot be written.
pub fn maintain(state: &Path, now_millis: u64) -> Result<Option<PathBuf>, String> {
    let today = et_date(now_millis as f64 / 1_000.0);
    let Some(number) = day_number(&today) else {
        return Ok(None);
    };
    let (this_monday, _) = iso_week(number);
    let (last_monday, label) = iso_week(this_monday - 7);
    if state
        .join(EXPORTS_DIR)
        .join(format!("{label}.json"))
        .exists()
    {
        return Ok(None);
    }
    let path = export_week(state, last_monday, now_millis)?;
    let _ = prune(state, &today);
    Ok(Some(path))
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

    fn key() -> FoldKey {
        FoldKey {
            model: "m".into(),
            effort: "high".into(),
            adapter: "tui".into(),
            ..FoldKey::unstamped("b", "c")
        }
    }

    /// Durations with only the first stage present.
    fn first(millis: Option<u64>) -> [Option<u64>; STAGES.len()] {
        let mut durations = [None; STAGES.len()];
        durations[0] = millis;
        durations
    }

    #[test]
    fn buckets_slugs_and_quantiles() {
        assert_eq!(bucket_of(99), 0);
        assert_eq!(bucket_of(100), 1);
        assert_eq!(bucket_of(3_600_000), 14);
        assert_eq!(bucket_label(7), "10-25 s");
        assert_eq!(slug("Claude Opus 4.5/high"), "claude-opus-4-5-high");
        let key = FoldKey {
            model: "m".into(),
            effort: "e".into(),
            adapter: "tui".into(),
            ..FoldKey::unstamped("a-b", "c")
        };
        assert!(summary_dir(Path::new("/s"), &key)
            .to_string_lossy()
            .ends_with("latency/a-b+c+m+e+tui"));
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
            fold(state, &key(), "2026-11-03", &first(Some(12_000))).unwrap();
        }
        let all = read_window(state, "2026-11-01", "2026-11-30");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].folded, 3);
        assert_eq!(all[0].stages["posted_to_admitted"][7], 3);
        let raw = std::fs::read(summary_dir(state, &key()).join("2026-11-03.json")).unwrap();
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
            fold(state, &key(), day, &first(Some(12_000))).unwrap();
        }
        fold(state, &key(), "2026-11-04", &first(Some(120))).unwrap();
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
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("latency-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stamped_requests_fold_apart_and_group_by_any_dimension() {
        // #897 chat-latency-stamps: role and protocol keep requests apart, the directory name
        // stays short whatever the stamps hold, and jammed summaries can be left out.
        let state = scratch("stamps");
        let liaison = FoldKey {
            harness: "claude".into(),
            profile: "coordinator".into(),
            role: "liaison".into(),
            protocol: "hands-delegating".into(),
            ..key()
        };
        let worker = FoldKey {
            role: "worker".into(),
            protocol: "self-tooling".into(),
            profile: "x".repeat(200),
            ..key()
        };
        let jammed = FoldKey {
            jammed: true,
            ..liaison.clone()
        };
        for (fold_key, millis) in [(&liaison, 2_000), (&worker, 30_000), (&jammed, 900_000)] {
            fold(&state, fold_key, "2026-11-05", &first(Some(millis))).unwrap();
        }
        assert_ne!(summary_dir(&state, &liaison), summary_dir(&state, &jammed));
        let name = summary_dir(&state, &worker);
        assert!(name.file_name().unwrap().len() < 120, "{}", name.display());
        let all = read_window(&state, "2026-11-01", "2026-11-30");
        assert_eq!(all.len(), 3);
        let by_role = group(&all, &parse_dimensions("role,protocol").unwrap(), false);
        assert_eq!(by_role.len(), 2);
        assert_eq!(
            by_role[&vec!["liaison".to_owned(), "hands-delegating".to_owned()]].len(),
            2
        );
        let clean = group(&all, &["role"], true);
        assert_eq!(
            clean[&vec!["liaison".to_owned()]].len(),
            1,
            "the jammed one is left out"
        );
        assert_eq!(
            parse_dimensions("profile,role").unwrap(),
            ["model", "effort", "adapter", "role"]
        );
        assert!(parse_dimensions("colour")
            .unwrap_err()
            .contains("\"colour\""));
        assert!(parse_dimensions(" , ").is_err());
        std::fs::remove_dir_all(&state).ok();
    }

    #[test]
    fn jam_windows_are_checked_kept_and_matched() {
        let state = scratch("jams");
        let jam = Jam {
            start_millis: 1_000,
            end_millis: 5_000,
            cause: "composer-blocked".into(),
            source: "bridge".into(),
            request: Some("k".into()),
            note: None,
        };
        append_jam(&state, &jam).unwrap();
        assert!(append_jam(
            &state,
            &Jam {
                cause: "gremlins".into(),
                ..jam.clone()
            }
        )
        .unwrap_err()
        .contains("not one of"));
        assert!(append_jam(
            &state,
            &Jam {
                end_millis: 10,
                ..jam.clone()
            }
        )
        .is_err());
        let jams = read_jams(&state);
        assert_eq!(jams, [jam]);
        assert!(in_jam(&jams, 1_000) && in_jam(&jams, 5_000) && !in_jam(&jams, 5_001));
        // A delivery that waits while the agent works is queueing, not a jam.
        for (error, cause) in [
            ("pane p1: the prompt still sits at the bottom of the screen (stranded)", Some("composer-blocked")),
            ("pane p1 composer already holds unsubmitted text \"x\"", Some("composer-blocked")),
            ("timed out waiting for the pane lock", Some("pane-lock")),
            ("the queue is blocked behind another sender", None),
            ("the queue is paused", Some("queue-held")),
            ("the headless wrapper is down", Some("target-down")),
            ("agentterm refused the paste; retrying", Some("refused-retry")),
            ("message m remains pending: pane p1 did not become idle/done within 0s; last status=working", None),
            ("pane p1: agentterm shows a running turn, and Enter would steer it", None),
            ("something else", None),
        ] {
            assert_eq!(jam_cause(error), cause, "{error}");
        }
        std::fs::remove_dir_all(&state).ok();
    }

    #[test]
    fn weeks_export_once_and_old_latency_stats_are_pruned() {
        let state = scratch("export");
        let day = day_number("2026-11-05").unwrap();
        assert_eq!(day_string(day), "2026-11-05");
        let (monday, label) = iso_week(day);
        assert_eq!(
            (day_string(monday).as_str(), label.as_str()),
            ("2026-11-02", "2026-W45")
        );
        assert_eq!(iso_week(day_number("2027-01-01").unwrap()).1, "2026-W53");
        fold(&state, &key(), "2026-10-28", &first(Some(2_000))).unwrap();
        fold(
            &state,
            &FoldKey {
                jammed: true,
                ..key()
            },
            "2026-10-29",
            &first(Some(9_000)),
        )
        .unwrap();
        // 2026-11-05 12:00 ET: last week is 2026-W44, 10-26 to 11-01.
        let now = 1_793_898_000_000;
        let path = maintain(&state, now).unwrap().expect("written");
        assert!(
            path.ends_with("latency-exports/2026-W44.json"),
            "{}",
            path.display()
        );
        let export: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(export["from"], "2026-10-26");
        assert_eq!(export["until"], "2026-11-01");
        assert_eq!(
            export["groups"].as_array().unwrap().len(),
            2,
            "jammed apart"
        );
        assert_eq!(export["groups"][0]["stages"]["posted_to_admitted"]["n"], 1);
        assert!(maintain(&state, now).unwrap().is_none(), "once a week");
        // 91 days before 2026-11-05 is 2026-08-06: everything of that day goes.
        let old = "2026-08-06";
        fold(&state, &key(), old, &first(Some(1))).unwrap();
        append_record(&state, old, &serde_json::json!({"key": "old"})).unwrap();
        append_record(&state, "2026-11-05", &serde_json::json!({"key": "new"})).unwrap();
        let old_millis = u64::try_from(day_number(old).unwrap()).unwrap() * 86_400_000;
        let old_jam = Jam {
            start_millis: old_millis,
            end_millis: old_millis + 1,
            cause: "other".into(),
            source: "operator".into(),
            request: None,
            note: Some("old".into()),
        };
        append_jam(&state, &old_jam).unwrap();
        append_jam(
            &state,
            &Jam {
                start_millis: now,
                end_millis: now,
                ..old_jam.clone()
            },
        )
        .unwrap();
        std::fs::write(state.join(EXPORTS_DIR).join("2026-W30.json"), "{}").unwrap();
        assert_eq!(prune(&state, "2026-11-05"), 4);
        assert!(!summary_dir(&state, &key())
            .join(format!("{old}.json"))
            .exists());
        assert!(summary_dir(&state, &key()).join("2026-10-28.json").exists());
        assert!(!state
            .join(RECORDS_DIR)
            .join(format!("{old}.jsonl"))
            .exists());
        assert!(state.join(RECORDS_DIR).join("2026-11-05.jsonl").exists());
        assert_eq!(read_jams(&state).len(), 1);
        assert!(path.exists(), "a recent export stays");
        std::fs::remove_dir_all(&state).ok();
    }
}
