//! Read-only audit of retired mechanical glossary projections.
//! The Rust mirror of `wrkviz.glossary_audit`.

use crate::json::{as_array, as_int, as_object, as_string, py_repr_str, read_json};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Audit schema version.
pub const AUDIT_SCHEMA_VERSION: i64 = 1;
/// Publication policy name.
pub const PUBLICATION_POLICY: &str = "semantic-only-v1";

const GENERIC_LANGUAGE: &[&str] = &[
    "agent-driven",
    "agent-friendly",
    "and",
    "anti-pattern",
    "as-is",
    "best-in-class",
    "bot-created",
    "bpf-based",
    "built-in",
    "case-by-case",
    "codex-led",
    "compile-time",
    "continuous-virtual-time-is-sacred",
    "counter-example",
    "cross-backend",
    "dev-repo",
    "fail-closed",
    "false",
    "fetch-and-add",
    "guest-visible",
    "half-finished",
    "human-created",
    "human-readable",
    "human-review",
    "in-guest",
    "in-process",
    "in-repo",
    "locally-validated",
    "main",
    "multi-day",
    "no-progress",
    "non-functional",
    "non-green",
    "non-leftmost",
    "one-line",
    "or",
    "output-file",
    "per-category",
    "per-coordinator",
    "per-session",
    "per-syscall",
    "per-team",
    "per-thread",
    "post-facto",
    "pre-commit",
    "project-specific",
    "prompt",
    "read-only",
    "recurring",
    "self-hosted",
    "session-scoped",
    "syscall-heavy",
    "the-coordinator",
    "top-level",
    "true",
    "wall-clock",
    "whitespace-delimited",
];
const PROCESS_ACTIONS: &[&str] = &[
    "auto-clears",
    "auto-expire",
    "auto-skip",
    "battle-testing",
    "fast-land",
    "ff-merged",
    "force-push",
    "get-target",
    "kicking-the-tires",
    "list-hosts",
    "over-index",
    "pre-work",
    "re-measured",
    "run-on-every",
    "support-yet",
    "testing-no-reboots",
    "throw-away",
];
const GENERIC_ACRONYMS: &[&str] = &[
    "API", "ARGUMENT", "BAD", "CI", "CLAUDE", "CLI", "CPU", "DRY", "FAIL", "FAST", "HW", "JSON",
    "JSONL", "OSS", "P0", "PR", "TUI", "UH", "WAY",
];

static TEAM_SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").unwrap());
static COMMAND_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:local-)?command-(?:args|caveat|message|name|stdout|stderr)$").unwrap()
});
static DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:\d+(?:\.\d+)?|N)[smhdw]$").unwrap());
static HEX_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{7,64}$").unwrap());
static HEX_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{7,64}\.\.[0-9a-f]{7,64}$").unwrap());
static ISSUE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^issuecomment-[0-9]+$").unwrap());
static ENV_ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]{2,}=").unwrap());
static METHOD_OR_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:::|\(\)|\([^)]*\))").unwrap());
static ERRNO_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^E[A-Z0-9_]{3,}$").unwrap());
static TEAM_OR_RECYCLED_AGENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(?:claude|codex|orc)-coord(?:-[0-9]+)?|agent-[0-9]+|goal-[a-z0-9-]+)$")
        .unwrap()
});
static OPERATIONAL_DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:is|means|names)\s+(?:an?\s+|the\s+)?(?:`[^`]+`\s+)?(?:command|option|flag|field|method|function|branch|label|team|environment variable)\b",
    )
    .unwrap()
});
static SCHEDULE_EXAMPLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:every\s+(?:\d+|N)\s*(?:seconds?|minutes?|hours?|days?|weeks?)|(?:check|run)\s+.+)$").unwrap()
});
static KIND_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "project",
            Regex::new(r"(?i)\b(?:repository|repo|workspace|codebase|development tree)\b").unwrap(),
        ),
        (
            "system",
            Regex::new(r"(?i)\b(?:system|subsystem|backend|runtime|engine|tool|server|service|framework|library|interface|protocol|platform|scheduler|runner|benchmark|workload|crate|package|matrix|instrumentation|front[ -]?end|test suite)\b").unwrap(),
        ),
        (
            "workstream",
            Regex::new(r"(?i)\b(?:workstream|task|initiative|track|investigation|audit|migration|landing workflow)\b").unwrap(),
        ),
        (
            "milestone",
            Regex::new(r"(?i)\b(?:milestone|release|achievement|shipped|landed|completed)\b").unwrap(),
        ),
    ]
});

fn casefold(text: &str) -> String {
    text.to_lowercase()
}

/// One retired term's mechanical triage.
#[derive(Clone, Debug)]
pub struct LegacyTermAudit {
    /// Term text.
    pub term: String,
    /// Term id.
    pub term_id: String,
    /// Definition status.
    pub definition_status: String,
    /// Disposition.
    pub disposition: &'static str,
    /// Reason.
    pub reason: String,
    /// Semantic kind hints.
    pub semantic_kind_hints: Vec<&'static str>,
}

impl LegacyTermAudit {
    fn to_json_obj(&self) -> Value {
        json!({
            "term": self.term,
            "term_id": self.term_id,
            "definition_status": self.definition_status,
            "disposition": self.disposition,
            "reason": self.reason,
            "semantic_kind_hints": self.semantic_kind_hints,
            "publication_authority": false,
        })
    }
}

/// One team's retired glossary audit.
#[derive(Clone, Debug)]
pub struct LegacyTeamGlossaryAudit {
    /// Team slug.
    pub team_slug: String,
    /// Cache path relative to the archive.
    pub cache_path: String,
    /// Cache present.
    pub cache_present: bool,
    /// Cache schema version.
    pub cache_schema_version: Option<i64>,
    /// Audited terms.
    pub terms: Vec<LegacyTermAudit>,
}

impl LegacyTeamGlossaryAudit {
    fn rejected_count(&self) -> i64 {
        self.terms
            .iter()
            .filter(|item| item.disposition == "rejected-mechanical")
            .count() as i64
    }

    fn review_count(&self) -> i64 {
        self.terms
            .iter()
            .filter(|item| item.disposition == "semantic-review-required")
            .count() as i64
    }

    fn reason_counts(&self) -> BTreeMap<String, i64> {
        let mut counts = BTreeMap::new();
        for item in &self.terms {
            *counts.entry(item.reason.clone()).or_insert(0) += 1;
        }
        counts
    }

    fn to_json_obj(&self, include_terms: bool) -> Value {
        let mut result = json!({
            "team_slug": self.team_slug,
            "cache_path": self.cache_path,
            "cache_present": self.cache_present,
            "cache_schema_version": self.cache_schema_version,
            "legacy_candidates": self.terms.len(),
            "rejected_mechanical": self.rejected_count(),
            "semantic_review_required": self.review_count(),
            "published_from_legacy_cache": 0,
            "reason_counts": self.reason_counts(),
        });
        if include_terms {
            result["terms"] = Value::Array(
                self.terms
                    .iter()
                    .map(LegacyTermAudit::to_json_obj)
                    .collect(),
            );
        }
        result
    }
}

/// Archive-wide glossary audit report.
#[derive(Clone, Debug)]
pub struct LegacyGlossaryAuditReport {
    /// Archive path (resolved).
    pub archive: String,
    /// Per-team audits.
    pub teams: Vec<LegacyTeamGlossaryAudit>,
}

impl LegacyGlossaryAuditReport {
    fn legacy_candidates(&self) -> i64 {
        self.teams.iter().map(|team| team.terms.len() as i64).sum()
    }

    fn rejected_count(&self) -> i64 {
        self.teams.iter().map(|team| team.rejected_count()).sum()
    }

    fn review_count(&self) -> i64 {
        self.teams.iter().map(|team| team.review_count()).sum()
    }

    /// JSON form of the report.
    pub fn to_json_obj(&self, include_terms: bool) -> Value {
        json!({
            "schema_version": AUDIT_SCHEMA_VERSION,
            "archive": self.archive,
            "publication_policy": PUBLICATION_POLICY,
            "legacy_cache_publication_authority": false,
            "totals": {
                "teams": self.teams.len(),
                "legacy_candidates": self.legacy_candidates(),
                "rejected_mechanical": self.rejected_count(),
                "semantic_review_required": self.review_count(),
                "published_from_legacy_cache": 0,
            },
            "teams": self
                .teams
                .iter()
                .map(|team| team.to_json_obj(include_terms))
                .collect::<Vec<_>>(),
        })
    }
}

fn semantic_kind_hints(definition: &str) -> Vec<&'static str> {
    KIND_PATTERNS
        .iter()
        .filter(|(_, pattern)| pattern.is_match(definition))
        .map(|(kind, _)| *kind)
        .collect()
}

fn mechanical_rejection_reason(
    term: &str,
    definition_status: &str,
    definition: &str,
    context: &str,
) -> Option<&'static str> {
    let clean = term.trim();
    let folded = casefold(clean);
    if definition_status != "supported" {
        return Some("definition-not-supported");
    }
    if COMMAND_FIELD.is_match(clean) || casefold(context).contains(&casefold(&format!("<{clean}>")))
    {
        return Some("transcript-markup-field");
    }
    if GENERIC_LANGUAGE.contains(&folded.as_str()) || GENERIC_ACRONYMS.contains(&clean) {
        return Some("generic-language-or-workflow");
    }
    if PROCESS_ACTIONS.contains(&folded.as_str()) {
        return Some("process-action");
    }
    if DURATION.is_match(clean) || SCHEDULE_EXAMPLE.is_match(clean) {
        return Some("duration-or-command-example");
    }
    if HEX_LITERAL.is_match(clean)
        || HEX_RANGE.is_match(clean)
        || ISSUE_COMMENT.is_match(clean)
        || ERRNO_NAME.is_match(clean)
        || TEAM_OR_RECYCLED_AGENT.is_match(clean)
    {
        return Some("opaque-record-identifier");
    }
    let mut chars = clean.chars();
    let first = chars.next();
    let last = clean.chars().last();
    if clean.is_empty()
        || matches!(first, Some('.') | Some(')') | Some(','))
        || matches!(last, Some(',') | Some(':') | Some('('))
        || clean.contains(',')
        || clean.contains(':')
        || clean.contains("...")
        || clean.split_whitespace().count() > 8
    {
        return Some("opaque-or-prose-fragment");
    }
    if clean.contains('_')
        || ENV_ASSIGNMENT.is_match(clean)
        || METHOD_OR_CALL.is_match(clean)
        || clean.contains('=')
    {
        return Some("code-or-configuration-literal");
    }
    if folded.starts_with("sudo ") || folded.starts_with("with-proxy ") {
        return Some("operational-command");
    }
    let head: String = definition.chars().take(240).collect();
    if OPERATIONAL_DEFINITION.is_match(&head) {
        return Some("operational-command-or-label");
    }
    None
}

/// Triage one retired entry.
pub fn audit_legacy_term(
    term: &str,
    term_id: &str,
    definition_status: &str,
    definition: &str,
    context: &str,
) -> Result<LegacyTermAudit, String> {
    let expected_id = crate::build_assets::glossary_term_id(term)?;
    if term_id != expected_id {
        return Err(format!(
            "legacy glossary ID {} does not match term {}",
            py_repr_str(term_id),
            py_repr_str(term)
        ));
    }
    let rejection = mechanical_rejection_reason(term, definition_status, definition, context);
    let hints = semantic_kind_hints(definition);
    if let Some(reason) = rejection {
        return Ok(LegacyTermAudit {
            term: term.to_string(),
            term_id: term_id.to_string(),
            definition_status: definition_status.to_string(),
            disposition: "rejected-mechanical",
            reason: reason.to_string(),
            semantic_kind_hints: Vec::new(),
        });
    }
    if hints.is_empty() {
        return Ok(LegacyTermAudit {
            term: term.to_string(),
            term_id: term_id.to_string(),
            definition_status: definition_status.to_string(),
            disposition: "rejected-mechanical",
            reason: "no-semantic-kind-evidence".to_string(),
            semantic_kind_hints: Vec::new(),
        });
    }
    Ok(LegacyTermAudit {
        term: term.to_string(),
        term_id: term_id.to_string(),
        definition_status: definition_status.to_string(),
        disposition: "semantic-review-required",
        reason: "legacy-schema-lacks-semantic-classification".to_string(),
        semantic_kind_hints: hints,
    })
}

fn selected_teams(archive: &Path, team_slugs: &[String]) -> Result<Vec<String>, String> {
    let selected: Vec<String> = if !team_slugs.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for slug in team_slugs {
            if !seen.insert(slug.clone()) {
                return Err("glossary audit team selection contains duplicates".to_string());
            }
        }
        team_slugs.to_vec()
    } else {
        let teams_root = archive.join("teams");
        let mut names: Vec<String> = crate::build_store::ingested_team_slugs(archive)?;
        if teams_root.is_dir() {
            let entries = std::fs::read_dir(&teams_root)
                .map_err(|error| crate::json::os_error(&error, &teams_root))?;
            for entry in entries.flatten() {
                let path = entry.path();
                let is_link = path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
                if path.is_dir() && !is_link {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
        }
        if names.is_empty()
            && !teams_root.is_dir()
            && !crate::build_store::candidate_store_roots(archive)?
                .iter()
                .any(|root| root.is_dir())
        {
            return Err(format!(
                "no team directory found at {}",
                teams_root.display()
            ));
        }
        names.sort();
        names
    };
    if selected.is_empty() {
        return Err(format!("no teams found in {}", archive.display()));
    }
    for slug in &selected {
        if !TEAM_SLUG.is_match(slug) {
            return Err(format!("invalid team slug {}", py_repr_str(slug)));
        }
    }
    Ok(selected)
}

fn audit_team(archive: &Path, team_slug: &str) -> Result<LegacyTeamGlossaryAudit, String> {
    let path: PathBuf = archive
        .join("teams")
        .join(team_slug)
        .join("summary_data")
        .join("glossary.json");
    let relative_path = format!("teams/{team_slug}/summary_data/glossary.json");
    let is_link = path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_link {
        return Err(format!(
            "refusing symlinked legacy glossary cache: {}",
            path.display()
        ));
    }
    if !path.is_file() {
        return Ok(LegacyTeamGlossaryAudit {
            team_slug: team_slug.to_string(),
            cache_path: relative_path,
            cache_present: false,
            cache_schema_version: None,
            terms: Vec::new(),
        });
    }
    let root_value = read_json(&path)?;
    let root = as_object(&root_value, &path.display().to_string())?;
    let schema_version = as_int(
        root.get("schema_version").unwrap_or(&Value::Null),
        &format!("{}.schema_version", path.display()),
    )?;
    if schema_version != 3 {
        return Err(format!(
            "{}: audit supports retired glossary schema 3, found {schema_version}",
            path.display()
        ));
    }
    let mut terms = Vec::new();
    for (index, raw_term) in as_array(
        root.get("terms").unwrap_or(&Value::Null),
        &format!("{}.terms", path.display()),
    )?
    .iter()
    .enumerate()
    {
        let where_ = format!("{}.terms[{index}]", path.display());
        let item = as_object(raw_term, &where_)?;
        terms.push(audit_legacy_term(
            as_string(
                item.get("term").unwrap_or(&Value::Null),
                &format!("{where_}.term"),
            )?,
            as_string(
                item.get("term_id").unwrap_or(&Value::Null),
                &format!("{where_}.term_id"),
            )?,
            as_string(
                item.get("definition_status").unwrap_or(&Value::Null),
                &format!("{where_}.definition_status"),
            )?,
            as_string(
                item.get("definition").unwrap_or(&Value::Null),
                &format!("{where_}.definition"),
            )?,
            as_string(
                item.get("context").unwrap_or(&Value::Null),
                &format!("{where_}.context"),
            )?,
        )?);
    }
    Ok(LegacyTeamGlossaryAudit {
        team_slug: team_slug.to_string(),
        cache_path: relative_path,
        cache_present: true,
        cache_schema_version: Some(schema_version),
        terms,
    })
}

/// Audit retired caches without a lock or a model call.
pub fn audit_legacy_glossaries(
    archive: &Path,
    team_slugs: &[String],
) -> Result<LegacyGlossaryAuditReport, String> {
    crate::cli::validation_checkpoint();
    let selected = selected_teams(archive, team_slugs)?;
    let resolved = crate::json::resolve_path(archive);
    let mut teams = Vec::new();
    for slug in &selected {
        teams.push(audit_team(archive, slug)?);
    }
    Ok(LegacyGlossaryAuditReport {
        archive: resolved.display().to_string(),
        teams,
    })
}

/// Render the report as operator text.
pub fn format_glossary_audit(report: &LegacyGlossaryAuditReport, include_terms: bool) -> String {
    let mut lines: Vec<String> = vec![
        format!("Glossary publication policy: {PUBLICATION_POLICY}"),
        "Legacy schema-3 definitions have no publication authority; builds link 0 of them."
            .to_string(),
        format!(
            "Archive total: {} legacy candidates; {} mechanically rejected; {} require semantic review; 0 published",
            report.legacy_candidates(),
            report.rejected_count(),
            report.review_count()
        ),
    ];
    for team in &report.teams {
        if !team.cache_present {
            lines.push(format!("{}: no legacy glossary cache", team.team_slug));
            continue;
        }
        lines.push(format!(
            "{}: {} candidates; {} rejected; {} review; 0 published",
            team.team_slug,
            team.terms.len(),
            team.rejected_count(),
            team.review_count()
        ));
        for (reason, count) in team.reason_counts() {
            lines.push(format!("  {reason}: {count}"));
        }
        if include_terms {
            for item in &team.terms {
                lines.push(format!(
                    "  [{}] {}: {}",
                    item.disposition, item.term, item.reason
                ));
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}
