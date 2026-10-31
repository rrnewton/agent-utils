//! The summarize tree at parity with the `summarize_archive`,
//! `plan_summaries`, and `_run_summary_tree` sections of
//! `py/wrkviz/pipeline.py`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::Digest as _;

use crate::build_store::validate_team_slug;
use crate::json::{as_array, as_int, as_object, as_string, dump_canonical, py_repr_str, read_json};
use crate::naming::{cached_agent_name, name_agents, AgentNameJob, AgentNameResult};
use crate::pipeline::load_archived_team;
use crate::summarize_core::{
    cached_summary, carried_forward_summary, char_len, char_take, clean_summary_result, one_line,
    py_trim, record_rollup_inputs, summarize_jobs, SummaryJob, SummaryResult, SummaryRunStats,
    WorkBullet,
};
use crate::summary_artifacts::SummaryArtifactProvenance;
use crate::summary_budget::{
    calibrate, load_state, running_state, write_state, Prices, SpendGuard, StageCount, SummaryPlan,
    AGENT_NAME_KIND, OVERVIEW_KIND, PHASE_KIND,
};
use crate::summary_catalog::{
    load_summary_catalog, merge_summary_catalog, select_summary_artifact, SummaryArtifactCatalog,
    SummaryArtifactReference,
};
use crate::summary_periods::{periods_for_range, Period, ROLLUP_KINDS};
use crate::summary_phases::{aggregate_stats, build_phases, phase_agent_ids, PhaseWindow};
use crate::summary_registry::{
    registry_json_obj, ContextComponent, ContextCoverage, PLAIN_LANGUAGE_ROLLUP_STYLE,
    PROJECT_OVERVIEW_STYLE, PROJECT_OVERVIEW_SUMMARIZER, TECHNICAL_ROLLUP_STYLE,
};
use crate::team_data::{Agent, TeamData};
use crate::token_usage::TokenUsage;
use crate::transcript_io::write_json_if_changed;
use crate::window::{apply_date_window, DateWindow};

const OVERVIEW_CONTEXT_CHARS: usize = 48_000;
const PRESENTATION_FALLBACK_VERSION: &str = "agent-team-timeline-presentation-fallback-v1";
const PROJECT_OVERVIEW_SCHEMA_VERSION: i64 = 3;

/// The summary root for one team.
pub fn summary_root(archive: &Path, team_slug: &str) -> Result<PathBuf, String> {
    validate_team_slug(team_slug)?;
    Ok(archive.join("teams").join(team_slug).join("summary_data"))
}

/// One glossary term (the summarize tree currently supplies none).
#[derive(Clone, Debug)]
pub struct GlossaryTerm {
    /// Exact term.
    pub term: String,
    /// Context.
    pub context: String,
    /// Definition.
    pub definition: String,
    /// Definition status.
    pub definition_status: String,
    /// When it may first affect summaries.
    pub summary_available_at_ms: i64,
}

fn glossary_prompt_text(terms: &[GlossaryTerm]) -> String {
    let mut lines = vec!["Project terminology (prefer these exact names):".to_string()];
    for term in terms {
        lines.push(format!("- {}: {}", term.term, term.context));
    }
    lines.join("\n")
}

fn plain_language_context_text(project_overview: &str, terms: &[GlossaryTerm]) -> String {
    let mut lines = vec![
        "Durable project overview (source-bounded):".to_string(),
        py_trim(project_overview).to_string(),
        String::new(),
        "Evidence-backed project terminology definitions:".to_string(),
    ];
    let supported: Vec<&GlossaryTerm> = terms
        .iter()
        .filter(|term| term.definition_status == "supported")
        .collect();
    if supported.is_empty() {
        lines.push(
            "- No model-backed glossary definitions are supported by current evidence.".to_string(),
        );
    } else {
        for term in supported {
            lines.push(format!("- {}: {}", term.term, term.definition));
        }
    }
    lines.join("\n")
}

#[derive(Clone, Debug)]
struct KnowledgeEpoch {
    epoch_id: String,
    cutoff_ms: i64,
    cutoff_reason: String,
}

impl KnowledgeEpoch {
    fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("epoch_id".into(), Value::String(self.epoch_id.clone()));
        out.insert("cutoff_ms".into(), self.cutoff_ms.into());
        out.insert(
            "cutoff_reason".into(),
            Value::String(self.cutoff_reason.clone()),
        );
        Value::Object(out)
    }
}

#[derive(Clone, Debug)]
struct ProjectOverviewInput {
    epoch: KnowledgeEpoch,
    start_ms: i64,
    end_ms: i64,
    transcript: String,
    event_ids: Vec<String>,
    context_sha256: String,
}

fn sha256_hex(text: &str) -> String {
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn current_knowledge_epoch(team: &TeamData) -> KnowledgeEpoch {
    let (cutoff_ms, reason) = if let Some(end) = team.window_end_ms {
        (end, "archive-window-end")
    } else {
        (
            team.events
                .iter()
                .map(|event| event.timestamp_ms)
                .max()
                .unwrap_or(0)
                + 1,
            "first-summary-source-frontier",
        )
    };
    let digest: String = sha256_hex(&format!("{}\0{}\0{}", team.team_slug, cutoff_ms, reason))
        .chars()
        .take(20)
        .collect();
    KnowledgeEpoch {
        epoch_id: format!("knowledge-{digest}"),
        cutoff_ms,
        cutoff_reason: reason.to_string(),
    }
}

fn overview_input_from_events(
    team: &TeamData,
    epoch: KnowledgeEpoch,
    events: &[crate::team_data::Event],
) -> ProjectOverviewInput {
    let mut parts: Vec<String> = Vec::new();
    let mut event_ids: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut retained: Vec<&crate::team_data::Event> = Vec::new();
    for event in events {
        let prefix = format!("[{}] {}: ", event.timestamp_ms, event.kind);
        let remaining = OVERVIEW_CONTEXT_CHARS.saturating_sub(used);
        if remaining <= char_len(&prefix) {
            break;
        }
        let line = format!(
            "{prefix}{}",
            one_line(event.text.as_deref().unwrap_or_default())
        );
        let excerpt = char_take(&line, remaining);
        if !excerpt.is_empty() {
            parts.push(excerpt.clone());
            event_ids.push(event.event_id.clone());
            retained.push(event);
            used += char_len(&excerpt) + 2;
        }
        if char_len(&excerpt) < char_len(&line) || used >= OVERVIEW_CONTEXT_CHARS {
            break;
        }
    }
    let (start_ms, end_ms) = if !retained.is_empty() {
        (
            retained.first().unwrap().timestamp_ms,
            retained.last().unwrap().timestamp_ms,
        )
    } else {
        let mut fallback = team.window_start_ms;
        if fallback.is_none() || fallback.unwrap() >= epoch.cutoff_ms {
            fallback = Some(
                team.events
                    .iter()
                    .filter(|event| event.timestamp_ms < epoch.cutoff_ms)
                    .map(|event| event.timestamp_ms)
                    .min()
                    .unwrap_or_else(|| (epoch.cutoff_ms - 1).max(0)),
            );
        }
        let fallback = fallback.unwrap();
        parts.push("No root user or assistant transcript text was retained.".to_string());
        (fallback, fallback)
    };
    let transcript = parts.join("\n\n");
    ProjectOverviewInput {
        epoch,
        start_ms,
        end_ms,
        context_sha256: sha256_hex(&transcript),
        transcript,
        event_ids,
    }
}

fn root_overview_input(team: &TeamData, epoch: Option<KnowledgeEpoch>) -> ProjectOverviewInput {
    let epoch = epoch.unwrap_or_else(|| current_knowledge_epoch(team));
    let mut events: Vec<crate::team_data::Event> = team
        .events
        .iter()
        .filter(|event| {
            event.thread_id == team.root_thread_id
                && matches!(event.kind.as_str(), "user_prompt" | "assistant_message")
                && event.text.as_deref().is_some_and(|text| !text.is_empty())
                && event.timestamp_ms < epoch.cutoff_ms
        })
        .cloned()
        .collect();
    events.sort_by(|a, b| {
        a.timestamp_ms
            .cmp(&b.timestamp_ms)
            .then(a.event_id.cmp(&b.event_id))
    });
    overview_input_from_events(team, epoch, &events)
}

fn recorded_overview_input(
    team: &TeamData,
    epoch: KnowledgeEpoch,
    event_ids: &[String],
) -> Option<ProjectOverviewInput> {
    let mut unique = event_ids.to_vec();
    unique.sort();
    unique.dedup();
    if unique.len() != event_ids.len() {
        return None;
    }
    let by_id: HashMap<&str, &crate::team_data::Event> = team
        .events
        .iter()
        .map(|event| (event.event_id.as_str(), event))
        .collect();
    let mut events: Vec<crate::team_data::Event> = Vec::new();
    for event_id in event_ids {
        let event = by_id.get(event_id.as_str())?;
        if event.thread_id != team.root_thread_id
            || event.text.as_deref().is_none_or(|text| text.is_empty())
            || event.timestamp_ms >= epoch.cutoff_ms
        {
            return None;
        }
        let mut event = (*event).clone();
        if !matches!(event.kind.as_str(), "user_prompt" | "assistant_message") {
            if event.kind != "system_input"
                || event.role.as_deref() != Some("user")
                || event.ingress_kind.as_deref() != Some("claude_system")
                || event.author_kind.as_deref() != Some("system")
            {
                return None;
            }
            event.kind = "user_prompt".to_string();
        }
        events.push(event);
    }
    let result = overview_input_from_events(team, epoch, &events);
    if result.event_ids == event_ids {
        Some(result)
    } else {
        None
    }
}

fn renewed_project_overview_input(
    team: &TeamData,
    previous_epoch: &KnowledgeEpoch,
) -> ProjectOverviewInput {
    let current = root_overview_input(team, None);
    let mut payload = serde_json::Map::new();
    payload.insert("team_slug".into(), Value::String(team.team_slug.clone()));
    payload.insert(
        "previous_epoch_id".into(),
        Value::String(previous_epoch.epoch_id.clone()),
    );
    payload.insert("cutoff_ms".into(), current.epoch.cutoff_ms.into());
    payload.insert(
        "cutoff_reason".into(),
        Value::String(current.epoch.cutoff_reason.clone()),
    );
    payload.insert(
        "event_ids".into(),
        Value::Array(
            current
                .event_ids
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    payload.insert(
        "context_sha256".into(),
        Value::String(current.context_sha256.clone()),
    );
    let digest: String = sha256_hex(&dump_canonical(&Value::Object(payload)))
        .chars()
        .take(20)
        .collect();
    let epoch = KnowledgeEpoch {
        epoch_id: format!("knowledge-{digest}"),
        cutoff_ms: current.epoch.cutoff_ms,
        cutoff_reason: current.epoch.cutoff_reason.clone(),
    };
    ProjectOverviewInput { epoch, ..current }
}

fn require_exact_keys(
    obj: &serde_json::Map<String, Value>,
    expected: &[&str],
    where_: &str,
) -> Result<(), String> {
    let mut missing: Vec<&str> = expected
        .iter()
        .filter(|key| !obj.contains_key(**key))
        .copied()
        .collect();
    let mut extra: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|key| !expected.contains(key))
        .collect();
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    missing.sort_unstable();
    extra.sort_unstable();
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        details.push(format!("extra {}", extra.join(", ")));
    }
    Err(format!(
        "{where_}: schema mismatch ({})",
        details.join("; ")
    ))
}

fn knowledge_epoch_from_json(value: &Value, where_: &str) -> Result<KnowledgeEpoch, String> {
    let obj = as_object(value, where_)?;
    require_exact_keys(obj, &["epoch_id", "cutoff_ms", "cutoff_reason"], where_)?;
    let epoch_id = as_string(obj.get("epoch_id").unwrap(), &format!("{where_}.epoch_id"))?;
    if !epoch_id.starts_with("knowledge-") {
        return Err(format!("{where_}.epoch_id: invalid knowledge epoch ID"));
    }
    let cutoff_reason = as_string(
        obj.get("cutoff_reason").unwrap(),
        &format!("{where_}.cutoff_reason"),
    )?;
    if !matches!(
        cutoff_reason,
        "archive-window-end" | "first-summary-source-frontier"
    ) {
        return Err(format!("{where_}.cutoff_reason: invalid cutoff reason"));
    }
    Ok(KnowledgeEpoch {
        epoch_id: epoch_id.to_string(),
        cutoff_ms: as_int(
            obj.get("cutoff_ms").unwrap(),
            &format!("{where_}.cutoff_ms"),
        )?,
        cutoff_reason: cutoff_reason.to_string(),
    })
}

enum FrozenOverview {
    Current(ProjectOverviewInput),
    SourceChanged(KnowledgeEpoch),
    None,
}

fn frozen_project_overview_input(
    archive: &Path,
    team: &TeamData,
) -> Result<FrozenOverview, String> {
    let path = summary_root(archive, &team.team_slug)?.join("project_overview.json");
    if !path.is_file() {
        return Ok(FrozenOverview::None);
    }
    let value = read_json(&path)?;
    let obj = as_object(&value, &path.display().to_string())?;
    if !obj.contains_key("schema_version") {
        return Ok(FrozenOverview::None);
    }
    let version = as_int(
        obj.get("schema_version").unwrap(),
        &format!("{}.schema_version", path.display()),
    )?;
    if version != PROJECT_OVERVIEW_SCHEMA_VERSION {
        return Ok(FrozenOverview::None);
    }
    require_exact_keys(
        obj,
        &[
            "schema_version",
            "style",
            "knowledge_epoch",
            "source",
            "summary",
        ],
        &path.display().to_string(),
    )?;
    if as_string(
        obj.get("style").unwrap(),
        &format!("{}.style", path.display()),
    )? != PROJECT_OVERVIEW_STYLE
    {
        return Err(format!(
            "{}: invalid project-overview style",
            path.display()
        ));
    }
    let epoch = knowledge_epoch_from_json(
        obj.get("knowledge_epoch").unwrap(),
        &format!("{}.knowledge_epoch", path.display()),
    )?;
    let source = as_object(
        obj.get("source").unwrap(),
        &format!("{}.source", path.display()),
    )?;
    require_exact_keys(
        source,
        &["event_ids", "start_ms", "end_ms", "context_sha256"],
        &format!("{}.source", path.display()),
    )?;
    let mut event_ids = Vec::new();
    for item in as_array(
        source.get("event_ids").unwrap(),
        &format!("{}.source.event_ids", path.display()),
    )? {
        event_ids
            .push(as_string(item, &format!("{}.source.event_ids[]", path.display()))?.to_string());
    }
    let start_ms = as_int(
        source.get("start_ms").unwrap(),
        &format!("{}.source.start_ms", path.display()),
    )?;
    let end_ms = as_int(
        source.get("end_ms").unwrap(),
        &format!("{}.source.end_ms", path.display()),
    )?;
    let expected_sha = as_string(
        source.get("context_sha256").unwrap(),
        &format!("{}.source.context_sha256", path.display()),
    )?;
    if end_ms < start_ms || end_ms >= epoch.cutoff_ms {
        return Err(format!(
            "{}.source: timestamps escape the knowledge epoch",
            path.display()
        ));
    }
    let outside_window =
        team.window_end_ms.is_some() && epoch.cutoff_ms > team.window_end_ms.unwrap();
    let validation_team = if outside_window {
        load_archived_team(archive, &team.team_slug)?
    } else {
        team.clone()
    };
    let current = root_overview_input(&validation_team, Some(epoch.clone()));
    let exact = current.event_ids == event_ids
        && current.start_ms == start_ms
        && current.end_ms == end_ms
        && current.context_sha256 == expected_sha;
    if !exact {
        let reconstructed = recorded_overview_input(&validation_team, epoch.clone(), &event_ids);
        let unchanged = reconstructed.as_ref().is_some_and(|item| {
            item.start_ms == start_ms
                && item.end_ms == end_ms
                && item.context_sha256 == expected_sha
        });
        if unchanged && current.event_ids != event_ids {
            return Ok(FrozenOverview::SourceChanged(epoch));
        }
        return Err(format!(
            "{}.source: frozen overview evidence was mutated or truncated",
            path.display()
        ));
    }
    if outside_window {
        Ok(FrozenOverview::None)
    } else {
        Ok(FrozenOverview::Current(current))
    }
}

fn project_overview_job(team: &TeamData, source: &ProjectOverviewInput) -> SummaryJob {
    let mut stats = std::collections::BTreeMap::new();
    stats.insert("source_events".into(), source.event_ids.len() as i64);
    stats.insert(
        "source_characters".into(),
        char_len(&source.transcript) as i64,
    );
    let coverage = ContextCoverage::new(
        vec![ContextComponent::new(
            "early_root_transcript",
            OVERVIEW_CONTEXT_CHARS as i64,
            (OVERVIEW_CONTEXT_CHARS).min(char_len(&source.transcript)) as i64,
            "characters",
        )
        .unwrap_or_else(|_| ContextComponent {
            name: "early_root_transcript".into(),
            requested: OVERVIEW_CONTEXT_CHARS as i64,
            provided: (OVERVIEW_CONTEXT_CHARS).min(char_len(&source.transcript)) as i64,
            unit: "characters".into(),
        })],
        "not-applicable",
        Vec::new(),
        true,
    )
    .unwrap_or_default();
    let mut job = SummaryJob::phase(
        &format!("project-overview:{}", team.team_slug),
        &team.team_slug,
        &format!("{} project overview", team.team_slug),
        source.start_ms,
        source.end_ms,
        String::new(),
        source.transcript.clone(),
        String::new(),
        stats,
    );
    job.summary_style = PROJECT_OVERVIEW_STYLE.to_string();
    job.context_coverage = coverage;
    job
}

fn phase_jobs(
    team: &TeamData,
    phases: &[PhaseWindow],
    terms: &[GlossaryTerm],
    context_chars: i64,
) -> Vec<SummaryJob> {
    phases
        .iter()
        .map(|phase| {
            let chronological: Vec<GlossaryTerm> = terms
                .iter()
                .filter(|term| term.summary_available_at_ms < phase.end_ms)
                .cloned()
                .collect();
            let coverage = ContextCoverage::new(
                vec![ContextComponent {
                    name: "ancestor_transcript".into(),
                    requested: context_chars,
                    provided: context_chars.min(char_len(&phase.prior_context) as i64),
                    unit: "characters".into(),
                }],
                "not-applicable",
                Vec::new(),
                true,
            )
            .unwrap_or_default();
            let mut job = SummaryJob::phase(
                &phase.summary_key,
                &team.team_slug,
                &phase.agent_label,
                phase.start_ms,
                phase.end_ms,
                phase.prior_context.clone(),
                phase.transcript_text.clone(),
                glossary_prompt_text(&chronological),
                phase.stats.to_mapping(),
            );
            job.context_coverage = coverage;
            job
        })
        .collect()
}

/// A summary result as persisted JSON.
pub fn summary_json(summary: &SummaryResult) -> Value {
    let cleaned = clean_summary_result(summary);
    let mut out = serde_json::Map::new();
    out.insert("key".into(), Value::String(cleaned.key.clone()));
    out.insert("phrase".into(), Value::String(cleaned.phrase.clone()));
    out.insert("paragraph".into(), Value::String(cleaned.paragraph.clone()));
    out.insert(
        "work_summary".into(),
        Value::Array(
            cleaned
                .work_summary
                .iter()
                .map(|bullet| {
                    let mut item = serde_json::Map::new();
                    item.insert("at_ms".into(), bullet.at_ms.into());
                    item.insert("text".into(), Value::String(bullet.text.clone()));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    out.insert("model".into(), Value::String(cleaned.model.clone()));
    out.insert(
        "prompt_version".into(),
        Value::String(cleaned.prompt_version.clone()),
    );
    out.insert(
        "input_hash".into(),
        Value::String(cleaned.input_hash.clone()),
    );
    out.insert(
        "generated_at".into(),
        Value::String(cleaned.generated_at.clone()),
    );
    if let Some(provenance) = &cleaned.artifact_provenance {
        out.insert("artifact_provenance".into(), provenance.to_json_obj());
    }
    Value::Object(out)
}

fn summary_from_json(value: &Value, where_: &str) -> Result<SummaryResult, String> {
    let obj = as_object(value, where_)?;
    let base = [
        "key",
        "phrase",
        "paragraph",
        "work_summary",
        "model",
        "prompt_version",
        "input_hash",
        "generated_at",
    ];
    let has_provenance = obj.contains_key("artifact_provenance");
    if obj.len() != base.len() + usize::from(has_provenance)
        || !base.iter().all(|key| obj.contains_key(*key))
    {
        require_exact_keys(obj, &base, where_)?;
    }
    let raw_bullets = as_array(
        obj.get("work_summary").unwrap(),
        &format!("{where_}.work_summary"),
    )?;
    let mut bullets = Vec::new();
    for (index, raw) in raw_bullets.iter().enumerate() {
        let bullet = as_object(raw, &format!("{where_}.work_summary[{index}]"))?;
        require_exact_keys(
            bullet,
            &["at_ms", "text"],
            &format!("{where_}.work_summary[{index}]"),
        )?;
        bullets.push(WorkBullet {
            at_ms: as_int(bullet.get("at_ms").unwrap(), "at_ms")?,
            text: as_string(bullet.get("text").unwrap(), "text")?.to_string(),
        });
    }
    let get = |key: &str| as_string(obj.get(key).unwrap(), &format!("{where_}.{key}"));
    Ok(clean_summary_result(&SummaryResult {
        key: get("key")?.to_string(),
        phrase: get("phrase")?.to_string(),
        paragraph: get("paragraph")?.to_string(),
        work_summary: bullets,
        model: get("model")?.to_string(),
        prompt_version: get("prompt_version")?.to_string(),
        input_hash: get("input_hash")?.to_string(),
        generated_at: get("generated_at")?.to_string(),
        artifact_provenance: if has_provenance {
            Some(SummaryArtifactProvenance::from_json_obj(
                obj.get("artifact_provenance"),
                &format!("{where_}.artifact_provenance"),
            )?)
        } else {
            None
        },
        summary_available: true,
    }))
}

fn summary_from_catalog_reference(
    summary_root: &Path,
    reference: &SummaryArtifactReference,
) -> Result<SummaryResult, String> {
    let path = summary_root.join(&reference.cache_path);
    let value = read_json(&path)?;
    let root = as_object(&value, &path.display().to_string())?;
    let provenance = &reference.provenance;
    if root.get("format").and_then(Value::as_str)
        == Some(crate::summary_artifacts::ARTIFACT_ENVELOPE_FORMAT)
    {
        require_exact_keys(
            root,
            &["format", "schema_version", "artifact", "result"],
            &path.display().to_string(),
        )?;
        if as_int(
            root.get("schema_version").unwrap(),
            &format!("{}.schema_version", path.display()),
        )? != crate::summary_artifacts::ARTIFACT_ENVELOPE_VERSION
        {
            return Err(format!(
                "{}: unsupported model-artifact envelope",
                path.display()
            ));
        }
        let cached = SummaryArtifactProvenance::from_json_obj(
            root.get("artifact"),
            &format!("{}.artifact", path.display()),
        )?;
        if cached != *provenance {
            return Err(format!(
                "{}: cache provenance differs from artifact catalog",
                path.display()
            ));
        }
    } else if !provenance.legacy_storage {
        return Err(format!(
            "{}: cataloged artifact lacks its common envelope",
            path.display()
        ));
    }
    let result = summary_from_json(
        root.get("result").unwrap_or(&Value::Null),
        &format!("{}.result", path.display()),
    )?;
    if result.input_hash != provenance.input_hash
        || result.model != provenance.model
        || result.prompt_version != provenance.prompt_version
    {
        return Err(format!(
            "{}: cached result differs from artifact provenance",
            path.display()
        ));
    }
    Ok(SummaryResult {
        artifact_provenance: Some(provenance.clone()),
        ..result
    })
}

fn period_key(period: &Period) -> String {
    format!("{}:{}", period.key, period.kind)
}

fn phases_in<'a>(period: &Period, phases: &'a [PhaseWindow]) -> Vec<&'a PhaseWindow> {
    phases
        .iter()
        .filter(|phase| phase.start_ms < period.end_ms && phase.end_ms > period.start_ms)
        .collect()
}

fn result_line(result: &SummaryResult, label: &str, at_ms: i64) -> String {
    let bullets = result
        .work_summary
        .iter()
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    py_trim(&format!(
        "[{at_ms}] {label}: {}. {} {bullets}",
        result.phrase, result.paragraph
    ))
    .to_string()
}

fn result_artifact_key(result: &SummaryResult) -> String {
    match &result.artifact_provenance {
        Some(provenance) => provenance.artifact_id.clone(),
        None => result.input_hash.clone(),
    }
}

fn fallback_generated_at(at_ms: i64) -> String {
    use chrono::TimeZone as _;
    let dt = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    if dt.timestamp_subsec_micros() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    }
}

/// An ephemeral unavailable-summary placeholder.
pub fn unavailable_summary(key: &str, at_ms: i64, subject: &str) -> SummaryResult {
    let identity = sha256_hex(&format!(
        "{PRESENTATION_FALLBACK_VERSION}\0{key}\0{subject}"
    ));
    SummaryResult {
        key: key.to_string(),
        phrase: "Summary unavailable".into(),
        paragraph: format!(
            "Summary unavailable: no cached model summary exists for this {subject}. The transcript and statistics shown here come directly from normalized logs."
        ),
        work_summary: Vec::new(),
        model: "none".into(),
        prompt_version: PRESENTATION_FALLBACK_VERSION.into(),
        input_hash: identity,
        generated_at: fallback_generated_at(at_ms),
        artifact_provenance: None,
        summary_available: false,
    }
}

fn prior_catalog_rollups(
    summary_root: &Path,
    catalog: &SummaryArtifactCatalog,
    kind: &str,
    summary_style: &str,
    before_ms: i64,
) -> Result<Vec<(Period, SummaryResult)>, String> {
    let plain = summary_style == PLAIN_LANGUAGE_ROLLUP_STYLE;
    let summarizer_id = if plain {
        "plain-language-rollup"
    } else {
        "technical-rollup"
    };
    let key_prefix = if plain { "rollup-plain" } else { "rollup" };
    let logical_prefix = format!("{key_prefix}:{kind}:");
    let mut logical_keys: Vec<String> = catalog
        .records
        .iter()
        .filter(|record| {
            record.provenance.summarizer_id == summarizer_id
                && record.provenance.logical_key.starts_with(&logical_prefix)
                && record.provenance.end_ms <= before_ms
        })
        .map(|record| record.provenance.logical_key.clone())
        .collect();
    logical_keys.sort();
    logical_keys.dedup();
    let mut result: Vec<(Period, SummaryResult)> = Vec::new();
    for logical_key in logical_keys {
        let Some(reference) =
            select_summary_artifact(catalog, &logical_key, summarizer_id, 1, None)?
        else {
            continue;
        };
        let provenance = &reference.provenance;
        let key = logical_key[logical_prefix.len()..].to_string();
        result.push((
            Period {
                kind: kind.to_string(),
                key: key.clone(),
                label: format!("Prior {kind} · {key}"),
                start_ms: provenance.start_ms,
                end_ms: provenance.end_ms,
                relative_path: String::new(),
                partial: false,
            },
            summary_from_catalog_reference(summary_root, &reference)?,
        ));
    }
    result.sort_by(|a, b| a.0.end_ms.cmp(&b.0.end_ms).then(a.0.key.cmp(&b.0.key)));
    Ok(result.into_iter().rev().take(10).rev().collect())
}

fn rollup_children<'a>(
    period: &Period,
    lower_periods: &[Period],
    phases: &'a [PhaseWindow],
) -> (Vec<Period>, Vec<&'a PhaseWindow>) {
    let own_phases = phases_in(period, phases);
    let lower: Vec<Period> = lower_periods
        .iter()
        .filter(|item| item.start_ms >= period.start_ms && item.end_ms <= period.end_ms)
        .cloned()
        .collect();
    let uncovered = own_phases
        .into_iter()
        .filter(|phase| {
            !lower
                .iter()
                .any(|item| item.start_ms <= phase.start_ms && phase.end_ms <= item.end_ms)
        })
        .collect();
    (lower, uncovered)
}

fn prior_same_level<'a>(
    completed: &'a [(Period, SummaryResult)],
    period: &Period,
) -> Vec<&'a (Period, SummaryResult)> {
    let earlier: Vec<&(Period, SummaryResult)> = completed
        .iter()
        .filter(|(item, _)| item.end_ms <= period.start_ms)
        .collect();
    earlier[earlier.len().saturating_sub(10)..].to_vec()
}

#[allow(clippy::too_many_arguments)]
fn rollup_job_for_period(
    team: &TeamData,
    period: &Period,
    phases: &[PhaseWindow],
    phase_results: &HashMap<String, SummaryResult>,
    lower_periods: &[Period],
    lower_results: &HashMap<String, SummaryResult>,
    completed_same_level: &[(Period, SummaryResult)],
    terms: &[GlossaryTerm],
    summary_style: &str,
    project_overview: Option<&SummaryResult>,
    same_period_technical: Option<&HashMap<String, SummaryResult>>,
) -> Result<SummaryJob, String> {
    let own_phases = phases_in(period, phases);
    let (lower, uncovered_phases) = rollup_children(period, lower_periods, phases);
    let mut transcript_parts: Vec<(i64, String)> = Vec::new();
    let mut dependency_results: Vec<&SummaryResult> = Vec::new();
    for item in &lower {
        let result = &lower_results[&period_key(item)];
        transcript_parts.push((
            item.start_ms,
            result_line(result, &item.label, item.start_ms),
        ));
        dependency_results.push(result);
    }
    for phase in &uncovered_phases {
        let result = &phase_results[&phase.summary_key];
        transcript_parts.push((
            phase.start_ms,
            result_line(result, &phase.agent_label, phase.start_ms),
        ));
        dependency_results.push(result);
    }
    transcript_parts.sort_by_key(|item| item.0);
    let transcript = transcript_parts
        .iter()
        .map(|item| item.1.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let prior = prior_same_level(completed_same_level, period);
    let prior_context = prior
        .iter()
        .map(|(item, result)| result_line(result, &item.label, item.start_ms))
        .collect::<Vec<_>>()
        .join("\n\n");
    dependency_results.extend(prior.iter().map(|(_, result)| result));
    let stats = aggregate_stats(
        &own_phases
            .iter()
            .map(|phase| (*phase).clone())
            .collect::<Vec<_>>(),
    );
    let plain = summary_style == PLAIN_LANGUAGE_ROLLUP_STYLE;
    let key_prefix = if plain { "rollup-plain" } else { "rollup" };
    let audience = if plain { "Plain-language" } else { "Technical" };
    let technical_result = if plain {
        same_period_technical.and_then(|map| map.get(&period_key(period)))
    } else {
        None
    };
    if plain && technical_result.is_none() {
        return Err(format!(
            "plain-language {} rollup {} lacks its same-period technical summary",
            period.kind,
            py_repr_str(&period.key)
        ));
    }
    let chronological: Vec<GlossaryTerm> = terms
        .iter()
        .filter(|term| term.summary_available_at_ms < period.end_ms)
        .cloned()
        .collect();
    let mut glossary = glossary_prompt_text(&chronological);
    if plain {
        glossary = plain_language_context_text(
            project_overview.map_or("", |item| item.paragraph.as_str()),
            &chronological,
        );
        if let Some(overview) = project_overview {
            dependency_results.push(overview);
        }
        if let Some(technical) = technical_result {
            dependency_results.push(technical);
        }
    }
    let earliest_activity = team
        .events
        .iter()
        .map(|event| event.timestamp_ms)
        .min()
        .unwrap_or(period.start_ms);
    let previous_period = completed_same_level.last().map(|(item, _)| item);
    let frontier_status =
        if period.start_ms <= earliest_activity && earliest_activity < period.end_ms {
            "project-start"
        } else if previous_period.is_some_and(|item| item.end_ms == period.start_ms) {
            "contiguous-extension"
        } else {
            "isolated-backfill"
        };
    let mut expected_prior = 0i64;
    if earliest_activity < period.start_ms {
        expected_prior = periods_for_range(
            earliest_activity,
            period.start_ms - 1,
            &team.display_timezone,
            &team.team_slug,
            std::slice::from_ref(&period.kind),
        )?
        .len()
        .min(10) as i64;
    }
    let mut components = vec![
        ContextComponent {
            name: "lower_level_summaries".into(),
            requested: transcript_parts.len() as i64,
            provided: transcript_parts.len() as i64,
            unit: "summaries".into(),
        },
        ContextComponent {
            name: "prior_same_level_summaries".into(),
            requested: expected_prior,
            provided: expected_prior.min(prior.len() as i64),
            unit: "summaries".into(),
        },
    ];
    if plain && project_overview.is_some() {
        components.push(ContextComponent {
            name: "project_overview".into(),
            requested: 1,
            provided: 1,
            unit: "artifacts".into(),
        });
    }
    if technical_result.is_some() {
        components.push(ContextComponent {
            name: "technical_summary".into(),
            requested: 1,
            provided: 1,
            unit: "summaries".into(),
        });
    }
    let coverage = ContextCoverage::new(
        components,
        frontier_status,
        prior.iter().map(|(item, _)| period_key(item)).collect(),
        true,
    )
    .unwrap_or_default();
    let mut dependency_keys: Vec<String> = Vec::new();
    for result in dependency_results {
        let key = result_artifact_key(result);
        if !dependency_keys.contains(&key) {
            dependency_keys.push(key);
        }
    }
    let mut job = SummaryJob::phase(
        &format!("{key_prefix}:{}:{}", period.kind, period.key),
        &team.team_slug,
        &format!(
            "{audience} {} super-summary · {}",
            period.kind, period.label
        ),
        period.start_ms,
        period.end_ms,
        prior_context,
        transcript,
        glossary,
        stats.to_mapping(),
    );
    job.summary_style = summary_style.to_string();
    job.factual_context = technical_result.map_or(String::new(), |result| {
        result_line(
            result,
            &format!("Authoritative technical {} summary", period.kind),
            period.start_ms,
        )
    });
    job.context_coverage = coverage;
    job.dependency_keys = dependency_keys;
    Ok(job)
}

type RollupMaps = (
    HashMap<String, SummaryResult>,
    HashMap<String, SummaryResult>,
);

type RollupResolver<'a> =
    &'a mut dyn FnMut(&SummaryJob, bool, bool) -> Result<Option<SummaryResult>, String>;

#[allow(clippy::too_many_arguments)]
fn walk_rollups(
    team: &TeamData,
    periods: &[Period],
    rollup_kinds: &[String],
    start_ms: i64,
    phases: &[PhaseWindow],
    phase_results: &HashMap<String, SummaryResult>,
    project_overview: &SummaryResult,
    terms: &[GlossaryTerm],
    summary_root: &Path,
    catalog: &SummaryArtifactCatalog,
    resolve: RollupResolver<'_>,
    unknown_phase_keys: &BTreeSet<String>,
    overview_known: bool,
) -> Result<RollupMaps, String> {
    let selected_kinds: Vec<&str> = ROLLUP_KINDS
        .iter()
        .copied()
        .filter(|kind| rollup_kinds.iter().any(|item| item == kind))
        .collect();
    let mut all_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut all_plain_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    let mut unknown_plain: BTreeSet<String> = BTreeSet::new();
    let mut previous_periods: Vec<Period> = Vec::new();
    let mut previous_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut previous_plain_results: HashMap<String, SummaryResult> = HashMap::new();
    for kind in selected_kinds {
        let current: Vec<Period> = periods
            .iter()
            .filter(|period| period.kind == kind)
            .cloned()
            .collect();
        let first_start = current.first().map_or(start_ms, |item| item.start_ms);
        let mut completed = prior_catalog_rollups(
            summary_root,
            catalog,
            kind,
            TECHNICAL_ROLLUP_STYLE,
            first_start,
        )?;
        let mut completed_plain = prior_catalog_rollups(
            summary_root,
            catalog,
            kind,
            PLAIN_LANGUAGE_ROLLUP_STYLE,
            first_start,
        )?;
        for period in &current {
            let key = period_key(period);
            let (lower, uncovered) = rollup_children(period, &previous_periods, phases);
            let children_known = !uncovered
                .iter()
                .any(|phase| unknown_phase_keys.contains(&phase.summary_key));
            let job = rollup_job_for_period(
                team,
                period,
                phases,
                phase_results,
                &previous_periods,
                &previous_results,
                &completed,
                terms,
                TECHNICAL_ROLLUP_STYLE,
                None,
                None,
            )?;
            let own_known =
                children_known && !lower.iter().any(|item| unknown.contains(&period_key(item)));
            let prior_known = !prior_same_level(&completed, period)
                .iter()
                .any(|(item, _)| unknown.contains(&period_key(item)));
            let mut result = resolve(&job, own_known, prior_known)?;
            if result.is_none() {
                unknown.insert(key.clone());
                result = Some(unavailable_summary(
                    &job.key,
                    period.end_ms,
                    &format!("planned {kind} rollup"),
                ));
            }
            let result = result.unwrap();
            all_results.insert(key.clone(), result.clone());
            completed.push((period.clone(), result.clone()));
            let mut same_period: HashMap<String, SummaryResult> = HashMap::new();
            same_period.insert(key.clone(), result);
            let plain_job = rollup_job_for_period(
                team,
                period,
                phases,
                phase_results,
                &previous_periods,
                &previous_plain_results,
                &completed_plain,
                terms,
                PLAIN_LANGUAGE_ROLLUP_STYLE,
                Some(project_overview),
                Some(&same_period),
            )?;
            let plain_own_known = overview_known
                && children_known
                && !unknown.contains(&key)
                && !lower
                    .iter()
                    .any(|item| unknown_plain.contains(&period_key(item)));
            let plain_prior_known = !prior_same_level(&completed_plain, period)
                .iter()
                .any(|(item, _)| unknown_plain.contains(&period_key(item)));
            let mut plain_result = resolve(&plain_job, plain_own_known, plain_prior_known)?;
            if plain_result.is_none() {
                unknown_plain.insert(key.clone());
                plain_result = Some(unavailable_summary(
                    &plain_job.key,
                    period.end_ms,
                    &format!("planned plain-language {kind} rollup"),
                ));
            }
            let plain_result = plain_result.unwrap();
            all_plain_results.insert(key.clone(), plain_result.clone());
            completed_plain.push((period.clone(), plain_result));
        }
        previous_periods = current.clone();
        previous_results = current
            .iter()
            .map(|period| (period_key(period), all_results[&period_key(period)].clone()))
            .collect();
        previous_plain_results = current
            .iter()
            .map(|period| {
                (
                    period_key(period),
                    all_plain_results[&period_key(period)].clone(),
                )
            })
            .collect();
    }
    Ok((all_results, all_plain_results))
}

fn agent_name_jobs(
    team: &TeamData,
    phases: &[PhaseWindow],
    phase_results: &HashMap<String, SummaryResult>,
    context_chars: i64,
) -> Vec<AgentNameJob> {
    let agents_by_id: HashMap<&str, &Agent> = team
        .agents
        .iter()
        .map(|agent| (agent.thread_id.as_str(), agent))
        .collect();
    let mut phases_by_agent: HashMap<String, Vec<&PhaseWindow>> = HashMap::new();
    for phase in phases {
        phases_by_agent
            .entry(phase.agent_id.clone())
            .or_default()
            .push(phase);
    }
    let selected_ids = phase_agent_ids(team, phases);
    let mut jobs = Vec::new();
    for agent in &team.agents {
        if !selected_ids.contains(&agent.thread_id) {
            continue;
        }
        let mut own_phases: Vec<&PhaseWindow> = phases_by_agent
            .get(&agent.thread_id)
            .cloned()
            .unwrap_or_default();
        own_phases.sort_by(|a, b| {
            a.start_ms
                .cmp(&b.start_ms)
                .then(a.phase_id.cmp(&b.phase_id))
        });
        let work_lines: Vec<String> = own_phases
            .iter()
            .map(|phase| {
                result_line(
                    &phase_results[&phase.summary_key],
                    &phase.agent_label,
                    phase.start_ms,
                )
            })
            .collect();
        let parent = agent
            .parent_thread_id
            .as_ref()
            .and_then(|parent| agents_by_id.get(parent.as_str()))
            .copied();
        let (job_start, job_end) = if !own_phases.is_empty() {
            (
                own_phases.first().unwrap().start_ms,
                own_phases.last().unwrap().end_ms,
            )
        } else {
            let start = agent
                .started_at_ms
                .max(team.window_start_ms.unwrap_or(agent.started_at_ms));
            let natural_end = agent
                .ended_at_ms
                .unwrap_or(team.window_end_ms.unwrap_or(start));
            (
                start,
                start.max(natural_end.min(team.window_end_ms.unwrap_or(natural_end))),
            )
        };
        let coverage = ContextCoverage::new(
            vec![
                ContextComponent {
                    name: "ancestor_transcript".into(),
                    requested: context_chars,
                    provided: context_chars.min(
                        own_phases
                            .first()
                            .map_or(0, |phase| char_len(&phase.prior_context) as i64),
                    ),
                    unit: "characters".into(),
                },
                ContextComponent {
                    name: "phase_work_summaries".into(),
                    requested: own_phases.len() as i64,
                    provided: own_phases.len() as i64,
                    unit: "summaries".into(),
                },
            ],
            "not-applicable",
            Vec::new(),
            true,
        )
        .unwrap_or_default();
        jobs.push(AgentNameJob {
            key: format!("agent-name:{}", agent.thread_id),
            team_slug: team.team_slug.clone(),
            thread_id: agent.thread_id.clone(),
            start_ms: job_start,
            end_ms: job_end,
            official_path: agent.agent_path.clone(),
            coordinator_nickname: agent.nickname.clone(),
            role: agent.role.clone(),
            depth: agent.depth,
            parent_official_path: parent.map(|item| item.agent_path.clone()),
            prior_context: own_phases
                .first()
                .map_or(String::new(), |phase| phase.prior_context.clone()),
            work_summary: if work_lines.is_empty() {
                "No substantive phase summary was available for this thread.".to_string()
            } else {
                work_lines.join("\n\n")
            },
            context_coverage: coverage,
            dependency_keys: own_phases
                .iter()
                .map(|phase| result_artifact_key(&phase_results[&phase.summary_key]))
                .collect(),
        });
    }
    jobs
}

fn agent_name_json(job: &AgentNameJob, result: &AgentNameResult) -> Value {
    let opt = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let mut name = serde_json::Map::new();
    name.insert("thread_id".into(), Value::String(result.thread_id.clone()));
    name.insert(
        "short_name".into(),
        Value::String(result.short_name.clone()),
    );
    name.insert("rationale".into(), Value::String(result.rationale.clone()));
    name.insert("lifetime_summary".into(), opt(&result.lifetime_summary));
    name.insert("model".into(), Value::String(result.model.clone()));
    name.insert(
        "prompt_version".into(),
        Value::String(result.prompt_version.clone()),
    );
    name.insert(
        "input_hash".into(),
        Value::String(result.input_hash.clone()),
    );
    name.insert(
        "generated_at".into(),
        Value::String(result.generated_at.clone()),
    );
    if let Some(provenance) = &result.artifact_provenance {
        name.insert("artifact_provenance".into(), provenance.to_json_obj());
    }
    let mut agent = serde_json::Map::new();
    agent.insert("thread_id".into(), Value::String(job.thread_id.clone()));
    agent.insert(
        "official_path".into(),
        Value::String(job.official_path.clone()),
    );
    agent.insert(
        "coordinator_nickname".into(),
        opt(&job.coordinator_nickname),
    );
    agent.insert("role".into(), opt(&job.role));
    agent.insert("depth".into(), job.depth.into());
    agent.insert(
        "parent_official_path".into(),
        opt(&job.parent_official_path),
    );
    let mut out = serde_json::Map::new();
    out.insert("schema_version".into(), 3.into());
    out.insert("agent".into(), Value::Object(agent));
    out.insert("name".into(), Value::Object(name));
    Value::Object(out)
}

fn write_phase_data(
    archive: &Path,
    team_slug: &str,
    phases: &[PhaseWindow],
    results: &HashMap<String, SummaryResult>,
) -> Result<i64, String> {
    let mut changed = 0i64;
    for phase in phases {
        let mut obj = serde_json::Map::new();
        obj.insert("phase_id".into(), Value::String(phase.phase_id.clone()));
        obj.insert("agent_id".into(), Value::String(phase.agent_id.clone()));
        obj.insert("start_ms".into(), phase.start_ms.into());
        obj.insert("end_ms".into(), phase.end_ms.into());
        obj.insert("summary".into(), summary_json(&results[&phase.summary_key]));
        if write_json_if_changed(
            &summary_root(archive, team_slug)?
                .join("phases")
                .join(format!("{}.json", phase.phase_id)),
            &Value::Object(obj),
        )? {
            changed += 1;
        }
    }
    Ok(changed)
}

fn write_agent_name_data(
    archive: &Path,
    team_slug: &str,
    jobs: &[AgentNameJob],
    results: &HashMap<String, AgentNameResult>,
) -> Result<i64, String> {
    let mut changed = 0i64;
    for job in jobs {
        if write_json_if_changed(
            &summary_root(archive, team_slug)?
                .join("agents")
                .join(format!("{}.json", job.thread_id)),
            &agent_name_json(job, &results[&job.thread_id]),
        )? {
            changed += 1;
        }
    }
    Ok(changed)
}

fn validate_project_overview(summary: &SummaryResult, where_: &str) -> Result<(), String> {
    if crate::summary_registry::summarizer_change_for_prompt(
        &PROJECT_OVERVIEW_SUMMARIZER,
        &summary.prompt_version,
    )
    .is_err()
    {
        return Err(format!("{where_}: unregistered project overview version"));
    }
    if !matches!(
        summary.phrase.as_str(),
        "Project overview supported" | "Insufficient evidence"
    ) {
        return Err(format!(
            "{where_}: invalid project-overview evidence status"
        ));
    }
    if !summary.work_summary.is_empty() {
        return Err(format!(
            "{where_}: project overview must not contain work bullets"
        ));
    }
    if summary.phrase == "Insufficient evidence"
        && !summary.paragraph.starts_with("Insufficient evidence:")
    {
        return Err(format!(
            "{where_}: insufficient overview lacks an evidence limit"
        ));
    }
    if crate::summarize_core::overview_text_has_link(&summary.paragraph) {
        return Err(format!(
            "{where_}: project overview contains an unverified link"
        ));
    }
    Ok(())
}

fn write_project_overview_data(
    archive: &Path,
    team_slug: &str,
    source: &ProjectOverviewInput,
    summary: &SummaryResult,
) -> Result<i64, String> {
    let mut source_obj = serde_json::Map::new();
    source_obj.insert(
        "event_ids".into(),
        Value::Array(
            source
                .event_ids
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    source_obj.insert("start_ms".into(), source.start_ms.into());
    source_obj.insert("end_ms".into(), source.end_ms.into());
    source_obj.insert(
        "context_sha256".into(),
        Value::String(source.context_sha256.clone()),
    );
    let mut obj = serde_json::Map::new();
    obj.insert(
        "schema_version".into(),
        PROJECT_OVERVIEW_SCHEMA_VERSION.into(),
    );
    obj.insert("style".into(), Value::String(PROJECT_OVERVIEW_STYLE.into()));
    obj.insert("knowledge_epoch".into(), source.epoch.to_json_obj());
    obj.insert("source".into(), Value::Object(source_obj));
    obj.insert("summary".into(), summary_json(summary));
    Ok(i64::from(write_json_if_changed(
        &summary_root(archive, team_slug)?.join("project_overview.json"),
        &Value::Object(obj),
    )?))
}

/// One summarize run's report.
#[derive(Clone, Debug)]
pub struct SummarizeReport {
    /// Backend.
    pub backend: String,
    /// Model.
    pub model: String,
    /// Reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Service tier.
    pub service_tier: Option<String>,
    /// Phases.
    pub phases: i64,
    /// Rollups (periods).
    pub rollups: i64,
    /// Agent names.
    pub agent_names: i64,
    /// Glossary terms.
    pub glossary_terms: i64,
    /// Project overviews.
    pub project_overviews: i64,
    /// Glossary definitions.
    pub glossary_definitions: i64,
    /// Catalog artifacts.
    pub catalog_artifacts: i64,
    /// Cache hits.
    pub cache_hits: i64,
    /// Cache misses.
    pub cache_misses: i64,
    /// Backend batches.
    pub backend_batches: i64,
    /// Newly spent usage.
    pub newly_spent_usage: TokenUsage,
    /// New receipts with unknown usage.
    pub newly_spent_unknown_receipts: i64,
    /// Usage behind artifacts.
    pub artifact_generation_usage: TokenUsage,
    /// Artifact receipts with unknown usage.
    pub artifact_generation_unknown_receipts: i64,
    /// Legacy artifacts.
    pub unknown_legacy_artifacts: i64,
    /// Usage run paths (archive-relative).
    pub usage_run_paths: Vec<String>,
    /// Files changed.
    pub files_changed: i64,
    /// Carried-forward rollups.
    pub carried_forward_rollups: i64,
}

impl SummarizeReport {
    /// Persisted report form.
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("backend".into(), Value::String(self.backend.clone()));
        out.insert("model".into(), Value::String(self.model.clone()));
        out.insert(
            "reasoning_effort".into(),
            self.reasoning_effort
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "service_tier".into(),
            self.service_tier.clone().map_or(Value::Null, Value::String),
        );
        out.insert("phases".into(), self.phases.into());
        out.insert("rollups".into(), self.rollups.into());
        out.insert("plain_language_rollups".into(), self.rollups.into());
        out.insert("rollup_summary_artifacts".into(), (self.rollups * 2).into());
        out.insert("agent_names".into(), self.agent_names.into());
        out.insert("glossary_terms".into(), self.glossary_terms.into());
        out.insert("project_overviews".into(), self.project_overviews.into());
        out.insert(
            "glossary_definitions".into(),
            self.glossary_definitions.into(),
        );
        out.insert("catalog_artifacts".into(), self.catalog_artifacts.into());
        out.insert("cache_hits".into(), self.cache_hits.into());
        out.insert("cache_misses".into(), self.cache_misses.into());
        out.insert("backend_batches".into(), self.backend_batches.into());
        out.insert("newly_spent_usage".into(), self.newly_spent_usage.to_json());
        out.insert(
            "newly_spent_unknown_receipts".into(),
            self.newly_spent_unknown_receipts.into(),
        );
        out.insert(
            "artifact_generation_usage".into(),
            self.artifact_generation_usage.to_json(),
        );
        out.insert(
            "artifact_generation_unknown_receipts".into(),
            self.artifact_generation_unknown_receipts.into(),
        );
        out.insert(
            "unknown_legacy_artifacts".into(),
            self.unknown_legacy_artifacts.into(),
        );
        out.insert(
            "usage_run_paths".into(),
            Value::Array(
                self.usage_run_paths
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert("files_changed".into(), self.files_changed.into());
        out.insert(
            "carried_forward_rollups".into(),
            self.carried_forward_rollups.into(),
        );
        Value::Object(out)
    }
}

struct SummaryTree {
    team: TeamData,
    summary_root: PathBuf,
    catalog: SummaryArtifactCatalog,
    phases: Vec<PhaseWindow>,
    phase_jobs: Vec<SummaryJob>,
    overview_source: ProjectOverviewInput,
    overview_job: SummaryJob,
    start_ms: i64,
    periods: Vec<Period>,
}

fn period_range(team: &TeamData, phases: &[PhaseWindow]) -> Result<(i64, i64), String> {
    let phase_start = phases.iter().map(|phase| phase.start_ms).min();
    let phase_end = phases.iter().map(|phase| phase.end_ms).max();
    let start_ms = team.window_start_ms.or(phase_start);
    let end_ms = team.window_end_ms.or(phase_end);
    let (Some(start_ms), Some(end_ms)) = (start_ms, end_ms) else {
        return Err(
            "selected timeline range contains no activity and lacks both date bounds".to_string(),
        );
    };
    if end_ms <= start_ms {
        return Err("selected timeline range is empty".to_string());
    }
    Ok((start_ms, end_ms - 1))
}

/// Options for one summarize invocation.
#[derive(Clone, Debug)]
pub struct SummarizeOptions {
    /// Max workers.
    pub max_workers: usize,
    /// Summary batch size.
    pub batch_size: usize,
    /// Name batch size.
    pub name_batch_size: usize,
    /// Phase minutes.
    pub phase_minutes: i64,
    /// Context chars.
    pub context_chars: i64,
    /// Transcript chars.
    pub transcript_chars: i64,
    /// Codex command.
    pub codex_command: Vec<String>,
    /// Claude command.
    pub claude_command: Vec<String>,
    /// Reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Service tier.
    pub service_tier: Option<String>,
    /// Summary window.
    pub summary_window: Option<DateWindow>,
    /// Rollup kinds.
    pub rollup_kinds: Vec<String>,
    /// Budget USD.
    pub budget_usd: Option<f64>,
    /// Prices.
    pub prices: Option<Prices>,
}

fn summary_tree(
    archive: &Path,
    team_slug: &str,
    options: &SummarizeOptions,
) -> Result<SummaryTree, String> {
    let mut team = load_archived_team(archive, team_slug)?;
    if options.summary_window.is_some() {
        team = apply_date_window(team, options.summary_window.as_ref());
    }
    let root = summary_root(archive, team_slug)?;
    let phases = build_phases(
        &team,
        options.phase_minutes,
        options.context_chars,
        options.transcript_chars,
    )?;
    let overview_source = match frozen_project_overview_input(archive, &team)? {
        FrozenOverview::Current(input) => input,
        FrozenOverview::SourceChanged(epoch) => renewed_project_overview_input(&team, &epoch),
        FrozenOverview::None => root_overview_input(&team, None),
    };
    // Frozen may return None above via the None arm; root input covers it.
    let overview_job = project_overview_job(&team, &overview_source);
    let (start_ms, end_inclusive) = period_range(&team, &phases)?;
    let periods = periods_for_range(
        start_ms,
        end_inclusive,
        &team.display_timezone,
        &team.team_slug,
        &options.rollup_kinds,
    )?;
    Ok(SummaryTree {
        catalog: load_summary_catalog(&root.join("artifacts.json"), team_slug)?,
        phase_jobs: phase_jobs(&team, &phases, &[], options.context_chars),
        overview_source,
        overview_job,
        start_ms,
        periods,
        summary_root: root,
        phases,
        team,
    })
}

fn request_json(
    tree: &SummaryTree,
    backend: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
    rollup_kinds: &[String],
) -> Value {
    let opt = |value: Option<&str>| value.map_or(Value::Null, |text| Value::String(text.into()));
    let opt_num = |value: Option<i64>| value.map_or(Value::Null, Into::into);
    let mut out = serde_json::Map::new();
    out.insert("backend".into(), Value::String(backend.into()));
    out.insert("model".into(), Value::String(model.into()));
    out.insert("reasoning_effort".into(), opt(reasoning_effort));
    out.insert("service_tier".into(), opt(service_tier));
    out.insert("window_start_ms".into(), opt_num(tree.team.window_start_ms));
    out.insert("window_end_ms".into(), opt_num(tree.team.window_end_ms));
    out.insert(
        "rollup_kinds".into(),
        Value::Array(
            rollup_kinds
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        ),
    );
    Value::Object(out)
}

fn rollup_index(summary_root: &Path) -> PathBuf {
    summary_root.join("rollup_inputs")
}

fn unfinished_state(summary_root: &Path) -> Option<Value> {
    let state = load_state(summary_root)?;
    if let Value::Object(obj) = &state {
        if obj.get("status").and_then(Value::as_str) == Some("complete") {
            return None;
        }
    }
    Some(state)
}

fn plan_tree(
    tree: &SummaryTree,
    backend: &str,
    model: &str,
    options: &SummarizeOptions,
) -> Result<SummaryPlan, String> {
    let cache = tree.summary_root.join("cache");
    let name_cache = tree.summary_root.join("name_cache");
    let index = rollup_index(&tree.summary_root);
    let reasoning_effort = options.reasoning_effort.as_deref();
    let service_tier = options.service_tier.as_deref();
    let mut stages: Vec<StageCount> = Vec::new();
    let add_stage = |name: &str, kind: &str, stages: &mut Vec<StageCount>| {
        stages.push(StageCount {
            name: name.to_string(),
            kind: kind.to_string(),
            ..StageCount::default()
        });
        stages.len() - 1
    };
    let phase_stage = add_stage("phase summaries", PHASE_KIND, &mut stages);
    let mut phase_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut unknown_phase_keys: BTreeSet<String> = BTreeSet::new();
    for job in &tree.phase_jobs {
        stages[phase_stage].total += 1;
        let hit = cached_summary(job, &cache, backend, model, reasoning_effort, service_tier)?;
        match hit {
            None => {
                stages[phase_stage].to_generate += 1;
                unknown_phase_keys.insert(job.key.clone());
                phase_results.insert(
                    job.key.clone(),
                    unavailable_summary(&job.key, job.end_ms, "planned phase"),
                );
            }
            Some(hit) => {
                stages[phase_stage].cached += 1;
                phase_results.insert(job.key.clone(), hit);
            }
        }
    }

    let name_stage = add_stage("agent names", AGENT_NAME_KIND, &mut stages);
    let agents_with_unknown: BTreeSet<String> = tree
        .phases
        .iter()
        .filter(|phase| unknown_phase_keys.contains(&phase.summary_key))
        .map(|phase| phase.agent_id.clone())
        .collect();
    for name_job in agent_name_jobs(
        &tree.team,
        &tree.phases,
        &phase_results,
        options.context_chars,
    ) {
        stages[name_stage].total += 1;
        let cached = if agents_with_unknown.contains(&name_job.thread_id) {
            None
        } else {
            cached_agent_name(
                &name_job,
                &name_cache,
                backend,
                model,
                reasoning_effort,
                service_tier,
            )?
        };
        match cached {
            Some(_) => stages[name_stage].cached += 1,
            None => stages[name_stage].to_generate += 1,
        }
    }

    let overview_stage = add_stage("project overview", OVERVIEW_KIND, &mut stages);
    stages[overview_stage].total = 1;
    let overview_cached = cached_summary(
        &tree.overview_job,
        &cache,
        backend,
        model,
        reasoning_effort,
        service_tier,
    )?;
    let overview = match overview_cached {
        None => {
            stages[overview_stage].to_generate = 1;
            unavailable_summary(
                &tree.overview_job.key,
                tree.overview_job.end_ms,
                "planned project overview",
            )
        }
        Some(hit) => {
            stages[overview_stage].cached = 1;
            hit
        }
    };
    let overview_known = stages[overview_stage].cached == 1;

    let mut rollup_stage_index: HashMap<(String, String), usize> = HashMap::new();
    for kind in ROLLUP_KINDS {
        if options.rollup_kinds.iter().any(|item| item == kind) {
            for (style, audience) in [
                (TECHNICAL_ROLLUP_STYLE, "technical"),
                (PLAIN_LANGUAGE_ROLLUP_STYLE, "plain-language"),
            ] {
                let index = add_stage(&format!("{kind} rollups ({audience})"), style, &mut stages);
                rollup_stage_index.insert((kind.to_string(), style.to_string()), index);
            }
        }
    }

    let mut resolve = |job: &SummaryJob,
                       own_known: bool,
                       prior_known: bool|
     -> Result<Option<SummaryResult>, String> {
        let kind = job.key.split(':').nth(1).unwrap_or_default().to_string();
        let stage_index = rollup_stage_index[&(kind, job.summary_style.clone())];
        stages[stage_index].total += 1;
        if own_known && prior_known {
            let hit = cached_summary(job, &cache, backend, model, reasoning_effort, service_tier)?;
            if let Some(hit) = hit {
                stages[stage_index].cached += 1;
                return Ok(Some(hit));
            }
        }
        if own_known {
            let carried = carried_forward_summary(
                job,
                &index,
                &cache,
                backend,
                model,
                reasoning_effort,
                service_tier,
            )?;
            if let Some(carried) = carried {
                stages[stage_index].carried_forward += 1;
                return Ok(Some(carried));
            }
        }
        stages[stage_index].to_generate += 1;
        Ok(None)
    };

    walk_rollups(
        &tree.team,
        &tree.periods,
        &options.rollup_kinds,
        tree.start_ms,
        &tree.phases,
        &phase_results,
        &overview,
        &[],
        &tree.summary_root,
        &tree.catalog,
        &mut resolve,
        &unknown_phase_keys,
        overview_known,
    )?;

    Ok(SummaryPlan {
        team_slug: tree.team.team_slug.clone(),
        stages,
        costs: calibrate(&cache, &name_cache, backend, model, reasoning_effort),
        prices: options.prices,
        budget_usd: options.budget_usd,
        previous_state: unfinished_state(&tree.summary_root),
        request: Some(request_json(
            tree,
            backend,
            model,
            reasoning_effort,
            service_tier,
            &options.rollup_kinds,
        )),
    })
}

/// Compute what a summarize run would do, without any model call.
pub fn plan_summaries(
    archive: &Path,
    team_slug: &str,
    backend: &str,
    model: &str,
    options: &SummarizeOptions,
) -> Result<SummaryPlan, String> {
    let effective =
        crate::token_usage::resolve_service_tier(backend, options.service_tier.as_deref())?;
    let mut tree_options = options.clone();
    tree_options.service_tier = effective.clone();
    // Python resolves the tier inside the locked helpers; keep the
    // effective value for job identities.
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    let tree = summary_tree(archive, team_slug, &tree_options)?;
    plan_tree(&tree, backend, model, &tree_options)
}

fn accumulate_stats(stats: &[SummaryRunStats]) -> SummaryRunStats {
    let mut out = SummaryRunStats::default();
    for item in stats {
        out.hits += item.hits;
        out.misses += item.misses;
        out.batches += item.batches;
        out.newly_spent_usage += item.newly_spent_usage;
        out.newly_spent_unknown_receipts += item.newly_spent_unknown_receipts;
        out.artifact_generation_usage += item.artifact_generation_usage;
        out.artifact_generation_unknown_receipts += item.artifact_generation_unknown_receipts;
        out.unknown_legacy_artifacts += item.unknown_legacy_artifacts;
    }
    out
}

/// Failure modes the CLI reports with distinct exit codes.
#[derive(Debug)]
pub enum SummarizeFailure {
    /// Ordinary failure (exit 2).
    Message(String),
    /// Plan over budget (exit 3).
    BudgetExceeded(String),
    /// Budget reached mid-run (exit 3).
    BudgetStop(String),
    /// Interrupted by SIGINT (exit 130).
    Interrupted,
}

impl SummarizeFailure {
    /// Error text.
    pub fn message(&self) -> &str {
        match self {
            Self::Message(text) | Self::BudgetExceeded(text) | Self::BudgetStop(text) => text,
            Self::Interrupted => "interrupted by user",
        }
    }

    /// Whether this is a budget failure (exit 3).
    pub fn is_budget(&self) -> bool {
        matches!(self, Self::BudgetExceeded(_) | Self::BudgetStop(_))
    }
}

fn classify_error(error: String) -> SummarizeFailure {
    if crate::summarize_core::interrupted() {
        SummarizeFailure::Interrupted
    } else if error.starts_with("budget reached:") {
        SummarizeFailure::BudgetStop(error)
    } else {
        SummarizeFailure::Message(error)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_summary_tree(
    archive: &Path,
    tree: &SummaryTree,
    backend: &str,
    model: &str,
    progress: &std::cell::RefCell<String>,
    guard: Option<&SpendGuard>,
    options: &SummarizeOptions,
) -> Result<SummarizeReport, String> {
    let team = &tree.team;
    let team_slug = &team.team_slug;
    let summary_root = &tree.summary_root;
    let reasoning_effort = options.reasoning_effort.as_deref();
    let service_tier = options.service_tier.as_deref();
    let mut changed: i64 = 0;
    if write_json_if_changed(&summary_root.join("summarizers.json"), &registry_json_obj())? {
        changed += 1;
    }
    let cache = summary_root.join("cache");
    let index = rollup_index(summary_root);
    *progress.borrow_mut() = "phase summaries".to_string();
    let (phase_results, phase_stats) = summarize_jobs(
        &tree.phase_jobs,
        &cache,
        backend,
        model,
        options.max_workers,
        options.batch_size,
        &options.codex_command,
        reasoning_effort,
        service_tier,
        &options.claude_command,
        guard,
    )?;
    changed += write_phase_data(archive, team_slug, &tree.phases, &phase_results)?;
    *progress.borrow_mut() = "agent names".to_string();
    let name_jobs_list = agent_name_jobs(team, &tree.phases, &phase_results, options.context_chars);
    let (agent_names, name_stats) = name_agents(
        &name_jobs_list,
        &summary_root.join("name_cache"),
        backend,
        model,
        options.max_workers,
        options.name_batch_size,
        &options.codex_command,
        reasoning_effort,
        service_tier,
        &options.claude_command,
        guard,
    )?;
    changed += write_agent_name_data(archive, team_slug, &name_jobs_list, &agent_names)?;
    *progress.borrow_mut() = "project overview".to_string();
    let (overview_results, overview_stats) = summarize_jobs(
        std::slice::from_ref(&tree.overview_job),
        &cache,
        backend,
        model,
        options.max_workers,
        options.batch_size,
        &options.codex_command,
        reasoning_effort,
        service_tier,
        &options.claude_command,
        guard,
    )?;
    let project_overview = overview_results[&tree.overview_job.key].clone();
    validate_project_overview(&project_overview, "generated project overview")?;
    changed +=
        write_project_overview_data(archive, team_slug, &tree.overview_source, &project_overview)?;

    let mut backend_stats: Vec<SummaryRunStats> = vec![phase_stats, name_stats, overview_stats];
    let mut carried_forward = 0i64;

    let mut resolve = |job: &SummaryJob,
                       _own_known: bool,
                       _prior_known: bool|
     -> Result<Option<SummaryResult>, String> {
        *progress.borrow_mut() = format!(
            "{} rollups ({})",
            job.key.split(':').nth(1).unwrap_or_default(),
            job.key
        );
        if cached_summary(job, &cache, backend, model, reasoning_effort, service_tier)?.is_none() {
            let carried = carried_forward_summary(
                job,
                &index,
                &cache,
                backend,
                model,
                reasoning_effort,
                service_tier,
            )?;
            if let Some(carried) = carried {
                carried_forward += 1;
                return Ok(Some(carried));
            }
        }
        let (results, stats) = summarize_jobs(
            std::slice::from_ref(job),
            &cache,
            backend,
            model,
            options.max_workers,
            options.batch_size,
            &options.codex_command,
            reasoning_effort,
            service_tier,
            &options.claude_command,
            guard,
        )?;
        backend_stats.push(stats);
        let result = results[&job.key].clone();
        if record_rollup_inputs(&index, job, &result)? {
            changed += 1;
        }
        Ok(Some(result))
    };

    let (all_results, all_plain_results) = walk_rollups(
        team,
        &tree.periods,
        &options.rollup_kinds,
        tree.start_ms,
        &tree.phases,
        &phase_results,
        &project_overview,
        &[],
        summary_root,
        &tree.catalog,
        &mut resolve,
        &BTreeSet::new(),
        true,
    )?;
    *progress.borrow_mut() = "writing rollups".to_string();
    for period in &tree.periods {
        let key = period_key(period);
        let result = &all_results[&key];
        let plain_result = &all_plain_results[&key];
        let mut obj = serde_json::Map::new();
        obj.insert("schema_version".into(), 2.into());
        obj.insert("kind".into(), Value::String(period.kind.clone()));
        obj.insert("key".into(), Value::String(period.key.clone()));
        obj.insert("start_ms".into(), period.start_ms.into());
        obj.insert("end_ms".into(), period.end_ms.into());
        obj.insert("partial".into(), Value::Bool(period.partial));
        obj.insert("technical_summary".into(), summary_json(result));
        obj.insert("plain_language_summary".into(), summary_json(plain_result));
        if write_json_if_changed(
            &summary_root
                .join("rollups")
                .join(&period.kind)
                .join(format!("{}.json", period.key)),
            &Value::Object(obj),
        )? {
            changed += 1;
        }
    }

    let mut summary_results: Vec<SummaryResult> = tree
        .phase_jobs
        .iter()
        .map(|job| phase_results[&job.key].clone())
        .collect();
    summary_results.push(project_overview.clone());
    summary_results.extend(all_results.values().cloned());
    summary_results.extend(all_plain_results.values().cloned());
    let mut catalog_additions: Vec<SummaryArtifactReference> = Vec::new();
    for result in &summary_results {
        catalog_additions.push(catalog_reference(result, "cache")?);
    }
    for result in agent_names.values() {
        catalog_additions.push(catalog_reference_name(result, "name_cache")?);
    }
    for reference in &catalog_additions {
        let cache_path = summary_root.join(&reference.cache_path);
        if !cache_path.is_file() {
            return Err(format!(
                "summary artifact cache entry is missing: {}",
                cache_path.display()
            ));
        }
    }
    let (catalog, catalog_changed) = merge_summary_catalog(
        &summary_root.join("artifacts.json"),
        team_slug,
        &catalog_additions,
    )?;
    if catalog_changed {
        changed += 1;
    }
    let combined = accumulate_stats(&backend_stats);
    let usage_run_paths: Vec<String> = backend_stats
        .iter()
        .filter_map(|item| item.usage_run_path.as_ref())
        .map(|path| {
            path.strip_prefix(archive)
                .map(|item| item.display().to_string())
                .unwrap_or_else(|_| path.display().to_string())
        })
        .collect();
    Ok(SummarizeReport {
        backend: backend.to_string(),
        model: model.to_string(),
        reasoning_effort: options.reasoning_effort.clone(),
        service_tier: service_tier.map(str::to_string),
        phases: tree.phases.len() as i64,
        rollups: tree.periods.len() as i64,
        agent_names: agent_names.len() as i64,
        glossary_terms: 0,
        project_overviews: 1,
        glossary_definitions: 0,
        catalog_artifacts: catalog.records.len() as i64,
        cache_hits: combined.hits,
        cache_misses: combined.misses,
        backend_batches: combined.batches,
        newly_spent_usage: combined.newly_spent_usage,
        newly_spent_unknown_receipts: combined.newly_spent_unknown_receipts,
        artifact_generation_usage: combined.artifact_generation_usage,
        artifact_generation_unknown_receipts: combined.artifact_generation_unknown_receipts,
        unknown_legacy_artifacts: combined.unknown_legacy_artifacts,
        usage_run_paths,
        files_changed: changed,
        carried_forward_rollups: carried_forward,
    })
}

fn catalog_reference(
    result: &SummaryResult,
    cache_directory: &str,
) -> Result<SummaryArtifactReference, String> {
    let Some(provenance) = &result.artifact_provenance else {
        return Err(format!(
            "model artifact {} lacks common provenance",
            py_repr_str(&result.input_hash)
        ));
    };
    SummaryArtifactReference::new(
        provenance.clone(),
        &format!("{cache_directory}/{}.json", result.input_hash),
    )
}

fn catalog_reference_name(
    result: &AgentNameResult,
    cache_directory: &str,
) -> Result<SummaryArtifactReference, String> {
    let Some(provenance) = &result.artifact_provenance else {
        return Err(format!(
            "model artifact {} lacks common provenance",
            py_repr_str(&result.input_hash)
        ));
    };
    SummaryArtifactReference::new(
        provenance.clone(),
        &format!("{cache_directory}/{}.json", result.input_hash),
    )
}

/// Run summarize over one archived team.
pub fn summarize_archive(
    archive: &Path,
    team_slug: &str,
    backend: &str,
    model: &str,
    options: &SummarizeOptions,
    on_plan: &dyn Fn(&SummaryPlan),
) -> Result<SummarizeReport, SummarizeFailure> {
    let _lock = crate::pipeline::archive_writer_lock(archive).map_err(SummarizeFailure::Message)?;
    summarize_archive_locked(archive, team_slug, backend, model, options, on_plan)
}

fn summarize_archive_locked(
    archive: &Path,
    team_slug: &str,
    backend: &str,
    model: &str,
    options: &SummarizeOptions,
    on_plan: &dyn Fn(&SummaryPlan),
) -> Result<SummarizeReport, SummarizeFailure> {
    let effective =
        crate::token_usage::resolve_service_tier(backend, options.service_tier.as_deref())
            .map_err(SummarizeFailure::Message)?;
    let mut options = options.clone();
    options.service_tier = effective.clone();
    let options = &options;
    if options.budget_usd.is_some() && options.prices.is_none() && backend != "heuristic" {
        return Err(SummarizeFailure::Message(
            "a summary budget needs model prices (--price)".to_string(),
        ));
    }
    let tree = summary_tree(archive, team_slug, options).map_err(SummarizeFailure::Message)?;
    let plan = plan_tree(&tree, backend, model, options).map_err(SummarizeFailure::Message)?;
    on_plan(&plan);
    if plan.over_budget() {
        let estimate = plan.estimated_usd().unwrap_or(0.0);
        return Err(SummarizeFailure::BudgetExceeded(format!(
            "refusing to start: the remaining summary tree is estimated at ${estimate:.2}, over the ${:.2} budget; nothing was spent. Raise --budget-usd, narrow the summary window, or drop rollup levels.",
            options.budget_usd.unwrap_or(0.0)
        )));
    }
    let guard = match (options.budget_usd, options.prices) {
        (Some(budget), Some(prices)) => Some(
            SpendGuard::new(budget, prices, plan.costs.clone())
                .map_err(SummarizeFailure::Message)?,
        ),
        _ => None,
    };
    let summary_root = tree.summary_root.clone();
    let mut state = running_state(
        &request_json(
            &tree,
            backend,
            model,
            options.reasoning_effort.as_deref(),
            effective.as_deref(),
            &options.rollup_kinds,
        ),
        &plan,
        load_state(&summary_root).as_ref(),
    );
    write_state(&summary_root, &state).map_err(SummarizeFailure::Message)?;
    let progress = std::cell::RefCell::new("phase summaries".to_string());
    let report = run_summary_tree(
        archive,
        &tree,
        backend,
        model,
        &progress,
        guard.as_ref(),
        options,
    );
    match report {
        Ok(report) => {
            state.insert("status".into(), Value::String("complete".into()));
            state.insert(
                "spent_usd".into(),
                guard.as_ref().map_or(Value::Null, |guard| {
                    let rounded = (guard.spent_usd() * 10_000.0).round() / 10_000.0;
                    serde_json::Number::from_f64(rounded).map_or(Value::Null, Value::Number)
                }),
            );
            write_state(&summary_root, &state).map_err(SummarizeFailure::Message)?;
            Ok(report)
        }
        Err(error) => {
            let failure = classify_error(error.clone());
            let reason = if matches!(failure, SummarizeFailure::Interrupted) {
                "interrupted"
            } else if failure.is_budget() && error.starts_with("budget reached:") {
                "budget"
            } else {
                "error"
            };
            state.insert("status".into(), Value::String("partial".into()));
            state.insert(
                "spent_usd".into(),
                guard.as_ref().map_or(Value::Null, |guard| {
                    let rounded = (guard.spent_usd() * 10_000.0).round() / 10_000.0;
                    serde_json::Number::from_f64(rounded).map_or(Value::Null, Value::Number)
                }),
            );
            let mut stopped = serde_json::Map::new();
            stopped.insert("reason".into(), Value::String(reason.into()));
            stopped.insert("stage".into(), Value::String(progress.borrow().clone()));
            stopped.insert(
                "detail".into(),
                Value::String(if matches!(failure, SummarizeFailure::Interrupted) {
                    String::new()
                } else {
                    char_take(&one_line(&error), 500)
                }),
            );
            state.insert("stopped".into(), Value::Object(stopped));
            let _ = write_state(&summary_root, &state);
            Err(failure)
        }
    }
}
