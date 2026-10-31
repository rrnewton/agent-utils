//! The shared ingest writer and the ingest commands built
//! on it: `py/wrkviz/pipeline.py`'s `_write_ingested_team`
//! and friends, at parity.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::build_store::team_build_root;
use crate::history::{
    load_claude_history_team, load_history_source_manifest, snapshot_claude_history,
    HistorySelection,
};
use crate::identity::{merge_site_identity, site_identity_from_json, IdentityOverrides};
use crate::json::{as_object, dump_compact, py_repr_str, read_json};
use crate::payloads::merge_payloads;
use crate::snapshot_store::{
    ensure_snapshot_store, resolve_snapshot_root, write_pointer, SnapshotLocation,
};
use crate::team_data::{
    source_digest, task_note_key, team_to_json, SourceSnapshot, TaskNote, TeamData,
};
use crate::transcript_io::{write_json_if_changed, write_text_if_changed};
use crate::window::{apply_date_window, DateWindow};

/// Counts and source identity produced by one archive ingest.
#[derive(Clone, Debug, Default)]
pub struct IngestReport {
    /// Team slug.
    pub team_slug: String,
    /// Source digest.
    pub source_digest: String,
    /// Source count.
    pub sources: i64,
    /// Source bytes.
    pub source_bytes: i64,
    /// Agent count.
    pub agents: i64,
    /// Event count.
    pub events: i64,
    /// Tool call count.
    pub tool_calls: i64,
    /// Edge count.
    pub edges: i64,
    /// Files changed.
    pub files_changed: i64,
    /// Artifact count.
    pub artifacts: i64,
    /// Project count.
    pub projects: i64,
    /// Retired message projections.
    pub retired_message_projections: i64,
    /// Retired message projection bytes.
    pub retired_message_projection_bytes: i64,
    /// Promoted task notes held.
    pub task_notes: i64,
    /// Newly promoted task notes.
    pub newly_promoted_task_notes: i64,
    /// Notes upstream deleted.
    pub task_notes_upstream_deleted: i64,
    /// Tool payloads held.
    pub tool_payloads: i64,
    /// Tool payload bytes held.
    pub tool_payload_bytes: i64,
    /// Newly stored payloads.
    pub newly_stored_tool_payloads: i64,
    /// Newly stored payload bytes.
    pub newly_stored_tool_payload_bytes: i64,
    /// Pruned payload shards.
    pub pruned_payload_shards: Vec<String>,
    /// Damaged payload shards.
    pub damaged_payload_shards: Vec<String>,
    /// Snapshot root (archive-relative).
    pub snapshot_root: String,
    /// Snapshot layout.
    pub snapshot_root_layout: String,
    /// Whether this run established the layout.
    pub snapshot_root_established: bool,
    /// History prompts outside the pattern.
    pub history_unmatched_prompts: i64,
    /// History prompts covered elsewhere.
    pub history_covered_prompts: i64,
    /// History covered sessions.
    pub history_covered_sessions: i64,
    /// Sources kept from the archive.
    pub upstream_retired_sources: Vec<String>,
    /// Append-prefix overrides accepted this run (Orc).
    pub orc_prefix_overrides: Vec<crate::orc::AppendPrefixOverride>,
}

impl IngestReport {
    /// The run-receipt JSON record.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        out.insert(
            "source_digest".into(),
            Value::String(self.source_digest.clone()),
        );
        out.insert("sources".into(), self.sources.into());
        out.insert("source_bytes".into(), self.source_bytes.into());
        out.insert("agents".into(), self.agents.into());
        out.insert("events".into(), self.events.into());
        out.insert("tool_calls".into(), self.tool_calls.into());
        out.insert("edges".into(), self.edges.into());
        out.insert("files_changed".into(), self.files_changed.into());
        out.insert("artifacts".into(), self.artifacts.into());
        out.insert("projects".into(), self.projects.into());
        out.insert(
            "orc_prefix_overrides".into(),
            Value::Array(
                self.orc_prefix_overrides
                    .iter()
                    .map(crate::orc::AppendPrefixOverride::to_json)
                    .collect(),
            ),
        );
        out.insert(
            "retired_message_projections".into(),
            self.retired_message_projections.into(),
        );
        out.insert(
            "retired_message_projection_bytes".into(),
            self.retired_message_projection_bytes.into(),
        );
        out.insert("task_notes".into(), self.task_notes.into());
        out.insert(
            "newly_promoted_task_notes".into(),
            self.newly_promoted_task_notes.into(),
        );
        out.insert(
            "task_notes_upstream_deleted".into(),
            self.task_notes_upstream_deleted.into(),
        );
        out.insert("tool_payloads".into(), self.tool_payloads.into());
        out.insert("tool_payload_bytes".into(), self.tool_payload_bytes.into());
        out.insert(
            "newly_stored_tool_payloads".into(),
            self.newly_stored_tool_payloads.into(),
        );
        out.insert(
            "newly_stored_tool_payload_bytes".into(),
            self.newly_stored_tool_payload_bytes.into(),
        );
        out.insert(
            "pruned_payload_shards".into(),
            Value::Array(
                self.pruned_payload_shards
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "damaged_payload_shards".into(),
            Value::Array(
                self.damaged_payload_shards
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "snapshot_root".into(),
            Value::String(self.snapshot_root.clone()),
        );
        out.insert(
            "snapshot_root_layout".into(),
            Value::String(self.snapshot_root_layout.clone()),
        );
        out.insert(
            "snapshot_root_established".into(),
            Value::Bool(self.snapshot_root_established),
        );
        out.insert(
            "history_unmatched_prompts".into(),
            self.history_unmatched_prompts.into(),
        );
        out.insert(
            "history_covered_prompts".into(),
            self.history_covered_prompts.into(),
        );
        out.insert(
            "history_covered_sessions".into(),
            self.history_covered_sessions.into(),
        );
        out.insert(
            "upstream_retired_sources".into(),
            Value::Array(
                self.upstream_retired_sources
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

fn build_raw(archive: &Path, team_slug: &str, name: &str) -> Result<PathBuf, String> {
    Ok(team_build_root(archive, team_slug)?.join("raw").join(name))
}

fn ensure_bulk_content_ignored(archive: &Path) -> Result<bool, String> {
    let path = archive.join(".gitignore");
    if path.exists() && !path.is_file() {
        return Err(format!(
            "archive .gitignore is not a file: {}",
            path.display()
        ));
    }
    let required = [
        "/.wrkviz.lock",
        "/teams/*/source_snapshots/",
        "/teams/*/payloads/",
        "/.wrkviz-trash/",
    ];
    let existing = if path.is_file() {
        std::fs::read_to_string(&path).map_err(|error| crate::json::os_error(&error, &path))?
    } else {
        String::new()
    };
    let existing_lines: std::collections::BTreeSet<&str> = existing.lines().collect();
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|line| !existing_lines.contains(line))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut text = existing.clone();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&missing.join("\n"));
    text.push('\n');
    write_text_if_changed(&path, &text)
}

fn archive_team(team: &TeamData) -> (TeamData, Vec<String>) {
    let mut payloads: Vec<String> = Vec::new();
    let mut tools = Vec::new();
    for tool in &team.tool_calls {
        let mut archived = tool.clone();
        if let Some(text) = &tool.input_text {
            payloads.push(text.clone());
            archived.input_payload = Some(crate::payloads::payload_ref(text));
        }
        if let Some(text) = &tool.output_text {
            payloads.push(text.clone());
            archived.output_payload = Some(crate::payloads::payload_ref(text));
        }
        archived.input_text = None;
        archived.output_text = None;
        tools.push(archived);
    }
    let sources: Vec<SourceSnapshot> = team
        .sources
        .iter()
        .map(|source| {
            let mut archived = source.clone();
            archived.working_directory = None;
            archived.repository_url =
                crate::artifacts::canonical_repository_url(source.repository_url.as_deref());
            archived
        })
        .collect();
    let mut out = team.clone();
    out.sources = sources;
    out.tool_calls = tools;
    (out, payloads)
}

fn merge_promoted_task_notes(
    promoted: &[TaskNote],
    observed: &[TaskNote],
    where_: &str,
) -> Result<(Vec<TaskNote>, i64), String> {
    let mut merged: Vec<TaskNote> = promoted.to_vec();
    let mut newly = 0i64;
    for note in observed {
        let key = task_note_key(note);
        match merged.iter_mut().find(|held| task_note_key(held) == key) {
            None => {
                merged.push(note.clone());
                newly += 1;
            }
            Some(prior) => {
                if (
                    prior.task_id.as_str(),
                    prior.content.as_str(),
                    prior.created_at.as_str(),
                ) != (
                    note.task_id.as_str(),
                    note.content.as_str(),
                    note.created_at.as_str(),
                ) {
                    return Err(format!(
                        "{where_}: promoted task note {}#{} disagrees with the current source about its immutable core",
                        prior.source_path, prior.note_id
                    ));
                }
                if prior.upstream_present && !note.upstream_present {
                    prior.upstream_present = false;
                }
            }
        }
    }
    merged.sort_by_key(task_note_key);
    Ok((merged, newly))
}

fn retire_message_projections(archive: &Path, team_slug: &str) -> Result<(i64, i64), String> {
    let root = team_build_root(archive, team_slug)?
        .join("raw")
        .join("messages");
    if root.is_symlink() || !root.is_dir() {
        return Ok((0, 0));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|error| crate::json::os_error(&error, &root))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    let mut files = 0i64;
    let mut freed = 0i64;
    let mut foreign = false;
    for entry in entries {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = name.strip_suffix(".json");
        let shape_ok = stem.is_some_and(|stem| {
            let bytes = stem.as_bytes();
            !bytes.is_empty()
                && bytes.len() <= 128
                && bytes[0].is_ascii_alphanumeric()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
        let is_file = entry.is_file() && !entry.is_symlink();
        if stem.is_none() || entry.is_symlink() || !is_file || !shape_ok {
            foreign = true;
            continue;
        }
        let size = std::fs::metadata(&entry)
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        std::fs::remove_file(&entry).map_err(|error| crate::json::os_error(&error, &entry))?;
        freed += size;
        files += 1;
    }
    if !foreign {
        let _ = std::fs::remove_dir(&root);
    }
    Ok((files, freed))
}

fn record_site_identity(
    archive: &Path,
    team: &TeamData,
    overrides: &IdentityOverrides,
) -> Result<bool, String> {
    let path = build_raw(archive, &team.team_slug, "site-identity.json")?;
    let previous = if path.is_file() {
        let document = read_json(&path)?;
        Some(site_identity_from_json(
            &document,
            &path.display().to_string(),
        )?)
    } else {
        None
    };
    if let Some(previous) = &previous {
        if previous.team_slug != team.team_slug {
            return Err(format!(
                "site identity team {} does not match {}",
                py_repr_str(&previous.team_slug),
                py_repr_str(&team.team_slug)
            ));
        }
    }
    let identity = merge_site_identity(
        &team.team_slug,
        &team.display_timezone,
        &overrides.display_timezone_source,
        &[],
        &[],
        &overrides.projects,
        &overrides.hosts,
        previous.as_ref(),
    )?;
    write_json_if_changed(&path, &identity.to_json())
}

/// Write provider-neutral normalized data after a provider
/// snapshot is durable.
pub fn write_ingested_team(
    archive: &Path,
    team_slug: &str,
    team: &TeamData,
    date_window: Option<&DateWindow>,
    files_changed: i64,
    location: &SnapshotLocation,
) -> Result<(TeamData, IngestReport), String> {
    let mut changed = files_changed;
    let (archived, detached) = archive_team(team);
    let payload_root = team_build_root(archive, team_slug)?.join("payloads");
    let payload_report = merge_payloads(&payload_root, &detached)?;
    changed += payload_report.files_changed;
    let notes_path = build_raw(archive, team_slug, "task-notes.jsonl")?;
    let existing_notes = crate::pipeline::load_promoted_task_notes(archive, team_slug)?;
    let (promoted, newly_promoted) = merge_promoted_task_notes(
        &existing_notes,
        &team.task_notes,
        &notes_path.display().to_string(),
    )?;
    if !promoted.is_empty() {
        let text: String = promoted
            .iter()
            .map(|note| {
                format!(
                    "{}\n",
                    dump_compact(&crate::team_data::task_note_to_json(note))
                )
            })
            .collect();
        if write_text_if_changed(&notes_path, &text)? {
            changed += 1;
        }
    }
    let catalog = crate::artifacts::extract_artifacts(team);
    if write_json_if_changed(
        &build_raw(archive, team_slug, "artifacts.json")?,
        &catalog.to_json(),
    )? {
        changed += 1;
    }
    crate::pipeline::validate_archive_id_pub(&archived.root_thread_id, "root thread id")?;
    for agent in &archived.agents {
        crate::pipeline::validate_archive_id_pub(&agent.thread_id, "thread id")?;
    }
    let mut stored_team = archived.clone();
    stored_team.task_notes = Vec::new();
    if write_json_if_changed(
        &build_raw(archive, team_slug, "team.json")?,
        &team_to_json(&stored_team),
    )? {
        changed += 1;
    }
    let (retired_files, retired_bytes) = retire_message_projections(archive, team_slug)?;
    changed += retired_files;
    let digest = source_digest(team);
    let mut snapshot = Map::new();
    snapshot.insert("provider".into(), Value::String(team.provider.clone()));
    snapshot.insert(
        "root_thread_id".into(),
        Value::String(team.root_thread_id.clone()),
    );
    snapshot.insert("team_slug".into(), Value::String(team.team_slug.clone()));
    snapshot.insert(
        "display_timezone".into(),
        Value::String(team.display_timezone.clone()),
    );
    snapshot.insert(
        "date_window".into(),
        date_window.map_or(Value::Null, DateWindow::to_json),
    );
    snapshot.insert("source_digest".into(), Value::String(digest.clone()));
    snapshot.insert(
        "sources".into(),
        Value::Array(
            archived
                .sources
                .iter()
                .map(crate::team_data::source_to_json)
                .collect(),
        ),
    );
    if write_json_if_changed(
        &build_raw(archive, team_slug, "source-snapshot.json")?,
        &Value::Object(snapshot),
    )? {
        changed += 1;
    }
    let pointer_established = crate::snapshot_store::pointer_is_recorded(archive)?;
    if write_pointer(archive, location)? {
        changed += 1;
    }
    let report = IngestReport {
        team_slug: team_slug.to_string(),
        source_digest: digest,
        sources: team.sources.len() as i64,
        source_bytes: team.sources.iter().map(|s| s.complete_bytes).sum(),
        agents: team.agents.len() as i64,
        events: team.events.len() as i64,
        tool_calls: team.tool_calls.len() as i64,
        edges: team.edges.len() as i64,
        files_changed: changed,
        artifacts: catalog.artifacts.len() as i64,
        projects: catalog.projects.len() as i64,
        retired_message_projections: retired_files,
        retired_message_projection_bytes: retired_bytes,
        task_notes: promoted.len() as i64,
        newly_promoted_task_notes: newly_promoted,
        task_notes_upstream_deleted: promoted
            .iter()
            .filter(|note| !note.upstream_present)
            .count() as i64,
        tool_payloads: payload_report.stored,
        tool_payload_bytes: payload_report.stored_bytes,
        newly_stored_tool_payloads: payload_report.newly_stored,
        newly_stored_tool_payload_bytes: payload_report.newly_stored_bytes,
        pruned_payload_shards: payload_report.pruned_shards.clone(),
        damaged_payload_shards: payload_report.damaged_shards.clone(),
        snapshot_root: location.archive_relative(),
        snapshot_root_layout: location.layout.clone(),
        snapshot_root_established: !pointer_established,
        ..IngestReport::default()
    };
    let mut out_team = archived;
    out_team.task_notes = promoted;
    Ok((out_team, report))
}

fn source_snapshot_location(
    archive: &Path,
    team_slug: &str,
    requested: Option<&Path>,
) -> Result<SnapshotLocation, String> {
    let location = resolve_snapshot_root(archive, team_slug, requested)?;
    ensure_snapshot_store(&location)?;
    Ok(location)
}

/// Snapshot and normalize the owner's Claude prompt history
/// as one prompt-only team.
#[allow(clippy::too_many_arguments)]
pub fn ingest_claude_history(
    archive: &Path,
    history_file: &Path,
    team_slug: &str,
    display_timezone: &str,
    project_pattern: &str,
    covered_session_ids: &[String],
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    ingest_claude_history_locked(
        archive,
        history_file,
        team_slug,
        display_timezone,
        project_pattern,
        covered_session_ids,
        date_window,
        identity_overrides,
        requested_snapshot_root,
    )
}

#[allow(clippy::too_many_arguments)]
fn ingest_claude_history_locked(
    archive: &Path,
    history_file: &Path,
    team_slug: &str,
    display_timezone: &str,
    project_pattern: &str,
    covered_session_ids: &[String],
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    crate::pipeline::ensure_archive_pub(archive, team_slug, true)?;
    let mut changed = i64::from(ensure_bulk_content_ignored(archive)?);
    let manifest_path = build_raw(archive, team_slug, "source-manifest.json")?;
    let previous_source = load_history_source_manifest(&manifest_path, team_slug, project_pattern)?;
    let location = source_snapshot_location(archive, team_slug, requested_snapshot_root)?;
    let snapshot = snapshot_claude_history(
        history_file,
        &location.root,
        previous_source.as_ref(),
        &crate::pipeline::utc_now(),
    )?;
    changed += snapshot.files_changed;
    let (team, selection): (TeamData, HistorySelection) = load_claude_history_team(
        &location.root.join(&snapshot.source.snapshot_path),
        team_slug,
        display_timezone,
        project_pattern,
        covered_session_ids,
    )?;
    let team = apply_date_window(team, date_window);
    if record_site_identity(archive, &team, identity_overrides)? {
        changed += 1;
    }
    let mut covered_sorted: Vec<String> = covered_session_ids.to_vec();
    covered_sorted.sort();
    covered_sorted.dedup();
    let mut manifest = Map::new();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert(
        "provider".into(),
        Value::String(crate::history::PROVIDER.to_string()),
    );
    manifest.insert(
        "root_thread_id".into(),
        Value::String(team.root_thread_id.clone()),
    );
    manifest.insert(
        "history_file".into(),
        Value::String(
            crate::json::resolve_path(history_file)
                .display()
                .to_string(),
        ),
    );
    manifest.insert(
        "project_pattern".into(),
        Value::String(project_pattern.to_string()),
    );
    manifest.insert(
        "covered_session_ids".into(),
        Value::Array(
            covered_sorted
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    manifest.insert(
        "snapshot_root".into(),
        Value::String(location.archive_relative()),
    );
    manifest.insert(
        "date_window".into(),
        date_window.map_or(Value::Null, DateWindow::to_json),
    );
    manifest.insert(
        "sources".into(),
        Value::Array(vec![snapshot.source.to_json()]),
    );
    if write_json_if_changed(&manifest_path, &Value::Object(manifest))? {
        changed += 1;
    }
    let (team, mut report) =
        write_ingested_team(archive, team_slug, &team, date_window, changed, &location)?;
    report.history_unmatched_prompts = selection.unmatched_prompts;
    report.history_covered_prompts = selection.covered_prompts;
    report.history_covered_sessions = selection.covered_sessions;
    Ok((team, report))
}

// ---------------------------------------------------------------- Codex

/// The recorded Codex manifest state for one team.
pub struct CodexManifestState {
    /// Recorded sources.
    pub sources: Vec<crate::history::SourceCopy>,
    /// Recorded continuation links.
    pub continuation_links: Vec<crate::codex::ContinuationLink>,
    /// Effective continuation thread ids.
    pub continuation_thread_ids: Vec<String>,
}

fn manifest_window_value(window: Option<&DateWindow>) -> Value {
    window.map_or(Value::Null, DateWindow::to_json)
}

fn manifest_window_bounds(
    value: &Value,
    where_: &str,
) -> Result<(Option<i64>, Option<i64>), String> {
    if value.is_null() {
        return Ok((None, None));
    }
    let object = as_object(value, where_)?;
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    let base = ["start_date", "end_date", "start_ms", "end_ms"];
    let exact = [
        "start_date",
        "end_date",
        "start_ms",
        "end_ms",
        "start_time",
        "end_time",
    ];
    let is_base = keys.len() == base.len() && base.iter().all(|key| object.contains_key(*key));
    let is_exact = keys.len() == exact.len() && exact.iter().all(|key| object.contains_key(*key));
    if !is_base && !is_exact {
        return Err(format!("{where_}: invalid date-window fields"));
    }
    // CPython compares 1000.0 == 1000: an integral float in
    // a recorded window reads as that integer.
    let window_int = |value: &Value, key: &str| -> Result<i64, String> {
        match value {
            Value::Number(number) if number.is_f64() => {
                let held = number.as_f64().unwrap_or_default();
                if held.fract() == 0.0 && held.abs() < 9.223_372_036_854_776e18 {
                    Ok(held as i64)
                } else {
                    Err(format!("{where_}.{key}: expected an integer"))
                }
            }
            other => crate::json::as_int(other, &format!("{where_}.{key}")),
        }
    };
    let start = match object.get("start_ms") {
        None | Some(Value::Null) => None,
        Some(value) => Some(window_int(value, "start_ms")?),
    };
    let end = match object.get("end_ms") {
        None | Some(Value::Null) => None,
        Some(value) => Some(window_int(value, "end_ms")?),
    };
    Ok((start, end))
}

fn window_is_same_or_narrower(
    recorded: &Value,
    requested: &Value,
    where_: &str,
) -> Result<bool, String> {
    let (recorded_start, recorded_end) =
        manifest_window_bounds(recorded, &format!("{where_}.recorded"))?;
    let (requested_start, requested_end) =
        manifest_window_bounds(requested, &format!("{where_}.requested"))?;
    let start_is_narrower = recorded_start.is_none()
        || requested_start.is_some_and(|start| start >= recorded_start.unwrap());
    let end_is_narrower =
        recorded_end.is_none() || requested_end.is_some_and(|end| end <= recorded_end.unwrap());
    Ok(start_is_narrower && end_is_narrower)
}

fn validate_manifest_window(
    recorded: &Value,
    window: Option<&DateWindow>,
    where_: &str,
) -> Result<(), String> {
    let requested = manifest_window_value(window);
    if recorded == &requested {
        return Ok(());
    }
    match window_is_same_or_narrower(recorded, &requested, where_) {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "archive date window may only stay unchanged or become narrower; \
             choose a new output directory to widen it"
                .to_string(),
        ),
        Err(error) => Err(format!("invalid archived date window: {error}")),
    }
}

/// Load the recorded Codex source manifest, if any.
pub fn load_codex_manifest(
    archive: &Path,
    team_slug: &str,
    root_thread_id: &str,
    date_window: Option<&DateWindow>,
    requested_continuation_thread_ids: &[String],
) -> Result<CodexManifestState, String> {
    let path = build_raw(archive, team_slug, "source-manifest.json")?;
    if !path.is_file() {
        return Ok(CodexManifestState {
            sources: Vec::new(),
            continuation_links: Vec::new(),
            continuation_thread_ids: requested_continuation_thread_ids.to_vec(),
        });
    }
    let document = read_json(&path)?;
    let object = as_object(&document, &path.display().to_string())?;
    if !crate::json::json_eq_one(object.get("schema_version"))
        || object.get("provider").and_then(Value::as_str) != Some("codex")
    {
        return Err(format!(
            "invalid Codex source manifest at {}",
            path.display()
        ));
    }
    let recorded_root = crate::json::as_string(
        object.get("root_thread_id").unwrap_or(&Value::Null),
        &format!("{}: root_thread_id", path.display()),
    )?;
    if recorded_root != root_thread_id {
        return Err(format!(
            "source manifest belongs to root {}, not {}",
            py_repr_str(recorded_root),
            py_repr_str(root_thread_id)
        ));
    }
    let null = Value::Null;
    validate_manifest_window(
        object.get("date_window").unwrap_or(&null),
        date_window,
        &path.display().to_string(),
    )?;
    let raw_sources = crate::json::as_array(
        object.get("sources").unwrap_or(&null),
        &format!("{}: sources", path.display()),
    )?;
    let mut sources = Vec::new();
    for (index, raw_source) in raw_sources.iter().enumerate() {
        let where_ = format!("{}: sources[{index}]", path.display());
        let _ = as_object(raw_source, &where_)?;
        sources.push(crate::codex::codex_source_from_json(raw_source, &where_)?);
    }
    let mut links = Vec::new();
    if let Some(raw_continuations) = object
        .get("continuation_sessions")
        .filter(|value| !value.is_null())
    {
        let values = crate::json::as_array(
            raw_continuations,
            &format!("{}: continuation_sessions", path.display()),
        )?;
        for (index, raw_link) in values.iter().enumerate() {
            links.push(crate::codex::ContinuationLink::from_json(
                raw_link,
                &format!("{}: continuation_sessions[{index}]", path.display()),
            )?);
        }
    }
    let recorded_ids: Vec<String> = links.iter().map(|link| link.thread_id.clone()).collect();
    let effective_ids = if !requested_continuation_thread_ids.is_empty() {
        if recorded_ids.as_slice()
            != &requested_continuation_thread_ids[..recorded_ids
                .len()
                .min(requested_continuation_thread_ids.len())]
            || recorded_ids.len() > requested_continuation_thread_ids.len()
        {
            return Err(
                "requested continuation sessions do not extend the recorded ordered prefix"
                    .to_string(),
            );
        }
        requested_continuation_thread_ids.to_vec()
    } else {
        recorded_ids
    };
    Ok(CodexManifestState {
        sources,
        continuation_links: links,
        continuation_thread_ids: effective_ids,
    })
}

/// Snapshot and normalize one Codex lineage.
#[allow(clippy::too_many_arguments)]
pub fn ingest_codex(
    archive: &Path,
    sessions_root: &Path,
    root_thread_id: &str,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    continuation_thread_ids: &[String],
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    ingest_codex_locked(
        archive,
        sessions_root,
        root_thread_id,
        team_slug,
        display_timezone,
        date_window,
        identity_overrides,
        continuation_thread_ids,
        requested_snapshot_root,
    )
}

#[allow(clippy::too_many_arguments)]
fn ingest_codex_locked(
    archive: &Path,
    sessions_root: &Path,
    root_thread_id: &str,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    continuation_thread_ids: &[String],
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    crate::pipeline::ensure_archive_pub(archive, team_slug, true)?;
    let mut changed = i64::from(ensure_bulk_content_ignored(archive)?);
    let manifest_state = load_codex_manifest(
        archive,
        team_slug,
        root_thread_id,
        date_window,
        continuation_thread_ids,
    )?;
    let location = source_snapshot_location(archive, team_slug, requested_snapshot_root)?;
    let source_copies = crate::codex::snapshot_codex_lineage(
        sessions_root,
        root_thread_id,
        &location.root,
        &manifest_state.sources,
        &crate::pipeline::utc_now(),
        &manifest_state.continuation_thread_ids,
        &manifest_state.continuation_links,
    )?;
    changed += source_copies.files_changed;
    let allowed_sources: Vec<String> = source_copies
        .sources
        .iter()
        .map(|source| source.snapshot_path.clone())
        .collect();
    let team = apply_date_window(
        crate::codex::load_codex_team(
            &location.root,
            root_thread_id,
            team_slug,
            display_timezone,
            Some(&allowed_sources),
            &source_copies.continuations,
        )?,
        date_window,
    );
    let mut parsed_paths: Vec<String> = team
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    let mut allowed_sorted = allowed_sources.clone();
    parsed_paths.sort();
    allowed_sorted.sort();
    if parsed_paths != allowed_sorted {
        return Err(
            "parsed source set differs from the just-validated source snapshot manifest"
                .to_string(),
        );
    }
    let inferred =
        crate::identity::infer_structured_identity(&crate::codex::codex_identity_metadata(
            &location.root,
            &allowed_sources,
            root_thread_id,
            &manifest_state.continuation_thread_ids,
        )?);
    if record_site_identity_inferred(archive, &team, &inferred, identity_overrides)? {
        changed += 1;
    }
    let mut manifest = Map::new();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert("provider".into(), Value::String("codex".to_string()));
    manifest.insert(
        "root_thread_id".into(),
        Value::String(root_thread_id.to_string()),
    );
    manifest.insert(
        "source_root".into(),
        Value::String(
            crate::json::resolve_path(sessions_root)
                .display()
                .to_string(),
        ),
    );
    manifest.insert(
        "snapshot_root".into(),
        Value::String(location.archive_relative()),
    );
    manifest.insert(
        "date_window".into(),
        date_window.map_or(Value::Null, DateWindow::to_json),
    );
    manifest.insert(
        "sources".into(),
        Value::Array(
            source_copies
                .sources
                .iter()
                .map(crate::history::SourceCopy::to_json)
                .collect(),
        ),
    );
    if !source_copies.continuations.is_empty() {
        manifest.insert(
            "continuation_sessions".into(),
            Value::Array(
                source_copies
                    .continuations
                    .iter()
                    .map(crate::codex::ContinuationLink::to_json)
                    .collect(),
            ),
        );
    }
    if write_json_if_changed(
        &build_raw(archive, team_slug, "source-manifest.json")?,
        &Value::Object(manifest),
    )? {
        changed += 1;
    }
    let (team, mut report) =
        write_ingested_team(archive, team_slug, &team, date_window, changed, &location)?;
    report.upstream_retired_sources = source_copies.upstream_retired.clone();
    Ok((team, report))
}

fn record_site_identity_inferred(
    archive: &Path,
    team: &TeamData,
    inferred: &(
        Vec<crate::identity::ProjectIdentity>,
        Vec<crate::identity::HostIdentity>,
    ),
    overrides: &IdentityOverrides,
) -> Result<bool, String> {
    let path = build_raw(archive, &team.team_slug, "site-identity.json")?;
    let previous = if path.is_file() {
        let document = read_json(&path)?;
        Some(site_identity_from_json(
            &document,
            &path.display().to_string(),
        )?)
    } else {
        None
    };
    if let Some(previous) = &previous {
        if previous.team_slug != team.team_slug {
            return Err(format!(
                "site identity team {} does not match {}",
                py_repr_str(&previous.team_slug),
                py_repr_str(&team.team_slug)
            ));
        }
    }
    let identity = merge_site_identity(
        &team.team_slug,
        &team.display_timezone,
        &overrides.display_timezone_source,
        &inferred.0,
        &inferred.1,
        &overrides.projects,
        &overrides.hosts,
        previous.as_ref(),
    )?;
    write_json_if_changed(&path, &identity.to_json())
}

// ---------------------------------------------------------------- Claude

/// Load the recorded Claude source manifest, if any.
pub fn load_claude_manifest(
    archive: &Path,
    team_slug: &str,
    root_thread_id: &str,
    date_window: Option<&DateWindow>,
) -> Result<Vec<crate::history::SourceCopy>, String> {
    let path = build_raw(archive, team_slug, "source-manifest.json")?;
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let document = read_json(&path)?;
    let object = as_object(&document, &path.display().to_string())?;
    if !crate::json::json_eq_one(object.get("schema_version"))
        || object.get("provider").and_then(Value::as_str) != Some("claude")
    {
        return Err(format!(
            "invalid Claude source manifest at {}",
            path.display()
        ));
    }
    let recorded_root = crate::json::as_string(
        object.get("root_thread_id").unwrap_or(&Value::Null),
        &format!("{}: root_thread_id", path.display()),
    )?;
    if recorded_root != root_thread_id {
        return Err(format!(
            "source manifest belongs to root {}, not {}",
            py_repr_str(recorded_root),
            py_repr_str(root_thread_id)
        ));
    }
    let null = Value::Null;
    validate_manifest_window(
        object.get("date_window").unwrap_or(&null),
        date_window,
        &path.display().to_string(),
    )?;
    let raw_sources = crate::json::as_array(
        object.get("sources").unwrap_or(&null),
        &format!("{}: sources", path.display()),
    )?;
    let mut sources = Vec::new();
    for (index, raw_source) in raw_sources.iter().enumerate() {
        let where_ = format!("{}: sources[{index}]", path.display());
        let _ = as_object(raw_source, &where_)?;
        sources.push(crate::history::SourceCopy::from_json(raw_source, &where_)?);
    }
    Ok(sources)
}

/// Snapshot and normalize one Claude coordinator lineage.
#[allow(clippy::too_many_arguments)]
pub fn ingest_claude(
    archive: &Path,
    session_file: &Path,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    ingest_claude_locked(
        archive,
        session_file,
        team_slug,
        display_timezone,
        date_window,
        identity_overrides,
        requested_snapshot_root,
    )
}

#[allow(clippy::too_many_arguments)]
fn ingest_claude_locked(
    archive: &Path,
    session_file: &Path,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let root_thread_id = session_file
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::pipeline::ensure_archive_pub(archive, team_slug, true)?;
    let mut changed = i64::from(ensure_bulk_content_ignored(archive)?);
    let previous_sources = load_claude_manifest(archive, team_slug, &root_thread_id, date_window)?;
    let location = source_snapshot_location(archive, team_slug, requested_snapshot_root)?;
    let source_copies = crate::claude::snapshot_claude_lineage(
        session_file,
        &location.root,
        &previous_sources,
        &crate::pipeline::utc_now(),
    )?;
    changed += source_copies.files_changed;
    let allowed_sources: Vec<String> = source_copies
        .sources
        .iter()
        .map(|source| source.snapshot_path.clone())
        .collect();
    let snapshot_session = location.root.join(
        session_file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let team = apply_date_window(
        crate::claude::load_claude_team(
            &snapshot_session,
            team_slug,
            display_timezone,
            Some(&allowed_sources),
        )?,
        date_window,
    );
    let mut parsed_paths: Vec<String> = team
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    let mut allowed_sorted = allowed_sources.clone();
    parsed_paths.sort();
    allowed_sorted.sort();
    if parsed_paths != allowed_sorted {
        return Err(
            "parsed source set differs from the just-validated source snapshot manifest"
                .to_string(),
        );
    }
    if record_site_identity(archive, &team, identity_overrides)? {
        changed += 1;
    }
    let mut manifest = Map::new();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert("provider".into(), Value::String("claude".to_string()));
    manifest.insert(
        "root_thread_id".into(),
        Value::String(root_thread_id.clone()),
    );
    manifest.insert(
        "source_root".into(),
        Value::String(
            crate::json::resolve_path(session_file.parent().unwrap_or(Path::new("")))
                .display()
                .to_string(),
        ),
    );
    manifest.insert(
        "root_session_file".into(),
        Value::String(
            crate::json::resolve_path(session_file)
                .display()
                .to_string(),
        ),
    );
    manifest.insert(
        "snapshot_root".into(),
        Value::String(location.archive_relative()),
    );
    manifest.insert(
        "date_window".into(),
        date_window.map_or(Value::Null, DateWindow::to_json),
    );
    manifest.insert(
        "sources".into(),
        Value::Array(
            source_copies
                .sources
                .iter()
                .map(crate::history::SourceCopy::to_json)
                .collect(),
        ),
    );
    if write_json_if_changed(
        &build_raw(archive, team_slug, "source-manifest.json")?,
        &Value::Object(manifest),
    )? {
        changed += 1;
    }
    let (team, mut report) =
        write_ingested_team(archive, team_slug, &team, date_window, changed, &location)?;
    report.upstream_retired_sources = source_copies.upstream_retired.clone();
    Ok((team, report))
}

// ---------------------------------------------------------------- Muse

/// Load the recorded Muse source manifest, if any.
pub fn load_muse_manifest(
    archive: &Path,
    team_slug: &str,
    root_session_id: &str,
    date_window: Option<&DateWindow>,
) -> Result<Vec<crate::muse::MuseSourceCopy>, String> {
    let path = build_raw(archive, team_slug, "source-manifest.json")?;
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let document = read_json(&path)?;
    let object = as_object(&document, &path.display().to_string())?;
    if !crate::json::json_eq_one(object.get("schema_version"))
        || object.get("provider").and_then(Value::as_str) != Some("muse")
    {
        return Err(format!(
            "invalid Muse source manifest at {}",
            path.display()
        ));
    }
    let recorded_root = crate::json::as_string(
        object.get("root_thread_id").unwrap_or(&Value::Null),
        &format!("{}: root_thread_id", path.display()),
    )?;
    if recorded_root != root_session_id {
        return Err(format!(
            "source manifest belongs to root {}, not {}",
            py_repr_str(recorded_root),
            py_repr_str(root_session_id)
        ));
    }
    let null = Value::Null;
    validate_manifest_window(
        object.get("date_window").unwrap_or(&null),
        date_window,
        &path.display().to_string(),
    )?;
    let raw_sources = crate::json::as_array(
        object.get("sources").unwrap_or(&null),
        &format!("{}: sources", path.display()),
    )?;
    let mut sources = Vec::new();
    for (index, raw_source) in raw_sources.iter().enumerate() {
        let where_ = format!("{}: sources[{index}]", path.display());
        let _ = as_object(raw_source, &where_)?;
        sources.push(crate::muse::MuseSourceCopy::from_json(raw_source, &where_)?);
    }
    Ok(sources)
}

/// Snapshot and normalize one Muse session and its subagents.
#[allow(clippy::too_many_arguments)]
pub fn ingest_muse(
    archive: &Path,
    session_dir: &Path,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    ingest_muse_locked(
        archive,
        session_dir,
        team_slug,
        display_timezone,
        date_window,
        identity_overrides,
        requested_snapshot_root,
    )
}

#[allow(clippy::too_many_arguments)]
fn ingest_muse_locked(
    archive: &Path,
    session_dir: &Path,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let root_session_id = session_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::pipeline::ensure_archive_pub(archive, team_slug, true)?;
    let mut changed = i64::from(ensure_bulk_content_ignored(archive)?);
    let previous_sources = load_muse_manifest(archive, team_slug, &root_session_id, date_window)?;
    let location = source_snapshot_location(archive, team_slug, requested_snapshot_root)?;
    let source_copies = crate::muse::snapshot_muse_lineage(
        session_dir,
        &location.root,
        &previous_sources,
        &crate::pipeline::utc_now(),
    )?;
    changed += source_copies.files_changed;
    let allowed_sources: Vec<String> = source_copies
        .sources
        .iter()
        .map(|source| source.snapshot_path.clone())
        .collect();
    let team = apply_date_window(
        crate::muse::load_muse_team(
            &location.root,
            &root_session_id,
            team_slug,
            display_timezone,
            &allowed_sources,
        )?,
        date_window,
    );
    if record_site_identity(archive, &team, identity_overrides)? {
        changed += 1;
    }
    let mut manifest = Map::new();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert("provider".into(), Value::String("muse".to_string()));
    manifest.insert(
        "root_thread_id".into(),
        Value::String(root_session_id.clone()),
    );
    manifest.insert(
        "source_root".into(),
        Value::String(crate::json::resolve_path(session_dir).display().to_string()),
    );
    manifest.insert(
        "snapshot_root".into(),
        Value::String(location.archive_relative()),
    );
    manifest.insert(
        "date_window".into(),
        date_window.map_or(Value::Null, DateWindow::to_json),
    );
    manifest.insert(
        "sources".into(),
        Value::Array(
            source_copies
                .sources
                .iter()
                .map(crate::muse::MuseSourceCopy::to_json)
                .collect(),
        ),
    );
    if write_json_if_changed(
        &build_raw(archive, team_slug, "source-manifest.json")?,
        &Value::Object(manifest),
    )? {
        changed += 1;
    }
    let (team, mut report) =
        write_ingested_team(archive, team_slug, &team, date_window, changed, &location)?;
    report.upstream_retired_sources = source_copies.upstream_retired.clone();
    Ok((team, report))
}

// ---------------------------------------------------------------- Orc

/// The recorded Orc manifest: sources, continuation
/// links, and the effective continuation specs.
#[derive(Clone, Debug)]
pub struct OrcManifestState {
    /// Retained source copies.
    pub sources: Vec<crate::orc::SourceCopy>,
    /// Recorded continuation links.
    pub continuation_links: Vec<crate::orc::ContinuationLink>,
    /// Effective continuation specs.
    pub continuation_specs: Vec<crate::orc::ContinuationSpec>,
}

fn py_str_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|held| py_repr_str(held))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn orc_continuation_specs(
    values: &[crate::orc::ContinuationSpec],
    where_: &str,
) -> Result<Vec<crate::orc::ContinuationSpec>, String> {
    let mut ids: Vec<&str> = values.iter().map(|spec| spec.session_id.as_str()).collect();
    ids.sort();
    ids.dedup();
    if ids.len() != values.len() {
        return Err(format!("{where_}: duplicate session ids are not allowed"));
    }
    Ok(values.to_vec())
}

/// Load the recorded Orc source manifest, if any.
pub fn load_orc_manifest(
    archive: &Path,
    team_slug: &str,
    root_session_id: &str,
    date_window: Option<&DateWindow>,
    requested_continuations: &[crate::orc::ContinuationSpec],
) -> Result<OrcManifestState, String> {
    let requested_specs =
        orc_continuation_specs(requested_continuations, "requested continuation sessions")?;
    let path = build_raw(archive, team_slug, "source-manifest.json")?;
    if !path.is_file() {
        return Ok(OrcManifestState {
            sources: Vec::new(),
            continuation_links: Vec::new(),
            continuation_specs: requested_specs,
        });
    }
    let document = read_json(&path)?;
    let object = as_object(&document, &path.display().to_string())?;
    let schema_version = crate::json::as_int(
        object.get("schema_version").unwrap_or(&Value::Null),
        &format!("{}: schema_version", path.display()),
    )?;
    if !(1..=5).contains(&schema_version)
        || object.get("provider").and_then(Value::as_str) != Some("orc")
    {
        return Err(format!("invalid Orc source manifest at {}", path.display()));
    }
    if (2..=5).contains(&schema_version) {
        let mut expected: Vec<&str> = vec![
            "schema_version",
            "provider",
            "root_session_id",
            "source_root",
            "snapshot_root",
            "date_window",
            "sources",
        ];
        if (3..=5).contains(&schema_version) {
            expected.push("continuation_sessions");
        }
        let present: Vec<&str> = object.keys().map(|held| held.as_str()).collect();
        if present.len() != expected.len() || !expected.iter().all(|held| present.contains(held)) {
            let missing: Vec<String> = expected
                .iter()
                .filter(|held| !present.contains(held))
                .map(|held| held.to_string())
                .collect();
            let unknown: Vec<String> = present
                .iter()
                .filter(|held| !expected.contains(held))
                .map(|held| held.to_string())
                .collect();
            return Err(format!(
                "invalid Orc source manifest fields at {}: missing={}, unknown={}",
                path.display(),
                py_str_list(&missing),
                py_str_list(&unknown)
            ));
        }
        let recorded_source_root = crate::json::as_string(
            object.get("source_root").unwrap_or(&Value::Null),
            &format!("{}: source_root", path.display()),
        )?;
        if !Path::new(recorded_source_root).is_absolute() {
            return Err(format!("{}: source_root must be absolute", path.display()));
        }
        // Recorded, shape-checked, and deliberately *not* compared against where this run is
        // about to write.
        //
        // It used to be compared, against the single hardcoded `teams/<slug>/source_snapshots`,
        // and that check stopped being expressible the moment the store became configurable and
        // relocatable: after a migration the manifest still says where the bytes were at the last
        // ingest, which is the honest thing for it to say and is no longer where they are. Making
        // `migrate-snapshots` rewrite the manifest instead was rejected -- `raw/source-manifest.json`
        // is bound by digest into `raw/normalized-generation.json`, so touching it would leave
        // every reader refusing the archive as a mixed generation until someone re-ingested, which
        // is a much worse outcome than a stale descriptive field.
        //
        // Nothing is lost, because the string equality was only ever a proxy for the question that
        // matters -- "are the previous snapshots this manifest names actually here?" -- and that
        // question is answered properly, per object and by SHA-256, in `prepare_snapshot_candidate`
        // a few frames down. A relocated tree whose contents are wrong fails there, on the bytes,
        // rather than here, on a path.
        //
        // The shape check stays: relative, non-empty. An absolute path in a tracked, HTTP-served
        // file would publish somebody's home directory, and a manifest that recorded one would keep
        // being wrong every time the archive moved.
        let recorded_snapshot_root = crate::json::as_string(
            object.get("snapshot_root").unwrap_or(&Value::Null),
            &format!("{}: snapshot_root", path.display()),
        )?;
        if recorded_snapshot_root.is_empty() || Path::new(recorded_snapshot_root).is_absolute() {
            return Err(format!(
                "{}: snapshot_root must be a non-empty path relative to the archive",
                path.display()
            ));
        }
    }
    let recorded_root = crate::json::as_string(
        object.get("root_session_id").unwrap_or(&Value::Null),
        &format!("{}: root_session_id", path.display()),
    )?;
    if recorded_root != root_session_id {
        return Err(format!(
            "source manifest belongs to root {}, not {}",
            py_repr_str(recorded_root),
            py_repr_str(root_session_id)
        ));
    }
    let null = Value::Null;
    validate_manifest_window(
        object.get("date_window").unwrap_or(&null),
        date_window,
        &path.display().to_string(),
    )?;
    let raw_sources = crate::json::as_array(
        object.get("sources").unwrap_or(&null),
        &format!("{}: sources", path.display()),
    )?;
    let record_schema = if (3..=5).contains(&schema_version) {
        2
    } else {
        schema_version
    };
    let mut sources = Vec::new();
    for (index, raw_source) in raw_sources.iter().enumerate() {
        let where_ = format!("{}: sources[{index}]", path.display());
        let held = as_object(raw_source, &where_)?;
        sources.push(crate::orc::SourceCopy::from_json(
            held,
            &where_,
            record_schema,
        )?);
    }
    let mut links: Vec<crate::orc::ContinuationLink> = Vec::new();
    if let Some(raw_continuations) = object
        .get("continuation_sessions")
        .filter(|held| !held.is_null())
    {
        if !(3..=5).contains(&schema_version) {
            return Err(format!(
                "{}: continuation_sessions requires manifest schema version 3, 4 or 5",
                path.display()
            ));
        }
        let held = crate::json::as_array(
            raw_continuations,
            &format!("{}: continuation_sessions", path.display()),
        )?;
        for (index, raw_link) in held.iter().enumerate() {
            let where_ = format!("{}: continuation_sessions[{index}]", path.display());
            let link = as_object(raw_link, &where_)?;
            let has_bounded_fields =
                link.contains_key("start_message_id") || link.contains_key("start_source_line");
            let has_boundary_table = link.contains_key("predecessor_source_table");
            if schema_version == 3 && has_bounded_fields {
                return Err(format!(
                    "{path}: schema-v3 continuation record cannot contain bounded fields",
                    path = path.display()
                ));
            }
            if (3..=4).contains(&schema_version) && has_boundary_table {
                return Err(format!(
                    "{}: schema-v{schema_version} continuation record cannot name a predecessor boundary table",
                    path.display()
                ));
            }
            if (4..=5).contains(&schema_version)
                && !(link.contains_key("start_message_id")
                    && link.contains_key("start_source_line"))
            {
                return Err(format!(
                    "{}: schema-v{schema_version} continuation record lacks bounded fields",
                    path.display()
                ));
            }
            // v5 is the version at which the boundary ordinal stopped being a bare integer whose
            // table had to be re-derived at read time. The key must be present; its *value* may
            // still be null, because a boundary whose ordinal resolves identically in both candidate
            // tables is deliberately left unresolved rather than guessed. Presence is therefore the
            // only thing worth demanding: it distinguishes "a table-aware writer looked at this
            // link" from "this link predates the question".
            if schema_version == 5 && !has_boundary_table {
                return Err(format!(
                    "{}: schema-v5 continuation record lacks its predecessor_source_table field",
                    path.display()
                ));
            }
            links.push(crate::orc::ContinuationLink::from_json(link, &where_)?);
        }
    }
    let mut recorded_specs: Vec<crate::orc::ContinuationSpec> = Vec::new();
    for (index, link) in links.iter().enumerate() {
        let mut held = Map::new();
        held.insert("session_id".into(), Value::String(link.session_id.clone()));
        held.insert(
            "start_message_id".into(),
            link.start_message_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        recorded_specs.push(crate::orc::ContinuationSpec::from_value(
            &Value::Object(held),
            &format!("{}: continuation_sessions[{index}]", path.display()),
        )?);
    }
    let effective_specs = if !requested_specs.is_empty() {
        if requested_specs.len() < recorded_specs.len()
            || requested_specs[..recorded_specs.len()] != recorded_specs[..]
        {
            return Err(
                "requested continuation sessions do not extend the recorded ordered prefix"
                    .to_string(),
            );
        }
        requested_specs
    } else {
        recorded_specs
    };
    Ok(OrcManifestState {
        sources,
        continuation_links: links,
        continuation_specs: effective_specs,
    })
}

/// Refuse a retired Orc lineage whose own snapshot
/// objects are missing or altered.
pub fn verify_retained_orc_snapshots(
    snapshot_root: &Path,
    sources: &[crate::orc::SourceCopy],
) -> Result<(), String> {
    for source in sources {
        let path = crate::orc::snapshot_path(snapshot_root, &source.snapshot_path)?;
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !path.is_file() {
            return Err(format!(
                "Orc lineage is gone upstream and the archive's own snapshot is missing too: {} (expected {})",
                source.source_path,
                path.display()
            ));
        }
        if crate::orc::sha256_file(&path)? != source.sha256 {
            return Err(format!(
                "Orc lineage is gone upstream and the archive's own snapshot does not match its manifest: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Snapshot and normalize one Orc lineage as a
/// serialized raw-data transaction.
#[allow(clippy::too_many_arguments)]
pub fn ingest_orc(
    archive: &Path,
    source_root: &Path,
    root_session_id: &str,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    continuation_specs: &[crate::orc::ContinuationSpec],
    accept_prefix_rewrite: &[String],
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    ingest_orc_locked(
        archive,
        source_root,
        root_session_id,
        team_slug,
        display_timezone,
        date_window,
        identity_overrides,
        continuation_specs,
        accept_prefix_rewrite,
        requested_snapshot_root,
    )
}

#[allow(clippy::too_many_arguments)]
fn ingest_orc_locked(
    archive: &Path,
    source_root: &Path,
    root_session_id: &str,
    team_slug: &str,
    display_timezone: &str,
    date_window: Option<&DateWindow>,
    identity_overrides: &IdentityOverrides,
    continuation_specs: &[crate::orc::ContinuationSpec],
    accept_prefix_rewrite: &[String],
    requested_snapshot_root: Option<&Path>,
) -> Result<(TeamData, IngestReport), String> {
    crate::pipeline::ensure_archive_pub(archive, team_slug, true)?;
    let mut changed = i64::from(ensure_bulk_content_ignored(archive)?);
    let manifest_state = load_orc_manifest(
        archive,
        team_slug,
        root_session_id,
        date_window,
        continuation_specs,
    )?;
    let location = source_snapshot_location(archive, team_slug, requested_snapshot_root)?;
    let snapshot_root = location.root.clone();
    changed += crate::orc::prune_orc_staging(&snapshot_root)?;
    // Absent in both spellings, not merely unreadable: a root that is present but irregular is
    // left to the snapshot path below, which refuses it. Child sessions that outlive a retired
    // root are not followed; the lineage is frozen at its last import.
    let root_database = source_root
        .join(".orc")
        .join("sessions")
        .join(root_session_id)
        .join("session.db");
    let retired_lineage = !manifest_state.sources.is_empty()
        && crate::compressed::resolve_source(&root_database).is_none();
    let snapshot = if retired_lineage {
        // The root session database is gone upstream in both spellings: the whole lineage has
        // retired, and the archive's content-addressed snapshots are the record. Re-normalize
        // from them -- after checking each against its manifest digest -- instead of refusing.
        // Nothing new can arrive for a lineage whose root is gone, so the checkpoint stands.
        verify_retained_orc_snapshots(&snapshot_root, &manifest_state.sources)?;
        crate::orc::SnapshotResult {
            sources: manifest_state.sources.clone(),
            files_changed: 0,
            continuations: manifest_state.continuation_links.clone(),
            prefix_overrides: Vec::new(),
        }
    } else {
        crate::orc::snapshot_orc_lineage(
            source_root,
            root_session_id,
            &snapshot_root,
            &manifest_state.sources,
            &crate::pipeline::utc_now(),
            &manifest_state.continuation_specs,
            &manifest_state.continuation_links,
            accept_prefix_rewrite,
        )?
    };
    changed += snapshot.files_changed;
    let mut resolved_specs: Vec<crate::orc::ContinuationSpec> = Vec::new();
    for (index, link) in snapshot.continuations.iter().enumerate() {
        let mut held = Map::new();
        held.insert("session_id".into(), Value::String(link.session_id.clone()));
        held.insert(
            "start_message_id".into(),
            link.start_message_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        resolved_specs.push(crate::orc::ContinuationSpec::from_value(
            &Value::Object(held),
            &format!("resolved continuation sessions[{index}]"),
        )?);
    }
    if resolved_specs != manifest_state.continuation_specs {
        return Err(
            "resolved continuation sessions differ from the requested ordered specs".to_string(),
        );
    }
    let team = apply_date_window(
        crate::orc::load_orc_team(
            &snapshot_root,
            root_session_id,
            team_slug,
            display_timezone,
            &snapshot.sources,
            &snapshot.continuations,
        )?,
        date_window,
    );
    let mut parsed_paths: Vec<String> = team
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    let mut expected_paths: Vec<String> = snapshot
        .sources
        .iter()
        .map(|source| source.source_path.clone())
        .collect();
    parsed_paths.sort();
    expected_paths.sort();
    if parsed_paths != expected_paths {
        return Err(
            "parsed source set differs from the validated Orc source snapshots".to_string(),
        );
    }
    if record_site_identity(archive, &team, identity_overrides)? {
        changed += 1;
    }
    let mut source_manifest = Map::new();
    source_manifest.insert(
        "schema_version".into(),
        if snapshot.continuations.is_empty() {
            2.into()
        } else {
            5.into()
        },
    );
    source_manifest.insert("provider".into(), Value::String("orc".to_string()));
    source_manifest.insert(
        "root_session_id".into(),
        Value::String(root_session_id.to_string()),
    );
    source_manifest.insert(
        "source_root".into(),
        Value::String(crate::json::resolve_path(source_root).display().to_string()),
    );
    source_manifest.insert(
        "snapshot_root".into(),
        Value::String(location.archive_relative()),
    );
    source_manifest.insert(
        "date_window".into(),
        date_window.map_or(Value::Null, DateWindow::to_json),
    );
    source_manifest.insert(
        "sources".into(),
        Value::Array(
            snapshot
                .sources
                .iter()
                .map(crate::orc::SourceCopy::to_json)
                .collect(),
        ),
    );
    if !snapshot.continuations.is_empty() {
        source_manifest.insert(
            "continuation_sessions".into(),
            Value::Array(
                snapshot
                    .continuations
                    .iter()
                    .map(crate::orc::ContinuationLink::to_json)
                    .collect(),
            ),
        );
    }
    if write_json_if_changed(
        &build_raw(archive, team_slug, "source-manifest.json")?,
        &Value::Object(source_manifest),
    )? {
        changed += 1;
    }
    let (archived, mut report) =
        write_ingested_team(archive, team_slug, &team, date_window, changed, &location)?;
    let marker_changed = write_json_if_changed(
        &build_raw(archive, team_slug, "normalized-generation.json")?,
        &Value::Object(crate::pipeline::normalized_generation_value_pub(
            archive, team_slug, &archived,
        )?),
    )?;
    let gc_changed = crate::orc::prune_orc_snapshot_objects(&snapshot_root, &snapshot.sources)?;
    report.files_changed += i64::from(marker_changed) + gc_changed;
    report.orc_prefix_overrides = snapshot.prefix_overrides.clone();
    report.upstream_retired_sources = if retired_lineage {
        let mut held: Vec<String> = snapshot
            .sources
            .iter()
            .map(|source| source.source_path.clone())
            .collect();
        held.sort();
        held
    } else {
        Vec::new()
    };
    Ok((archived, report))
}
