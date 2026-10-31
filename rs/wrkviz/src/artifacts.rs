//! Deterministic, evidence-backed work-output artifact
//! extraction: `py/wrkviz/artifacts.py` at parity.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::team_data::{TeamData, ToolCall};

/// The extractor version stamped into every catalog.
pub const EXTRACTOR_VERSION: &str = "work-artifacts-v1";

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_id(prefix: &str, material: &str) -> String {
    format!("{prefix}{}", &sha256_hex(material.as_bytes())[..20])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Commit,
    PullRequest,
    MergeRequest,
    Diff,
    Issue,
    Task,
    Repository,
    Gist,
    Paste,
    UploadedFile,
    BuildArtifact,
    Url,
}

impl Kind {
    fn value(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::PullRequest => "pull_request",
            Kind::MergeRequest => "merge_request",
            Kind::Diff => "diff",
            Kind::Issue => "issue",
            Kind::Task => "task",
            Kind::Repository => "repository",
            Kind::Gist => "gist",
            Kind::Paste => "paste",
            Kind::UploadedFile => "uploaded_file",
            Kind::BuildArtifact => "build_artifact",
            Kind::Url => "url",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Relation {
    Produced,
    Published,
    Updated,
    Referenced,
}

impl Relation {
    fn value(self) -> &'static str {
        match self {
            Relation::Produced => "produced",
            Relation::Published => "published",
            Relation::Updated => "updated",
            Relation::Referenced => "referenced",
        }
    }
}

#[derive(Clone, Debug)]
struct Evidence {
    evidence_id: String,
    source_kind: String,
    source_id: String,
    source_line: i64,
    thread_id: String,
    turn_id: Option<String>,
    timestamp_ms: i64,
    relation: Relation,
    action: String,
    confidence: &'static str,
    matched_text: String,
}

impl Evidence {
    fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "evidence_id".into(),
            Value::String(self.evidence_id.clone()),
        );
        out.insert(
            "source_kind".into(),
            Value::String(self.source_kind.clone()),
        );
        out.insert("source_id".into(), Value::String(self.source_id.clone()));
        out.insert("source_line".into(), self.source_line.into());
        out.insert("thread_id".into(), Value::String(self.thread_id.clone()));
        out.insert(
            "turn_id".into(),
            self.turn_id.clone().map_or(Value::Null, Value::String),
        );
        out.insert("timestamp_ms".into(), self.timestamp_ms.into());
        out.insert(
            "relation".into(),
            Value::String(self.relation.value().to_string()),
        );
        out.insert("action".into(), Value::String(self.action.clone()));
        out.insert(
            "confidence".into(),
            Value::String(self.confidence.to_string()),
        );
        out.insert(
            "matched_text".into(),
            Value::String(self.matched_text.clone()),
        );
        out.insert(
            "extractor".into(),
            Value::String(EXTRACTOR_VERSION.to_string()),
        );
        Value::Object(out)
    }
}

/// One deduplicated work output or external reference.
#[derive(Clone, Debug)]
pub struct WorkArtifact {
    /// Content-derived id.
    pub artifact_id: String,
    /// Kind value.
    pub kind: String,
    /// Locator.
    pub locator: String,
    /// Link URL when proven.
    pub url: Option<String>,
    /// Label.
    pub label: String,
    /// Title.
    pub title: Option<String>,
    /// External id.
    pub external_id: Option<String>,
    /// Project URL.
    pub project_url: Option<String>,
    /// Project slug.
    pub project_slug: Option<String>,
    /// Producer thread.
    pub producer_thread_id: Option<String>,
    /// Produced-at ms.
    pub produced_at_ms: Option<i64>,
    evidence: Vec<Evidence>,
}

impl WorkArtifact {
    /// JSON form.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "artifact_id".into(),
            Value::String(self.artifact_id.clone()),
        );
        out.insert("kind".into(), Value::String(self.kind.clone()));
        out.insert("locator".into(), Value::String(self.locator.clone()));
        out.insert(
            "url".into(),
            self.url.clone().map_or(Value::Null, Value::String),
        );
        out.insert("label".into(), Value::String(self.label.clone()));
        out.insert(
            "title".into(),
            self.title.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "external_id".into(),
            self.external_id.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "project_url".into(),
            self.project_url.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "project_slug".into(),
            self.project_slug.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "producer_thread_id".into(),
            self.producer_thread_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "produced_at_ms".into(),
            self.produced_at_ms.map_or(Value::Null, Into::into),
        );
        out.insert(
            "evidence".into(),
            Value::Array(self.evidence.iter().map(Evidence::to_json).collect()),
        );
        Value::Object(out)
    }
}

/// A repository/project identity derived from artifact URLs.
#[derive(Clone, Debug)]
pub struct ProjectIdentity {
    /// Content-derived id.
    pub project_id: String,
    /// Host.
    pub host: String,
    /// Slug.
    pub slug: String,
    /// URL.
    pub url: String,
    /// Evidence ids.
    pub evidence_ids: Vec<String>,
}

impl ProjectIdentity {
    /// JSON form.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("project_id".into(), Value::String(self.project_id.clone()));
        out.insert("host".into(), Value::String(self.host.clone()));
        out.insert("slug".into(), Value::String(self.slug.clone()));
        out.insert("url".into(), Value::String(self.url.clone()));
        out.insert(
            "evidence_ids".into(),
            Value::Array(
                self.evidence_ids
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

/// Evidence-based interval index over one catalog (render-time).
pub struct ArtifactRangeIndex {
    all: Vec<(i64, String, bool)>,
    by_thread: std::collections::HashMap<String, Vec<(i64, String, bool)>>,
}

impl ArtifactRangeIndex {
    /// Build the index from a catalog's evidence records.
    pub fn from_catalog(catalog: &ArtifactCatalog) -> Self {
        let mut all: Vec<(i64, String, bool)> = Vec::new();
        let mut by_thread: std::collections::HashMap<String, Vec<(i64, String, bool)>> =
            std::collections::HashMap::new();
        for artifact in &catalog.artifacts {
            for evidence in &artifact.evidence {
                let output = matches!(
                    evidence.relation,
                    Relation::Produced | Relation::Published | Relation::Updated
                );
                let entry = (evidence.timestamp_ms, artifact.artifact_id.clone(), output);
                all.push(entry.clone());
                by_thread
                    .entry(evidence.thread_id.clone())
                    .or_default()
                    .push(entry);
            }
        }
        all.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        for entries in by_thread.values_mut() {
            entries.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        }
        Self { all, by_thread }
    }

    /// Deduplicated sorted artifact ids in the half-open interval.
    pub fn ids_for_range(
        &self,
        start_ms: i64,
        end_ms: i64,
        thread_id: Option<&str>,
        outputs_only: bool,
    ) -> Vec<String> {
        if end_ms <= start_ms {
            return Vec::new();
        }
        let series: &[(i64, String, bool)] = match thread_id {
            None => &self.all,
            Some(thread) => match self.by_thread.get(thread) {
                Some(entries) => entries,
                None => return Vec::new(),
            },
        };
        let left = series.partition_point(|entry| entry.0 < start_ms);
        let right = series.partition_point(|entry| entry.0 < end_ms);
        let mut ids: Vec<String> = series[left..right]
            .iter()
            .filter(|entry| !outputs_only || entry.2)
            .map(|entry| entry.1.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// The versioned artifact extraction result for one team.
#[derive(Clone, Debug)]
pub struct ArtifactCatalog {
    /// Source digest.
    pub source_digest: String,
    /// Artifacts.
    pub artifacts: Vec<WorkArtifact>,
    /// Projects.
    pub projects: Vec<ProjectIdentity>,
}

impl ArtifactCatalog {
    /// Strictly load a catalog, rejecting unstable identifiers.
    /// Mirrors `artifact_catalog_from_json`.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        use crate::json::{as_array, as_int, as_object, as_string};
        fn optional_string(value: Option<&Value>, where_: &str) -> Result<Option<String>, String> {
            match value {
                None | Some(Value::Null) => Ok(None),
                Some(other) => Ok(Some(as_string(other, where_)?.to_string())),
            }
        }
        let root = as_object(value, "artifact catalog")?;
        let schema_version = as_int(
            root.get("schema_version").unwrap_or(&Value::Null),
            "artifact catalog.schema_version",
        )?;
        if schema_version != 1 {
            return Err(format!(
                "unsupported artifact catalog schema {schema_version}"
            ));
        }
        let _extractor_version = as_string(
            root.get("extractor_version").unwrap_or(&Value::Null),
            "artifact catalog.extractor_version",
        )?;
        let mut artifacts = Vec::new();
        for (index, raw_artifact) in as_array(
            root.get("artifacts").unwrap_or(&Value::Null),
            "artifact catalog.artifacts",
        )?
        .iter()
        .enumerate()
        {
            let where_ = format!("artifact catalog.artifacts[{index}]");
            let item = as_object(raw_artifact, &where_)?;
            let mut evidence = Vec::new();
            for (evidence_index, raw_evidence) in as_array(
                item.get("evidence").unwrap_or(&Value::Null),
                &format!("{where_}.evidence"),
            )?
            .iter()
            .enumerate()
            {
                let evidence_where = format!("{where_}.evidence[{evidence_index}]");
                let evidence_item = as_object(raw_evidence, &evidence_where)?;
                let relation_raw = as_string(
                    evidence_item.get("relation").unwrap_or(&Value::Null),
                    &format!("{evidence_where}.relation"),
                )?;
                let relation = match relation_raw {
                    "produced" => Relation::Produced,
                    "published" => Relation::Published,
                    "updated" => Relation::Updated,
                    "referenced" => Relation::Referenced,
                    other => {
                        return Err(format!("'{other}' is not a valid EvidenceRelation"));
                    }
                };
                let confidence_raw = as_string(
                    evidence_item.get("confidence").unwrap_or(&Value::Null),
                    &format!("{evidence_where}.confidence"),
                )?;
                let confidence: &'static str = match confidence_raw {
                    "high" => "high",
                    "medium" => "medium",
                    other => {
                        return Err(format!("'{other}' is not a valid EvidenceConfidence"));
                    }
                };
                evidence.push(Evidence {
                    evidence_id: as_string(
                        evidence_item.get("evidence_id").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.evidence_id"),
                    )?
                    .to_string(),
                    source_kind: as_string(
                        evidence_item.get("source_kind").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.source_kind"),
                    )?
                    .to_string(),
                    source_id: as_string(
                        evidence_item.get("source_id").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.source_id"),
                    )?
                    .to_string(),
                    source_line: as_int(
                        evidence_item.get("source_line").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.source_line"),
                    )?,
                    thread_id: as_string(
                        evidence_item.get("thread_id").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.thread_id"),
                    )?
                    .to_string(),
                    turn_id: optional_string(
                        evidence_item.get("turn_id"),
                        &format!("{evidence_where}.turn_id"),
                    )?,
                    timestamp_ms: as_int(
                        evidence_item.get("timestamp_ms").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.timestamp_ms"),
                    )?,
                    relation,
                    action: as_string(
                        evidence_item.get("action").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.action"),
                    )?
                    .to_string(),
                    confidence,
                    matched_text: as_string(
                        evidence_item.get("matched_text").unwrap_or(&Value::Null),
                        &format!("{evidence_where}.matched_text"),
                    )?
                    .to_string(),
                });
                let _ = as_string(
                    evidence_item.get("extractor").unwrap_or(&Value::Null),
                    &format!("{evidence_where}.extractor"),
                )?;
            }
            let kind_raw = as_string(
                item.get("kind").unwrap_or(&Value::Null),
                &format!("{where_}.kind"),
            )?;
            let kind = match kind_raw {
                "commit" => Kind::Commit,
                "pull_request" => Kind::PullRequest,
                "merge_request" => Kind::MergeRequest,
                "diff" => Kind::Diff,
                "issue" => Kind::Issue,
                "task" => Kind::Task,
                "repository" => Kind::Repository,
                "gist" => Kind::Gist,
                "paste" => Kind::Paste,
                "uploaded_file" => Kind::UploadedFile,
                "build_artifact" => Kind::BuildArtifact,
                "url" => Kind::Url,
                other => {
                    return Err(format!("'{other}' is not a valid ArtifactKind"));
                }
            };
            let locator = as_string(
                item.get("locator").unwrap_or(&Value::Null),
                &format!("{where_}.locator"),
            )?
            .to_string();
            let identity = UrlIdentity {
                kind,
                url: locator.clone(),
                label: as_string(
                    item.get("label").unwrap_or(&Value::Null),
                    &format!("{where_}.label"),
                )?
                .to_string(),
                external_id: optional_string(
                    item.get("external_id"),
                    &format!("{where_}.external_id"),
                )?,
                project_url: optional_string(
                    item.get("project_url"),
                    &format!("{where_}.project_url"),
                )?,
                project_slug: optional_string(
                    item.get("project_slug"),
                    &format!("{where_}.project_slug"),
                )?,
            };
            let mut evidence_ids = std::collections::HashSet::new();
            for record in &evidence {
                if !evidence_ids.insert(record.evidence_id.clone()) {
                    return Err(format!("{where_}: duplicate evidence IDs"));
                }
            }
            for record in &evidence {
                let material = [
                    record.source_kind.as_str(),
                    record.source_id.as_str(),
                    &record.source_line.to_string(),
                    identity.kind.value(),
                    identity.url.as_str(),
                    record.relation.value(),
                    record.action.as_str(),
                ]
                .join("\0");
                if record.evidence_id != hash_id("evidence-", &material) {
                    return Err(format!(
                        "{where_}: evidence_id does not match its source and artifact"
                    ));
                }
            }
            let recorded_id = as_string(
                item.get("artifact_id").unwrap_or(&Value::Null),
                &format!("{where_}.artifact_id"),
            )?
            .to_string();
            if recorded_id != artifact_id(&identity) {
                return Err(format!("{where_}: artifact_id does not match kind and URL"));
            }
            let link_url = optional_string(item.get("url"), &format!("{where_}.url"))?;
            let allowed: Vec<Option<&str>> = if kind == Kind::Commit && !locator.starts_with("urn:")
            {
                vec![None, Some(locator.as_str())]
            } else if locator.starts_with("urn:") {
                vec![None]
            } else {
                vec![Some(locator.as_str())]
            };
            if !allowed.contains(&link_url.as_deref()) {
                return Err(format!("{where_}: URL does not match its stable locator"));
            }
            artifacts.push(WorkArtifact {
                artifact_id: recorded_id,
                kind: kind_raw.to_string(),
                locator: identity.url.clone(),
                url: link_url,
                label: identity.label.clone(),
                title: optional_string(item.get("title"), &format!("{where_}.title"))?,
                external_id: identity.external_id.clone(),
                project_url: identity.project_url.clone(),
                project_slug: identity.project_slug.clone(),
                producer_thread_id: optional_string(
                    item.get("producer_thread_id"),
                    &format!("{where_}.producer_thread_id"),
                )?,
                produced_at_ms: match item.get("produced_at_ms") {
                    None | Some(Value::Null) => None,
                    Some(other) => Some(as_int(other, &format!("{where_}.produced_at_ms"))?),
                },
                evidence,
            });
        }
        let mut projects = Vec::new();
        for (index, raw_project) in as_array(
            root.get("projects").unwrap_or(&Value::Null),
            "artifact catalog.projects",
        )?
        .iter()
        .enumerate()
        {
            let where_ = format!("artifact catalog.projects[{index}]");
            let item = as_object(raw_project, &where_)?;
            let url = as_string(
                item.get("url").unwrap_or(&Value::Null),
                &format!("{where_}.url"),
            )?
            .to_string();
            let project_id = as_string(
                item.get("project_id").unwrap_or(&Value::Null),
                &format!("{where_}.project_id"),
            )?
            .to_string();
            if project_id != hash_id("project-", &url) {
                return Err(format!("{where_}: project_id does not match URL"));
            }
            projects.push(ProjectIdentity {
                project_id,
                host: as_string(
                    item.get("host").unwrap_or(&Value::Null),
                    &format!("{where_}.host"),
                )?
                .to_string(),
                slug: as_string(
                    item.get("slug").unwrap_or(&Value::Null),
                    &format!("{where_}.slug"),
                )?
                .to_string(),
                url,
                evidence_ids: as_array(
                    item.get("evidence_ids").unwrap_or(&Value::Null),
                    &format!("{where_}.evidence_ids"),
                )?
                .iter()
                .map(|raw_id| {
                    as_string(raw_id, &format!("{where_}.evidence_ids[]")).map(str::to_string)
                })
                .collect::<Result<Vec<_>, _>>()?,
            });
        }
        Ok(Self {
            source_digest: as_string(
                root.get("source_digest").unwrap_or(&Value::Null),
                "artifact catalog.source_digest",
            )?
            .to_string(),
            artifacts,
            projects,
        })
    }

    /// The complete deterministic catalog JSON.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("schema_version".into(), 1.into());
        out.insert(
            "extractor_version".into(),
            Value::String(EXTRACTOR_VERSION.to_string()),
        );
        out.insert(
            "source_digest".into(),
            Value::String(self.source_digest.clone()),
        );
        out.insert(
            "artifacts".into(),
            Value::Array(self.artifacts.iter().map(WorkArtifact::to_json).collect()),
        );
        out.insert(
            "projects".into(),
            Value::Array(self.projects.iter().map(ProjectIdentity::to_json).collect()),
        );
        Value::Object(out)
    }
}

#[derive(Clone, Debug)]
struct UrlIdentity {
    kind: Kind,
    url: String,
    label: String,
    external_id: Option<String>,
    project_url: Option<String>,
    project_slug: Option<String>,
}

#[derive(Clone)]
struct Source {
    source_kind: &'static str,
    source_id: String,
    source_line: i64,
    thread_id: String,
    turn_id: Option<String>,
    timestamp_ms: i64,
    text: String,
    tool: Option<ToolCall>,
}

struct Builder {
    identity: UrlIdentity,
    title: Option<String>,
    evidence: BTreeMap<String, Evidence>,
}

fn safe_match(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(256)
        .collect()
}

fn property(text: &str, key: &str) -> Option<String> {
    let pattern = format!(
        r#"["']?{}["']?\s*:\s*("(?:\\.|[^"\\])*")"#,
        regex::escape(key)
    );
    let regex = Regex::new(&pattern).ok()?;
    let captured = regex.captures(text)?.get(1)?.as_str().to_string();
    let value: Value = serde_json::from_str(&captured).ok()?;
    value.as_str().map(str::to_string)
}

fn json_object(text: &str) -> Map<String, Value> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Map::new();
    };
    value.as_object().cloned().unwrap_or_default()
}

fn tool_command(tool: &ToolCall) -> String {
    let text = tool.input_text.clone().unwrap_or_default();
    let object = json_object(&text);
    for key in ["cmd", "command", "script"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            return value.to_string();
        }
    }
    for key in ["cmd", "command", "script"] {
        if let Some(value) = property(&text, key) {
            return value;
        }
    }
    text
}

fn flatten_output(text: Option<&str>) -> String {
    let Some(text) = text else {
        return String::new();
    };
    let Ok(raw) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    fn visit(value: &Value, fragments: &mut Vec<String>) {
        match value {
            Value::String(text) => fragments.push(text.clone()),
            Value::Array(items) => {
                for item in items {
                    visit(item, fragments);
                }
            }
            Value::Object(map) => {
                for key in ["text", "output"] {
                    if let Some(item) = map.get(key) {
                        visit(item, fragments);
                    }
                }
            }
            _ => {}
        }
    }
    let mut fragments = Vec::new();
    visit(&raw, &mut fragments);
    if fragments.is_empty() {
        text.to_string()
    } else {
        fragments.join("\n")
    }
}

/// POSIX shlex with punctuation `;&|` (newline is punctuation
/// too, since whitespace is only space/tab/CR). A `#` at a token
/// boundary starts a comment.
fn shell_segments(command: &str) -> Vec<Vec<String>> {
    let Some(tokens) = shell_tokens(command) else {
        return Vec::new();
    };
    let mut result: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for token in tokens {
        if !token.is_empty() && token.chars().all(|c| matches!(c, ';' | '|' | '&' | '\n')) {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
        } else {
            current.push(token);
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn shell_tokens(command: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut index = 0;
    let mut at_boundary = true;
    while index < chars.len() {
        let c = chars[index];
        match c {
            ' ' | '\t' | '\r' => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
                at_boundary = true;
                index += 1;
            }
            '\n' | ';' | '|' | '&' => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
                let mut run = String::new();
                while index < chars.len() && matches!(chars[index], '\n' | ';' | '|' | '&') {
                    run.push(chars[index]);
                    index += 1;
                }
                tokens.push(run);
                at_boundary = true;
            }
            '#' if at_boundary && !has_token => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '\'' => {
                has_token = true;
                at_boundary = false;
                index += 1;
                let mut closed = false;
                while index < chars.len() {
                    if chars[index] == '\'' {
                        closed = true;
                        index += 1;
                        break;
                    }
                    current.push(chars[index]);
                    index += 1;
                }
                if !closed {
                    return None;
                }
            }
            '"' => {
                has_token = true;
                at_boundary = false;
                index += 1;
                let mut closed = false;
                while index < chars.len() {
                    match chars[index] {
                        '"' => {
                            closed = true;
                            index += 1;
                            break;
                        }
                        '\\' if index + 1 < chars.len()
                            && matches!(chars[index + 1], '$' | '`' | '"' | '\\' | '\n') =>
                        {
                            if chars[index + 1] != '\n' {
                                current.push(chars[index + 1]);
                            }
                            index += 2;
                        }
                        '\\' => {
                            current.push('\\');
                            index += 1;
                        }
                        other => {
                            current.push(other);
                            index += 1;
                        }
                    }
                }
                if !closed {
                    return None;
                }
            }
            '\\' => {
                has_token = true;
                at_boundary = false;
                if index + 1 < chars.len() {
                    if chars[index + 1] != '\n' {
                        current.push(chars[index + 1]);
                    }
                    index += 2;
                } else {
                    index += 1;
                }
            }
            other => {
                has_token = true;
                at_boundary = false;
                current.push(other);
                index += 1;
            }
        }
    }
    if has_token {
        tokens.push(current);
    }
    Some(tokens)
}

fn executable_tokens(segment: &[String]) -> Vec<String> {
    let mut tokens: Vec<String> = segment.to_vec();
    while !tokens.is_empty()
        && tokens[0].contains('=')
        && !tokens[0].starts_with('-')
        && !tokens[0].starts_with('/')
    {
        tokens.remove(0);
    }
    while !tokens.is_empty()
        && ["env", "command", "sudo", "with-proxy", "timeout"].contains(&tokens[0].as_str())
    {
        let wrapper = tokens.remove(0);
        if wrapper == "env" {
            while !tokens.is_empty() && (tokens[0].starts_with('-') || tokens[0].contains('=')) {
                let option = tokens.remove(0);
                if (option == "-u" || option == "--unset") && !tokens.is_empty() {
                    tokens.remove(0);
                }
            }
        } else if wrapper == "timeout" {
            if !tokens.is_empty() {
                tokens.remove(0);
            }
        } else {
            while !tokens.is_empty() && tokens[0].starts_with('-') {
                tokens.remove(0);
            }
        }
    }
    tokens
}

fn command_has(tool: &ToolCall, executable: &str, arguments: &[&str]) -> bool {
    let command = tool_command(tool);
    let stripped: String = command
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | '\\'))
        .collect();
    if !stripped.contains(executable) {
        return false;
    }
    let desired: Vec<&str> = std::iter::once(executable)
        .chain(arguments.iter().copied())
        .collect();
    for raw_segment in shell_segments(&command) {
        let segment = executable_tokens(&raw_segment);
        if segment.len() >= desired.len()
            && segment[..desired.len()]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                == desired
        {
            return true;
        }
        if executable == "git" && segment.first().map(String::as_str) == Some("git") {
            let mut index = 1;
            while index < segment.len() && segment[index].starts_with('-') {
                if segment[index] == "-C" && index + 1 < segment.len() {
                    index += 2;
                } else {
                    index += 1;
                }
            }
            if segment.len() >= index + arguments.len()
                && segment[index..index + arguments.len()]
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    == arguments
            {
                return true;
            }
        }
        if matches!(
            segment.first().map(String::as_str),
            Some("bash") | Some("sh")
        ) {
            for (index, token) in segment.iter().enumerate() {
                if (token == "-c" || token == "-lc") && index + 1 < segment.len() {
                    let nested = ToolCall {
                        input_text: Some(segment[index + 1].clone()),
                        ..tool.clone()
                    };
                    if command_has(&nested, executable, arguments) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn command_has_option(tool: &ToolCall, executable: &str, options: &[&str]) -> bool {
    for raw_segment in shell_segments(&tool_command(tool)) {
        let segment = executable_tokens(&raw_segment);
        if segment.first().map(String::as_str) == Some(executable)
            && segment[1..].iter().any(|t| options.contains(&t.as_str()))
        {
            return true;
        }
    }
    false
}

fn command_title(tool: &ToolCall) -> Option<String> {
    for raw_segment in shell_segments(&tool_command(tool)) {
        let segment = executable_tokens(&raw_segment);
        if !matches!(
            segment.first().map(String::as_str),
            Some("gh") | Some("glab")
        ) {
            continue;
        }
        for (index, token) in segment.iter().enumerate().skip(1) {
            let mut value: Option<String> = None;
            if (token == "--title" || token == "-t") && index + 1 < segment.len() {
                value = Some(segment[index + 1].clone());
            } else if let Some(rest) = token.strip_prefix("--title=") {
                value = Some(rest.to_string());
            }
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                let compact: String = value
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(300)
                    .collect();
                if !compact.is_empty() {
                    return Some(compact);
                }
                return None;
            }
        }
    }
    None
}

fn tool_workdir(tool: &ToolCall) -> Option<String> {
    let text = tool.input_text.clone().unwrap_or_default();
    let object = json_object(&text);
    for key in ["workdir", "cwd"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            if value.starts_with('/') {
                let trimmed = value.trim_end_matches('/');
                return Some(if trimmed.is_empty() {
                    "/".into()
                } else {
                    trimmed.into()
                });
            }
        }
    }
    for key in ["workdir", "cwd"] {
        if let Some(value) = property(&text, key) {
            if value.starts_with('/') {
                let trimmed = value.trim_end_matches('/');
                return Some(if trimmed.is_empty() {
                    "/".into()
                } else {
                    trimmed.into()
                });
            }
        }
    }
    None
}

fn trim_url(raw: &str) -> String {
    let mut result = raw
        .trim_end_matches(|c| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '`' | '*' | '_'
            )
        })
        .to_string();
    for suffix in [r"\n", r"\r", r"\t"] {
        if let Some(stripped) = result.strip_suffix(suffix) {
            result = stripped.to_string();
        }
    }
    result
        .trim_end_matches(|c| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '`' | '*' | '_'
            )
        })
        .to_string()
}

struct SplitUrl {
    scheme: String,
    hostname: Option<String>,
    port: Option<u16>,
    port_invalid: bool,
    has_userinfo: bool,
    path: String,
}

fn split_url(value: &str) -> Option<SplitUrl> {
    let (scheme, rest) = value.split_once("://")?;
    let authority_end = rest
        .find(|c| ['/', '?', '#'].contains(&c))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let after = &rest[authority_end..];
    let path = if after.starts_with('/') {
        after
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_string()
    } else {
        String::new()
    };
    let has_userinfo = authority.contains('@');
    let hostport = authority.rsplit('@').next().unwrap_or_default();
    let (hostname, port, port_invalid) = if let Some(stripped) = hostport.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((host, tail)) => {
                let port_text = tail.strip_prefix(':');
                let (port, invalid) = match port_text {
                    None | Some("") => (None, false),
                    Some(text) => match text.parse::<u16>() {
                        Ok(port) => (Some(port), false),
                        Err(_) => (None, true),
                    },
                };
                (Some(host.to_string()), port, invalid)
            }
            None => (None, None, true),
        }
    } else if hostport.ends_with(':') {
        (
            Some(hostport.strip_suffix(':').unwrap_or(hostport).to_string()),
            None,
            false,
        )
    } else {
        match hostport.rsplit_once(':') {
            Some((host, text)) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
                match text.parse::<u16>() {
                    Ok(port) => (Some(host.to_string()), Some(port), false),
                    Err(_) => (Some(host.to_string()), None, true),
                }
            }
            Some((host, text)) if !text.is_empty() => (Some(host.to_string()), None, true),
            _ => (Some(hostport.to_string()), None, false),
        }
    };
    Some(SplitUrl {
        scheme: scheme.to_lowercase(),
        hostname: hostname.map(|host| host.to_lowercase().trim_end_matches('.').to_string()),
        port,
        port_invalid,
        has_userinfo,
        path,
    })
}

fn normalized_http(raw: &str) -> Option<String> {
    let candidate = trim_url(raw);
    let parts = split_url(&candidate)?;
    if parts.scheme != "http" && parts.scheme != "https" {
        return None;
    }
    let host = parts.hostname.filter(|h| !h.is_empty())?;
    if parts.has_userinfo {
        return None;
    }
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return None;
    }
    if parts.port_invalid {
        return None;
    }
    let netloc = match parts.port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    let raw_path = if parts.path.is_empty() {
        "/".to_string()
    } else {
        parts.path.clone()
    };
    let mut path = String::new();
    let mut last_slash = false;
    for c in raw_path.chars() {
        if c == '/' {
            if last_slash {
                continue;
            }
            last_slash = true;
        } else {
            last_slash = false;
        }
        path.push(c);
    }
    Some(format!("https://{netloc}{path}"))
}

fn repository_parts(host: &str, path_parts: &[String]) -> Option<(String, String)> {
    if host == "github.com" && path_parts.len() >= 2 {
        let owner_ok = {
            let owner = &path_parts[0];
            let bytes = owner.as_bytes();
            !owner.is_empty()
                && owner.len() <= 39
                && bytes[0].is_ascii_alphanumeric()
                && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        };
        let repo_ok = {
            let repo = &path_parts[1];
            !repo.is_empty()
                && repo.len() <= 100
                && repo
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
                && repo != "."
                && repo != ".."
        };
        if !owner_ok || !repo_ok {
            return None;
        }
        let slug = format!("{}/{}", path_parts[0], path_parts[1]);
        return Some((slug.clone(), format!("https://github.com/{slug}")));
    }
    if host.ends_with("gitlab.com") {
        if let Some(split) = path_parts.iter().position(|part| part == "-") {
            if split >= 2 {
                let slug = path_parts[..split].join("/");
                return Some((slug.clone(), format!("https://{host}/{slug}")));
            }
        }
        if path_parts.len() >= 2 {
            let slug = format!("{}/{}", path_parts[0], path_parts[1]);
            return Some((slug.clone(), format!("https://{host}/{slug}")));
        }
    }
    None
}

fn is_py_digit(text: &str) -> bool {
    // CPython `str.isdigit`: Unicode digit property.
    !text.is_empty() && text.chars().all(|c| c.is_numeric())
}

fn is_hex(text: &str, min: usize, max: usize) -> bool {
    text.len() >= min && text.len() <= max && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn url_identity(raw: &str) -> Option<UrlIdentity> {
    let normalized = normalized_http(raw)?;
    let parts = split_url(&normalized)?;
    let host = parts.hostname.unwrap_or_default();
    let mut path_parts: Vec<String> = parts
        .path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if host == "github.com" && path_parts.len() >= 2 {
        path_parts[1] = path_parts[1]
            .strip_suffix(".git")
            .unwrap_or(&path_parts[1])
            .to_string();
    }
    if host == "gist.github.com" && path_parts.len() >= 2 {
        let base = format!(
            "https://gist.github.com/{}/{}",
            path_parts[0], path_parts[1]
        );
        return Some(UrlIdentity {
            kind: Kind::Gist,
            url: base,
            label: format!("Gist {}", path_parts[1]),
            external_id: Some(path_parts[1].clone()),
            project_url: None,
            project_slug: None,
        });
    }
    let repository = repository_parts(&host, &path_parts);
    let project_slug = repository.as_ref().map(|(slug, _)| slug.clone());
    let project_url = repository.as_ref().map(|(_, url)| url.clone());
    if host == "github.com" {
        if let Some((slug, url)) = &repository {
            let tail = &path_parts[2..];
            if tail.is_empty() {
                return Some(UrlIdentity {
                    kind: Kind::Repository,
                    url: url.clone(),
                    label: slug.clone(),
                    external_id: Some(slug.clone()),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 2 && tail[0] == "pull" && is_py_digit(&tail[1]) {
                let number = tail[1].clone();
                return Some(UrlIdentity {
                    kind: Kind::PullRequest,
                    url: format!("{url}/pull/{number}"),
                    label: format!("{slug} PR #{number}"),
                    external_id: Some(number),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 2 && tail[0] == "issues" && is_py_digit(&tail[1]) {
                let number = tail[1].clone();
                return Some(UrlIdentity {
                    kind: Kind::Issue,
                    url: format!("{url}/issues/{number}"),
                    label: format!("{slug} issue #{number}"),
                    external_id: Some(number),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 2
                && (tail[0] == "commit" || tail[0] == "commits")
                && is_hex(&tail[1], 7, 40)
                && tail[1].len() >= 7
            {
                let revision = tail[1].to_lowercase();
                return Some(UrlIdentity {
                    kind: Kind::Commit,
                    url: format!("{url}/commit/{revision}"),
                    label: format!("{slug} commit {}", &revision[..12.min(revision.len())]),
                    external_id: Some(revision),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 4
                && tail[0] == "actions"
                && tail[1] == "runs"
                && tail.contains(&"artifacts".to_string())
            {
                return Some(UrlIdentity {
                    kind: Kind::BuildArtifact,
                    url: normalized.clone(),
                    label: format!("{slug} build artifact"),
                    external_id: Some(tail[tail.len() - 1].clone()),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 3 && tail[0] == "releases" && tail[1] == "download" {
                return Some(UrlIdentity {
                    kind: Kind::UploadedFile,
                    url: normalized.clone(),
                    label: tail[tail.len() - 1].clone(),
                    external_id: Some(tail[tail.len() - 1].clone()),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
        }
    }
    if host.ends_with("gitlab.com") {
        if let Some((slug, url)) = &repository {
            let marker = path_parts.iter().position(|part| part == "-");
            let tail: &[String] = match marker {
                Some(index) => &path_parts[index + 1..],
                None => &[],
            };
            if tail.len() >= 2 && tail[0] == "merge_requests" && is_py_digit(&tail[1]) {
                let number = tail[1].clone();
                return Some(UrlIdentity {
                    kind: Kind::MergeRequest,
                    url: format!("{url}/-/merge_requests/{number}"),
                    label: format!("{slug} MR !{number}"),
                    external_id: Some(number),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 2 && tail[0] == "issues" && is_py_digit(&tail[1]) {
                let number = tail[1].clone();
                return Some(UrlIdentity {
                    kind: Kind::Issue,
                    url: format!("{url}/-/issues/{number}"),
                    label: format!("{slug} issue #{number}"),
                    external_id: Some(number),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
            if tail.len() >= 2 && tail[0] == "commit" && is_hex(&tail[1], 7, 40) {
                let revision = tail[1].to_lowercase();
                return Some(UrlIdentity {
                    kind: Kind::Commit,
                    url: format!("{url}/-/commit/{revision}"),
                    label: format!("{slug} commit {}", &revision[..12.min(revision.len())]),
                    external_id: Some(revision),
                    project_url: Some(url.clone()),
                    project_slug: Some(slug.clone()),
                });
            }
        }
    }
    let last = path_parts.last().cloned().unwrap_or_default();
    if Regex::new(r"^D[1-9][0-9]*$").unwrap().is_match(&last) {
        return Some(UrlIdentity {
            kind: Kind::Diff,
            url: normalized.clone(),
            label: format!("Diff {last}"),
            external_id: Some(last),
            project_url: None,
            project_slug: None,
        });
    }
    if Regex::new(r"^T[1-9][0-9]*$").unwrap().is_match(&last) {
        return Some(UrlIdentity {
            kind: Kind::Task,
            url: normalized.clone(),
            label: format!("Task {last}"),
            external_id: Some(last),
            project_url: None,
            project_slug: None,
        });
    }
    if ["paste.rs", "pastebin.com", "pasty.lol", "dpaste.org"].contains(&host.as_str()) {
        let identifier = if path_parts.is_empty() {
            host.clone()
        } else {
            last.clone()
        };
        return Some(UrlIdentity {
            kind: Kind::Paste,
            url: normalized.clone(),
            label: format!("Paste {identifier}"),
            external_id: Some(identifier),
            project_url: None,
            project_slug: None,
        });
    }
    Some(UrlIdentity {
        kind: Kind::Url,
        url: normalized.clone(),
        label: normalized,
        external_id: None,
        project_url,
        project_slug,
    })
}

fn repository_identity(raw: &str) -> Option<UrlIdentity> {
    let value = raw.trim();
    // SSH remote: git@host:slug (slug may end .git).
    if let Some(found) = ssh_remote(value) {
        let normalized = normalized_http(&found)?;
        return identity_from_normalized(&normalized);
    }
    let normalized = normalized_http(value)?;
    identity_from_normalized(&normalized)
}

fn identity_from_normalized(normalized: &str) -> Option<UrlIdentity> {
    let parts = split_url(normalized)?;
    let host = parts.hostname.unwrap_or_default();
    let path = parts.path.strip_suffix(".git").unwrap_or(&parts.path);
    let path_parts: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    let (slug, url) = repository_parts(&host, &path_parts)?;
    Some(UrlIdentity {
        kind: Kind::Repository,
        url: url.clone(),
        label: slug.clone(),
        external_id: Some(slug.clone()),
        project_url: Some(url),
        project_slug: Some(slug),
    })
}

fn ssh_remote(value: &str) -> Option<String> {
    // (?<![A-Za-z0-9])git@host:slug(.git)?(?=$|[\s'"),;])
    let regex = Regex::new(r"git@([A-Za-z0-9.-]+):([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)").ok()?;
    for captured in regex.captures_iter(value) {
        let whole = captured.get(0)?;
        let start = whole.start();
        if start > 0 {
            let prev = value.as_bytes()[start - 1];
            if prev.is_ascii_alphanumeric() {
                continue;
            }
        }
        let end = whole.end();
        let rest = &value[end..];
        let slug_full = captured.get(2)?.as_str();
        let slug = slug_full.strip_suffix(".git").unwrap_or(slug_full);
        // The greedy slug may have consumed a trailing .git plus
        // boundary handling; recompute end from the matched text.
        let _ = rest;
        let host = captured.get(1)?.as_str().to_lowercase();
        return Some(format!("https://{host}/{slug}"));
    }
    None
}

/// Return a credential-free canonical repository URL, if
/// *raw* proves one.
pub fn canonical_repository_url(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    repository_identity(raw).map(|identity| identity.url)
}

fn artifact_key(identity: &UrlIdentity) -> String {
    format!("{}\0{}", identity.kind.value(), identity.url)
}

fn artifact_id(identity: &UrlIdentity) -> String {
    hash_id("artifact-", &artifact_key(identity))
}

fn evidence_id(
    source: &Source,
    identity: &UrlIdentity,
    relation: Relation,
    action: &str,
) -> String {
    let material = [
        source.source_kind,
        source.source_id.as_str(),
        &source.source_line.to_string(),
        identity.kind.value(),
        identity.url.as_str(),
        relation.value(),
        action,
    ]
    .join("\0");
    hash_id("evidence-", &material)
}

fn make_evidence(
    source: &Source,
    identity: &UrlIdentity,
    relation: Relation,
    action: &str,
    confidence: &'static str,
    matched_text: &str,
) -> Evidence {
    Evidence {
        evidence_id: evidence_id(source, identity, relation, action),
        source_kind: source.source_kind.to_string(),
        source_id: source.source_id.clone(),
        source_line: source.source_line,
        thread_id: source.thread_id.clone(),
        turn_id: source.turn_id.clone(),
        timestamp_ms: source.timestamp_ms,
        relation,
        action: action.to_string(),
        confidence,
        matched_text: safe_match(matched_text),
    }
}

fn add(
    builders: &mut BTreeMap<String, Builder>,
    identity: UrlIdentity,
    evidence: Evidence,
    title: Option<String>,
) {
    let key = artifact_key(&identity);
    let builder = builders.entry(key).or_insert_with(|| Builder {
        identity: identity.clone(),
        title: None,
        evidence: BTreeMap::new(),
    });
    if builder.title.is_none() {
        if let Some(title) = title {
            builder.title = Some(title);
        }
    }
    builder
        .evidence
        .insert(evidence.evidence_id.clone(), evidence);
}

fn successful(tool: &ToolCall) -> bool {
    if !["completed", "success", "succeeded"].contains(&tool.status.as_str()) {
        return false;
    }
    let output = flatten_output(tool.output_text.as_deref());
    let failures =
        Regex::new(r"(?i)(?:exit(?:_code)?|process exited with code)[=: ]+([0-9]+)").unwrap();
    let captures: Vec<String> = failures
        .captures_iter(&output)
        .map(|c| c[1].to_string())
        .collect();
    if !captures.is_empty() {
        return captures.iter().all(|value| value == "0");
    }
    !Regex::new(r"(?i)(?:^|\n)(?:fatal:|error:|failed(?:\s|:)|.*permission denied|.*no such file)")
        .unwrap()
        .is_match(&output)
}

fn push_succeeded(tool: &ToolCall) -> bool {
    if !successful(tool) {
        return false;
    }
    let output = flatten_output(tool.output_text.as_deref());
    Regex::new(r"(?:^|\n)To\s+(?:https?://|ssh://|git@|[^\s]+:[^\s]+)")
        .unwrap()
        .is_match(&output)
        && Regex::new(r"(?i)(?:->|\.\.|new branch|new tag|Everything up-to-date)")
            .unwrap()
            .is_match(&output)
}

fn url_action(source: &Source, identity: &UrlIdentity) -> (Relation, &'static str) {
    let Some(tool) = &source.tool else {
        return (Relation::Referenced, "mentioned");
    };
    let confirmed = source.source_kind == "tool_output" && successful(tool);
    if confirmed && identity.kind == Kind::PullRequest && command_has(tool, "gh", &["pr", "create"])
    {
        return (Relation::Produced, "created_pull_request");
    }
    if confirmed
        && identity.kind == Kind::MergeRequest
        && command_has(tool, "glab", &["mr", "create"])
    {
        return (Relation::Produced, "created_merge_request");
    }
    if confirmed
        && matches!(identity.kind, Kind::Issue | Kind::Task)
        && (command_has(tool, "gh", &["issue", "create"])
            || command_has(tool, "glab", &["issue", "create"]))
    {
        return (Relation::Produced, "created_issue");
    }
    if confirmed
        && identity.kind == Kind::Diff
        && (command_has(tool, "jf", &["submit"]) || command_has(tool, "arc", &["diff"]))
    {
        return (Relation::Produced, "created_diff");
    }
    if confirmed && identity.kind == Kind::Gist && command_has(tool, "gh", &["gist", "create"]) {
        return (Relation::Produced, "created_gist");
    }
    let upload = command_has(tool, "gh", &["release", "upload"])
        || command_has(tool, "aws", &["s3", "cp"])
        || command_has(tool, "rclone", &["copy"])
        || command_has(tool, "rclone", &["copyto"])
        || command_has_option(tool, "curl", &["-T", "--upload-file"]);
    if confirmed
        && matches!(
            identity.kind,
            Kind::Url | Kind::UploadedFile | Kind::BuildArtifact
        )
        && upload
    {
        return (Relation::Published, "uploaded");
    }
    if confirmed
        && identity.kind == Kind::PullRequest
        && ["edit", "merge", "ready", "close", "reopen"]
            .iter()
            .any(|action| command_has(tool, "gh", &["pr", action]))
    {
        return (Relation::Updated, "updated_pull_request");
    }
    (
        Relation::Referenced,
        if source.source_kind == "tool_output" {
            "observed"
        } else {
            "mentioned"
        },
    )
}

fn source_records(team: &TeamData) -> Vec<Source> {
    let mut records: Vec<Source> = Vec::new();
    let starts: BTreeMap<&str, i64> = team
        .agents
        .iter()
        .map(|agent| (agent.thread_id.as_str(), agent.started_at_ms))
        .collect();
    for snapshot in &team.sources {
        if let Some(repository_url) = &snapshot.repository_url {
            records.push(Source {
                source_kind: "source_metadata",
                source_id: snapshot.path.clone(),
                source_line: 1,
                thread_id: snapshot.thread_id.clone(),
                turn_id: None,
                timestamp_ms: starts
                    .get(snapshot.thread_id.as_str())
                    .copied()
                    .unwrap_or(0),
                text: repository_url.clone(),
                tool: None,
            });
        }
    }
    for event in &team.events {
        if let Some(text) = &event.text {
            if !text.is_empty() {
                records.push(Source {
                    source_kind: "event_text",
                    source_id: event.event_id.clone(),
                    source_line: event.source_line,
                    thread_id: event.thread_id.clone(),
                    turn_id: event.turn_id.clone(),
                    timestamp_ms: event.timestamp_ms,
                    text: text.clone(),
                    tool: None,
                });
            }
        }
    }
    for tool in &team.tool_calls {
        if let Some(text) = &tool.input_text {
            if !text.is_empty() {
                records.push(Source {
                    source_kind: "tool_input",
                    source_id: tool.call_id.clone(),
                    source_line: tool.source_line,
                    thread_id: tool.thread_id.clone(),
                    turn_id: tool.turn_id.clone(),
                    timestamp_ms: tool.started_at_ms,
                    text: text.clone(),
                    tool: Some(tool.clone()),
                });
            }
        }
        if let Some(text) = &tool.output_text {
            if !text.is_empty() {
                records.push(Source {
                    source_kind: "tool_output",
                    source_id: tool.call_id.clone(),
                    source_line: tool.source_line,
                    thread_id: tool.thread_id.clone(),
                    turn_id: tool.turn_id.clone(),
                    timestamp_ms: tool.ended_at_ms.unwrap_or(tool.started_at_ms),
                    text: flatten_output(Some(text)),
                    tool: Some(tool.clone()),
                });
            }
        }
    }
    records.sort_by(|a, b| {
        (a.timestamp_ms, a.source_kind, a.source_id.as_str()).cmp(&(
            b.timestamp_ms,
            b.source_kind,
            b.source_id.as_str(),
        ))
    });
    records
}

fn is_slug_pair(slug: &str) -> bool {
    let Some((owner, repo)) = slug.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !repo.is_empty()
        && !repo.contains('/')
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/'))
}

fn repo_from_slug(slug: &str) -> UrlIdentity {
    let url = format!("https://github.com/{slug}");
    UrlIdentity {
        kind: Kind::Repository,
        url: url.clone(),
        label: slug.to_string(),
        external_id: Some(slug.to_string()),
        project_url: Some(url),
        project_slug: Some(slug.to_string()),
    }
}

fn http_urls(text: &str) -> Vec<String> {
    let regex = Regex::new(r#"(?i)https?://[^\s<>"'`\\|${}…]+"#).unwrap();
    regex
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn source_repositories(source: &Source) -> Vec<UrlIdentity> {
    let mut found: BTreeMap<String, UrlIdentity> = BTreeMap::new();
    let mut inspect_urls = source.source_kind == "source_metadata";
    if let Some(tool) = &source.tool {
        if source.source_kind == "tool_output" {
            inspect_urls = command_has(tool, "git", &["remote", "get-url"])
                || command_has(tool, "git", &["remote", "-v"])
                || command_has(tool, "git", &["config", "--get", "remote.origin.url"]);
        }
    }
    if inspect_urls {
        for raw in http_urls(&source.text) {
            if let Some(identity) = repository_identity(&raw) {
                found.insert(identity.url.clone(), identity);
            }
        }
        // SSH remotes in the same text.
        let ssh = Regex::new(r"git@([A-Za-z0-9.-]+):([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)").unwrap();
        for captured in ssh.captures_iter(&source.text) {
            let host = captured[1].to_lowercase();
            let slug = captured[2]
                .strip_suffix(".git")
                .unwrap_or(&captured[2])
                .to_string();
            if let Some(identity) = repository_identity(&format!("https://{host}/{slug}")) {
                found.insert(identity.url.clone(), identity);
            }
        }
    }
    if let Some(tool) = &source.tool {
        if source.source_kind == "tool_input" {
            for raw_segment in shell_segments(&tool_command(tool)) {
                let segment = executable_tokens(&raw_segment);
                if !matches!(
                    segment.first().map(String::as_str),
                    Some("gh") | Some("glab")
                ) {
                    continue;
                }
                for (index, token) in segment.iter().enumerate().skip(1) {
                    let slug: Option<String> =
                        if (token == "-R" || token == "--repo") && index + 1 < segment.len() {
                            Some(segment[index + 1].clone())
                        } else {
                            token.strip_prefix("--repo=").map(str::to_string)
                        };
                    if let Some(slug) = slug {
                        if is_slug_pair(&slug) {
                            let identity = repo_from_slug(&slug);
                            found.insert(identity.url.clone(), identity);
                        }
                    }
                }
            }
        }
    }
    found.into_values().collect()
}

fn path_repository(
    workdir: Option<&str>,
    repositories_by_path: &BTreeMap<String, BTreeSet<String>>,
) -> Option<String> {
    let workdir = workdir?;
    let path_parts: Vec<&str> = workdir.split('/').collect();
    let mut candidates: Vec<(usize, String)> = Vec::new();
    for (base, urls) in repositories_by_path {
        let base_parts: Vec<&str> = base.split('/').collect();
        let is_prefix = base_parts.len() <= path_parts.len()
            && base_parts
                .iter()
                .zip(path_parts.iter())
                .all(|(a, b)| a == b);
        if workdir == base || is_prefix {
            for url in urls {
                candidates.push((base_parts.len(), url.clone()));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let max_depth = candidates.iter().map(|(depth, _)| *depth).max()?;
    let urls: BTreeSet<String> = candidates
        .into_iter()
        .filter(|(depth, _)| *depth == max_depth)
        .map(|(_, url)| url)
        .collect();
    if urls.len() == 1 {
        urls.into_iter().next()
    } else {
        None
    }
}

fn context_maps(
    records: &[Source],
    team: &TeamData,
) -> (
    BTreeMap<String, BTreeSet<String>>,
    BTreeMap<String, BTreeSet<String>>,
) {
    let mut by_path: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut by_thread: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for snapshot in &team.sources {
        let Some(repository_url) = &snapshot.repository_url else {
            continue;
        };
        let Some(identity) = repository_identity(repository_url) else {
            continue;
        };
        by_thread
            .entry(snapshot.thread_id.clone())
            .or_default()
            .insert(identity.url.clone());
        if let Some(workdir) = &snapshot.working_directory {
            let trimmed = workdir.trim_end_matches('/');
            let key = if trimmed.is_empty() { "/" } else { trimmed };
            by_path
                .entry(key.to_string())
                .or_default()
                .insert(identity.url);
        }
    }
    for source in records {
        let Some(tool) = &source.tool else {
            continue;
        };
        if source.source_kind != "tool_output" {
            continue;
        }
        if !(command_has(tool, "git", &["remote", "get-url"])
            || command_has(tool, "git", &["remote", "-v"])
            || command_has(tool, "git", &["config", "--get", "remote.origin.url"]))
        {
            continue;
        }
        let repositories = source_repositories(source);
        if repositories.is_empty() {
            continue;
        }
        let urls: BTreeSet<String> = repositories.iter().map(|item| item.url.clone()).collect();
        if let Some(workdir) = tool_workdir(tool) {
            if !by_path.contains_key(&workdir) && urls.len() == 1 {
                by_path.insert(workdir, urls);
            }
        }
    }
    (by_path, by_thread)
}

fn context_repository(
    source: &Source,
    by_path: &BTreeMap<String, BTreeSet<String>>,
    by_thread: &BTreeMap<String, BTreeSet<String>>,
) -> Option<String> {
    if let Some(tool) = &source.tool {
        if let Some(repository) = path_repository(tool_workdir(tool).as_deref(), by_path) {
            return Some(repository);
        }
    }
    let urls = by_thread.get(&source.thread_id)?;
    if urls.len() == 1 {
        urls.iter().next().cloned()
    } else {
        None
    }
}

fn commit_identity(repository_url: Option<&str>, revision: &str) -> UrlIdentity {
    let revision = revision.to_lowercase();
    let repository = repository_url.and_then(repository_identity);
    match repository {
        None => UrlIdentity {
            kind: Kind::Commit,
            url: format!("urn:git-commit:{revision}"),
            label: format!("Git commit {}", &revision[..12.min(revision.len())]),
            external_id: Some(revision),
            project_url: None,
            project_slug: None,
        },
        Some(repository) => UrlIdentity {
            kind: Kind::Commit,
            url: format!("{}/commit/{revision}", repository.url),
            label: format!(
                "{} commit {}",
                repository.project_slug.clone().unwrap_or_default(),
                &revision[..12.min(revision.len())]
            ),
            external_id: Some(revision),
            project_url: Some(repository.url.clone()),
            project_slug: repository.project_slug.clone(),
        },
    }
}

fn full_shas(text: &str) -> Vec<String> {
    // (?<![0-9a-f])([0-9a-f]{40})(?![0-9a-f]), case-insensitive.
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index + 40 <= bytes.len() {
        let window = &text[index..index + 40];
        if window.bytes().all(|b| b.is_ascii_hexdigit())
            && (index == 0 || !bytes[index - 1].is_ascii_hexdigit())
            && (index + 40 == bytes.len() || !bytes[index + 40].is_ascii_hexdigit())
        {
            found.push(window.to_string());
            index += 40;
        } else {
            index += 1;
        }
    }
    found
}

fn produced_commits(
    records: &[Source],
    builders: &mut BTreeMap<String, Builder>,
    by_path: &BTreeMap<String, BTreeSet<String>>,
    by_thread: &BTreeMap<String, BTreeSet<String>>,
) {
    let commit_output = Regex::new(r"(?mi)^\[([^\]\s]+) ([0-9a-f]{7,40})\] (.+)$").unwrap();
    let mut last_commit: BTreeMap<(String, Option<String>), UrlIdentity> = BTreeMap::new();
    for source in records {
        let Some(tool) = &source.tool else {
            continue;
        };
        if source.source_kind != "tool_output" {
            continue;
        }
        let workdir = tool_workdir(tool);
        let context_key = (source.thread_id.clone(), workdir);
        let repository_url = context_repository(source, by_path, by_thread);
        if command_has(tool, "git", &["commit"]) && successful(tool) {
            for captured in commit_output.captures_iter(&source.text) {
                let mut revision = captured[2].to_lowercase();
                let matches: BTreeSet<String> = full_shas(&source.text)
                    .iter()
                    .map(|value| value.to_lowercase())
                    .filter(|value| value.starts_with(&revision))
                    .collect();
                if matches.len() == 1 {
                    revision = matches.into_iter().next().unwrap();
                }
                let identity = commit_identity(repository_url.as_deref(), &revision);
                let title = captured[3].to_string();
                let evidence = make_evidence(
                    source,
                    &identity,
                    Relation::Produced,
                    "committed",
                    if repository_url.is_some() {
                        "high"
                    } else {
                        "medium"
                    },
                    &format!("{revision} {title}"),
                );
                add(
                    builders,
                    identity.clone(),
                    evidence,
                    Some(title.trim().to_string()),
                );
                last_commit.insert(context_key.clone(), identity);
            }
        }
        if command_has(tool, "git", &["push"]) && push_succeeded(tool) {
            let mut pushed = last_commit.get(&context_key).cloned();
            if pushed.is_none() {
                let full = full_shas(&source.text);
                let unique: BTreeSet<String> =
                    full.iter().map(|value| value.to_lowercase()).collect();
                if unique.len() == 1 {
                    pushed = Some(commit_identity(repository_url.as_deref(), &full[0]));
                }
            }
            if let Some(identity) = pushed {
                let matched = identity
                    .external_id
                    .clone()
                    .unwrap_or_else(|| identity.url.clone());
                let evidence = make_evidence(
                    source,
                    &identity,
                    Relation::Published,
                    "pushed",
                    if repository_url.is_some() {
                        "high"
                    } else {
                        "medium"
                    },
                    &matched,
                );
                add(builders, identity, evidence, None);
            }
        }
    }
}

fn contextual_references(source: &Source, repository_url: Option<&str>) -> Vec<UrlIdentity> {
    let mut found: BTreeMap<String, UrlIdentity> = BTreeMap::new();
    // Qualified: owner/repo#N with non-word boundary before.
    let qualified = Regex::new(r"([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)#([1-9][0-9]*)").unwrap();
    for captured in qualified.captures_iter(&source.text) {
        let whole = captured.get(0).unwrap();
        if whole.start() > 0 {
            let prev = source.text.as_bytes()[whole.start() - 1];
            if prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b'.' | b'/' | b'-') {
                continue;
            }
        }
        if captured.get(2).is_some_and(|m| {
            let after = &source.text[m.end()..];
            after.chars().next().is_some_and(|c| c.is_ascii_digit())
        }) {
            continue;
        }
        let repo = repo_from_slug(&captured[1]);
        let number = captured[2].to_string();
        let identity = UrlIdentity {
            kind: Kind::PullRequest,
            url: format!("{}/pull/{number}", repo.url),
            label: format!(
                "{} PR #{number}",
                repo.project_slug.clone().unwrap_or_default()
            ),
            external_id: Some(number),
            project_url: Some(repo.url.clone()),
            project_slug: repo.project_slug.clone(),
        };
        found.insert(artifact_key(&identity), identity);
    }
    if let Some(repository_url) = repository_url {
        if let Some(repository) = repository_identity(repository_url) {
            let slug = repository.project_slug.clone().unwrap_or_default();
            let pr = Regex::new(r"(?i)PR\s+#([1-9][0-9]*)").unwrap();
            for captured in pr.captures_iter(&source.text) {
                let whole = captured.get(0).unwrap();
                if whole.start() > 0 {
                    let prev = source.text.as_bytes()[whole.start() - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' {
                        continue;
                    }
                }
                let number = captured[1].to_string();
                let identity = UrlIdentity {
                    kind: Kind::PullRequest,
                    url: format!("{}/pull/{number}", repository.url),
                    label: format!("{slug} PR #{number}"),
                    external_id: Some(number),
                    project_url: Some(repository.url.clone()),
                    project_slug: repository.project_slug.clone(),
                };
                found.insert(artifact_key(&identity), identity);
            }
            let issue = Regex::new(r"(?i)(?:issue|task)\s+#([1-9][0-9]*)").unwrap();
            for captured in issue.captures_iter(&source.text) {
                let whole = captured.get(0).unwrap();
                if whole.start() > 0 {
                    let prev = source.text.as_bytes()[whole.start() - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' {
                        continue;
                    }
                }
                let number = captured[1].to_string();
                let identity = UrlIdentity {
                    kind: Kind::Issue,
                    url: format!("{}/issues/{number}", repository.url),
                    label: format!("{slug} issue #{number}"),
                    external_id: Some(number),
                    project_url: Some(repository.url.clone()),
                    project_slug: repository.project_slug.clone(),
                };
                found.insert(artifact_key(&identity), identity);
            }
        }
    }
    found.into_values().collect()
}

/// Extract a deterministic catalog from normalized events
/// and unredacted tool records.
pub fn extract_artifacts(team: &TeamData) -> ArtifactCatalog {
    let records = source_records(team);
    let (by_path, by_thread) = context_maps(&records, team);
    let mut builders: BTreeMap<String, Builder> = BTreeMap::new();
    for source in &records {
        for repository in source_repositories(source) {
            let evidence = make_evidence(
                source,
                &repository,
                Relation::Referenced,
                "repository_observed",
                "high",
                &repository.url.clone(),
            );
            add(&mut builders, repository, evidence, None);
        }
        let mut seen_urls: BTreeSet<String> = BTreeSet::new();
        for raw in http_urls(&source.text) {
            let Some(identity) = url_identity(&raw) else {
                continue;
            };
            if !seen_urls.insert(identity.url.clone()) {
                continue;
            }
            let (relation, action) = url_action(source, &identity);
            if identity.kind == Kind::Url
                && !matches!(relation, Relation::Produced | Relation::Published)
            {
                continue;
            }
            let evidence = make_evidence(
                source,
                &identity,
                relation,
                action,
                "high",
                &identity.url.clone(),
            );
            let title = if source.tool.is_some()
                && matches!(relation, Relation::Produced | Relation::Updated)
            {
                source.tool.as_ref().and_then(command_title)
            } else {
                None
            };
            add(&mut builders, identity.clone(), evidence, title);
            if relation != Relation::Referenced {
                if let Some(project_url) = &identity.project_url {
                    if let Some(project_identity) = repository_identity(project_url) {
                        let evidence = make_evidence(
                            source,
                            &project_identity,
                            Relation::Referenced,
                            "project_of_output",
                            "high",
                            &project_identity.url.clone(),
                        );
                        add(&mut builders, project_identity, evidence, None);
                    }
                }
            }
        }
        let context = context_repository(source, &by_path, &by_thread);
        for identity in contextual_references(source, context.as_deref()) {
            let evidence = make_evidence(
                source,
                &identity,
                Relation::Referenced,
                "mentioned",
                "medium",
                &identity.label.clone(),
            );
            add(&mut builders, identity, evidence, None);
        }
    }
    produced_commits(&records, &mut builders, &by_path, &by_thread);
    let mut artifacts: Vec<WorkArtifact> = Vec::new();
    for builder in builders.into_values() {
        let mut ordered: Vec<Evidence> = builder.evidence.into_values().collect();
        ordered.sort_by(|a, b| {
            (
                a.timestamp_ms,
                a.source_kind.as_str(),
                a.evidence_id.as_str(),
            )
                .cmp(&(
                    b.timestamp_ms,
                    b.source_kind.as_str(),
                    b.evidence_id.as_str(),
                ))
        });
        let produced: Vec<&Evidence> = ordered
            .iter()
            .filter(|item| matches!(item.relation, Relation::Produced | Relation::Published))
            .collect();
        let producer = produced.iter().min_by(|a, b| {
            (a.timestamp_ms, a.evidence_id.as_str()).cmp(&(b.timestamp_ms, b.evidence_id.as_str()))
        });
        let identity = builder.identity;
        let mut link_url: Option<String> = if identity.url.starts_with("urn:") {
            None
        } else {
            Some(identity.url.clone())
        };
        if identity.kind == Kind::Commit {
            let link_proven = ordered.iter().any(|item| {
                item.relation == Relation::Published
                    || (item.relation == Relation::Referenced && item.matched_text == identity.url)
            });
            if !link_proven {
                link_url = None;
            }
        }
        artifacts.push(WorkArtifact {
            artifact_id: artifact_id(&identity),
            kind: identity.kind.value().to_string(),
            locator: identity.url.clone(),
            url: link_url,
            label: identity.label.clone(),
            title: builder.title,
            external_id: identity.external_id.clone(),
            project_url: identity.project_url.clone(),
            project_slug: identity.project_slug.clone(),
            producer_thread_id: producer.map(|item| item.thread_id.clone()),
            produced_at_ms: producer.map(|item| item.timestamp_ms),
            evidence: ordered,
        });
    }
    artifacts.sort_by(|a, b| {
        (
            a.produced_at_ms.unwrap_or(i64::MAX),
            a.kind.as_str(),
            a.locator.as_str(),
        )
            .cmp(&(
                b.produced_at_ms.unwrap_or(i64::MAX),
                b.kind.as_str(),
                b.locator.as_str(),
            ))
    });
    let mut project_evidence: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for artifact in &artifacts {
        if artifact.kind != "repository" {
            continue;
        }
        let Some(project_url) = &artifact.project_url else {
            continue;
        };
        let strong: BTreeSet<String> = artifact
            .evidence
            .iter()
            .filter(|item| {
                matches!(
                    item.action.as_str(),
                    "repository_observed" | "project_of_output"
                )
            })
            .map(|item| item.evidence_id.clone())
            .collect();
        if !strong.is_empty() {
            project_evidence
                .entry(project_url.clone())
                .or_default()
                .extend(strong);
        }
    }
    let mut projects: Vec<ProjectIdentity> = Vec::new();
    for (url, evidence_ids) in &project_evidence {
        let Some(identity) = repository_identity(url) else {
            continue;
        };
        let Some(slug) = identity.project_slug.clone() else {
            continue;
        };
        let host = split_url(url)
            .and_then(|parts| parts.hostname)
            .unwrap_or_default();
        projects.push(ProjectIdentity {
            project_id: hash_id("project-", url),
            host,
            slug,
            url: url.clone(),
            evidence_ids: evidence_ids.iter().cloned().collect(),
        });
    }
    projects.sort_by(|a, b| {
        (a.host.as_str(), a.slug.as_str()).cmp(&(b.host.as_str(), b.slug.as_str()))
    });
    ArtifactCatalog {
        source_digest: crate::team_data::source_digest(team),
        artifacts,
        projects,
    }
}
