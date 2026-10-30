//! `wrkviz query list`: concise records and stable references
//! over a built timeline, for the four list resources
//! (`teams`, `agents`, `phases`, `rollups`) in all four output
//! formats.
//!
//! The acceptance and projection rules are the Python
//! `TimelineQuery.list_records` rules, sentence for sentence,
//! over the two generations this crate reads: schema 3 (spine
//! line ranges, team-stamped by the shard they came from) and
//! schema 1 (the monolith, sole-team stamped). A schema-2
//! bootstrap declines to the refusal below rather than to a
//! misreading; that generation is a named unported corner.

use crate::inspect::{open_schema_3, schema_file, SchemaThree};
use crate::json::{
    as_array, as_int, as_object, as_string, dump_canonical, dump_oneline, py_repr, resolve_path,
};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The list filters the CLI collects (`_add_query_filters`).
pub struct Filters {
    /// Team slugs to include; empty selects every team.
    pub teams: Vec<String>,
    /// Inclusive RFC3339 overlap boundary, unparsed.
    pub start_time: Option<String>,
    /// Exclusive RFC3339 overlap boundary, unparsed.
    pub end_time: Option<String>,
    /// Rollup kinds to include; empty selects every kind.
    pub kinds: Vec<String>,
    /// Canonical `agent:TEAM::ID` reference to select.
    pub agent: Option<String>,
}

struct Window {
    start: Option<i64>,
    end: Option<i64>,
}

impl Window {
    fn overlaps(&self, start_ms: i64, end_ms: i64) -> bool {
        let effective_end = if end_ms <= start_ms {
            start_ms + 1
        } else {
            end_ms
        };
        if let Some(end) = self.end {
            if start_ms >= end {
                return false;
            }
        }
        match self.start {
            None => true,
            Some(start) => effective_end > start,
        }
    }
}

fn parse_instant(raw: &str, label: &str) -> Result<i64, String> {
    let message = format!("{label} must be an RFC3339 timestamp with an explicit offset or Z");
    let bytes = raw.as_bytes();
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    let shape = bytes.len() >= 20
        && digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && bytes[4] == b'-'
        && digit(5)
        && digit(6)
        && bytes[7] == b'-'
        && digit(8)
        && digit(9)
        && bytes[10] == b'T'
        && digit(11)
        && digit(12)
        && bytes[13] == b':'
        && digit(14)
        && digit(15)
        && bytes[16] == b':'
        && digit(17)
        && digit(18);
    if !shape {
        return Err(message);
    }
    let mut rest = &raw[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let count = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if count == 0 || count > 6 {
            return Err(message);
        }
        rest = &fraction[count..];
    }
    let zone_ok = rest == "Z"
        || (rest.len() == 6
            && (rest.starts_with('+') || rest.starts_with('-'))
            && rest.as_bytes()[1].is_ascii_digit()
            && rest.as_bytes()[2].is_ascii_digit()
            && rest.as_bytes()[3] == b':'
            && rest.as_bytes()[4].is_ascii_digit()
            && rest.as_bytes()[5].is_ascii_digit());
    if !zone_ok {
        return Err(message);
    }
    let normalized = if let Some(stripped) = raw.strip_suffix('Z') {
        format!("{stripped}+00:00")
    } else {
        raw.to_string()
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(&normalized).map_err(|_| message.clone())?;
    Ok(parsed.timestamp_millis())
}

fn parse_window(filters: &Filters) -> Result<Option<Window>, String> {
    if filters.start_time.is_none() && filters.end_time.is_none() {
        return Ok(None);
    }
    let start = filters
        .start_time
        .as_deref()
        .map(|raw| parse_instant(raw, "start time"))
        .transpose()?;
    let end = filters
        .end_time
        .as_deref()
        .map(|raw| parse_instant(raw, "end time"))
        .transpose()?;
    if let (Some(start_ms), Some(end_ms)) = (start, end) {
        if start_ms >= end_ms {
            return Err("start bound must be earlier than end bound".to_string());
        }
    }
    Ok(Some(Window { start, end }))
}

/// `datetime.fromtimestamp(ms / 1000, utc).isoformat(timespec="milliseconds")` with `Z`.
fn timestamp(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(instant) => instant.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => String::new(),
    }
}

type ReferenceOf = fn(&Map<String, Value>, &str) -> Result<String, String>;

fn local_identifier<'a>(team: &str, identifier: &'a str) -> &'a str {
    let prefix = format!("{team}::");
    identifier.strip_prefix(&prefix).unwrap_or(identifier)
}

fn team_of(record: &Map<String, Value>, where_: &str) -> Result<String, String> {
    Ok(as_string(
        record.get("team").unwrap_or(&Value::Null),
        &format!("{where_}.team"),
    )?
    .to_string())
}

fn agent_ref(record: &Map<String, Value>, where_: &str) -> Result<String, String> {
    let team = team_of(record, where_)?;
    let identifier = as_string(
        record.get("id").unwrap_or(&Value::Null),
        &format!("{where_}.id"),
    )?;
    Ok(format!(
        "agent:{team}::{}",
        local_identifier(&team, identifier)
    ))
}

fn phase_ref(record: &Map<String, Value>, where_: &str) -> Result<String, String> {
    let team = team_of(record, where_)?;
    let identifier = as_string(
        record.get("id").unwrap_or(&Value::Null),
        &format!("{where_}.id"),
    )?;
    Ok(format!(
        "phase:{team}::{}",
        local_identifier(&team, identifier)
    ))
}

fn rollup_ref(record: &Map<String, Value>, where_: &str) -> Result<String, String> {
    let team = team_of(record, where_)?;
    let kind = as_string(
        record.get("kind").unwrap_or(&Value::Null),
        &format!("{where_}.kind"),
    )?;
    let start_ms = as_int(
        record.get("start_ms").unwrap_or(&Value::Null),
        &format!("{where_}.start_ms"),
    )?;
    Ok(format!("rollup:{team}::{kind}::{start_ms}"))
}

fn interval(record: &Map<String, Value>, where_: &str) -> Result<(i64, i64), String> {
    let start_ms = as_int(
        record.get("start_ms").unwrap_or(&Value::Null),
        &format!("{where_}.start_ms"),
    )?;
    let end_ms = as_int(
        record.get("end_ms").unwrap_or(&Value::Null),
        &format!("{where_}.end_ms"),
    )?;
    Ok((start_ms, end_ms))
}

fn copy_fields(record: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    for key in keys {
        if let Some(value) = record.get(*key) {
            out.insert((*key).to_string(), value.clone());
        }
    }
    out
}

fn with_times(
    result: &mut Map<String, Value>,
    record: &Map<String, Value>,
    where_: &str,
) -> Result<(), String> {
    let (start_ms, end_ms) = interval(record, where_)?;
    result.insert("start_time".to_string(), Value::String(timestamp(start_ms)));
    result.insert("end_time".to_string(), Value::String(timestamp(end_ms)));
    Ok(())
}

fn optional_int(record: &Map<String, Value>, key: &str) -> Option<i64> {
    match record.get(key) {
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    }
}

fn optional_string<'a>(record: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    record.get(key).and_then(Value::as_str)
}

enum Generation {
    Three(SchemaThree),
    One(Map<String, Value>),
}

/// An opened timeline: teams plus a generation to read
/// collections from.
pub struct Timeline {
    root: PathBuf,
    teams: Vec<Map<String, Value>>,
    sole_team: Option<String>,
    generation: Generation,
    collections: RefCell<BTreeMap<&'static str, Vec<Map<String, Value>>>>,
}

fn record_array(source: &Map<String, Value>, key: &str) -> Result<Vec<Map<String, Value>>, String> {
    let raw = as_array(
        source.get(key).unwrap_or(&Value::Null),
        &format!("timeline.{key}"),
    )?;
    let mut out = Vec::new();
    for (index, value) in raw.iter().enumerate() {
        out.push(as_object(value, &format!("timeline.{key}[{index}]"))?.clone());
    }
    Ok(out)
}

impl Timeline {
    /// Open one archive: a complete schema-3 generation when
    /// present, otherwise the schema-1 monolith.
    pub fn open(root: &Path) -> Result<Timeline, String> {
        let root = resolve_path(root);
        let (generation, source) = match open_schema_3(&root) {
            Ok(three) => {
                let source = three.bootstrap.clone();
                (Generation::Three(three), source)
            }
            Err(_) => {
                let v2 = root.join("data/timeline-v2.json");
                if let Ok(meta) = std::fs::symlink_metadata(&v2) {
                    if !meta.is_file() {
                        return Err(format!(
                            "timeline schema-2 bootstrap is not a regular file: {}",
                            v2.display()
                        ));
                    }
                    return Err(
                        "the Rust edition has not ported the schema-2 timeline generation yet; \
                         the Python edition remains the oracle for it"
                            .to_string(),
                    );
                }
                let path = root.join("data/timeline.json");
                if !path.is_file() {
                    return Err(format!("no built timeline at {}", path.display()));
                }
                let value = crate::json::read_json(&path)?;
                let source = as_object(&value, &path.display().to_string())?.clone();
                let version = as_int(
                    source.get("schema_version").unwrap_or(&Value::Null),
                    "timeline.schema_version",
                )?;
                if version != 1 {
                    return Err(format!(
                        "unsupported timeline schema version {version}; expected 1"
                    ));
                }
                (Generation::One(source.clone()), source)
            }
        };
        let teams = record_array(&source, "teams")?;
        let sole_team = if teams.len() == 1 {
            Some(
                as_string(
                    teams[0].get("slug").unwrap_or(&Value::Null),
                    "timeline.teams[0].slug",
                )?
                .to_string(),
            )
        } else {
            None
        };
        Ok(Timeline {
            root,
            teams,
            sole_team,
            generation,
            collections: RefCell::new(BTreeMap::new()),
        })
    }

    /// One team's line range of one spine kind, unwrapped and
    /// team-stamped (`_SchemaThreeArchive._group`).
    fn spine_group(
        &self,
        three: &SchemaThree,
        kind: &str,
        team: &str,
    ) -> Result<Vec<Map<String, Value>>, String> {
        let Some(entry) = three
            .shards
            .iter()
            .find(|entry| entry.stream == "spine" && entry.team.as_deref() == Some(team))
        else {
            return Ok(Vec::new());
        };
        let Some(&(first, count)) = entry.line_ranges.get(kind) else {
            return Ok(Vec::new());
        };
        let Some(path) = schema_file(&self.root, &entry.path) else {
            return Err(format!("schema-3 shard vanished: {}", entry.path));
        };
        let compressed =
            std::fs::read(&path).map_err(|error| crate::json::os_error(&error, &path))?;
        let mut decoded = Vec::new();
        flate2::read::MultiGzDecoder::new(&compressed[..])
            .read_to_end(&mut decoded)
            .map_err(|error| error.to_string())?;
        let text = String::from_utf8(decoded).map_err(|error| error.to_string())?;
        let lines: Vec<&str> = text.lines().collect();
        let mut records = Vec::new();
        for offset in 0..count {
            let index = (first + offset) as usize;
            let Some(line) = lines.get(index) else {
                return Err(format!(
                    "schema-3 shard {} ended before line {}",
                    entry.path,
                    first + offset
                ));
            };
            let value: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
            let mut record = as_object(&value, &entry.path)?.clone();
            let found = record.remove("record_kind").unwrap_or(Value::Null);
            if found.as_str() != Some(kind) {
                return Err(format!(
                    "{}: expected a {} record, found {}",
                    entry.path,
                    py_repr(&Value::String(kind.to_string())),
                    py_repr(&found)
                ));
            }
            match record.get("team") {
                None | Some(Value::Null) => {
                    record.insert("team".to_string(), Value::String(team.to_string()));
                }
                Some(declared) => {
                    if declared.as_str() != Some(team) {
                        return Err(format!(
                            "{}: record names team {} in {}'s spine shard",
                            entry.path,
                            py_repr(declared),
                            py_repr(&Value::String(team.to_string()))
                        ));
                    }
                }
            }
            records.push(record);
        }
        Ok(records)
    }

    fn spine(
        &self,
        three: &SchemaThree,
        kind: &str,
        wanted: &[String],
    ) -> Result<Vec<Map<String, Value>>, String> {
        let mut slugs: Vec<String> = three
            .shards
            .iter()
            .filter(|entry| entry.stream == "spine")
            .filter_map(|entry| entry.team.clone())
            .collect();
        slugs.sort();
        slugs.dedup();
        let mut out = Vec::new();
        for slug in slugs {
            if !wanted.is_empty() && !wanted.contains(&slug) {
                continue;
            }
            out.extend(self.spine_group(three, kind, &slug)?);
        }
        Ok(out)
    }

    fn collection(
        &self,
        name: &'static str,
        pushdown: Option<&[String]>,
    ) -> Result<Vec<Map<String, Value>>, String> {
        if pushdown.is_none() {
            if let Some(cached) = self.collections.borrow().get(name) {
                return Ok(cached.clone());
            }
        }
        let kind = match name {
            "agents" => "agent",
            "phases" => "phase_card",
            "rollups" => "rollup",
            _ => unreachable!(),
        };
        let records = match &self.generation {
            Generation::Three(three) => self.spine(three, kind, pushdown.unwrap_or(&[]))?,
            Generation::One(source) => {
                let mut records = record_array(source, name)?;
                if let Some(sole) = &self.sole_team {
                    for record in &mut records {
                        if record.get("team").is_none_or(Value::is_null) {
                            record.insert("team".to_string(), Value::String(sole.clone()));
                        }
                    }
                }
                records
            }
        };
        if pushdown.is_some() {
            // A pushed-down read is already scoped; do not let
            // it stand in for the whole collection in the cache.
            return Ok(records);
        }
        self.collections.borrow_mut().insert(name, records.clone());
        Ok(records)
    }

    fn scoped(
        &self,
        name: &'static str,
        filters: &Filters,
    ) -> Result<Vec<Map<String, Value>>, String> {
        if matches!(self.generation, Generation::Three(_)) && !filters.teams.is_empty() {
            return self.collection(name, Some(&filters.teams));
        }
        self.collection(name, None)
    }

    fn for_team(&self, name: &'static str, team: &str) -> Result<Vec<Map<String, Value>>, String> {
        match &self.generation {
            Generation::Three(three) => {
                let kind = match name {
                    "agents" => "agent",
                    "phases" => "phase_card",
                    "rollups" => "rollup",
                    _ => unreachable!(),
                };
                self.spine_group(three, kind, team)
            }
            Generation::One(_) => {
                let singular = match name {
                    "agents" => "agent",
                    "phases" => "phase",
                    "rollups" => "rollup",
                    _ => unreachable!(),
                };
                let mut out = Vec::new();
                for record in self.collection(name, None)? {
                    if team_of(&record, singular)? == team {
                        out.push(record);
                    }
                }
                Ok(out)
            }
        }
    }

    /// Resolve one stable reference to its record kind, the
    /// way `--agent` validation needs it.
    fn lookup(&self, reference: &str) -> Result<Option<String>, String> {
        let Some((kind, remainder)) = reference.split_once(':') else {
            return Ok(None);
        };
        if kind == "team" {
            for record in &self.teams {
                if optional_string(record, "slug") == Some(remainder) {
                    return Ok(Some("team".to_string()));
                }
            }
            return Ok(None);
        }
        if !["agent", "phase", "rollup"].contains(&kind) {
            return Ok(None);
        }
        match &self.generation {
            Generation::Three(_) => {
                let Some((team, _)) = remainder.split_once("::") else {
                    return Ok(None);
                };
                let (name, reference_of): (&'static str, ReferenceOf) = match kind {
                    "agent" => ("agents", agent_ref),
                    "phase" => ("phases", phase_ref),
                    _ => ("rollups", rollup_ref),
                };
                for record in self.for_team(name, team)? {
                    let singular = match kind {
                        "agent" => "agent",
                        "phase" => "phase",
                        _ => "rollup",
                    };
                    if reference_of(&record, singular)? == reference {
                        return Ok(Some(kind.to_string()));
                    }
                }
                Ok(None)
            }
            Generation::One(_) => {
                // Build the whole index first, the way
                // `_index` does: a duplicate anywhere refuses
                // the lookup, even after the sought reference.
                let mut entries: Vec<(String, String)> = Vec::new();
                for (name, reference_of) in [
                    ("agents", agent_ref as ReferenceOf),
                    ("phases", phase_ref as ReferenceOf),
                    ("rollups", rollup_ref as ReferenceOf),
                ] {
                    let singular = match name {
                        "agents" => "agent",
                        "phases" => "phase",
                        _ => "rollup",
                    };
                    for record in self.collection(name, None)? {
                        let found = reference_of(&record, singular)?;
                        if entries.iter().any(|(existing, _)| existing == &found) {
                            return Err(format!(
                                "duplicate stable reference {}",
                                py_repr(&Value::String(found))
                            ));
                        }
                        entries.push((found, singular.to_string()));
                    }
                }
                Ok(entries
                    .into_iter()
                    .find(|(found, _)| found == reference)
                    .map(|(_, kind)| kind))
            }
        }
    }

    fn validated_agent_filter(&self, filters: &Filters) -> Result<Option<String>, String> {
        let Some(reference) = &filters.agent else {
            return Ok(None);
        };
        match self.lookup(reference)? {
            None => Err(format!(
                "unknown stable reference {}",
                py_repr(&Value::String(reference.clone()))
            )),
            Some(kind) if kind != "agent" => {
                Err("--agent must be an agent:... reference".to_string())
            }
            Some(_) => Ok(Some(reference.clone())),
        }
    }

    fn team_range(&self, slug: &str) -> Result<Option<(i64, i64)>, String> {
        let mut intervals = Vec::new();
        for record in self.for_team("agents", slug)? {
            let reference = agent_ref(&record, "agent")?;
            intervals.push(interval(&record, &format!("agent {reference}"))?);
        }
        if intervals.is_empty() {
            for record in self.for_team("rollups", slug)? {
                let reference = rollup_ref(&record, "rollup")?;
                intervals.push(interval(&record, &format!("rollup {reference}"))?);
            }
        }
        if intervals.is_empty() {
            return Ok(None);
        }
        let start = intervals.iter().map(|(start, _)| *start).min().unwrap_or(0);
        let end = intervals.iter().map(|(_, end)| *end).max().unwrap_or(0);
        Ok(Some((start, end)))
    }

    /// `TimelineQuery.list_records` for one resource.
    pub fn list_records(
        &self,
        resource: &str,
        filters: &Filters,
    ) -> Result<Vec<Map<String, Value>>, String> {
        let window = parse_window(filters)?;
        let selected_agent = self.validated_agent_filter(filters)?;
        let selected =
            |team: &str| filters.teams.is_empty() || filters.teams.iter().any(|t| t == team);
        let mut results: Vec<Map<String, Value>> = Vec::new();
        match resource {
            "teams" => {
                for (index, record) in self.teams.clone().iter().enumerate() {
                    let slug = as_string(
                        record.get("slug").unwrap_or(&Value::Null),
                        &format!("timeline.teams[{index}].slug"),
                    )?
                    .to_string();
                    let range = self.team_range(&slug)?;
                    if !selected(&slug) {
                        continue;
                    }
                    if let (Some((start, end)), Some(window)) = (range, &window) {
                        if !window.overlaps(start, end) {
                            continue;
                        }
                    }
                    let mut result = copy_fields(
                        record,
                        &["slug", "label", "provider", "projects", "hosts", "stats"],
                    );
                    result.insert("ref".to_string(), Value::String(format!("team:{slug}")));
                    if let Some((start, end)) = range {
                        result.insert("start_ms".to_string(), Value::from(start));
                        result.insert("end_ms".to_string(), Value::from(end));
                        result.insert("start_time".to_string(), Value::String(timestamp(start)));
                        result.insert("end_time".to_string(), Value::String(timestamp(end)));
                    }
                    results.push(result);
                }
            }
            "agents" => {
                for (index, record) in self.scoped("agents", filters)?.iter().enumerate() {
                    let where_ = format!("timeline.agents[{index}]");
                    let reference = agent_ref(record, &where_)?;
                    let team = team_of(record, &where_)?;
                    if !selected(&team) {
                        continue;
                    }
                    if let Some(agent) = &selected_agent {
                        if &reference != agent {
                            continue;
                        }
                    }
                    if let Some(window) = &window {
                        let (start, end) = interval(record, &where_)?;
                        if !window.overlaps(start, end) {
                            continue;
                        }
                    }
                    let mut result = copy_fields(
                        record,
                        &[
                            "team",
                            "short_name",
                            "official_name",
                            "nickname",
                            "lifetime_summary",
                            "summary_available",
                            "depth",
                            "status",
                            "start_ms",
                            "end_ms",
                        ],
                    );
                    result.insert("ref".to_string(), Value::String(reference));
                    let parent_ref = match record.get("parent_id") {
                        None | Some(Value::Null) => Value::Null,
                        Some(parent) => {
                            let parent = as_string(parent, &format!("{where_}.parent_id"))?;
                            Value::String(format!(
                                "agent:{team}::{}",
                                local_identifier(&team, parent)
                            ))
                        }
                    };
                    result.insert("parent_ref".to_string(), parent_ref);
                    with_times(&mut result, record, &where_)?;
                    results.push(result);
                }
            }
            "phases" => {
                for (index, record) in self.scoped("phases", filters)?.iter().enumerate() {
                    let where_ = format!("timeline.phases[{index}]");
                    let team = team_of(record, &where_)?;
                    let agent_id = as_string(
                        record.get("agent_id").unwrap_or(&Value::Null),
                        &format!("{where_}.agent_id"),
                    )?;
                    let current_agent =
                        format!("agent:{team}::{}", local_identifier(&team, agent_id));
                    if !selected(&team) {
                        continue;
                    }
                    if let Some(agent) = &selected_agent {
                        if &current_agent != agent {
                            continue;
                        }
                    }
                    if let Some(window) = &window {
                        let (start, end) = interval(record, &where_)?;
                        if !window.overlaps(start, end) {
                            continue;
                        }
                    }
                    let mut result = copy_fields(
                        record,
                        &[
                            "team",
                            "phrase",
                            "paragraph",
                            "summary_available",
                            "stats",
                            "start_ms",
                            "end_ms",
                        ],
                    );
                    result.insert(
                        "ref".to_string(),
                        Value::String(phase_ref(record, &where_)?),
                    );
                    result.insert("agent_ref".to_string(), Value::String(current_agent));
                    with_times(&mut result, record, &where_)?;
                    results.push(result);
                }
            }
            "rollups" => {
                for (index, record) in self.scoped("rollups", filters)?.iter().enumerate() {
                    let where_ = format!("timeline.rollups[{index}]");
                    let team = team_of(record, &where_)?;
                    let kind = as_string(
                        record.get("kind").unwrap_or(&Value::Null),
                        &format!("{where_}.kind"),
                    )?;
                    if !selected(&team) {
                        continue;
                    }
                    if !filters.kinds.is_empty() && !filters.kinds.iter().any(|k| k == kind) {
                        continue;
                    }
                    if let Some(window) = &window {
                        let (start, end) = interval(record, &where_)?;
                        if !window.overlaps(start, end) {
                            continue;
                        }
                    }
                    let mut result = copy_fields(
                        record,
                        &[
                            "team",
                            "kind",
                            "label",
                            "start_ms",
                            "end_ms",
                            "technical_path",
                            "plain_language_path",
                            "technical_summary_available",
                            "plain_language_summary_available",
                            "summary_available",
                            "stats",
                        ],
                    );
                    result.insert(
                        "ref".to_string(),
                        Value::String(rollup_ref(record, &where_)?),
                    );
                    with_times(&mut result, record, &where_)?;
                    results.push(result);
                }
            }
            other => {
                return Err(format!(
                    "unsupported query resource {}",
                    py_repr(&Value::String(other.to_string()))
                ))
            }
        }
        results.sort_by(|left, right| {
            let key = |item: &Map<String, Value>| {
                (
                    match optional_int(item, "start_ms") {
                        Some(value) if value != 0 => value,
                        _ => -1,
                    },
                    optional_string(item, "ref").unwrap_or("").to_string(),
                )
            };
            key(left).cmp(&key(right))
        });
        Ok(results)
    }
}

/// `format_query` for one list command.
pub fn format_query(
    command: &str,
    items: &[Map<String, Value>],
    format: &str,
) -> Result<String, String> {
    match format {
        "json" => {
            let envelope = serde_json::json!({
                "schema_version": 1,
                "command": command,
                "count": items.len(),
                "items": items,
            });
            Ok(dump_canonical(&envelope))
        }
        "jsonl" => {
            let mut out = String::new();
            for item in items {
                out.push_str(&dump_oneline(&Value::Object(item.clone())));
                out.push('\n');
            }
            Ok(out)
        }
        "markdown" => {
            let body = items
                .iter()
                .map(markdown_item)
                .collect::<Vec<_>>()
                .join("\n\n");
            Ok(format!(
                "# wrkviz query: {command}\n\n{body}\n\n_{} result(s)._\n",
                items.len()
            ))
        }
        "text" => {
            let body = items.iter().map(text_item).collect::<Vec<_>>().join("\n\n");
            Ok(if body.is_empty() {
                body
            } else {
                format!("{body}\n")
            })
        }
        other => Err(format!(
            "unsupported query output format {}",
            py_repr(&Value::String(other.to_string()))
        )),
    }
}

fn markdown_title(item: &Map<String, Value>) -> String {
    for key in ["short_name", "label", "phrase", "slug", "ref"] {
        if let Some(value) = optional_string(item, key) {
            if !value.is_empty() {
                return value.replace('\n', " ");
            }
        }
    }
    "Result".to_string()
}

fn title_case(key: &str) -> String {
    key.replace('_', " ")
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn markdown_item(item: &Map<String, Value>) -> String {
    let mut lines = vec![format!("## {}", markdown_title(item)), String::new()];
    for key in [
        "ref",
        "record_type",
        "team",
        "provider",
        "kind",
        "agent_ref",
        "parent_ref",
        "start_time",
        "end_time",
        "at_time",
        "field",
        "summary_available",
        "technical_summary_available",
        "plain_language_summary_available",
    ] {
        match item.get(key) {
            Some(Value::String(text)) => lines.push(format!("- {}: `{text}`", title_case(key))),
            Some(Value::Number(number)) => lines.push(format!("- {}: `{number}`", title_case(key))),
            Some(Value::Bool(flag)) => {
                let rendered = if *flag { "True" } else { "False" };
                lines.push(format!("- {}: `{rendered}`", title_case(key)))
            }
            _ => {}
        }
    }
    lines.push(String::new());
    for key in [
        "lifetime_summary",
        "paragraph",
        "excerpt",
        "technical_markdown",
        "plain_language_markdown",
    ] {
        let Some(text) = optional_string(item, key) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if ["technical_markdown", "plain_language_markdown"].contains(&key) {
            lines.push(format!("### {}", title_case(key)));
            lines.push(String::new());
        }
        lines.push(text.trim_end().to_string());
        lines.push(String::new());
    }
    if let Some(detail) = item.get("detail").and_then(Value::as_object) {
        if let Some(transcript) = detail.get("transcript").and_then(Value::as_array) {
            lines.push("### Transcript".to_string());
            lines.push(String::new());
            for raw in transcript {
                let message = raw.as_object();
                let role = message
                    .and_then(|m| m.get("role"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let at_ms = message
                    .and_then(|m| m.get("at_ms"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let text = message
                    .and_then(|m| m.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                lines.push(format!("#### {role} · {}", timestamp(at_ms)));
                lines.push(String::new());
                lines.push(text.trim_end().to_string());
                lines.push(String::new());
            }
        }
    }
    lines.join("\n").trim_end().to_string()
}

fn text_item(item: &Map<String, Value>) -> String {
    let record_type = optional_string(item, "record_type").unwrap_or("record");
    let ordinal = optional_int(item, "ordinal").or_else(|| optional_int(item, "prompt_ordinal"));
    let at = optional_string(item, "timestamp_local")
        .or_else(|| optional_string(item, "at_time"))
        .or_else(|| optional_string(item, "start_time"))
        .unwrap_or("unknown time");
    let team = optional_string(item, "team_slug")
        .or_else(|| optional_string(item, "team"))
        .unwrap_or("unknown team");
    let number = ordinal
        .map(|value| format!(" #{value}"))
        .unwrap_or_default();
    let header = format!("[{record_type}{number} · {at} · {team}]");
    let body = optional_string(item, "text")
        .or_else(|| optional_string(item, "excerpt"))
        .or_else(|| optional_string(item, "paragraph"))
        .map(str::to_string)
        .unwrap_or_else(|| dump_canonical(&Value::Object(item.clone())));
    format!("{header}\n{}", body.trim_end())
}
