//! Presentation rendering at parity with `py/wrkviz/render.py`:
//! Markdown summaries, per-phase detail documents, the schema-1
//! timeline object, static assets, and stale-presentation retirement.
//! Schema 3 is delegated to `timeline_v3`; schema 2 is retired
//! (never written, never deleted).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::activity_bins::build_activity_bins;
use crate::artifacts::{ArtifactCatalog, ArtifactRangeIndex};
use crate::build_assets::{
    find_pull_request_references, glossary_catalog_markdown, glossary_markdown, gzip_sidecar_path,
    sync_gzip_sidecar, write_gzip_only, write_text_with_gzip_invalidation, GlossaryTerm,
    PullRequestMetadata,
};
use crate::identity::SiteIdentity;
use crate::json::{as_array, as_object, as_string, dump_canonical, py_repr_str, read_json};
use crate::naming::AgentNameResult;
use crate::search_index::build_search_records;
use crate::summarize_core::{clean_summary_prose, SummaryResult};
use crate::summary_periods::{period_heading, Period};
use crate::summary_phases::{PhaseStats, PhaseWindow, TranscriptEntry};
use crate::team_data::source_digest;
use crate::team_data::{Agent, Edge, Event, TeamData};
use crate::timeline_v3::{is_schema_3_path, write_timeline_v3};
use crate::transcript_io::{write_text_if_changed, write_text_if_changed_executable};

const SCHEMA_1_TIMELINE_PATH: &str = "data/timeline.json";
const SCHEMA_2_BOOTSTRAP_PATH: &str = "data/timeline-v2/bootstrap.json";
const SCHEMA_2_PRESENTATION_ROOT: &str = "data/timeline-v2";
const PRESENTATION_MANIFEST: &str = "data/export.json";

const STATIC_INDEX: &str = include_str!("../../../py/wrkviz/static/index.html");
const STATIC_CORE: &str = include_str!("../../../py/wrkviz/static/timeline-core.js");
const STATIC_APP: &str = include_str!("../../../py/wrkviz/static/app.js");
const STATIC_CSS: &str = include_str!("../../../py/wrkviz/static/style.css");
const VENDOR_README: &str = include_str!("../../../py/wrkviz/static/vendor/README.md");
const VENDOR_JS: &str = include_str!("../../../py/wrkviz/static/vendor/markdown-it-15.0.0.min.js");
const VENDOR_LICENSE: &str =
    include_str!("../../../py/wrkviz/static/vendor/markdown-it-LICENSE.txt");
const STANDALONE_SERVER: &str = include_str!("../../../py/wrkviz/standalone_server.py");
const RUN_STATS: &str = include_str!("../../../py/wrkviz/run_stats.py");
const STANDALONE_QUERY: &str = include_str!("../../../py/wrkviz/query.py");

fn presentation_common() -> BTreeSet<String> {
    [
        "index.html",
        "timeline-core.js",
        "app.js",
        "style.css",
        "serve.py",
        "run_stats.py",
        "timeline",
        "Makefile",
        "README.md",
        "vendor/README.md",
        "vendor/markdown-it-15.0.0.min.js",
        "vendor/markdown-it-LICENSE.txt",
    ]
    .iter()
    .map(|item| item.to_string())
    .collect()
}

/// Inputs render_archive consumes (all cache-derived; zero tokens).
pub struct RenderInputs {
    /// Team.
    pub team: TeamData,
    /// Phases.
    pub phases: Vec<PhaseWindow>,
    /// Phase summaries by summary key.
    pub phase_summaries: HashMap<String, SummaryResult>,
    /// Periods.
    pub periods: Vec<Period>,
    /// Technical rollups by period key.
    pub rollup_summaries: HashMap<String, SummaryResult>,
    /// Plain rollups by period key.
    pub plain_rollup_summaries: HashMap<String, SummaryResult>,
    /// Rollup stats by period key.
    pub rollup_stats: HashMap<String, PhaseStats>,
    /// Glossary terms.
    pub glossary_terms: Vec<GlossaryTerm>,
    /// Project overview.
    pub project_overview: SummaryResult,
    /// Agent names by thread id.
    pub agent_names: HashMap<String, AgentNameResult>,
    /// Pull metadata by (repository, number).
    pub pull_metadata: HashMap<(String, i64), PullRequestMetadata>,
    /// Artifact catalog.
    pub artifact_catalog: ArtifactCatalog,
    /// Site identity.
    pub site_identity: SiteIdentity,
    /// Published render (schema 3).
    pub published: bool,
    /// Precompress.
    pub precompress: bool,
    /// Export window JSON for the manifest (export slice only).
    pub export_window: Option<Value>,
    /// Namespace agents in search records (multi-team export only).
    pub namespace_agents: bool,
}

/// Build statistics (render_archive's report values).
#[derive(Clone, Debug, Default)]
pub struct BuildReport {
    /// Files changed.
    pub files_changed: i64,
    /// Phases.
    pub phases: i64,
    /// Activity bins.
    pub activity_bins: i64,
    /// Agents.
    pub agents: i64,
    /// Edges.
    pub edges: i64,
    /// Rollups.
    pub rollups: i64,
    /// Summary files.
    pub summary_files: i64,
    /// Artifacts.
    pub artifacts: i64,
    /// Projects.
    pub projects: i64,
    /// Detail shards.
    pub detail_shards: i64,
    /// Search records.
    pub search_records: i64,
    /// Search shards.
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

impl BuildReport {
    /// Persisted report form (sorted keys at serialization).
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("files_changed".into(), self.files_changed.into());
        out.insert("phases".into(), self.phases.into());
        out.insert("activity_bins".into(), self.activity_bins.into());
        out.insert("agents".into(), self.agents.into());
        out.insert("edges".into(), self.edges.into());
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

fn iso_ms(at_ms: i64) -> String {
    use chrono::TimeZone as _;
    let dt = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    if dt.timestamp_subsec_millis() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    }
}

fn local_time(at_ms: i64, display_timezone: &str) -> String {
    use chrono::TimeZone as _;
    let Ok(zone) = display_timezone.parse::<chrono_tz::Tz>() else {
        return iso_ms(at_ms);
    };
    let utc = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    utc.with_timezone(&zone)
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string()
}

fn rstrip(text: &str) -> String {
    text.trim_end().to_string()
}

fn generated_path(archive: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let candidate = archive.join(relative_path);
    let root = std::fs::canonicalize(archive).unwrap_or_else(|_| archive.to_path_buf());
    if let Some(parent) = candidate.parent() {
        if parent.exists() {
            let parent_real =
                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            if !parent_real.starts_with(&root) {
                return Err(format!(
                    "generated presentation path escapes archive: {}",
                    py_repr_str(relative_path)
                ));
            }
        }
    }
    if candidate
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refusing generated presentation symlink: {}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

fn remove_generated_file(archive: &Path, relative_path: &str) -> Result<i64, String> {
    let path = generated_path(archive, relative_path)?;
    let sidecar = gzip_sidecar_path(&path);
    if sidecar
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refusing symlinked gzip sidecar: {}",
            sidecar.display()
        ));
    }
    let mut changed = 0i64;
    if !path.exists() {
        if sidecar.is_file() {
            std::fs::remove_file(&sidecar)
                .map_err(|error| crate::json::os_error(&error, &sidecar))?;
            return Ok(1);
        }
        return Ok(0);
    }
    if !path.is_file() {
        return Err(format!(
            "generated presentation path is not a file: {}",
            path.display()
        ));
    }
    std::fs::remove_file(&path).map_err(|error| crate::json::os_error(&error, &path))?;
    changed += 1;
    if sidecar.is_file() {
        std::fs::remove_file(&sidecar).map_err(|error| crate::json::os_error(&error, &sidecar))?;
        changed += 1;
    }
    Ok(changed)
}

fn in_window(team: &TeamData, timestamp_ms: i64) -> bool {
    if let Some(start) = team.window_start_ms {
        if timestamp_ms < start {
            return false;
        }
    }
    match team.window_end_ms {
        None => true,
        Some(end) => timestamp_ms < end,
    }
}

fn time_range(team: &TeamData, phases: &[PhaseWindow]) -> (i64, i64) {
    let mut values: Vec<i64> = Vec::new();
    values.extend(phases.iter().map(|phase| phase.start_ms));
    values.extend(phases.iter().map(|phase| phase.end_ms));
    for agent in &team.agents {
        values.push(agent.started_at_ms);
        if let Some(end) = agent.ended_at_ms {
            values.push(end);
        }
    }
    for turn in &team.turns {
        values.push(turn.started_at_ms);
        if let Some(end) = turn.ended_at_ms {
            values.push(end);
        }
    }
    values.extend(team.events.iter().map(|event| event.timestamp_ms));
    for tool in &team.tool_calls {
        values.push(tool.started_at_ms);
        if let Some(end) = tool.ended_at_ms {
            values.push(end);
        }
    }
    values.extend(team.edges.iter().map(|edge| edge.timestamp_ms));
    if let Some(start) = team.window_start_ms {
        values.push(start);
    }
    if let Some(end) = team.window_end_ms {
        values.push(end);
    }
    if values.is_empty() {
        return (0, 1000);
    }
    let start = team
        .window_start_ms
        .unwrap_or_else(|| *values.iter().min().unwrap());
    let end = team
        .window_end_ms
        .unwrap_or_else(|| *values.iter().max().unwrap());
    (start, (start + 1000).max(end))
}

type PullMap = HashMap<(String, i64), PullRequestMetadata>;

fn pull_request_references(text: &str, metadata: &PullMap) -> Vec<Value> {
    let mut result = Vec::new();
    for reference in find_pull_request_references(text) {
        let mut item = serde_json::Map::new();
        item.insert("start".into(), reference.start.into());
        item.insert("end".into(), reference.end.into());
        item.insert("text".into(), Value::String(reference.text.clone()));
        item.insert("kind".into(), Value::String(reference.kind.into()));
        item.insert(
            "repository".into(),
            Value::String(reference.repository.clone()),
        );
        item.insert("number".into(), reference.number.into());
        item.insert("url".into(), Value::String(reference.url()));
        if let Some(pull) = metadata.get(&(reference.repository.clone(), reference.number)) {
            item.insert("title".into(), Value::String(pull.title.clone()));
            item.insert("state".into(), Value::String(pull.state.clone()));
            item.insert("draft".into(), Value::Bool(pull.draft));
            item.insert(
                "merged_at".into(),
                pull.merged_at.clone().map_or(Value::Null, Value::String),
            );
            item.insert(
                "body_excerpt".into(),
                Value::String(pull.body_excerpt.clone()),
            );
            item.insert("base_ref".into(), Value::String(pull.base_ref.clone()));
            item.insert("head_label".into(), Value::String(pull.head_label.clone()));
            item.insert(
                "author".into(),
                pull.author.clone().map_or(Value::Null, Value::String),
            );
            item.insert("updated_at".into(), Value::String(pull.updated_at.clone()));
        }
        result.push(Value::Object(item));
    }
    result
}

fn agent_name<'a>(
    agent: &Agent,
    names: &'a HashMap<String, AgentNameResult>,
) -> Result<&'a AgentNameResult, String> {
    names
        .get(&agent.thread_id)
        .ok_or_else(|| format!("missing hindsight name for agent {}", agent.agent_path))
}

fn official_leaf(agent: &Agent) -> String {
    let leaf = agent
        .agent_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if leaf.is_empty() {
        "root".into()
    } else {
        leaf.into()
    }
}

fn agent_summary_available(result: &AgentNameResult) -> bool {
    result.summary_available
        && !clean_summary_prose(result.lifetime_summary.as_deref().unwrap_or_default()).is_empty()
}

fn agent_identity_obj(agent: &Agent, name: &AgentNameResult) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("short_name".into(), Value::String(name.short_name.clone()));
    obj.insert(
        "official_name".into(),
        Value::String(agent.agent_path.clone()),
    );
    obj.insert("official_leaf".into(), Value::String(official_leaf(agent)));
    obj.insert(
        "coordinator_nickname".into(),
        Value::String(agent.nickname.clone().unwrap_or_default()),
    );
    obj.insert(
        "naming_rationale".into(),
        Value::String(name.rationale.clone()),
    );
    obj.insert(
        "lifetime_summary".into(),
        Value::String(clean_summary_prose(
            name.lifetime_summary.as_deref().unwrap_or_default(),
        )),
    );
    obj.insert("naming_model".into(), Value::String(name.model.clone()));
    obj.insert(
        "naming_input_hash".into(),
        Value::String(name.input_hash.clone()),
    );
    obj.insert(
        "summary_available".into(),
        Value::Bool(agent_summary_available(name)),
    );
    Value::Object(obj)
}

fn phase_markdown(
    team: &TeamData,
    agent: &Agent,
    name: &AgentNameResult,
    phase: &PhaseWindow,
    summary: &SummaryResult,
) -> String {
    let stats = &phase.stats;
    let tz = &team.display_timezone;
    let mut lines: Vec<String> = vec![
        format!("# {}", summary.phrase),
        String::new(),
        format!("Team: `{}`  ", team.team_slug),
        format!("Agent: `{}`  ", name.short_name),
        format!("Official agent path: `{}`  ", agent.agent_path),
        format!(
            "Window: {} to {}",
            local_time(phase.start_ms, tz),
            local_time(phase.end_ms, tz)
        ),
        String::new(),
        format!("> {}", summary.paragraph),
        String::new(),
        "## Agent work summary".into(),
        String::new(),
    ];
    if !summary.work_summary.is_empty() {
        lines.extend(
            summary
                .work_summary
                .iter()
                .map(|item| format!("- **{}** — {}", local_time(item.at_ms, tz), item.text)),
        );
    } else {
        lines.push("_No substantive work item survived the archival filter._".into());
    }
    lines.extend([
        String::new(),
        "## Window statistics".into(),
        String::new(),
        format!("- User prompts: {}", stats.user_prompts),
        format!("- Agent responses: {}", stats.agent_responses),
        format!("- Inter-agent messages: {}", stats.inter_agent_messages),
    ]);
    if stats.external_messages != 0 {
        lines.push(format!("- External messages: {}", stats.external_messages));
    }
    lines.extend([
        format!("- Tool calls: {}", stats.tool_calls),
        String::new(),
        "## Summary provenance".into(),
        String::new(),
        format!("- Model: `{}`", summary.model),
        format!("- Prompt: `{}`", summary.prompt_version),
        format!("- Input hash: `{}`", summary.input_hash),
    ]);
    format!("{}\n", rstrip(&lines.join("\n")))
}

fn rollup_markdown(
    team: &TeamData,
    period: &Period,
    summary: &SummaryResult,
    stats: &PhaseStats,
    audience: &str,
) -> String {
    let tz = &team.display_timezone;
    let mut lines: Vec<String> = vec![
        format!(
            "# {} {} {} {} summary",
            period_heading(period, tz),
            team.team_slug,
            period.kind,
            audience
        ),
        String::new(),
        format!("> **{}.** {}", summary.phrase, summary.paragraph),
        String::new(),
        "## Agent work summary".into(),
        String::new(),
    ];
    if !summary.work_summary.is_empty() {
        lines.extend(
            summary
                .work_summary
                .iter()
                .map(|item| format!("- **{}** — {}", local_time(item.at_ms, tz), item.text)),
        );
    } else {
        lines.push("_No substantive work item was recorded in this range._".into());
    }
    lines.extend([
        String::new(),
        "## Aggregate statistics".into(),
        String::new(),
        format!("- User prompts: {}", stats.user_prompts),
        format!("- Agent responses: {}", stats.agent_responses),
        format!("- Inter-agent messages: {}", stats.inter_agent_messages),
    ]);
    if stats.external_messages != 0 {
        lines.push(format!("- External messages: {}", stats.external_messages));
    }
    lines.extend([
        format!("- Tool calls: {}", stats.tool_calls),
        String::new(),
        "## Summary provenance".into(),
        String::new(),
        format!(
            "Model `{}` · prompt `{}` · input `{}`.",
            summary.model, summary.prompt_version, summary.input_hash
        ),
    ]);
    format!("{}\n", rstrip(&lines.join("\n")))
}

fn plain_language_path(period: &Period) -> Result<String, String> {
    period
        .relative_path
        .strip_suffix(".md")
        .map(|base| format!("{base}-plain-language.md"))
        .ok_or_else(|| {
            format!(
                "rollup Markdown path lacks .md suffix: {}",
                period.relative_path
            )
        })
}

fn agent_markdown(
    team: &TeamData,
    agent: &Agent,
    name: &AgentNameResult,
    phases: &[PhaseWindow],
    summaries: &HashMap<String, SummaryResult>,
) -> String {
    let tz = &team.display_timezone;
    let lifetime = clean_summary_prose(name.lifetime_summary.as_deref().unwrap_or_default());
    let mut lines: Vec<String> = vec![
        format!("# {}", name.short_name),
        String::new(),
        format!(
            "Team: `{}` · thread `{}`  ",
            team.team_slug, agent.thread_id
        ),
        format!("Official path: `{}`  ", agent.agent_path),
        format!(
            "Coordinator nickname: `{}`  ",
            agent.nickname.as_deref().unwrap_or("none")
        ),
        format!("Naming rationale: {}", name.rationale),
        String::new(),
        "## Lifetime summary".into(),
        String::new(),
        if lifetime.is_empty() {
            "Unavailable in this summary version.".into()
        } else {
            lifetime
        },
        String::new(),
        "## Work phases".into(),
        String::new(),
    ];
    for phase in phases {
        let result = &summaries[&phase.summary_key];
        lines.extend([
            format!("### {} — {}", local_time(phase.start_ms, tz), result.phrase),
            String::new(),
            result.paragraph.clone(),
            String::new(),
        ]);
        lines.extend(
            result
                .work_summary
                .iter()
                .map(|item| format!("- **{}** — {}", local_time(item.at_ms, tz), item.text)),
        );
        lines.push(String::new());
    }
    format!("{}\n", rstrip(&lines.join("\n")))
}

fn event_objs(team: &TeamData, visible: &BTreeSet<String>) -> Vec<Value> {
    let mut result: Vec<(i64, String, String)> = Vec::new();
    for event in &team.events {
        if visible.contains(&event.thread_id)
            && in_window(team, event.timestamp_ms)
            && matches!(
                event.kind.as_str(),
                "user_prompt" | "assistant_message" | "inter_agent_message" | "external_message"
            )
        {
            result.push((
                event.timestamp_ms,
                event.thread_id.clone(),
                event.kind.clone(),
            ));
        }
    }
    for tool in &team.tool_calls {
        if !visible.contains(&tool.thread_id) || !in_window(team, tool.started_at_ms) {
            continue;
        }
        let count: i64 = tool.nested_tools.iter().map(|item| item.1).sum();
        let count = if count == 0 { 1 } else { count };
        for _ in 0..count {
            result.push((tool.started_at_ms, tool.thread_id.clone(), "tool_call".into()));
        }
    }
    result.sort();
    result
        .into_iter()
        .map(|(at_ms, agent_id, kind)| {
            let mut obj = serde_json::Map::new();
            obj.insert("at_ms".into(), at_ms.into());
            obj.insert("agent_id".into(), Value::String(agent_id));
            obj.insert("kind".into(), Value::String(kind));
            Value::Object(obj)
        })
        .collect()
}

fn event_stats(events: &[Value], agents: i64) -> BTreeMap<String, i64> {
    let mut counts: BTreeMap<String, i64> = [
        ("user_prompts", 0),
        ("agent_responses", 0),
        ("inter_agent_messages", 0),
        ("external_messages", 0),
        ("tool_calls", 0),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    let labels: HashMap<&str, &str> = [
        ("user_prompt", "user_prompts"),
        ("assistant_message", "agent_responses"),
        ("inter_agent_message", "inter_agent_messages"),
        ("external_message", "external_messages"),
        ("tool_call", "tool_calls"),
    ]
    .into_iter()
    .collect();
    for event in events {
        if let Value::Object(obj) = event {
            if let Some(Value::String(kind)) = obj.get("kind") {
                if let Some(label) = labels.get(kind.as_str()) {
                    *counts.entry(label.to_string()).or_default() += 1;
                }
            }
        }
    }
    counts.insert("active_agents".into(), agents);
    counts.insert("events".into(), events.len() as i64);
    counts
}

fn safe_presentation_file(relative: &str) -> Result<String, String> {
    if is_schema_3_path(relative) {
        return Ok(relative.to_string());
    }
    let base = relative.strip_suffix(".gz").unwrap_or(relative);
    if relative.ends_with(".gz") && base.ends_with(".gz") {
        return Err(format!(
            "nested presentation gzip sidecar: {}",
            py_repr_str(relative)
        ));
    }
    let parts: Vec<&str> = base.split('/').collect();
    if base.starts_with('/')
        || parts.is_empty()
        || parts.iter().any(|part| matches!(*part, "" | "." | ".."))
    {
        return Err(format!(
            "unsafe presentation manifest path: {}",
            py_repr_str(relative)
        ));
    }
    let fixed: BTreeSet<&str> = [
        PRESENTATION_MANIFEST,
        "data/timeline.json",
        "data/timeline-v2.json",
        "data/timeline-v2/manifest.json",
        "data/artifacts.json",
    ]
    .into_iter()
    .collect();
    if presentation_common().contains(base) || base == "query.py" || fixed.contains(base) {
        return Ok(relative.to_string());
    }
    if (parts.len() == 3 || parts.len() == 4)
        && parts[0] == "data"
        && parts[1] == "details"
        && base.ends_with(".json")
    {
        return Ok(relative.to_string());
    }
    if parts.len() == 4
        && parts[0] == "data"
        && parts[1] == "timeline-v2"
        && parts[2] == "objects"
        && base.ends_with(".json")
    {
        let stem = parts[3].strip_suffix(".json").unwrap_or_default();
        if stem.len() == 64
            && stem
                .chars()
                .all(|item| item.is_ascii_hexdigit() && !item.is_ascii_uppercase())
        {
            return Ok(relative.to_string());
        }
    }
    if parts.len() >= 4 && parts[0] == "teams" && parts[2] == "summaries" && base.ends_with(".md") {
        return Ok(relative.to_string());
    }
    Err(format!(
        "unrecognized presentation manifest path: {}",
        py_repr_str(relative)
    ))
}

fn previous_presentation_files(archive: &Path) -> Result<BTreeSet<String>, String> {
    let path = generated_path(archive, PRESENTATION_MANIFEST)?;
    if !path.is_file() {
        return Ok(BTreeSet::new());
    }
    let value = read_json(&path)?;
    let root = as_object(&value, &path.display().to_string())?;
    if root.get("schema_version").and_then(Value::as_i64) != Some(1)
        || !matches!(
            root.get("kind").and_then(Value::as_str),
            Some("single-team-export" | "multi-team-export")
        )
    {
        return Err(format!(
            "unsupported presentation manifest at {}",
            path.display()
        ));
    }
    let raw = as_array(
        root.get("generated_files").unwrap_or(&Value::Null),
        &format!("{}.generated_files", path.display()),
    )?;
    let mut result = BTreeSet::new();
    for item in raw {
        let relative = as_string(item, "generated file")?;
        safe_presentation_file(relative)?;
        result.insert(relative.to_string());
    }
    Ok(result)
}

fn remove_stale_presentation_files(
    archive: &Path,
    previous: &BTreeSet<String>,
    current: &BTreeSet<String>,
) -> Result<i64, String> {
    let mut changed = 0i64;
    for relative in previous.difference(current) {
        safe_presentation_file(relative)?;
        let path = generated_path(archive, relative)?;
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|error| crate::json::os_error(&error, &path))?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn schema_2_presentation_files(archive: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for relative in [
        SCHEMA_2_BOOTSTRAP_PATH,
        "data/timeline-v2/bootstrap.json.gz",
    ] {
        let path = archive.join(relative);
        if path.is_file()
            && !path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
        {
            found.insert(relative.to_string());
        }
    }
    let root = archive.join(SCHEMA_2_PRESENTATION_ROOT);
    if !root.is_dir()
        || root
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
    {
        return found;
    }
    fn walk(dir: &Path, archive: &Path, found: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue;
            }
            if path.is_dir() {
                walk(&path, archive, found);
            } else if path.is_file() {
                if let Ok(relative) = path.strip_prefix(archive) {
                    found.insert(relative.display().to_string().replace('\\', "/"));
                }
            }
        }
    }
    walk(&root, archive, &mut found);
    found
}

fn retired_generation_files(archive: &Path, current: &BTreeSet<String>) -> BTreeSet<String> {
    let mut retired = BTreeSet::new();
    for relative in [SCHEMA_1_TIMELINE_PATH, "data/timeline.json.gz"] {
        if current.contains(relative) {
            continue;
        }
        let path = archive.join(relative);
        if path.is_file()
            && !path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
        {
            retired.insert(relative.to_string());
        }
    }
    retired.extend(
        schema_2_presentation_files(archive)
            .difference(current)
            .cloned(),
    );
    retired
}

fn prune_retired_query_artifacts(archive: &Path, manifest_owned: bool) -> Result<i64, String> {
    let launcher = archive.join("query.py");
    let mut recognized = manifest_owned;
    let mut changed = 0i64;
    let launcher_link = launcher
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if launcher.exists() || launcher_link {
        if launcher_link || !launcher.is_file() {
            if manifest_owned {
                return Err(format!(
                    "retired generated query launcher is unsafe: {}",
                    launcher.display()
                ));
            }
            return Ok(0);
        }
        if manifest_owned
            || std::fs::read(&launcher).map_err(|error| crate::json::os_error(&error, &launcher))?
                == STANDALONE_QUERY.as_bytes()
        {
            std::fs::remove_file(&launcher)
                .map_err(|error| crate::json::os_error(&error, &launcher))?;
            recognized = true;
            changed = 1;
        } else {
            return Ok(0);
        }
    }
    let marker = archive.join(".agent-team-timeline.json");
    if !recognized
        && (!marker.is_file()
            || marker
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false))
    {
        return Ok(changed);
    }
    let cache = archive.join("__pycache__");
    if !cache.exists() {
        return Ok(changed);
    }
    if cache
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
        || !cache.is_dir()
    {
        return Err(format!(
            "retired query bytecode cache is unsafe: {}",
            cache.display()
        ));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("query.") && name.ends_with(".pyc") {
                candidates.push(entry.path());
            }
        }
    }
    candidates.sort();
    for candidate in candidates {
        if candidate
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
            || !candidate.is_file()
        {
            return Err(format!(
                "retired query bytecode entry is unsafe: {}",
                candidate.display()
            ));
        }
        std::fs::remove_file(&candidate)
            .map_err(|error| crate::json::os_error(&error, &candidate))?;
        changed += 1;
    }
    let _ = std::fs::remove_dir(&cache);
    Ok(changed)
}

/// The archive Makefile.
pub fn archive_makefile() -> &'static str {
    ".DEFAULT_GOAL := help\n.PHONY: help serve open run-stats query prompts\nPYTHON ?= python3\nPORT ?= 8765\nQUERY_ARGS ?= teams\n\nPROMPT_ARGS ?=\n\nhelp:\n\t@printf '%s\\n' \\\n\t  'Agent timeline archive commands:' \\\n\t  '  make serve                 Serve the website on PORT (default 8765)' \\\n\t  '  make open                  Serve it and ask Python to open a browser' \\\n\t  '  make run-stats             Print recorded pipeline runs and token costs' \\\n\t  '  make query QUERY_ARGS=...  Run the read-only ./timeline CLI' \\\n\t  '  make prompts PROMPT_ARGS=...  Print owner prompts' \\\n\t  '  ./timeline --help          Discover all query and search commands' \\\n\t  '' \\\n\t  'Variables: PORT, PYTHON, QUERY_ARGS, PROMPT_ARGS'\n\nserve:\n\t$(PYTHON) ./serve.py --port $(PORT)\n\nopen:\n\t$(PYTHON) ./serve.py --port $(PORT) --open\n\nrun-stats:\n\t$(PYTHON) ./run_stats.py\n\nquery:\n\t@./timeline $(QUERY_ARGS)\n\nprompts:\n\t@./timeline prompts $(PROMPT_ARGS)\n"
}

/// The archive README.
pub fn archive_readme(heading: &str, introduction: &str, export_window_note: &str) -> String {
    format!(
        "# {heading}\n\n{introduction}\n\n## Start here\n\n```bash\nmake help\nmake serve\n# open http://127.0.0.1:8765/\n```\n\nDo not open `index.html` directly: browsers block its JSON requests from `file://`. Use `PORT=9000 make serve` to choose another port, or `make open` to ask Python to open the browser. The bundled server negotiates deterministic gzip sidecars and revalidates cached files.\n\n## Human-facing methods\n\n- `./timeline`: canonical, dependency-free, read-only CLI for teams, agents, phases, prompts, messages, full-text search, and stable references. Start with `./timeline --help`.\n- `./serve.py`: local static web server used by `make serve` and `make open`. Start with `./serve.py --help` when calling it directly.\n- `./run_stats.py`: prints pipeline-run status and recorded model-token costs; `make run-stats` is the convenient wrapper.\n- `Makefile`: discoverable wrappers for those methods. Plain `make` and `make help` print the command summary without starting a server.\n\nThere is deliberately no second query launcher: `./timeline` is the single supported archive-local CLI.\n\n## Read-only query quickstart\n\nPrompt output defaults to readable text; supported formats are `json`, `jsonl`, `markdown`, and `text`. Copy a stable reference returned by a list command or `search` into `show`; references use `team:TEAM`, `agent:TEAM::ID`, `phase:TEAM::ID`, or `rollup:TEAM::KIND::START_MS`.\n\n```bash\n./timeline teams\n./timeline agents --team TEAM --format jsonl\n./timeline show agent:TEAM::AGENT_ID --format markdown\n./timeline show phase:TEAM::PHASE_ID --transcript --format markdown\n./timeline search \"SEARCH TEXT\" --in owner-prompts --limit 20\n./timeline search \"SEARCH TEXT\" --in agent-responses --limit 20\n./timeline search \"SEARCH TEXT\" --in all-transcript --limit 20\n./timeline prompts --range 200-300\n./timeline prompts --format jsonl > prompts.jsonl\n```\n\nWhen present, `extracted/transcripts/prompts.jsonl.gz` is the complete mechanical prompt report and `messages.jsonl.gz` adds mechanically associated responses. Both are ordinary multi-member gzip -- `gunzip -c` gives the plain JSONL -- with a `.index.jsonl` sidecar that lets a query read only the part it needs.\n\n## Top-level map\n\n- `README.md`, `Makefile`, `timeline`, `serve.py`, and `run_stats.py` are the human entrypoints described above.\n- `index.html`, `app.js`, `timeline-core.js`, `style.css`, and `vendor/` are the generated offline website payload.\n- `manifest.json`, when present, is the latest pipeline-run status; `runs/` contains append-only per-run receipts and token accounting.\n- `data/` is the generated website/query projection; its `export.json` inventories presentation-owned files and the selected time window.\n- `extracted/` contains zero-token mechanical transcript projections when extraction has run.\n- `teams/` contains per-team normalized inputs, cached summaries, and published Markdown summaries.\n- `snapshot-root.json`, when present, records where this archive keeps the vendor source snapshots ingestion reads. They are input rather than output and are usually the largest thing involved, so they are kept outside this directory and are not served; back that store up alongside this archive if you intend to keep ingesting into it.\n- `qa/`, when present, contains human- or automation-produced validation evidence.\n- `.agent-team-timeline.json` marks this as a managed archive, `.agent-team-timeline.lock` serializes writers, and `.gitignore` identifies local-only state. None is a user command.\n\n{export_window_note}\n"
    )
}

fn summary_for_agent_at<'a>(
    agent_id: &str,
    at_ms: i64,
    phases_by_agent: &HashMap<String, Vec<PhaseWindow>>,
    summaries: &'a HashMap<String, SummaryResult>,
) -> Option<&'a SummaryResult> {
    let own = phases_by_agent.get(agent_id)?;
    if own.is_empty() {
        return None;
    }
    let containing: Vec<&PhaseWindow> = own
        .iter()
        .filter(|phase| phase.start_ms <= at_ms && at_ms <= phase.end_ms)
        .collect();
    let pool: Vec<&PhaseWindow> = if containing.is_empty() {
        own.iter().collect()
    } else {
        containing
    };
    let selected = pool
        .iter()
        .min_by_key(|phase| ((phase.start_ms - at_ms).abs(), phase.start_ms))?;
    summaries.get(&selected.summary_key)
}

fn agent_name_value(agent: &Agent, names: &HashMap<String, AgentNameResult>) -> String {
    agent_name(agent, names)
        .map(|item| item.short_name.clone())
        .unwrap_or_default()
}

fn edge_obj(
    edge: &Edge,
    agents: &HashMap<String, Agent>,
    phases_by_agent: &HashMap<String, Vec<PhaseWindow>>,
    summaries: &HashMap<String, SummaryResult>,
    names: &HashMap<String, AgentNameResult>,
) -> Option<Value> {
    let target = agents.get(&edge.to_thread_id)?;
    if !agents.contains_key(&edge.from_thread_id) {
        return None;
    }
    let summary = summary_for_agent_at(
        &edge.to_thread_id,
        edge.timestamp_ms,
        phases_by_agent,
        summaries,
    );
    let readable_kind = if edge.kind == "followup" {
        "message"
    } else {
        edge.kind.as_str()
    };
    let action = match readable_kind {
        "spawn" => "Spawn".to_string(),
        "continuation" => "Continue as".to_string(),
        "message" => "Message to".to_string(),
        "interrupt" => "Interrupt".to_string(),
        other => other.replace('_', " "),
    };
    let action = if matches!(
        readable_kind,
        "spawn" | "continuation" | "message" | "interrupt"
    ) {
        action
    } else {
        // Python .title() on the replaced string.
        let mut chars = action.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => action,
        }
    };
    let target_name = agent_name(target, names).ok()?.short_name.clone();
    let phrase = format!("{action} {target_name}");
    let paragraph = match summary {
        Some(item) if item.summary_available => item.paragraph.clone(),
        _ => phrase.clone(),
    };
    let (full_text, status) = if edge.content_availability == "encrypted" {
        (
            String::new(),
            "Codex persisted this collaboration payload encrypted. The exact plaintext is not available to an offline transcript parser; the paragraph is inferred from the receiving agent's work.".to_string(),
        )
    } else if edge.message_text.is_none() {
        (
            String::new(),
            "No plaintext message body was present in the source log.".to_string(),
        )
    } else {
        (edge.message_text.clone().unwrap_or_default(), String::new())
    };
    let mut obj = serde_json::Map::new();
    obj.insert("id".into(), Value::String(edge.edge_id.clone()));
    obj.insert(
        "source_id".into(),
        Value::String(edge.from_thread_id.clone()),
    );
    obj.insert("target_id".into(), Value::String(edge.to_thread_id.clone()));
    obj.insert("source_ms".into(), edge.timestamp_ms.into());
    obj.insert(
        "target_ms".into(),
        if matches!(readable_kind, "spawn" | "continuation") {
            target.started_at_ms
        } else {
            edge.timestamp_ms
        }
        .into(),
    );
    obj.insert(
        "kind".into(),
        Value::String(
            if matches!(
                readable_kind,
                "spawn" | "continuation" | "message" | "result"
            ) {
                readable_kind
            } else {
                "other"
            }
            .into(),
        ),
    );
    obj.insert("phrase".into(), Value::String(phrase));
    obj.insert("paragraph".into(), Value::String(paragraph));
    obj.insert("full_text".into(), Value::String(full_text));
    obj.insert("content_status".into(), Value::String(status));
    Some(Value::Object(obj))
}

fn result_edge_objs(
    team: &TeamData,
    agents: &HashMap<String, Agent>,
    phases_by_agent: &HashMap<String, Vec<PhaseWindow>>,
    summaries: &HashMap<String, SummaryResult>,
    names: &HashMap<String, AgentNameResult>,
) -> Vec<Value> {
    let continuation_targets: BTreeSet<&str> = team
        .edges
        .iter()
        .filter(|edge| edge.kind == "continuation")
        .map(|edge| edge.to_thread_id.as_str())
        .collect();
    let mut finals_by_agent: HashMap<String, Vec<&Event>> = HashMap::new();
    for event in &team.events {
        if matches!(
            event.kind.as_str(),
            "assistant_message" | "inter_agent_message"
        ) && event.phase.as_deref() == Some("final_answer")
            && event.text.is_some()
        {
            finals_by_agent
                .entry(event.thread_id.clone())
                .or_default()
                .push(event);
        }
    }
    let mut turns_by_key: HashMap<(String, String), &crate::team_data::Turn> = HashMap::new();
    for turn in &team.turns {
        turns_by_key
            .entry((turn.thread_id.clone(), turn.turn_id.clone()))
            .or_insert(turn);
    }
    let mut triggers_by_target: HashMap<String, Vec<&Edge>> = HashMap::new();
    for edge in &team.edges {
        if matches!(edge.kind.as_str(), "spawn" | "followup") {
            triggers_by_target
                .entry(edge.to_thread_id.clone())
                .or_default()
                .push(edge);
        }
    }
    let mut result = Vec::new();
    for agent in &team.agents {
        let Some(parent_id) = &agent.parent_thread_id else {
            continue;
        };
        if !agents.contains_key(&agent.thread_id)
            || !agents.contains_key(parent_id)
            || continuation_targets.contains(agent.thread_id.as_str())
        {
            continue;
        }
        let mut finals: Vec<&Event> = finals_by_agent
            .get(&agent.thread_id)
            .cloned()
            .unwrap_or_default();
        finals.sort_by(|a, b| (a.timestamp_ms, &a.event_id).cmp(&(b.timestamp_ms, &b.event_id)));
        for final_event in finals {
            if !in_window(team, final_event.timestamp_ms) {
                continue;
            }
            let target_id = result_target(agent, final_event, &turns_by_key, &triggers_by_target);
            let Some(target_id) = target_id else {
                continue;
            };
            if !agents.contains_key(target_id) {
                continue;
            }
            let summary = summary_for_agent_at(
                &agent.thread_id,
                final_event.timestamp_ms,
                phases_by_agent,
                summaries,
            );
            let text = final_event.text.clone().unwrap_or_default();
            let mut obj = serde_json::Map::new();
            obj.insert(
                "id".into(),
                Value::String(format!("turn-result-{}", final_event.event_id)),
            );
            obj.insert("source_id".into(), Value::String(agent.thread_id.clone()));
            obj.insert("target_id".into(), Value::String(target_id.to_string()));
            obj.insert("source_ms".into(), final_event.timestamp_ms.into());
            obj.insert("target_ms".into(), final_event.timestamp_ms.into());
            obj.insert("kind".into(), Value::String("message".into()));
            obj.insert(
                "phrase".into(),
                Value::String(format!(
                    "{} reports progress",
                    agent_name_value(agent, names)
                )),
            );
            obj.insert(
                "paragraph".into(),
                Value::String(match summary {
                    Some(item) if item.summary_available => item.paragraph.clone(),
                    _ => text.chars().take(500).collect(),
                }),
            );
            obj.insert("full_text".into(), Value::String(text));
            obj.insert("content_status".into(), Value::String(String::new()));
            result.push(Value::Object(obj));
        }
        let Some(ended_at) = agent.ended_at_ms else {
            continue;
        };
        let phase_end_ms = phases_by_agent
            .get(&agent.thread_id)
            .map(|phases| {
                phases
                    .iter()
                    .map(|phase| phase.end_ms - 1)
                    .max()
                    .unwrap_or(ended_at)
            })
            .unwrap_or(ended_at);
        let return_ms = ended_at.max(phase_end_ms);
        if !in_window(team, return_ms) {
            continue;
        }
        let parent = &agents[parent_id];
        let summary = summary_for_agent_at(&agent.thread_id, return_ms, phases_by_agent, summaries);
        let child_name = agent_name_value(agent, names);
        let parent_name = agent_name_value(parent, names);
        let phrase = if agent.status == "completed" {
            format!("{child_name} returns to {parent_name}")
        } else {
            format!(
                "{child_name} lifetime ends at {parent_name} ({})",
                agent.status
            )
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "id".into(),
            Value::String(format!("result-{}", agent.thread_id)),
        );
        obj.insert("source_id".into(), Value::String(agent.thread_id.clone()));
        obj.insert("target_id".into(), Value::String(parent_id.clone()));
        obj.insert("source_ms".into(), return_ms.into());
        obj.insert("target_ms".into(), return_ms.into());
        obj.insert("kind".into(), Value::String("result".into()));
        obj.insert("phrase".into(), Value::String(phrase));
        obj.insert(
            "paragraph".into(),
            Value::String(match summary {
                Some(item) if item.summary_available => item.paragraph.clone(),
                _ => format!("{child_name} ended with status {}.", agent.status),
            }),
        );
        obj.insert("full_text".into(), Value::String(String::new()));
        obj.insert(
            "content_status".into(),
            Value::String(format!(
                "Structural lifetime join. Agent status: {}. Turn responses are available as detailed message edges.",
                agent.status
            )),
        );
        result.push(Value::Object(obj));
    }
    result
}

fn result_target<'a>(
    agent: &'a Agent,
    final_event: &Event,
    turns_by_key: &HashMap<(String, String), &crate::team_data::Turn>,
    triggers_by_target: &HashMap<String, Vec<&'a Edge>>,
) -> Option<&'a str> {
    let turn = final_event
        .turn_id
        .as_ref()
        .and_then(|turn_id| turns_by_key.get(&(final_event.thread_id.clone(), turn_id.clone())));
    let Some(turn) = turn else {
        return agent.parent_thread_id.as_deref();
    };
    let triggers: Vec<&&Edge> = triggers_by_target
        .get(&agent.thread_id)
        .map(|edges| {
            edges
                .iter()
                .filter(|edge| (edge.timestamp_ms - turn.started_at_ms).abs() <= 2_000)
                .collect()
        })
        .unwrap_or_default();
    if triggers.is_empty() {
        return agent.parent_thread_id.as_deref();
    }
    let trigger = triggers
        .iter()
        .min_by_key(|edge| {
            (
                (edge.timestamp_ms - turn.started_at_ms).abs(),
                edge.timestamp_ms,
                edge.edge_id.clone(),
            )
        })
        .unwrap();
    Some(trigger.from_thread_id.as_str())
}

fn transcript_entry_obj(entry: &TranscriptEntry, metadata: &PullMap) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("at_ms".into(), entry.at_ms.into());
    obj.insert("role".into(), Value::String(entry.role.clone()));
    obj.insert("text".into(), Value::String(entry.text.clone()));
    obj.insert(
        "tools".into(),
        Value::Array(
            entry
                .tools
                .iter()
                .map(|tool| {
                    let mut item = serde_json::Map::new();
                    item.insert("name".into(), Value::String(tool.name.clone()));
                    item.insert("count".into(), tool.count.into());
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    let mut value = Value::Object(obj);
    if let Value::Object(obj) = &mut value {
        obj.insert(
            "pull_requests".into(),
            Value::Array(pull_request_references(&entry.text, metadata)),
        );
    }
    value
}

/// Regenerate all presentation files without a summary backend.
#[allow(clippy::too_many_lines)]
pub fn render_archive(archive: &Path, inputs: RenderInputs) -> Result<BuildReport, String> {
    let RenderInputs {
        team,
        phases,
        phase_summaries,
        periods,
        rollup_summaries,
        plain_rollup_summaries,
        rollup_stats,
        glossary_terms,
        project_overview,
        agent_names,
        pull_metadata,
        artifact_catalog,
        site_identity,
        published,
        precompress,
        export_window,
        namespace_agents,
    } = inputs;
    let previous_presentation_files = previous_presentation_files(archive)?;
    let mut changed: i64 = 0;
    let mut compressible_paths: BTreeSet<String> = BTreeSet::new();
    let mut gzip_only_paths: BTreeSet<String> = BTreeSet::new();
    let mut published_summary_paths: BTreeSet<String> = BTreeSet::new();
    let mut phase_paths: HashMap<String, String> = HashMap::new();
    let agents_by_id: HashMap<String, Agent> = team
        .agents
        .iter()
        .map(|agent| (agent.thread_id.clone(), agent.clone()))
        .collect();
    let visible_agent_ids = crate::summary_phases::phase_agent_ids(&team, &phases);
    let visible_set: BTreeSet<String> = visible_agent_ids.iter().cloned().collect();
    let mut phases_by_agent: HashMap<String, Vec<PhaseWindow>> = HashMap::new();
    for phase in &phases {
        phases_by_agent
            .entry(phase.agent_id.clone())
            .or_default()
            .push(phase.clone());
    }
    for own in phases_by_agent.values_mut() {
        own.sort_by(|a, b| (a.start_ms, &a.phase_id).cmp(&(b.start_ms, &b.phase_id)));
    }
    let artifact_index = ArtifactRangeIndex::from_catalog(&artifact_catalog);
    let mut phase_artifact_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut phase_output_artifact_ids: HashMap<String, Vec<String>> = HashMap::new();
    for phase in &phases {
        let summary = &phase_summaries[&phase.summary_key];
        let agent = &agents_by_id[&phase.agent_id];
        let name = agent_name(agent, &agent_names)?;
        let raw_path = format!(
            "teams/{}/summaries/phases/{}.md",
            team.team_slug, phase.phase_id
        );
        let detail_path = format!("data/details/{}.json", phase.phase_id);
        compressible_paths.insert(raw_path.clone());
        gzip_only_paths.insert(detail_path.clone());
        phase_paths.insert(phase.phase_id.clone(), detail_path.clone());
        let artifact_ids = artifact_index.ids_for_range(
            phase.start_ms,
            phase.end_ms,
            Some(&phase.agent_id),
            false,
        );
        let output_ids =
            artifact_index.ids_for_range(phase.start_ms, phase.end_ms, Some(&phase.agent_id), true);
        phase_artifact_ids.insert(phase.phase_id.clone(), artifact_ids.clone());
        phase_output_artifact_ids.insert(phase.phase_id.clone(), output_ids.clone());
        if summary.summary_available {
            changed += write_text_with_gzip_invalidation(
                &generated_path(archive, &raw_path)?,
                &phase_markdown(&team, agent, name, phase, summary),
            )?;
            published_summary_paths.insert(raw_path.clone());
        } else {
            changed += remove_generated_file(archive, &raw_path)?;
        }
        let mut detail = serde_json::Map::new();
        detail.insert("phrase".into(), Value::String(summary.phrase.clone()));
        detail.insert("paragraph".into(), Value::String(summary.paragraph.clone()));
        detail.insert(
            "summary_available".into(),
            Value::Bool(summary.summary_available),
        );
        detail.insert(
            "stats".into(),
            Value::Object(
                phase
                    .stats
                    .to_mapping()
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        );
        detail.insert(
            "work_summary".into(),
            Value::Array(
                summary
                    .work_summary
                    .iter()
                    .map(|item| {
                        let mut entry = serde_json::Map::new();
                        entry.insert("at_ms".into(), item.at_ms.into());
                        entry.insert("text".into(), Value::String(item.text.clone()));
                        entry.insert(
                            "pull_requests".into(),
                            Value::Array(pull_request_references(&item.text, &pull_metadata)),
                        );
                        Value::Object(entry)
                    })
                    .collect(),
            ),
        );
        detail.insert(
            "transcript".into(),
            Value::Array(
                phase
                    .transcript
                    .iter()
                    .map(|entry| transcript_entry_obj(entry, &pull_metadata))
                    .collect(),
            ),
        );
        detail.insert(
            "raw_summary_path".into(),
            Value::String(if summary.summary_available {
                raw_path.clone()
            } else {
                String::new()
            }),
        );
        detail.insert("agent".into(), agent_identity_obj(agent, name));
        detail.insert(
            "artifact_ids".into(),
            Value::Array(
                artifact_ids
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        detail.insert(
            "output_artifact_ids".into(),
            Value::Array(
                output_ids
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        changed += write_gzip_only(
            &generated_path(archive, &detail_path)?,
            &dump_canonical(&Value::Object(detail)),
        )?;
    }

    for agent in &team.agents {
        let own: Vec<PhaseWindow> = phases_by_agent
            .get(&agent.thread_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let mut own_sorted = own;
        own_sorted.sort_by_key(|phase| phase.start_ms);
        let agent_path = format!(
            "teams/{}/summaries/agents/{}.md",
            team.team_slug, agent.thread_id
        );
        compressible_paths.insert(agent_path.clone());
        if own_sorted.is_empty() {
            changed += remove_generated_file(archive, &agent_path)?;
            continue;
        }
        let name = agent_name(agent, &agent_names)?;
        if agent_summary_available(name) {
            changed += write_text_with_gzip_invalidation(
                &generated_path(archive, &agent_path)?,
                &agent_markdown(&team, agent, name, &own_sorted, &phase_summaries),
            )?;
            published_summary_paths.insert(agent_path);
        } else {
            changed += remove_generated_file(archive, &agent_path)?;
        }
    }

    let mut rollup_paths: HashMap<String, (String, String)> = HashMap::new();
    for period in &periods {
        let period_key = format!("{}:{}", period.key, period.kind);
        let summary = &rollup_summaries[&period_key];
        let plain_summary = &plain_rollup_summaries[&period_key];
        let stats = &rollup_stats[&period_key];
        let technical_path = if summary.summary_available {
            period.relative_path.clone()
        } else {
            String::new()
        };
        let plain_path = if plain_summary.summary_available {
            plain_language_path(period)?
        } else {
            String::new()
        };
        rollup_paths.insert(
            period_key.clone(),
            (technical_path.clone(), plain_path.clone()),
        );
        if !technical_path.is_empty() {
            changed += write_text_with_gzip_invalidation(
                &generated_path(archive, &technical_path)?,
                &rollup_markdown(&team, period, summary, stats, "technical"),
            )?;
            published_summary_paths.insert(technical_path);
        } else {
            changed += remove_generated_file(archive, &period.relative_path)?;
        }
        let expected_plain_path = plain_language_path(period)?;
        compressible_paths.insert(period.relative_path.clone());
        compressible_paths.insert(expected_plain_path.clone());
        if !plain_path.is_empty() {
            changed += write_text_with_gzip_invalidation(
                &generated_path(archive, &plain_path)?,
                &rollup_markdown(&team, period, plain_summary, stats, "plain-language"),
            )?;
            published_summary_paths.insert(plain_path);
        } else {
            changed += remove_generated_file(archive, &expected_plain_path)?;
        }
    }

    let weeks: BTreeSet<String> = glossary_terms
        .iter()
        .map(|term| term.week.clone())
        .collect();
    let glossary_root_relative = format!("teams/{}/summaries/glossary", team.team_slug);
    let glossary_root = generated_path(archive, &glossary_root_relative)?;
    if glossary_root
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "refusing symlinked generated glossary directory: {}",
            glossary_root.display()
        ));
    }
    let mut expected_week_paths: BTreeSet<PathBuf> = BTreeSet::new();
    for week in &weeks {
        let year = week.split("-W").next().unwrap_or_default();
        let path = format!(
            "teams/{}/summaries/glossary/{year}/{week}-{}-glossary.md",
            team.team_slug, team.team_slug
        );
        let week_path = generated_path(archive, &path)?;
        compressible_paths.insert(path.clone());
        let parent_link = week_path
            .parent()
            .map(|parent| {
                parent
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if parent_link
            || week_path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
        {
            return Err(format!(
                "refusing symlinked generated glossary path: {}",
                week_path.display()
            ));
        }
        expected_week_paths
            .insert(std::fs::canonicalize(&week_path).unwrap_or_else(|_| week_path.clone()));
        changed += write_text_with_gzip_invalidation(
            &week_path,
            &glossary_markdown(&team.team_slug, week, &glossary_terms),
        )?;
        published_summary_paths.insert(path);
    }
    if glossary_root.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&glossary_root) {
            for entry in entries.flatten() {
                if entry
                    .path()
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    return Err(format!(
                        "refusing symlink in generated glossary directory: {}",
                        entry.path().display()
                    ));
                }
            }
        }
        let mut stale: Vec<PathBuf> = Vec::new();
        if let Ok(years) = std::fs::read_dir(&glossary_root) {
            for year_entry in years.flatten() {
                if year_entry.path().is_dir() {
                    if let Ok(files) = std::fs::read_dir(year_entry.path()) {
                        for file in files.flatten() {
                            if file.file_name().to_string_lossy().ends_with(".md") {
                                stale.push(file.path());
                            }
                        }
                    }
                }
            }
        }
        stale.sort();
        for stale_path in stale {
            let year = stale_path
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = stale_path
                .file_name()
                .map(|item| item.to_string_lossy().into_owned())
                .unwrap_or_default();
            let prefix = format!("{year}-W");
            let suffix = format!("-{}-glossary.md", team.team_slug);
            let week_number = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(&suffix))
                .unwrap_or_default();
            let owned_name = year.len() == 4
                && year.chars().all(|item| item.is_ascii_digit())
                && name.starts_with(&prefix)
                && name.ends_with(&suffix)
                && week_number.len() == 2
                && week_number.chars().all(|item| item.is_ascii_digit());
            if !owned_name {
                continue;
            }
            let resolved =
                std::fs::canonicalize(&stale_path).unwrap_or_else(|_| stale_path.clone());
            if expected_week_paths.contains(&resolved) {
                continue;
            }
            let parent_link = stale_path
                .parent()
                .map(|parent| {
                    parent
                        .symlink_metadata()
                        .map(|meta| meta.file_type().is_symlink())
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if parent_link
                || stale_path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
            {
                return Err(format!(
                    "refusing symlinked generated glossary path: {}",
                    stale_path.display()
                ));
            }
            if stale_path.is_file() {
                let sidecar = gzip_sidecar_path(&stale_path);
                if sidecar
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    return Err(format!(
                        "refusing symlinked gzip sidecar: {}",
                        sidecar.display()
                    ));
                }
                std::fs::remove_file(&stale_path)
                    .map_err(|error| crate::json::os_error(&error, &stale_path))?;
                changed += 1;
                if sidecar.is_file() {
                    std::fs::remove_file(&sidecar)
                        .map_err(|error| crate::json::os_error(&error, &sidecar))?;
                    changed += 1;
                }
            }
        }
    }
    let expected_glossary_catalog_path = format!(
        "teams/{}/summaries/glossary/{}-glossary.md",
        team.team_slug, team.team_slug
    );
    compressible_paths.insert(expected_glossary_catalog_path.clone());
    let mut glossary_catalog_path = String::new();
    if !glossary_terms.is_empty() || project_overview.summary_available {
        glossary_catalog_path = expected_glossary_catalog_path.clone();
        changed += write_text_with_gzip_invalidation(
            &generated_path(archive, &glossary_catalog_path)?,
            &glossary_catalog_markdown(
                &team.team_slug,
                &glossary_terms,
                &project_overview.paragraph,
            ),
        )?;
        published_summary_paths.insert(glossary_catalog_path.clone());
    } else {
        changed += remove_generated_file(archive, &expected_glossary_catalog_path)?;
    }

    for (relative, text) in [
        ("index.html", STATIC_INDEX),
        ("timeline-core.js", STATIC_CORE),
        ("app.js", STATIC_APP),
        ("style.css", STATIC_CSS),
    ] {
        changed += write_text_with_gzip_invalidation(&generated_path(archive, relative)?, text)?;
        compressible_paths.insert(relative.to_string());
    }
    changed += i64::from(write_text_if_changed(
        &generated_path(archive, "vendor/README.md")?,
        VENDOR_README,
    )?);
    changed += write_text_with_gzip_invalidation(
        &generated_path(archive, "vendor/markdown-it-15.0.0.min.js")?,
        VENDOR_JS,
    )?;
    compressible_paths.insert("vendor/markdown-it-15.0.0.min.js".into());
    changed += i64::from(write_text_if_changed(
        &generated_path(archive, "vendor/markdown-it-LICENSE.txt")?,
        VENDOR_LICENSE,
    )?);
    changed += i64::from(write_text_if_changed_executable(
        &generated_path(archive, "serve.py")?,
        STANDALONE_SERVER,
    )?);
    changed += i64::from(write_text_if_changed_executable(
        &generated_path(archive, "run_stats.py")?,
        RUN_STATS,
    )?);
    changed += i64::from(write_text_if_changed_executable(
        &generated_path(archive, "timeline")?,
        STANDALONE_QUERY,
    )?);
    changed += i64::from(write_text_if_changed(
        &generated_path(archive, "Makefile")?,
        archive_makefile(),
    )?);
    changed += i64::from(write_text_if_changed(
        &generated_path(archive, "README.md")?,
        &archive_readme(
            &format!("{} agent-team timeline", team.team_slug),
            "This directory is a self-contained, version-controllable timeline archive.",
            "For an exported package, the requested slice is recorded in `data/export.json` under `display_window`; `./timeline` reports the actual team and record intervals. Do not infer the slice from file modification times.",
        ),
    )?);

    let mut summary_files: Vec<Value> = Vec::new();
    for relative in &published_summary_paths {
        let path = generated_path(archive, relative)?;
        if !path.is_file()
            || path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
        {
            return Err(format!(
                "published summary path is missing or unsafe: {}",
                path.display()
            ));
        }
        let summaries_root =
            generated_path(archive, &format!("teams/{}/summaries", team.team_slug))?;
        let inner = path
            .strip_prefix(&summaries_root)
            .map(|item| item.display().to_string().replace('\\', "/"))
            .unwrap_or_default();
        let kind = inner.split('/').next().unwrap_or("summary").to_string();
        let stem = path
            .file_stem()
            .and_then(|item| item.to_str())
            .unwrap_or_default()
            .to_string();
        let mut item = serde_json::Map::new();
        item.insert("label".into(), Value::String(stem.replace('-', " ")));
        item.insert("kind".into(), Value::String(kind));
        item.insert("period".into(), Value::String(stem));
        item.insert("path".into(), Value::String(relative.clone()));
        summary_files.push(Value::Object(item));
    }

    let (start_ms, end_ms) = time_range(&team, &phases);
    let latest_ms = team
        .events
        .iter()
        .filter(|event| {
            visible_set.contains(&event.thread_id) && in_window(&team, event.timestamp_ms)
        })
        .map(|event| event.timestamp_ms)
        .max()
        .unwrap_or(end_ms);

    let mut agent_objs: Vec<Value> = Vec::new();
    for agent in &team.agents {
        if !visible_set.contains(&agent.thread_id) {
            continue;
        }
        let own = phases_by_agent
            .get(&agent.thread_id)
            .cloned()
            .unwrap_or_default();
        let own_end = own
            .iter()
            .map(|phase| phase.end_ms)
            .max()
            .unwrap_or(agent.ended_at_ms.unwrap_or(end_ms));
        let name = agent_name(agent, &agent_names)?;
        let track_start = agent.started_at_ms.max(start_ms);
        let mut track_end = own_end
            .max(agent.ended_at_ms.unwrap_or(own_end))
            .min(end_ms);
        track_end = track_end.max(track_start + 1000);
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), Value::String(agent.thread_id.clone()));
        obj.insert("team".into(), Value::String(team.team_slug.clone()));
        obj.insert(
            "parent_id".into(),
            agent
                .parent_thread_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        obj.insert("path".into(), Value::String(agent.agent_path.clone()));
        obj.insert("label".into(), Value::String(name.short_name.clone()));
        obj.insert("short_name".into(), Value::String(name.short_name.clone()));
        obj.insert(
            "official_name".into(),
            Value::String(agent.agent_path.clone()),
        );
        obj.insert("official_leaf".into(), Value::String(official_leaf(agent)));
        obj.insert(
            "nickname".into(),
            Value::String(agent.nickname.clone().unwrap_or_default()),
        );
        obj.insert(
            "naming_rationale".into(),
            Value::String(name.rationale.clone()),
        );
        obj.insert(
            "lifetime_summary".into(),
            Value::String(clean_summary_prose(
                name.lifetime_summary.as_deref().unwrap_or_default(),
            )),
        );
        obj.insert(
            "summary_available".into(),
            Value::Bool(agent_summary_available(name)),
        );
        obj.insert("depth".into(), agent.depth.into());
        obj.insert("start_ms".into(), track_start.into());
        obj.insert("end_ms".into(), track_end.min(end_ms).into());
        obj.insert("status".into(), Value::String(agent.status.clone()));
        obj.insert(
            "artifact_ids".into(),
            Value::Array(
                artifact_index
                    .ids_for_range(track_start, track_end, Some(&agent.thread_id), false)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        obj.insert(
            "output_artifact_ids".into(),
            Value::Array(
                artifact_index
                    .ids_for_range(track_start, track_end, Some(&agent.thread_id), true)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        agent_objs.push(Value::Object(obj));
    }

    let mut phase_objs: Vec<Value> = Vec::new();
    for phase in &phases {
        let summary = &phase_summaries[&phase.summary_key];
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), Value::String(phase.phase_id.clone()));
        obj.insert("agent_id".into(), Value::String(phase.agent_id.clone()));
        obj.insert("start_ms".into(), phase.start_ms.into());
        obj.insert("end_ms".into(), phase.end_ms.into());
        obj.insert("phrase".into(), Value::String(summary.phrase.clone()));
        obj.insert("paragraph".into(), Value::String(summary.paragraph.clone()));
        obj.insert(
            "summary_available".into(),
            Value::Bool(summary.summary_available),
        );
        obj.insert(
            "detail_path".into(),
            Value::String(phase_paths[&phase.phase_id].clone()),
        );
        obj.insert(
            "stats".into(),
            Value::Object(
                phase
                    .stats
                    .to_mapping()
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        );
        obj.insert(
            "states".into(),
            Value::Array(
                phase
                    .states
                    .iter()
                    .map(|state| state.to_json_obj())
                    .collect(),
            ),
        );
        obj.insert(
            "artifact_ids".into(),
            Value::Array(
                phase_artifact_ids[&phase.phase_id]
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "output_artifact_ids".into(),
            Value::Array(
                phase_output_artifact_ids[&phase.phase_id]
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        phase_objs.push(Value::Object(obj));
    }

    let visible_agents: HashMap<String, Agent> = agents_by_id
        .iter()
        .filter(|(id, _)| visible_set.contains(*id))
        .map(|(id, agent)| (id.clone(), agent.clone()))
        .collect();
    let mut edge_objs: Vec<Value> = Vec::new();
    for edge in &team.edges {
        if !visible_set.contains(&edge.from_thread_id)
            || !visible_set.contains(&edge.to_thread_id)
            || !in_window(&team, edge.timestamp_ms)
        {
            continue;
        }
        if let Some(value) = edge_obj(
            edge,
            &visible_agents,
            &phases_by_agent,
            &phase_summaries,
            &agent_names,
        ) {
            edge_objs.push(value);
        }
    }
    edge_objs.extend(result_edge_objs(
        &team,
        &visible_agents,
        &phases_by_agent,
        &phase_summaries,
        &agent_names,
    ));
    edge_objs.sort_by(|a, b| {
        let key = |value: &Value| -> (i64, String) {
            let obj = value.as_object();
            (
                obj.and_then(|item| item.get("source_ms"))
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                obj.and_then(|item| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        };
        key(a).cmp(&key(b))
    });

    let mut rollup_objs: Vec<Value> = Vec::new();
    for period in &periods {
        let period_key = format!("{}:{}", period.key, period.kind);
        let (technical_path, plain_path) = rollup_paths[&period_key].clone();
        let technical_available = rollup_summaries[&period_key].summary_available;
        let plain_available = plain_rollup_summaries[&period_key].summary_available;
        let mut obj = serde_json::Map::new();
        obj.insert("kind".into(), Value::String(period.kind.clone()));
        obj.insert(
            "label".into(),
            Value::String(format!(
                "{}{}",
                period.label,
                if period.partial { " · partial" } else { "" }
            )),
        );
        obj.insert("start_ms".into(), period.start_ms.into());
        obj.insert("end_ms".into(), period.end_ms.into());
        obj.insert("path".into(), Value::String(period.relative_path.clone()));
        obj.insert("technical_path".into(), Value::String(technical_path));
        obj.insert("plain_language_path".into(), Value::String(plain_path));
        obj.insert(
            "technical_summary_available".into(),
            Value::Bool(technical_available),
        );
        obj.insert(
            "plain_language_summary_available".into(),
            Value::Bool(plain_available),
        );
        obj.insert(
            "summary_available".into(),
            Value::Bool(technical_available || plain_available),
        );
        obj.insert(
            "stats".into(),
            Value::Object(
                rollup_stats[&period_key]
                    .to_mapping()
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        );
        obj.insert(
            "artifact_ids".into(),
            Value::Array(
                artifact_index
                    .ids_for_range(period.start_ms, period.end_ms, None, false)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        obj.insert(
            "output_artifact_ids".into(),
            Value::Array(
                artifact_index
                    .ids_for_range(period.start_ms, period.end_ms, None, true)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        rollup_objs.push(Value::Object(obj));
    }

    let glossary_objs: Vec<Value> = glossary_terms
        .iter()
        .map(|term| {
            let mut obj = serde_json::Map::new();
            obj.insert("id".into(), Value::String(term.term_id.clone()));
            obj.insert("term".into(), Value::String(term.term.clone()));
            obj.insert("introduced_at_ms".into(), term.introduced_at_ms.into());
            obj.insert(
                "available_at_ms".into(),
                term.summary_available_at_ms().into(),
            );
            obj.insert("occurrences".into(), term.occurrences.into());
            obj.insert("context".into(), Value::String(term.context.clone()));
            obj.insert("definition".into(), Value::String(term.definition.clone()));
            obj.insert(
                "definition_status".into(),
                Value::String(term.definition_status.clone()),
            );
            obj.insert("week".into(), Value::String(term.week.clone()));
            obj.insert(
                "url".into(),
                Value::String(format!("#glossary/{}", term.term_id)),
            );
            Value::Object(obj)
        })
        .collect();

    let activity_bins = build_activity_bins(
        &team.team_slug,
        &team.root_thread_id,
        &phases,
        &team.display_timezone,
        start_ms,
        end_ms,
    )?;
    let events_json = event_objs(&team, &visible_set);
    let search_records = build_search_records(
        &team,
        &visible_set,
        namespace_agents,
        Some(start_ms),
        Some(end_ms),
    )?;
    let stats_map = event_stats(&events_json, visible_set.len() as i64);
    let stats_value = Value::Object(
        stats_map
            .iter()
            .map(|(key, value)| (key.clone(), (*value).into()))
            .collect(),
    );

    let mut timeline = serde_json::Map::new();
    timeline.insert("schema_version".into(), 1.into());
    timeline.insert("generated_at".into(), Value::String(iso_ms(latest_ms)));
    timeline.insert("source_digest".into(), Value::String(source_digest(&team)));
    timeline.insert(
        "display_timezone".into(),
        Value::String(team.display_timezone.clone()),
    );
    timeline.insert(
        "display_timezone_source".into(),
        Value::String(site_identity.display_timezone_source.clone()),
    );
    let mut range = serde_json::Map::new();
    range.insert("start_ms".into(), start_ms.into());
    range.insert("end_ms".into(), end_ms.into());
    timeline.insert("range".into(), Value::Object(range));
    let mut team_obj = serde_json::Map::new();
    team_obj.insert("slug".into(), Value::String(team.team_slug.clone()));
    team_obj.insert("label".into(), Value::String(team.team_slug.clone()));
    team_obj.insert(
        "projects".into(),
        Value::Array(
            site_identity
                .projects
                .iter()
                .map(|item| item.to_json())
                .collect(),
        ),
    );
    team_obj.insert(
        "hosts".into(),
        Value::Array(
            site_identity
                .hosts
                .iter()
                .map(|item| item.to_json())
                .collect(),
        ),
    );
    team_obj.insert("stats".into(), stats_value.clone());
    timeline.insert("teams".into(), Value::Array(vec![Value::Object(team_obj)]));
    timeline.insert("agents".into(), Value::Array(agent_objs));
    timeline.insert("phases".into(), Value::Array(phase_objs));
    timeline.insert(
        "activity_bins".into(),
        Value::Array(
            activity_bins
                .iter()
                .map(|item| item.to_json_obj())
                .collect(),
        ),
    );
    timeline.insert("edges".into(), Value::Array(edge_objs.clone()));
    timeline.insert("events".into(), Value::Array(events_json));
    timeline.insert("rollups".into(), Value::Array(rollup_objs));
    timeline.insert("glossary".into(), Value::Array(glossary_objs));
    timeline.insert(
        "glossary_path".into(),
        Value::String(glossary_catalog_path.clone()),
    );
    let mut overview_obj = serde_json::Map::new();
    overview_obj.insert(
        "text".into(),
        Value::String(project_overview.paragraph.clone()),
    );
    overview_obj.insert(
        "summary_available".into(),
        Value::Bool(project_overview.summary_available),
    );
    overview_obj.insert(
        "evidence_status".into(),
        Value::String(
            if project_overview.phrase == "Project overview supported" {
                "supported"
            } else {
                "insufficient-evidence"
            }
            .into(),
        ),
    );
    overview_obj.insert(
        "model".into(),
        Value::String(project_overview.model.clone()),
    );
    overview_obj.insert(
        "prompt_version".into(),
        Value::String(project_overview.prompt_version.clone()),
    );
    overview_obj.insert(
        "input_hash".into(),
        Value::String(project_overview.input_hash.clone()),
    );
    timeline.insert("project_overview".into(), Value::Object(overview_obj));
    timeline.insert("summary_files".into(), Value::Array(summary_files.clone()));
    timeline.insert(
        "artifact_catalog_path".into(),
        Value::String("data/artifacts.json".into()),
    );
    timeline.insert(
        "projects".into(),
        Value::Array(
            artifact_catalog
                .projects
                .iter()
                .map(|item| item.to_json())
                .collect(),
        ),
    );
    timeline.insert("stats".into(), stats_value);
    let timeline_value = Value::Object(timeline);

    changed += write_gzip_only(
        &generated_path(archive, "data/artifacts.json")?,
        &dump_canonical(&artifact_catalog.to_json()),
    )?;
    gzip_only_paths.insert("data/artifacts.json".to_string());

    let mut v3_report = None;
    if !published {
        changed += write_text_with_gzip_invalidation(
            &generated_path(archive, SCHEMA_1_TIMELINE_PATH)?,
            &dump_canonical(&timeline_value),
        )?;
        compressible_paths.insert(SCHEMA_1_TIMELINE_PATH.to_string());
    }
    if precompress {
        for relative in &compressible_paths {
            changed += i64::from(sync_gzip_sidecar(&generated_path(archive, relative)?)?);
        }
    }
    if published {
        let report = write_timeline_v3(archive, &timeline_value, &search_records)?;
        changed += report.files_changed;
        v3_report = Some(report);
    }

    let mut generated_files: BTreeSet<String> = presentation_common();
    generated_files.extend(published_summary_paths.iter().cloned());
    generated_files.insert(PRESENTATION_MANIFEST.to_string());
    if !published {
        generated_files.insert(SCHEMA_1_TIMELINE_PATH.to_string());
    }
    if let Some(report) = &v3_report {
        generated_files.extend(report.generated_files.iter().cloned());
    }
    for relative in &compressible_paths {
        if gzip_sidecar_path(&generated_path(archive, relative)?).is_file() {
            generated_files.insert(format!("{relative}.gz"));
        }
    }
    for relative in &gzip_only_paths {
        generated_files.insert(format!("{relative}.gz"));
    }
    let retired_files = if published {
        retired_generation_files(archive, &generated_files)
    } else {
        BTreeSet::new()
    };
    for relative in generated_files.union(&retired_files) {
        safe_presentation_file(relative)?;
    }
    let retired_query_was_managed = previous_presentation_files.contains("query.py");
    let stale_previous: BTreeSet<String> = previous_presentation_files
        .difference(&retired_files)
        .cloned()
        .collect();
    changed += remove_stale_presentation_files(archive, &stale_previous, &generated_files)?;
    changed += prune_retired_query_artifacts(archive, retired_query_was_managed)?;

    let mut manifest = serde_json::Map::new();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert("kind".into(), Value::String("single-team-export".into()));
    manifest.insert(
        "teams".into(),
        Value::Array(vec![Value::String(team.team_slug.clone())]),
    );
    manifest.insert(
        "display_timezone".into(),
        Value::String(team.display_timezone.clone()),
    );
    manifest.insert(
        "display_window".into(),
        export_window.unwrap_or(Value::Null),
    );
    manifest.insert(
        "source_digest".into(),
        timeline_value["source_digest"].clone(),
    );
    manifest.insert(
        "generated_files".into(),
        Value::Array(
            generated_files
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    manifest.insert(
        "retired_files".into(),
        Value::Array(
            retired_files
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    changed += i64::from(write_text_if_changed(
        &generated_path(archive, PRESENTATION_MANIFEST)?,
        &dump_canonical(&Value::Object(manifest)),
    )?);

    Ok(BuildReport {
        files_changed: changed,
        phases: phases.len() as i64,
        activity_bins: activity_bins.len() as i64,
        agents: visible_set.len() as i64,
        edges: edge_objs.len() as i64,
        rollups: periods.len() as i64,
        summary_files: summary_files.len() as i64,
        artifacts: artifact_catalog.artifacts.len() as i64,
        projects: artifact_catalog.projects.len() as i64,
        detail_shards: v3_report
            .as_ref()
            .map_or(0, |report| report.timeline_shards),
        search_records: search_records.len() as i64,
        search_shards: v3_report.as_ref().map_or(0, |report| report.search_shards),
        bootstrap_bytes: v3_report
            .as_ref()
            .map_or(0, |report| report.bootstrap_bytes as i64),
        bootstrap_transfer_bytes: v3_report
            .as_ref()
            .map_or(0, |report| report.bootstrap_bytes as i64),
        shard_object_bytes: v3_report.as_ref().map_or(0, |report| {
            (report.compressed_bytes + report.index_bytes) as i64
        }),
        shard_transfer_bytes: v3_report.as_ref().map_or(0, |report| {
            (report.compressed_bytes + report.index_bytes) as i64
        }),
    })
}
