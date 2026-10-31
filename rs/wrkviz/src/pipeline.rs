//! Archive transactions at parity with `py/wrkviz/pipeline.py`:
//! the writer lock, archive marker handling, archived-team
//! loading, and run provenance.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::Digest as _;

use crate::build_store::{team_build_root, validate_team_slug};
use crate::json::{as_object, os_error, py_repr_str, read_json};
use crate::team_data::{task_note_from_json, task_note_key, team_from_json, TaskNote, TeamData};
use crate::transcript_io::{read_jsonl, write_json_if_changed};

const ARCHIVE_MARKER_FILE: &str = ".wrkviz.json";
const LEGACY_ARCHIVE_MARKER_FILE: &str = ".agent-team-timeline.json";
const ARCHIVE_LOCK: &str = ".wrkviz.lock";
const LEGACY_ARCHIVE_LOCK: &str = ".agent-team-timeline.lock";

/// The current UTC time in ISO 8601 form.
pub fn utc_now() -> String {
    let now = chrono::Utc::now();
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    }
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| os_error(&error, path))?;
    Ok(sha2::Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Validate an archive identifier (shared with ingest).
pub fn validate_archive_id_pub(value: &str, label: &str) -> Result<(), String> {
    validate_archive_id(value, label)
}

fn validate_archive_id(value: &str, label: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{label} is not a safe archive identifier: {}",
            py_repr_str(value)
        ))
    }
}

fn archive_marker_path(archive: &Path) -> PathBuf {
    let current = archive.join(ARCHIVE_MARKER_FILE);
    if current.is_file() {
        return current;
    }
    let legacy = archive.join(LEGACY_ARCHIVE_MARKER_FILE);
    if legacy.is_file() {
        legacy
    } else {
        current
    }
}

fn is_archive_marker(marker: &serde_json::Map<String, Value>) -> bool {
    matches!(
        marker.get("tool").and_then(Value::as_str),
        Some("wrkviz") | Some("agent-team-timeline")
    ) && crate::json::json_eq_one(marker.get("schema_version"))
}

fn marker_value() -> Value {
    let mut marker = serde_json::Map::new();
    marker.insert("schema_version".into(), 1.into());
    marker.insert("tool".into(), Value::String("wrkviz".into()));
    Value::Object(marker)
}

/// Ensure the archive marker (shared with ingest).
pub fn ensure_archive_pub(archive: &Path, team_slug: &str, create: bool) -> Result<(), String> {
    ensure_archive(archive, team_slug, create)
}

fn ensure_archive(archive: &Path, team_slug: &str, create: bool) -> Result<(), String> {
    validate_team_slug(team_slug)?;
    let marker_path = archive_marker_path(archive);
    if marker_path.is_file() {
        let marker = read_json(&marker_path)?;
        let marker = as_object(&marker, &marker_path.display().to_string())?;
        if !is_archive_marker(marker) {
            return Err(format!(
                "invalid wrkviz archive marker at {}",
                marker_path.display()
            ));
        }
        if marker_path
            .file_name()
            .map(|name| name == LEGACY_ARCHIVE_MARKER_FILE)
            == Some(true)
        {
            write_json_if_changed(&archive.join(ARCHIVE_MARKER_FILE), &marker_value())?;
            std::fs::remove_file(&marker_path).map_err(|error| os_error(&error, &marker_path))?;
            let legacy_lock = archive.join(LEGACY_ARCHIVE_LOCK);
            if legacy_lock.is_file() {
                std::fs::remove_file(&legacy_lock)
                    .map_err(|error| os_error(&error, &legacy_lock))?;
            }
        }
        return Ok(());
    }
    let legacy_raw = team_build_root(archive, team_slug)?
        .join("raw")
        .join("team.json");
    if legacy_raw.is_file() {
        write_json_if_changed(&archive.join(ARCHIVE_MARKER_FILE), &marker_value())?;
        return Ok(());
    }
    if archive.exists() {
        if !archive.is_dir() {
            return Err(format!(
                "archive output is not a directory: {}",
                archive.display()
            ));
        }
        let mut entries = std::fs::read_dir(archive)
            .map_err(|error| os_error(&error, archive))?
            .flatten();
        if entries.any(|entry| entry.file_name() != ARCHIVE_LOCK) {
            return Err(format!(
                "refusing non-empty non-archive output directory {}; choose a new directory",
                archive.display()
            ));
        }
    }
    if !create {
        return Err(format!("not a wrkviz archive: {}", archive.display()));
    }
    write_json_if_changed(&archive.join(ARCHIVE_MARKER_FILE), &marker_value())?;
    Ok(())
}

/// Serialize raw archive transactions across processes (flock).
pub struct WriterLock {
    _file: std::fs::File,
}

/// Take the archive writer lock.
pub fn archive_writer_lock(archive: &Path) -> Result<WriterLock, String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    if archive.exists() && !archive.is_dir() {
        return Err(format!(
            "archive output is not a directory: {}",
            archive.display()
        ));
    }
    std::fs::create_dir_all(archive).map_err(|error| os_error(&error, archive))?;
    let lock_path = archive.join(ARCHIVE_LOCK);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(0o400000 | 0o2000000)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot safely open archive writer lock {}: {}",
                lock_path.display(),
                os_error(&error, &lock_path)
            )
        })?;
    fs2::FileExt::lock_exclusive(&file).map_err(|error| os_error(&error, &lock_path))?;
    Ok(WriterLock { _file: file })
}

fn raw_team_path(archive: &Path, team_slug: &str) -> Result<PathBuf, String> {
    Ok(team_build_root(archive, team_slug)?
        .join("raw")
        .join("team.json"))
}

fn task_notes_path(archive: &Path, team_slug: &str) -> Result<PathBuf, String> {
    Ok(team_build_root(archive, team_slug)?
        .join("raw")
        .join("task-notes.jsonl"))
}

/// Load promoted task notes (shared with ingest).
pub fn load_promoted_task_notes(archive: &Path, team_slug: &str) -> Result<Vec<TaskNote>, String> {
    load_promoted_task_notes_inner(archive, team_slug)
}

fn load_promoted_task_notes_inner(
    archive: &Path,
    team_slug: &str,
) -> Result<Vec<TaskNote>, String> {
    let path = task_notes_path(archive, team_slug)?;
    let mut notes = Vec::new();
    let mut previous_key: Option<(String, i64)> = None;
    for (index, record) in read_jsonl(&path)?.iter().enumerate() {
        let note = task_note_from_json(record, &format!("{}:{}", path.display(), index + 1))?;
        let key = task_note_key(&note);
        if let Some(previous) = &previous_key {
            if key <= *previous {
                return Err(format!(
                    "{}:{}: promoted task notes are not strictly ordered by (source_path, note_id)",
                    path.display(),
                    index + 1
                ));
            }
        }
        previous_key = Some(key);
        notes.push(note);
    }
    Ok(notes)
}

/// The normalized-generation marker value (shared with
/// ingest).
pub fn normalized_generation_value_pub(
    archive: &Path,
    team_slug: &str,
    team: &TeamData,
) -> Result<serde_json::Map<String, Value>, String> {
    normalized_generation_value(archive, team_slug, team)
}

fn normalized_generation_value(
    archive: &Path,
    team_slug: &str,
    _team: &TeamData,
) -> Result<serde_json::Map<String, Value>, String> {
    let source_manifest_path = team_build_root(archive, team_slug)?
        .join("raw")
        .join("source-manifest.json");
    let manifest_doc = read_json(&source_manifest_path)?;
    let canonical = crate::json::dump_canonical(&manifest_doc);
    let source_manifest_sha256 = sha2::Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let notes_path = task_notes_path(archive, team_slug)?;
    let task_notes_sha256 = if notes_path.is_file() {
        file_sha256(&notes_path)?
    } else {
        sha2::Sha256::digest(b"")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let mut value = serde_json::Map::new();
    value.insert("schema_version".into(), 1.into());
    value.insert("tool".into(), Value::String("wrkviz".into()));
    value.insert("normalizer_schema_version".into(), 3.into());
    value.insert("provider".into(), Value::String("orc".into()));
    value.insert(
        "source_manifest_sha256".into(),
        Value::String(source_manifest_sha256),
    );
    value.insert(
        "team_sha256".into(),
        Value::String(file_sha256(&raw_team_path(archive, team_slug)?)?),
    );
    let artifact_path = team_build_root(archive, team_slug)?
        .join("raw")
        .join("artifacts.json");
    value.insert(
        "artifact_catalog_sha256".into(),
        Value::String(file_sha256(&artifact_path)?),
    );
    value.insert("task_notes_sha256".into(), Value::String(task_notes_sha256));
    value.insert(
        "source_digest".into(),
        Value::String(crate::team_data::source_digest(_team)),
    );
    Ok(value)
}

fn validate_normalized_generation(
    archive: &Path,
    team_slug: &str,
    team: &TeamData,
) -> Result<(), String> {
    let marker_path = team_build_root(archive, team_slug)?
        .join("raw")
        .join("normalized-generation.json");
    let unsafe_marker = std::fs::symlink_metadata(&marker_path)
        .map(|meta| meta.file_type().is_symlink() || !meta.is_file())
        .unwrap_or(true);
    if unsafe_marker {
        return Err(format!(
            "incomplete Orc normalized generation for {}; rerun ingest",
            py_repr_str(team_slug)
        ));
    }
    let marker = read_json(&marker_path)?;
    let marker = as_object(&marker, &marker_path.display().to_string())?;
    const BASE: [&str; 8] = [
        "schema_version",
        "tool",
        "normalizer_schema_version",
        "provider",
        "source_manifest_sha256",
        "team_sha256",
        "artifact_catalog_sha256",
        "source_digest",
    ];
    let keys: Vec<&str> = marker.keys().map(String::as_str).collect();
    let base_set = keys.len() == BASE.len() && BASE.iter().all(|key| marker.contains_key(*key));
    let extended_set = keys.len() == BASE.len() + 1
        && BASE.iter().all(|key| marker.contains_key(*key))
        && marker.contains_key("task_notes_sha256");
    if !base_set && !extended_set {
        return Err(format!(
            "invalid Orc normalized generation marker at {}",
            marker_path.display()
        ));
    }
    let expected = normalized_generation_value(archive, team_slug, team)?;
    let comparable: serde_json::Map<String, Value> = marker
        .iter()
        .filter(|(key, _)| key.as_str() != "tool")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let comparable_expected: serde_json::Map<String, Value> = expected
        .iter()
        .filter(|(key, _)| key.as_str() != "tool" && marker.contains_key(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let tool_recognised = matches!(
        marker.get("tool").and_then(Value::as_str),
        Some("wrkviz") | Some("agent-team-timeline")
    );
    if comparable != comparable_expected || !tool_recognised {
        return Err(format!(
            "stale or incomplete Orc normalized generation for {}; rerun ingest",
            py_repr_str(team_slug)
        ));
    }
    Ok(())
}

/// One run to record in the archive's provenance.
pub struct RunReceipt<'a> {
    /// Command argv.
    pub command: &'a [String],
    /// Start timestamp.
    pub started_at: &'a str,
    /// Status.
    pub status: &'a str,
    /// Primary team slug.
    pub team_slug: &'a str,
    /// Error text.
    pub error: Option<&'a str>,
    /// All teams the run covered.
    pub team_slugs: &'a [String],
    /// Mechanical payload.
    pub mechanical: Option<Value>,
    /// Ingest report payload.
    pub ingest: Option<Value>,
}

/// Append run provenance as one serialized manifest transaction.
/// Only the mechanical (extract-transcripts) shape is modelled;
/// ingest/summary/build receipts arrive with their commands.
pub fn record_run(archive: &Path, receipt: &RunReceipt<'_>) -> Result<PathBuf, String> {
    let _lock = archive_writer_lock(archive)?;
    record_run_locked(archive, receipt, None)
}

/// Record a run carrying a summarize report payload.
pub fn record_run_summaries(
    archive: &Path,
    receipt: &RunReceipt<'_>,
    summaries: Option<Value>,
) -> Result<PathBuf, String> {
    let _lock = archive_writer_lock(archive)?;
    record_run_locked(archive, receipt, summaries)
}

/// Record a run carrying a build report payload.
pub fn record_run_build(
    archive: &Path,
    receipt: &RunReceipt<'_>,
    build: Option<Value>,
) -> Result<PathBuf, String> {
    let _lock = archive_writer_lock(archive)?;
    record_run_locked_full(archive, receipt, None, build)
}

fn record_run_locked(
    archive: &Path,
    receipt: &RunReceipt<'_>,
    summaries: Option<Value>,
) -> Result<PathBuf, String> {
    record_run_locked_full(archive, receipt, summaries, None)
}

#[allow(clippy::too_many_arguments)]
fn record_run_locked_full(
    archive: &Path,
    receipt: &RunReceipt<'_>,
    summaries: Option<Value>,
    build: Option<Value>,
) -> Result<PathBuf, String> {
    let (command, started_at, status, team_slug, error, team_slugs, mechanical) = (
        receipt.command,
        receipt.started_at,
        receipt.status,
        receipt.team_slug,
        receipt.error,
        receipt.team_slugs,
        receipt.mechanical.clone(),
    );
    let mut recorded: Vec<String> = if team_slugs.is_empty() {
        vec![team_slug.to_string()]
    } else {
        team_slugs.to_vec()
    };
    recorded.sort();
    recorded.dedup();
    if !recorded.iter().any(|slug| slug == team_slug) {
        return Err("primary run team must be present in team_slugs".to_string());
    }
    for slug in &recorded {
        validate_team_slug(slug)?;
    }
    ensure_archive(archive, team_slug, true)?;
    let completed_at = utc_now();
    let stamp = completed_at
        .replace(['-', ':', '.'], "")
        .replace("+0000", "Z");
    let mut material = command.join("\0");
    material.push('\0');
    material.push_str(&completed_at);
    let digest = sha2::Sha256::digest(material.as_bytes());
    let short: String = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let run_id = format!("{stamp}-{short}");
    let mut run_obj = serde_json::Map::new();
    run_obj.insert("schema_version".into(), 1.into());
    run_obj.insert("run_id".into(), Value::String(run_id.clone()));
    run_obj.insert("tool_version".into(), Value::String("0.1.0".into()));
    run_obj.insert("started_at".into(), Value::String(started_at.to_string()));
    run_obj.insert("completed_at".into(), Value::String(completed_at.clone()));
    run_obj.insert("status".into(), Value::String(status.to_string()));
    run_obj.insert("team_slug".into(), Value::String(team_slug.to_string()));
    run_obj.insert(
        "command".into(),
        Value::Array(
            command
                .iter()
                .map(|arg| Value::String(arg.clone()))
                .collect(),
        ),
    );
    run_obj.insert(
        "ingest".into(),
        receipt.ingest.clone().unwrap_or(Value::Null),
    );
    run_obj.insert("summaries".into(), summaries.unwrap_or(Value::Null));
    run_obj.insert(
        "build".into(),
        build.unwrap_or_else(|| Value::Object(serde_json::Map::new())),
    );
    run_obj.insert(
        "error".into(),
        error.map_or(Value::Null, |text| Value::String(text.to_string())),
    );
    if let Some(mechanical) = mechanical {
        run_obj.insert("mechanical".into(), mechanical);
    }
    if recorded.len() > 1 {
        run_obj.insert(
            "team_slugs".into(),
            Value::Array(
                recorded
                    .iter()
                    .map(|slug| Value::String(slug.clone()))
                    .collect(),
            ),
        );
    }
    let path = archive.join("runs").join(format!("{run_id}.json"));
    write_json_if_changed(&path, &Value::Object(run_obj.clone()))?;
    let manifest_path = archive.join("manifest.json");
    let mut created_at = started_at.to_string();
    let mut run_count = 0i64;
    let mut manifest = serde_json::Map::new();
    if manifest_path.is_file() {
        let old = read_json(&manifest_path)?;
        let old = as_object(&old, &manifest_path.display().to_string())?;
        if let Some(value) = old.get("created_at").and_then(Value::as_str) {
            created_at = value.to_string();
        }
        if let Some(value) = old.get("run_count").and_then(Value::as_i64) {
            run_count = value;
        }
        manifest = old.clone();
    }
    let mut teams: Vec<String> = match manifest.get("teams") {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    for slug in &recorded {
        if !teams.contains(slug) {
            teams.push(slug.clone());
        }
    }
    teams.sort();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert("tool_version".into(), Value::String("0.1.0".into()));
    manifest.insert("created_at".into(), Value::String(created_at));
    manifest.insert("last_run_at".into(), Value::String(completed_at.clone()));
    manifest.insert("last_run_id".into(), Value::String(run_id));
    manifest.insert("last_run_status".into(), Value::String(status.to_string()));
    manifest.insert("run_count".into(), (run_count + 1).into());
    manifest.insert(
        "teams".into(),
        Value::Array(
            teams
                .iter()
                .map(|slug| Value::String(slug.clone()))
                .collect(),
        ),
    );
    if let Some(ingest) = &receipt.ingest {
        if let Some(digest) = ingest.get("source_digest").and_then(Value::as_str) {
            manifest.insert(
                "latest_source_digest".into(),
                Value::String(digest.to_string()),
            );
        }
    }
    write_json_if_changed(&manifest_path, &Value::Object(manifest))?;
    Ok(path)
}

/// Load and validate the normalized team snapshot in *archive*.
pub fn load_archived_team(archive: &Path, team_slug: &str) -> Result<TeamData, String> {
    ensure_archive(archive, team_slug, false)?;
    let path = raw_team_path(archive, team_slug)?;
    if !path.is_file() {
        return Err(format!(
            "no ingested team {}; run `wrkviz ingest`",
            py_repr_str(team_slug)
        ));
    }
    let document = read_json(&path)?;
    let mut team = team_from_json(&document)?;
    team.task_notes = load_promoted_task_notes(archive, team_slug)?;
    if team.team_slug != team_slug {
        return Err(format!(
            "archived team slug {} does not match requested {}",
            py_repr_str(&team.team_slug),
            py_repr_str(team_slug)
        ));
    }
    validate_team_slug(&team.team_slug)?;
    validate_archive_id(&team.root_thread_id, "root thread id")?;
    for agent in &team.agents {
        validate_archive_id(&agent.thread_id, "thread id")?;
    }
    let source_manifest_path = team_build_root(archive, team_slug)?
        .join("raw")
        .join("source-manifest.json");
    if source_manifest_path.is_file() {
        let manifest = read_json(&source_manifest_path)?;
        let manifest = as_object(&manifest, &source_manifest_path.display().to_string())?;
        let is_orc = manifest.get("provider").and_then(Value::as_str) == Some("orc")
            && matches!(
                manifest.get("schema_version").and_then(Value::as_i64),
                Some(2..=5)
            );
        if is_orc {
            validate_normalized_generation(archive, team_slug, &team)?;
        }
    }
    Ok(team)
}
