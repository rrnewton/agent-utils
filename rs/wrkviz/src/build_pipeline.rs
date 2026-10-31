//! Build orchestration at parity with `pipeline._build_archive_locked`:
//! cache loaders (phase/agent/rollup/overview), artifact catalog and
//! site identity loading, and the locked `build_archive` entry point.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::artifacts::ArtifactCatalog;
use crate::build_assets::{GlossaryTerm, PullRequestMetadata};
use crate::build_render::{render_archive, BuildReport, RenderInputs};
use crate::identity::{site_identity_from_json, SiteIdentity};
use crate::json::{as_array, as_int, as_object, as_string, py_repr_str, read_json};
use crate::naming::AgentNameResult;
use crate::pipeline::{archive_writer_lock, ensure_archive_pub, load_archived_team};
use crate::summarize_core::{
    carried_forward_matches, clean_summary_prose, input_hash_for_provenance, SummaryJob,
    SummaryResult,
};
use crate::summarize_pipeline::{
    agent_name_jobs, frozen_project_overview_input, period_key, period_range, phase_jobs,
    phases_in, prior_catalog_rollups, require_exact_keys, rollup_index, rollup_job_for_period,
    root_overview_input, summary_from_catalog_reference, summary_from_json, summary_root,
    unavailable_summary, validate_project_overview, FrozenOverview,
    PROJECT_OVERVIEW_SCHEMA_VERSION,
};
use crate::summary_artifacts::SummaryArtifactProvenance;
use crate::summary_catalog::{
    load_summary_catalog, select_summary_artifact, SummaryArtifactCatalog, SummaryArtifactReference,
};
use crate::summary_periods::{periods_for_range, Period, DEFAULT_ROLLUP_KINDS, ROLLUP_KINDS};
use crate::summary_phases::{aggregate_stats, build_phases, PhaseWindow};
use crate::summary_registry::{summarizer_change_for_prompt, AGENT_LIFETIME_SUMMARIZER};
use crate::summary_registry::{PLAIN_LANGUAGE_ROLLUP_STYLE, TECHNICAL_ROLLUP_STYLE};
use crate::team_data::{source_digest, Agent, TeamData};

type ValidatedRollups = (
    HashMap<String, SummaryResult>,
    HashMap<String, SummaryResult>,
);
use crate::window::DateWindow;

const PRESENTATION_FALLBACK_VERSION: &str = "agent-team-timeline-presentation-fallback-v1";

fn summary_projection_missing(path: &Path) -> Result<bool, String> {
    match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(crate::json::os_error(&error, path)),
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                let mode = {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt as _;
                        metadata.mode()
                    }
                    #[cfg(not(unix))]
                    {
                        0
                    }
                };
                let _ = mode;
                return Err(format!(
                    "{}: summary projection is not a regular file",
                    path.display()
                ));
            }
            Ok(false)
        }
    }
}

fn summary_provenance_matches_job(
    provenance: &SummaryArtifactProvenance,
    job: &SummaryJob,
) -> bool {
    if provenance.logical_key != job.key
        || provenance.team_slug != job.team_slug
        || provenance.start_ms != job.start_ms
        || provenance.end_ms != job.end_ms
        || provenance.dependency_keys != job.dependency_keys
        || provenance.context_coverage != job.context_coverage
    {
        return false;
    }
    input_hash_for_provenance(job, provenance).ok().as_deref()
        == Some(provenance.input_hash.as_str())
}

fn summary_matches_job(result: &SummaryResult, job: &SummaryJob) -> bool {
    match &result.artifact_provenance {
        Some(provenance) => {
            provenance.input_hash == result.input_hash
                && summary_provenance_matches_job(provenance, job)
        }
        None => false,
    }
}

fn select_catalog_summary(
    summary_root: &Path,
    catalog: &SummaryArtifactCatalog,
    logical_key: &str,
    summarizer_id: &str,
    fallback: SummaryResult,
) -> Result<SummaryResult, String> {
    match select_summary_artifact(catalog, logical_key, summarizer_id, 1, None)? {
        None => Ok(fallback),
        Some(reference) => summary_from_catalog_reference(summary_root, &reference),
    }
}

fn select_catalog_summary_for_job(
    summary_root: &Path,
    catalog: &SummaryArtifactCatalog,
    job: &SummaryJob,
    summarizer_id: &str,
    fallback: SummaryResult,
) -> Result<SummaryResult, String> {
    let compatible = SummaryArtifactCatalog {
        team_slug: catalog.team_slug.clone(),
        records: catalog
            .records
            .iter()
            .filter(|reference| {
                reference.provenance.summarizer_id == summarizer_id
                    && summary_provenance_matches_job(&reference.provenance, job)
            })
            .cloned()
            .collect(),
    };
    match select_summary_artifact(&compatible, &job.key, summarizer_id, 1, None)? {
        None => Ok(fallback),
        Some(reference) => summary_from_catalog_reference(summary_root, &reference),
    }
}

fn load_phase_summaries(
    archive: &Path,
    team_slug: &str,
    phases: &[PhaseWindow],
    jobs: &[SummaryJob],
    catalog: &SummaryArtifactCatalog,
) -> Result<HashMap<String, SummaryResult>, String> {
    let root = summary_root(archive, team_slug)?;
    let mut jobs_by_key: HashMap<&str, &SummaryJob> = HashMap::new();
    for job in jobs {
        if jobs_by_key.insert(job.key.as_str(), job).is_some() {
            return Err("duplicate phase summary job key".into());
        }
    }
    let mut result = HashMap::new();
    for phase in phases {
        let path = root.join("phases").join(format!("{}.json", phase.phase_id));
        let job = jobs_by_key.get(phase.summary_key.as_str()).ok_or_else(|| {
            format!(
                "missing phase summary job {}",
                py_repr_str(&phase.summary_key)
            )
        })?;
        let fallback = unavailable_summary(&phase.summary_key, phase.end_ms, "work phase");
        let mut candidate = if summary_projection_missing(&path)? {
            select_catalog_summary_for_job(
                &root,
                catalog,
                job,
                "phase-work-summary",
                fallback.clone(),
            )?
        } else {
            let value = read_json(&path)?;
            let obj = as_object(&value, &path.display().to_string())?;
            require_exact_keys(
                obj,
                &["phase_id", "agent_id", "start_ms", "end_ms", "summary"],
                &path.display().to_string(),
            )?;
            let recorded_phase = as_string(
                obj.get("phase_id").unwrap(),
                &format!("{}.phase_id", path.display()),
            )?;
            let recorded_agent = as_string(
                obj.get("agent_id").unwrap(),
                &format!("{}.agent_id", path.display()),
            )?;
            let recorded_start = as_int(
                obj.get("start_ms").unwrap(),
                &format!("{}.start_ms", path.display()),
            )?;
            let recorded_end = as_int(
                obj.get("end_ms").unwrap(),
                &format!("{}.end_ms", path.display()),
            )?;
            if recorded_phase != phase.phase_id || recorded_agent != phase.agent_id {
                return Err(format!(
                    "{}: phase projection identity mismatch",
                    path.display()
                ));
            }
            let projected = summary_from_json(
                obj.get("summary").unwrap_or(&Value::Null),
                &path.display().to_string(),
            )?;
            if recorded_start != phase.start_ms || recorded_end != phase.end_ms {
                fallback.clone()
            } else {
                projected
            }
        };
        if !summary_matches_job(&candidate, job) {
            candidate = select_catalog_summary_for_job(
                &root,
                catalog,
                job,
                "phase-work-summary",
                fallback,
            )?;
        }
        result.insert(phase.summary_key.clone(), candidate);
    }
    Ok(result)
}

fn fallback_generated_at(at_ms: i64) -> String {
    use chrono::TimeZone as _;
    let dt = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00")
        .to_string()
        .replace("+00:00", "Z")
}

fn fallback_agent_name(agent: &Agent) -> AgentNameResult {
    let leaf = agent
        .agent_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let short_name = if agent.parent_thread_id.is_none() {
        "Coordinator".to_string()
    } else {
        let readable: String = {
            let mut out = String::new();
            let mut last_sep = false;
            for ch in leaf.chars() {
                if ch == '-' || ch == '_' {
                    if !last_sep {
                        out.push(' ');
                    }
                    last_sep = true;
                } else {
                    out.push(ch);
                    last_sep = false;
                }
            }
            out.trim().to_string()
        };
        let readable = if readable.is_empty() {
            "Unnamed agent".to_string()
        } else {
            readable
        };
        readable
            .chars()
            .take(48)
            .collect::<String>()
            .trim_end()
            .to_string()
    };
    let identity = crate::orc::sha256_hex(
        format!(
            "{PRESENTATION_FALLBACK_VERSION}\0{}\0{}",
            agent.thread_id, agent.agent_path
        )
        .as_bytes(),
    );
    AgentNameResult {
        thread_id: agent.thread_id.clone(),
        short_name,
        rationale: "Mechanical name from the official agent path; no cached hindsight name is available.".into(),
        lifetime_summary: Some("Summary unavailable: no cached agent-lifetime summary exists. Open a work phase to inspect normalized transcript and statistics.".into()),
        model: "none".into(),
        prompt_version: PRESENTATION_FALLBACK_VERSION.into(),
        input_hash: identity,
        generated_at: fallback_generated_at(agent.ended_at_ms.unwrap_or(agent.started_at_ms)),
        artifact_provenance: None,
        summary_available: false,
    }
}

fn nonempty_string(value: Option<&Value>, where_: &str) -> Result<String, String> {
    let raw = as_string(value.unwrap_or(&Value::Null), where_)?
        .trim()
        .to_string();
    if raw.is_empty() {
        return Err(format!("{where_}: must not be empty"));
    }
    Ok(raw)
}

fn agent_name_provenance_matches_job(
    provenance: &SummaryArtifactProvenance,
    job: &crate::naming::AgentNameJob,
) -> bool {
    if provenance.logical_key != job.key
        || provenance.team_slug != job.team_slug
        || provenance.start_ms != job.start_ms
        || provenance.end_ms != job.end_ms
        || provenance.dependency_keys != job.dependency_keys
        || provenance.context_coverage != job.context_coverage
    {
        return false;
    }
    crate::naming::input_hash_for_provenance(job, provenance)
        .ok()
        .as_deref()
        == Some(provenance.input_hash.as_str())
}

fn agent_name_matches_job(result: &AgentNameResult, job: &crate::naming::AgentNameJob) -> bool {
    match &result.artifact_provenance {
        Some(provenance) => {
            provenance.input_hash == result.input_hash
                && agent_name_provenance_matches_job(provenance, job)
        }
        None => false,
    }
}

fn agent_name_from_catalog_reference(
    summary_root: &Path,
    reference: &SummaryArtifactReference,
    agent: &Agent,
) -> Result<AgentNameResult, String> {
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
    } else {
        if !provenance.legacy_storage {
            return Err(format!(
                "{}: cataloged artifact lacks its common envelope",
                path.display()
            ));
        }
        require_exact_keys(
            root,
            &["cache_version", "backend", "usage_receipt_id", "result"],
            &path.display().to_string(),
        )?;
        if as_string(
            root.get("backend").unwrap(),
            &format!("{}.backend", path.display()),
        )? != provenance.backend
        {
            return Err(format!(
                "{}: legacy cache backend differs from catalog",
                path.display()
            ));
        }
    }
    let where_ = format!("{}.result", path.display());
    let raw = as_object(root.get("result").unwrap_or(&Value::Null), &where_)?;
    require_exact_keys(
        raw,
        &[
            "thread_id",
            "short_name",
            "rationale",
            "lifetime_summary",
            "model",
            "prompt_version",
            "input_hash",
            "generated_at",
        ],
        &where_,
    )?;
    let prompt_version = nonempty_string(
        raw.get("prompt_version"),
        &format!("{where_}.prompt_version"),
    )?;
    let contract = summarizer_change_for_prompt(&AGENT_LIFETIME_SUMMARIZER, &prompt_version)
        .map_err(|error| error.to_string())?;
    if contract.output_schema_version < 2 {
        return Err(format!(
            "{}: agent-lifetime artifact lacks a lifetime summary",
            path.display()
        ));
    }
    let result = AgentNameResult {
        thread_id: nonempty_string(raw.get("thread_id"), &format!("{where_}.thread_id"))?,
        short_name: nonempty_string(raw.get("short_name"), &format!("{where_}.short_name"))?,
        rationale: nonempty_string(raw.get("rationale"), &format!("{where_}.rationale"))?,
        lifetime_summary: Some(nonempty_string(
            Some(&Value::String(clean_summary_prose(&nonempty_string(
                raw.get("lifetime_summary"),
                &format!("{where_}.lifetime_summary"),
            )?))),
            &format!("{where_}.lifetime_summary"),
        )?),
        model: nonempty_string(raw.get("model"), &format!("{where_}.model"))?,
        prompt_version: prompt_version.clone(),
        input_hash: nonempty_string(raw.get("input_hash"), &format!("{where_}.input_hash"))?,
        generated_at: nonempty_string(raw.get("generated_at"), &format!("{where_}.generated_at"))?,
        artifact_provenance: Some(provenance.clone()),
        summary_available: true,
    };
    if result.thread_id != agent.thread_id
        || result.input_hash != provenance.input_hash
        || result.model != provenance.model
        || result.prompt_version != provenance.prompt_version
    {
        return Err(format!(
            "{}: cached agent name differs from artifact provenance",
            path.display()
        ));
    }
    Ok(result)
}

fn select_catalog_agent_name_for_job(
    summary_root: &Path,
    catalog: &SummaryArtifactCatalog,
    agent: &Agent,
    job: &crate::naming::AgentNameJob,
) -> Result<AgentNameResult, String> {
    let compatible = SummaryArtifactCatalog {
        team_slug: catalog.team_slug.clone(),
        records: catalog
            .records
            .iter()
            .filter(|reference| {
                reference.provenance.summarizer_id == AGENT_LIFETIME_SUMMARIZER.summarizer_id
                    && agent_name_provenance_matches_job(&reference.provenance, job)
            })
            .cloned()
            .collect(),
    };
    match select_summary_artifact(
        &compatible,
        &job.key,
        AGENT_LIFETIME_SUMMARIZER.summarizer_id,
        2,
        None,
    )? {
        None => Ok(fallback_agent_name(agent)),
        Some(reference) => agent_name_from_catalog_reference(summary_root, &reference, agent),
    }
}

fn load_agent_names(
    archive: &Path,
    team: &TeamData,
    selected_ids: &BTreeSet<String>,
    jobs: &[crate::naming::AgentNameJob],
    catalog: &SummaryArtifactCatalog,
) -> Result<HashMap<String, AgentNameResult>, String> {
    let root = summary_root(archive, &team.team_slug)?;
    let mut jobs_by_thread: HashMap<&str, &crate::naming::AgentNameJob> = HashMap::new();
    for job in jobs {
        if jobs_by_thread.insert(job.thread_id.as_str(), job).is_some() {
            return Err("duplicate agent-name job thread".into());
        }
    }
    let mut results = HashMap::new();
    for agent in &team.agents {
        if !selected_ids.contains(&agent.thread_id) {
            continue;
        }
        let job = jobs_by_thread
            .get(agent.thread_id.as_str())
            .ok_or_else(|| {
                format!(
                    "missing agent-name job for {}",
                    py_repr_str(&agent.thread_id)
                )
            })?;
        let path = root
            .join("agents")
            .join(format!("{}.json", agent.thread_id));
        if summary_projection_missing(&path)? {
            results.insert(
                agent.thread_id.clone(),
                select_catalog_agent_name_for_job(&root, catalog, agent, job)?,
            );
            continue;
        }
        let value = read_json(&path)?;
        let root_obj = as_object(&value, &path.display().to_string())?;
        let schema_version = as_int(
            root_obj.get("schema_version").unwrap_or(&Value::Null),
            &format!("{}.schema_version", path.display()),
        )?;
        if !(1..=3).contains(&schema_version) {
            return Err(format!(
                "unsupported agent-name schema at {}",
                path.display()
            ));
        }
        let identity = as_object(
            root_obj.get("agent").unwrap_or(&Value::Null),
            &format!("{}.agent", path.display()),
        )?;
        let recorded_thread = nonempty_string(
            identity.get("thread_id"),
            &format!("{}.agent.thread_id", path.display()),
        )?;
        let recorded_path = nonempty_string(
            identity.get("official_path"),
            &format!("{}.agent.official_path", path.display()),
        )?;
        let recorded_depth = as_int(
            identity.get("depth").unwrap_or(&Value::Null),
            &format!("{}.agent.depth", path.display()),
        )?;
        if recorded_thread != agent.thread_id
            || recorded_path != agent.agent_path
            || recorded_depth != agent.depth
        {
            return Err(format!(
                "stale hindsight name metadata for {}; run summarize",
                agent.agent_path
            ));
        }
        let raw_name = as_object(
            root_obj.get("name").unwrap_or(&Value::Null),
            &format!("{}.name", path.display()),
        )?;
        let prompt_version = nonempty_string(
            raw_name.get("prompt_version"),
            &format!("{}.name.prompt_version", path.display()),
        )?;
        let prompt_contract =
            summarizer_change_for_prompt(&AGENT_LIFETIME_SUMMARIZER, &prompt_version)
                .map_err(|_| format!("{}: unregistered agent-lifetime version", path.display()))?;
        let lifetime_summary: Option<String> = if schema_version == 1 {
            if prompt_contract.output_schema_version != 1 {
                return Err(format!(
                    "{}: agent-name wrapper and prompt schemas disagree",
                    path.display()
                ));
            }
            None
        } else {
            if prompt_contract.output_schema_version < 2 {
                return Err(format!(
                    "{}: agent-name wrapper and prompt schemas disagree",
                    path.display()
                ));
            }
            Some(nonempty_string(
                Some(&Value::String(clean_summary_prose(&nonempty_string(
                    raw_name.get("lifetime_summary"),
                    &format!("{}.name.lifetime_summary", path.display()),
                )?))),
                &format!("{}.name.lifetime_summary", path.display()),
            )?)
        };
        let result = AgentNameResult {
            thread_id: nonempty_string(
                raw_name.get("thread_id"),
                &format!("{}.name.thread_id", path.display()),
            )?,
            short_name: nonempty_string(
                raw_name.get("short_name"),
                &format!("{}.name.short_name", path.display()),
            )?,
            rationale: nonempty_string(
                raw_name.get("rationale"),
                &format!("{}.name.rationale", path.display()),
            )?,
            lifetime_summary,
            model: nonempty_string(
                raw_name.get("model"),
                &format!("{}.name.model", path.display()),
            )?,
            prompt_version: prompt_version.clone(),
            input_hash: nonempty_string(
                raw_name.get("input_hash"),
                &format!("{}.name.input_hash", path.display()),
            )?,
            generated_at: nonempty_string(
                raw_name.get("generated_at"),
                &format!("{}.name.generated_at", path.display()),
            )?,
            artifact_provenance: if raw_name.contains_key("artifact_provenance") {
                Some(SummaryArtifactProvenance::from_json_obj(
                    raw_name.get("artifact_provenance"),
                    &format!("{}.name.artifact_provenance", path.display()),
                )?)
            } else {
                None
            },
            summary_available: true,
        };
        if result.thread_id != agent.thread_id {
            return Err(format!(
                "agent-name result thread mismatch at {}",
                path.display()
            ));
        }
        if let Some(provenance) = &result.artifact_provenance {
            if provenance.summarizer_id != AGENT_LIFETIME_SUMMARIZER.summarizer_id
                || provenance.logical_key != format!("agent-name:{}", agent.thread_id)
                || provenance.team_slug != team.team_slug
                || provenance.input_hash != result.input_hash
                || provenance.model != result.model
                || provenance.prompt_version != result.prompt_version
            {
                return Err(format!(
                    "{}: agent-name provenance differs from result",
                    path.display()
                ));
            }
        }
        if result.lifetime_summary.is_none() {
            results.insert(agent.thread_id.clone(), result);
        } else if !agent_name_matches_job(&result, job) {
            results.insert(
                agent.thread_id.clone(),
                select_catalog_agent_name_for_job(&root, catalog, agent, job)?,
            );
        } else {
            results.insert(agent.thread_id.clone(), result);
        }
    }
    Ok(results)
}

fn validate_legacy_project_overview(
    root: &serde_json::Map<String, Value>,
    path: &Path,
    version: i64,
) -> Result<(), String> {
    let mut root_keys: Vec<&str> = vec!["schema_version", "style", "source", "summary"];
    if version == 2 {
        root_keys.push("knowledge_epoch");
    }
    require_exact_keys(root, &root_keys, &path.display().to_string())?;
    if as_string(
        root.get("style").unwrap(),
        &format!("{}.style", path.display()),
    )? != crate::summary_registry::PROJECT_OVERVIEW_STYLE
    {
        return Err(format!(
            "{}: invalid project-overview style",
            path.display()
        ));
    }
    if version == 2 {
        // Epoch shape is validated by summarize's writer; here we only
        // require the object to be present and object-shaped.
        let epoch = root.get("knowledge_epoch").unwrap_or(&Value::Null);
        as_object(epoch, &format!("{}.knowledge_epoch", path.display()))?;
    }
    let source = as_object(
        root.get("source").unwrap_or(&Value::Null),
        &format!("{}.source", path.display()),
    )?;
    let mut source_keys: Vec<&str> = vec!["event_ids", "start_ms", "end_ms", "context_sha256"];
    if version == 2 {
        source_keys.push("transcript");
    }
    require_exact_keys(source, &source_keys, &format!("{}.source", path.display()))?;
    for (index, item) in as_array(
        source.get("event_ids").unwrap_or(&Value::Null),
        &format!("{}.source.event_ids", path.display()),
    )?
    .iter()
    .enumerate()
    {
        as_string(
            item,
            &format!("{}.source.event_ids[{index}]", path.display()),
        )?;
    }
    as_int(
        source.get("start_ms").unwrap(),
        &format!("{}.source.start_ms", path.display()),
    )?;
    as_int(
        source.get("end_ms").unwrap(),
        &format!("{}.source.end_ms", path.display()),
    )?;
    Ok(())
}

fn load_project_overview(archive: &Path, team: &TeamData) -> Result<SummaryResult, String> {
    let root_path = summary_root(archive, &team.team_slug)?;
    let path = root_path.join("project_overview.json");
    if summary_projection_missing(&path)? {
        let fallback_source = root_overview_input(team, None);
        return Ok(unavailable_summary(
            &format!("project-overview:{}", team.team_slug),
            fallback_source.end_ms,
            "project overview",
        ));
    }
    let value = read_json(&path)?;
    let root = as_object(&value, &path.display().to_string())?;
    let version = as_int(
        root.get("schema_version").unwrap_or(&Value::Null),
        &format!("{}.schema_version", path.display()),
    )?;
    if matches!(version, 1 | 2) {
        validate_legacy_project_overview(root, &path, version)?;
        let fallback_source = root_overview_input(team, None);
        return Ok(unavailable_summary(
            &format!("project-overview:{}", team.team_slug),
            fallback_source.end_ms,
            "project overview stored with a legacy schema",
        ));
    }
    if version != PROJECT_OVERVIEW_SCHEMA_VERSION {
        return Err(format!(
            "{}: unsupported project-overview schema; run summarize",
            path.display()
        ));
    }
    let summary = summary_from_json(
        root.get("summary").unwrap_or(&Value::Null),
        &format!("{}.summary", path.display()),
    )?;
    validate_project_overview(&summary, &format!("{}.summary", path.display()))?;
    match frozen_project_overview_input(archive, team) {
        Ok(FrozenOverview::Current(_)) => Ok(summary),
        Ok(FrozenOverview::SourceChanged(_)) => {
            let fallback_source = root_overview_input(team, None);
            Ok(unavailable_summary(
                &format!("project-overview:{}", team.team_slug),
                fallback_source.end_ms,
                "stale project overview",
            ))
        }
        Ok(FrozenOverview::None) => {
            let fallback_source = root_overview_input(team, None);
            Ok(unavailable_summary(
                &format!("project-overview:{}", team.team_slug),
                fallback_source.end_ms,
                "project overview for this time slice",
            ))
        }
        Err(error) => {
            if error.contains("frozen overview evidence was mutated or truncated") {
                let fallback_source = root_overview_input(team, None);
                Ok(unavailable_summary(
                    &format!("project-overview:{}", team.team_slug),
                    fallback_source.end_ms,
                    "stale project overview",
                ))
            } else {
                Err(error)
            }
        }
    }
}

fn load_rollup_projection(
    path: &Path,
    period: &Period,
) -> Result<(SummaryResult, SummaryResult, bool), String> {
    let value = read_json(path)?;
    let obj = as_object(&value, &path.display().to_string())?;
    require_exact_keys(
        obj,
        &[
            "schema_version",
            "kind",
            "key",
            "start_ms",
            "end_ms",
            "partial",
            "technical_summary",
            "plain_language_summary",
        ],
        &path.display().to_string(),
    )?;
    if as_int(
        obj.get("schema_version").unwrap(),
        &format!("{}.schema_version", path.display()),
    )? != 2
    {
        return Err(format!(
            "{}: rollup lacks a plain-language summary; run summarize",
            path.display()
        ));
    }
    if as_string(
        obj.get("kind").unwrap(),
        &format!("{}.kind", path.display()),
    )? != period.kind
        || as_string(obj.get("key").unwrap(), &format!("{}.key", path.display()))? != period.key
        || as_int(
            obj.get("start_ms").unwrap(),
            &format!("{}.start_ms", path.display()),
        )? != period.start_ms
        || as_int(
            obj.get("end_ms").unwrap(),
            &format!("{}.end_ms", path.display()),
        )? != period.end_ms
    {
        return Err(format!(
            "{}: rollup projection identity mismatch",
            path.display()
        ));
    }
    let recorded_partial = match obj.get("partial") {
        Some(Value::Bool(value)) => *value,
        _ => {
            return Err(format!("{}.partial: expected a boolean", path.display()));
        }
    };
    Ok((
        summary_from_json(
            obj.get("technical_summary").unwrap_or(&Value::Null),
            &format!("{}.technical_summary", path.display()),
        )?,
        summary_from_json(
            obj.get("plain_language_summary").unwrap_or(&Value::Null),
            &format!("{}.plain_language_summary", path.display()),
        )?,
        recorded_partial,
    ))
}

#[allow(clippy::too_many_arguments)]
fn validate_rollup_inputs(
    team: &TeamData,
    periods: &[Period],
    phases: &[PhaseWindow],
    phase_results: &HashMap<String, SummaryResult>,
    project_overview: &SummaryResult,
    terms: &[GlossaryTerm],
    technical_candidates: &HashMap<String, SummaryResult>,
    plain_candidates: &HashMap<String, SummaryResult>,
    suppressed_keys: &BTreeSet<String>,
    summary_root: &Path,
    catalog: &SummaryArtifactCatalog,
) -> Result<ValidatedRollups, String> {
    let selected_kinds: Vec<&str> = ROLLUP_KINDS
        .iter()
        .copied()
        .filter(|kind| periods.iter().any(|period| period.kind == *kind))
        .collect();
    let mut validated: HashMap<String, SummaryResult> = HashMap::new();
    let mut validated_plain: HashMap<String, SummaryResult> = HashMap::new();
    let mut previous_periods: Vec<Period> = Vec::new();
    let mut previous_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut previous_plain_results: HashMap<String, SummaryResult> = HashMap::new();
    for kind in &selected_kinds {
        let current: Vec<Period> = periods
            .iter()
            .filter(|period| period.kind == *kind)
            .cloned()
            .collect();
        let first_start_ms = current[0].start_ms;
        let mut completed = prior_catalog_rollups(
            summary_root,
            catalog,
            kind,
            TECHNICAL_ROLLUP_STYLE,
            first_start_ms,
        )?;
        let mut completed_plain = prior_catalog_rollups(
            summary_root,
            catalog,
            kind,
            PLAIN_LANGUAGE_ROLLUP_STYLE,
            first_start_ms,
        )?;
        for period in &current {
            let key = period_key(period);
            let technical_job = rollup_job_for_period(
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
            let mut technical = technical_candidates[&key].clone();
            if !summary_matches_job(&technical, &technical_job)
                && !carried_forward_matches(&technical_job, &technical, &rollup_index(summary_root))
            {
                let technical_fallback = unavailable_summary(
                    &technical_job.key,
                    period.end_ms,
                    &format!("stale {} rollup", period.kind),
                );
                technical = if suppressed_keys.contains(&key) {
                    technical_fallback
                } else {
                    select_catalog_summary_for_job(
                        summary_root,
                        catalog,
                        &technical_job,
                        "technical-rollup",
                        technical_fallback,
                    )?
                };
            }
            validated.insert(key.clone(), technical.clone());
            completed.push((period.clone(), technical));

            let same_technical: HashMap<String, SummaryResult> =
                [(key.clone(), validated[&key].clone())]
                    .into_iter()
                    .collect();
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
                Some(&same_technical),
            )?;
            let mut plain = plain_candidates[&key].clone();
            if !summary_matches_job(&plain, &plain_job)
                && !carried_forward_matches(&plain_job, &plain, &rollup_index(summary_root))
            {
                let plain_fallback = unavailable_summary(
                    &plain_job.key,
                    period.end_ms,
                    &format!("stale plain-language {} rollup", period.kind),
                );
                plain = if suppressed_keys.contains(&key) {
                    plain_fallback
                } else {
                    select_catalog_summary_for_job(
                        summary_root,
                        catalog,
                        &plain_job,
                        "plain-language-rollup",
                        plain_fallback,
                    )?
                };
            }
            validated_plain.insert(key.clone(), plain.clone());
            completed_plain.push((period.clone(), plain));
        }
        previous_periods = current.clone();
        previous_results = current
            .iter()
            .map(|period| (period_key(period), validated[&period_key(period)].clone()))
            .collect();
        previous_plain_results = current
            .iter()
            .map(|period| {
                (
                    period_key(period),
                    validated_plain[&period_key(period)].clone(),
                )
            })
            .collect();
    }
    Ok((validated, validated_plain))
}

fn load_glossary(archive: &Path, team_slug: &str) -> Result<Vec<GlossaryTerm>, String> {
    let path = summary_root(archive, team_slug)?.join("semantic_glossary.json");
    if summary_projection_missing(&path)? {
        return Ok(Vec::new());
    }
    Err(format!(
        "{}: semantic glossary projection is not supported by this version",
        path.display()
    ))
}

fn artifact_catalog_path(archive: &Path, team_slug: &str) -> Result<PathBuf, String> {
    crate::build_store::team_build_root(archive, team_slug)
        .map(|root| root.join("raw").join("artifacts.json"))
}

fn load_artifact_catalog(
    archive: &Path,
    team_slug: &str,
    team: &TeamData,
) -> Result<ArtifactCatalog, String> {
    let path = artifact_catalog_path(archive, team_slug)?;
    if !path.is_file() {
        return Ok(ArtifactCatalog {
            source_digest: source_digest(team),
            artifacts: Vec::new(),
            projects: Vec::new(),
        });
    }
    let catalog = ArtifactCatalog::from_json(&read_json(&path)?)?;
    let expected = source_digest(team);
    if catalog.source_digest != expected {
        return Err(format!(
            "stale artifact catalog for {}; rerun ingest before building",
            py_repr_str(team_slug)
        ));
    }
    Ok(catalog)
}

fn load_site_identity(archive: &Path, team: &TeamData) -> Result<SiteIdentity, String> {
    let path = crate::build_store::team_build_root(archive, &team.team_slug)?
        .join("raw")
        .join("site-identity.json");
    if !path.is_file() {
        return Ok(SiteIdentity {
            team_slug: team.team_slug.clone(),
            projects: Vec::new(),
            hosts: Vec::new(),
            display_timezone: team.display_timezone.clone(),
            display_timezone_source: "legacy_team_data".into(),
        });
    }
    let identity = site_identity_from_json(&read_json(&path)?, &path.display().to_string())?;
    if identity.team_slug != team.team_slug {
        return Err(format!(
            "site identity team {} does not match {}",
            py_repr_str(&identity.team_slug),
            py_repr_str(&team.team_slug)
        ));
    }
    if identity.display_timezone != team.display_timezone {
        return Err(
            "site identity display timezone differs from normalized team data; rerun ingest".into(),
        );
    }
    Ok(identity)
}

fn load_pull_cache_records(path: &Path) -> Result<Vec<PullRequestMetadata>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let value = read_json(path)?;
    let root = as_object(&value, "GitHub metadata cache")?;
    require_exact_keys(
        root,
        &["schema_version", "records"],
        "GitHub metadata cache",
    )?;
    let version = as_int(
        root.get("schema_version").unwrap_or(&Value::Null),
        "GitHub metadata cache.schema_version",
    )?;
    if version != 1 {
        return Err(format!(
            "unsupported GitHub metadata cache schema {version}"
        ));
    }
    let records_obj = as_object(
        root.get("records").unwrap_or(&Value::Null),
        "GitHub metadata cache.records",
    )?;
    let mut keys: Vec<&String> = records_obj.keys().collect();
    keys.sort();
    let mut records = Vec::new();
    for cache_key in keys {
        let where_ = format!("GitHub metadata cache.records[{cache_key:?}]");
        let item = as_object(records_obj.get(cache_key).unwrap(), &where_)?;
        require_exact_keys(
            item,
            &[
                "repository",
                "number",
                "title",
                "state",
                "draft",
                "merged_at",
                "body_excerpt",
                "base_ref",
                "head_label",
                "author",
                "updated_at",
                "etag",
                "fetched_at",
            ],
            &where_,
        )?;
        let repository = as_string(
            item.get("repository").unwrap(),
            &format!("{where_}.repository"),
        )?
        .to_string();
        let number = as_int(item.get("number").unwrap(), &format!("{where_}.number"))?;
        let expected_key = format!("{repository}#{number}");
        if *cache_key != expected_key {
            return Err(format!(
                "GitHub metadata cache key does not match its record: {cache_key:?} != {expected_key:?}"
            ));
        }
        let opt = |key: &str| -> Result<Option<String>, String> {
            match item.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(other) => Ok(Some(
                    as_string(other, &format!("{where_}.{key}"))?.to_string(),
                )),
            }
        };
        let draft = match item.get("draft") {
            Some(Value::Bool(value)) => *value,
            _ => return Err(format!("{where_}.draft: expected a boolean")),
        };
        records.push(PullRequestMetadata {
            repository,
            number,
            title: as_string(item.get("title").unwrap(), &format!("{where_}.title"))?.to_string(),
            state: as_string(item.get("state").unwrap(), &format!("{where_}.state"))?.to_string(),
            draft,
            merged_at: opt("merged_at")?,
            body_excerpt: as_string(
                item.get("body_excerpt").unwrap(),
                &format!("{where_}.body_excerpt"),
            )?
            .to_string(),
            base_ref: as_string(item.get("base_ref").unwrap(), &format!("{where_}.base_ref"))?
                .to_string(),
            head_label: as_string(
                item.get("head_label").unwrap(),
                &format!("{where_}.head_label"),
            )?
            .to_string(),
            author: opt("author")?,
            updated_at: as_string(
                item.get("updated_at").unwrap(),
                &format!("{where_}.updated_at"),
            )?
            .to_string(),
        });
    }
    Ok(records)
}

/// Regenerate presentation exclusively from cached structured data.
#[allow(clippy::too_many_arguments)]
pub fn build_archive_locked(
    archive: &Path,
    team_slug: &str,
    phase_minutes: i64,
    display_window: Option<&DateWindow>,
    rollup_kinds: &[String],
    output: Option<&Path>,
    precompress: bool,
    published: bool,
) -> Result<BuildReport, String> {
    let target = output.unwrap_or(archive);
    ensure_archive_pub(target, team_slug, output.is_some())?;
    let mut team = load_archived_team(archive, team_slug)?;
    if let Some(window) = display_window {
        team = crate::window::apply_date_window(team, Some(window));
    }
    let root = summary_root(archive, team_slug)?;
    let catalog_path = root.join("artifacts.json");
    let catalog = load_summary_catalog(&catalog_path, team_slug)?;
    let phases = build_phases(&team, phase_minutes, 16_000, 30_000)?;
    let jobs = phase_jobs(&team, &phases, &[], 16_000);
    let phase_results = load_phase_summaries(archive, team_slug, &phases, &jobs, &catalog)?;
    let name_jobs = agent_name_jobs(&team, &phases, &phase_results, 16_000);
    let selected_ids = crate::summary_phases::phase_agent_ids(&team, &phases);
    let agent_names = load_agent_names(archive, &team, &selected_ids, &name_jobs, &catalog)?;
    let project_overview = load_project_overview(archive, &team)?;
    let (start_ms, end_ms) = period_range(&team, &phases)?;
    let kinds: Vec<String> = if rollup_kinds.is_empty() {
        DEFAULT_ROLLUP_KINDS
            .iter()
            .map(|item| item.to_string())
            .collect()
    } else {
        rollup_kinds.to_vec()
    };
    let periods = periods_for_range(
        start_ms,
        end_ms,
        &team.display_timezone,
        &team.team_slug,
        &kinds,
    )?;
    let mut rollup_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut plain_rollup_results: HashMap<String, SummaryResult> = HashMap::new();
    let mut rollup_stats: HashMap<String, crate::summary_phases::PhaseStats> = HashMap::new();
    let mut suppressed: BTreeSet<String> = BTreeSet::new();
    for period in &periods {
        let path = root
            .join("rollups")
            .join(&period.kind)
            .join(format!("{}.json", period.key));
        let key = period_key(period);
        let in_period: Vec<PhaseWindow> = phases_in(period, &phases).into_iter().cloned().collect();
        rollup_stats.insert(key.clone(), aggregate_stats(&in_period));
        let projection_missing = summary_projection_missing(&path)?;
        if display_window.is_some() && period.partial {
            suppressed.insert(key.clone());
            if !projection_missing {
                load_rollup_projection(&path, period)?;
            }
            rollup_results.insert(
                key.clone(),
                unavailable_summary(
                    &format!("rollup:{}:{}", period.kind, period.key),
                    period.end_ms,
                    &format!("partial {} rollup", period.kind),
                ),
            );
            plain_rollup_results.insert(
                key.clone(),
                unavailable_summary(
                    &format!("rollup-plain:{}:{}", period.kind, period.key),
                    period.end_ms,
                    &format!("partial plain-language {} rollup", period.kind),
                ),
            );
            continue;
        }
        if projection_missing {
            rollup_results.insert(
                key.clone(),
                select_catalog_summary(
                    &root,
                    &catalog,
                    &format!("rollup:{}:{}", period.kind, period.key),
                    "technical-rollup",
                    unavailable_summary(
                        &format!("rollup:{}:{}", period.kind, period.key),
                        period.end_ms,
                        &format!("{} rollup", period.kind),
                    ),
                )?,
            );
            plain_rollup_results.insert(
                key.clone(),
                select_catalog_summary(
                    &root,
                    &catalog,
                    &format!("rollup-plain:{}:{}", period.kind, period.key),
                    "plain-language-rollup",
                    unavailable_summary(
                        &format!("rollup-plain:{}:{}", period.kind, period.key),
                        period.end_ms,
                        &format!("plain-language {} rollup", period.kind),
                    ),
                )?,
            );
            continue;
        }
        let (projected_technical, projected_plain, recorded_partial) =
            load_rollup_projection(&path, period)?;
        if recorded_partial != period.partial {
            suppressed.insert(key.clone());
            rollup_results.insert(
                key.clone(),
                unavailable_summary(
                    &format!("rollup:{}:{}", period.kind, period.key),
                    period.end_ms,
                    &format!("stale {} rollup", period.kind),
                ),
            );
            plain_rollup_results.insert(
                key.clone(),
                unavailable_summary(
                    &format!("rollup-plain:{}:{}", period.kind, period.key),
                    period.end_ms,
                    &format!("stale plain-language {} rollup", period.kind),
                ),
            );
            continue;
        }
        rollup_results.insert(
            key.clone(),
            select_catalog_summary(
                &root,
                &catalog,
                &format!("rollup:{}:{}", period.kind, period.key),
                "technical-rollup",
                projected_technical,
            )?,
        );
        plain_rollup_results.insert(
            key.clone(),
            select_catalog_summary(
                &root,
                &catalog,
                &format!("rollup-plain:{}:{}", period.kind, period.key),
                "plain-language-rollup",
                projected_plain,
            )?,
        );
    }
    let terms = load_glossary(archive, team_slug)?;
    let (rollup_results, plain_rollup_results) = validate_rollup_inputs(
        &team,
        &periods,
        &phases,
        &phase_results,
        &project_overview,
        &terms,
        &rollup_results,
        &plain_rollup_results,
        &suppressed,
        &root,
        &catalog,
    )?;
    let artifact_catalog = load_artifact_catalog(archive, team_slug, &team)?;
    let pull_path = root.join("github").join("pulls.json");
    let pull_records = load_pull_cache_records(&pull_path)?;
    let mut pull_metadata: HashMap<(String, i64), PullRequestMetadata> = HashMap::new();
    for record in pull_records {
        pull_metadata.insert(
            (record.repository.clone(), record.number),
            PullRequestMetadata {
                repository: record.repository,
                number: record.number,
                title: record.title,
                state: record.state,
                draft: record.draft,
                merged_at: record.merged_at,
                body_excerpt: record.body_excerpt,
                base_ref: record.base_ref,
                head_label: record.head_label,
                author: record.author,
                updated_at: record.updated_at,
            },
        );
    }
    let site_identity = load_site_identity(archive, &team)?;
    render_archive(
        target,
        RenderInputs {
            team,
            phases,
            phase_summaries: phase_results,
            periods,
            rollup_summaries: rollup_results,
            plain_rollup_summaries: plain_rollup_results,
            rollup_stats,
            glossary_terms: terms,
            project_overview,
            agent_names,
            pull_metadata,
            artifact_catalog,
            site_identity,
            published,
            precompress,
            export_window: display_window.map(|window| window.to_json()),
            namespace_agents: false,
        },
    )
}

/// Build one site under the archive writer locks.
#[allow(clippy::too_many_arguments)]
pub fn build_archive(
    archive: &Path,
    team_slug: &str,
    phase_minutes: i64,
    display_window: Option<&DateWindow>,
    rollup_kinds: &[String],
    output: Option<&Path>,
    precompress: bool,
    published: bool,
) -> Result<BuildReport, String> {
    let target = output.unwrap_or(archive);
    let _lock = archive_writer_lock(archive)?;
    let _target_lock = if target != archive {
        Some(archive_writer_lock(target)?)
    } else {
        None
    };
    build_archive_locked(
        archive,
        team_slug,
        phase_minutes,
        display_window,
        rollup_kinds,
        output,
        precompress,
        published,
    )
}
