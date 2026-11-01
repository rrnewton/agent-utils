//! Deterministic, zero-token composition of independently
//! summarized team sites: `py/wrkviz/multi_team.py` at parity.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::json::{as_array, as_int, as_object, as_string, dump_canonical, os_error, read_json};
use crate::team_data::TeamData;
use crate::window::DateWindow;

const EXPORT_MANIFEST: &str = "data/export.json";
const COMMON_FILES: [&str; 11] = [
    "index.html",
    "timeline-core.js",
    "app.js",
    "style.css",
    "serve.py",
    "run_stats.py",
    "timeline",
    "Makefile",
    "vendor/README.md",
    "vendor/markdown-it-15.0.0.min.js",
    "vendor/markdown-it-LICENSE.txt",
];

/// What a combined export produced, mirroring the oracle's
/// report dict key for key.
pub struct CombinedReport {
    /// Files changed.
    pub files_changed: i64,
    /// Teams combined.
    pub teams: i64,
    /// Phases merged.
    pub phases: i64,
    /// Activity bins merged.
    pub activity_bins: i64,
    /// Agents merged.
    pub agents: i64,
    /// Edges merged.
    pub edges: i64,
    /// Events merged.
    pub events: i64,
    /// Rollups merged.
    pub rollups: i64,
    /// Summary files merged.
    pub summary_files: i64,
    /// Artifacts merged.
    pub artifacts: i64,
    /// Projects merged.
    pub projects: i64,
    /// Schema-3 timeline shards.
    pub detail_shards: i64,
    /// Search records indexed.
    pub search_records: i64,
    /// Schema-3 search shards.
    pub search_shards: i64,
    /// Bootstrap bytes.
    pub bootstrap_bytes: i64,
    /// Bootstrap transfer bytes.
    pub bootstrap_transfer_bytes: i64,
    /// Shard object bytes.
    pub shard_object_bytes: i64,
    /// Shard transfer bytes.
    pub shard_transfer_bytes: i64,
}

impl CombinedReport {
    /// The report as a JSON object, in the oracle's dict order.
    pub fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert("files_changed".into(), self.files_changed.into());
        out.insert("teams".into(), self.teams.into());
        out.insert("phases".into(), self.phases.into());
        out.insert("activity_bins".into(), self.activity_bins.into());
        out.insert("agents".into(), self.agents.into());
        out.insert("edges".into(), self.edges.into());
        out.insert("events".into(), self.events.into());
        out.insert("rollups".into(), self.rollups.into());
        out.insert("summary_files".into(), self.summary_files.into());
        out.insert("artifacts".into(), self.artifacts.into());
        out.insert("projects".into(), self.projects.into());
        out.insert("detail_shards".into(), self.detail_shards.into());
        out.insert("search_records".into(), self.search_records.into());
        out.insert("search_shards".into(), self.search_shards.into());
        out.insert("bootstrap_bytes".into(), self.bootstrap_bytes.into());
        out.insert(
            "bootstrap_transfer_bytes".into(),
            self.bootstrap_transfer_bytes.into(),
        );
        out.insert("shard_object_bytes".into(), self.shard_object_bytes.into());
        out.insert(
            "shard_transfer_bytes".into(),
            self.shard_transfer_bytes.into(),
        );
        Value::Object(out)
    }
}

struct RenderedTeam {
    slug: String,
    provider: String,
    team: TeamData,
    root: PathBuf,
    timeline: Map<String, Value>,
    artifacts: Map<String, Value>,
}

#[derive(Default)]
struct TeamValues {
    agents: Vec<Value>,
    phases: Vec<Value>,
    activity_bins: Vec<Value>,
    edges: Vec<Value>,
    events: Vec<Value>,
    rollups: Vec<Value>,
    summary_files: Vec<Value>,
    glossary: Vec<Value>,
    project_overviews: Vec<Value>,
}

#[derive(Default)]
struct Merged {
    teams: Vec<Value>,
    agents: Vec<Value>,
    phases: Vec<Value>,
    activity_bins: Vec<Value>,
    edges: Vec<Value>,
    events: Vec<Value>,
    rollups: Vec<Value>,
    summary_files: Vec<Value>,
    glossary: Vec<Value>,
    project_overviews: Vec<Value>,
    artifacts: Vec<Value>,
    projects: Vec<Value>,
}

fn namespaced(team_slug: &str, value: &str) -> String {
    format!("{team_slug}::{value}")
}

fn prefixed_id(team_slug: &str, value: &str, prefix: &str, label: &str) -> Result<String, String> {
    let rest = value.strip_prefix(prefix);
    let valid = match rest {
        Some(rest) => {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        }
        None => false,
    };
    if !valid {
        return Err(format!(
            "team {team_slug}: invalid {label} ID {}",
            crate::json::py_repr_str(value)
        ));
    }
    Ok(format!("{prefix}{team_slug}-{}", &value[prefix.len()..]))
}

fn artifact_id(team_slug: &str, value: &str) -> Result<String, String> {
    prefixed_id(team_slug, value, "artifact-", "artifact")
}

fn glossary_id(team_slug: &str, value: &str) -> Result<String, String> {
    prefixed_id(team_slug, value, "term-", "glossary")
}

fn get_str(item: &Map<String, Value>, key: &str, where_: &str) -> Result<String, String> {
    as_string(
        item.get(key).unwrap_or(&Value::Null),
        &format!("{where_}.{key}"),
    )
    .map(str::to_string)
}

fn optional_string(
    item: &Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<Option<String>, String> {
    match item.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            as_string(value, &format!("{where_}.{key}")).map(|text| Some(text.to_string()))
        }
    }
}

type Remap<'a> = &'a dyn Fn(&str) -> Result<String, String>;

fn remap_string_list(
    item: &mut Map<String, Value>,
    field: &str,
    where_: &str,
    remap: Remap<'_>,
) -> Result<(), String> {
    let Some(raw) = item.get(field) else {
        return Ok(());
    };
    if raw.is_null() {
        return Ok(());
    }
    let values = as_array(raw, &format!("{where_}.{field}"))?;
    let mut out = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let text = as_string(value, &format!("{where_}.{field}[{index}]"))?;
        out.push(Value::String(remap(text)?));
    }
    item.insert(field.to_string(), Value::Array(out));
    Ok(())
}

fn team_stats(events: &[Value], agent_count: i64) -> Result<Map<String, Value>, String> {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for key in [
        "user_prompts",
        "agent_responses",
        "inter_agent_messages",
        "external_messages",
        "tool_calls",
    ] {
        counts.insert(key, 0);
    }
    for (index, raw) in events.iter().enumerate() {
        let event = as_object(raw, &format!("events[{index}]"))?;
        let kind = as_string(
            event.get("kind").unwrap_or(&Value::Null),
            &format!("events[{index}].kind"),
        )?;
        let key = match kind {
            "user_prompt" => Some("user_prompts"),
            "assistant_message" => Some("agent_responses"),
            "inter_agent_message" => Some("inter_agent_messages"),
            "external_message" => Some("external_messages"),
            "tool_call" => Some("tool_calls"),
            _ => None,
        };
        if let Some(key) = key {
            *counts.get_mut(key).unwrap() += 1;
        }
    }
    let mut out = Map::new();
    out.insert("user_prompts".into(), counts["user_prompts"].into());
    out.insert("agent_responses".into(), counts["agent_responses"].into());
    out.insert(
        "inter_agent_messages".into(),
        counts["inter_agent_messages"].into(),
    );
    out.insert(
        "external_messages".into(),
        counts["external_messages"].into(),
    );
    out.insert("tool_calls".into(), counts["tool_calls"].into());
    out.insert("active_agents".into(), agent_count.into());
    out.insert("events".into(), (events.len() as i64).into());
    Ok(out)
}

fn obj_mut(value: &Value, where_: &str) -> Result<Map<String, Value>, String> {
    Ok(as_object(value, where_)?.clone())
}

fn transform_team(rendered: &RenderedTeam) -> Result<(Map<String, Value>, TeamValues), String> {
    let team_slug = rendered.slug.as_str();
    let timeline = &rendered.timeline;
    let mut values = TeamValues::default();

    let raw_teams = as_array(
        timeline.get("teams").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.teams"),
    )?;
    if raw_teams.len() != 1 {
        return Err(format!(
            "team {team_slug}: rendered timeline must describe exactly one team"
        ));
    }
    let mut team = obj_mut(&raw_teams[0], &format!("{team_slug}.timeline.teams[0]"))?;
    if get_str(&team, "slug", &format!("{team_slug}.timeline.teams[0]"))? != team_slug {
        return Err(format!(
            "team {team_slug}: rendered team slug does not match"
        ));
    }
    team.insert("provider".into(), Value::String(rendered.provider.clone()));

    for (index, raw) in as_array(
        timeline.get("agents").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.agents"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.agents[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let id = get_str(&item, "id", &where_)?;
        item.insert("id".into(), Value::String(namespaced(team_slug, &id)));
        let parent = optional_string(&item, "parent_id", &where_)?.filter(|text| !text.is_empty());
        item.insert(
            "parent_id".into(),
            parent.map_or(Value::Null, |text| {
                Value::String(namespaced(team_slug, &text))
            }),
        );
        item.insert("team".into(), Value::String(team_slug.to_string()));
        remap_string_list(&mut item, "artifact_ids", &where_, &|value| {
            artifact_id(team_slug, value)
        })?;
        remap_string_list(&mut item, "output_artifact_ids", &where_, &|value| {
            artifact_id(team_slug, value)
        })?;
        values.agents.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("phases").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.phases"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.phases[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let phase_id = get_str(&item, "id", &where_)?;
        item.insert("id".into(), Value::String(namespaced(team_slug, &phase_id)));
        let agent_id = get_str(&item, "agent_id", &where_)?;
        item.insert(
            "agent_id".into(),
            Value::String(namespaced(team_slug, &agent_id)),
        );
        item.insert("team".into(), Value::String(team_slug.to_string()));
        let detail_path = get_str(&item, "detail_path", &where_)?;
        let parts = posix_parts(&detail_path);
        let raw_name = detail_path.rsplit('/').next().unwrap_or("");
        if detail_path.starts_with('/')
            || parts.len() != 3
            || parts[0] != "data"
            || parts[1] != "details"
            || raw_name != parts[2]
        {
            return Err(format!("{where_}.detail_path is not a direct detail file"));
        }
        item.insert(
            "detail_path".into(),
            Value::String(format!("data/details/{team_slug}/{}", parts[2])),
        );
        remap_string_list(&mut item, "artifact_ids", &where_, &|value| {
            artifact_id(team_slug, value)
        })?;
        remap_string_list(&mut item, "output_artifact_ids", &where_, &|value| {
            artifact_id(team_slug, value)
        })?;
        values.phases.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("activity_bins").unwrap_or(&EMPTY_ARRAY),
        &format!("{team_slug}.timeline.activity_bins"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.activity_bins[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        if get_str(&item, "team", &where_)? != team_slug {
            return Err(format!("{where_}.team does not match its rendered team"));
        }
        item.insert("team".into(), Value::String(team_slug.to_string()));
        values.activity_bins.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("edges").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.edges"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.edges[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let id = get_str(&item, "id", &where_)?;
        item.insert("id".into(), Value::String(namespaced(team_slug, &id)));
        for field in ["source_id", "target_id"] {
            let value = get_str(&item, field, &where_)?;
            item.insert(field.into(), Value::String(namespaced(team_slug, &value)));
        }
        item.insert("team".into(), Value::String(team_slug.to_string()));
        values.edges.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("events").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.events"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.events[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let agent_id = get_str(&item, "agent_id", &where_)?;
        item.insert(
            "agent_id".into(),
            Value::String(namespaced(team_slug, &agent_id)),
        );
        item.insert("team".into(), Value::String(team_slug.to_string()));
        values.events.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("rollups").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.rollups"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.rollups[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        item.insert("team".into(), Value::String(team_slug.to_string()));
        remap_string_list(&mut item, "artifact_ids", &where_, &|value| {
            artifact_id(team_slug, value)
        })?;
        remap_string_list(&mut item, "output_artifact_ids", &where_, &|value| {
            artifact_id(team_slug, value)
        })?;
        values.rollups.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("summary_files").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.summary_files"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.summary_files[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        item.insert("team".into(), Value::String(team_slug.to_string()));
        values.summary_files.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        timeline.get("glossary").unwrap_or(&Value::Null),
        &format!("{team_slug}.timeline.glossary"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.timeline.glossary[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let raw_id = get_str(&item, "id", &where_)?;
        let term_id = glossary_id(team_slug, &raw_id)?;
        item.insert("id".into(), Value::String(term_id.clone()));
        item.insert("url".into(), Value::String(format!("#glossary/{term_id}")));
        item.insert("team".into(), Value::String(team_slug.to_string()));
        values.glossary.push(Value::Object(item));
    }

    if let Some(overview) = timeline.get("project_overview") {
        if !overview.is_null() {
            let mut item = obj_mut(overview, &format!("{team_slug}.timeline.project_overview"))?;
            item.insert("team".into(), Value::String(team_slug.to_string()));
            values.project_overviews.push(Value::Object(item));
        }
    }

    let stats = team_stats(&values.events, values.agents.len() as i64)?;
    team.insert("stats".into(), Value::Object(stats));
    Ok((team, values))
}

const EMPTY_ARRAY: Value = Value::Array(Vec::new());

fn transform_artifacts(rendered: &RenderedTeam) -> Result<(Vec<Value>, Vec<Value>), String> {
    let team_slug = rendered.slug.as_str();
    let mut artifacts = Vec::new();
    let mut projects = Vec::new();
    for (index, raw) in as_array(
        rendered.artifacts.get("artifacts").unwrap_or(&Value::Null),
        &format!("{team_slug}.artifacts.artifacts"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.artifacts.artifacts[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let raw_id = get_str(&item, "artifact_id", &where_)?;
        item.insert(
            "artifact_id".into(),
            Value::String(artifact_id(team_slug, &raw_id)?),
        );
        let producer =
            optional_string(&item, "producer_thread_id", &where_)?.filter(|text| !text.is_empty());
        item.insert(
            "producer_thread_id".into(),
            producer.map_or(Value::Null, |text| {
                Value::String(namespaced(team_slug, &text))
            }),
        );
        let mut evidence_values = Vec::new();
        for (evidence_index, raw_evidence) in as_array(
            item.get("evidence").unwrap_or(&Value::Null),
            &format!("{where_}.evidence"),
        )?
        .iter()
        .enumerate()
        {
            let evidence_where = format!("{where_}.evidence[{evidence_index}]");
            let mut evidence = obj_mut(raw_evidence, &evidence_where)?;
            for field in ["evidence_id", "source_id", "thread_id"] {
                let value = get_str(&evidence, field, &evidence_where)?;
                evidence.insert(field.into(), Value::String(namespaced(team_slug, &value)));
            }
            let turn = optional_string(&evidence, "turn_id", &evidence_where)?
                .filter(|text| !text.is_empty());
            evidence.insert(
                "turn_id".into(),
                turn.map_or(Value::Null, |text| {
                    Value::String(namespaced(team_slug, &text))
                }),
            );
            evidence_values.push(Value::Object(evidence));
        }
        item.insert("evidence".into(), Value::Array(evidence_values));
        item.insert("team".into(), Value::String(team_slug.to_string()));
        artifacts.push(Value::Object(item));
    }

    for (index, raw) in as_array(
        rendered.artifacts.get("projects").unwrap_or(&Value::Null),
        &format!("{team_slug}.artifacts.projects"),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{team_slug}.artifacts.projects[{index}]");
        let mut item = obj_mut(raw, &where_)?;
        let project_id = get_str(&item, "project_id", &where_)?;
        item.insert(
            "project_id".into(),
            Value::String(namespaced(team_slug, &project_id)),
        );
        remap_string_list(&mut item, "evidence_ids", &where_, &|value| {
            Ok(namespaced(team_slug, value))
        })?;
        item.insert("team".into(), Value::String(team_slug.to_string()));
        projects.push(Value::Object(item));
    }
    Ok((artifacts, projects))
}

fn transform_detail(
    team_slug: &str,
    detail: &Map<String, Value>,
    where_: &str,
) -> Result<Map<String, Value>, String> {
    let mut item = detail.clone();
    item.insert("team".into(), Value::String(team_slug.to_string()));
    remap_string_list(&mut item, "artifact_ids", where_, &|value| {
        artifact_id(team_slug, value)
    })?;
    remap_string_list(&mut item, "output_artifact_ids", where_, &|value| {
        artifact_id(team_slug, value)
    })?;
    Ok(item)
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn combined_digest(rendered_teams: &[RenderedTeam]) -> Result<String, String> {
    let mut source_values = Vec::with_capacity(rendered_teams.len());
    for rendered in rendered_teams {
        let digest = as_string(
            rendered
                .timeline
                .get("source_digest")
                .unwrap_or(&Value::Null),
            &format!("{}.timeline.source_digest", rendered.slug),
        )?;
        let mut entry = Map::new();
        entry.insert("team".into(), Value::String(rendered.slug.clone()));
        entry.insert("source_digest".into(), Value::String(digest.to_string()));
        source_values.push(Value::Object(entry));
    }
    Ok(sha256_hex(&dump_canonical(&Value::Array(source_values))))
}

/// PurePosixPath parts: empty and `.` components collapse away.
fn posix_parts(raw: &str) -> Vec<&str> {
    raw.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

fn posix_suffix(raw: &str) -> &str {
    let name = raw.rsplit('/').next().unwrap_or("");
    if name.starts_with('.') && !name[1..].contains('.') {
        return "";
    }
    match name.rfind('.') {
        Some(index) => &name[index..],
        None => "",
    }
}

fn is_hex64_json(name: &str) -> bool {
    name.len() == 69
        && name.ends_with(".json")
        && name[..64]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_generated_path(raw: &str) -> Result<Vec<String>, String> {
    let parts = posix_parts(raw);
    if raw.starts_with('/') || parts.is_empty() || parts.contains(&"..") {
        return Err(format!(
            "unsafe generated path in export manifest: {}",
            crate::json::py_repr_str(raw)
        ));
    }
    // The oracle validates the normalized PurePosixPath, so a
    // trailing slash or `./` component names the same file.
    let normalized = parts.join("/");
    if crate::timeline_v3::is_schema_3_path(&normalized) {
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    if let Some(base) = normalized.strip_suffix(".gz") {
        if base.ends_with(".gz") {
            return Err(format!(
                "nested gzip sidecar in export manifest: {}",
                crate::json::py_repr_str(raw)
            ));
        }
        safe_generated_path(base)?;
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    if COMMON_FILES.contains(&normalized.as_str())
        || normalized == "query.py"
        || normalized == "README.md"
        || normalized == EXPORT_MANIFEST
    {
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    if parts.len() >= 4
        && parts[0] == "teams"
        && parts[2] == "summaries"
        && posix_suffix(&normalized) == ".md"
    {
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    if parts.len() == 4
        && parts[0] == "data"
        && parts[1] == "details"
        && posix_suffix(&normalized) == ".json"
    {
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    if matches!(
        normalized.as_str(),
        "data/timeline.json"
            | "data/timeline-v2.json"
            | "data/timeline-v2/manifest.json"
            | "data/artifacts.json"
    ) {
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    if parts.len() == 4
        && parts[0] == "data"
        && parts[1] == "timeline-v2"
        && parts[2] == "objects"
        && is_hex64_json(parts[3])
    {
        return Ok(parts.iter().map(|part| part.to_string()).collect());
    }
    Err(format!(
        "unrecognized generated path in export manifest: {}",
        crate::json::py_repr_str(raw)
    ))
}

/// `Path.resolve()` parity: canonicalize what exists, then
/// apply the remaining `.`/`..` components lexically, so
/// aliased paths collapse to one resolved spelling.
fn resolve_lenient(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    if let Ok(resolved) = absolute.canonicalize() {
        return normalize_absolute(&resolved);
    }
    // Canonicalize the deepest existing ancestor, then re-append.
    let mut existing = absolute.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                existing = existing
                    .parent()
                    .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
            }
            None => break,
        }
    }
    let mut resolved = existing.canonicalize().unwrap_or(existing);
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    normalize_absolute(&resolved)
}

fn normalize_absolute(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn output_path(output: &Path, raw: &str) -> Result<PathBuf, String> {
    let parts = safe_generated_path(raw)?;
    let root = resolve_lenient(output);
    let mut cursor = output.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        cursor.push(part);
        if is_symlink(&cursor) {
            return Err(format!(
                "generated output path crosses a symlink: {}",
                crate::json::py_repr_str(raw)
            ));
        }
    }
    let mut candidate = output.to_path_buf();
    for part in &parts {
        candidate.push(part);
    }
    let parent = candidate
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    if !resolve_lenient(&parent).starts_with(&root) {
        return Err(format!(
            "generated output path escapes through a symlink: {}",
            crate::json::py_repr_str(raw)
        ));
    }
    if is_symlink(&candidate) {
        return Err(format!(
            "refusing generated output symlink: {}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

fn previous_generated_files(output: &Path) -> Result<BTreeSet<String>, String> {
    let path = output_path(output, EXPORT_MANIFEST)?;
    if !path.is_file() {
        return Ok(BTreeSet::new());
    }
    let label = path.display().to_string();
    let value = read_json(&path)?;
    let root = as_object(&value, &label)?;
    if !crate::json::json_eq_one(root.get("schema_version"))
        || root.get("kind") != Some(&Value::String("multi-team-export".to_string()))
    {
        return Err(format!("unsupported multi-team export manifest at {label}"));
    }
    let mut values = BTreeSet::new();
    for (index, raw) in as_array(
        root.get("generated_files").unwrap_or(&Value::Null),
        &format!("{label}.generated_files"),
    )?
    .iter()
    .enumerate()
    {
        let entry = as_string(raw, &format!("{label}.generated_files[{index}]"))?;
        safe_generated_path(entry)?;
        values.insert(entry.to_string());
    }
    Ok(values)
}

fn remove_stale_files(
    output: &Path,
    previous: &BTreeSet<String>,
    current: &BTreeSet<String>,
) -> Result<i64, String> {
    let mut changed = 0i64;
    for raw in previous.difference(current) {
        safe_generated_path(raw)?;
        let path = output_path(output, raw)?;
        if path.is_file() || is_symlink(&path) {
            std::fs::remove_file(&path).map_err(|error| os_error(&error, &path))?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn copy_text_file(source: &Path, target: &Path) -> Result<i64, String> {
    if !source.is_file() {
        return Err(format!(
            "rendered team output is missing {}",
            source.display()
        ));
    }
    let text = std::fs::read_to_string(source).map_err(|error| os_error(&error, source))?;
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(source)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    #[cfg(not(unix))]
    let executable = false;
    crate::build_assets::write_text_with_gzip_invalidation_executable(target, &text, executable)
}

fn write_gzip_only_json(path: &Path, value: &Value) -> Result<i64, String> {
    crate::build_assets::write_gzip_only(path, &dump_canonical(value))
}

fn read_json_stored(path: &Path) -> Result<Value, String> {
    if path.is_file() {
        return read_json(path);
    }
    let sidecar = path.with_file_name(format!(
        "{}.gz",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if !sidecar.is_file() {
        return Err(format!("no such file: {}", path.display()));
    }
    let bytes = std::fs::read(&sidecar).map_err(|error| os_error(&error, &sidecar))?;
    let text = {
        use std::io::Read as _;
        let mut decoder = flate2::read::MultiGzDecoder::new(bytes.as_slice());
        let mut text = String::new();
        decoder
            .read_to_string(&mut text)
            .map_err(|error| os_error(&error, &sidecar))?;
        text
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))?;
    Ok(value)
}

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_root() -> Result<TempRoot, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    loop {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("wrkviz-combine-{}-{serial}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(TempRoot(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(os_error(&error, &path)),
        }
    }
}

fn collect_team_files(root: &Path) -> Result<Vec<(Vec<String>, PathBuf)>, String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(Vec<String>, PathBuf)>) -> Result<(), String> {
        let mut entries: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(|error| os_error(&error, dir))? {
            let entry = entry.map_err(|error| os_error(&error, dir))?;
            entries.push(entry.path());
        }
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, base, out)?;
            } else if path.is_file() {
                let relative = path.strip_prefix(base).unwrap_or(&path);
                let parts: Vec<String> = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.push((parts, path.clone()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn build_combined_archive_locked(
    archive: &Path,
    team_slugs: &[String],
    output: &Path,
    display_timezone: &str,
    phase_minutes: i64,
    display_window: Option<&DateWindow>,
    rollup_kinds: &[String],
) -> Result<CombinedReport, String> {
    let mut ordered_slugs: Vec<String> = team_slugs.to_vec();
    ordered_slugs.sort();
    if ordered_slugs.len() < 2 {
        return Err("combined export requires at least two teams".to_string());
    }
    if ordered_slugs.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("combined export team slugs must be unique".to_string());
    }
    crate::pipeline::ensure_archive_pub(output, &ordered_slugs[0], true)?;
    let previous_files = previous_generated_files(output)?;

    let temp = temp_root()?;
    let temporary_root = temp.0.clone();
    let mut rendered_teams: Vec<RenderedTeam> = Vec::new();
    for team_slug in &ordered_slugs {
        let team = crate::pipeline::load_archived_team(archive, team_slug)?;
        let team_root = temporary_root.join(team_slug);
        crate::build_pipeline::build_archive_locked(
            archive,
            team_slug,
            phase_minutes,
            display_window,
            rollup_kinds,
            Some(&team_root),
            false,
            false,
        )?;
        let timeline_path = team_root.join("data").join("timeline.json");
        let artifacts_path = team_root.join("data").join("artifacts.json");
        let timeline_label = timeline_path.display().to_string();
        let artifacts_label = artifacts_path.display().to_string();
        let timeline = obj_mut(&read_json(&timeline_path)?, &timeline_label)?;
        let artifacts = obj_mut(&read_json_stored(&artifacts_path)?, &artifacts_label)?;
        rendered_teams.push(RenderedTeam {
            slug: team_slug.clone(),
            provider: team.provider.clone(),
            team,
            root: team_root,
            timeline,
            artifacts,
        });
    }

    let mut merged = Merged::default();
    let mut generated_files: BTreeSet<String> =
        COMMON_FILES.iter().map(|item| item.to_string()).collect();
    generated_files.insert("README.md".to_string());
    generated_files.insert(EXPORT_MANIFEST.to_string());
    let mut compressible_files: BTreeSet<String> = COMMON_FILES
        .iter()
        .filter(|item| matches!(posix_suffix(item), ".css" | ".html" | ".js"))
        .map(|item| item.to_string())
        .collect();
    let mut gzip_only_files: BTreeSet<String> = BTreeSet::new();
    let mut changed: i64 = 0;

    let first_root = rendered_teams[0].root.clone();
    for relative in COMMON_FILES {
        changed += copy_text_file(&first_root.join(relative), &output_path(output, relative)?)?;
    }

    for rendered in &rendered_teams {
        let (team_record, values) = transform_team(rendered)?;
        merged.teams.push(Value::Object(team_record));
        merged.agents.extend(values.agents);
        merged.phases.extend(values.phases);
        merged.activity_bins.extend(values.activity_bins);
        merged.edges.extend(values.edges);
        merged.events.extend(values.events);
        merged.rollups.extend(values.rollups);
        merged.summary_files.extend(values.summary_files);
        merged.glossary.extend(values.glossary);
        merged.project_overviews.extend(values.project_overviews);
        let (artifacts, projects) = transform_artifacts(rendered)?;
        merged.artifacts.extend(artifacts);
        merged.projects.extend(projects);

        for (parts, source) in collect_files_under(
            &rendered.root.join("teams").join(&rendered.slug),
            &rendered.slug,
        )? {
            let relative = parts.join("/");
            if relative.ends_with(".gz") {
                continue;
            }
            safe_generated_path(&relative)?;
            generated_files.insert(relative.clone());
            if posix_suffix(&relative) == ".md" {
                compressible_files.insert(relative.clone());
            }
            changed += copy_text_file(&source, &output_path(output, &relative)?)?;
        }

        let source_phases = as_array(
            rendered.timeline.get("phases").unwrap_or(&Value::Null),
            &format!("{}.timeline.phases", rendered.slug),
        )?
        .clone();
        for (index, raw_phase) in source_phases.iter().enumerate() {
            let where_ = format!("{}.timeline.phases[{index}]", rendered.slug);
            let phase = obj_mut(raw_phase, &where_)?;
            let source_relative = get_str(&phase, "detail_path", &where_)?;
            let source_path = rendered.root.join(&source_relative);
            let detail_label = source_path.display().to_string();
            let detail = obj_mut(&read_json_stored(&source_path)?, &detail_label)?;
            let name = posix_parts(&source_relative)
                .last()
                .map(|part| part.to_string())
                .unwrap_or_default();
            let target_relative = format!("data/details/{}/{name}", rendered.slug);
            safe_generated_path(&target_relative)?;
            gzip_only_files.insert(target_relative.clone());
            changed += write_gzip_only_json(
                &output_path(output, &target_relative)?,
                &Value::Object(transform_detail(&rendered.slug, &detail, &detail_label)?),
            )?;
        }
    }

    let mut start_ms: Option<i64> = None;
    let mut end_ms: Option<i64> = None;
    for rendered in &rendered_teams {
        let range = obj_mut(
            rendered.timeline.get("range").unwrap_or(&Value::Null),
            &format!("{}.timeline.range", rendered.slug),
        )?;
        let start = as_int(
            range.get("start_ms").unwrap_or(&Value::Null),
            "timeline.range.start_ms",
        )?;
        let end = as_int(
            range.get("end_ms").unwrap_or(&Value::Null),
            "timeline.range.end_ms",
        )?;
        start_ms = Some(start_ms.map_or(start, |current: i64| current.min(start)));
        end_ms = Some(end_ms.map_or(end, |current: i64| current.max(end)));
    }
    let start_ms = start_ms.unwrap_or(0);
    let end_ms = end_ms.unwrap_or(0);
    let mut generated_at = String::new();
    for rendered in &rendered_teams {
        let value = as_string(
            rendered
                .timeline
                .get("generated_at")
                .unwrap_or(&Value::Null),
            &format!("{}.timeline.generated_at", rendered.slug),
        )?;
        if value > generated_at.as_str() {
            generated_at = value.to_string();
        }
    }
    let total_stats = team_stats(&merged.events, merged.agents.len() as i64)?;
    let source_digest = combined_digest(&rendered_teams)?;

    let mut timeline = Map::new();
    timeline.insert("schema_version".into(), 1.into());
    timeline.insert("generated_at".into(), Value::String(generated_at));
    timeline.insert("source_digest".into(), Value::String(source_digest.clone()));
    timeline.insert(
        "display_timezone".into(),
        Value::String(display_timezone.to_string()),
    );
    timeline.insert(
        "display_timezone_source".into(),
        Value::String("combined_export".to_string()),
    );
    let mut range = Map::new();
    range.insert("start_ms".into(), start_ms.into());
    range.insert("end_ms".into(), end_ms.into());
    timeline.insert("range".into(), Value::Object(range));
    timeline.insert("teams".into(), Value::Array(merged.teams.clone()));
    timeline.insert("agents".into(), Value::Array(merged.agents.clone()));
    timeline.insert("phases".into(), Value::Array(merged.phases.clone()));
    timeline.insert(
        "activity_bins".into(),
        Value::Array(merged.activity_bins.clone()),
    );
    timeline.insert("edges".into(), Value::Array(merged.edges.clone()));
    timeline.insert("events".into(), Value::Array(merged.events.clone()));
    timeline.insert("rollups".into(), Value::Array(merged.rollups.clone()));
    timeline.insert("glossary".into(), Value::Array(merged.glossary.clone()));
    timeline.insert("glossary_path".into(), Value::String(String::new()));
    timeline.insert(
        "project_overviews".into(),
        Value::Array(merged.project_overviews.clone()),
    );
    timeline.insert(
        "summary_files".into(),
        Value::Array(merged.summary_files.clone()),
    );
    timeline.insert(
        "artifact_catalog_path".into(),
        Value::String("data/artifacts.json".to_string()),
    );
    timeline.insert("projects".into(), Value::Array(merged.projects.clone()));
    timeline.insert("stats".into(), Value::Object(total_stats));
    let timeline = Value::Object(timeline);

    let mut artifact_catalog = Map::new();
    artifact_catalog.insert("schema_version".into(), 1.into());
    artifact_catalog.insert(
        "extractor_version".into(),
        Value::String("multi-team-export-v1".to_string()),
    );
    artifact_catalog.insert("source_digest".into(), Value::String(source_digest.clone()));
    artifact_catalog.insert(
        "teams".into(),
        Value::Array(
            ordered_slugs
                .iter()
                .map(|slug| Value::String(slug.clone()))
                .collect(),
        ),
    );
    artifact_catalog.insert("artifacts".into(), Value::Array(merged.artifacts.clone()));
    artifact_catalog.insert("projects".into(), Value::Array(merged.projects.clone()));
    changed += write_gzip_only_json(
        &output_path(output, "data/artifacts.json")?,
        &Value::Object(artifact_catalog),
    )?;
    gzip_only_files.insert("data/artifacts.json".to_string());

    let mut search_records: Vec<Value> = Vec::new();
    for rendered in &rendered_teams {
        let rendered_range = obj_mut(
            rendered.timeline.get("range").unwrap_or(&Value::Null),
            &format!("{}.timeline.range", rendered.slug),
        )?;
        let mut visible_agent_ids: BTreeSet<String> = BTreeSet::new();
        for raw_agent in as_array(
            rendered.timeline.get("agents").unwrap_or(&Value::Null),
            &format!("{}.timeline.agents", rendered.slug),
        )? {
            let agent = as_object(raw_agent, "timeline agent")?;
            let id = as_string(agent.get("id").unwrap_or(&Value::Null), "timeline agent.id")?;
            visible_agent_ids.insert(id.to_string());
        }
        let team_start = as_int(
            rendered_range.get("start_ms").unwrap_or(&Value::Null),
            &format!("{}.timeline.range.start_ms", rendered.slug),
        )?;
        let team_end = as_int(
            rendered_range.get("end_ms").unwrap_or(&Value::Null),
            &format!("{}.timeline.range.end_ms", rendered.slug),
        )?;
        search_records.extend(crate::search_index::build_search_records(
            &rendered.team,
            &visible_agent_ids,
            true,
            Some(team_start),
            Some(team_end),
        )?);
    }

    let readme = crate::build_render::archive_readme(
        "Combined agent-team timeline",
        &format!(
            "Teams: {}\n\nThis directory is a self-contained, zero-token export of cached summaries.",
            ordered_slugs
                .iter()
                .map(|slug| format!("`{slug}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "The requested export slice is recorded in `data/export.json` under `display_window`; `./timeline` reports the actual team and record intervals. Do not infer the slice from file modification times.",
    );
    changed += i64::from(crate::transcript_io::write_text_if_changed(
        &output_path(output, "README.md")?,
        &readme,
    )?);

    for relative in &compressible_files {
        let source_path = output_path(output, relative)?;
        changed += i64::from(crate::build_assets::sync_gzip_sidecar(&source_path)?);
        let sidecar = crate::build_assets::gzip_sidecar_path(&source_path);
        if sidecar.is_file() {
            let sidecar_relative = format!("{relative}.gz");
            safe_generated_path(&sidecar_relative)?;
            generated_files.insert(sidecar_relative);
        }
    }
    for relative in &gzip_only_files {
        let sidecar_relative = format!("{relative}.gz");
        safe_generated_path(&sidecar_relative)?;
        generated_files.insert(sidecar_relative);
    }

    let v3_report = crate::timeline_v3::write_timeline_v3(output, &timeline, &search_records)?;
    changed += v3_report.files_changed;
    for relative in &v3_report.generated_files {
        safe_generated_path(relative)?;
        generated_files.insert(relative.clone());
    }
    let retired_files = crate::build_render::retired_generation_files(output, &generated_files);
    for relative in &retired_files {
        safe_generated_path(relative)?;
    }
    let mut export_manifest = Map::new();
    export_manifest.insert("schema_version".into(), 1.into());
    export_manifest.insert(
        "kind".into(),
        Value::String("multi-team-export".to_string()),
    );
    export_manifest.insert(
        "teams".into(),
        Value::Array(
            ordered_slugs
                .iter()
                .map(|slug| Value::String(slug.clone()))
                .collect(),
        ),
    );
    export_manifest.insert(
        "display_timezone".into(),
        Value::String(display_timezone.to_string()),
    );
    export_manifest.insert(
        "display_window".into(),
        display_window.map_or(Value::Null, |window| window.to_json()),
    );
    export_manifest.insert(
        "rollup_kinds".into(),
        Value::Array(
            rollup_kinds
                .iter()
                .map(|kind| Value::String(kind.clone()))
                .collect(),
        ),
    );
    export_manifest.insert("source_digest".into(), Value::String(source_digest));
    export_manifest.insert(
        "generated_files".into(),
        Value::Array(
            generated_files
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    export_manifest.insert(
        "retired_files".into(),
        Value::Array(
            retired_files
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    let retired_query_was_managed = previous_files.contains("query.py");
    let stale: BTreeSet<String> = previous_files.difference(&retired_files).cloned().collect();
    changed += remove_stale_files(output, &stale, &generated_files)?;
    changed +=
        crate::build_render::prune_retired_query_artifacts(output, retired_query_was_managed)?;
    changed += i64::from(crate::transcript_io::write_json_if_changed(
        &output_path(output, EXPORT_MANIFEST)?,
        &Value::Object(export_manifest),
    )?);

    Ok(CombinedReport {
        files_changed: changed,
        teams: ordered_slugs.len() as i64,
        phases: merged.phases.len() as i64,
        activity_bins: merged.activity_bins.len() as i64,
        agents: merged.agents.len() as i64,
        edges: merged.edges.len() as i64,
        events: merged.events.len() as i64,
        rollups: merged.rollups.len() as i64,
        summary_files: merged.summary_files.len() as i64,
        artifacts: merged.artifacts.len() as i64,
        projects: merged.projects.len() as i64,
        detail_shards: v3_report.timeline_shards,
        search_records: search_records.len() as i64,
        search_shards: v3_report.search_shards,
        bootstrap_bytes: v3_report.bootstrap_bytes as i64,
        bootstrap_transfer_bytes: v3_report.bootstrap_bytes as i64,
        shard_object_bytes: (v3_report.compressed_bytes + v3_report.index_bytes) as i64,
        shard_transfer_bytes: (v3_report.compressed_bytes + v3_report.index_bytes) as i64,
    })
}

fn collect_files_under(dir: &Path, slug: &str) -> Result<Vec<(Vec<String>, PathBuf)>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    collect_team_files(dir).map(|items| {
        items
            .into_iter()
            .map(|(parts, path)| {
                let mut full = vec!["teams".to_string(), slug.to_string()];
                full.extend(parts);
                (full, path)
            })
            .collect()
    })
}

/// Build a combined site while serializing its source snapshot
/// and target.
#[allow(clippy::too_many_arguments)]
pub fn build_combined_archive(
    archive: &Path,
    team_slugs: &[String],
    output: &Path,
    display_timezone: &str,
    phase_minutes: i64,
    display_window: Option<&DateWindow>,
    rollup_kinds: &[String],
) -> Result<CombinedReport, String> {
    let archive_resolved = resolve_lenient(archive);
    let output_resolved = resolve_lenient(output);
    let mut lock_paths: Vec<PathBuf> = vec![archive_resolved.clone()];
    if output_resolved != archive_resolved {
        lock_paths.push(output_resolved);
    }
    lock_paths.sort_by_key(|left| left.display().to_string());
    lock_paths.dedup();
    let mut _locks = Vec::new();
    for path in &lock_paths {
        _locks.push(crate::pipeline::archive_writer_lock(path)?);
    }
    build_combined_archive_locked(
        archive,
        team_slugs,
        output,
        display_timezone,
        phase_minutes,
        display_window,
        rollup_kinds,
    )
}
