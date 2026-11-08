//! One weekly summary for a whole project, rolled up from its teams' own weekly summaries.
//!
//! Calendar rollups are per team: a week of one team is summarized from that team's days, and
//! nothing above that combines teams. A project weekly is the next level of the same bottom-up
//! tree. Its only evidence is each named team's already-written weekly *technical* rollup for
//! the week, run through the same weekly prompt the team rollups use; a plain-language weekly
//! is then written from that technical result, exactly as a team's is. No transcript, phase or
//! other team is read, so the project weekly cannot say anything its inputs did not.
//!
//! The inputs are explicit and checked. Every named team must have a weekly rollup for the
//! week in the selected agent scope, all for the same interval; a missing one is refused by
//! name rather than silently leaving that team out. Each output lists its inputs (team,
//! rollup path, input hash).
//!
//! Outputs live under ``summary_scopes/<scope>/project/<name>/`` in the archive: their own
//! cache, ``rollups/weekly/<week>.json``, Markdown under ``summaries/weekly/``, and a run
//! record per run under ``runs/``. Nothing in a team's tree, in ``summary_data/``, or in the
//! website is touched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent_scope::{validate_agent_scope, ALL_AGENTS};
use crate::build_store::validate_team_slug;
use crate::json::{as_int, as_object, as_string, dump_canonical, read_json};
use crate::pipeline::{archive_writer_lock, utc_now};
use crate::summarize_core::{
    cached_summary, clean_summary_prose, summarize_jobs, SummaryJob, SummaryResult,
};
use crate::summarize_pipeline::{
    summary_from_json, summary_json, summary_root_for_scope, PRESENTATION_FALLBACK_VERSION,
};
use crate::summary_registry::{
    ContextComponent, ContextCoverage, PLAIN_LANGUAGE_ROLLUP_STYLE, TECHNICAL_ROLLUP_STYLE,
};
use crate::transcript_io::{write_json_if_changed, write_text_if_changed};

/// Project weekly record schema version.
pub const PROJECT_WEEKLY_SCHEMA_VERSION: i64 = 1;

/// The line a project weekly built from coordinator-only team weeklies carries under its heading.
pub const PROJECT_COORDINATOR_SCOPE_NOTE: &str = "**Scope: coordinator only.** Every input is a \
team weekly built from that team's root coordinator session and its declared continuations. \
In-session child agents are excluded except as their coordinator saw them, so this is not \
full-team coverage.";

/// One team's weekly technical rollup, as the project weekly reads it.
#[derive(Clone, Debug)]
pub struct ProjectWeeklyInput {
    /// Team slug.
    pub team_slug: String,
    /// Archive-relative rollup path.
    pub rollup_path: String,
    /// Interval start ms.
    pub start_ms: i64,
    /// Interval end ms.
    pub end_ms: i64,
    /// Whether the input covers only part of the week.
    pub partial: bool,
    /// The technical summary.
    pub technical: SummaryResult,
}

/// What one project weekly run read, called and wrote.
#[derive(Clone, Debug)]
pub struct ProjectWeeklyReport {
    /// Project slug.
    pub name: String,
    /// ISO week key.
    pub week: String,
    /// Agent scope read.
    pub agent_scope: String,
    /// Team weekly inputs.
    pub inputs: Vec<ProjectWeeklyInput>,
    /// Technical weekly result.
    pub technical: SummaryResult,
    /// Plain-language weekly result.
    pub plain: SummaryResult,
    /// Written rollup record path.
    pub rollup_path: PathBuf,
    /// Written technical Markdown path.
    pub technical_markdown: PathBuf,
    /// Written plain-language Markdown path.
    pub plain_markdown: PathBuf,
    /// Written run record path.
    pub run_path: PathBuf,
    /// Cache hits.
    pub cache_hits: i64,
    /// Cache misses.
    pub cache_misses: i64,
    /// Backend batches.
    pub backend_batches: i64,
}

/// Where one named project rollup keeps its cache, data, Markdown and run records.
pub fn project_root(archive: &Path, agent_scope: &str, name: &str) -> Result<PathBuf, String> {
    validate_team_slug(name)?;
    Ok(archive
        .join("summary_scopes")
        .join(validate_agent_scope(agent_scope)?)
        .join("project")
        .join(name))
}

fn is_week_key(week: &str) -> bool {
    // [0-9]{4}-W[0-9]{2}
    let bytes = week.as_bytes();
    bytes.len() == 8
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5] == b'W'
        && bytes[6].is_ascii_digit()
        && bytes[7].is_ascii_digit()
}

/// Read every named team's weekly rollup for *week*; refuse, by name, any that is missing.
pub fn load_weekly_inputs(
    archive: &Path,
    team_slugs: &[String],
    week: &str,
    agent_scope: &str,
) -> Result<Vec<ProjectWeeklyInput>, String> {
    if !is_week_key(week) {
        return Err(format!(
            "week must be an ISO week key like 2026-W45, not {}",
            crate::json::py_repr_str(week)
        ));
    }
    if team_slugs.is_empty() {
        return Err("a project weekly needs at least one team".to_string());
    }
    if team_slugs
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != team_slugs.len()
    {
        return Err("a team is named more than once".to_string());
    }
    let mut sorted: Vec<&String> = team_slugs.iter().collect();
    sorted.sort();
    let mut missing: Vec<String> = Vec::new();
    let mut inputs: Vec<ProjectWeeklyInput> = Vec::new();
    for slug in sorted {
        let path = summary_root_for_scope(archive, slug, agent_scope)?
            .join("rollups")
            .join("weekly")
            .join(format!("{week}.json"));
        let relative = path
            .strip_prefix(archive)
            .map(|item| item.display().to_string())
            .unwrap_or_else(|_| path.display().to_string());
        if !path.is_file() {
            missing.push(format!("{slug} ({relative})"));
            continue;
        }
        let where_ = || path.display().to_string();
        let value = read_json(&path)?;
        let obj = as_object(&value, &where_())?;
        if obj.get("kind").and_then(Value::as_str) != Some("weekly")
            || obj.get("key").and_then(Value::as_str) != Some(week)
        {
            // Match the strict field reads first so a malformed record
            // fails the same way, then the week check.
            as_string(
                obj.get("kind").unwrap_or(&Value::Null),
                &format!("{}.kind", where_()),
            )?;
            as_string(
                obj.get("key").unwrap_or(&Value::Null),
                &format!("{}.key", where_()),
            )?;
            return Err(format!("{}: not the weekly rollup for {week}", where_()));
        }
        if let Some(recorded_scope) = obj.get("agent_scope") {
            if recorded_scope.as_str() != Some(agent_scope) {
                return Err(format!(
                    "{}: records agent scope {}, not {}",
                    where_(),
                    crate::json::py_repr_str(&recorded_scope.to_string()),
                    crate::json::py_repr_str(agent_scope)
                ));
            }
        }
        let technical = summary_from_json(
            obj.get("technical_summary").unwrap_or(&Value::Null),
            &format!("{}.technical_summary", where_()),
        )?;
        if technical.prompt_version == PRESENTATION_FALLBACK_VERSION {
            missing.push(format!("{slug} ({relative} holds no model summary)"));
            continue;
        }
        inputs.push(ProjectWeeklyInput {
            team_slug: slug.clone(),
            rollup_path: relative,
            start_ms: as_int(
                obj.get("start_ms").unwrap_or(&Value::Null),
                &format!("{}.start_ms", where_()),
            )?,
            end_ms: as_int(
                obj.get("end_ms").unwrap_or(&Value::Null),
                &format!("{}.end_ms", where_()),
            )?,
            partial: flag(obj.get("partial"), &format!("{}.partial", where_()))?,
            technical,
        });
    }
    if !missing.is_empty() {
        return Err(format!(
            "no {agent_scope}-scope weekly rollup for {week} from: {}. Summarize those teams \
             with --rollup-kind weekly first, or leave them out.",
            missing.join("; ")
        ));
    }
    let intervals: std::collections::HashSet<(i64, i64)> = inputs
        .iter()
        .map(|item| (item.start_ms, item.end_ms))
        .collect();
    if intervals.len() != 1 {
        return Err(format!(
            "the {week} rollups cover different intervals (different timezones?): {}",
            inputs
                .iter()
                .map(|item| format!("{} {}..{}", item.team_slug, item.start_ms, item.end_ms))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(inputs)
}

fn flag(value: Option<&Value>, where_: &str) -> Result<bool, String> {
    match value {
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(format!("{where_}: expected true or false")),
    }
}

/// The technical weekly job: the teams' weekly technical summaries and nothing else.
pub fn project_weekly_job(
    name: &str,
    week: &str,
    inputs: &[ProjectWeeklyInput],
) -> Result<SummaryJob, String> {
    let (start_ms, end_ms) = (inputs[0].start_ms, inputs[0].end_ms);
    let coverage = ContextCoverage {
        components: vec![ContextComponent::new(
            "lower_level_summaries",
            inputs.len() as i64,
            inputs.len() as i64,
            "summaries",
        )?],
        ..ContextCoverage::default()
    };
    let mut dependency_keys: Vec<String> = Vec::new();
    for item in inputs {
        let key = crate::summarize_pipeline::result_artifact_key(&item.technical);
        if !dependency_keys.contains(&key) {
            dependency_keys.push(key);
        }
    }
    Ok(SummaryJob {
        key: format!("project-rollup:weekly:{week}"),
        team_slug: name.to_string(),
        agent_label: format!(
            "Technical weekly super-summary · {week} · {} teams",
            inputs.len()
        ),
        start_ms,
        end_ms,
        prior_context: String::new(),
        transcript: inputs
            .iter()
            .map(|item| {
                crate::summarize_pipeline::result_line(
                    &item.technical,
                    &format!("Team {} weekly", item.team_slug),
                    start_ms,
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        glossary: String::new(),
        stats: BTreeMap::new(),
        summary_style: TECHNICAL_ROLLUP_STYLE.to_string(),
        factual_context: String::new(),
        context_coverage: coverage,
        dependency_keys,
    })
}

fn plain_job(technical_job: &SummaryJob, technical: &SummaryResult, week: &str) -> SummaryJob {
    let mut components = technical_job.context_coverage.components.clone();
    if let Ok(component) = ContextComponent::new("technical_summary", 1, 1, "summaries") {
        components.push(component);
    }
    let mut dependency_keys = technical_job.dependency_keys.clone();
    dependency_keys.push(crate::summarize_pipeline::result_artifact_key(technical));
    SummaryJob {
        key: format!("project-rollup-plain:weekly:{week}"),
        team_slug: technical_job.team_slug.clone(),
        agent_label: technical_job
            .agent_label
            .replacen("Technical", "Plain-language", 1),
        start_ms: technical_job.start_ms,
        end_ms: technical_job.end_ms,
        prior_context: String::new(),
        transcript: technical_job.transcript.clone(),
        glossary: crate::summarize_pipeline::plain_language_context_text("", &[]),
        stats: BTreeMap::new(),
        summary_style: PLAIN_LANGUAGE_ROLLUP_STYLE.to_string(),
        factual_context: crate::summarize_pipeline::result_line(
            technical,
            "Authoritative technical weekly summary",
            technical_job.start_ms,
        ),
        context_coverage: ContextCoverage {
            components,
            ..technical_job.context_coverage.clone()
        },
        dependency_keys,
    }
}

fn local(at_ms: i64, display_timezone: &str) -> Result<String, String> {
    use chrono::TimeZone as _;
    let zone = display_timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| format!("No time zone found with key {display_timezone}"))?;
    let utc = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    Ok(utc
        .with_timezone(&zone)
        .format("%Y-%m-%d %H:%M %Z")
        .to_string())
}

#[allow(clippy::too_many_arguments)]
fn project_markdown(
    name: &str,
    week: &str,
    agent_scope: &str,
    audience: &str,
    summary: &SummaryResult,
    inputs: &[ProjectWeeklyInput],
    display_timezone: &str,
) -> Result<String, String> {
    let scope = if agent_scope != ALL_AGENTS {
        "coordinator-only "
    } else {
        ""
    };
    let partial = inputs.iter().any(|item| item.partial);
    let mut lines: Vec<String> = vec![
        format!("# {week} {name} project weekly {scope}{audience} summary"),
        String::new(),
    ];
    if agent_scope != ALL_AGENTS {
        lines.push(format!("> {PROJECT_COORDINATOR_SCOPE_NOTE}"));
        lines.push(String::new());
    }
    lines.push(format!(
        "> Rolled up from {} team weekly technical summaries ({} to {}{}). It adds no evidence of its own.",
        inputs.len(),
        local(inputs[0].start_ms, display_timezone)?,
        local(inputs[0].end_ms, display_timezone)?,
        if partial {
            "; the inputs cover only part of the week"
        } else {
            ""
        }
    ));
    lines.push(String::new());
    lines.push(format!(
        "**{}.** {}",
        summary.phrase,
        clean_summary_prose(&summary.paragraph)
    ));
    lines.push(String::new());
    lines.push("## Work summary".to_string());
    lines.push(String::new());
    // Every input reaches the model tagged with the week's start, so an
    // item's time is that start, not when the work happened; the page lists
    // the items without one.
    lines.extend(
        summary
            .work_summary
            .iter()
            .map(|item| format!("- {}", item.text)),
    );
    if summary.work_summary.is_empty() {
        lines.push("_No substantive work item was recorded in this range._".to_string());
    }
    lines.extend([String::new(), "## Inputs".to_string(), String::new()]);
    lines.extend(inputs.iter().map(|item| {
        format!(
            "- `{}`: `{}` (input `{}`)",
            item.team_slug, item.rollup_path, item.technical.input_hash
        )
    }));
    lines.extend([
        String::new(),
        "## Summary provenance".to_string(),
        String::new(),
        format!(
            "Model `{}` · prompt `{}` · input `{}`.",
            summary.model, summary.prompt_version, summary.input_hash
        ),
    ]);
    Ok(format!("{}\n", lines.join("\n").trim_end()))
}

/// Return the inputs and whether the technical weekly is already cached; no model call.
#[allow(clippy::too_many_arguments)]
pub fn plan_project_weekly(
    archive: &Path,
    name: &str,
    team_slugs: &[String],
    week: &str,
    backend: &str,
    rollup_model: &str,
    agent_scope: &str,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
) -> Result<(Vec<ProjectWeeklyInput>, bool), String> {
    let inputs = load_weekly_inputs(archive, team_slugs, week, agent_scope)?;
    let job = project_weekly_job(name, week, &inputs)?;
    let cache = project_root(archive, agent_scope, name)?.join("cache");
    let hit = cached_summary(
        &job,
        &cache,
        backend,
        rollup_model,
        reasoning_effort,
        service_tier,
    )?;
    Ok((inputs, hit.is_some()))
}

/// Options for one project weekly run.
#[derive(Clone, Debug)]
pub struct ProjectWeeklyOptions {
    /// IANA display timezone for Markdown times.
    pub display_timezone: String,
    /// Backend reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Backend service tier.
    pub service_tier: Option<String>,
    /// Codex command.
    pub codex_command: Vec<String>,
    /// Claude command.
    pub claude_command: Vec<String>,
    /// Muse command.
    pub muse_command: Vec<String>,
    /// Per-backend-call timeout seconds.
    pub backend_timeout_seconds: Option<f64>,
    /// The invoking command, recorded in the run record.
    pub command: Vec<String>,
}

impl Default for ProjectWeeklyOptions {
    fn default() -> Self {
        Self {
            display_timezone: "UTC".to_string(),
            reasoning_effort: None,
            service_tier: None,
            codex_command: vec!["codex".to_string()],
            claude_command: vec!["claude".to_string()],
            muse_command: vec!["muse".to_string()],
            backend_timeout_seconds: None,
            command: Vec::new(),
        }
    }
}

/// Write the project's technical and plain-language weekly from its teams' weeklies.
///
/// Holds the archive writer lock, so it neither reads a team rollup a
/// summarize run is still writing nor races another project rollup.
#[allow(clippy::too_many_arguments)]
pub fn summarize_project_weekly(
    archive: &Path,
    name: &str,
    team_slugs: &[String],
    week: &str,
    backend: &str,
    rollup_model: &str,
    agent_scope: &str,
    options: &ProjectWeeklyOptions,
) -> Result<ProjectWeeklyReport, String> {
    options
        .display_timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| format!("No time zone found with key {}", options.display_timezone))?;
    let _lock = archive_writer_lock(archive)?;
    summarize_project_weekly_locked(
        archive,
        name,
        team_slugs,
        week,
        backend,
        rollup_model,
        agent_scope,
        options,
    )
}

#[allow(clippy::too_many_arguments)]
fn summarize_project_weekly_locked(
    archive: &Path,
    name: &str,
    team_slugs: &[String],
    week: &str,
    backend: &str,
    rollup_model: &str,
    agent_scope: &str,
    options: &ProjectWeeklyOptions,
) -> Result<ProjectWeeklyReport, String> {
    crate::summarize_core::set_backend_timeout(options.backend_timeout_seconds);
    let started_at = utc_now();
    let inputs = load_weekly_inputs(archive, team_slugs, week, agent_scope)?;
    let root = project_root(archive, agent_scope, name)?;
    let cache = root.join("cache");
    let agent_command: &[String] = if backend == "muse" {
        &options.muse_command
    } else {
        &options.claude_command
    };
    let generate = |job: &SummaryJob| -> Result<(SummaryResult, crate::summarize_core::SummaryRunStats), String> {
        let (results, stats) = summarize_jobs(
            std::slice::from_ref(job),
            &cache,
            backend,
            rollup_model,
            1,
            1,
            &options.codex_command,
            options.reasoning_effort.as_deref(),
            options.service_tier.as_deref(),
            agent_command,
            None,
        )?;
        Ok((results[&job.key].clone(), stats))
    };
    let technical_job = project_weekly_job(name, week, &inputs)?;
    let (technical, technical_stats) = generate(&technical_job)?;
    let (plain, plain_stats) = generate(&plain_job(&technical_job, &technical, week))?;

    let input_records: Vec<Value> = inputs
        .iter()
        .map(|item| {
            serde_json::json!({
                "team": item.team_slug,
                "rollup": item.rollup_path,
                "input_hash": item.technical.input_hash,
            })
        })
        .collect();
    let rollup_path = root
        .join("rollups")
        .join("weekly")
        .join(format!("{week}.json"));
    write_json_if_changed(
        &rollup_path,
        &serde_json::json!({
            "schema_version": PROJECT_WEEKLY_SCHEMA_VERSION,
            "kind": "weekly",
            "key": week,
            "name": name,
            "agent_scope": agent_scope,
            "start_ms": inputs[0].start_ms,
            "end_ms": inputs[0].end_ms,
            "partial": inputs.iter().any(|item| item.partial),
            "inputs": input_records,
            "technical_summary": summary_json(&technical),
            "plain_language_summary": summary_json(&plain),
        }),
    )?;
    let year = week.split("-W").next().unwrap_or_default();
    let markdown_root = root.join("summaries").join("weekly").join(year);
    let technical_markdown = markdown_root.join(format!("{week}-{name}-weekly.md"));
    let plain_markdown = markdown_root.join(format!("{week}-{name}-weekly-plain-language.md"));
    for (path, summary, audience) in [
        (&technical_markdown, &technical, "technical"),
        (&plain_markdown, &plain, "plain-language"),
    ] {
        write_text_if_changed(
            path,
            &project_markdown(
                name,
                week,
                agent_scope,
                audience,
                summary,
                &inputs,
                &options.display_timezone,
            )?,
        )?;
    }
    let hits = technical_stats.hits + plain_stats.hits;
    let misses = technical_stats.misses + plain_stats.misses;
    let batches = technical_stats.batches + plain_stats.batches;
    let completed_at = utc_now();
    let stamp: String = completed_at
        .replace("+00:00", "Z")
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == 'T' || *ch == 'Z')
        .collect();
    let run_path = root.join("runs").join(format!("{stamp}-{week}.json"));
    let outputs: Vec<String> = [&rollup_path, &technical_markdown, &plain_markdown]
        .iter()
        .map(|path| {
            path.strip_prefix(archive)
                .map(|item| item.display().to_string())
                .unwrap_or_else(|_| path.display().to_string())
        })
        .collect();
    let run = serde_json::json!({
        "schema_version": 1,
        "command": options.command,
        "started_at": started_at,
        "completed_at": completed_at,
        "name": name,
        "week": week,
        "agent_scope": agent_scope,
        "backend": backend,
        "model": rollup_model,
        "reasoning_effort": options.reasoning_effort,
        "inputs": input_records_from(&inputs),
        "cache_hits": hits,
        "cache_misses": misses,
        "backend_batches": batches,
        "outputs": outputs,
    });
    if let Some(parent) = run_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&run_path, dump_canonical(&run))
        .map_err(|error| format!("cannot write {}: {error}", run_path.display()))?;
    Ok(ProjectWeeklyReport {
        name: name.to_string(),
        week: week.to_string(),
        agent_scope: agent_scope.to_string(),
        inputs,
        technical,
        plain,
        rollup_path,
        technical_markdown,
        plain_markdown,
        run_path,
        cache_hits: hits,
        cache_misses: misses,
        backend_batches: batches,
    })
}

fn input_records_from(inputs: &[ProjectWeeklyInput]) -> Vec<Value> {
    inputs
        .iter()
        .map(|item| {
            serde_json::json!({
                "team": item.team_slug,
                "rollup": item.rollup_path,
                "input_hash": item.technical.input_hash,
            })
        })
        .collect()
}
