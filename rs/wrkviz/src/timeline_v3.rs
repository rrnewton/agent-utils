//! Schema 3 timeline shards at parity with `py/wrkviz/timeline_v3.py`:
//! per-day timeline shards, spine shards, one bins shard, and the three
//! search streams under `data/timeline-v3/`, plus the bootstrap.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::json::{
    as_array, as_int, as_object, as_string, dump_canonical, dump_compact, py_repr_str,
};
use crate::search_bloom::{build_trigram_bloom, compact_search_text};
use crate::seekable::{index_path_for, write_seekable_jsonl, WriteReport};
use crate::transcript_io::write_text_if_changed;

/// Bootstrap root directory.
pub const SCHEMA_3_ROOT: &str = "data/timeline-v3";
/// Timeline stream root.
pub const SCHEMA_3_TIMELINE_ROOT: &str = "data/timeline-v3/timeline";
/// Spine stream root.
pub const SCHEMA_3_SPINE_ROOT: &str = "data/timeline-v3/spine";
/// Search stream root.
pub const SCHEMA_3_SEARCH_ROOT: &str = "data/timeline-v3/search";
/// Search bloom root.
pub const SCHEMA_3_SEARCH_BLOOM_ROOT: &str = "data/timeline-v3/search-bloom";
/// Search links root.
pub const SCHEMA_3_SEARCH_LINKS_ROOT: &str = "data/timeline-v3/search-links";
/// Bins path.
pub const SCHEMA_3_BINS_PATH: &str = "data/timeline-v3/bins.jsonl.gz";
/// Bootstrap path.
pub const SCHEMA_3_BOOTSTRAP_PATH: &str = "data/timeline-v3/bootstrap.json";
/// Search streams.
pub const SCHEMA_3_SEARCH_STREAMS: [&str; 3] = ["search", "search_bloom", "search_links"];
/// Record kind key.
pub const SCHEMA_3_RECORD_KIND_KEY: &str = "record_kind";
/// Timestamp key.
pub const SCHEMA_3_TIMESTAMP_KEY: &str = "at_ms";
/// Schema 3 version.
pub const SCHEMA_3_VERSION: i64 = 3;
/// Index suffix.
pub const INDEX_SUFFIX: &str = ".index.jsonl";
/// Day ms.
pub const DAY_MS: i64 = 86_400_000;
const TARGET_CHUNK_BYTES: u64 = 1 << 20;

const BOOTSTRAP_FIELDS: [&str; 11] = [
    "schema_version",
    "site",
    "date_range",
    "display_timezone",
    "project_overview",
    "glossary",
    "generation_inputs",
    "generation_inputs_revision",
    "objective_counts",
    "objective_digest",
    "project_overviews",
];
const BOOTSTRAP_SINGLE_FIELDS: [&str; 6] = [
    "site",
    "date_range",
    "display_timezone",
    "project_overview",
    "glossary",
    "project_overviews",
];
const KNOWN_FIELDS: [&str; 20] = [
    "schema_version",
    "site",
    "teams",
    "agents",
    "phases",
    "events",
    "edges",
    "rollups",
    "projects",
    "summary_files",
    "activity_bins",
    "date_range",
    "display_timezone",
    "project_overview",
    "glossary",
    "generation_inputs",
    "generation_inputs_revision",
    "objective_counts",
    "objective_digest",
    "project_overviews",
];
const SPINE_KIND_ORDER: [&str; 10] = [
    "team",
    "agent",
    "activity_bounds",
    "phase_card",
    "structural_edge",
    "rollup",
    "project",
    "summary_file",
    "glossary_term",
    "project_overview",
];
const PHASE_CARD_FIELDS: [&str; 14] = [
    "activity_source_counts",
    "agent_label",
    "end_ms",
    "generated_at",
    "id",
    "input_hash",
    "model",
    "paragraph",
    "phrase",
    "prompt_version",
    "start_ms",
    "summary_available",
    "team",
    "work_summary",
];
const SPINE_SORT_FIELD: [(&str, &str); 10] = [
    ("agent", "id"),
    ("activity_bounds", "ref"),
    ("phase_card", "id"),
    ("structural_edge", "id"),
    ("rollup", "id"),
    ("project", "id"),
    ("summary_file", "path"),
    ("glossary_term", "term_id"),
    ("project_overview", "team"),
    ("team", "path"),
];
const STRUCTURAL_EDGE_KINDS: [&str; 2] = ["spawn", "continuation"];
const SEARCH_KNOWLEDGE_KIND: &str = "knowledge";
const SEARCH_RECORD_KIND: &str = "search_record";
const ACTIVITY_BOUNDS_KIND: &str = "activity_bounds";
const SEARCH_BLOOM_KIND: &str = "search_bloom";
const SEARCH_PROMPT_KIND: &str = "search_prompt_excerpt";
const SEARCH_RESPONSE_KIND: &str = "search_response_link";
const SEARCH_LINK_KIND_ORDER: [&str; 2] = [SEARCH_PROMPT_KIND, SEARCH_RESPONSE_KIND];
const SEARCH_EXCERPT_CHARACTERS: usize = 200;
const SEARCH_PROMPT_TYPES: [&str; 2] = ["prompt", "inter_agent_prompt"];
const SEARCH_RESPONSE_TYPES: [&str; 6] = [
    "response",
    "inter_agent",
    "inter_agent_prompt",
    "inter_agent_response",
    "tool",
    "external",
];

fn slug_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

#[derive(Clone)]
struct Line {
    at_ms: Option<i64>,
    kind: String,
    key: String,
    line: String,
    record: Value,
    end_ms: Option<i64>,
}

impl Line {
    fn time_order(&self) -> (i64, String, String) {
        (self.at_ms.unwrap_or(0), self.kind.clone(), self.line.clone())
    }
    fn kind_order(&self) -> (String, String) {
        (self.key.clone(), self.line.clone())
    }
    fn search_order(&self) -> (i64, String, String) {
        (self.at_ms.unwrap_or(0), self.key.clone(), self.line.clone())
    }
}

/// One written shard.
struct ShardReport {
    stream: String,
    team: Option<String>,
    day: Option<String>,
    relative_path: String,
    write: WriteReport,
    t_end_exclusive: Option<i64>,
    counts: BTreeMap<String, i64>,
    line_ranges: BTreeMap<String, (i64, i64)>,
}

impl ShardReport {
    fn index_relative_path(&self) -> String {
        index_path_for(Path::new(&self.relative_path))
            .display()
            .to_string()
            .replace('\\', "/")
    }
    fn generated_files(&self) -> Vec<String> {
        vec![self.relative_path.clone(), self.index_relative_path()]
    }
    fn catalog_obj(&self) -> Value {
        let index = &self.write.index;
        let members: Vec<Value> = index.members.iter().map(|member| {
            let mut entry = serde_json::Map::new();
            entry.insert("c_off".into(), member.c_off.into());
            entry.insert("c_len".into(), member.c_len.into());
            entry.insert("u_off".into(), member.u_off.into());
            entry.insert("u_len".into(), member.u_len.into());
            entry.insert("l0".into(), member.l0.into());
            entry.insert("n".into(), member.n.into());
            entry.insert("t0".into(), member.t0.map_or(Value::Null, Into::into));
            entry.insert("t1".into(), member.t1.map_or(Value::Null, Into::into));
            Value::Object(entry)
        }).collect();
        let ranges: Value = {
            let mut out = serde_json::Map::new();
            for (kind, (start, line_count)) in &self.line_ranges {
                let mut entry = serde_json::Map::new();
                entry.insert("start".into(), (*start).into());
                entry.insert("lines".into(), (*line_count).into());
                out.insert(kind.clone(), Value::Object(entry));
            }
            Value::Object(out)
        };
        let counts: Value = {
            let mut out = serde_json::Map::new();
            for (kind, count) in &self.counts {
                out.insert(kind.clone(), (*count).into());
            }
            Value::Object(out)
        };
        let mut out = serde_json::Map::new();
        out.insert("file".into(), Value::String(self.relative_path.clone()));
        out.insert(
            "index_file".into(),
            Value::String(self.index_relative_path()),
        );
        out.insert("stream".into(), Value::String(self.stream.clone()));
        out.insert(
            "team".into(),
            self.team.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "day".into(),
            self.day.clone().map_or(Value::Null, Value::String),
        );
        out.insert("records".into(), index.record_count.into());
        out.insert("c_size".into(), index.c_size.into());
        out.insert("u_size".into(), index.u_size.into());
        out.insert(
            "c_sha256".into(),
            Value::String(index.c_sha256.clone()),
        );
        out.insert(
            "u_sha256".into(),
            Value::String(index.u_sha256.clone()),
        );
        out.insert(
            "timestamps_sorted".into(),
            Value::Bool(index.timestamps_sorted()),
        );
        out.insert("members".into(), Value::Array(members));
        out.insert("counts".into(), counts);
        out.insert("line_ranges".into(), ranges);
        out.insert(
            "t0".into(),
            index
                .members
                .first()
                .and_then(|member| member.t0)
                .map_or(Value::Null, Into::into),
        );
        out.insert(
            "t1".into(),
            index
                .members
                .last()
                .and_then(|member| member.t1)
                .map_or(Value::Null, Into::into),
        );
        out.insert(
            "t_end_exclusive".into(),
            self.t_end_exclusive.map_or(Value::Null, Into::into),
        );
        Value::Object(out)
    }
}

/// What write_timeline_v3 did.
pub struct TimelineV3Report {
    /// Files changed.
    pub files_changed: i64,
    /// Generated files.
    pub generated_files: Vec<String>,
    /// Timeline shards.
    pub timeline_shards: i64,
    /// Spine shards.
    pub spine_shards: i64,
    /// Search shards.
    pub search_shards: i64,
    /// Search records.
    pub search_records: i64,
    /// Search bytes.
    pub search_bytes: u64,
    /// Record count.
    pub record_count: u64,
    /// Bootstrap bytes.
    pub bootstrap_bytes: usize,
    /// Compressed bytes.
    pub compressed_bytes: u64,
    /// Uncompressed bytes.
    pub uncompressed_bytes: u64,
    /// Index bytes.
    pub index_bytes: u64,
    /// Removed files.
    pub removed_files: Vec<String>,
}

impl TimelineV3Report {
    /// Total on-disk bytes.
    pub fn total_bytes(&self) -> u64 {
        self.compressed_bytes + self.index_bytes + self.bootstrap_bytes as u64
    }
}

fn local_identifier(team: &str, identifier: &str) -> String {
    let prefix = format!("{team}::");
    identifier
        .strip_prefix(&prefix)
        .map(str::to_string)
        .unwrap_or_else(|| identifier.to_string())
}

/// The archive's stable reference for one agent.
pub fn agent_reference(team: &str, identifier: &str) -> String {
    format!("agent:{team}::{}", local_identifier(team, identifier))
}

/// The archive's stable reference for one phase.
pub fn phase_reference(team: &str, identifier: &str) -> String {
    format!("phase:{team}::{}", local_identifier(team, identifier))
}

/// The archive's stable reference for one rollup.
pub fn rollup_reference(team: &str, kind: &str, start_ms: i64) -> String {
    format!("rollup:{team}::{kind}::{start_ms}")
}

/// The UTC midnight at or before a timestamp.
pub fn utc_day_start(timestamp_ms: i64) -> i64 {
    (timestamp_ms.div_euclid(DAY_MS)) * DAY_MS
}

/// The YYYY-MM-DD shard label for a day start.
pub fn utc_day_label(day_start_ms: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Utc
        .timestamp_millis_opt(day_start_ms)
        .single()
        .map(|item| item.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn encode(record: &Value) -> Result<String, String> {
    if crate::json::has_nonfinite(record) {
        return Err("schema-3 record contains a non-finite number".to_string());
    }
    Ok(dump_compact(record))
}

/// Whether a path is schema-3 shaped.
pub fn is_schema_3_path(relative: &str) -> bool {
    if relative == SCHEMA_3_BOOTSTRAP_PATH {
        return true;
    }
    let body = relative.strip_suffix(INDEX_SUFFIX).unwrap_or(relative);
    if !body.ends_with(".jsonl.gz") {
        return false;
    }
    let root: Vec<&str> = SCHEMA_3_ROOT.split('/').collect();
    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() < root.len() || parts[..root.len()] != root[..] {
        return false;
    }
    let tail = &parts[root.len()..];
    if tail.is_empty() {
        return false;
    }
    for part in tail {
        let stem = part.strip_suffix(".jsonl.gz").unwrap_or(part);
        if stem.is_empty() || stem.starts_with('.') || !stem.chars().all(slug_char) {
            return false;
        }
    }
    let depths: HashMap<&str, usize> = [
        ("bins.jsonl.gz", 1usize),
        ("spine", 2),
        ("search-bloom", 2),
        ("search-links", 2),
        ("timeline", 3),
        ("search", 3),
    ]
    .into_iter()
    .collect();
    depths.get(tail[0]).copied() == Some(tail.len())
}

/// Whether a bare name is one of the two schema-3 shapes.
pub fn is_schema_3_shard_name(name: &str) -> bool {
    name.ends_with(".jsonl.gz") || name.ends_with(".jsonl.gz.index.jsonl")
}

fn safe_slug(slug: &str, where_: &str) -> Result<String, String> {
    if slug.is_empty() || slug.starts_with('.') || !slug.chars().all(slug_char) {
        return Err(format!(
            "{where_}: team slug is not a safe path component: {}",
            py_repr_str(slug)
        ));
    }
    Ok(slug.to_string())
}

fn envelope(
    record: &Value,
    kind: &str,
    at_ms: Option<i64>,
    where_: &str,
) -> Result<Value, String> {
    let mut projected = match record.clone() {
        Value::Object(map) => map,
        _ => return Err(format!("{where_}: expected an object")),
    };
    if projected.contains_key(SCHEMA_3_RECORD_KIND_KEY) {
        return Err(format!(
            "{where_}: record already carries {}; the schema-3 envelope would overwrite it",
            py_repr_str(SCHEMA_3_RECORD_KIND_KEY)
        ));
    }
    projected.insert(
        SCHEMA_3_RECORD_KIND_KEY.into(),
        Value::String(kind.into()),
    );
    if let Some(at_ms) = at_ms {
        if let Some(existing) = projected.get(SCHEMA_3_TIMESTAMP_KEY) {
            if !existing.is_null()
                && existing.as_i64() != Some(at_ms)
            {
                return Err(format!(
                    "{where_}: record carries {}={} but the schema-3 projection places it at {at_ms}",
                    SCHEMA_3_TIMESTAMP_KEY,
                    crate::json::py_repr(existing)
                ));
            }
        }
        projected.insert(SCHEMA_3_TIMESTAMP_KEY.into(), at_ms.into());
    }
    Ok(Value::Object(projected))
}

fn field_int(record: &Value, field: &str, where_: &str) -> Result<i64, String> {
    let obj = as_object(record, where_)?;
    as_int(
        obj.get(field).unwrap_or(&Value::Null),
        &format!("{where_}.{field}"),
    )
}

fn team_of(record: &Value, sole_team: Option<&str>, where_: &str) -> Result<String, String> {
    let obj = as_object(record, where_)?;
    if let Some(raw) = obj.get("team") {
        if !raw.is_null() {
            return Ok(as_string(raw, &format!("{where_}.team"))?.to_string());
        }
    }
    sole_team.map(str::to_string).ok_or_else(|| {
        format!("{where_}: record has no 'team' and the timeline names more than one team")
    })
}

fn collection(timeline: &Value, field: &str) -> Result<Vec<Value>, String> {
    let obj = match timeline {
        Value::Object(map) => map,
        _ => return Err("timeline: expected an object".to_string()),
    };
    match obj.get(field) {
        None => Ok(Vec::new()),
        Some(value) => {
            let items = as_array(value, &format!("timeline.{field}"))?;
            let mut out = Vec::new();
            for (index, item) in items.iter().enumerate() {
                as_object(item, &format!("timeline.{field}[{index}]"))?;
                out.push(item.clone());
            }
            Ok(out)
        }
    }
}

fn output_path(output: &Path, relative: &str, make_parents: bool) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(output).unwrap_or_else(|_| output.to_path_buf());
    let mut cursor = output.to_path_buf();
    let parts: Vec<&str> = relative.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        cursor = cursor.join(part);
        if cursor.symlink_metadata().map(|meta| meta.file_type().is_symlink()).unwrap_or(false) {
            return Err(format!(
                "schema-3 output path crosses a symlink: {}",
                py_repr_str(relative)
            ));
        }
        if make_parents && !cursor.exists() {
            std::fs::create_dir_all(&cursor)
                .map_err(|error| crate::json::os_error(&error, &cursor))?;
        }
    }
    let candidate = output.join(relative);
    if candidate
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refusing to write schema-3 output through a symlink: {}",
            candidate.display()
        ));
    }
    if let Some(parent) = candidate.parent() {
        if let Ok(parent_real) = std::fs::canonicalize(parent) {
            if !parent_real.starts_with(&root) {
                return Err(format!(
                    "schema-3 output path escapes the archive: {}",
                    py_repr_str(relative)
                ));
            }
        }
    }
    Ok(candidate)
}

fn remove_unplanned(output: &Path, published: &BTreeSet<String>) -> Vec<String> {
    let root = output.join(SCHEMA_3_ROOT);
    if root
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(true)
        || !root.is_dir()
    {
        return Vec::new();
    }
    let mut removed: Vec<String> = Vec::new();
    fn walk(
        output: &Path,
        dir: &Path,
        root: &Path,
        published: &BTreeSet<String>,
        removed: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        // Bottom-up: recurse first.
        for path in &entries {
            if path.is_dir()
                && !path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
            {
                walk(output, path, root, published, removed);
            }
        }
        for path in &entries {
            let Some(name) = path.file_name().and_then(|item| item.to_str()) else {
                continue;
            };
            if !is_schema_3_shard_name(name) {
                continue;
            }
            let relative = path
                .strip_prefix(output)
                .map(|item| item.display().to_string().replace('\\', "/"))
                .unwrap_or_default();
            if published.contains(&relative)
                || path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
                || !path.is_file()
            {
                continue;
            }
            if std::fs::remove_file(path).is_ok() {
                removed.push(relative);
            }
        }
        if dir != root {
            let empty = std::fs::read_dir(dir)
                .map(|mut items| items.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
    walk(output, &root, &root, published, &mut removed);
    removed.sort();
    removed
}

struct PlannedShard {
    relative_path: String,
    stream: String,
    team: Option<String>,
    day: Option<String>,
    lines: Vec<Line>,
    line_ranges: BTreeMap<String, (i64, i64)>,
}

fn write_shard(
    output: &Path,
    relative: &str,
    stream: &str,
    team: Option<&str>,
    day: Option<&str>,
    lines: &[Line],
) -> Result<ShardReport, String> {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut end_values: Vec<i64> = Vec::new();
    for item in lines {
        *counts.entry(item.kind.clone()).or_default() += 1;
        if let Some(end) = item.end_ms {
            end_values.push(end);
        }
    }
    let records: Vec<Value> = lines.iter().map(|item| item.record.clone()).collect();
    let write = write_seekable_jsonl(
        &output_path(output, relative, true)?,
        &records,
        SCHEMA_3_TIMESTAMP_KEY,
    )?;
    Ok(ShardReport {
        stream: stream.to_string(),
        team: team.map(str::to_string),
        day: day.map(str::to_string),
        relative_path: relative.to_string(),
        write,
        t_end_exclusive: end_values.into_iter().max(),
        counts,
        line_ranges: BTreeMap::new(),
    })
}

fn timeline_lines(
    timeline: &Value,
    sole_team: Option<&str>,
) -> Result<BTreeMap<(String, i64), Vec<Line>>, String> {
    let mut buckets: BTreeMap<(String, i64), Vec<Line>> = BTreeMap::new();
    let place = |buckets: &mut BTreeMap<(String, i64), Vec<Line>>,
                 team: String,
                 at_ms: i64,
                 item: Line| {
        buckets.entry((team, utc_day_start(at_ms))).or_default().push(item);
    };
    for (index, record) in collection(timeline, "events")?.iter().enumerate() {
        let where_ = format!("timeline.events[{index}]");
        let at_ms = field_int(record, "at_ms", &where_)?;
        let team = team_of(record, sole_team, &where_)?;
        place(
            &mut buckets,
            team,
            at_ms,
            Line {
                at_ms: Some(at_ms),
                kind: "event".into(),
                key: String::new(),
                line: encode(&envelope(record, "event", Some(at_ms), &where_)?)?,
                record: envelope(record, "event", Some(at_ms), &where_)?,
                end_ms: Some(at_ms + 1),
            },
        );
    }
    for (index, record) in collection(timeline, "phases")?.iter().enumerate() {
        let where_ = format!("timeline.phases[{index}]");
        let start_ms = field_int(record, "start_ms", &where_)?;
        let end_ms = field_int(record, "end_ms", &where_)?;
        let team = team_of(record, sole_team, &where_)?;
        place(
            &mut buckets,
            team,
            start_ms,
            Line {
                at_ms: Some(start_ms),
                kind: "phase".into(),
                key: String::new(),
                line: encode(&envelope(record, "phase", Some(start_ms), &where_)?)?,
                record: envelope(record, "phase", Some(start_ms), &where_)?,
                end_ms: Some(end_ms),
            },
        );
    }
    for (index, record) in collection(timeline, "edges")?.iter().enumerate() {
        let where_ = format!("timeline.edges[{index}]");
        let obj = as_object(record, &where_)?;
        if STRUCTURAL_EDGE_KINDS.contains(&obj.get("kind").and_then(Value::as_str).unwrap_or_default()) {
            continue;
        }
        let source_ms = field_int(record, "source_ms", &where_)?;
        let target_ms = field_int(record, "target_ms", &where_)?;
        let at_ms = source_ms.min(target_ms);
        let team = team_of(record, sole_team, &where_)?;
        place(
            &mut buckets,
            team,
            at_ms,
            Line {
                at_ms: Some(at_ms),
                kind: "edge".into(),
                key: String::new(),
                line: encode(&envelope(record, "edge", Some(at_ms), &where_)?)?,
                record: envelope(record, "edge", Some(at_ms), &where_)?,
                end_ms: Some(source_ms.max(target_ms) + 1),
            },
        );
    }
    Ok(buckets)
}

fn minimum_activity_range(
    bounds_start: i64,
    bounds_end: i64,
    intervals: &[(i64, i64)],
    points: &[i64],
) -> (i64, i64) {
    let (mut extent_start, mut extent_end) = (bounds_end, bounds_start);
    for (start, end) in intervals {
        let clipped_start = bounds_start.max(*start);
        let clipped_end = bounds_end.min(*end);
        if clipped_end > clipped_start {
            extent_start = extent_start.min(clipped_start);
            extent_end = extent_end.max(clipped_end);
        }
    }
    let first_point = points.partition_point(|point| *point < bounds_start);
    if first_point < points.len() && points[first_point] < bounds_end {
        let last_point = points.partition_point(|point| *point < bounds_end) - 1;
        extent_start = extent_start.min(points[first_point]);
        extent_end = extent_end.max(bounds_end.min(points[last_point] + 1));
    }
    if extent_end <= extent_start {
        extent_start = bounds_start;
        extent_end = bounds_end;
    }
    let minimum = (bounds_end - bounds_start).min(1000);
    if extent_end - extent_start < minimum {
        let missing = minimum - (extent_end - extent_start).max(0);
        extent_start = bounds_start.max(extent_start - missing / 2);
        extent_end = extent_start + minimum;
        if extent_end > bounds_end {
            extent_end = bounds_end;
            extent_start = extent_end - minimum;
        }
    }
    (extent_start, extent_end)
}

fn record_id(record: &Value, field: &str) -> String {
    match record.get(field) {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

fn phase_intervals(record: &Value, where_: &str) -> Result<Vec<(i64, i64)>, String> {
    let start = field_int(record, "start_ms", where_)?;
    let end = field_int(record, "end_ms", where_)?;
    let obj = as_object(record, where_)?;
    let mut intervals = Vec::new();
    let mut valid_state = false;
    if let Some(states) = obj.get("states") {
        for (index, raw) in as_array(states, &format!("{where_}.states"))?.iter().enumerate() {
            let state_where = format!("{where_}.states[{index}]");
            let state_start = field_int(raw, "start_ms", &state_where)?;
            let state_end = field_int(raw, "end_ms", &state_where)?;
            if state_end <= state_start {
                continue;
            }
            valid_state = true;
            let state = as_object(raw, &state_where)?;
            let keep = match state.get("kind") {
                Some(Value::String(kind)) => kind.to_lowercase() != "idle",
                _ => true,
            };
            if keep {
                intervals.push((state_start, state_end));
            }
        }
    }
    if !valid_state && end > start {
        intervals.push((start, end));
    }
    Ok(intervals)
}

/// Phase/agent/rollup activity bounds.
pub fn activity_bounds(
    timeline: &Value,
) -> Result<
    (
        BTreeMap<String, (i64, i64)>,
        BTreeMap<String, (i64, i64)>,
        Vec<(i64, i64)>,
    ),
    String,
> {
    let mut phase_intervals_map: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    let mut agent_intervals: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    let mut global_intervals: Vec<(i64, i64)> = Vec::new();
    let mut phase_records: Vec<Value> = Vec::new();
    for (index, record) in collection(timeline, "phases")?.iter().enumerate() {
        let where_ = format!("timeline.phases[{index}]");
        let phase_id = record_id(record, "id");
        let agent_id = record_id(record, "agent_id");
        let intervals = phase_intervals(record, &where_)?;
        phase_intervals_map.insert(phase_id.clone(), intervals.clone());
        agent_intervals.entry(agent_id).or_default().extend(intervals.iter().copied());
        global_intervals.extend(intervals);
        phase_records.push(record.clone());
    }
    let mut agent_points: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut global_points: Vec<i64> = Vec::new();
    for (index, record) in collection(timeline, "events")?.iter().enumerate() {
        let where_ = format!("timeline.events[{index}]");
        let at_ms = field_int(record, "at_ms", &where_)?;
        agent_points.entry(record_id(record, "agent_id")).or_default().push(at_ms);
        global_points.push(at_ms);
    }
    for (index, record) in collection(timeline, "edges")?.iter().enumerate() {
        let where_ = format!("timeline.edges[{index}]");
        let source_ms = field_int(record, "source_ms", &where_)?;
        let target_ms = field_int(record, "target_ms", &where_)?;
        agent_points.entry(record_id(record, "source_id")).or_default().push(source_ms);
        agent_points.entry(record_id(record, "target_id")).or_default().push(target_ms);
        global_points.extend([source_ms, target_ms]);
    }
    for points in agent_points.values_mut() {
        points.sort_unstable();
    }
    global_points.sort_unstable();

    let empty: Vec<i64> = Vec::new();
    let mut phase_bounds: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for record in &phase_records {
        let phase_id = record_id(record, "id");
        let agent_id = record_id(record, "agent_id");
        let start = field_int(record, "start_ms", &format!("phase {phase_id}"))?;
        let end = field_int(record, "end_ms", &format!("phase {phase_id}"))?;
        phase_bounds.insert(
            phase_id.clone(),
            minimum_activity_range(
                start,
                end,
                phase_intervals_map.get(&phase_id).map(|v| v.as_slice()).unwrap_or(&[]),
                agent_points.get(&agent_id).unwrap_or(&empty),
            ),
        );
    }
    let mut agent_bounds: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for (index, record) in collection(timeline, "agents")?.iter().enumerate() {
        let where_ = format!("timeline.agents[{index}]");
        let agent_id = record_id(record, "id");
        let start = field_int(record, "start_ms", &where_)?;
        let end = field_int(record, "end_ms", &where_)?;
        agent_bounds.insert(
            agent_id.clone(),
            minimum_activity_range(
                start,
                end,
                agent_intervals.get(&agent_id).map(|v| v.as_slice()).unwrap_or(&[]),
                agent_points.get(&agent_id).unwrap_or(&empty),
            ),
        );
    }
    let mut rollup_bounds: Vec<(i64, i64)> = Vec::new();
    for (index, record) in collection(timeline, "rollups")?.iter().enumerate() {
        let where_ = format!("timeline.rollups[{index}]");
        let start = field_int(record, "start_ms", &where_)?;
        let end = field_int(record, "end_ms", &where_)?;
        rollup_bounds.push(minimum_activity_range(
            start,
            end,
            &global_intervals,
            &global_points,
        ));
    }
    Ok((phase_bounds, agent_bounds, rollup_bounds))
}

fn own_interval(record: &Value, where_: &str) -> Result<(i64, i64), String> {
    Ok((
        field_int(record, "start_ms", where_)?,
        field_int(record, "end_ms", where_)?,
    ))
}

fn bounds_record(reference: &str, bounds: (i64, i64)) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("ref".into(), Value::String(reference.into()));
    out.insert("activity_start_ms".into(), bounds.0.into());
    out.insert("activity_end_ms".into(), bounds.1.into());
    Value::Object(out)
}

fn spine_lines(
    timeline: &Value,
    sole_team: Option<&str>,
) -> Result<BTreeMap<String, BTreeMap<String, Vec<Line>>>, String> {
    let mut buckets: BTreeMap<String, BTreeMap<String, Vec<Line>>> = BTreeMap::new();
    fn sort_field(kind: &str) -> &'static str {
        SPINE_SORT_FIELD
            .iter()
            .find(|(item, _)| *item == kind)
            .map(|(_, field)| *field)
            .unwrap_or("")
    }
    let mut place = |buckets: &mut BTreeMap<String, BTreeMap<String, Vec<Line>>>,
                     team: String,
                     kind: &str,
                     record: &Value,
                     where_: &str|
     -> Result<(), String> {
        let raw = record.get(sort_field(kind));
        let key = match raw {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        let enveloped = envelope(record, kind, None, where_)?;
        buckets
            .entry(team)
            .or_default()
            .entry(kind.to_string())
            .or_default()
            .push(Line {
                at_ms: None,
                kind: kind.to_string(),
                key,
                line: encode(&enveloped)?,
                record: enveloped,
                end_ms: None,
            });
        Ok(())
    };
    for (index, record) in collection(timeline, "teams")?.iter().enumerate() {
        let where_ = format!("timeline.teams[{index}]");
        let obj = as_object(record, &where_)?;
        let team = as_string(obj.get("slug").unwrap_or(&Value::Null), &format!("{where_}.slug"))?.to_string();
        place(&mut buckets, team, "team", record, &where_)?;
    }
    for (index, record) in collection(timeline, "agents")?.iter().enumerate() {
        let where_ = format!("timeline.agents[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "agent", record, &where_)?;
    }
    for (index, record) in collection(timeline, "phases")?.iter().enumerate() {
        let where_ = format!("timeline.phases[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        let obj = as_object(record, &where_)?;
        let mut card = serde_json::Map::new();
        for (key, value) in obj {
            if PHASE_CARD_FIELDS.contains(&key.as_str()) {
                card.insert(key.clone(), value.clone());
            }
        }
        place(&mut buckets, team, "phase_card", &Value::Object(card), &where_)?;
    }
    for (index, record) in collection(timeline, "edges")?.iter().enumerate() {
        let where_ = format!("timeline.edges[{index}]");
        let obj = as_object(record, &where_)?;
        if !STRUCTURAL_EDGE_KINDS.contains(&obj.get("kind").and_then(Value::as_str).unwrap_or_default()) {
            continue;
        }
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "structural_edge", record, &where_)?;
    }
    for (index, record) in collection(timeline, "rollups")?.iter().enumerate() {
        let where_ = format!("timeline.rollups[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "rollup", record, &where_)?;
    }
    for (index, record) in collection(timeline, "projects")?.iter().enumerate() {
        let where_ = format!("timeline.projects[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "project", record, &where_)?;
    }
    for (index, record) in collection(timeline, "summary_files")?.iter().enumerate() {
        let where_ = format!("timeline.summary_files[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "summary_file", record, &where_)?;
    }
    for (index, record) in collection(timeline, "glossary")?.iter().enumerate() {
        let where_ = format!("timeline.glossary[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "glossary_term", record, &where_)?;
    }
    for (index, record) in collection(timeline, "project_overviews")?.iter().enumerate() {
        let where_ = format!("timeline.project_overviews[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        place(&mut buckets, team, "project_overview", record, &where_)?;
    }
    let timeline_obj = as_object(timeline, "timeline")?;
    if let Some(singular) = timeline_obj.get("project_overview") {
        if !singular.is_null() {
            let where_ = "timeline.project_overview";
            as_object(singular, where_)?;
            let team = team_of(singular, sole_team, where_)?;
            place(&mut buckets, team, "project_overview", singular, where_)?;
        }
    }
    let (phase_bounds, agent_bounds, rollup_bounds) = activity_bounds(timeline)?;
    for (index, record) in collection(timeline, "agents")?.iter().enumerate() {
        let where_ = format!("timeline.agents[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        let obj = as_object(record, &where_)?;
        let identifier = as_string(obj.get("id").unwrap_or(&Value::Null), &format!("{where_}.id"))?.to_string();
        let bounds = agent_bounds
            .get(&identifier)
            .copied()
            .map_or_else(|| own_interval(record, &where_), Ok)?;
        place(
            &mut buckets,
            team.clone(),
            ACTIVITY_BOUNDS_KIND,
            &bounds_record(&agent_reference(&team, &identifier), bounds),
            &where_,
        )?;
    }
    for (index, record) in collection(timeline, "phases")?.iter().enumerate() {
        let where_ = format!("timeline.phases[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        let obj = as_object(record, &where_)?;
        let identifier = as_string(obj.get("id").unwrap_or(&Value::Null), &format!("{where_}.id"))?.to_string();
        let bounds = phase_bounds
            .get(&identifier)
            .copied()
            .map_or_else(|| own_interval(record, &where_), Ok)?;
        place(
            &mut buckets,
            team.clone(),
            ACTIVITY_BOUNDS_KIND,
            &bounds_record(&phase_reference(&team, &identifier), bounds),
            &where_,
        )?;
    }
    let rollups = collection(timeline, "rollups")?;
    if rollup_bounds.len() != rollups.len() {
        return Err(
            "timeline.rollups: activity bounds do not line up with the rollups they describe"
                .to_string(),
        );
    }
    for (index, record) in rollups.iter().enumerate() {
        let where_ = format!("timeline.rollups[{index}]");
        let team = team_of(record, sole_team, &where_)?;
        let obj = as_object(record, &where_)?;
        let kind = as_string(obj.get("kind").unwrap_or(&Value::Null), &format!("{where_}.kind"))?.to_string();
        place(
            &mut buckets,
            team.clone(),
            ACTIVITY_BOUNDS_KIND,
            &bounds_record(
                &rollup_reference(&team, &kind, field_int(record, "start_ms", &where_)?),
                rollup_bounds[index],
            ),
            &where_,
        )?;
    }
    Ok(buckets)
}

fn ordered_spine(
    by_kind: &BTreeMap<String, Vec<Line>>,
    where_: &str,
) -> Result<(Vec<Line>, BTreeMap<String, (i64, i64)>), String> {
    let unknown: Vec<&String> = by_kind
        .keys()
        .filter(|kind| !SPINE_KIND_ORDER.contains(&kind.as_str()))
        .collect();
    if !unknown.is_empty() {
        let mut names: Vec<String> = unknown.iter().map(|item| item.to_string()).collect();
        names.sort();
        return Err(format!(
            "{where_}: unclassified spine kinds [{}]",
            names
                .iter()
                .map(|item| py_repr_str(item))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut ordered: Vec<Line> = Vec::new();
    let mut ranges: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for kind in SPINE_KIND_ORDER {
        let Some(group) = by_kind.get(kind) else {
            continue;
        };
        if group.is_empty() {
            continue;
        }
        let mut group = group.clone();
        group.sort_by(|a, b| a.kind_order().cmp(&b.kind_order()));
        ranges.insert(kind.to_string(), (ordered.len() as i64, group.len() as i64));
        ordered.extend(group);
    }
    Ok((ordered, ranges))
}

fn ordered_links(
    by_kind: &BTreeMap<String, Vec<Line>>,
    where_: &str,
) -> Result<(Vec<Line>, BTreeMap<String, (i64, i64)>), String> {
    let unknown: Vec<&String> = by_kind
        .keys()
        .filter(|kind| !SEARCH_LINK_KIND_ORDER.contains(&kind.as_str()))
        .collect();
    if !unknown.is_empty() {
        let mut names: Vec<String> = unknown.iter().map(|item| item.to_string()).collect();
        names.sort();
        return Err(format!(
            "{where_}: unclassified linkage kinds [{}]",
            names
                .iter()
                .map(|item| py_repr_str(item))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut ordered: Vec<Line> = Vec::new();
    let mut ranges: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for kind in SEARCH_LINK_KIND_ORDER {
        let Some(group) = by_kind.get(kind) else {
            continue;
        };
        if group.is_empty() {
            continue;
        }
        let mut group = group.clone();
        group.sort_by(|a, b| a.kind_order().cmp(&b.kind_order()));
        ranges.insert(kind.to_string(), (ordered.len() as i64, group.len() as i64));
        ordered.extend(group);
    }
    Ok((ordered, ranges))
}

fn bins_lines(
    timeline: &Value,
    sole_team: Option<&str>,
) -> Result<Vec<Line>, String> {
    let mut lines = Vec::new();
    for (index, record) in collection(timeline, "activity_bins")?.iter().enumerate() {
        let where_ = format!("timeline.activity_bins[{index}]");
        team_of(record, sole_team, &where_)?;
        let start_ms = field_int(record, "start_ms", &where_)?;
        let end_ms = field_int(record, "end_ms", &where_)?;
        let enveloped = envelope(record, "activity_bin", Some(start_ms), &where_)?;
        lines.push(Line {
            at_ms: Some(start_ms),
            kind: "activity_bin".into(),
            key: String::new(),
            line: encode(&enveloped)?,
            record: enveloped,
            end_ms: Some(end_ms),
        });
    }
    lines.sort_by(|a, b| a.time_order().cmp(&b.time_order()));
    Ok(lines)
}

struct SearchPlan {
    records: BTreeMap<(String, i64), Vec<Line>>,
    texts: BTreeMap<(String, i64), Vec<String>>,
    links: BTreeMap<String, BTreeMap<String, Vec<Line>>>,
}

fn search_string(record: &Value, field: &str, where_: &str) -> Result<String, String> {
    let obj = as_object(record, where_)?;
    Ok(as_string(obj.get(field).unwrap_or(&Value::Null), &format!("{where_}.{field}"))?.to_string())
}

fn link_line(kind: &str, key: &str, record: Value, where_: &str) -> Result<Line, String> {
    let enveloped = envelope(&record, kind, None, where_)?;
    Ok(Line {
        at_ms: None,
        kind: kind.to_string(),
        key: key.to_string(),
        line: encode(&enveloped)?,
        record: enveloped,
        end_ms: None,
    })
}

fn search_lines(
    search_records: &[Value],
    known_slugs: &BTreeSet<String>,
) -> Result<SearchPlan, String> {
    let mut plan = SearchPlan {
        records: BTreeMap::new(),
        texts: BTreeMap::new(),
        links: BTreeMap::new(),
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, record) in search_records.iter().enumerate() {
        let where_ = format!("search_records[{index}]");
        let reference = search_string(record, "ref", &where_)?;
        if !seen.insert(reference.clone()) {
            return Err(format!(
                "{where_}: duplicate search record reference {}",
                py_repr_str(&reference)
            ));
        }
        let team = search_string(record, "team", &where_)?;
        if !known_slugs.contains(&team) {
            return Err(format!(
                "{where_}: search record names team {}, absent from teams[]",
                py_repr_str(&team)
            ));
        }
        let at_ms = field_int(record, "at_ms", &where_)?;
        let text = search_string(record, "text", &where_)?;
        let bucket = (team.clone(), utc_day_start(at_ms));
        let enveloped = envelope(record, SEARCH_RECORD_KIND, Some(at_ms), &where_)?;
        plan.records
            .entry(bucket.clone())
            .or_default()
            .push(Line {
                at_ms: Some(at_ms),
                kind: SEARCH_RECORD_KIND.into(),
                key: reference.clone(),
                line: encode(&enveloped)?,
                record: enveloped,
                end_ms: Some(at_ms + 1),
            });
        plan.texts.entry(bucket).or_default().push(text.clone());

        let record_type = search_string(record, "record_type", &where_)?;
        let by_kind = plan.links.entry(team).or_default();
        if SEARCH_PROMPT_TYPES.contains(&record_type.as_str()) {
            let mut excerpt_obj = serde_json::Map::new();
            excerpt_obj.insert("ref".into(), Value::String(reference.clone()));
            excerpt_obj.insert(
                "excerpt".into(),
                Value::String(
                    compact_search_text(&text)
                        .chars()
                        .take(SEARCH_EXCERPT_CHARACTERS)
                        .collect(),
                ),
            );
            by_kind
                .entry(SEARCH_PROMPT_KIND.to_string())
                .or_default()
                .push(link_line(
                    SEARCH_PROMPT_KIND,
                    &reference,
                    Value::Object(excerpt_obj),
                    &where_,
                )?);
        }
        if !SEARCH_RESPONSE_TYPES.contains(&record_type.as_str()) {
            continue;
        }
        let obj = as_object(record, &where_)?;
        let Some(raw_prompt) = obj.get("prompt_ref") else {
            continue;
        };
        if raw_prompt.is_null() {
            continue;
        }
        let prompt_reference = as_string(raw_prompt, &format!("{where_}.prompt_ref"))?.to_string();
        if prompt_reference == reference {
            continue;
        }
        let mut link_obj = serde_json::Map::new();
        link_obj.insert("ref".into(), Value::String(reference.clone()));
        link_obj.insert("prompt_ref".into(), Value::String(prompt_reference));
        link_obj.insert("at_ms".into(), at_ms.into());
        link_obj.insert(
            "agent_ref".into(),
            Value::String(search_string(record, "agent_ref", &where_)?),
        );
        by_kind
            .entry(SEARCH_RESPONSE_KIND.to_string())
            .or_default()
            .push(link_line(
                SEARCH_RESPONSE_KIND,
                &reference,
                Value::Object(link_obj),
                &where_,
            )?);
    }
    Ok(plan)
}

fn search_bloom_lines(
    plan: &SearchPlan,
    relative_of: &BTreeMap<(String, i64), String>,
) -> Result<BTreeMap<String, Vec<Line>>, String> {
    let mut lines: BTreeMap<String, Vec<Line>> = BTreeMap::new();
    for bucket in plan.records.keys() {
        let (team, day_start) = bucket;
        let bloom = build_trigram_bloom(&plan.texts[bucket]);
        let mut record = serde_json::Map::new();
        record.insert("team".into(), Value::String(team.clone()));
        record.insert("day".into(), Value::String(utc_day_label(*day_start)));
        record.insert(
            "shard".into(),
            Value::String(relative_of[bucket].clone()),
        );
        record.insert("records".into(), (plan.records[bucket].len() as i64).into());
        record.insert("bloom".into(), bloom.catalog_obj());
        let record = Value::Object(record);
        let where_ = format!("search bloom {team} {}", utc_day_label(*day_start));
        let enveloped = envelope(&record, SEARCH_BLOOM_KIND, Some(*day_start), &where_)?;
        lines.entry(team.clone()).or_default().push(Line {
            at_ms: Some(*day_start),
            kind: SEARCH_BLOOM_KIND.into(),
            key: String::new(),
            line: encode(&enveloped)?,
            record: enveloped,
            end_ms: Some(day_start + DAY_MS),
        });
    }
    Ok(lines)
}

fn bootstrap(
    timeline: &Value,
    teams: &[Value],
    shards: &[ShardReport],
    target_chunk_bytes: u64,
) -> Value {
    let timeline_obj = as_object(timeline, "timeline").unwrap();
    let mut root = serde_json::Map::new();
    root.insert("schema_version".into(), SCHEMA_3_VERSION.into());
    root.insert(
        "kind".into(),
        Value::String("timeline-v3-bootstrap".into()),
    );
    for field in BOOTSTRAP_FIELDS {
        if let Some(value) = timeline_obj.get(field) {
            root.insert(field.into(), value.clone());
        }
    }
    let mut codec = serde_json::Map::new();
    codec.insert(
        "container".into(),
        Value::String("multi-member-gzip".into()),
    );
    codec.insert("level".into(), 6.into());
    codec.insert("target_chunk_bytes".into(), target_chunk_bytes.into());
    codec.insert(
        "timestamp_key".into(),
        Value::String(SCHEMA_3_TIMESTAMP_KEY.into()),
    );
    codec.insert(
        "record_kind_key".into(),
        Value::String(SCHEMA_3_RECORD_KIND_KEY.into()),
    );
    codec.insert("index_suffix".into(), Value::String(INDEX_SUFFIX.into()));
    root.insert("codec".into(), Value::Object(codec));
    root.insert("teams".into(), Value::Array(teams.to_vec()));
    let mut by_stream: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for stream in ["timeline", "spine", "bins", "search", "search_bloom", "search_links"] {
        by_stream.insert(stream.to_string(), Vec::new());
    }
    for shard in shards {
        by_stream
            .entry(shard.stream.clone())
            .or_default()
            .push(shard.catalog_obj());
    }
    let strings = |items: &[&str]| -> Value {
        Value::Array(
            items
                .iter()
                .map(|item| Value::String((*item).into()))
                .collect(),
        )
    };
    let stream_obj = |addressing: &str, sort: &str, kinds: &[&str], shards: Vec<Value>| -> Value {
        let mut out = serde_json::Map::new();
        out.insert("addressing".into(), Value::String(addressing.into()));
        out.insert("sort".into(), Value::String(sort.into()));
        out.insert("record_kinds".into(), strings(kinds));
        out.insert("shards".into(), Value::Array(shards));
        Value::Object(out)
    };
    let take = |by_stream: &mut BTreeMap<String, Vec<Value>>, name: &str| -> Vec<Value> {
        by_stream.remove(name).unwrap_or_default()
    };
    let mut streams = serde_json::Map::new();
    streams.insert(
        "timeline".into(),
        stream_obj(
            "per-team-utc-day",
            "at_ms, record_kind, encoded line",
            &["edge", "event", "phase"],
            take(&mut by_stream, "timeline"),
        ),
    );
    streams.insert(
        "spine".into(),
        stream_obj(
            "per-team-line-range",
            "record_kind in declared order, then the kind's identifier",
            &SPINE_KIND_ORDER,
            take(&mut by_stream, "spine"),
        ),
    );
    streams.insert(
        "bins".into(),
        stream_obj(
            "single-shard-time-range",
            "at_ms, record_kind, encoded line",
            &["activity_bin"],
            take(&mut by_stream, "bins"),
        ),
    );
    if !by_stream["search"].is_empty() {
        streams.insert(
            "search".into(),
            stream_obj(
                "per-team-utc-day",
                "at_ms, record_kind, encoded line",
                &[SEARCH_RECORD_KIND],
                take(&mut by_stream, "search"),
            ),
        );
        streams.insert(
            "search_bloom".into(),
            stream_obj(
                "per-team-time-range",
                "at_ms, record_kind, encoded line",
                &[SEARCH_BLOOM_KIND],
                take(&mut by_stream, "search_bloom"),
            ),
        );
        streams.insert(
            "search_links".into(),
            stream_obj(
                "per-team-line-range",
                "record_kind in declared order, then the record's reference",
                &SEARCH_LINK_KIND_ORDER,
                take(&mut by_stream, "search_links"),
            ),
        );
    }
    root.insert("streams".into(), Value::Object(streams));
    Value::Object(root)
}

/// Publish the schema-3 shards, then the bootstrap.
pub fn write_timeline_v3(
    output: &Path,
    raw_timeline: &Value,
    search_records: &[Value],
) -> Result<TimelineV3Report, String> {
    let timeline = raw_timeline.clone();
    let timeline_obj = as_object(&timeline, "timeline")?;
    if as_int(
        timeline_obj.get("schema_version").unwrap_or(&Value::Null),
        "timeline.schema_version",
    )? != 1
    {
        return Err("schema-3 projection requires a schema-1 source timeline".to_string());
    }
    let unknown: Vec<String> = timeline_obj
        .keys()
        .filter(|key| !KNOWN_FIELDS.contains(&key.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "timeline: unclassified schema-1 fields [{}]; schema 3 would drop them silently, so classify each one as bootstrap or streamed before publishing",
            unknown
                .iter()
                .map(|item| py_repr_str(item))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // The single-team bootstrap duplicates these timeline-level copies.
    let teams = collection(&timeline, "teams")?;
    let mut slugs: Vec<String> = Vec::new();
    for (index, team) in teams.iter().enumerate() {
        let obj = as_object(team, &format!("timeline.teams[{index}]"))?;
        slugs.push(safe_slug(
            as_string(obj.get("slug").unwrap_or(&Value::Null), &format!("timeline.teams[{index}].slug"))?,
            "timeline.teams",
        )?);
    }
    let unique: BTreeSet<String> = slugs.iter().cloned().collect();
    if unique.len() != slugs.len() {
        return Err("timeline.teams: duplicate team slug".to_string());
    }
    let sole_team: Option<&str> = if slugs.len() == 1 {
        Some(slugs[0].as_str())
    } else {
        None
    };
    let known_slugs: BTreeSet<String> = slugs.iter().cloned().collect();

    let mut planned: Vec<PlannedShard> = Vec::new();
    let timeline_buckets = timeline_lines(&timeline, sole_team)?;
    for ((team, day_start), lines) in &timeline_buckets {
        if !known_slugs.contains(team) {
            return Err(format!(
                "timeline: record names team {}, absent from teams[]",
                py_repr_str(team)
            ));
        }
        let mut lines = lines.clone();
        lines.sort_by(|a, b| a.time_order().cmp(&b.time_order()));
        let day = utc_day_label(*day_start);
        planned.push(PlannedShard {
            relative_path: format!("{SCHEMA_3_TIMELINE_ROOT}/{team}/{day}.jsonl.gz"),
            stream: "timeline".into(),
            team: Some(team.clone()),
            day: Some(day),
            lines,
            line_ranges: BTreeMap::new(),
        });
    }
    let spine_buckets = spine_lines(&timeline, sole_team)?;
    for (team, by_kind) in &spine_buckets {
        if !known_slugs.contains(team) {
            return Err(format!(
                "timeline: record names team {}, absent from teams[]",
                py_repr_str(team)
            ));
        }
        let (ordered, ranges) = ordered_spine(by_kind, &format!("spine {team}"))?;
        planned.push(PlannedShard {
            relative_path: format!("{SCHEMA_3_SPINE_ROOT}/{team}.jsonl.gz"),
            stream: "spine".into(),
            team: Some(team.clone()),
            day: None,
            lines: ordered,
            line_ranges: ranges,
        });
    }
    planned.push(PlannedShard {
        relative_path: SCHEMA_3_BINS_PATH.into(),
        stream: "bins".into(),
        team: None,
        day: None,
        lines: bins_lines(&timeline, sole_team)?,
        line_ranges: BTreeMap::new(),
    });
    let search_plan = search_lines(search_records, &known_slugs)?;
    let mut search_paths: BTreeMap<(String, i64), String> = BTreeMap::new();
    for (bucket, lines) in &search_plan.records {
        let (team, day_start) = bucket;
        let day = utc_day_label(*day_start);
        let relative = format!("{SCHEMA_3_SEARCH_ROOT}/{team}/{day}.jsonl.gz");
        search_paths.insert(bucket.clone(), relative.clone());
        let mut lines = lines.clone();
        lines.sort_by(|a, b| a.search_order().cmp(&b.search_order()));
        planned.push(PlannedShard {
            relative_path: relative,
            stream: "search".into(),
            team: Some(team.clone()),
            day: Some(day),
            lines,
            line_ranges: BTreeMap::new(),
        });
    }
    if !search_plan.records.is_empty() {
        let bloom_lines = search_bloom_lines(&search_plan, &search_paths)?;
        for (team, lines) in &bloom_lines {
            let mut lines = lines.clone();
            lines.sort_by(|a, b| a.time_order().cmp(&b.time_order()));
            planned.push(PlannedShard {
                relative_path: format!("{SCHEMA_3_SEARCH_BLOOM_ROOT}/{team}.jsonl.gz"),
                stream: "search_bloom".into(),
                team: Some(team.clone()),
                day: None,
                lines,
                line_ranges: BTreeMap::new(),
            });
        }
        let search_teams: BTreeSet<String> = search_plan
            .records
            .keys()
            .map(|(team, _)| team.clone())
            .collect();
        for team in &search_teams {
            let empty: BTreeMap<String, Vec<Line>> = BTreeMap::new();
            let (ordered, ranges) = ordered_links(
                search_plan.links.get(team).unwrap_or(&empty),
                &format!("search links {team}"),
            )?;
            planned.push(PlannedShard {
                relative_path: format!("{SCHEMA_3_SEARCH_LINKS_ROOT}/{team}.jsonl.gz"),
                stream: "search_links".into(),
                team: Some(team.clone()),
                day: None,
                lines: ordered,
                line_ranges: ranges,
            });
        }
    }
    for item in &planned {
        output_path(output, &item.relative_path, false)?;
    }
    output_path(output, SCHEMA_3_BOOTSTRAP_PATH, false)?;

    let mut shards: Vec<ShardReport> = Vec::new();
    let mut changed = 0i64;
    let mut record_count = 0u64;
    for item in &planned {
        let mut shard = write_shard(
            output,
            &item.relative_path,
            &item.stream,
            item.team.as_deref(),
            item.day.as_deref(),
            &item.lines,
        )?;
        shard.line_ranges = item.line_ranges.clone();
        changed += i64::from(shard.write.data_changed) + i64::from(shard.write.index_changed);
        record_count += shard.write.index.record_count;
        shards.push(shard);
    }
    let bootstrap_value = bootstrap(&timeline, &teams, &shards, TARGET_CHUNK_BYTES);
    let bootstrap_text = dump_canonical(&bootstrap_value);
    if write_text_if_changed(
        &output_path(output, SCHEMA_3_BOOTSTRAP_PATH, true)?,
        &bootstrap_text,
    )? {
        changed += 1;
    }
    let mut generated: Vec<String> = vec![SCHEMA_3_BOOTSTRAP_PATH.to_string()];
    for shard in &shards {
        generated.extend(shard.generated_files());
    }
    let published: BTreeSet<String> = generated
        .iter()
        .filter(|item| *item != SCHEMA_3_BOOTSTRAP_PATH)
        .cloned()
        .collect();
    let removed = remove_unplanned(output, &published);
    changed += removed.len() as i64;
    let index_bytes: u64 = shards
        .iter()
        .map(|shard| {
            std::fs::metadata(output.join(shard.index_relative_path()))
                .map(|meta| meta.len())
                .unwrap_or(0)
        })
        .sum();
    generated.sort();
    Ok(TimelineV3Report {
        files_changed: changed,
        generated_files: generated,
        timeline_shards: shards.iter().filter(|item| item.stream == "timeline").count() as i64,
        spine_shards: shards.iter().filter(|item| item.stream == "spine").count() as i64,
        search_shards: shards.iter().filter(|item| item.stream == "search").count() as i64,
        search_records: shards
            .iter()
            .filter(|item| item.stream == "search")
            .map(|item| item.write.index.record_count)
            .sum::<u64>() as i64,
        search_bytes: shards
            .iter()
            .filter(|item| SCHEMA_3_SEARCH_STREAMS.contains(&item.stream.as_str()))
            .map(|item| item.write.index.c_size)
            .sum(),
        record_count,
        bootstrap_bytes: bootstrap_text.len(),
        compressed_bytes: shards.iter().map(|item| item.write.index.c_size).sum(),
        uncompressed_bytes: shards.iter().map(|item| item.write.index.u_size).sum(),
        index_bytes,
        removed_files: removed,
    })
}
