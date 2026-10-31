//! Deterministic, zero-model exports of coordinator prompts
//! and responses: `py/wrkviz/transcript_export.py` at parity.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::Digest as _;

use crate::build_store::{shared_build_file, shared_build_root};
use crate::json::{as_array, as_object, as_string, dump_canonical, py_repr_str};
use crate::pipeline::load_archived_team;
use crate::seekable::write_seekable_jsonl;
use crate::team_data::{Event, TeamData};
use crate::transcript_io::{
    read_document, read_jsonl, write_text_if_changed, write_text_if_changed_executable,
};

const MANAGED_FILES: [&str; 3] = ["prompts.jsonl", "messages.jsonl", "system-inputs.jsonl"];
const CONTAINER_SUFFIX: &str = ".gz";
const BASELINE_FILE: &str = "occurrences.jsonl";
const AUTHORSHIP_RULES_FILE: &str = "authorship-rules.json";
const UNCLASSIFIED: [&str; 2] = ["external_or_unknown", "unknown"];

type Record = serde_json::Map<String, Value>;
type GroupKey = (String, String, i64, i64, String, String);
type UnionResult = (
    Vec<Value>,
    usize,
    usize,
    std::collections::HashMap<String, AuthorshipMigration>,
);

fn rec_get<'a>(record: &'a Value, key: &str) -> Option<&'a Value> {
    record.as_object().and_then(|map| map.get(key))
}

fn rec_string(record: &Value, key: &str, where_: &str) -> Result<String, String> {
    match rec_get(record, key) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(format!("{where_}: expected a string")),
    }
}

fn rec_int(record: &Value, key: &str, where_: &str) -> Result<i64, String> {
    match rec_get(record, key) {
        Some(value) => crate::json::as_int(value, where_),
        None => Err(format!("{where_}: expected an integer")),
    }
}

fn sha256_text(text: &str) -> String {
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stable_id(prefix: &str, fields: &[&str]) -> String {
    let material = fields.join("\0");
    let digest = sha2::Sha256::digest(material.as_bytes());
    let short: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{prefix}:{short}")
}

fn instant_utc(timestamp_ms: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(timestamp_ms)
        .map(|moment| moment.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .ok_or_else(|| "timestamp out of range for platform time_t".to_string())
}

fn instant_in_zone(timestamp_ms: i64, zone: &chrono_tz::Tz) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(timestamp_ms)
        .map(|moment| {
            moment
                .with_timezone(zone)
                .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
                .to_string()
        })
        .ok_or_else(|| "timestamp out of range for platform time_t".to_string())
}

fn team_zone(team: &TeamData) -> Result<chrono_tz::Tz, String> {
    team.display_timezone.parse::<chrono_tz::Tz>().map_err(|_| {
        format!(
            "team {} has unknown display timezone {}",
            py_repr_str(&team.team_slug),
            py_repr_str(&team.display_timezone)
        )
    })
}

fn rule_from_json(value: &Value, where_: &str) -> Result<PromptAuthorshipRule, String> {
    let obj = as_object(value, where_)?;
    const EXPECTED: [&str; 8] = [
        "rule_id",
        "team_slug",
        "ingress_kind",
        "author_kind",
        "reason",
        "start_ms",
        "end_ms",
        "source_native_ids",
    ];
    if obj.len() != EXPECTED.len() || !EXPECTED.iter().all(|key| obj.contains_key(*key)) {
        let missing: Vec<&str> = EXPECTED
            .iter()
            .copied()
            .filter(|key| !obj.contains_key(*key))
            .collect();
        let unknown: Vec<&str> = obj
            .keys()
            .map(String::as_str)
            .filter(|key| !EXPECTED.contains(key))
            .collect();
        return Err(format!(
            "{where_}: invalid fields; missing={}, unknown={}",
            crate::json::py_repr(&Value::Array(
                missing
                    .iter()
                    .map(|key| Value::String(key.to_string()))
                    .collect()
            )),
            crate::json::py_repr(&Value::Array(
                unknown
                    .iter()
                    .map(|key| Value::String(key.to_string()))
                    .collect()
            ))
        ));
    }
    let get = |key: &str| obj.get(key).unwrap_or(&Value::Null);
    let opt_ms = |key: &str| -> Result<Option<i64>, String> {
        match get(key) {
            Value::Null => Ok(None),
            value => crate::json::as_int(value, &format!("{where_}.{key}")).map(Some),
        }
    };
    let mut native_ids = Vec::new();
    for (index, item) in as_array(
        get("source_native_ids"),
        &format!("{where_}.source_native_ids"),
    )?
    .iter()
    .enumerate()
    {
        native_ids
            .push(as_string(item, &format!("{where_}.source_native_ids[{index}]"))?.to_string());
    }
    let rule = PromptAuthorshipRule {
        rule_id: as_string(get("rule_id"), &format!("{where_}.rule_id"))?.to_string(),
        team_slug: as_string(get("team_slug"), &format!("{where_}.team_slug"))?.to_string(),
        ingress_kind: as_string(get("ingress_kind"), &format!("{where_}.ingress_kind"))?
            .to_string(),
        author_kind: as_string(get("author_kind"), &format!("{where_}.author_kind"))?.to_string(),
        reason: as_string(get("reason"), &format!("{where_}.reason"))?.to_string(),
        start_ms: opt_ms("start_ms")?,
        end_ms: opt_ms("end_ms")?,
        source_native_ids: native_ids,
    };
    rule.validate()?;
    Ok(rule)
}

fn rules_overlap(left: &PromptAuthorshipRule, right: &PromptAuthorshipRule) -> bool {
    if left.team_slug != right.team_slug || left.ingress_kind != right.ingress_kind {
        return false;
    }
    if let (Some(left_end), Some(right_start)) = (left.end_ms, right.start_ms) {
        if left_end <= right_start {
            return false;
        }
    }
    if let (Some(right_end), Some(left_start)) = (right.end_ms, left.start_ms) {
        if right_end <= left_start {
            return false;
        }
    }
    if left.source_native_ids.is_empty() || right.source_native_ids.is_empty() {
        return true;
    }
    left.source_native_ids
        .iter()
        .any(|id| right.source_native_ids.contains(id))
}

fn validate_rules(
    rules: &[PromptAuthorshipRule],
    team_slugs: &BTreeSet<String>,
) -> Result<Vec<PromptAuthorshipRule>, String> {
    let mut ordered = rules.to_vec();
    let key = |rule: &PromptAuthorshipRule| {
        (
            rule.team_slug.clone(),
            rule.ingress_kind.clone(),
            i128::from(rule.start_ms.unwrap_or(-1)),
            rule.end_ms.map_or(1i128 << 63, i128::from),
            rule.rule_id.clone(),
        )
    };
    ordered.sort_by_key(key);
    let mut ids = BTreeSet::new();
    for rule in &ordered {
        if !ids.insert(rule.rule_id.clone()) {
            return Err(format!(
                "duplicate prompt authorship rule {}",
                py_repr_str(&rule.rule_id)
            ));
        }
        if !team_slugs.contains(&rule.team_slug) {
            return Err(format!(
                "prompt authorship rule {} selects unknown team {}",
                py_repr_str(&rule.rule_id),
                py_repr_str(&rule.team_slug)
            ));
        }
    }
    for (index, left) in ordered.iter().enumerate() {
        for right in &ordered[index + 1..] {
            if rules_overlap(left, right) {
                return Err(format!(
                    "overlapping prompt authorship rules {} and {}",
                    py_repr_str(&left.rule_id),
                    py_repr_str(&right.rule_id)
                ));
            }
        }
    }
    Ok(ordered)
}

fn load_rules(path: &Path) -> Result<Vec<PromptAuthorshipRule>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let unsafe_file = std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink() || !meta.is_file())
        .unwrap_or(false);
    if unsafe_file {
        return Err(format!(
            "prompt authorship rules are not a regular file: {}",
            path.display()
        ));
    }
    let root = read_document(path)?;
    let root = as_object(&root, &path.display().to_string())?;
    if root.len() != 2 || !root.contains_key("schema_version") || !root.contains_key("rules") {
        return Err(format!(
            "invalid prompt authorship rules document at {}",
            path.display()
        ));
    }
    if crate::json::as_int(
        root.get("schema_version").unwrap_or(&Value::Null),
        &format!("{}.schema_version", path.display()),
    )? != 1
    {
        return Err(format!(
            "unsupported prompt authorship rules schema at {}",
            path.display()
        ));
    }
    let mut rules = Vec::new();
    for (index, value) in as_array(
        root.get("rules").unwrap_or(&Value::Null),
        &format!("{}.rules", path.display()),
    )?
    .iter()
    .enumerate()
    {
        rules.push(rule_from_json(
            value,
            &format!("{}.rules[{index}]", path.display()),
        )?);
    }
    Ok(rules)
}

struct AuthorshipMigration {
    rule_id: String,
    source_native_ids: Vec<String>,
}

fn apply_authorship_rules(
    records: &mut [Value],
    rules: &[PromptAuthorshipRule],
    migrations: &std::collections::HashMap<String, AuthorshipMigration>,
) -> Result<std::collections::HashMap<String, u64>, String> {
    let mut applied: std::collections::HashMap<String, u64> =
        rules.iter().map(|rule| (rule.rule_id.clone(), 0)).collect();
    for record in records.iter_mut() {
        if rec_get(record, "record_type").and_then(Value::as_str) != Some("prompt") {
            continue;
        }
        let record_id = rec_string(record, "record_id", "prompt.record_id")?;
        let migrated = migrations.get(&record_id);
        let current_kind = rec_get(record, "source_author_kind")
            .or_else(|| rec_get(record, "author_kind"))
            .cloned()
            .unwrap_or(Value::Null);
        let source_kind = current_kind.as_str().map(str::to_string);
        let source_version = rec_get(record, "source_classification_version")
            .or_else(|| rec_get(record, "classification_version"))
            .cloned()
            .unwrap_or(Value::Null);
        let object = record.as_object_mut().expect("record object");
        object.insert(
            "source_author_kind".into(),
            source_kind.clone().map_or(Value::Null, Value::String),
        );
        object.insert(
            "source_classification_version".into(),
            source_version.clone(),
        );
        object.insert(
            "author_kind".into(),
            source_kind.clone().map_or(Value::Null, Value::String),
        );
        object.insert("classification_version".into(), source_version.clone());
        object.remove("authorship_rule_id");
        object.remove("authorship_rule_reason");
        if !matches!(source_kind.as_deref(), Some(kind) if UNCLASSIFIED.contains(&kind)) {
            continue;
        }
        let mut matches: Vec<&PromptAuthorshipRule> = Vec::new();
        for rule in rules {
            if rule.matches(record)? {
                matches.push(rule);
            }
        }
        if matches.is_empty() {
            if let Some(migrated) = migrated {
                if let Some(prior_rule) = rules.iter().find(|rule| rule.rule_id == migrated.rule_id)
                {
                    for prior_native in &migrated.source_native_ids {
                        let mut prior_record = record.clone();
                        prior_record.as_object_mut().expect("record object").insert(
                            "source_native_id".into(),
                            Value::String(prior_native.clone()),
                        );
                        if prior_rule.matches(&prior_record)? {
                            matches.push(prior_rule);
                            break;
                        }
                    }
                }
            }
        }
        if matches.len() > 1 {
            return Err(format!(
                "multiple prompt authorship rules matched record {}",
                py_repr_str(&record_id)
            ));
        }
        let Some(rule) = matches.first() else {
            continue;
        };
        let object = record.as_object_mut().expect("record object");
        object.insert(
            "author_kind".into(),
            Value::String(rule.author_kind.clone()),
        );
        let base_version = source_version
            .as_str()
            .map_or("unknown".to_string(), str::to_string);
        object.insert(
            "classification_version".into(),
            Value::String(format!("{base_version}+rule:{}", rule.rule_id)),
        );
        object.insert(
            "authorship_rule_id".into(),
            Value::String(rule.rule_id.clone()),
        );
        object.insert(
            "authorship_rule_reason".into(),
            Value::String(rule.reason.clone()),
        );
        *applied.get_mut(&rule.rule_id).expect("rule count") += 1;
    }
    Ok(applied)
}

/// One authorship rule from `authorship-rules.json`.
#[derive(Clone, Debug)]
pub struct PromptAuthorshipRule {
    /// Rule id.
    pub rule_id: String,
    /// Team slug.
    pub team_slug: String,
    /// Ingress kind.
    pub ingress_kind: String,
    /// Author kind.
    pub author_kind: String,
    /// Reason.
    pub reason: String,
    /// Start ms.
    pub start_ms: Option<i64>,
    /// End ms.
    pub end_ms: Option<i64>,
    /// Source native ids.
    pub source_native_ids: Vec<String>,
}

impl PromptAuthorshipRule {
    fn validate(&self) -> Result<(), String> {
        for (label, value) in [
            ("rule_id", &self.rule_id),
            ("team_slug", &self.team_slug),
            ("ingress_kind", &self.ingress_kind),
            ("reason", &self.reason),
        ] {
            if value.trim().is_empty() || value.contains('\0') {
                return Err(format!("prompt authorship {label} must be non-empty"));
            }
        }
        if !["owner_human", "other_human", "agent", "system"].contains(&self.author_kind.as_str()) {
            return Err(
                "prompt authorship author_kind must be owner_human, other_human, agent, or system"
                    .to_string(),
            );
        }
        if let Some(start) = self.start_ms {
            if start < 0 {
                return Err("prompt authorship start_ms must be non-negative".to_string());
            }
        }
        if let Some(end) = self.end_ms {
            if end < 0 {
                return Err("prompt authorship end_ms must be non-negative".to_string());
            }
        }
        if let (Some(start), Some(end)) = (self.start_ms, self.end_ms) {
            if start >= end {
                return Err("prompt authorship interval must be non-empty".to_string());
            }
        }
        let mut seen = BTreeSet::new();
        for id in &self.source_native_ids {
            if !seen.insert(id) {
                return Err("prompt authorship source_native_ids contain duplicates".to_string());
            }
        }
        if self
            .source_native_ids
            .iter()
            .any(|value| value.is_empty() || value.contains('\0'))
        {
            return Err(
                "prompt authorship source_native_ids must contain non-empty strings".to_string(),
            );
        }
        Ok(())
    }

    fn to_json(&self) -> Value {
        let mut value = Record::new();
        value.insert("rule_id".into(), Value::String(self.rule_id.clone()));
        value.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        value.insert(
            "ingress_kind".into(),
            Value::String(self.ingress_kind.clone()),
        );
        value.insert(
            "author_kind".into(),
            Value::String(self.author_kind.clone()),
        );
        value.insert("reason".into(), Value::String(self.reason.clone()));
        value.insert(
            "start_ms".into(),
            self.start_ms.map_or(Value::Null, Value::from),
        );
        value.insert(
            "end_ms".into(),
            self.end_ms.map_or(Value::Null, Value::from),
        );
        value.insert(
            "source_native_ids".into(),
            Value::Array(
                self.source_native_ids
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        );
        Value::Object(value)
    }

    fn matches_scope(&self, record: &Value) -> Result<bool, String> {
        if rec_get(record, "team_slug").and_then(Value::as_str) != Some(self.team_slug.as_str()) {
            return Ok(false);
        }
        if rec_get(record, "ingress_kind").and_then(Value::as_str)
            != Some(self.ingress_kind.as_str())
        {
            return Ok(false);
        }
        let timestamp_ms = rec_int(record, "timestamp_ms", "prompt.timestamp_ms")?;
        if let Some(start) = self.start_ms {
            if timestamp_ms < start {
                return Ok(false);
            }
        }
        if let Some(end) = self.end_ms {
            if timestamp_ms >= end {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn matches(&self, record: &Value) -> Result<bool, String> {
        if !self.matches_scope(record)? {
            return Ok(false);
        }
        if self.source_native_ids.is_empty() {
            return Ok(true);
        }
        Ok(match rec_get(record, "source_native_id") {
            Some(Value::String(id)) => self.source_native_ids.contains(id),
            _ => false,
        })
    }
}

/// One archive team the projection carried forward unread.
#[derive(Clone, Debug)]
pub struct TranscriptTeamSkip {
    /// Team slug.
    pub team_slug: String,
    /// Error type name.
    pub error_type: String,
    /// Error text.
    pub error: String,
}

impl TranscriptTeamSkip {
    /// The one-line summary.
    pub fn summary(&self) -> String {
        format!("{}: {}: {}", self.team_slug, self.error_type, self.error)
    }

    fn to_json(&self) -> Value {
        let mut value = Record::new();
        value.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        value.insert("error_type".into(), Value::String(self.error_type.clone()));
        value.insert("error".into(), Value::String(self.error.clone()));
        Value::Object(value)
    }
}

/// One configured rule no projected team can take.
#[derive(Clone, Debug)]
pub struct DroppedAuthorshipRule {
    /// Rule id.
    pub rule_id: String,
    /// Team slug.
    pub team_slug: String,
}

impl DroppedAuthorshipRule {
    /// The one-line summary.
    pub fn summary(&self) -> String {
        format!(
            "{} (team {} has no normalized data)",
            self.rule_id, self.team_slug
        )
    }

    fn to_json(&self) -> Value {
        let mut value = Record::new();
        value.insert("rule_id".into(), Value::String(self.rule_id.clone()));
        value.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        Value::Object(value)
    }
}

struct GroupedEvent<'a> {
    team: &'a TeamData,
    events: Vec<&'a Event>,
    text: String,
}

fn coordinator_threads(team: &TeamData) -> BTreeSet<&str> {
    let mut result = BTreeSet::new();
    result.insert(team.root_thread_id.as_str());
    for agent in &team.agents {
        if agent.role.as_deref() == Some("coordinator") {
            result.insert(agent.thread_id.as_str());
        }
    }
    result
}

fn group_key(event: &Event) -> (String, String, i64, i64, String, String) {
    let native = event.source_native_id.clone().unwrap_or_default();
    let fallback = if native.is_empty() {
        event
            .event_id
            .rsplit_once(':')
            .map(|(head, _)| head.to_string())
            .unwrap_or_else(|| event.event_id.clone())
    } else {
        String::new()
    };
    (
        event.thread_id.clone(),
        event.turn_id.clone().unwrap_or_default(),
        event.timestamp_ms,
        event.source_line,
        event.kind.clone(),
        if native.is_empty() { fallback } else { native },
    )
}

fn group_events<'a>(team: &'a TeamData, kinds: &[&str]) -> Vec<GroupedEvent<'a>> {
    let coordinator = coordinator_threads(team);
    let mut grouped: Vec<(GroupKey, Vec<&Event>)> = Vec::new();
    for event in &team.events {
        if !coordinator.contains(event.thread_id.as_str())
            || !kinds.contains(&event.kind.as_str())
            || team
                .window_start_ms
                .is_some_and(|start| event.timestamp_ms < start)
            || team
                .window_end_ms
                .is_some_and(|end| event.timestamp_ms >= end)
            || event
                .text
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
        {
            continue;
        }
        let key = group_key(event);
        match grouped.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, events)) => events.push(event),
            None => grouped.push((key, vec![event])),
        }
    }
    let mut result: Vec<GroupedEvent> = grouped
        .into_iter()
        .map(|(_, events)| {
            let mut ordered = events;
            ordered.sort_by(|left, right| left.event_id.cmp(&right.event_id));
            let text = ordered
                .iter()
                .filter_map(|event| event.text.as_deref().map(str::trim))
                .collect::<Vec<_>>()
                .join("\n\n");
            GroupedEvent {
                team,
                events: ordered,
                text,
            }
        })
        .collect();
    result.sort_by(|left, right| {
        let (left, right) = (left.events[0], right.events[0]);
        (left.timestamp_ms, left.source_line, &left.event_id).cmp(&(
            right.timestamp_ms,
            right.source_line,
            &right.event_id,
        ))
    });
    result
}

fn agent_fields(team: &TeamData, thread_id: &str) -> (String, String) {
    for agent in &team.agents {
        if agent.thread_id == thread_id {
            return (agent.agent_path.clone(), agent.source_path.clone());
        }
    }
    ("/root".to_string(), String::new())
}

fn base_record(group: &GroupedEvent, record_type: &str) -> Result<Value, String> {
    let event = group.events[0];
    let team = group.team;
    let (agent_path, source_path) = agent_fields(team, &event.thread_id);
    let digest = sha256_text(&group.text);
    let native = event.source_native_id.as_deref().unwrap_or("");
    let mut occurrence_fields: Vec<&str> = vec![
        &team.team_slug,
        &team.provider,
        &event.thread_id,
        &source_path,
        native,
    ];
    let source_line = event.source_line.to_string();
    let timestamp = event.timestamp_ms.to_string();
    occurrence_fields.push(&source_line);
    occurrence_fields.push(&timestamp);
    occurrence_fields.push(&digest);
    for item in &group.events {
        occurrence_fields.push(&item.event_id);
    }
    let logical_native = if native.is_empty() {
        event.event_id.as_str()
    } else {
        native
    };
    let logical_fields: Vec<&str> = vec![
        &team.provider,
        &event.thread_id,
        logical_native,
        &timestamp,
        &digest,
    ];
    let zone = team_zone(team)?;
    let mut record = Record::new();
    record.insert("schema_version".into(), 1.into());
    record.insert("record_type".into(), Value::String(record_type.to_string()));
    record.insert(
        "record_id".into(),
        Value::String(stable_id(record_type, &occurrence_fields)),
    );
    record.insert(
        "logical_record_id".into(),
        Value::String(stable_id(
            &format!("logical-{record_type}"),
            &logical_fields,
        )),
    );
    record.insert("team_slug".into(), Value::String(team.team_slug.clone()));
    record.insert("provider".into(), Value::String(team.provider.clone()));
    record.insert("thread_id".into(), Value::String(event.thread_id.clone()));
    record.insert(
        "turn_id".into(),
        event.turn_id.clone().map_or(Value::Null, Value::String),
    );
    record.insert("agent_path".into(), Value::String(agent_path));
    record.insert("timestamp_ms".into(), event.timestamp_ms.into());
    record.insert(
        "timestamp_utc".into(),
        Value::String(instant_utc(event.timestamp_ms)?),
    );
    record.insert(
        "timestamp_local".into(),
        Value::String(instant_in_zone(event.timestamp_ms, &zone)?),
    );
    record.insert(
        "display_timezone".into(),
        Value::String(team.display_timezone.clone()),
    );
    record.insert("text".into(), Value::String(group.text.clone()));
    record.insert("content_sha256".into(), Value::String(digest));
    record.insert("source_path".into(), Value::String(source_path));
    record.insert("source_line".into(), event.source_line.into());
    record.insert(
        "source_event_ids".into(),
        Value::Array(
            group
                .events
                .iter()
                .map(|item| Value::String(item.event_id.clone()))
                .collect(),
        ),
    );
    record.insert(
        "source_native_id".into(),
        event
            .source_native_id
            .clone()
            .map_or(Value::Null, Value::String),
    );
    record.insert(
        "ingress_kind".into(),
        event
            .ingress_kind
            .clone()
            .map_or(Value::Null, Value::String),
    );
    record.insert(
        "author_kind".into(),
        event.author_kind.clone().map_or(Value::Null, Value::String),
    );
    record.insert(
        "classification_version".into(),
        event
            .classification_version
            .clone()
            .map_or(Value::Null, Value::String),
    );
    Ok(Value::Object(record))
}

fn record_sort_key(record: &Value) -> Result<(i64, i64, String, String), String> {
    let record_type = rec_string(record, "record_type", "record.record_type")?;
    Ok((
        rec_int(record, "timestamp_ms", "record.timestamp_ms")?,
        if record_type == "prompt" { 0 } else { 1 },
        rec_string(record, "team_slug", "record.team_slug")?,
        rec_string(record, "record_id", "record.record_id")?,
    ))
}

fn sort_records(records: &mut [Value]) -> Result<(), String> {
    let mut keyed: Vec<((i64, i64, String, String), Value)> = Vec::new();
    for record in records.iter() {
        keyed.push((record_sort_key(record)?, record.clone()));
    }
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    for (index, (_, record)) in keyed.into_iter().enumerate() {
        records[index] = record;
    }
    Ok(())
}

/// Counts produced by one mechanical transcript export.
#[derive(Clone, Debug, Default)]
pub struct TranscriptExportReport {
    /// Teams read.
    pub teams: usize,
    /// Prompts.
    pub prompts: usize,
    /// Responses.
    pub responses: usize,
    /// System inputs.
    pub system_inputs: usize,
    /// Carried forward.
    pub carried_forward: usize,
    /// Files changed.
    pub files_changed: usize,
    /// Reclassified.
    pub reclassified: usize,
    /// Skipped teams.
    pub skipped_teams: Vec<TranscriptTeamSkip>,
    /// Dropped rules.
    pub dropped_authorship_rules: Vec<DroppedAuthorshipRule>,
}

impl TranscriptExportReport {
    /// Whether this projection is missing anything.
    pub fn partial(&self) -> bool {
        !self.skipped_teams.is_empty() || !self.dropped_authorship_rules.is_empty()
    }

    /// One line naming everything missing, when partial.
    pub fn partiality_summary(&self) -> Option<String> {
        if !self.partial() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.skipped_teams.is_empty() {
            parts.push(format!(
                "{} of {} archive teams could not be read and were carried forward unchanged: {}",
                self.skipped_teams.len(),
                self.teams + self.skipped_teams.len(),
                self.skipped_teams
                    .iter()
                    .map(TranscriptTeamSkip::summary)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if !self.dropped_authorship_rules.is_empty() {
            parts.push(format!(
                "{} prompt authorship rule(s) were not applied: {}",
                self.dropped_authorship_rules.len(),
                self.dropped_authorship_rules
                    .iter()
                    .map(DroppedAuthorshipRule::summary)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        Some(parts.join(" | "))
    }

    /// The run-receipt JSON object.
    pub fn to_json(&self) -> Value {
        let mut value = Record::new();
        value.insert("teams".into(), (self.teams as u64).into());
        value.insert("prompts".into(), (self.prompts as u64).into());
        value.insert("responses".into(), (self.responses as u64).into());
        value.insert("system_inputs".into(), (self.system_inputs as u64).into());
        value.insert(
            "carried_forward".into(),
            (self.carried_forward as u64).into(),
        );
        value.insert("reclassified".into(), (self.reclassified as u64).into());
        value.insert("files_changed".into(), (self.files_changed as u64).into());
        value.insert(
            "skipped_teams".into(),
            Value::Array(
                self.skipped_teams
                    .iter()
                    .map(TranscriptTeamSkip::to_json)
                    .collect(),
            ),
        );
        value.insert(
            "teams_skipped".into(),
            (self.skipped_teams.len() as u64).into(),
        );
        value.insert(
            "dropped_prompt_authorship_rules".into(),
            Value::Array(
                self.dropped_authorship_rules
                    .iter()
                    .map(DroppedAuthorshipRule::to_json)
                    .collect(),
            ),
        );
        value.insert("model_calls".into(), 0.into());
        value.insert("model_tokens".into(), 0.into());
        Value::Object(value)
    }
}

const MUTABLE_KEYS: [&str; 9] = [
    "ordinal",
    "ingress_kind",
    "author_kind",
    "classification_version",
    "source_author_kind",
    "source_classification_version",
    "authorship_rule_id",
    "authorship_rule_reason",
    "in_reply_to_prompt_id",
];

fn immutable_projection(record: &Value) -> Value {
    let object = record.as_object().expect("record object");
    Value::Object(
        object
            .iter()
            .filter(|(key, _)| !MUTABLE_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

fn source_event_ids(record: &Value) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    match rec_get(record, "source_event_ids") {
        Some(Value::Array(values)) => {
            for value in values {
                ids.push(match value {
                    Value::String(text) => text.clone(),
                    _ => return Err("record.source_event_ids[]: expected a string".into()),
                });
            }
            Ok(ids)
        }
        _ => Err("record.source_event_ids: expected an array".into()),
    }
}

fn source_occurrence_id(record: &Value) -> Result<String, String> {
    let ids = source_event_ids(record)?;
    let native = match rec_get(record, "source_native_id") {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    let team_slug = rec_string(record, "team_slug", "record.team_slug")?;
    let provider = rec_string(record, "provider", "record.provider")?;
    let thread_id = rec_string(record, "thread_id", "record.thread_id")?;
    let source_path = rec_string(record, "source_path", "record.source_path")?;
    let mut fields: Vec<&str> = vec![&team_slug, &provider, &thread_id, &source_path, &native];
    let line = rec_int(record, "source_line", "record.source_line")?.to_string();
    let timestamp = rec_int(record, "timestamp_ms", "record.timestamp_ms")?.to_string();
    fields.push(&line);
    fields.push(&timestamp);
    let digest = rec_string(record, "content_sha256", "record.content_sha256")?;
    fields.push(&digest);
    for id in &ids {
        fields.push(id);
    }
    Ok(stable_id("source-occurrence", &fields))
}

fn monotonic_union(old: &[Value], new: &[Value]) -> Result<UnionResult, String> {
    use std::collections::HashMap;
    let mut merged: Vec<(String, Value)> = Vec::new();
    let mut old_by_source: Vec<(String, Value)> = Vec::new();
    let mut old_by_migration: Vec<(String, Value)> = Vec::new();
    for record in old {
        let record_id = rec_string(record, "record_id", "old record.record_id")?;
        if merged.iter().any(|(id, _)| *id == record_id) {
            return Err(format!(
                "duplicate existing transcript record {}",
                py_repr_str(&record_id)
            ));
        }
        old_by_source.push((source_occurrence_id(record)?, record.clone()));
        old_by_migration.push((provenance_migration_id(record)?, record.clone()));
        merged.push((record_id, record.clone()));
    }
    let mut incoming_ids = BTreeSet::new();
    let mut effective_new_ids = BTreeSet::new();
    let mut new_source_ids = BTreeSet::new();
    let mut new_migration_ids = BTreeSet::new();
    let mut record_id_rewrites: HashMap<String, String> = HashMap::new();
    let mut migrated_authorship: HashMap<String, AuthorshipMigration> = HashMap::new();
    let mut reclassified = 0usize;
    let in_merged =
        |merged: &[(String, Value)], id: &str| merged.iter().any(|(existing, _)| existing == id);
    for incoming in new {
        let mut record = incoming.clone();
        if let Some(Value::String(linked)) = rec_get(&record, "in_reply_to_prompt_id").cloned() {
            let rewritten = record_id_rewrites.get(&linked).cloned().unwrap_or(linked);
            record
                .as_object_mut()
                .expect("record object")
                .insert("in_reply_to_prompt_id".into(), Value::String(rewritten));
        }
        let record_id = rec_string(&record, "record_id", "new record.record_id")?;
        if !incoming_ids.insert(record_id.clone()) {
            return Err(format!(
                "duplicate new transcript record {}",
                py_repr_str(&record_id)
            ));
        }
        let source_id = source_occurrence_id(&record)?;
        if !new_source_ids.insert(source_id.clone()) {
            return Err(format!(
                "duplicate new transcript source occurrence {}",
                py_repr_str(&source_id)
            ));
        }
        let migration_id = provenance_migration_id(&record)?;
        if !new_migration_ids.insert(migration_id.clone()) {
            return Err(format!(
                "duplicate new transcript migration occurrence {}",
                py_repr_str(&migration_id)
            ));
        }
        if let Some(position) = merged.iter().position(|(id, _)| *id == record_id) {
            let previous = merged[position].1.clone();
            if immutable_projection(&previous) != immutable_projection(&record) {
                return Err(format!(
                    "immutable transcript occurrence changed for {}",
                    py_repr_str(&record_id)
                ));
            }
            merged[position].1 = record;
            effective_new_ids.insert(record_id);
            continue;
        }
        let active_source_classes: Vec<Value> = old_by_source
            .iter()
            .filter(|(id, candidate)| {
                *id == source_id
                    && in_merged(
                        &merged,
                        rec_string(candidate, "record_id", "old record.record_id")
                            .unwrap_or_default()
                            .as_str(),
                    )
            })
            .map(|(_, candidate)| candidate.clone())
            .collect();
        let same_class: Vec<Value> = active_source_classes
            .iter()
            .filter(|candidate| {
                rec_get(candidate, "record_type") == rec_get(&record, "record_type")
            })
            .cloned()
            .collect();
        if same_class.len() > 1 || (!same_class.is_empty() && active_source_classes.len() > 1) {
            return Err(format!(
                "ambiguous stable transcript identity for {}",
                py_repr_str(&source_id)
            ));
        }
        if let Some(candidate) = same_class.first() {
            if provenance_migration_projection(candidate)
                != provenance_migration_projection(&record)
            {
                return Err(format!(
                    "immutable transcript occurrence changed while retaining its stable identity for {}",
                    py_repr_str(&source_id)
                ));
            }
            let (incoming_id, previous_id, authorship) =
                preserve_occurrence_identity(&mut record, candidate)?;
            if let Some(authorship) = authorship {
                migrated_authorship.insert(previous_id.clone(), authorship);
            }
            let position = merged
                .iter()
                .position(|(id, _)| *id == previous_id)
                .expect("candidate in merged");
            merged[position].1 = record;
            effective_new_ids.insert(previous_id.clone());
            record_id_rewrites.insert(incoming_id, previous_id);
            continue;
        }
        for previous_class in &active_source_classes {
            let previous_id = rec_string(previous_class, "record_id", "old record.record_id")?;
            if reclassification_projection(previous_class) != reclassification_projection(&record) {
                return Err(format!(
                    "immutable transcript occurrence changed while its message class was refined for {}",
                    py_repr_str(&source_id)
                ));
            }
            merged.retain(|(id, _)| *id != previous_id);
            reclassified += 1;
        }
        if !active_source_classes.is_empty() {
            merged.push((record_id.clone(), record));
            effective_new_ids.insert(record_id);
            continue;
        }
        let migration_candidates: Vec<Value> = old_by_migration
            .iter()
            .filter(|(id, candidate)| {
                *id == migration_id
                    && in_merged(
                        &merged,
                        rec_string(candidate, "record_id", "old record.record_id")
                            .unwrap_or_default()
                            .as_str(),
                    )
            })
            .map(|(_, candidate)| candidate.clone())
            .collect();
        if migration_candidates.len() > 1 {
            return Err(format!(
                "ambiguous transcript provenance migration for {}",
                py_repr_str(&migration_id)
            ));
        }
        if let Some(candidate) = migration_candidates.first() {
            if provenance_migration_projection(candidate)
                != provenance_migration_projection(&record)
            {
                return Err(format!(
                    "immutable transcript occurrence changed during provenance migration for {}",
                    py_repr_str(&migration_id)
                ));
            }
            let (incoming_id, previous_id, authorship) =
                preserve_occurrence_identity(&mut record, candidate)?;
            if let Some(authorship) = authorship {
                migrated_authorship.insert(previous_id.clone(), authorship);
            }
            let position = merged
                .iter()
                .position(|(id, _)| *id == previous_id)
                .expect("candidate in merged");
            merged[position].1 = record;
            effective_new_ids.insert(previous_id.clone());
            record_id_rewrites.insert(incoming_id, previous_id);
            continue;
        }
        merged.push((record_id.clone(), record));
        effective_new_ids.insert(record_id);
    }
    let carried = merged
        .iter()
        .filter(|(id, _)| !effective_new_ids.contains(id))
        .count();
    let mut rows: Vec<Value> = merged.into_iter().map(|(_, record)| record).collect();
    sort_records(&mut rows)?;
    Ok((rows, carried, reclassified, migrated_authorship))
}

fn container_manifest_entry(report: &crate::seekable::WriteReport) -> Value {
    let index = &report.index;
    let mut entry = Record::new();
    entry.insert("sha256".into(), Value::String(index.u_sha256.clone()));
    entry.insert("bytes".into(), Value::Number(index.u_size.into()));
    entry.insert("records".into(), Value::Number(index.record_count.into()));
    entry.insert(
        "container".into(),
        Value::String("multi-member-gzip".into()),
    );
    entry.insert(
        "path".into(),
        Value::String(
            report
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    );
    entry.insert(
        "index_path".into(),
        Value::String(
            report
                .index_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    );
    entry.insert("c_sha256".into(), Value::String(index.c_sha256.clone()));
    entry.insert("c_bytes".into(), Value::Number(index.c_size.into()));
    Value::Object(entry)
}

fn remove_superseded_plain_projection(path: &Path) -> Result<bool, String> {
    let is_link = std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_link || !path.is_file() {
        return Ok(false);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(crate::json::os_error(&error, path)),
    }
}

const ARCHIVE_MAKEFILE: &str = ".DEFAULT_GOAL := help\n.PHONY: help serve open run-stats query prompts\nPYTHON ?= python3\nPORT ?= 8765\nQUERY_ARGS ?= teams\n\nPROMPT_ARGS ?=\n\nhelp:\n\t@printf '%s\\n' \\\n\t  'Agent timeline archive commands:' \\\n\t  '  make serve                 Serve the website on PORT (default 8765)' \\\n\t  '  make open                  Serve it and ask Python to open a browser' \\\n\t  '  make run-stats             Print recorded pipeline runs and token costs' \\\n\t  '  make query QUERY_ARGS=...  Run the read-only ./timeline CLI' \\\n\t  '  make prompts PROMPT_ARGS=...  Print owner prompts' \\\n\t  '  ./timeline --help          Discover all query and search commands' \\\n\t  '' \\\n\t  'Variables: PORT, PYTHON, QUERY_ARGS, PROMPT_ARGS'\n\nserve:\n\t$(PYTHON) ./serve.py --port $(PORT)\n\nopen:\n\t$(PYTHON) ./serve.py --port $(PORT) --open\n\nrun-stats:\n\t$(PYTHON) ./run_stats.py\n\nquery:\n\t@./timeline $(QUERY_ARGS)\n\nprompts:\n\t@./timeline prompts $(PROMPT_ARGS)\n";

fn logical_records(occurrences: &[Value], record_type: &str) -> Result<Vec<Value>, String> {
    let mut grouped: Vec<(String, Vec<Value>)> = Vec::new();
    for occurrence in occurrences {
        if rec_get(occurrence, "record_type").and_then(Value::as_str) != Some(record_type) {
            continue;
        }
        let logical_id = rec_string(
            occurrence,
            "logical_record_id",
            "occurrence.logical_record_id",
        )?;
        match grouped.iter_mut().find(|(id, _)| *id == logical_id) {
            Some((_, group)) => group.push(occurrence.clone()),
            None => grouped.push((logical_id, vec![occurrence.clone()])),
        }
    }
    let mut result = Vec::new();
    for (logical_id, group) in grouped {
        let mut ordered = group;
        sort_records(&mut ordered)?;
        let attributed: Vec<&Value> = ordered
            .iter()
            .filter(|item| {
                matches!(rec_get(item, "author_kind"), Some(Value::String(kind))
                    if !UNCLASSIFIED.contains(&kind.as_str()))
            })
            .collect();
        let mut kinds = BTreeSet::new();
        for item in &attributed {
            if let Some(kind) = rec_get(item, "author_kind").and_then(Value::as_str) {
                kinds.insert(kind.to_string());
            }
        }
        if kinds.len() > 1 {
            let kinds_value = Value::Array(
                kinds
                    .iter()
                    .map(|kind| Value::String(kind.clone()))
                    .collect(),
            );
            return Err(format!(
                "conflicting authorship classifications for {}: {}",
                py_repr_str(&logical_id),
                crate::json::py_repr(&kinds_value)
            ));
        }
        let representative = attributed.first().copied().unwrap_or(&ordered[0]);
        let mut record = representative.clone();
        let object = record.as_object_mut().expect("record object");
        object.insert("record_id".into(), Value::String(logical_id));
        let mut occurrence_ids: Vec<String> = Vec::new();
        let mut occurrence_teams = BTreeSet::new();
        for item in &ordered {
            occurrence_ids.push(rec_string(item, "record_id", "occurrence.record_id")?);
            occurrence_teams.insert(rec_string(item, "team_slug", "occurrence.team_slug")?);
        }
        occurrence_ids.sort();
        object.insert(
            "occurrence_ids".into(),
            Value::Array(
                occurrence_ids
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        );
        object.insert(
            "occurrence_teams".into(),
            Value::Array(
                occurrence_teams
                    .iter()
                    .map(|team| Value::String(team.clone()))
                    .collect(),
            ),
        );
        object.insert("occurrence_count".into(), (ordered.len() as u64).into());
        result.push(record);
    }
    sort_records(&mut result)?;
    Ok(result)
}

fn provenance_migration_id(record: &Value) -> Result<String, String> {
    let ids = source_event_ids(record)?;
    let team_slug = rec_string(record, "team_slug", "record.team_slug")?;
    let provider = rec_string(record, "provider", "record.provider")?;
    let thread_id = rec_string(record, "thread_id", "record.thread_id")?;
    let source_path = rec_string(record, "source_path", "record.source_path")?;
    let mut fields: Vec<&str> = vec![&team_slug, &provider, &thread_id, &source_path];
    let timestamp = rec_int(record, "timestamp_ms", "record.timestamp_ms")?.to_string();
    fields.push(&timestamp);
    let digest = rec_string(record, "content_sha256", "record.content_sha256")?;
    fields.push(&digest);
    for id in &ids {
        fields.push(id);
    }
    Ok(stable_id("provenance-migration", &fields))
}

fn reclassification_projection(record: &Value) -> Value {
    let mut projected = immutable_projection(record);
    let object = projected.as_object_mut().expect("record object");
    object.remove("record_type");
    object.remove("record_id");
    object.remove("logical_record_id");
    projected
}

fn provenance_migration_projection(record: &Value) -> Value {
    let mut projected = immutable_projection(record);
    let object = projected.as_object_mut().expect("record object");
    for key in [
        "record_id",
        "logical_record_id",
        "source_native_id",
        "source_native_id_history",
        "source_line",
    ] {
        object.remove(key);
    }
    projected
}

fn source_native_id_history(record: &Value) -> Result<Vec<String>, String> {
    match rec_get(record, "source_native_id_history") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => {
            let mut ids = Vec::new();
            for value in values {
                match value {
                    Value::String(text) => ids.push(text.clone()),
                    _ => return Err("record.source_native_id_history[]: expected a string".into()),
                }
            }
            let mut sorted = ids.clone();
            sorted.sort();
            sorted.dedup();
            if sorted.len() != ids.len() {
                return Err("record.source_native_id_history contains duplicates".into());
            }
            Ok(ids)
        }
        Some(_) => Err("record.source_native_id_history: expected an array".into()),
    }
}

fn preserve_occurrence_identity(
    record: &mut Value,
    candidate: &Value,
) -> Result<(String, String, Option<AuthorshipMigration>), String> {
    let incoming_id = rec_string(record, "record_id", "new record.record_id")?;
    let previous_id = rec_string(candidate, "record_id", "old record.record_id")?;
    let logical = rec_string(
        candidate,
        "logical_record_id",
        "old record.logical_record_id",
    )?;
    let mut prior_native_ids: BTreeSet<String> =
        source_native_id_history(candidate)?.into_iter().collect();
    let prior_native = rec_get(candidate, "source_native_id").cloned();
    let current_native = rec_get(record, "source_native_id").cloned();
    if let Some(Value::String(prior)) = &prior_native {
        if current_native.as_ref().and_then(Value::as_str) != Some(prior.as_str()) {
            prior_native_ids.insert(prior.clone());
        }
    }
    if let Some(Value::String(current)) = &current_native {
        prior_native_ids.remove(current);
    }
    let object = record.as_object_mut().expect("record object");
    object.insert("record_id".into(), Value::String(previous_id.clone()));
    object.insert("logical_record_id".into(), Value::String(logical));
    if prior_native_ids.is_empty() {
        object.remove("source_native_id_history");
    } else {
        object.insert(
            "source_native_id_history".into(),
            Value::Array(
                prior_native_ids
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        );
    }
    let migration = match rec_get(candidate, "authorship_rule_id") {
        Some(Value::String(rule_id)) => Some(AuthorshipMigration {
            rule_id: rule_id.clone(),
            source_native_ids: prior_native_ids.into_iter().collect(),
        }),
        _ => None,
    };
    Ok((incoming_id, previous_id, migration))
}

fn previous_generation_file(
    root: &Path,
    archive: &Path,
    name: &str,
    entry: &Value,
    where_: &Path,
) -> Result<(PathBuf, String), String> {
    let where_s = where_.display();
    if name == BASELINE_FILE {
        return Ok((
            shared_build_file(archive, name)?,
            as_string(
                rec_get(entry, "sha256").unwrap_or(&Value::Null),
                &format!("{where_s}.files.{name}.sha256"),
            )?
            .to_string(),
        ));
    }
    match rec_get(entry, "container") {
        None | Some(Value::Null) => Ok((
            root.join(name),
            as_string(
                rec_get(entry, "sha256").unwrap_or(&Value::Null),
                &format!("{where_s}.files.{name}.sha256"),
            )?
            .to_string(),
        )),
        Some(container) => {
            let container_text =
                as_string(container, &format!("{where_s}.files.{name}.container"))?;
            if container_text != "multi-member-gzip" {
                return Err(format!(
                    "{where_s}: {name} declares container {}, which this exporter does not write; expected {}",
                    crate::json::py_repr(container),
                    crate::json::py_repr_str("multi-member-gzip")
                ));
            }
            let declared = as_string(
                rec_get(entry, "path").unwrap_or(&Value::Null),
                &format!("{where_s}.files.{name}.path"),
            )?;
            if declared != format!("{name}{CONTAINER_SUFFIX}") {
                return Err(format!(
                    "{where_s}: {name} declares its container as {}, not {}",
                    crate::json::py_repr_str(declared),
                    crate::json::py_repr_str(&format!("{name}{CONTAINER_SUFFIX}"))
                ));
            }
            Ok((
                root.join(declared),
                as_string(
                    rec_get(entry, "c_sha256").unwrap_or(&Value::Null),
                    &format!("{where_s}.files.{name}.c_sha256"),
                )?
                .to_string(),
            ))
        }
    }
}

fn validate_previous_manifest(root: &Path, archive: &Path) -> Result<Option<Value>, String> {
    let path = root.join("manifest.json");
    if !path.exists() {
        return Ok(None);
    }
    let manifest = read_document(&path)?;
    let manifest_obj = as_object(&manifest, &path.display().to_string())?;
    if !crate::json::json_eq_one(manifest_obj.get("schema_version")) {
        return Err(format!(
            "unsupported transcript export manifest at {}",
            path.display()
        ));
    }
    let files = as_object(
        manifest_obj.get("files").unwrap_or(&Value::Null),
        &format!("{}.files", path.display()),
    )?;
    for name in MANAGED_FILES.iter().chain([BASELINE_FILE].iter()) {
        let entry = as_object(
            files.get(*name).unwrap_or(&Value::Null),
            &format!("{}.files.{name}", path.display()),
        )?;
        let (managed, expected) =
            previous_generation_file(root, archive, name, &Value::Object(entry.clone()), &path)?;
        let unsafe_file = std::fs::symlink_metadata(&managed)
            .map(|meta| meta.file_type().is_symlink() || !meta.is_file())
            .unwrap_or(true);
        if unsafe_file {
            return Err(format!(
                "transcript export file is missing or unsafe: {}",
                managed.display()
            ));
        }
        let bytes =
            std::fs::read(&managed).map_err(|error| crate::json::os_error(&error, &managed))?;
        if sha256_hex(&bytes) != expected {
            return Err(format!(
                "transcript export generation is incomplete: {} digest mismatch",
                managed.display()
            ));
        }
    }
    if let Some(rules_entry) = files.get(AUTHORSHIP_RULES_FILE) {
        let entry = as_object(
            rules_entry,
            &format!("{}.files.{AUTHORSHIP_RULES_FILE}", path.display()),
        )?;
        let expected = as_string(
            entry.get("sha256").unwrap_or(&Value::Null),
            &format!("{}.files.{AUTHORSHIP_RULES_FILE}.sha256", path.display()),
        )?;
        let rules_path = root.join(AUTHORSHIP_RULES_FILE);
        let unsafe_file = std::fs::symlink_metadata(&rules_path)
            .map(|meta| meta.file_type().is_symlink() || !meta.is_file())
            .unwrap_or(true);
        if unsafe_file {
            return Err(format!(
                "transcript export file is missing or unsafe: {}",
                rules_path.display()
            ));
        }
        let bytes = std::fs::read(&rules_path)
            .map_err(|error| crate::json::os_error(&error, &rules_path))?;
        if sha256_hex(&bytes) != expected {
            return Err(format!(
                "transcript export generation is incomplete: {} digest mismatch",
                rules_path.display()
            ));
        }
    }
    Ok(Some(manifest))
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn response_records(teams: &[TeamData], prompts: &[Value]) -> Result<Vec<Value>, String> {
    let mut candidates: Vec<((String, String, String), Vec<Value>)> = Vec::new();
    for prompt in prompts {
        let Some(turn_id) = rec_get(prompt, "turn_id").and_then(Value::as_str) else {
            continue;
        };
        let key = (
            rec_string(prompt, "team_slug", "prompt.team_slug")?,
            rec_string(prompt, "thread_id", "prompt.thread_id")?,
            turn_id.to_string(),
        );
        match candidates.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, values)) => values.push(prompt.clone()),
            None => candidates.push((key, vec![prompt.clone()])),
        }
    }
    for (_, values) in candidates.iter_mut() {
        let mut keyed: Vec<((i64, i64), Value)> = Vec::new();
        for value in values.iter() {
            keyed.push((
                (
                    rec_int(value, "timestamp_ms", "prompt.timestamp_ms")?,
                    rec_int(value, "source_line", "prompt.source_line")?,
                ),
                value.clone(),
            ));
        }
        keyed.sort_by_key(|(key, _)| *key);
        *values = keyed.into_iter().map(|(_, value)| value).collect();
    }
    let mut responses = Vec::new();
    for team in teams {
        for group in group_events(team, &["assistant_message"]) {
            let mut record = base_record(&group, "response")?;
            let first = group.events[0];
            let linked = match &first.turn_id {
                Some(turn_id) => {
                    let key = (
                        team.team_slug.clone(),
                        first.thread_id.clone(),
                        turn_id.clone(),
                    );
                    let eligible: Vec<&Value> = candidates
                        .iter()
                        .find(|(existing, _)| *existing == key)
                        .map(|(_, values)| {
                            values
                                .iter()
                                .filter(|prompt| {
                                    rec_int(prompt, "timestamp_ms", "prompt.timestamp_ms")
                                        .unwrap_or(i64::MAX)
                                        <= first.timestamp_ms
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    eligible.last().copied()
                }
                None => None,
            };
            let value = match linked {
                Some(prompt) => Value::String(rec_string(prompt, "record_id", "prompt.record_id")?),
                None => Value::Null,
            };
            record
                .as_object_mut()
                .expect("record object")
                .insert("in_reply_to_prompt_id".into(), value);
            responses.push(record);
        }
    }
    Ok(responses)
}

fn prune_retired_query_artifacts(archive: &Path) -> Result<u64, String> {
    let launcher = archive.join("query.py");
    let mut recognized = false;
    let mut changed = 0u64;
    if launcher.exists() || launcher.is_symlink() {
        let is_link = launcher.is_symlink();
        if is_link || !launcher.is_file() {
            return Ok(0);
        }
        let bundled = STANDALONE_QUERY.as_bytes();
        let matches = std::fs::read(&launcher)
            .map(|bytes| bytes == bundled)
            .unwrap_or(false);
        if matches {
            std::fs::remove_file(&launcher)
                .map_err(|error| crate::json::os_error(&error, &launcher))?;
            recognized = true;
            changed = 1;
        } else {
            return Ok(0);
        }
    }
    let marker = archive.join(".agent-team-timeline.json");
    if !recognized && !(marker.is_file() && !marker.is_symlink()) {
        return Ok(changed);
    }
    let cache = archive.join("__pycache__");
    if !cache.exists() {
        return Ok(changed);
    }
    if cache.is_symlink() || !cache.is_dir() {
        return Err(format!(
            "retired query bytecode cache is unsafe: {}",
            cache.display()
        ));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("query.")
                && name.ends_with(".pyc")
                && name["query.".len()..].contains('.')
            {
                candidates.push(entry.path());
            }
        }
    }
    candidates.sort();
    for candidate in candidates {
        let unsafe_entry = candidate.is_symlink() || !candidate.is_file();
        if unsafe_entry {
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

const STANDALONE_QUERY: &str = include_str!("../../../py/wrkviz/query.py");

/// Update the archive's append-only coordinator transcript projection.
pub fn export_transcripts(
    archive: &Path,
    teams: &[TeamData],
    skipped_teams: &[TranscriptTeamSkip],
) -> Result<TranscriptExportReport, String> {
    let mut ordered_teams: Vec<&TeamData> = teams.iter().collect();
    ordered_teams.sort_by(|left, right| left.team_slug.cmp(&right.team_slug));
    if ordered_teams.is_empty() {
        return Err("transcript extraction requires at least one ingested team".to_string());
    }
    let loaded_slugs: BTreeSet<String> = ordered_teams
        .iter()
        .map(|team| team.team_slug.clone())
        .collect();
    if loaded_slugs.len() != ordered_teams.len() {
        return Err("transcript extraction received duplicate team slugs".to_string());
    }
    let mut ordered_skips: Vec<&TranscriptTeamSkip> = skipped_teams.iter().collect();
    ordered_skips.sort_by(|left, right| left.team_slug.cmp(&right.team_slug));
    let skipped_slugs: BTreeSet<String> = ordered_skips
        .iter()
        .map(|skip| skip.team_slug.clone())
        .collect();
    if skipped_slugs.len() != ordered_skips.len() {
        return Err("transcript extraction received duplicate skipped team slugs".to_string());
    }
    let overlap: Vec<&String> = skipped_slugs.intersection(&loaded_slugs).collect();
    if !overlap.is_empty() {
        return Err(format!(
            "transcript extraction received a team as both loaded and skipped: {}",
            overlap
                .iter()
                .map(|slug| slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let root = archive.join("extracted").join("transcripts");
    let baseline_root = shared_build_root(archive)?;
    let previous_manifest = validate_previous_manifest(&root, archive)?;
    if let Some(previous) = &previous_manifest {
        let mut previous_teams = BTreeSet::new();
        let previous_teams_values = previous
            .get("teams")
            .and_then(Value::as_array)
            .ok_or("transcript manifest teams: expected an array")?;
        for value in previous_teams_values {
            previous_teams.insert(as_string(value, "transcript manifest team")?.to_string());
        }
        let represented: BTreeSet<String> = loaded_slugs.union(&skipped_slugs).cloned().collect();
        let omitted: Vec<&String> = previous_teams.difference(&represented).collect();
        if !omitted.is_empty() {
            return Err(format!(
                "monotonic transcript export cannot omit previously extracted teams: {}",
                omitted
                    .iter()
                    .map(|slug| slug.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let previous_occurrences = read_jsonl(&shared_build_file(archive, BASELINE_FILE)?)?;
    let configured_rules = load_rules(&root.join(AUTHORSHIP_RULES_FILE))?;
    let mut projected_teams: BTreeSet<String> =
        loaded_slugs.union(&skipped_slugs).cloned().collect();
    for record in &previous_occurrences {
        projected_teams.insert(rec_string(record, "team_slug", "old record.team_slug")?);
    }
    let dropped_authorship_rules: Vec<DroppedAuthorshipRule> = configured_rules
        .iter()
        .filter(|rule| !projected_teams.contains(&rule.team_slug))
        .map(|rule| DroppedAuthorshipRule {
            rule_id: rule.rule_id.clone(),
            team_slug: rule.team_slug.clone(),
        })
        .collect();
    let retained: Vec<PromptAuthorshipRule> = configured_rules
        .iter()
        .filter(|rule| projected_teams.contains(&rule.team_slug))
        .cloned()
        .collect();
    let rules = validate_rules(&retained, &projected_teams)?;
    let mut current_prompts: Vec<Value> = Vec::new();
    for team in &ordered_teams {
        for group in group_events(team, &["user_prompt"]) {
            current_prompts.push(base_record(&group, "prompt")?);
        }
    }
    let team_values: Vec<TeamData> = ordered_teams.iter().map(|team| (*team).clone()).collect();
    let current_responses = response_records(&team_values, &current_prompts)?;
    let mut current_system: Vec<Value> = Vec::new();
    for team in &ordered_teams {
        for group in group_events(team, &["system_input"]) {
            current_system.push(base_record(&group, "system_input")?);
        }
    }
    let mut current_occurrences = current_prompts;
    current_occurrences.extend(current_responses);
    current_occurrences.extend(current_system);
    let (mut occurrences, carried, reclassified, migrated_authorship) =
        monotonic_union(&previous_occurrences, &current_occurrences)?;
    let rule_counts = apply_authorship_rules(&mut occurrences, &rules, &migrated_authorship)?;
    let mut prompts = logical_records(&occurrences, "prompt")?;
    for (index, prompt) in prompts.iter_mut().enumerate() {
        prompt
            .as_object_mut()
            .expect("record object")
            .insert("ordinal".into(), ((index + 1) as u64).into());
    }
    let mut occurrence_to_logical: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for prompt in &prompts {
        let logical_id = rec_string(prompt, "record_id", "prompt.record_id")?;
        if let Some(Value::Array(ids)) = rec_get(prompt, "occurrence_ids") {
            for id in ids {
                if let Value::String(id) = id {
                    occurrence_to_logical.insert(id.clone(), logical_id.clone());
                }
            }
        }
    }
    let mut responses = logical_records(&occurrences, "response")?;
    for response in responses.iter_mut() {
        let linked = match rec_get(response, "in_reply_to_prompt_id") {
            Some(Value::String(id)) => occurrence_to_logical.get(id).cloned(),
            _ => None,
        };
        response.as_object_mut().expect("record object").insert(
            "in_reply_to_prompt_id".into(),
            linked.map_or(Value::Null, Value::String),
        );
    }
    let system_inputs = logical_records(&occurrences, "system_input")?;
    let mut messages: Vec<Value> = prompts.iter().chain(responses.iter()).cloned().collect();
    sort_records(&mut messages)?;
    let mut changed = 0u64;
    let mut file_manifest = Record::new();
    for name in MANAGED_FILES {
        let rows = match name {
            "prompts.jsonl" => &prompts,
            "messages.jsonl" => &messages,
            _ => &system_inputs,
        };
        let report = write_seekable_jsonl(
            &root.join(format!("{name}{CONTAINER_SUFFIX}")),
            rows,
            "timestamp_ms",
        )?;
        changed += (report.data_changed as u64) + (report.index_changed as u64);
        if remove_superseded_plain_projection(&root.join(name))? {
            changed += 1;
        }
        file_manifest.insert(name.into(), container_manifest_entry(&report));
    }
    let occurrences_text = canonical_jsonl(&occurrences);
    std::fs::create_dir_all(&baseline_root)
        .map_err(|error| crate::json::os_error(&error, &baseline_root))?;
    if write_text_if_changed(&baseline_root.join(BASELINE_FILE), &occurrences_text)? {
        changed += 1;
    }
    let rules_text = {
        let values: Vec<Value> = rules.iter().map(PromptAuthorshipRule::to_json).collect();
        let mut document = Record::new();
        document.insert("schema_version".into(), 1.into());
        document.insert("rules".into(), Value::Array(values));
        dump_canonical(&Value::Object(document))
    };
    if write_text_if_changed(&root.join(AUTHORSHIP_RULES_FILE), &rules_text)? {
        changed += 1;
    }
    let mut baseline_entry = Record::new();
    baseline_entry.insert(
        "sha256".into(),
        Value::String(sha256_text(&occurrences_text)),
    );
    baseline_entry.insert("bytes".into(), (occurrences_text.len() as u64).into());
    baseline_entry.insert("records".into(), (occurrences.len() as u64).into());
    file_manifest.insert(BASELINE_FILE.into(), Value::Object(baseline_entry));
    let mut rules_entry = Record::new();
    rules_entry.insert("sha256".into(), Value::String(sha256_text(&rules_text)));
    rules_entry.insert("bytes".into(), (rules_text.len() as u64).into());
    rules_entry.insert("records".into(), (rules.len() as u64).into());
    file_manifest.insert(AUTHORSHIP_RULES_FILE.into(), Value::Object(rules_entry));
    let mut source_generations: Vec<Value> = Vec::new();
    for team in &ordered_teams {
        let mut entry = Record::new();
        entry.insert("team_slug".into(), Value::String(team.team_slug.clone()));
        entry.insert("provider".into(), Value::String(team.provider.clone()));
        entry.insert(
            "root_thread_id".into(),
            Value::String(team.root_thread_id.clone()),
        );
        entry.insert(
            "source_digest".into(),
            Value::String(crate::team_data::source_digest(team)),
        );
        entry.insert("sources".into(), (team.sources.len() as u64).into());
        source_generations.push(Value::Object(entry));
    }
    let mut counts = Record::new();
    counts.insert("prompts".into(), (prompts.len() as u64).into());
    counts.insert("responses".into(), (responses.len() as u64).into());
    counts.insert("system_inputs".into(), (system_inputs.len() as u64).into());
    counts.insert("occurrences".into(), (occurrences.len() as u64).into());
    counts.insert("carried_forward".into(), (carried as u64).into());
    counts.insert("reclassified".into(), (reclassified as u64).into());
    counts.insert(
        "prompt_authorship_rules".into(),
        (rules.len() as u64).into(),
    );
    counts.insert("teams_read".into(), (ordered_teams.len() as u64).into());
    counts.insert("teams_skipped".into(), (ordered_skips.len() as u64).into());
    let unclassified = prompts
        .iter()
        .filter(|prompt| {
            matches!(rec_get(prompt, "author_kind"), Some(Value::String(kind))
                if UNCLASSIFIED.contains(&kind.as_str()))
        })
        .count();
    let mut rule_application_counts = Record::new();
    for (rule_id, count) in &rule_counts {
        rule_application_counts.insert(rule_id.clone(), (*count).into());
    }
    let mut prompt_authorship = Record::new();
    prompt_authorship.insert(
        "rule_application_counts".into(),
        Value::Object(rule_application_counts),
    );
    prompt_authorship.insert("unclassified_prompts".into(), (unclassified as u64).into());
    let mut manifest = Record::new();
    manifest.insert("schema_version".into(), 1.into());
    manifest.insert(
        "kind".into(),
        Value::String("mechanical-coordinator-transcript-export".into()),
    );
    manifest.insert("model_calls".into(), 0.into());
    manifest.insert("model_tokens".into(), 0.into());
    let mut all_teams: Vec<String> = loaded_slugs.union(&skipped_slugs).cloned().collect();
    all_teams.sort();
    manifest.insert(
        "teams".into(),
        Value::Array(
            all_teams
                .iter()
                .map(|team| Value::String(team.clone()))
                .collect(),
        ),
    );
    manifest.insert(
        "source_generations".into(),
        Value::Array(source_generations),
    );
    manifest.insert(
        "skipped_teams".into(),
        Value::Array(ordered_skips.iter().map(|skip| skip.to_json()).collect()),
    );
    manifest.insert(
        "dropped_prompt_authorship_rules".into(),
        Value::Array(
            dropped_authorship_rules
                .iter()
                .map(DroppedAuthorshipRule::to_json)
                .collect(),
        ),
    );
    manifest.insert("counts".into(), Value::Object(counts));
    manifest.insert("prompt_authorship".into(), Value::Object(prompt_authorship));
    manifest.insert(
        "ordinal_contract".into(),
        Value::String(
            "Prompt ordinals are 1-based chronological projection indexes; stable record_id values, not ordinals, are durable identities."
                .into(),
        ),
    );
    manifest.insert("files".into(), Value::Object(file_manifest));
    if write_text_if_changed(
        &root.join("manifest.json"),
        &dump_canonical(&Value::Object(manifest)),
    )? {
        changed += 1;
    }
    if write_text_if_changed_executable(&archive.join("timeline"), STANDALONE_QUERY)? {
        changed += 1;
    }
    if write_text_if_changed(&archive.join("Makefile"), ARCHIVE_MAKEFILE)? {
        changed += 1;
    }
    changed += prune_retired_query_artifacts(archive)?;
    Ok(TranscriptExportReport {
        teams: ordered_teams.len(),
        prompts: prompts.len(),
        responses: responses.len(),
        system_inputs: system_inputs.len(),
        carried_forward: carried,
        files_changed: changed as usize,
        reclassified,
        skipped_teams: ordered_skips.iter().map(|skip| (*skip).clone()).collect(),
        dropped_authorship_rules,
    })
}

fn canonical_jsonl(records: &[Value]) -> String {
    if records.is_empty() {
        return String::new();
    }
    let mut text = String::new();
    for record in records {
        text.push_str(&crate::json::dump_compact(record));
        text.push('\n');
    }
    text
}

/// Load the selected teams and export transcripts (pipeline entry).
pub fn extract_transcripts_archive(
    archive: &Path,
    team_slugs: &[String],
) -> Result<TranscriptExportReport, String> {
    let _lock = crate::pipeline::archive_writer_lock(archive)?;
    let selected: Vec<String> = if team_slugs.is_empty() {
        crate::build_store::ingested_team_slugs(archive)?
    } else {
        team_slugs.to_vec()
    };
    if selected.is_empty() {
        return Err(format!("no ingested teams found in {}", archive.display()));
    }
    let unique: BTreeSet<&String> = selected.iter().collect();
    if unique.len() != selected.len() {
        return Err("transcript extraction team selection contains duplicates".to_string());
    }
    let mut teams: Vec<TeamData> = Vec::new();
    let mut skipped: Vec<TranscriptTeamSkip> = Vec::new();
    for slug in &selected {
        match load_archived_team(archive, slug) {
            Ok(team) => teams.push(team),
            Err(error) => skipped.push(TranscriptTeamSkip {
                team_slug: slug.clone(),
                error_type: classify_load_error(&error).to_string(),
                error,
            }),
        }
    }
    if teams.is_empty() {
        return Err(format!(
            "no archive team in {} could be loaded for transcript extraction: {}",
            archive.display(),
            skipped
                .iter()
                .map(TranscriptTeamSkip::summary)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    export_transcripts(archive, &teams, &skipped)
}

fn classify_load_error(message: &str) -> &'static str {
    if message.contains("interrupted migration") {
        "BuildLocationError"
    } else if message.contains("invalid utf-8") || message.contains("invalid unicode") {
        "UnicodeDecodeError"
    } else if message.starts_with("[Errno 2]") {
        "FileNotFoundError"
    } else if message.starts_with("[Errno 13]") {
        "PermissionError"
    } else if message.starts_with("[Errno") {
        "OSError"
    } else if message.contains(" at line ") && message.contains(" column ") {
        "JSONDecodeError"
    } else if message.contains("unicode") || message.contains("Unicode") {
        "UnicodeDecodeError"
    } else {
        "ValueError"
    }
}
