//! The `ingest-project` manifest: `py/wrkviz/project_config.py`'s
//! strict schema-v1 project config, per-team dispatch to the
//! provider importers, and the global transcript refresh, at
//! parity. One failing team never ends the run; the outcome is
//! returned, and callers consult `ProjectIngestReport::failed`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::identity::{parse_identity_overrides, IdentityOverrides};
use crate::ingest::IngestReport;
use crate::json::{as_array, as_int, as_object, as_string, py_repr_str};
use crate::orc::ContinuationSpec;
use crate::transcript_export::{PromptAuthorshipRule, TranscriptExportReport};
use crate::window::{parse_date_window, DateWindow};

/// The project-ingest manifest schema this edition reads.
pub const PROJECT_INGEST_SCHEMA_VERSION: i64 = 1;

/// One Codex coordinator root and its ordered continuations.
#[derive(Clone, Debug)]
pub struct CodexProjectSource {
    /// Sessions root directory.
    pub sessions_root: PathBuf,
    /// Root thread id.
    pub root_session: String,
    /// Continuation thread ids, in order.
    pub continuation_sessions: Vec<String>,
}

/// One canonical Claude coordinator JSONL file.
#[derive(Clone, Debug)]
pub struct ClaudeProjectSource {
    /// The session file.
    pub session_file: PathBuf,
}

/// One Muse session directory.
#[derive(Clone, Debug)]
pub struct MuseProjectSource {
    /// The session directory.
    pub session_dir: PathBuf,
}

/// One Claude Code `history.jsonl` plus its selection pattern.
#[derive(Clone, Debug)]
pub struct ClaudeHistoryProjectSource {
    /// The history file.
    pub history_file: PathBuf,
    /// Working-directory selection pattern.
    pub project_pattern: String,
    /// `config` or `none`.
    pub covered_sessions: String,
}

/// One Orc coordinator root and its ordered continuations.
#[derive(Clone, Debug)]
pub struct OrcProjectSource {
    /// Source root directory.
    pub source_root: PathBuf,
    /// Root session id.
    pub root_session: String,
    /// Continuation specs, in order.
    pub continuation_sessions: Vec<ContinuationSpec>,
}

/// The provider-specific half of one registered team.
#[derive(Clone, Debug)]
pub enum ProjectSource {
    /// Codex.
    Codex(CodexProjectSource),
    /// Claude.
    Claude(ClaudeProjectSource),
    /// Muse.
    Muse(MuseProjectSource),
    /// Claude history.
    ClaudeHistory(ClaudeHistoryProjectSource),
    /// Orc.
    Orc(OrcProjectSource),
}

/// Validated provider inputs and archive identity for one team.
#[derive(Clone, Debug)]
pub struct ProjectTeamConfig {
    /// Team slug.
    pub slug: String,
    /// Provider name.
    pub provider: String,
    /// Provider source.
    pub source: ProjectSource,
    /// Display timezone.
    pub timezone: String,
    /// Date window.
    pub date_window: Option<DateWindow>,
    /// Identity overrides.
    pub identity_overrides: IdentityOverrides,
    /// Prompt authorship rules.
    pub prompt_authorship_rules: Vec<PromptAuthorshipRule>,
}

/// A validated schema-v1 project manifest.
#[derive(Clone, Debug)]
pub struct ProjectIngestConfig {
    /// The config file, resolved.
    pub config_path: PathBuf,
    /// SHA-256 of the config bytes.
    pub config_sha256: String,
    /// Archive output directory.
    pub output: PathBuf,
    /// Snapshot store override.
    pub snapshot_root: Option<PathBuf>,
    /// Registered teams, in manifest order.
    pub teams: Vec<ProjectTeamConfig>,
}

impl ProjectIngestConfig {
    /// Every team-scoped rule in manifest order.
    pub fn prompt_authorship_rules(&self) -> Vec<PromptAuthorshipRule> {
        self.teams
            .iter()
            .flat_map(|team| team.prompt_authorship_rules.iter().cloned())
            .collect()
    }

    /// Every session a `claude` team archives in full, by stem.
    pub fn claude_session_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .teams
            .iter()
            .filter_map(|team| match &team.source {
                ProjectSource::Claude(source) => source
                    .session_file
                    .file_stem()
                    .map(|held| held.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Configured teams in manifest order, validating a filter.
    pub fn select_teams(&self, requested: &[String]) -> Result<Vec<ProjectTeamConfig>, String> {
        let mut unique = requested.to_vec();
        unique.sort();
        unique.dedup();
        if unique.len() != requested.len() {
            return Err("--team selection contains duplicates".to_string());
        }
        let unknown: Vec<&String> = requested
            .iter()
            .filter(|held| !self.teams.iter().any(|team| &team.slug == *held))
            .collect();
        if !unknown.is_empty() {
            let mut names: Vec<String> = unknown.iter().map(|held| held.to_string()).collect();
            names.sort();
            return Err(format!(
                "team selection is not registered in project config: {}",
                names.join(", ")
            ));
        }
        if requested.is_empty() {
            return Ok(self.teams.clone());
        }
        Ok(self
            .teams
            .iter()
            .filter(|team| requested.contains(&team.slug))
            .cloned()
            .collect())
    }
}

/// Mechanical ingest result for one configured team.
#[derive(Clone, Debug)]
pub struct ProjectTeamIngestResult {
    /// Team slug.
    pub team_slug: String,
    /// Provider name.
    pub provider: String,
    /// The ingest report.
    pub ingest: IngestReport,
}

impl ProjectTeamIngestResult {
    /// This result as run-receipt metadata.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        out.insert("provider".into(), Value::String(self.provider.clone()));
        out.insert("ingest".into(), self.ingest.to_json());
        Value::Object(out)
    }
}

/// One configured team whose ingest raised.
#[derive(Clone, Debug)]
pub struct ProjectTeamIngestFailure {
    /// Team slug.
    pub team_slug: String,
    /// Provider name.
    pub provider: String,
    /// The failure class name.
    pub error_type: String,
    /// The failure message.
    pub error: String,
}

impl ProjectTeamIngestFailure {
    /// This failure as run-receipt metadata.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        out.insert("provider".into(), Value::String(self.provider.clone()));
        out.insert("error_type".into(), Value::String(self.error_type.clone()));
        out.insert("error".into(), Value::String(self.error.clone()));
        Value::Object(out)
    }

    /// One operator-readable line.
    pub fn summary(&self) -> String {
        format!(
            "{} ({}): {}: {}",
            self.team_slug, self.provider, self.error_type, self.error
        )
    }
}

/// Results of configured team ingests and the transcript pass.
#[derive(Clone, Debug)]
pub struct ProjectIngestReport {
    /// SHA-256 of the config bytes.
    pub config_sha256: String,
    /// Successful teams.
    pub teams: Vec<ProjectTeamIngestResult>,
    /// The transcript report, when extraction ran.
    pub transcripts: Option<TranscriptExportReport>,
    /// Failed teams.
    pub failures: Vec<ProjectTeamIngestFailure>,
    /// The transcript failure line, when extraction raised.
    pub transcript_error: Option<String>,
    /// The `team:session` pairs this run was permitted to
    /// re-baseline, sorted.
    pub accept_prefix_rewrite_sessions: Vec<String>,
}

impl ProjectIngestReport {
    /// Whether any selected team, or the projection, failed.
    pub fn failed(&self) -> bool {
        !self.failures.is_empty()
            || self.transcripts.is_none()
            || self
                .transcripts
                .as_ref()
                .map(|held| held.partial())
                .unwrap_or(false)
    }

    /// One line naming every failure, or `None` when clean.
    pub fn failure_summary(&self) -> Option<String> {
        if !self.failed() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if !self.failures.is_empty() {
            let selected = self.teams.len() + self.failures.len();
            parts.push(format!(
                "{} of {selected} teams failed: {}",
                self.failures.len(),
                self.failures
                    .iter()
                    .map(ProjectTeamIngestFailure::summary)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if let Some(error) = &self.transcript_error {
            parts.push(format!("transcript extraction failed: {error}"));
        }
        if let Some(transcripts) = &self.transcripts {
            if let Some(partiality) = transcripts.partiality_summary() {
                parts.push(format!("transcript extraction was partial: {partiality}"));
            }
        }
        Some(parts.join(" | "))
    }

    /// The run-receipt payload.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "schema_version".into(),
            Value::Number(PROJECT_INGEST_SCHEMA_VERSION.into()),
        );
        out.insert(
            "config_sha256".into(),
            Value::String(self.config_sha256.clone()),
        );
        out.insert(
            "status".into(),
            Value::String(if self.failed() { "failed" } else { "completed" }.to_string()),
        );
        out.insert(
            "teams".into(),
            Value::Array(
                self.teams
                    .iter()
                    .map(ProjectTeamIngestResult::to_json)
                    .collect(),
            ),
        );
        out.insert("teams_succeeded".into(), self.teams.len().into());
        out.insert(
            "failed_teams".into(),
            Value::Array(
                self.failures
                    .iter()
                    .map(ProjectTeamIngestFailure::to_json)
                    .collect(),
            ),
        );
        out.insert("teams_failed".into(), self.failures.len().into());
        out.insert(
            "transcript_extraction".into(),
            self.transcripts
                .as_ref()
                .map_or(Value::Null, TranscriptExportReport::to_json),
        );
        out.insert(
            "transcript_extraction_error".into(),
            self.transcript_error
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "accepted_prefix_rewrite_sessions".into(),
            Value::Array(
                self.accept_prefix_rewrite_sessions
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        out.insert("model_calls".into(), 0.into());
        out.insert("model_tokens".into(), 0.into());
        out.insert("website_build_performed".into(), Value::Bool(false));
        Value::Object(out)
    }
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

fn check_fields(
    value: &Map<String, Value>,
    where_: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<(), String> {
    let missing: Vec<String> = required
        .iter()
        .filter(|held| !value.contains_key(**held))
        .map(|held| held.to_string())
        .collect();
    let mut unknown: Vec<String> = value
        .keys()
        .filter(|held| !required.contains(&held.as_str()) && !optional.contains(&held.as_str()))
        .cloned()
        .collect();
    let mut missing_sorted = missing.clone();
    missing_sorted.sort();
    unknown.sort();
    if !missing_sorted.is_empty() || !unknown.is_empty() {
        return Err(format!(
            "{where_}: invalid fields; missing={}, unknown={}",
            py_str_list(&missing_sorted),
            py_str_list(&unknown)
        ));
    }
    Ok(())
}

fn required_string(value: Option<&Value>, where_: &str) -> Result<String, String> {
    let held = as_string(value.unwrap_or(&Value::Null), where_)?;
    let held = held.trim().to_string();
    if held.is_empty() || held.contains('\0') {
        return Err(format!("{where_}: expected a non-empty string"));
    }
    Ok(held)
}

fn optional_string(
    value: &Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<Option<String>, String> {
    match value.get(key) {
        None => Ok(None),
        Some(held) => Ok(Some(required_string(
            Some(held),
            &format!("{where_}.{key}"),
        )?)),
    }
}

fn string_array(value: &Value, where_: &str) -> Result<Vec<String>, String> {
    let items = as_array(value, where_)?;
    let mut result: Vec<String> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        result.push(required_string(Some(item), &format!("{where_}[{index}]"))?);
    }
    let mut unique = result.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != result.len() {
        return Err(format!("{where_}: duplicate values are not allowed"));
    }
    Ok(result)
}

fn orc_continuation_array(value: &Value, where_: &str) -> Result<Vec<ContinuationSpec>, String> {
    let items = as_array(value, where_)?;
    let mut result: Vec<ContinuationSpec> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let item_where = format!("{where_}[{index}]");
        let spec = ContinuationSpec::from_value(item, &item_where)?;
        if item.as_str().is_none() && spec.start_message_id.is_none() {
            return Err(format!(
                "{item_where}.start_message_id: expected a non-empty string"
            ));
        }
        result.push(spec);
    }
    let ids: Vec<&str> = result.iter().map(|spec| spec.session_id.as_str()).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != ids.len() {
        return Err(format!("{where_}: duplicate session ids are not allowed"));
    }
    Ok(result)
}

#[derive(Clone, Debug, Default)]
struct WindowSpec {
    start_date: Option<String>,
    start_time: Option<String>,
    end_date: Option<String>,
    end_time: Option<String>,
}

impl WindowSpec {
    fn resolve(&self, timezone: &str) -> Result<Option<DateWindow>, String> {
        parse_date_window(
            self.start_date.as_deref(),
            self.end_date.as_deref(),
            timezone,
            self.start_time.as_deref(),
            self.end_time.as_deref(),
        )
    }
}

fn expand_user(raw: &str) -> PathBuf {
    if raw == "~" || raw.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(&raw[2.min(raw.len())..]);
        }
    }
    PathBuf::from(raw)
}

fn relative_output(config_path: &Path, raw: &str) -> Result<PathBuf, String> {
    let candidate = expand_user(raw);
    if candidate.is_absolute() {
        return Err("project config output must be relative to the config file".to_string());
    }
    let parent = config_path.parent().unwrap_or(Path::new("/"));
    Ok(crate::json::resolve_path(&parent.join(candidate)))
}

fn resolved_against(config_path: &Path, raw: &str) -> PathBuf {
    let candidate = expand_user(raw);
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        config_path
            .parent()
            .unwrap_or(Path::new("/"))
            .join(candidate)
    };
    crate::json::resolve_path(&absolute)
}

fn project_values(value: &Value, where_: &str) -> Result<Vec<String>, String> {
    let items = as_array(value, where_)?;
    let mut result: Vec<String> = Vec::new();
    for (index, raw) in items.iter().enumerate() {
        let item_where = format!("{where_}[{index}]");
        let item = as_object(raw, &item_where)?;
        check_fields(item, &item_where, &["label", "repository_url"], &[])?;
        let label = required_string(item.get("label"), &format!("{item_where}.label"))?;
        let repository_url = required_string(
            item.get("repository_url"),
            &format!("{item_where}.repository_url"),
        )?;
        result.push(format!("{label}={repository_url}"));
    }
    Ok(result)
}

fn window_spec(value: &Value, where_: &str) -> Result<WindowSpec, String> {
    let obj = as_object(value, where_)?;
    check_fields(
        obj,
        where_,
        &[],
        &["start_date", "start_time", "end_date", "end_time"],
    )?;
    let spec = WindowSpec {
        start_date: optional_string(obj, "start_date", where_)?,
        start_time: optional_string(obj, "start_time", where_)?,
        end_date: optional_string(obj, "end_date", where_)?,
        end_time: optional_string(obj, "end_time", where_)?,
    };
    if spec.start_date.is_some() && spec.start_time.is_some() {
        return Err(format!(
            "{where_}: choose start_date or start_time, not both"
        ));
    }
    if spec.end_date.is_some() && spec.end_time.is_some() {
        return Err(format!("{where_}: choose end_date or end_time, not both"));
    }
    Ok(spec)
}

fn prompt_authorship_rules(
    value: &Value,
    where_: &str,
    team_slug: &str,
) -> Result<Vec<PromptAuthorshipRule>, String> {
    let items = as_array(value, where_)?;
    let mut result: Vec<PromptAuthorshipRule> = Vec::new();
    for (index, raw) in items.iter().enumerate() {
        let item_where = format!("{where_}[{index}]");
        let item = as_object(raw, &item_where)?;
        check_fields(
            item,
            &item_where,
            &["id", "ingress_kind", "author_kind", "reason"],
            &["start_time", "end_time", "source_native_ids"],
        )?;
        let window = parse_date_window(
            None,
            None,
            "UTC",
            optional_string(item, "start_time", &item_where)?.as_deref(),
            optional_string(item, "end_time", &item_where)?.as_deref(),
        )?;
        let source_native_ids = match item.get("source_native_ids") {
            Some(held) => string_array(held, &format!("{item_where}.source_native_ids"))?,
            None => Vec::new(),
        };
        result.push(PromptAuthorshipRule {
            rule_id: required_string(item.get("id"), &format!("{item_where}.id"))?,
            team_slug: team_slug.to_string(),
            ingress_kind: required_string(
                item.get("ingress_kind"),
                &format!("{item_where}.ingress_kind"),
            )?,
            author_kind: required_string(
                item.get("author_kind"),
                &format!("{item_where}.author_kind"),
            )?,
            reason: required_string(item.get("reason"), &format!("{item_where}.reason"))?,
            start_ms: window.as_ref().and_then(|held| held.start_ms),
            end_ms: window.as_ref().and_then(|held| held.end_ms),
            source_native_ids,
        });
    }
    Ok(result)
}

fn codex_source(
    value: &Map<String, Value>,
    where_: &str,
    config_path: &Path,
) -> Result<CodexProjectSource, String> {
    check_fields(
        value,
        where_,
        &["sessions_root", "root_session"],
        &["continuation_sessions"],
    )?;
    let root_session =
        required_string(value.get("root_session"), &format!("{where_}.root_session"))?;
    let continuations = match value.get("continuation_sessions") {
        Some(held) => string_array(held, &format!("{where_}.continuation_sessions"))?,
        None => Vec::new(),
    };
    if continuations.contains(&root_session) {
        return Err(format!(
            "{where_}: root_session cannot also be a continuation"
        ));
    }
    Ok(CodexProjectSource {
        sessions_root: resolved_against(
            config_path,
            &required_string(
                value.get("sessions_root"),
                &format!("{where_}.sessions_root"),
            )?,
        ),
        root_session,
        continuation_sessions: continuations,
    })
}

fn claude_source(
    value: &Map<String, Value>,
    where_: &str,
    config_path: &Path,
) -> Result<ClaudeProjectSource, String> {
    check_fields(value, where_, &["session_file"], &[])?;
    Ok(ClaudeProjectSource {
        session_file: resolved_against(
            config_path,
            &required_string(value.get("session_file"), &format!("{where_}.session_file"))?,
        ),
    })
}

fn muse_source(
    value: &Map<String, Value>,
    where_: &str,
    config_path: &Path,
) -> Result<MuseProjectSource, String> {
    check_fields(value, where_, &["session_dir"], &[])?;
    Ok(MuseProjectSource {
        session_dir: resolved_against(
            config_path,
            &required_string(value.get("session_dir"), &format!("{where_}.session_dir"))?,
        ),
    })
}

fn claude_history_source(
    value: &Map<String, Value>,
    where_: &str,
    config_path: &Path,
) -> Result<ClaudeHistoryProjectSource, String> {
    check_fields(
        value,
        where_,
        &["history_file", "project_pattern"],
        &["covered_sessions"],
    )?;
    let pattern = required_string(
        value.get("project_pattern"),
        &format!("{where_}.project_pattern"),
    )?;
    crate::history::compile_project_pattern(&pattern)
        .map_err(|error| format!("{where_}.project_pattern: {error}"))?;
    let covered = match value.get("covered_sessions") {
        Some(held) => required_string(Some(held), &format!("{where_}.covered_sessions"))?,
        None => "config".to_string(),
    };
    if covered != "config" && covered != "none" {
        return Err(format!(
            "{where_}.covered_sessions: expected config or none"
        ));
    }
    Ok(ClaudeHistoryProjectSource {
        history_file: resolved_against(
            config_path,
            &required_string(value.get("history_file"), &format!("{where_}.history_file"))?,
        ),
        project_pattern: pattern,
        covered_sessions: covered,
    })
}

fn orc_source(
    value: &Map<String, Value>,
    where_: &str,
    config_path: &Path,
) -> Result<OrcProjectSource, String> {
    check_fields(
        value,
        where_,
        &["source_root", "root_session"],
        &["continuation_sessions"],
    )?;
    let root_session =
        required_string(value.get("root_session"), &format!("{where_}.root_session"))?;
    let continuations = match value.get("continuation_sessions") {
        Some(held) => orc_continuation_array(held, &format!("{where_}.continuation_sessions"))?,
        None => Vec::new(),
    };
    if continuations
        .iter()
        .any(|spec| spec.session_id == root_session)
    {
        return Err(format!(
            "{where_}: root_session cannot also be a continuation"
        ));
    }
    Ok(OrcProjectSource {
        source_root: resolved_against(
            config_path,
            &required_string(value.get("source_root"), &format!("{where_}.source_root"))?,
        ),
        root_session,
        continuation_sessions: continuations,
    })
}

#[allow(clippy::too_many_arguments)]
fn team_config(
    value: &Value,
    where_: &str,
    config_path: &Path,
    default_timezone: &str,
    default_projects: &[String],
    default_hosts: &[String],
    default_window: &WindowSpec,
) -> Result<ProjectTeamConfig, String> {
    let obj = as_object(value, where_)?;
    check_fields(
        obj,
        where_,
        &["slug", "provider", "source"],
        &[
            "timezone",
            "projects",
            "source_hosts",
            "window",
            "prompt_authorship_rules",
        ],
    )?;
    let slug = required_string(obj.get("slug"), &format!("{where_}.slug"))?;
    if slug.chars().count() > 64 || !valid_team_slug(&slug) {
        return Err(format!(
            "{where_}.slug: expected 1-64 lowercase letters/digits separated by hyphens"
        ));
    }
    let provider = required_string(obj.get("provider"), &format!("{where_}.provider"))?;
    if !["codex", "claude", "orc", "muse", "claude-history"].contains(&provider.as_str()) {
        return Err(format!(
            "{where_}.provider: expected codex, claude, orc, muse, or claude-history"
        ));
    }
    let source_value = as_object(
        obj.get("source").unwrap_or(&Value::Null),
        &format!("{where_}.source"),
    )?;
    let source_where = format!("{where_}.source");
    let source = match provider.as_str() {
        "codex" => ProjectSource::Codex(codex_source(source_value, &source_where, config_path)?),
        "claude" => ProjectSource::Claude(claude_source(source_value, &source_where, config_path)?),
        "muse" => ProjectSource::Muse(muse_source(source_value, &source_where, config_path)?),
        "claude-history" => ProjectSource::ClaudeHistory(claude_history_source(
            source_value,
            &source_where,
            config_path,
        )?),
        _ => ProjectSource::Orc(orc_source(source_value, &source_where, config_path)?),
    };
    let timezone = match obj.get("timezone") {
        Some(held) => required_string(Some(held), &format!("{where_}.timezone"))?,
        None => default_timezone.to_string(),
    };
    let project_values = match obj.get("projects") {
        Some(held) => project_values(held, &format!("{where_}.projects"))?,
        None => default_projects.to_vec(),
    };
    let host_values = match obj.get("source_hosts") {
        Some(held) => string_array(held, &format!("{where_}.source_hosts"))?,
        None => default_hosts.to_vec(),
    };
    let (projects, hosts) = parse_identity_overrides(&project_values, &host_values)?;
    let spec = match obj.get("window") {
        Some(held) => window_spec(held, &format!("{where_}.window"))?,
        None => default_window.clone(),
    };
    let authorship_rules = match obj.get("prompt_authorship_rules") {
        Some(held) => {
            prompt_authorship_rules(held, &format!("{where_}.prompt_authorship_rules"), &slug)?
        }
        None => Vec::new(),
    };
    Ok(ProjectTeamConfig {
        slug,
        provider,
        source,
        date_window: spec.resolve(&timezone)?,
        timezone,
        identity_overrides: IdentityOverrides {
            projects,
            hosts,
            display_timezone_source: "config".to_string(),
        },
        prompt_authorship_rules: authorship_rules,
    })
}

fn valid_team_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Load and strictly validate one project-ingest schema-v1
/// JSON file.
pub fn load_project_ingest_config(path: &Path) -> Result<ProjectIngestConfig, String> {
    let config_path = crate::json::resolve_path(&expand_user(&path.to_string_lossy()));
    let config_bytes =
        std::fs::read(&config_path).map_err(|error| crate::json::os_error(&error, &config_path))?;
    let text = String::from_utf8(config_bytes.clone())
        .map_err(|error| format!("{}: invalid utf-8: {}", config_path.display(), error))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{}: invalid JSON: {error}", config_path.display()))?;
    let where_ = config_path.display().to_string();
    let root = as_object(&document, &where_)?;
    check_fields(
        root,
        &where_,
        &["schema_version", "output", "teams"],
        &[
            "timezone",
            "projects",
            "source_hosts",
            "window",
            "snapshot_root",
        ],
    )?;
    let schema_version = as_int(
        root.get("schema_version").unwrap_or(&Value::Null),
        &format!("{where_}.schema_version"),
    )?;
    if schema_version != PROJECT_INGEST_SCHEMA_VERSION {
        return Err(format!(
            "{}: unsupported schema_version {}; expected {}",
            config_path.display(),
            schema_version,
            PROJECT_INGEST_SCHEMA_VERSION
        ));
    }
    let output = relative_output(
        &config_path,
        &required_string(root.get("output"), &format!("{where_}.output"))?,
    )?;
    let snapshot_root = match root.get("snapshot_root") {
        Some(held) => Some(resolved_against(
            &config_path,
            &required_string(Some(held), &format!("{where_}.snapshot_root"))?,
        )),
        None => None,
    };
    let timezone = match root.get("timezone") {
        Some(held) => required_string(Some(held), &format!("{where_}.timezone"))?,
        None => "America/New_York".to_string(),
    };
    let projects = match root.get("projects") {
        Some(held) => project_values(held, &format!("{where_}.projects"))?,
        None => Vec::new(),
    };
    let source_hosts = match root.get("source_hosts") {
        Some(held) => string_array(held, &format!("{where_}.source_hosts"))?,
        None => Vec::new(),
    };
    let window = match root.get("window") {
        Some(held) => window_spec(held, &format!("{where_}.window"))?,
        None => WindowSpec::default(),
    };
    let raw_teams = as_array(
        root.get("teams").unwrap_or(&Value::Null),
        &format!("{where_}.teams"),
    )?;
    if raw_teams.is_empty() {
        return Err(format!("{where_}.teams: expected at least one team"));
    }
    let mut teams: Vec<ProjectTeamConfig> = Vec::new();
    for (index, raw) in raw_teams.iter().enumerate() {
        teams.push(team_config(
            raw,
            &format!("{where_}.teams[{index}]"),
            &config_path,
            &timezone,
            &projects,
            &source_hosts,
            &window,
        )?);
    }
    let slugs: Vec<&str> = teams.iter().map(|team| team.slug.as_str()).collect();
    let mut unique = slugs.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != slugs.len() {
        return Err(format!(
            "{where_}.teams: duplicate team slugs are not allowed"
        ));
    }
    let rule_ids: Vec<&str> = teams
        .iter()
        .flat_map(|team| {
            team.prompt_authorship_rules
                .iter()
                .map(|rule| rule.rule_id.as_str())
        })
        .collect();
    let mut unique_rules = rule_ids.clone();
    unique_rules.sort();
    unique_rules.dedup();
    if unique_rules.len() != rule_ids.len() {
        return Err(format!(
            "{where_}.teams: duplicate prompt authorship rule ids are not allowed"
        ));
    }
    let config_sha256 = {
        let mut hasher = sha2::Sha256::new();
        hasher.update(&config_bytes);
        format!("{:x}", hasher.finalize())
    };
    Ok(ProjectIngestConfig {
        config_path,
        config_sha256,
        output,
        snapshot_root,
        teams,
    })
}

/// Dispatch one configured team to its provider importer.
fn ingest_team(
    output: &Path,
    team: &ProjectTeamConfig,
    accept_prefix_rewrite: &[String],
    snapshot_root: Option<&Path>,
    claude_session_ids: &[String],
) -> Result<IngestReport, String> {
    let overrides = &team.identity_overrides;
    let report = match &team.source {
        ProjectSource::ClaudeHistory(source) => {
            crate::ingest::ingest_claude_history(
                output,
                &source.history_file,
                &team.slug,
                &team.timezone,
                &source.project_pattern,
                if source.covered_sessions == "config" {
                    claude_session_ids
                } else {
                    &[]
                },
                team.date_window.as_ref(),
                overrides,
                snapshot_root,
            )?
            .1
        }
        ProjectSource::Codex(source) => {
            crate::ingest::ingest_codex(
                output,
                &source.sessions_root,
                &source.root_session,
                &team.slug,
                &team.timezone,
                team.date_window.as_ref(),
                overrides,
                &source.continuation_sessions,
                snapshot_root,
            )?
            .1
        }
        ProjectSource::Claude(source) => {
            crate::ingest::ingest_claude(
                output,
                &source.session_file,
                &team.slug,
                &team.timezone,
                team.date_window.as_ref(),
                overrides,
                snapshot_root,
            )?
            .1
        }
        ProjectSource::Muse(source) => {
            crate::ingest::ingest_muse(
                output,
                &source.session_dir,
                &team.slug,
                &team.timezone,
                team.date_window.as_ref(),
                overrides,
                snapshot_root,
            )?
            .1
        }
        ProjectSource::Orc(source) => {
            crate::ingest::ingest_orc(
                output,
                &source.source_root,
                &source.root_session,
                &team.slug,
                &team.timezone,
                team.date_window.as_ref(),
                overrides,
                &source.continuation_sessions,
                accept_prefix_rewrite,
                snapshot_root,
            )?
            .1
        }
    };
    Ok(report)
}

fn errno_error_type(message: &str) -> Option<&'static str> {
    let rest = message.strip_prefix("[Errno ")?;
    let errno: i64 = rest.split(']').next()?.parse().ok()?;
    Some(match errno {
        2 => "FileNotFoundError",
        13 => "PermissionError",
        17 => "FileExistsError",
        20 => "NotADirectoryError",
        21 => "IsADirectoryError",
        _ => "OSError",
    })
}

fn team_failure(team: &ProjectTeamConfig, error: &str) -> ProjectTeamIngestFailure {
    // Every provider parse error is a ValueError in the
    // oracle, and every filesystem fault an OSError; that
    // pair is the classified surface, so nothing here
    // carries a traceback.
    let error_type = match errno_error_type(error) {
        Some(held) => held.to_string(),
        None => match team.provider.as_str() {
            "codex" => "CodexParseError",
            "claude" => "ClaudeParseError",
            "claude-history" => "ClaudeHistoryParseError",
            "muse" => "MuseParseError",
            _ => "OrcParseError",
        }
        .to_string(),
    };
    ProjectTeamIngestFailure {
        team_slug: team.slug.clone(),
        provider: team.provider.clone(),
        error_type,
        error: error.to_string(),
    }
}

fn prefix_rewrite_selection(
    selected: &[ProjectTeamConfig],
    requested: &[String],
) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut unique = requested.to_vec();
    unique.sort();
    unique.dedup();
    if unique.len() != requested.len() {
        return Err("--accept-orc-prefix-rewrite selection contains duplicates".to_string());
    }
    let mut accepted: Vec<(String, Vec<String>)> = Vec::new();
    let mut malformed: Vec<String> = Vec::new();
    for value in requested {
        let Some((slug, session_id)) = value.split_once(':') else {
            malformed.push(value.clone());
            continue;
        };
        if slug.is_empty() || session_id.is_empty() {
            malformed.push(value.clone());
            continue;
        }
        match accepted.iter_mut().find(|(held, _)| held == slug) {
            Some((_, sessions)) => {
                if !sessions.iter().any(|held| held == session_id) {
                    sessions.push(session_id.to_string());
                }
            }
            None => accepted.push((slug.to_string(), vec![session_id.to_string()])),
        }
    }
    if !malformed.is_empty() {
        return Err(format!(
            "--accept-orc-prefix-rewrite takes TEAM:SESSION, naming the one team and the one session the refusal showed you; these do not: {}",
            malformed.join(", ")
        ));
    }
    let mut unknown: Vec<String> = accepted
        .iter()
        .map(|(slug, _)| slug.clone())
        .filter(|slug| !selected.iter().any(|team| &team.slug == slug))
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        return Err(format!(
            "--accept-orc-prefix-rewrite names teams this run does not ingest: {}",
            unknown.join(", ")
        ));
    }
    let mut not_orc: Vec<String> = accepted
        .iter()
        .map(|(slug, _)| slug.clone())
        .filter(|slug| {
            selected
                .iter()
                .find(|team| &team.slug == slug)
                .map(|team| team.provider != "orc")
                .unwrap_or(false)
        })
        .collect();
    not_orc.sort();
    if !not_orc.is_empty() {
        return Err(format!(
            "--accept-orc-prefix-rewrite applies only to orc teams; these are not: {}",
            not_orc.join(", ")
        ));
    }
    Ok(accepted)
}

/// Ingest selected registered teams, then refresh the
/// global transcript JSONL.
pub fn ingest_project(
    config: &ProjectIngestConfig,
    requested_teams: &[String],
    accept_prefix_rewrite: &[String],
) -> Result<ProjectIngestReport, String> {
    let selected = config.select_teams(requested_teams)?;
    let accepted = prefix_rewrite_selection(&selected, accept_prefix_rewrite)?;
    let claude_session_ids = config.claude_session_ids();
    let mut results: Vec<ProjectTeamIngestResult> = Vec::new();
    let mut failures: Vec<ProjectTeamIngestFailure> = Vec::new();
    // One team must not be able to end the run.
    for team in &selected {
        let mut sessions: Vec<String> = accepted
            .iter()
            .find(|(slug, _)| slug == &team.slug)
            .map(|(_, held)| held.clone())
            .unwrap_or_default();
        sessions.sort();
        match ingest_team(
            &config.output,
            team,
            &sessions,
            config.snapshot_root.as_deref(),
            &claude_session_ids,
        ) {
            Ok(report) => results.push(ProjectTeamIngestResult {
                team_slug: team.slug.clone(),
                provider: team.provider.clone(),
                ingest: report,
            }),
            Err(error) => failures.push(team_failure(team, &error)),
        }
    }
    let rules = config.prompt_authorship_rules();
    let (transcripts, transcript_error) =
        match crate::transcript_export::extract_transcripts_archive_with_rules(
            &config.output,
            &rules,
        ) {
            Ok(report) => (Some(report), None),
            Err(error) => {
                let error_type = errno_error_type(&error).unwrap_or("ValueError");
                (None, Some(format!("{error_type}: {error}")))
            }
        };
    let mut accepted_pairs: Vec<String> = accepted
        .iter()
        .flat_map(|(slug, sessions)| {
            sessions
                .iter()
                .map(|session| format!("{slug}:{session}"))
                .collect::<Vec<_>>()
        })
        .collect();
    accepted_pairs.sort();
    Ok(ProjectIngestReport {
        config_sha256: config.config_sha256.clone(),
        teams: results,
        transcripts,
        failures,
        transcript_error,
        accept_prefix_rewrite_sessions: accepted_pairs,
    })
}
