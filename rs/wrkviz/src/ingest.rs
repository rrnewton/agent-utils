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
use crate::json::{dump_compact, py_repr_str, read_json};
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
        out.insert("orc_prefix_overrides".into(), Value::Array(Vec::new()));
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
