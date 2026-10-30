//! `wrkviz inspect`: archive/run/source counts as JSON, and the
//! schema-3 generation reader beneath it.
//!
//! The schema-3 bootstrap and shard catalogue are defined here
//! once, as the milestone's source of truth for the archive
//! record format; the acceptance rules are the Python
//! `_SchemaThreeArchive` rules, sentence for sentence.

use crate::json::{
    as_array, as_int, as_object, as_string, dump_pretty, optional_int, py_repr, py_repr_str,
    read_json, resolve_path,
};
use crate::snapshot_store::pointer_summary;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const BOOTSTRAP_PATH: &str = "data/timeline-v3.json";
const SCHEMA_ROOT: &str = "data/timeline-v3";
const STREAMS: [&str; 3] = ["timeline", "spine", "bins"];
const SEARCH_STREAMS: [&str; 3] = ["search", "search_bloom", "search_links"];

/// One catalogue entry from the schema-3 bootstrap. This
/// record type is the milestone's single source of truth for
/// the shard catalogue; the Python mirror validates against it.
#[derive(Clone)]
#[allow(dead_code)]
struct ShardEntry {
    stream: String,
    team: Option<String>,
    day: Option<String>,
    path: String,
    index_path: String,
    records: i64,
    c_bytes: i64,
    line_ranges: BTreeMap<String, (i64, i64)>,
}

/// A declined generation (fall back), as opposed to an
/// unreadable bootstrap (also fall back, with the reason kept).
enum OpenError {
    Declined(String),
    Unreadable(String),
}

fn declined(message: String) -> OpenError {
    OpenError::Declined(message)
}

fn unreadable(message: String) -> OpenError {
    OpenError::Unreadable(message)
}

fn line_ranges(raw: &Value, where_: &str) -> Result<BTreeMap<String, (i64, i64)>, OpenError> {
    if raw.is_null() {
        return Ok(BTreeMap::new());
    }
    let mut ranges = BTreeMap::new();
    let obj = as_object(raw, where_).map_err(unreadable)?;
    for (kind, value) in obj {
        let pair_where = format!("{where_}.{kind}");
        let pair = as_array(value, &pair_where).map_err(unreadable)?;
        if pair.len() != 2 {
            return Err(declined(format!("{pair_where}: expected [first, count]")));
        }
        let first = as_int(&pair[0], &format!("{pair_where}[0]")).map_err(unreadable)?;
        let count = as_int(&pair[1], &format!("{pair_where}[1]")).map_err(unreadable)?;
        if first < 0 || count < 0 {
            return Err(declined(format!("{pair_where}: negative line range")));
        }
        ranges.insert(kind.clone(), (first, count));
    }
    Ok(ranges)
}

fn shard(raw: &Value, stream: &str, where_: &str) -> Result<ShardEntry, OpenError> {
    let entry = as_object(raw, where_).map_err(unreadable)?;
    let get = |key: &str| entry.get(key).unwrap_or(&Value::Null);
    let team = if get("team").is_null() {
        None
    } else {
        Some(
            as_string(get("team"), &format!("{where_}.team"))
                .map_err(unreadable)?
                .to_string(),
        )
    };
    let day = if get("day").is_null() {
        None
    } else {
        Some(
            as_string(get("day"), &format!("{where_}.day"))
                .map_err(unreadable)?
                .to_string(),
        )
    };
    let path = as_string(get("path"), &format!("{where_}.path"))
        .map_err(unreadable)?
        .to_string();
    let index_path = as_string(get("index_path"), &format!("{where_}.index_path"))
        .map_err(unreadable)?
        .to_string();
    let records = as_int(get("records"), &format!("{where_}.records")).map_err(unreadable)?;
    let c_bytes = as_int(get("c_bytes"), &format!("{where_}.c_bytes")).map_err(unreadable)?;
    // Validate the optional ints exactly as the Python entry
    // constructor does; the values themselves are not needed for
    // counts.
    for key in ["t0", "t1", "t_end_exclusive"] {
        optional_int(get(key), &format!("{where_}.{key}")).map_err(unreadable)?;
    }
    Ok(ShardEntry {
        stream: stream.to_string(),
        team,
        day,
        path,
        index_path,
        records,
        c_bytes,
        line_ranges: line_ranges(get("line_ranges"), &format!("{where_}.line_ranges"))?,
    })
}

/// Resolve an archive-relative schema-3 path to an openable
/// regular file, or None (symlinked components or leaf, or
/// missing): `_schema_3_file`.
fn schema_file(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.starts_with('/') {
        return None;
    }
    let mut cursor = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        cursor.push(part);
        if cursor.is_symlink() {
            return None;
        }
    }
    if cursor.is_file() {
        Some(cursor)
    } else {
        None
    }
}

fn check_present(root: &Path, entry: &ShardEntry) -> Result<(), OpenError> {
    for (relative, label) in [(&entry.path, "shard"), (&entry.index_path, "sidecar")] {
        if !relative.starts_with(&format!("{SCHEMA_ROOT}/")) {
            return Err(declined(format!(
                "schema-3 {label} path is outside {SCHEMA_ROOT}/: {}",
                py_repr_str(relative)
            )));
        }
        if schema_file(root, relative).is_none() {
            return Err(declined(format!("schema-3 {label} is missing: {relative}")));
        }
    }
    let size = schema_file(root, &entry.path)
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len() as i64)
        .unwrap_or(-1);
    if size != entry.c_bytes {
        return Err(declined(format!(
            "schema-3 shard {} is {size} bytes; the bootstrap says {} -- the generation is partly published",
            entry.path, entry.c_bytes
        )));
    }
    Ok(())
}

fn check_codec(bootstrap: &Map<String, Value>) -> Result<(), OpenError> {
    let codec = as_object(
        bootstrap.get("codec").unwrap_or(&Value::Null),
        "timeline-v3.codec",
    )
    .map_err(unreadable)?;
    for (key, expected) in [
        ("container", "multi-member-gzip"),
        ("timestamp_key", "at_ms"),
        ("record_kind_key", "record_kind"),
        ("index_suffix", ".index.jsonl"),
    ] {
        let found = codec.get(key).unwrap_or(&Value::Null);
        if found.as_str() != Some(expected) {
            return Err(declined(format!(
                "timeline-v3.codec.{key} is {}; this reader implements {}",
                py_repr(found),
                py_repr_str(expected)
            )));
        }
    }
    Ok(())
}

fn catalog(root: &Path, bootstrap: &Map<String, Value>) -> Result<Vec<ShardEntry>, OpenError> {
    let streams = as_object(
        bootstrap.get("streams").unwrap_or(&Value::Null),
        "timeline-v3.streams",
    )
    .map_err(unreadable)?;
    let missing: Vec<&str> = STREAMS
        .iter()
        .filter(|name| !streams.contains_key(**name))
        .copied()
        .collect();
    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|name| py_repr_str(name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(declined(format!("timeline-v3.streams is missing [{list}]")));
    }
    let present: Vec<&str> = SEARCH_STREAMS
        .iter()
        .filter(|name| streams.contains_key(**name))
        .copied()
        .collect();
    if !present.is_empty() && present.len() != SEARCH_STREAMS.len() {
        let list = present
            .iter()
            .map(|name| py_repr_str(name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(declined(format!(
            "timeline-v3.streams carries part of the transcript search corpus ([{list}]) and not the rest; a corpus without its prefilter or its relationship sidecar answers searches wrongly rather than slowly"
        )));
    }
    let mut shards = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for stream in STREAMS.iter().chain(present.iter()) {
        let where_ = format!("timeline-v3.streams.{stream}");
        let section =
            as_object(streams.get(*stream).unwrap_or(&Value::Null), &where_).map_err(unreadable)?;
        let raw_shards = as_array(
            section.get("shards").unwrap_or(&Value::Null),
            &format!("{where_}.shards"),
        )
        .map_err(unreadable)?;
        for (index, raw) in raw_shards.iter().enumerate() {
            let entry = shard(raw, stream, &format!("{where_}.shards[{index}]"))?;
            if !seen.insert(entry.path.clone()) {
                return Err(declined(format!(
                    "duplicate schema-3 shard {}",
                    py_repr_str(&entry.path)
                )));
            }
            check_present(root, &entry)?;
            shards.push(entry);
        }
    }
    Ok(shards)
}

fn declared_teams(bootstrap: &Map<String, Value>) -> Result<BTreeSet<String>, OpenError> {
    let teams = as_array(
        bootstrap.get("teams").unwrap_or(&Value::Null),
        "timeline-v3.teams",
    )
    .map_err(unreadable)?;
    let mut slugs = BTreeSet::new();
    for (index, raw) in teams.iter().enumerate() {
        let team = as_object(raw, &format!("timeline-v3.teams[{index}]")).map_err(unreadable)?;
        slugs.insert(
            as_string(
                team.get("slug").unwrap_or(&Value::Null),
                &format!("timeline-v3.teams[{index}].slug"),
            )
            .map_err(unreadable)?
            .to_string(),
        );
    }
    if slugs.is_empty() {
        return Err(declined(
            "timeline-v3.teams: must name at least one team".to_string(),
        ));
    }
    Ok(slugs)
}

fn check_search_corpus(shards: &[ShardEntry]) -> Result<(), OpenError> {
    for entry in shards {
        if !SEARCH_STREAMS.contains(&entry.stream.as_str()) {
            continue;
        }
        if entry.team.is_none() || (entry.stream == "search" && entry.day.is_none()) {
            return Err(declined(format!(
                "transcript search shard {} does not name the team and day it is addressed by",
                entry.path
            )));
        }
    }
    let teams: BTreeSet<&str> = shards
        .iter()
        .filter(|entry| entry.stream == "search")
        .filter_map(|entry| entry.team.as_deref())
        .collect();
    if teams.is_empty() {
        return Ok(());
    }
    for stream in ["search_bloom", "search_links"] {
        let published: BTreeSet<&str> = shards
            .iter()
            .filter(|entry| entry.stream == stream)
            .filter_map(|entry| entry.team.as_deref())
            .collect();
        let missing: Vec<&str> = teams.difference(&published).copied().collect();
        if !missing.is_empty() {
            let list = missing
                .iter()
                .map(|name| py_repr_str(name))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(declined(format!(
                "the transcript search corpus has no {stream} shard for [{list}]"
            )));
        }
    }
    Ok(())
}

fn is_shard_name(name: &str) -> bool {
    name.ends_with(".jsonl.gz") || name.ends_with(".jsonl.gz.index.jsonl")
}

fn schema_tree(root: &Path) -> BTreeSet<String> {
    let start = root.join(SCHEMA_ROOT);
    let mut found = BTreeSet::new();
    if start.is_symlink() || !start.is_dir() {
        return found;
    }
    let mut pending = vec![start];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_shard_name(&name) {
                    if let Ok(relative) = path.strip_prefix(root) {
                        found.insert(relative.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
    }
    found
}

fn check_nothing_unnamed(root: &Path, shards: &[ShardEntry]) -> Result<(), OpenError> {
    let mut named: BTreeSet<String> = BTreeSet::new();
    for entry in shards {
        named.insert(entry.path.clone());
        named.insert(entry.index_path.clone());
    }
    let stray: Vec<String> = schema_tree(root).difference(&named).cloned().collect();
    if stray.is_empty() {
        return Ok(());
    }
    let shown = stray.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let shown = if stray.len() > 3 {
        format!("{shown}, ...")
    } else {
        shown
    };
    Err(declined(format!(
        "{} schema-3 file(s) on disk are named by no entry in {BOOTSTRAP_PATH} ({shown}) -- a build published shards and did not reach its bootstrap, so this catalogue is older than the tree it describes",
        stray.len()
    )))
}

/// An accepted schema-3 generation: its bootstrap and catalogue.
struct SchemaThree {
    bootstrap: Map<String, Value>,
    shards: Vec<ShardEntry>,
}

fn open_schema_3(root: &Path) -> Result<SchemaThree, String> {
    match open_inner(root) {
        Ok(opened) => Ok(opened),
        Err(OpenError::Declined(message)) => Err(message),
        Err(OpenError::Unreadable(message)) => {
            Err(format!("schema-3 bootstrap is unreadable: {message}"))
        }
    }
}

fn open_inner(root: &Path) -> Result<SchemaThree, OpenError> {
    let path = root.join(BOOTSTRAP_PATH);
    if path.is_symlink() || !path.is_file() {
        return Err(declined("no schema-3 bootstrap".to_string()));
    }
    let encoded =
        std::fs::read(&path).map_err(|error| unreadable(crate::json::os_error(&error, &path)))?;
    let text = String::from_utf8(encoded).map_err(|error| unreadable(error.to_string()))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| unreadable(error.to_string()))?;
    let bootstrap = as_object(&value, &path.display().to_string())
        .map_err(unreadable)?
        .clone();
    let version = bootstrap.get("schema_version").unwrap_or(&Value::Null);
    let version_ok =
        version.as_i64() == Some(3) || version.as_f64().is_some_and(|number| number == 3.0);
    if !version_ok || bootstrap.get("kind").and_then(Value::as_str) != Some("timeline-v3-bootstrap")
    {
        return Err(declined(format!(
            "{BOOTSTRAP_PATH} is not a schema-3 bootstrap"
        )));
    }
    check_codec(&bootstrap)?;
    let shards = catalog(root, &bootstrap)?;
    let declared = declared_teams(&bootstrap)?;
    let published: BTreeSet<Option<String>> = shards
        .iter()
        .filter(|entry| entry.stream == "spine")
        .map(|entry| entry.team.clone())
        .collect();
    let declared_set: BTreeSet<Option<String>> =
        declared.iter().map(|slug| Some(slug.clone())).collect();
    if declared_set != published {
        let missing: Vec<&str> = declared
            .iter()
            .filter(|slug| !published.contains(&Some((*slug).clone())))
            .map(String::as_str)
            .collect();
        let extra: Vec<&str> = published
            .iter()
            .filter_map(|team| team.as_deref())
            .filter(|slug| !declared.contains(*slug))
            .collect();
        let render = |items: Vec<&str>| {
            items
                .iter()
                .map(|name| py_repr_str(name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(declined(format!(
            "schema-3 spine shards do not cover the published teams (missing [{}], unexpected [{}])",
            render(missing),
            render(extra)
        )));
    }
    check_search_corpus(&shards)?;
    check_nothing_unnamed(root, &shards)?;
    Ok(SchemaThree { bootstrap, shards })
}

/// Whether `root` holds a schema-3 generation complete enough
/// to read, and why not if not.
pub fn schema_3_completeness(root: &Path) -> (bool, String) {
    match open_schema_3(root) {
        Ok(_) => (true, String::new()),
        Err(reason) => (false, reason),
    }
}

const COUNT_SOURCES: [(&str, &[(&str, &str)]); 6] = [
    ("agents", &[("spine", "agent")]),
    ("phases", &[("timeline", "phase")]),
    (
        "edges",
        &[("timeline", "edge"), ("spine", "structural_edge")],
    ),
    ("events", &[("timeline", "event")]),
    ("rollups", &[("spine", "rollup")]),
    ("summary_files", &[("spine", "summary_file")]),
];

/// How many records of each schema-1 collection a complete
/// schema-3 generation holds; `None` when there is none.
pub fn schema_3_record_counts(root: &Path) -> Result<Option<Map<String, Value>>, String> {
    let opened = match open_schema_3(root) {
        Ok(opened) => opened,
        Err(_) => return Ok(None),
    };
    let mut tallies: BTreeMap<(String, String), i64> = BTreeMap::new();
    let streams = as_object(
        opened.bootstrap.get("streams").unwrap_or(&Value::Null),
        "timeline-v3.streams",
    )?;
    for (stream, section) in streams {
        let where_ = format!("timeline-v3.streams.{stream}");
        let section = as_object(section, &where_)?;
        let shards = as_array(
            section.get("shards").unwrap_or(&Value::Null),
            &format!("{where_}.shards"),
        )?;
        for (index, raw) in shards.iter().enumerate() {
            let entry = as_object(raw, &format!("{where_}.shards[{index}]"))?;
            if let Some(counts) = entry.get("counts") {
                if !counts.is_null() {
                    let counts = as_object(counts, &format!("{where_}.shards[{index}].counts"))?;
                    for (kind, value) in counts {
                        let total = tallies.entry((stream.clone(), kind.clone())).or_insert(0);
                        *total +=
                            as_int(value, &format!("{where_}.shards[{index}].counts.{kind}"))?;
                    }
                }
            }
        }
    }
    // Add the spine line-range counts from the validated
    // catalogue (keyed by shard path).
    for entry in &opened.shards {
        for (kind, (_first, count)) in &entry.line_ranges {
            *tallies
                .entry((entry.stream.clone(), kind.clone()))
                .or_insert(0) += *count;
        }
    }
    let mut counts = Map::new();
    for (name, sources) in COUNT_SOURCES {
        let total: i64 = sources
            .iter()
            .map(|(stream, kind)| {
                tallies
                    .get(&(stream.to_string(), kind.to_string()))
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        counts.insert(name.to_string(), json!(total));
    }
    Ok(Some(counts))
}

fn inspect_counts(archive: &Path) -> Result<Map<String, Value>, String> {
    if let Some(counts) = schema_3_record_counts(archive)? {
        return Ok(counts);
    }
    let timeline_path = archive.join("data").join("timeline.json");
    if !timeline_path.is_file() {
        let (_complete, declined) = schema_3_completeness(archive);
        return Err(format!(
            "no built timeline in {}: {declined}, and no {}",
            archive.display(),
            timeline_path.display()
        ));
    }
    let timeline = read_json(&timeline_path)?;
    let timeline = as_object(&timeline, &timeline_path.display().to_string())?;
    let mut counts = Map::new();
    for name in [
        "agents",
        "phases",
        "edges",
        "events",
        "rollups",
        "summary_files",
    ] {
        let items = as_array(
            timeline.get(name).unwrap_or(&Value::Null),
            &format!("timeline.{name}"),
        )?;
        counts.insert(name.to_string(), json!(items.len() as i64));
    }
    Ok(counts)
}

/// `wrkviz inspect --output <archive>`: counts as JSON. The
/// manifest is read first, as the Python `_inspect` reads it.
pub fn inspect(archive: &Path) -> Result<String, String> {
    let manifest_path = archive.join("manifest.json");
    let manifest: Value = if manifest_path.is_file() {
        let value = read_json(&manifest_path)?;
        Value::Object(as_object(&value, &manifest_path.display().to_string())?.clone())
    } else {
        Value::Object(Map::new())
    };
    let mut result = inspect_counts(archive)?;
    result.insert(
        "archive".to_string(),
        Value::String(resolve_path(archive).display().to_string()),
    );
    result.insert("manifest".to_string(), manifest);
    result.insert("source_snapshots".to_string(), pointer_summary(archive)?);
    Ok(dump_pretty(&Value::Object(result)))
}
