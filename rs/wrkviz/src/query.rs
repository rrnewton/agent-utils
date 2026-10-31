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
    as_array, as_int, as_object, as_string, dump_canonical, dump_oneline, parse_json_lenient,
    py_number_str, py_repr, resolve_path,
};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

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
    // CPython's `fromisoformat` refuses a leap second and a
    // clock field out of range, but accepts an offset minute
    // of 60 (an hour by another spelling); chrono does the
    // opposite on both. Validate the fields and normalize
    // the offset before chrono sees them.
    let field = |range: std::ops::Range<usize>| raw[range].parse::<i64>().unwrap_or(99);
    if field(11..13) > 23 || field(14..16) > 59 || field(17..19) > 59 {
        return Err(message);
    }
    let fraction = &raw[19..raw.len() - rest.len()];
    let normalized = if rest == "Z" {
        format!("{}+00:00", &raw[..raw.len() - 1])
    } else {
        let sign: i64 = if rest.starts_with('-') { -1 } else { 1 };
        let offset_hour: i64 = rest[1..3].parse().unwrap_or(99);
        let offset_minute: i64 = rest[4..6].parse().unwrap_or(99);
        let total = sign * (offset_hour * 60 + offset_minute);
        if !(-1440..1440).contains(&total) {
            return Err(message);
        }
        format!(
            "{}{}{}{:02}:{:02}",
            &raw[..19],
            fraction,
            if total < 0 { "-" } else { "+" },
            total.abs() / 60,
            total.abs() % 60
        )
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(&normalized).map_err(|_| message.clone())?;
    Ok(parsed.timestamp_millis())
}

/// An integer JSON value as a normalized decimal string
/// (`as_int` without the `i64` ceiling: CPython integers are
/// arbitrary precision, and a timestamp beyond `i64` is a
/// value to compare, not a refusal).
fn int_literal(value: &Value, where_: &str) -> Result<String, String> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        _ => return Err(format!("{where_}: expected an integer")),
    };
    let digits = raw.strip_prefix('-').unwrap_or(&raw);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{where_}: expected an integer"));
    }
    Ok(dec_normalize(&raw))
}

fn dec_normalize(raw: &str) -> String {
    let (negative, digits) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let magnitude = digits.trim_start_matches('0');
    if magnitude.is_empty() {
        return "0".to_string();
    }
    if negative {
        format!("-{magnitude}")
    } else {
        magnitude.to_string()
    }
}

fn dec_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (left_neg, left_mag) = (left.starts_with('-'), left.trim_start_matches('-'));
    let (right_neg, right_mag) = (right.starts_with('-'), right.trim_start_matches('-'));
    let left_mag = left_mag.trim_start_matches('0');
    let right_mag = right_mag.trim_start_matches('0');
    let (left_mag, right_mag) = (
        if left_mag.is_empty() { "0" } else { left_mag },
        if right_mag.is_empty() { "0" } else { right_mag },
    );
    let (left_neg, right_neg) = (left_neg && left_mag != "0", right_neg && right_mag != "0");
    if left_neg != right_neg {
        return if left_neg {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let magnitude_order = left_mag
        .len()
        .cmp(&right_mag.len())
        .then_with(|| left_mag.cmp(right_mag));
    if left_neg {
        magnitude_order.reverse()
    } else {
        magnitude_order
    }
}

fn dec_add_one(value: &str) -> String {
    let (negative, magnitude) = (value.starts_with('-'), value.trim_start_matches('-'));
    let magnitude = magnitude.trim_start_matches('0');
    let magnitude = if magnitude.is_empty() { "0" } else { magnitude };
    if negative && magnitude != "0" {
        // -(n) + 1 = -(n - 1)
        let mut digits: Vec<u8> = magnitude.bytes().collect();
        let mut index = digits.len();
        loop {
            index -= 1;
            if digits[index] == b'0' {
                digits[index] = b'9';
            } else {
                digits[index] -= 1;
                break;
            }
        }
        let reduced = String::from_utf8(digits).unwrap_or_default();
        let reduced = reduced.trim_start_matches('0');
        if reduced.is_empty() {
            return "0".to_string();
        }
        return format!("-{reduced}");
    }
    let mut digits: Vec<u8> = magnitude.bytes().collect();
    let mut carry = true;
    for digit in digits.iter_mut().rev() {
        if !carry {
            break;
        }
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            carry = false;
        }
    }
    let mut out = String::from_utf8(digits).unwrap_or_default();
    if carry {
        out.insert(0, '1');
    }
    out
}

impl Window {
    fn contains_dec(&self, timestamp: &str) -> bool {
        if let Some(start) = self.start {
            if dec_cmp(timestamp, &start.to_string()) == std::cmp::Ordering::Less {
                return false;
            }
        }
        match self.end {
            None => true,
            Some(end) => dec_cmp(timestamp, &end.to_string()) == std::cmp::Ordering::Less,
        }
    }

    fn overlaps_dec(&self, start: &str, end: &str) -> bool {
        let effective_end = if dec_cmp(end, start) != std::cmp::Ordering::Greater {
            dec_add_one(start)
        } else {
            end.to_string()
        };
        if let Some(limit) = self.end {
            if dec_cmp(start, &limit.to_string()) != std::cmp::Ordering::Less {
                return false;
            }
        }
        match self.start {
            None => true,
            Some(limit) => {
                dec_cmp(&effective_end, &limit.to_string()) == std::cmp::Ordering::Greater
            }
        }
    }
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
type LookupEntry = (String, Map<String, Value>);

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
    let start_ms = int_literal(
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

fn sorted_refs(mut references: Vec<String>) -> Value {
    references.sort();
    Value::Array(references.into_iter().map(Value::String).collect())
}

/// `PurePosixPath(name).suffix`: empty for a dotfile name
/// like `.json`, where the dot leads the name.
fn posix_suffix(parts: &[String]) -> &str {
    let Some(name) = parts.last() else {
        return "";
    };
    match name.rfind('.') {
        Some(at) if at > 0 => &name[at..],
        _ => "",
    }
}

fn summary_available(record: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match record.get(key) {
        None => Ok(true),
        Some(Value::Bool(available)) => Ok(*available),
        Some(_) => Err(format!("{key}: expected a boolean")),
    }
}

fn agent_reference_for_id(team: &str, identifier: &str) -> String {
    format!("agent:{team}::{}", local_identifier(team, identifier))
}

fn same_agent_identifier(team: &str, left: &str, right: &str) -> bool {
    local_identifier(team, left) == local_identifier(team, right)
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

    /// `TimelineQuery._project_overviews` for the selected teams.
    fn project_overviews(&self, wanted: &[String]) -> Result<Vec<Map<String, Value>>, String> {
        match &self.generation {
            Generation::Three(three) => self.spine(three, "project_overview", wanted),
            Generation::One(source) => {
                if let Some(plural) = source.get("project_overviews") {
                    if !plural.is_null() {
                        let raw = as_array(plural, "timeline.project_overviews")?;
                        let mut out = Vec::new();
                        for (index, value) in raw.iter().enumerate() {
                            out.push(
                                as_object(value, &format!("timeline.project_overviews[{index}]"))?
                                    .clone(),
                            );
                        }
                        return Ok(out);
                    }
                }
                let Some(singular) = source.get("project_overview") else {
                    return Ok(Vec::new());
                };
                if singular.is_null() {
                    return Ok(Vec::new());
                }
                if self.teams.len() != 1 {
                    return Err(
                        "timeline.project_overview requires exactly one timeline team".to_string(),
                    );
                }
                let mut result = as_object(singular, "timeline.project_overview")?.clone();
                let slug = as_string(
                    self.teams[0].get("slug").unwrap_or(&Value::Null),
                    "timeline.teams[0].slug",
                )?
                .to_string();
                result.insert("team".to_string(), Value::String(slug));
                Ok(vec![result])
            }
        }
    }

    /// `TimelineQuery.summary_stats`: availability and text
    /// totals for every generated-summary surface.
    pub fn summary_stats(
        &self,
        filters: &Filters,
    ) -> Result<Vec<(String, SummaryKindStats)>, String> {
        let window = parse_window(filters)?;
        let selected =
            |team: &str| filters.teams.is_empty() || filters.teams.iter().any(|t| t == team);
        let overlaps = |record: &Map<String, Value>, reference: &str| -> Result<bool, String> {
            match &window {
                None => Ok(true),
                Some(window) => {
                    let start = int_literal(
                        record.get("start_ms").unwrap_or(&Value::Null),
                        &format!("{reference}.start_ms"),
                    )?;
                    let end = int_literal(
                        record.get("end_ms").unwrap_or(&Value::Null),
                        &format!("{reference}.end_ms"),
                    )?;
                    Ok(window.overlaps_dec(&start, &end))
                }
            }
        };
        let mut project_texts: Vec<String> = Vec::new();
        let mut project_unavailable = 0i64;
        if window.is_none() {
            for record in self.project_overviews(&filters.teams)? {
                let team = team_of(&record, "project_overview")?;
                if !selected(&team) {
                    continue;
                }
                if summary_available(&record, "summary_available")? {
                    project_texts.push(
                        as_string(
                            record.get("text").unwrap_or(&Value::Null),
                            "project_overview.text",
                        )?
                        .to_string(),
                    );
                } else {
                    project_unavailable += 1;
                }
            }
        }
        let mut agent_texts: Vec<String> = Vec::new();
        let mut agent_unavailable = 0i64;
        for record in self.scoped("agents", filters)? {
            let team = team_of(&record, "agent")?;
            if !selected(&team) {
                continue;
            }
            let reference = agent_ref(&record, "agent")?;
            if !overlaps(&record, &reference)? {
                continue;
            }
            if summary_available(&record, "summary_available")? {
                agent_texts.push(
                    as_string(
                        record.get("lifetime_summary").unwrap_or(&Value::Null),
                        "agent.lifetime_summary",
                    )?
                    .to_string(),
                );
            } else {
                agent_unavailable += 1;
            }
        }
        let mut phase_texts: Vec<String> = Vec::new();
        let mut phase_unavailable = 0i64;
        for record in self.scoped("phases", filters)? {
            let team = team_of(&record, "phase")?;
            if !selected(&team) {
                continue;
            }
            let reference = phase_ref(&record, "phase")?;
            if !overlaps(&record, &reference)? {
                continue;
            }
            if summary_available(&record, "summary_available")? {
                let phrase =
                    as_string(record.get("phrase").unwrap_or(&Value::Null), "phase.phrase")?;
                let paragraph = as_string(
                    record.get("paragraph").unwrap_or(&Value::Null),
                    "phase.paragraph",
                )?;
                phase_texts.push(format!("{phrase}\n{paragraph}"));
            } else {
                phase_unavailable += 1;
            }
        }
        let mut technical_texts: Vec<String> = Vec::new();
        let mut technical_unavailable = 0i64;
        let mut plain_texts: Vec<String> = Vec::new();
        let mut plain_unavailable = 0i64;
        for record in self.scoped("rollups", filters)? {
            let team = team_of(&record, "rollup")?;
            let kind =
                as_string(record.get("kind").unwrap_or(&Value::Null), "rollup.kind")?.to_string();
            let reference = rollup_ref(&record, "rollup")?;
            if !selected(&team) {
                continue;
            }
            if !filters.kinds.is_empty() && !filters.kinds.iter().any(|k| k == &kind) {
                continue;
            }
            if !overlaps(&record, &reference)? {
                continue;
            }
            if summary_available(&record, "technical_summary_available")? {
                let relative = as_string(
                    record.get("technical_path").unwrap_or(&Value::Null),
                    "rollup.technical_path",
                )?;
                technical_texts.push(self.read_markdown(&self.rollup_file(relative, &team)?)?);
            } else {
                technical_unavailable += 1;
            }
            if summary_available(&record, "plain_language_summary_available")? {
                let relative = as_string(
                    record.get("plain_language_path").unwrap_or(&Value::Null),
                    "rollup.plain_language_path",
                )?;
                plain_texts.push(self.read_markdown(&self.rollup_file(relative, &team)?)?);
            } else {
                plain_unavailable += 1;
            }
        }
        Ok(vec![
            (
                "project_overviews".to_string(),
                SummaryKindStats::from_texts(project_texts, project_unavailable),
            ),
            (
                "agent_lifetimes".to_string(),
                SummaryKindStats::from_texts(agent_texts, agent_unavailable),
            ),
            (
                "work_phases".to_string(),
                SummaryKindStats::from_texts(phase_texts, phase_unavailable),
            ),
            (
                "rollup_technical".to_string(),
                SummaryKindStats::from_texts(technical_texts, technical_unavailable),
            ),
            (
                "rollup_plain_language".to_string(),
                SummaryKindStats::from_texts(plain_texts, plain_unavailable),
            ),
        ])
    }

    fn read_stream_records(
        &self,
        entry: &crate::inspect::ShardEntry,
    ) -> Result<Vec<Map<String, Value>>, String> {
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
        let mut records = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let value = parse_json_lenient(line)?;
            records.push(as_object(&value, &entry.path)?.clone());
        }
        Ok(records)
    }

    fn unwrap_record(
        record: &Map<String, Value>,
        kind: &str,
        team: &str,
        where_: &str,
    ) -> Result<Map<String, Value>, String> {
        let mut record = record.clone();
        let found = record.remove("record_kind").unwrap_or(Value::Null);
        if found.as_str() != Some(kind) {
            return Err(format!(
                "{where_}: expected a {} record, found {}",
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
                        "{where_}: record names team {} in {}'s spine shard",
                        py_repr(declared),
                        py_repr(&Value::String(team.to_string()))
                    ));
                }
            }
        }
        Ok(record)
    }

    fn has_search_corpus(three: &SchemaThree) -> bool {
        three
            .shards
            .iter()
            .any(|entry| entry.stream == "search" && entry.team.is_some() && entry.day.is_some())
    }

    fn search_blooms(
        &self,
        three: &SchemaThree,
        wanted: &[String],
    ) -> Result<std::collections::BTreeMap<String, Option<Value>>, String> {
        let archive_teams: Vec<String> = {
            let mut teams: Vec<String> = three
                .shards
                .iter()
                .filter(|entry| entry.stream == "spine")
                .filter_map(|entry| entry.team.clone())
                .collect();
            teams.sort();
            teams.dedup();
            teams
        };
        let in_scope: Vec<String> = if wanted.is_empty() {
            archive_teams.clone()
        } else {
            archive_teams
                .iter()
                .filter(|team| wanted.iter().any(|item| item == *team))
                .cloned()
                .collect()
        };
        let mut blooms = std::collections::BTreeMap::new();
        for team in in_scope {
            let Some(entry) = three.shards.iter().find(|entry| {
                entry.stream == "search_bloom" && entry.team.as_deref() == Some(&team)
            }) else {
                continue;
            };
            let owned: std::collections::BTreeSet<&str> = three
                .shards
                .iter()
                .filter(|item| item.stream == "search" && item.team.as_deref() == Some(&team))
                .map(|item| item.path.as_str())
                .collect();
            for record in self.read_stream_records(entry)? {
                let where_ = format!("{} bloom", entry.path);
                let record = Self::unwrap_record(&record, "search_bloom", &team, &where_)?;
                let shard = as_string(
                    record.get("shard").unwrap_or(&Value::Null),
                    &format!("{where_}.shard"),
                )?;
                if !owned.contains(shard) {
                    return Err(format!(
                        "{where_}: prefilter names {}, which is not a search shard of team {}",
                        py_repr(&Value::String(shard.to_string())),
                        py_repr(&Value::String(team.clone()))
                    ));
                }
                blooms.insert(shard.to_string(), record.get("bloom").cloned());
            }
        }
        Ok(blooms)
    }

    fn validated_search_record(
        &self,
        record: &Map<String, Value>,
        where_: &str,
        shard_team: &str,
        shard_start: i64,
        shard_end: i64,
        seen: &mut std::collections::BTreeSet<String>,
    ) -> Result<Map<String, Value>, String> {
        let reference = as_string(
            record.get("ref").unwrap_or(&Value::Null),
            &format!("{where_}.ref"),
        )?
        .to_string();
        if as_int(
            record.get("schema_version").unwrap_or(&Value::Null),
            &format!("{where_}.schema_version"),
        )? != 1
        {
            return Err(format!("unsupported transcript search record: {reference}"));
        }
        let record_team = as_string(
            record.get("team").unwrap_or(&Value::Null),
            &format!("{where_}.team"),
        )?;
        let at_ms = as_int(
            record.get("at_ms").unwrap_or(&Value::Null),
            &format!("{where_}.at_ms"),
        )?;
        if record_team != shard_team || !(shard_start <= at_ms && at_ms < shard_end) {
            return Err(format!(
                "transcript search record escapes shard: {reference}"
            ));
        }
        let message_prefix = format!("message:{record_team}::");
        let tool_prefix = format!("tool:{record_team}::");
        if !reference.starts_with(&message_prefix) && !reference.starts_with(&tool_prefix) {
            return Err(format!(
                "invalid transcript search reference {}",
                py_repr(&Value::String(reference.clone()))
            ));
        }
        let record_type = as_string(
            record.get("record_type").unwrap_or(&Value::Null),
            &format!("{where_}.record_type"),
        )?;
        let event_identifier = as_string(
            record.get("event_id").unwrap_or(&Value::Null),
            &format!("{where_}.event_id"),
        )?;
        let expected = format!(
            "{}{}",
            if record_type == "tool" {
                &tool_prefix
            } else {
                &message_prefix
            },
            event_identifier
        );
        if reference != expected {
            return Err(format!(
                "transcript search reference kind mismatch: {reference}"
            ));
        }
        let agent_reference = as_string(
            record.get("agent_ref").unwrap_or(&Value::Null),
            &format!("{where_}.agent_ref"),
        )?
        .to_string();
        let agent_identifier = as_string(
            record.get("agent_id").unwrap_or(&Value::Null),
            &format!("{where_}.agent_id"),
        )?;
        if agent_reference_for_id(record_team, agent_identifier) != agent_reference {
            return Err(format!(
                "transcript search record agent identity mismatch: {reference}"
            ));
        }
        let Some((agent_kind, agent_record)) = self.lookup_entry(&agent_reference)? else {
            return Err(format!(
                "transcript search record has unknown agent {}",
                py_repr(&Value::String(agent_reference.clone()))
            ));
        };
        if agent_kind != "agent" {
            return Err(format!(
                "transcript search record has unknown agent {}",
                py_repr(&Value::String(agent_reference.clone()))
            ));
        }
        if team_of(&agent_record, "search record agent")? != record_team {
            return Err(format!(
                "transcript search record agent belongs to another team: {reference}"
            ));
        }
        if let Some(prompt_reference) = record.get("prompt_ref") {
            if !prompt_reference.is_null() {
                let prompt_reference =
                    as_string(prompt_reference, &format!("{where_}.prompt_ref"))?;
                if !prompt_reference.starts_with(&message_prefix) {
                    return Err(format!(
                        "transcript search prompt reference belongs to another team: {reference}"
                    ));
                }
            }
        }
        if let Some(in_scope) = record.get("prompt_in_scope") {
            if !in_scope.is_null() && !matches!(in_scope, Value::Bool(_)) {
                return Err(format!("{where_}.prompt_in_scope: expected a boolean"));
            }
        }
        let role = as_string(
            record.get("role").unwrap_or(&Value::Null),
            &format!("{where_}.role"),
        )?;
        if ![
            "user",
            "assistant",
            "agent",
            "system",
            "external",
            "goal",
            "tool",
            "event",
        ]
        .contains(&role)
        {
            return Err(format!(
                "unsupported transcript search role {}",
                py_repr(&Value::String(role.to_string()))
            ));
        }
        as_string(
            record.get("text").unwrap_or(&Value::Null),
            &format!("{where_}.text"),
        )?;
        if seen.contains(&reference) {
            return Err(format!(
                "duplicate transcript search reference {}",
                py_repr(&Value::String(reference.clone()))
            ));
        }
        seen.insert(reference);
        Ok(record.clone())
    }

    fn iter_search_records(
        &self,
        filters: Option<&Filters>,
        bloom_terms: &[String],
    ) -> Result<Vec<Map<String, Value>>, String> {
        match &self.generation {
            Generation::Three(three) if Self::has_search_corpus(three) => {
                let timeline_teams: std::collections::BTreeSet<String> = self
                    .teams
                    .iter()
                    .map(|team| {
                        as_string(
                            team.get("slug").unwrap_or(&Value::Null),
                            "timeline team.slug",
                        )
                        .map(str::to_string)
                    })
                    .collect::<Result<_, _>>()?;
                let blooms = if bloom_terms.iter().any(|term| bloom_can_prune(term)) {
                    self.search_blooms(
                        three,
                        filters.map(|item| item.teams.as_slice()).unwrap_or(&[]),
                    )?
                } else {
                    std::collections::BTreeMap::new()
                };
                let window = match filters {
                    None => None,
                    Some(filters) => parse_window(filters)?,
                };
                let mut seen = std::collections::BTreeSet::new();
                let mut out = Vec::new();
                for entry in three.shards.iter().filter(|entry| entry.stream == "search") {
                    let where_ = format!("timeline-v3.streams.search {}", entry.path);
                    let (Some(shard_team), Some(day)) =
                        (entry.team.as_deref(), entry.day.as_deref())
                    else {
                        return Err(format!("invalid transcript search shard scope at {where_}"));
                    };
                    if !timeline_teams.contains(shard_team) {
                        return Err(format!("invalid transcript search shard scope at {where_}"));
                    }
                    let (shard_start, shard_end) = day_bounds(day, &where_)?;
                    if let Some(filters) = filters {
                        if !filters.teams.is_empty()
                            && !filters.teams.iter().any(|team| team == shard_team)
                        {
                            continue;
                        }
                    }
                    if let Some(window) = &window {
                        if !window.overlaps(shard_start, shard_end) {
                            continue;
                        }
                    }
                    if !blooms.is_empty() {
                        let raw = blooms.get(&entry.path).cloned().unwrap_or(None);
                        if !bloom_might_match(
                            raw.as_ref(),
                            bloom_terms,
                            &format!("{where_}.bloom"),
                        )? {
                            continue;
                        }
                    }
                    let records = self.read_stream_records(entry)?;
                    if records.len() as i64 != entry.records {
                        return Err(format!(
                            "transcript search shard count mismatch: {} holds {} records, the bootstrap says {}",
                            entry.path,
                            records.len(),
                            entry.records
                        ));
                    }
                    for (index, record) in records.iter().enumerate() {
                        let record =
                            Self::unwrap_record(record, "search_record", shard_team, &entry.path)?;
                        out.push(self.validated_search_record(
                            &record,
                            &format!("{}.records[{index}]", entry.path),
                            shard_team,
                            shard_start,
                            shard_end,
                            &mut seen,
                        )?);
                    }
                }
                Ok(out)
            }
            _ => {
                let bootstrap = self.root.join("data").join("timeline-v2.json");
                if !bootstrap.is_file() {
                    return Err(
                        "this archive has no transcript search corpus; rebuild the website"
                            .to_string(),
                    );
                }
                Err("schema 2 transcript search is not yet ported in the Rust edition".to_string())
            }
        }
    }

    fn search_link_context(
        &self,
        filters: &Filters,
        selected_agent: Option<&str>,
        candidate_prompt_refs: &std::collections::BTreeSet<String>,
        needed_prompt_refs: &std::collections::BTreeSet<String>,
    ) -> Result<SearchLinkContext, String> {
        let mut prompt_excerpts = std::collections::BTreeMap::new();
        let mut response_counts: std::collections::BTreeMap<String, i64> =
            std::collections::BTreeMap::new();
        if candidate_prompt_refs.is_empty() && needed_prompt_refs.is_empty() {
            return Ok(SearchLinkContext {
                prompt_excerpts,
                response_counts,
            });
        }
        let Generation::Three(three) = &self.generation else {
            // Without a schema-3 corpus the context comes from
            // the same records the search walked.
            let team_filters = Filters {
                teams: filters.teams.clone(),
                start_time: None,
                end_time: None,
                kinds: Vec::new(),
                agent: None,
            };
            for record in self.iter_search_records(Some(&team_filters), &[])? {
                let reference = as_string(
                    record.get("ref").unwrap_or(&Value::Null),
                    "search record.ref",
                )?
                .to_string();
                let record_type = as_string(
                    record.get("record_type").unwrap_or(&Value::Null),
                    "search record.record_type",
                )?;
                if needed_prompt_refs.contains(&reference)
                    && ["prompt", "inter_agent_prompt"].contains(&record_type)
                {
                    let text = as_string(
                        record.get("text").unwrap_or(&Value::Null),
                        "search record.text",
                    )?;
                    let compact: String = compact_text(text).chars().take(320).collect();
                    prompt_excerpts.insert(reference.clone(), compact);
                }
                if !["response", "inter_agent_response"].contains(&record_type) {
                    continue;
                }
                let prompt_reference = optional_string(&record, "prompt_ref");
                let Some(prompt_reference) = prompt_reference else {
                    continue;
                };
                if !candidate_prompt_refs.contains(prompt_reference) {
                    continue;
                }
                let at_ms = as_int(
                    record.get("at_ms").unwrap_or(&Value::Null),
                    "search record.at_ms",
                )?;
                let window = parse_window(filters)?;
                if let Some(window) = &window {
                    if !window.contains_dec(&at_ms.to_string()) {
                        continue;
                    }
                }
                let agent_reference = as_string(
                    record.get("agent_ref").unwrap_or(&Value::Null),
                    "search record.agent_ref",
                )?;
                if let Some(selected) = selected_agent {
                    if agent_reference != selected {
                        continue;
                    }
                }
                *response_counts
                    .entry(prompt_reference.to_string())
                    .or_insert(0) += 1;
            }
            return Ok(SearchLinkContext {
                prompt_excerpts,
                response_counts,
            });
        };
        // Schema-3 linkage sidecars.
        let mut days_by_team: std::collections::BTreeMap<String, std::collections::BTreeSet<i64>> =
            std::collections::BTreeMap::new();
        for entry in three.shards.iter().filter(|entry| entry.stream == "search") {
            let (Some(team), Some(day)) = (entry.team.as_deref(), entry.day.as_deref()) else {
                continue;
            };
            let (start_ms, _) =
                day_bounds(day, &format!("timeline-v3.streams.search {}", entry.path))?;
            days_by_team
                .entry(team.to_string())
                .or_default()
                .insert(start_ms);
        }
        let archive_teams: Vec<String> = {
            let mut teams: Vec<String> = three
                .shards
                .iter()
                .filter(|entry| entry.stream == "spine")
                .filter_map(|entry| entry.team.clone())
                .collect();
            teams.sort();
            teams.dedup();
            teams
        };
        let in_scope: Vec<String> = if filters.teams.is_empty() {
            archive_teams.clone()
        } else {
            archive_teams
                .iter()
                .filter(|team| filters.teams.iter().any(|item| item == *team))
                .cloned()
                .collect()
        };
        let mut seen_prompts = std::collections::BTreeSet::new();
        let mut seen_responses = std::collections::BTreeSet::new();
        let window = parse_window(filters)?;
        for team in in_scope {
            let Some(entry) = three.shards.iter().find(|entry| {
                entry.stream == "search_links" && entry.team.as_deref() == Some(&team)
            }) else {
                continue;
            };
            let where_ = format!("timeline-v3.streams.search_links {team}");
            let message_prefix = format!("message:{team}::");
            let agent_prefix = format!("agent:{team}::");
            let days = days_by_team.get(&team).cloned().unwrap_or_default();
            let read_half = |kind: &str| -> Result<Vec<Map<String, Value>>, String> {
                let Some(&(first, count)) = entry.line_ranges.get(kind) else {
                    return Ok(Vec::new());
                };
                let records = self.read_stream_records(entry)?;
                let mut out = Vec::new();
                for offset in 0..count {
                    let Some(record) = records.get((first + offset) as usize) else {
                        return Err(format!(
                            "schema-3 shard {} ended before line {}",
                            entry.path,
                            first + offset
                        ));
                    };
                    out.push(Self::unwrap_record(record, kind, &team, &entry.path)?);
                }
                Ok(out)
            };
            if !needed_prompt_refs.is_empty() {
                for (index, prompt) in read_half("search_prompt")?.iter().enumerate() {
                    let spot = format!("{where_}.prompts[{index}]");
                    let reference = as_string(
                        prompt.get("ref").unwrap_or(&Value::Null),
                        &format!("{spot}.ref"),
                    )?
                    .to_string();
                    if !reference.starts_with(&message_prefix) {
                        return Err(format!(
                            "transcript search linkage prompt belongs to another team: {reference}"
                        ));
                    }
                    let excerpt = as_string(
                        prompt.get("excerpt").unwrap_or(&Value::Null),
                        &format!("{spot}.excerpt"),
                    )?
                    .to_string();
                    if !needed_prompt_refs.contains(&reference) {
                        continue;
                    }
                    if seen_prompts.contains(&reference) {
                        return Err(format!(
                            "duplicate transcript search linkage prompt {}",
                            py_repr(&Value::String(reference.clone()))
                        ));
                    }
                    seen_prompts.insert(reference.clone());
                    prompt_excerpts.insert(reference, excerpt);
                }
            }
            if !candidate_prompt_refs.is_empty() {
                for (index, response) in read_half("search_response")?.iter().enumerate() {
                    let spot = format!("{where_}.responses[{index}]");
                    let reference = as_string(
                        response.get("ref").unwrap_or(&Value::Null),
                        &format!("{spot}.ref"),
                    )?
                    .to_string();
                    if !reference.starts_with(&message_prefix) {
                        return Err(format!(
                            "transcript search linkage response belongs to another team: {reference}"
                        ));
                    }
                    let prompt_reference = as_string(
                        response.get("prompt_ref").unwrap_or(&Value::Null),
                        &format!("{spot}.prompt_ref"),
                    )?
                    .to_string();
                    if !prompt_reference.starts_with(&message_prefix) {
                        return Err(format!(
                            "transcript search linkage prompt belongs to another team: {reference}"
                        ));
                    }
                    let at_ms = as_int(
                        response.get("at_ms").unwrap_or(&Value::Null),
                        &format!("{spot}.at_ms"),
                    )?;
                    if !days.contains(&(at_ms - at_ms.rem_euclid(86_400_000))) {
                        return Err(format!(
                            "transcript search linkage response escapes shard: {reference}"
                        ));
                    }
                    let agent_reference = as_string(
                        response.get("agent_ref").unwrap_or(&Value::Null),
                        &format!("{spot}.agent_ref"),
                    )?
                    .to_string();
                    if !agent_reference.starts_with(&agent_prefix) {
                        return Err(format!(
                            "transcript search linkage agent belongs to another team: {reference}"
                        ));
                    }
                    let Some((agent_kind, agent_record)) = self.lookup_entry(&agent_reference)?
                    else {
                        return Err(format!(
                            "transcript search linkage response has unknown agent {}",
                            py_repr(&Value::String(agent_reference.clone()))
                        ));
                    };
                    if agent_kind != "agent" {
                        return Err(format!(
                            "transcript search linkage response has unknown agent {}",
                            py_repr(&Value::String(agent_reference.clone()))
                        ));
                    }
                    if team_of(&agent_record, "search linkage response agent")? != team {
                        return Err(format!(
                            "transcript search linkage agent belongs to another team: {reference}"
                        ));
                    }
                    if !candidate_prompt_refs.contains(&prompt_reference) {
                        continue;
                    }
                    if let Some(window) = &window {
                        if !window.contains_dec(&at_ms.to_string()) {
                            continue;
                        }
                    }
                    if let Some(selected) = selected_agent {
                        if agent_reference != selected {
                            continue;
                        }
                    }
                    if seen_responses.contains(&reference) {
                        return Err(format!(
                            "duplicate transcript search linkage response {}",
                            py_repr(&Value::String(reference.clone()))
                        ));
                    }
                    seen_responses.insert(reference);
                    *response_counts.entry(prompt_reference).or_insert(0) += 1;
                }
            }
        }
        Ok(SearchLinkContext {
            prompt_excerpts,
            response_counts,
        })
    }

    fn phase_reference_for_search_record(
        &self,
        record: &Map<String, Value>,
    ) -> Result<Option<String>, String> {
        let team = as_string(
            record.get("team").unwrap_or(&Value::Null),
            "search record.team",
        )?
        .to_string();
        let agent_id = as_string(
            record.get("agent_id").unwrap_or(&Value::Null),
            "search record.agent_id",
        )?;
        let at_ms = as_int(
            record.get("at_ms").unwrap_or(&Value::Null),
            "search record.at_ms",
        )?;
        let local = local_identifier(&team, agent_id);
        let mut best: Option<(i64, String)> = None;
        for phase in self.collection("phases", None)? {
            let phase_team = team_of(&phase, "phase")?;
            if phase_team != team {
                continue;
            }
            let phase_agent = as_string(
                phase.get("agent_id").unwrap_or(&Value::Null),
                "phase.agent_id",
            )?;
            if local_identifier(&phase_team, phase_agent) != local {
                continue;
            }
            let (start_ms, end_ms) = interval(&phase, "phase")?;
            if start_ms <= at_ms && at_ms < end_ms {
                let candidate = (end_ms - start_ms, phase_ref(&phase, "phase")?);
                if best.as_ref().is_none_or(|current| candidate < *current) {
                    best = Some(candidate);
                }
            }
        }
        Ok(best.map(|(_, reference)| reference))
    }

    /// `TimelineQuery.search_v2` over the canonical corpus.
    #[allow(clippy::too_many_arguments)]
    pub fn search_v2(
        &self,
        needle: &str,
        corpus: &str,
        filters: &Filters,
        case_sensitive: bool,
        match_mode: &str,
        sort: &str,
        prompt_author: &str,
        linkage: &str,
        roles: &[String],
        offset: i64,
        limit: i64,
    ) -> Result<SearchResults, String> {
        if !["owner-prompts", "agent-responses", "all-transcript"].contains(&corpus) {
            return Err(format!(
                "unsupported search corpus {}",
                py_repr(&Value::String(corpus.to_string()))
            ));
        }
        if !["smart", "phrase", "literal"].contains(&match_mode) {
            return Err(format!(
                "unsupported search match mode {}",
                py_repr(&Value::String(match_mode.to_string()))
            ));
        }
        if !["relevance", "newest", "oldest"].contains(&sort) {
            return Err(format!(
                "unsupported search sort {}",
                py_repr(&Value::String(sort.to_string()))
            ));
        }
        if !["any", "owner", "agent", "unclassified"].contains(&prompt_author) {
            return Err(format!(
                "unsupported prompt author filter {}",
                py_repr(&Value::String(prompt_author.to_string()))
            ));
        }
        if !["any", "linked", "unlinked"].contains(&linkage) {
            return Err(format!(
                "unsupported linkage filter {}",
                py_repr(&Value::String(linkage.to_string()))
            ));
        }
        if roles.iter().any(|role| {
            ![
                "user",
                "assistant",
                "agent",
                "system",
                "external",
                "goal",
                "tool",
                "event",
            ]
            .contains(&role.as_str())
        }) {
            return Err("unsupported transcript role filter".to_string());
        }
        if offset < 0 {
            return Err("--offset must not be negative".to_string());
        }
        if limit < 1 {
            return Err("--limit must be at least 1".to_string());
        }
        let selected_agent = self.validated_agent_filter(filters)?;
        let matcher = compile_search_matcher(needle, match_mode, case_sensitive)?;
        let window = parse_window(filters)?;
        type Candidate = (i64, i64, String, Map<String, Value>, TextMatch);
        let mut candidates: Vec<Candidate> = Vec::new();
        for record in self.iter_search_records(Some(filters), &matcher.bloom_terms)? {
            let reference = as_string(
                record.get("ref").unwrap_or(&Value::Null),
                "search record.ref",
            )?
            .to_string();
            let record_type = as_string(
                record.get("record_type").unwrap_or(&Value::Null),
                "search record.record_type",
            )?
            .to_string();
            let text = as_string(
                record.get("text").unwrap_or(&Value::Null),
                "search record.text",
            )?
            .to_string();
            let team = as_string(
                record.get("team").unwrap_or(&Value::Null),
                "search record.team",
            )?;
            if !filters.teams.is_empty() && !filters.teams.iter().any(|item| item == team) {
                continue;
            }
            let at_ms = as_int(
                record.get("at_ms").unwrap_or(&Value::Null),
                "search record.at_ms",
            )?;
            if let Some(window) = &window {
                if !window.contains_dec(&at_ms.to_string()) {
                    continue;
                }
            }
            let agent_reference = as_string(
                record.get("agent_ref").unwrap_or(&Value::Null),
                "search record.agent_ref",
            )?;
            if let Some(selected) = &selected_agent {
                if agent_reference != selected {
                    continue;
                }
            }
            let role = as_string(
                record.get("role").unwrap_or(&Value::Null),
                "search record.role",
            )?;
            if !roles.is_empty() && !roles.iter().any(|item| item == role) {
                continue;
            }
            let author_kind = optional_string(&record, "author_kind");
            let prompt_kind = optional_string(&record, "prompt_author_kind");
            if corpus == "owner-prompts"
                && !(record_type == "prompt" && author_kind == Some("owner_human"))
            {
                continue;
            }
            if corpus == "agent-responses"
                && !["response", "inter_agent_response"].contains(&record_type.as_str())
            {
                continue;
            }
            let selected_prompt_kind =
                if ["prompt", "inter_agent_prompt"].contains(&record_type.as_str()) {
                    author_kind
                } else {
                    prompt_kind
                };
            if prompt_author == "owner" && selected_prompt_kind != Some("owner_human") {
                continue;
            }
            if prompt_author == "agent"
                && !matches!(selected_prompt_kind, Some(kind) if ["agent", "system"].contains(&kind))
            {
                continue;
            }
            if prompt_author == "unclassified"
                && matches!(selected_prompt_kind, Some(kind) if ["owner_human", "other_human", "agent", "system"].contains(&kind))
            {
                continue;
            }
            let Some(text_match) = matcher.match_text(&text) else {
                continue;
            };
            candidates.push((text_match.score, at_ms, reference, record, text_match));
        }
        let candidate_prompt_refs: std::collections::BTreeSet<String> = candidates
            .iter()
            .filter(|(_, _, _, record, _)| {
                matches!(
                    record.get("record_type").and_then(Value::as_str),
                    Some("prompt") | Some("inter_agent_prompt")
                )
            })
            .map(|(_, _, reference, _, _)| reference.clone())
            .collect();
        let needed_prompt_refs: std::collections::BTreeSet<String> = candidates
            .iter()
            .filter_map(|(_, _, reference, record, _)| {
                optional_string(record, "prompt_ref")
                    .filter(|prompt_ref| *prompt_ref != reference.as_str())
                    .map(str::to_string)
            })
            .collect();
        let SearchLinkContext {
            prompt_excerpts,
            response_counts,
        } = self.search_link_context(
            filters,
            selected_agent.as_deref(),
            &candidate_prompt_refs,
            &needed_prompt_refs,
        )?;
        let mut matches: Vec<(i64, i64, String, Map<String, Value>)> = Vec::new();
        for (score, at_ms, reference, record, text_match) in &candidates {
            let record_type = as_string(
                record.get("record_type").unwrap_or(&Value::Null),
                "search record.record_type",
            )?;
            let prompt_reference = optional_string(record, "prompt_ref");
            let linked = if ["prompt", "inter_agent_prompt"].contains(&record_type) {
                response_counts.get(reference).copied().unwrap_or(0) > 0
            } else {
                prompt_reference.is_some() && prompt_reference != Some(reference.as_str())
            };
            if linkage == "linked" && !linked {
                continue;
            }
            if linkage == "unlinked" && linked {
                continue;
            }
            let excerpt_details = search_excerpt(text_match);
            let excerpt = as_string(
                excerpt_details.get("text").unwrap_or(&Value::Null),
                "search excerpt.text",
            )?
            .to_string();
            let mut item = Map::new();
            item.insert("result_id".to_string(), Value::String(reference.clone()));
            item.insert("ref".to_string(), Value::String(reference.clone()));
            item.insert(
                "record_type".to_string(),
                Value::String(record_type.to_string()),
            );
            item.insert(
                "role".to_string(),
                record.get("role").cloned().unwrap_or(Value::Null),
            );
            item.insert(
                "team".to_string(),
                record.get("team").cloned().unwrap_or(Value::Null),
            );
            item.insert(
                "agent_ref".to_string(),
                record.get("agent_ref").cloned().unwrap_or(Value::Null),
            );
            item.insert(
                "agent_path".to_string(),
                record.get("agent_path").cloned().unwrap_or(Value::Null),
            );
            item.insert("at_ms".to_string(), Value::from(*at_ms));
            item.insert("at_time".to_string(), Value::String(timestamp(*at_ms)));
            item.insert(
                "prompt_ref".to_string(),
                record.get("prompt_ref").cloned().unwrap_or(Value::Null),
            );
            item.insert(
                "prompt_at_ms".to_string(),
                record.get("prompt_at_ms").cloned().unwrap_or(Value::Null),
            );
            item.insert(
                "prompt_in_scope".to_string(),
                record
                    .get("prompt_in_scope")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            item.insert(
                "prompt_author_kind".to_string(),
                record
                    .get("prompt_author_kind")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            item.insert(
                "linked_response_count".to_string(),
                Value::from(response_counts.get(reference).copied().unwrap_or(0)),
            );
            item.insert("score".to_string(), Value::from(*score));
            item.insert(
                "ranking_version".to_string(),
                Value::String("search-rank-v1".to_string()),
            );
            item.insert(
                "content_fidelity".to_string(),
                record
                    .get("content_fidelity")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            item.insert("excerpt".to_string(), Value::String(excerpt));
            item.insert(
                "excerpt_details".to_string(),
                Value::Object(excerpt_details),
            );
            if let Some(phase_reference) = self.phase_reference_for_search_record(record)? {
                item.insert("phase_ref".to_string(), Value::String(phase_reference));
            }
            if let Some(prompt_reference) = prompt_reference {
                if prompt_reference != reference.as_str() {
                    if let Some(prompt_excerpt) = prompt_excerpts.get(prompt_reference) {
                        item.insert(
                            "prompt_excerpt".to_string(),
                            Value::String(prompt_excerpt.clone()),
                        );
                    }
                }
            }
            matches.push((*score, *at_ms, reference.clone(), item));
        }
        match sort {
            "relevance" => matches.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| b.1.cmp(&a.1))
                    .then_with(|| a.2.cmp(&b.2))
            }),
            "newest" => matches.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| b.0.cmp(&a.0))
                    .then_with(|| a.2.cmp(&b.2))
            }),
            _ => matches.sort_by(|a, b| {
                a.1.cmp(&b.1)
                    .then_with(|| b.0.cmp(&a.0))
                    .then_with(|| a.2.cmp(&b.2))
            }),
        }
        let total_matches = matches.len() as i64;
        let page: Vec<Map<String, Value>> = matches
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(_, _, _, item)| item)
            .collect();
        Ok(SearchResults {
            items: page,
            total_matches,
            offset,
            offset_display: offset.to_string(),
            limit,
            corpus: corpus.to_string(),
            match_mode: match_mode.to_string(),
            sort: sort.to_string(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn search_match(
        results: &mut Vec<Map<String, Value>>,
        needle: &str,
        case_sensitive: bool,
        reference: &str,
        record_type: &str,
        team: &str,
        field: &str,
        text: &str,
        start_ms: Option<i64>,
        at_ms: Option<i64>,
    ) {
        let contains = if case_sensitive {
            text.contains(needle)
        } else {
            py_casefold(text).contains(&py_casefold(needle))
        };
        if !contains {
            return;
        }
        let compact = compact_text(text);
        // The oracle finds the needle in casefolded
        // coordinates and slices the compact text with them;
        // reproduce that, shifted windows and all.
        let haystack: Vec<char> = (if case_sensitive {
            compact.clone()
        } else {
            py_casefold(&compact)
        })
        .chars()
        .collect();
        let sought: Vec<char> = (if case_sensitive {
            needle.to_string()
        } else {
            py_casefold(needle)
        })
        .chars()
        .collect();
        let compact_chars: Vec<char> = compact.chars().collect();
        let position = haystack
            .windows(sought.len().max(1))
            .position(|window| window == sought.as_slice());
        let excerpt = match position {
            None => compact_chars.iter().take(240).collect::<String>(),
            Some(position) => {
                let start = position.saturating_sub(100);
                let end = (position + needle.chars().count() + 140).min(compact_chars.len());
                let prefix = if start > 0 { "…" } else { "" };
                let suffix = if end < compact_chars.len() { "…" } else { "" };
                format!(
                    "{prefix}{}{suffix}",
                    compact_chars[start..end].iter().collect::<String>()
                )
            }
        };
        let mut result = Map::new();
        result.insert("ref".to_string(), Value::String(reference.to_string()));
        result.insert(
            "record_type".to_string(),
            Value::String(record_type.to_string()),
        );
        result.insert("team".to_string(), Value::String(team.to_string()));
        result.insert("field".to_string(), Value::String(field.to_string()));
        result.insert("excerpt".to_string(), Value::String(excerpt));
        if let Some(start_ms) = start_ms {
            result.insert("start_ms".to_string(), Value::from(start_ms));
            result.insert("start_time".to_string(), Value::String(timestamp(start_ms)));
        }
        if let Some(at_ms) = at_ms {
            result.insert("at_ms".to_string(), Value::from(at_ms));
            result.insert("at_time".to_string(), Value::String(timestamp(at_ms)));
        }
        results.push(result);
    }

    /// `TimelineQuery.search`: the compatibility scopes.
    pub fn search(
        &self,
        needle: &str,
        scope: &str,
        filters: &Filters,
        case_sensitive: bool,
        limit: i64,
    ) -> Result<Vec<Map<String, Value>>, String> {
        if needle.is_empty() {
            return Err("search text must not be empty".to_string());
        }
        if !["summaries", "transcripts", "all"].contains(&scope) {
            return Err(format!(
                "unsupported search scope {}",
                py_repr(&Value::String(scope.to_string()))
            ));
        }
        if limit < 1 {
            return Err("--limit must be at least 1".to_string());
        }
        let selected_agent = self.validated_agent_filter(filters)?;
        let window = parse_window(filters)?;
        let mut matches: Vec<Map<String, Value>> = Vec::new();
        if ["summaries", "all"].contains(&scope) {
            // agents
            for record in self.scoped("agents", filters)? {
                let team = team_of(&record, "agent")?;
                let reference = agent_ref(&record, "agent")?;
                if !filters.teams.is_empty() && !filters.teams.iter().any(|item| item == &team) {
                    continue;
                }
                if let Some(selected) = &selected_agent {
                    if &reference != selected {
                        continue;
                    }
                }
                let (start_ms, _) = interval(&record, &reference)?;
                if let Some(window) = &window {
                    let (_, end_ms) = interval(&record, &reference)?;
                    if !window.overlaps(start_ms, end_ms) {
                        continue;
                    }
                }
                let mut fields: Vec<&str> = vec!["short_name", "official_name", "nickname"];
                if summary_available(&record, "summary_available")? {
                    fields.extend(["lifetime_summary", "naming_rationale"]);
                }
                for field in fields {
                    if let Some(text) = optional_string(&record, field) {
                        if !text.is_empty() {
                            Self::search_match(
                                &mut matches,
                                needle,
                                case_sensitive,
                                &reference,
                                "agent",
                                &team,
                                field,
                                text,
                                Some(start_ms),
                                None,
                            );
                        }
                    }
                }
            }
            // phases
            for record in self.scoped("phases", filters)? {
                let team = team_of(&record, "phase")?;
                let current_agent = agent_reference_for_id(
                    &team,
                    as_string(
                        record.get("agent_id").unwrap_or(&Value::Null),
                        "phase.agent_id",
                    )?,
                );
                if !filters.teams.is_empty() && !filters.teams.iter().any(|item| item == &team) {
                    continue;
                }
                if let Some(selected) = &selected_agent {
                    if &current_agent != selected {
                        continue;
                    }
                }
                let reference = phase_ref(&record, "phase")?;
                let (start_ms, end_ms) = interval(&record, &reference)?;
                if let Some(window) = &window {
                    if !window.overlaps(start_ms, end_ms) {
                        continue;
                    }
                }
                if !summary_available(&record, "summary_available")? {
                    continue;
                }
                for field in ["phrase", "paragraph"] {
                    if let Some(text) = optional_string(&record, field) {
                        if !text.is_empty() {
                            Self::search_match(
                                &mut matches,
                                needle,
                                case_sensitive,
                                &reference,
                                "phase",
                                &team,
                                field,
                                text,
                                Some(start_ms),
                                None,
                            );
                        }
                    }
                }
            }
            // rollups
            if selected_agent.is_none() {
                for record in self.scoped("rollups", filters)? {
                    let team = team_of(&record, "rollup")?;
                    let kind =
                        as_string(record.get("kind").unwrap_or(&Value::Null), "rollup.kind")?;
                    let reference = rollup_ref(&record, "rollup")?;
                    if !filters.teams.is_empty() && !filters.teams.iter().any(|item| item == &team)
                    {
                        continue;
                    }
                    if !filters.kinds.is_empty() && !filters.kinds.iter().any(|item| item == kind) {
                        continue;
                    }
                    let (start_ms, end_ms) = interval(&record, &reference)?;
                    if let Some(window) = &window {
                        if !window.overlaps(start_ms, end_ms) {
                            continue;
                        }
                    }
                    let mut fields: Vec<(&str, String)> = Vec::new();
                    if summary_available(&record, "technical_summary_available")? {
                        fields.push((
                            "technical_markdown",
                            as_string(
                                record.get("technical_path").unwrap_or(&Value::Null),
                                "rollup.technical_path",
                            )?
                            .to_string(),
                        ));
                    }
                    if summary_available(&record, "plain_language_summary_available")? {
                        fields.push((
                            "plain_language_markdown",
                            as_string(
                                record.get("plain_language_path").unwrap_or(&Value::Null),
                                "rollup.plain_language_path",
                            )?
                            .to_string(),
                        ));
                    }
                    for (field, relative) in fields {
                        let text = self.read_markdown(&self.rollup_file(&relative, &team)?)?;
                        Self::search_match(
                            &mut matches,
                            needle,
                            case_sensitive,
                            &reference,
                            "rollup",
                            &team,
                            field,
                            &text,
                            Some(start_ms),
                            None,
                        );
                    }
                }
            }
        }
        if ["transcripts", "all"].contains(&scope) {
            for record in self.scoped("phases", filters)? {
                let team = team_of(&record, "phase")?;
                let current_agent = agent_reference_for_id(
                    &team,
                    as_string(
                        record.get("agent_id").unwrap_or(&Value::Null),
                        "phase.agent_id",
                    )?,
                );
                if !filters.teams.is_empty() && !filters.teams.iter().any(|item| item == &team) {
                    continue;
                }
                if let Some(selected) = &selected_agent {
                    if &current_agent != selected {
                        continue;
                    }
                }
                let reference = phase_ref(&record, "phase")?;
                let (start_ms, end_ms) = interval(&record, &reference)?;
                if let Some(window) = &window {
                    if !window.overlaps(start_ms, end_ms) {
                        continue;
                    }
                }
                let relative = as_string(
                    record.get("detail_path").unwrap_or(&Value::Null),
                    "phase.detail_path",
                )?;
                let detail_path = self.detail_file(relative)?;
                let detail_value = self.read_json_file(&detail_path)?;
                let detail = as_object(&detail_value, relative)?;
                let transcript = as_array(
                    detail.get("transcript").unwrap_or(&Value::Null),
                    &format!("{relative}.transcript"),
                )?;
                for (index, raw_message) in transcript.iter().enumerate() {
                    let message =
                        as_object(raw_message, &format!("{relative}.transcript[{index}]"))?;
                    let at_ms = as_int(
                        message.get("at_ms").unwrap_or(&Value::Null),
                        &format!("{relative}.transcript[{index}].at_ms"),
                    )?;
                    if let Some(window) = &window {
                        if !window.contains_dec(&at_ms.to_string()) {
                            continue;
                        }
                    }
                    let text = as_string(
                        message.get("text").unwrap_or(&Value::Null),
                        &format!("{relative}.transcript[{index}].text"),
                    )?;
                    let contains = if case_sensitive {
                        text.contains(needle)
                    } else {
                        py_casefold(text).contains(&py_casefold(needle))
                    };
                    if !contains {
                        continue;
                    }
                    let role = as_string(
                        message.get("role").unwrap_or(&Value::Null),
                        &format!("{relative}.transcript[{index}].role"),
                    )?;
                    Self::search_match(
                        &mut matches,
                        needle,
                        case_sensitive,
                        &reference,
                        "transcript",
                        &team,
                        role,
                        text,
                        None,
                        Some(at_ms),
                    );
                }
            }
        }
        matches.sort_by(|left, right| {
            let left_at = optional_int(left, "at_ms")
                .or_else(|| optional_int(left, "start_ms"))
                .unwrap_or(-1);
            let right_at = optional_int(right, "at_ms")
                .or_else(|| optional_int(right, "start_ms"))
                .unwrap_or(-1);
            left_at
                .cmp(&right_at)
                .then_with(|| {
                    optional_string(left, "ref")
                        .unwrap_or("")
                        .cmp(optional_string(right, "ref").unwrap_or(""))
                })
                .then_with(|| {
                    optional_string(left, "field")
                        .unwrap_or("")
                        .cmp(optional_string(right, "field").unwrap_or(""))
                })
        });
        matches.truncate(limit as usize);
        Ok(matches)
    }

    /// `_show_search_record` for `message:`/`tool:` references.
    pub fn show_search_record(&self, reference: &str) -> Result<Map<String, Value>, String> {
        let (_, remainder) = reference.split_once(':').unzip();
        let Some(remainder) = remainder else {
            return Err(format!(
                "invalid transcript search reference {}",
                py_repr(&Value::String(reference.to_string()))
            ));
        };
        let Some((team, _)) = remainder.split_once("::") else {
            return Err(format!(
                "invalid transcript search reference {}",
                py_repr(&Value::String(reference.to_string()))
            ));
        };
        if team.is_empty() {
            return Err(format!(
                "invalid transcript search reference {}",
                py_repr(&Value::String(reference.to_string()))
            ));
        }
        let team_filters = Filters {
            teams: vec![team.to_string()],
            start_time: None,
            end_time: None,
            kinds: Vec::new(),
            agent: None,
        };
        let mut record: Option<Map<String, Value>> = None;
        let mut prompts: std::collections::BTreeMap<String, Map<String, Value>> =
            std::collections::BTreeMap::new();
        let mut responses: Vec<Map<String, Value>> = Vec::new();
        for candidate in self.iter_search_records(Some(&team_filters), &[])? {
            let candidate_reference = as_string(
                candidate.get("ref").unwrap_or(&Value::Null),
                "search record.ref",
            )?
            .to_string();
            let candidate_type = as_string(
                candidate.get("record_type").unwrap_or(&Value::Null),
                "search record.record_type",
            )?;
            if ["prompt", "inter_agent_prompt"].contains(&candidate_type) {
                prompts.insert(candidate_reference.clone(), candidate.clone());
            }
            if candidate_reference == reference {
                record = Some(candidate.clone());
            }
            if optional_string(&candidate, "prompt_ref") == Some(reference)
                && candidate_reference != reference
                && ["response", "inter_agent_response"].contains(&candidate_type)
            {
                responses.push(candidate);
            }
        }
        let Some(record) = record else {
            return Err(format!(
                "unknown stable reference {}",
                py_repr(&Value::String(reference.to_string()))
            ));
        };
        let mut result = record.clone();
        result.insert(
            "at_time".to_string(),
            Value::String(timestamp(as_int(
                record.get("at_ms").unwrap_or(&Value::Null),
                "search record.at_ms",
            )?)),
        );
        if let Some(phase_reference) = self.phase_reference_for_search_record(&record)? {
            result.insert("phase_ref".to_string(), Value::String(phase_reference));
        }
        let prompt_reference = optional_string(&record, "prompt_ref");
        if let Some(prompt_reference) = prompt_reference {
            if prompt_reference != reference {
                let linked = prompts.get(prompt_reference).cloned();
                result.insert(
                    "linked_prompt".to_string(),
                    linked.map(Value::Object).unwrap_or(Value::Null),
                );
            }
        }
        let record_type = as_string(
            record.get("record_type").unwrap_or(&Value::Null),
            "search record.record_type",
        )?;
        if ["prompt", "inter_agent_prompt"].contains(&record_type) {
            responses.sort_by(|left, right| {
                let left_at = optional_int(left, "at_ms").unwrap_or(0);
                let right_at = optional_int(right, "at_ms").unwrap_or(0);
                left_at.cmp(&right_at).then_with(|| {
                    optional_string(left, "ref")
                        .unwrap_or("")
                        .cmp(optional_string(right, "ref").unwrap_or(""))
                })
            });
            result.insert(
                "linked_responses".to_string(),
                Value::Array(responses.into_iter().map(Value::Object).collect()),
            );
        }
        Ok(result)
    }

    fn lookup_entry(&self, reference: &str) -> Result<Option<LookupEntry>, String> {
        let Some((kind, remainder)) = reference.split_once(':') else {
            return Ok(None);
        };
        if kind == "team" {
            for (index, record) in self.teams.iter().enumerate() {
                let slug = as_string(
                    record.get("slug").unwrap_or(&Value::Null),
                    &format!("timeline.teams[{index}].slug"),
                )?;
                if slug == remainder {
                    return Ok(Some(("team".to_string(), record.clone())));
                }
            }
            return Ok(None);
        }
        if !["agent", "phase", "rollup"].contains(&kind) {
            return Ok(None);
        }
        match &self.generation {
            Generation::Three(three) => {
                let Some((team, _)) = remainder.split_once("::") else {
                    return Ok(None);
                };
                let kind_name = match kind {
                    "agent" => "agent",
                    "phase" => "phase_card",
                    _ => "rollup",
                };
                let reference_of: ReferenceOf = match kind {
                    "agent" => agent_ref,
                    "phase" => phase_ref,
                    _ => rollup_ref,
                };
                let mut bounds = std::collections::BTreeMap::new();
                for record in self.spine_group(three, "activity_bounds", team)? {
                    let where_ = format!("{team} activity bounds");
                    let bound_ref = as_string(
                        record.get("ref").unwrap_or(&Value::Null),
                        &format!("{where_}.ref"),
                    )?
                    .to_string();
                    let start = as_int(
                        record.get("activity_start_ms").unwrap_or(&Value::Null),
                        &format!("{where_}.activity_start_ms"),
                    )?;
                    let end = as_int(
                        record.get("activity_end_ms").unwrap_or(&Value::Null),
                        &format!("{where_}.activity_end_ms"),
                    )?;
                    bounds.insert(bound_ref, (start, end));
                }
                // Python's refs() builds the whole team table,
                // so any duplicate reference in it refuses.
                let mut seen = std::collections::BTreeSet::new();
                let mut table_entry = None;
                for record in self.spine_group(three, kind_name, team)? {
                    let found_ref = reference_of(&record, kind)?;
                    if !seen.insert(found_ref.clone()) {
                        return Err(format!(
                            "duplicate stable reference {}",
                            py_repr(&Value::String(found_ref))
                        ));
                    }
                    if found_ref == reference {
                        let mut merged = record.clone();
                        if let Some((start, end)) = bounds.get(&found_ref) {
                            merged.insert("activity_start_ms".to_string(), Value::from(*start));
                            merged.insert("activity_end_ms".to_string(), Value::from(*end));
                        }
                        table_entry = Some((kind.to_string(), merged));
                    }
                }
                Ok(table_entry)
            }
            Generation::One(_) => {
                let mut entries: Vec<(String, String, Map<String, Value>)> = Vec::new();
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
                        if entries.iter().any(|(existing, _, _)| existing == &found) {
                            return Err(format!(
                                "duplicate stable reference {}",
                                py_repr(&Value::String(found))
                            ));
                        }
                        entries.push((found, singular.to_string(), record));
                    }
                }
                Ok(entries
                    .into_iter()
                    .find(|(found, _, _)| found == reference)
                    .map(|(_, kind, record)| (kind, record)))
            }
        }
    }

    /// Resolve one stable reference to its record kind, the
    /// way `--agent` validation needs it.
    fn lookup(&self, reference: &str) -> Result<Option<String>, String> {
        Ok(self.lookup_entry(reference)?.map(|(kind, _record)| kind))
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

    fn safe_file(&self, relative: &str) -> Result<(PathBuf, Vec<String>), String> {
        let escaped = || {
            format!(
                "archive path escapes root: {}",
                py_repr(&Value::String(relative.to_string()))
            )
        };
        if relative.starts_with('/') || relative.is_empty() {
            return Err(escaped());
        }
        let parts: Vec<String> = relative
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_string)
            .collect();
        if parts.is_empty() || parts.iter().any(|part| part == "..") {
            return Err(escaped());
        }
        let mut joined = self.root.clone();
        for part in &parts {
            joined.push(part);
        }
        let path = resolve_path(&joined);
        if !path.starts_with(&self.root) {
            return Err(escaped());
        }
        if path.is_file() {
            return Ok((path, parts));
        }
        let mut stored_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        stored_name.push_str(".gz");
        let stored = path.with_file_name(stored_name);
        if stored.is_file() {
            return Ok((stored, parts));
        }
        Err(format!("archive file is missing: {relative}"))
    }

    fn detail_file(&self, relative: &str) -> Result<PathBuf, String> {
        let (path, parts) = self.safe_file(relative)?;
        let suffix = posix_suffix(&parts);
        if parts.len() < 3 || parts[0] != "data" || parts[1] != "details" || suffix != ".json" {
            return Err(format!(
                "phase detail path is outside data/details: {}",
                py_repr(&Value::String(relative.to_string()))
            ));
        }
        Ok(path)
    }

    fn rollup_file(&self, relative: &str, team: &str) -> Result<PathBuf, String> {
        let (path, parts) = self.safe_file(relative)?;
        let suffix = posix_suffix(&parts);
        if parts.len() < 5
            || parts[0] != "teams"
            || parts[1] != team
            || parts[2] != "summaries"
            || suffix != ".md"
        {
            return Err(format!(
                "rollup path is outside teams/{team}/summaries: {}",
                py_repr(&Value::String(relative.to_string()))
            ));
        }
        Ok(path)
    }

    fn read_json_file(&self, path: &Path) -> Result<Value, String> {
        let compressed = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".gz"));
        let bytes = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
        let bytes = if compressed {
            let mut decoded = Vec::new();
            flate2::read::MultiGzDecoder::new(&bytes[..])
                .read_to_end(&mut decoded)
                .map_err(|error| error.to_string())?;
            decoded
        } else {
            bytes
        };
        let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        serde_json::from_str(&text).map_err(|error| error.to_string())
    }

    fn read_markdown(&self, path: &Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
        match std::str::from_utf8(&bytes) {
            Ok(text) => Ok(text.to_string()),
            Err(error) => Err(py_utf8_error(&error, &bytes)),
        }
    }

    /// `TimelineQuery.show` for team, agent, phase, and rollup
    /// references, after the transcript-reference refusal in
    /// the CLI has handled `message:` and `tool:`.
    pub fn show_record(
        &self,
        reference: &str,
        transcript: bool,
    ) -> Result<Map<String, Value>, String> {
        let Some((kind, record)) = self.lookup_entry(reference)? else {
            if reference.starts_with("message:") || reference.starts_with("tool:") {
                return self.show_search_record(reference);
            }
            return Err(format!(
                "unknown stable reference {}",
                py_repr(&Value::String(reference.to_string()))
            ));
        };
        let mut result = record.clone();
        result.insert("ref".to_string(), Value::String(reference.to_string()));
        result.insert("record_type".to_string(), Value::String(kind.clone()));
        match kind.as_str() {
            "team" => {
                let slug =
                    as_string(record.get("slug").unwrap_or(&Value::Null), "team.slug")?.to_string();
                let mut agents = Vec::new();
                for candidate in self.for_team("agents", &slug)? {
                    agents.push(agent_ref(&candidate, "agent")?);
                }
                result.insert("agent_refs".to_string(), sorted_refs(agents));
                let mut rollups = Vec::new();
                for candidate in self.for_team("rollups", &slug)? {
                    rollups.push(rollup_ref(&candidate, "rollup")?);
                }
                result.insert("rollup_refs".to_string(), sorted_refs(rollups));
                if let Some((start, end)) = self.team_range(&slug)? {
                    result.insert("start_ms".to_string(), Value::from(start));
                    result.insert("end_ms".to_string(), Value::from(end));
                    result.insert("start_time".to_string(), Value::String(timestamp(start)));
                    result.insert("end_time".to_string(), Value::String(timestamp(end)));
                }
            }
            "agent" => {
                let team = team_of(&record, "agent")?;
                let identifier =
                    as_string(record.get("id").unwrap_or(&Value::Null), "agent.id")?.to_string();
                let parent_ref = match record.get("parent_id") {
                    None | Some(Value::Null) => Value::Null,
                    Some(parent) => Value::String(agent_reference_for_id(
                        &team,
                        as_string(parent, "agent.parent_id")?,
                    )),
                };
                result.insert("parent_ref".to_string(), parent_ref);
                let mut children = Vec::new();
                for candidate in self.for_team("agents", &team)? {
                    if let Some(parent) = candidate.get("parent_id").and_then(Value::as_str) {
                        if same_agent_identifier(&team, parent, &identifier) {
                            children.push(agent_ref(&candidate, "agent")?);
                        }
                    }
                }
                result.insert("child_refs".to_string(), sorted_refs(children));
                let mut phases = Vec::new();
                for candidate in self.for_team("phases", &team)? {
                    let agent_id = as_string(
                        candidate.get("agent_id").unwrap_or(&Value::Null),
                        "phase.agent_id",
                    )?;
                    if same_agent_identifier(&team, agent_id, &identifier) {
                        phases.push(phase_ref(&candidate, "phase")?);
                    }
                }
                result.insert("phase_refs".to_string(), sorted_refs(phases));
                with_times(&mut result, &record, "agent")?;
            }
            "phase" => {
                let team = team_of(&record, "phase")?;
                let identifier = as_string(
                    record.get("agent_id").unwrap_or(&Value::Null),
                    "phase.agent_id",
                )?;
                result.insert(
                    "agent_ref".to_string(),
                    Value::String(agent_reference_for_id(&team, identifier)),
                );
                let relative = as_string(
                    record.get("detail_path").unwrap_or(&Value::Null),
                    "phase.detail_path",
                )?
                .to_string();
                let detail_value = self.read_json_file(&self.detail_file(&relative)?)?;
                let detail = as_object(&detail_value, &relative)?;
                let mut shown = detail.clone();
                if !transcript {
                    shown.remove("transcript");
                }
                result.insert("detail".to_string(), Value::Object(shown));
                with_times(&mut result, &record, "phase")?;
            }
            "rollup" => {
                let team = team_of(&record, "rollup")?;
                if summary_available(&record, "technical_summary_available")? {
                    let relative = as_string(
                        record.get("technical_path").unwrap_or(&Value::Null),
                        "rollup.technical_path",
                    )?;
                    result.insert(
                        "technical_markdown".to_string(),
                        Value::String(self.read_markdown(&self.rollup_file(relative, &team)?)?),
                    );
                }
                if summary_available(&record, "plain_language_summary_available")? {
                    let relative = as_string(
                        record.get("plain_language_path").unwrap_or(&Value::Null),
                        "rollup.plain_language_path",
                    )?;
                    result.insert(
                        "plain_language_markdown".to_string(),
                        Value::String(self.read_markdown(&self.rollup_file(relative, &team)?)?),
                    );
                }
                with_times(&mut result, &record, "rollup")?;
            }
            _ => unreachable!(),
        }
        Ok(result)
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

/// `TextTotals`: records, whitespace-delimited words, and
/// UTF-8 text bytes over a set of texts.
#[derive(Clone, Copy, Default)]
pub struct TextTotals {
    records: i64,
    words: i64,
    utf8_bytes: i64,
}

impl TextTotals {
    fn add_text(&mut self, text: &str) {
        self.records += 1;
        // Python `str.split()`: Unicode whitespace plus the
        // U+001C..=U+001F separators `str.isspace` also admits.
        self.words += text
            .split(|ch: char| ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}'))
            .filter(|word| !word.is_empty())
            .count() as i64;
        self.utf8_bytes += text.len() as i64;
    }

    fn plus(&self, other: &TextTotals) -> TextTotals {
        TextTotals {
            records: self.records + other.records,
            words: self.words + other.words,
            utf8_bytes: self.utf8_bytes + other.utf8_bytes,
        }
    }

    fn to_map(self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("records".to_string(), Value::from(self.records));
        map.insert("words".to_string(), Value::from(self.words));
        map.insert("utf8_bytes".to_string(), Value::from(self.utf8_bytes));
        map
    }
}

/// `SummaryKindStats`: availability plus content totals for
/// one summary surface.
pub struct SummaryKindStats {
    available: i64,
    unavailable: i64,
    content: TextTotals,
}

impl SummaryKindStats {
    fn from_texts(texts: Vec<String>, unavailable: i64) -> SummaryKindStats {
        let mut content = TextTotals::default();
        for text in &texts {
            content.add_text(text);
        }
        SummaryKindStats {
            available: texts.len() as i64,
            unavailable,
            content,
        }
    }

    fn to_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("available".to_string(), Value::from(self.available));
        map.insert("unavailable".to_string(), Value::from(self.unavailable));
        map.insert(
            "total".to_string(),
            Value::from(self.available + self.unavailable),
        );
        map.insert("content".to_string(), Value::Object(self.content.to_map()));
        map
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

struct ChunkMember {
    c_off: i64,
    c_len: i64,
    #[allow(dead_code)]
    u_off: i64,
    u_len: i64,
    #[allow(dead_code)]
    l0: i64,
    #[allow(dead_code)]
    n: i64,
}

struct ChunkIndex {
    timestamp_key: String,
    record_count: i64,
    c_size: i64,
    u_size: i64,
    #[allow(dead_code)]
    c_sha256: String,
    u_sha256: String,
    data_file: String,
    members: Vec<ChunkMember>,
}

fn parse_chunk_index(text: &str, where_: &str) -> Result<ChunkIndex, String> {
    let lines: Vec<&str> = text.split('\n').filter(|line| !line.is_empty()).collect();
    if lines.is_empty() {
        return Err(format!("{where_}: index is empty"));
    }
    let header_value: Value = serde_json::from_str(lines[0])
        .map_err(|error| format!("{where_}: malformed index: {error}"))?;
    let header = as_object(&header_value, &format!("{where_} header"))?;
    if as_string(
        header.get("format").unwrap_or(&Value::Null),
        &format!("{where_} header.format"),
    )? != "wrkviz/seekable-jsonl-index"
    {
        return Err(format!("{where_}: not a chunked-JSONL index"));
    }
    let version = as_int(
        header.get("version").unwrap_or(&Value::Null),
        &format!("{where_} header.version"),
    )?;
    if version != 1 {
        return Err(format!("{where_}: unsupported index version {version}"));
    }
    let codec = as_string(
        header.get("codec").unwrap_or(&Value::Null),
        &format!("{where_} header.codec"),
    )?;
    if codec != "gzip" {
        return Err(format!(
            "{where_}: index describes codec {}; this reader implements 'gzip' only",
            py_repr(&Value::String(codec.to_string()))
        ));
    }
    let mut members: Vec<ChunkMember> = Vec::new();
    let (mut c_off, mut u_off, mut line_no) = (0i64, 0i64, 0i64);
    for (offset, raw_line) in lines[1..].iter().enumerate() {
        let spot = format!("{where_} member {offset}");
        let value: Value = serde_json::from_str(raw_line)
            .map_err(|error| format!("{where_}: malformed index: {error}"))?;
        let obj = as_object(&value, &spot)?;
        let field = |key: &str| -> Result<i64, String> {
            as_int(
                obj.get(key).unwrap_or(&Value::Null),
                &format!("{spot}.{key}"),
            )
        };
        let member = ChunkMember {
            c_off: field("c_off")?,
            c_len: field("c_len")?,
            u_off: field("u_off")?,
            u_len: field("u_len")?,
            l0: field("l0")?,
            n: field("n")?,
        };
        // The timestamp bounds are validated as integers when
        // present, the way the oracle validates them.
        for key in ["t0", "t1"] {
            let value = obj.get(key).unwrap_or(&Value::Null);
            if !value.is_null() {
                int_literal(value, &format!("{spot}.{key}"))?;
            }
        }
        if member.c_off != c_off || member.u_off != u_off || member.l0 != line_no {
            return Err(format!("{spot}: not contiguous with the preceding members"));
        }
        if member.c_len <= 0 || member.u_len < 0 || member.n < 0 {
            return Err(format!("{spot}: non-positive extent"));
        }
        c_off += member.c_len;
        u_off += member.u_len;
        line_no += member.n;
        members.push(member);
    }
    if members.is_empty() {
        return Err(format!("{where_}: index describes no members"));
    }
    let member_count = as_int(
        header.get("member_count").unwrap_or(&Value::Null),
        &format!("{where_} header.member_count"),
    )?;
    if member_count != members.len() as i64 {
        return Err(format!(
            "{where_}: header member count disagrees with the table"
        ));
    }
    let record_count = as_int(
        header.get("record_count").unwrap_or(&Value::Null),
        &format!("{where_} header.record_count"),
    )?;
    let c_size = as_int(
        header.get("c_size").unwrap_or(&Value::Null),
        &format!("{where_} header.c_size"),
    )?;
    let u_size = as_int(
        header.get("u_size").unwrap_or(&Value::Null),
        &format!("{where_} header.u_size"),
    )?;
    if record_count != line_no || c_size != c_off || u_size != u_off {
        return Err(format!(
            "{where_}: header totals disagree with the member table"
        ));
    }
    match header.get("timestamps_sorted") {
        Some(Value::Bool(_)) => {}
        _ => {
            return Err(format!(
                "{where_} header.timestamps_sorted: expected a boolean"
            ))
        }
    }
    Ok(ChunkIndex {
        timestamp_key: as_string(
            header.get("timestamp_key").unwrap_or(&Value::Null),
            &format!("{where_} header.timestamp_key"),
        )?
        .to_string(),
        record_count,
        c_size,
        u_size,
        c_sha256: as_string(
            header.get("c_sha256").unwrap_or(&Value::Null),
            &format!("{where_} header.c_sha256"),
        )?
        .to_string(),
        u_sha256: as_string(
            header.get("u_sha256").unwrap_or(&Value::Null),
            &format!("{where_} header.u_sha256"),
        )?
        .to_string(),
        data_file: as_string(
            header.get("data_file").unwrap_or(&Value::Null),
            &format!("{where_} header.data_file"),
        )?
        .to_string(),
        members,
    })
}

struct LineSource {
    name: String,
    path: String,
    lines: Vec<Vec<u8>>,
    data: Vec<u8>,
    sha256: String,
    check_on_exhaust: bool,
}

impl LineSource {
    fn parse(&self, number: usize) -> Result<Map<String, Value>, String> {
        let line = &self.lines[number];
        let where_ = format!("{} line {number}", self.path);
        let line = std::str::from_utf8(line).map_err(|error| {
            format!(
                "{where_}: not a JSON record: {}",
                py_utf8_error(&error, line)
            )
        })?;
        let value = parse_json_lenient(line)
            .map_err(|error| format!("{where_}: not a JSON record: {error}"))?;
        Ok(as_object(&value, &where_)?.clone())
    }

    /// The digest verdict an end-to-end read reaches when the
    /// consumer actually exhausts the stream.
    fn finish(&self, exhausted: bool) -> Result<(), String> {
        if exhausted && self.check_on_exhaust && hex_digest(&self.data) != self.sha256 {
            return Err(format!(
                "transcript export generation is incomplete: {}",
                self.name
            ));
        }
        Ok(())
    }
}

struct ManagedFile {
    name: String,
    path: PathBuf,
    index_path: PathBuf,
    sha256: String,
    declared_bytes: Option<i64>,
    records: Option<i64>,
    #[allow(dead_code)]
    compressed_sha256: Option<String>,
    compressed_bytes: Option<i64>,
}

/// CPython's `UnicodeDecodeError` spelling for one bad byte.
fn py_utf8_error(error: &std::str::Utf8Error, bytes: &[u8]) -> String {
    let position = error.valid_up_to();
    let byte = bytes.get(position).copied().unwrap_or(0);
    let reason = match error.error_len() {
        None => "unexpected end of data",
        Some(_) if matches!(byte, 0x80..=0xbf) || byte > 0xf4 => "invalid start byte",
        Some(_) => "invalid continuation byte",
    };
    format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}")
}

fn is_link_or_missing(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.file_type().is_symlink() || !meta.is_file(),
        Err(_) => true,
    }
}

/// The transcript projections under `extracted/transcripts`,
/// read through the chunked container the manifest declares
/// (`TranscriptQuery`, full-stream path only: `query stats`
/// reads every record and reproduces the declared digest).
pub struct TranscriptQuery {
    declared: std::collections::BTreeMap<String, ManagedFile>,
}

impl TranscriptQuery {
    /// Open the manifest and check every declared projection
    /// the way `TranscriptQuery.__init__` does.
    pub fn open(root: &Path) -> Result<TranscriptQuery, String> {
        let root = resolve_path(root);
        let transcript_root = root.join("extracted").join("transcripts");
        let manifest_path = transcript_root.join("manifest.json");
        if is_link_or_missing(&manifest_path) {
            return Err(format!(
                "no extracted transcripts at {}; run extract-transcripts",
                manifest_path.display()
            ));
        }
        let manifest_value = crate::json::read_json(&manifest_path)?;
        let manifest = as_object(&manifest_value, &manifest_path.display().to_string())?;
        let schema = as_int(
            manifest.get("schema_version").unwrap_or(&Value::Null),
            "transcript manifest schema",
        )?;
        if schema != 1 {
            return Err(format!(
                "unsupported transcript export schema version {schema}; expected 1"
            ));
        }
        let files = as_object(
            manifest.get("files").unwrap_or(&Value::Null),
            "transcript manifest files",
        )?;
        let mut declared = std::collections::BTreeMap::new();
        for name in ["prompts.jsonl", "messages.jsonl", "system-inputs.jsonl"] {
            let entry = as_object(
                files.get(name).unwrap_or(&Value::Null),
                &format!("transcript manifest files.{name}"),
            )?;
            let managed = Self::declare(&transcript_root, name, entry)?;
            if let Some(expected) = managed.compressed_bytes {
                let size = std::fs::metadata(&managed.path)
                    .map(|meta| meta.len() as i64)
                    .unwrap_or(-1);
                if size != expected {
                    let file_name = managed
                        .path
                        .file_name()
                        .map(|part| part.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    return Err(format!(
                        "transcript export generation is incomplete: {file_name} holds {size} bytes, the manifest declares {expected}"
                    ));
                }
            }
            declared.insert(name.to_string(), managed);
        }
        Ok(TranscriptQuery { declared })
    }

    fn declare(
        transcript_root: &Path,
        name: &str,
        entry: &Map<String, Value>,
    ) -> Result<ManagedFile, String> {
        let Some(container) = entry.get("container") else {
            return Err(format!(
                "{name} is an uncompressed transcript projection; this reader reads the seekable container only -- re-run extract-transcripts to rebuild the corpus"
            ));
        };
        let container = as_string(container, &format!("{name}.container"))?;
        if container != "multi-member-gzip" {
            return Err(format!(
                "{name} declares container {}; this reader implements 'multi-member-gzip' only",
                py_repr(&Value::String(container.to_string()))
            ));
        }
        let expected_name = format!("{name}.gz");
        let declared_path = as_string(
            entry.get("path").unwrap_or(&Value::Null),
            &format!("{name}.path"),
        )?;
        if declared_path != expected_name {
            return Err(format!(
                "{name} declares its container as {}, not {}",
                py_repr(&Value::String(declared_path.to_string())),
                py_repr(&Value::String(expected_name.clone()))
            ));
        }
        let expected_index = format!("{expected_name}.index.jsonl");
        let declared_index = as_string(
            entry.get("index_path").unwrap_or(&Value::Null),
            &format!("{name}.index_path"),
        )?;
        if declared_index != expected_index {
            return Err(format!(
                "{name} declares its index as {}, not {}",
                py_repr(&Value::String(declared_index.to_string())),
                py_repr(&Value::String(expected_index.clone()))
            ));
        }
        let path = transcript_root.join(&expected_name);
        if is_link_or_missing(&path) {
            return Err(format!(
                "transcript export file is missing or unsafe: {}",
                path.display()
            ));
        }
        let compressed_sha256 = match entry.get("c_sha256") {
            None | Some(Value::Null) => None,
            Some(value) => Some(as_string(value, &format!("{name}.c_sha256"))?.to_string()),
        };
        Ok(ManagedFile {
            name: name.to_string(),
            index_path: transcript_root.join(&expected_index),
            path,
            sha256: as_string(
                entry.get("sha256").unwrap_or(&Value::Null),
                &format!("{name}.sha256"),
            )?
            .to_string(),
            declared_bytes: crate::json::optional_int(
                entry.get("bytes").unwrap_or(&Value::Null),
                &format!("{name}.bytes"),
            )?,
            records: crate::json::optional_int(
                entry.get("records").unwrap_or(&Value::Null),
                &format!("{name}.records"),
            )?,
            compressed_sha256,
            compressed_bytes: crate::json::optional_int(
                entry.get("c_bytes").unwrap_or(&Value::Null),
                &format!("{name}.c_bytes"),
            )?,
        })
    }

    /// Every record of one projection, with the uncompressed
    /// stream's digest reproduced over the inflated bytes.
    fn stream(&self, name: &str) -> Result<Vec<Map<String, Value>>, String> {
        self.stream_mode(name, true)
    }

    /// Every record of one projection; the digest is checked
    /// only on the oracle's end-to-end paths (and always when
    /// no sidecar vouches for the stream).
    fn stream_mode(
        &self,
        name: &str,
        verify_digest: bool,
    ) -> Result<Vec<Map<String, Value>>, String> {
        let declared = self
            .declared
            .get(name)
            .ok_or_else(|| format!("{name} is not a managed transcript projection"))?;
        // Without a sidecar nothing else vouches for the
        // stream, so the digest is reproduced on every path.
        let verify = verify_digest || is_link_or_missing(&declared.index_path);
        let data = self.inflate(declared, verify)?;
        let mut records = Vec::new();
        if data.is_empty() {
            return Ok(records);
        }
        if !data.ends_with(b"\n") {
            return Err(format!(
                "{}: shard does not end at a line boundary",
                declared.path.display()
            ));
        }
        for (number, line) in data[..data.len() - 1]
            .split(|byte| *byte == b'\n')
            .enumerate()
        {
            let where_ = format!("{} line {number}", declared.path.display());
            let line = std::str::from_utf8(line).map_err(|error| {
                format!(
                    "{where_}: not a JSON record: {}",
                    py_utf8_error(&error, line)
                )
            })?;
            let value = parse_json_lenient(line)
                .map_err(|error| format!("{where_}: not a JSON record: {error}"))?;
            records.push(as_object(&value, &where_)?.clone());
        }
        Ok(records)
    }

    /// One projection inflated but not yet parsed: the list
    /// commands parse records on demand, the way the oracle's
    /// generators do, so a defect beyond the returned page is
    /// never read.
    fn line_source(&self, name: &str, verify_digest: bool) -> Result<LineSource, String> {
        let declared = self
            .declared
            .get(name)
            .ok_or_else(|| format!("{name} is not a managed transcript projection"))?;
        let data = self.inflate(declared, false)?;
        let mut lines: Vec<&[u8]> = Vec::new();
        if !data.is_empty() {
            if !data.ends_with(b"\n") {
                return Err(format!(
                    "{}: shard does not end at a line boundary",
                    declared.path.display()
                ));
            }
            lines = data[..data.len() - 1]
                .split(|byte| *byte == b'\n')
                .collect();
        }
        Ok(LineSource {
            name: declared.name.clone(),
            path: declared.path.display().to_string(),
            lines: lines.iter().map(|line| line.to_vec()).collect(),
            data,
            sha256: declared.sha256.clone(),
            check_on_exhaust: verify_digest || is_link_or_missing(&declared.index_path),
        })
    }

    fn inflate(&self, declared: &ManagedFile, verify_digest: bool) -> Result<Vec<u8>, String> {
        let incomplete = || {
            format!(
                "transcript export generation is incomplete: {}",
                declared.name
            )
        };
        if is_link_or_missing(&declared.path) {
            return Err(format!(
                "archive shard is missing or unsafe: {}",
                declared.path.display()
            ));
        }
        let compressed = std::fs::read(&declared.path)
            .map_err(|error| crate::json::os_error(&error, &declared.path))?;
        let data = if is_link_or_missing(&declared.index_path) {
            let mut decoded = Vec::new();
            match flate2::read::MultiGzDecoder::new(&compressed[..]).read_to_end(&mut decoded) {
                Ok(_) => decoded,
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(incomplete());
                }
                Err(error) => {
                    return Err(format!(
                        "{}: not a gzip member: {error}",
                        declared.path.display()
                    ));
                }
            }
        } else {
            let raw = std::fs::read(&declared.index_path)
                .map_err(|error| crate::json::os_error(&error, &declared.index_path))?;
            let text = match std::str::from_utf8(&raw) {
                Ok(text) => text.to_string(),
                Err(error) => {
                    return Err(format!(
                        "{}: malformed index: {}",
                        declared.index_path.display(),
                        py_utf8_error(&error, &raw)
                    ));
                }
            };
            let index = parse_chunk_index(&text, &declared.index_path.display().to_string())?;
            let file_name = declared
                .path
                .file_name()
                .map(|part| part.to_string_lossy().into_owned())
                .unwrap_or_default();
            if index.data_file != file_name {
                return Err(format!(
                    "{}: index describes {}, not {}",
                    declared.index_path.display(),
                    py_repr(&Value::String(index.data_file.clone())),
                    py_repr(&Value::String(file_name))
                ));
            }
            if compressed.len() as i64 != index.c_size {
                return Err(format!(
                    "{}: index describes {} compressed bytes but {} holds {}",
                    declared.index_path.display(),
                    index.c_size,
                    declared.path.display(),
                    compressed.len()
                ));
            }
            if index.timestamp_key != "timestamp_ms" {
                return Err(format!(
                    "{} is indexed on {}, not 'timestamp_ms'; a transcript seek would search the wrong axis",
                    file_name,
                    py_repr(&Value::String(index.timestamp_key.clone()))
                ));
            }
            if index.u_sha256 != declared.sha256 {
                return Err(format!(
                    "transcript export generation is incomplete: {} indexes a stream the manifest does not describe",
                    declared.name
                ));
            }
            if let Some(expected) = declared.declared_bytes {
                if index.u_size != expected {
                    return Err(format!(
                        "transcript export generation is incomplete: {} indexes {} uncompressed bytes, the manifest declares {expected}",
                        declared.name, index.u_size
                    ));
                }
            }
            if let Some(expected) = declared.records {
                if index.record_count != expected {
                    return Err(format!(
                        "transcript export generation is incomplete: {} indexes {} records, the manifest declares {expected}",
                        declared.name, index.record_count
                    ));
                }
            }
            let mut data = Vec::new();
            for member in &index.members {
                let start = member.c_off as usize;
                let available = compressed.len().saturating_sub(start);
                if available < member.c_len as usize {
                    return Err(format!(
                        "{}: member at {} is {available} bytes, index says {}",
                        declared.path.display(),
                        member.c_off,
                        member.c_len
                    ));
                }
                let slice = &compressed[start..start + member.c_len as usize];
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(slice)
                    .read_to_end(&mut decoded)
                    .map_err(|error| {
                        // zlib's spellings for the two failures a
                        // damaged member most plausibly hits.
                        let message = error.to_string();
                        let message = if message.contains("checksum") {
                            "CRC check failed".to_string()
                        } else if message.contains("unexpected end") {
                            "Compressed file ended before the end-of-stream marker was reached"
                                .to_string()
                        } else {
                            message
                        };
                        format!(
                            "{}: member at {} did not inflate: {message}",
                            declared.path.display(),
                            member.c_off
                        )
                    })?;
                if decoded.len() as i64 != member.u_len {
                    return Err(format!(
                        "{}: member at {} inflates to {} bytes, index says {}",
                        declared.path.display(),
                        member.c_off,
                        decoded.len(),
                        member.u_len
                    ));
                }
                if !decoded.is_empty() && !decoded.ends_with(b"\n") {
                    return Err(format!(
                        "{}: member does not end at a line boundary",
                        declared.path.display()
                    ));
                }
                data.extend_from_slice(&decoded);
            }
            data
        };
        if verify_digest && hex_digest(&data) != declared.sha256 {
            return Err(incomplete());
        }
        Ok(data)
    }

    /// `--verify`: reproduce every managed projection's
    /// compressed and uncompressed digests, consulted or not.
    pub fn verify_all(&self) -> Result<(), String> {
        for name in ["prompts.jsonl", "messages.jsonl", "system-inputs.jsonl"] {
            let declared = self
                .declared
                .get(name)
                .ok_or_else(|| format!("{name} is not a managed transcript projection"))?;
            if let Some(expected) = &declared.compressed_sha256 {
                let bytes = std::fs::read(&declared.path)
                    .map_err(|error| crate::json::os_error(&error, &declared.path))?;
                if hex_digest(&bytes) != *expected {
                    let file_name = declared
                        .path
                        .file_name()
                        .map(|part| part.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    return Err(format!(
                        "transcript export generation is incomplete: {file_name} does not hash to the compressed digest the manifest declares"
                    ));
                }
            }
            self.stream(name)?;
        }
        Ok(())
    }

    fn selected(
        record: &Map<String, Value>,
        filters: &Filters,
        window: &Option<Window>,
    ) -> Result<bool, String> {
        let team = as_string(
            record.get("team_slug").unwrap_or(&Value::Null),
            "transcript record.team_slug",
        )?;
        if !filters.teams.is_empty() && !filters.teams.iter().any(|t| t == team) {
            return Ok(false);
        }
        let Some(window) = window else {
            return Ok(true);
        };
        let at_ms = int_literal(
            record.get("timestamp_ms").unwrap_or(&Value::Null),
            "transcript record.timestamp_ms",
        )?;
        Ok(window.contains_dec(&at_ms))
    }

    /// `TranscriptQuery.content_stats`: the five prompt and
    /// response totals.
    pub fn content_stats(
        &self,
        filters: &Filters,
    ) -> Result<(TextTotals, TextTotals, TextTotals, TextTotals, TextTotals), String> {
        let window = parse_window(filters)?;
        let (mut human, mut bot, mut unattributed) = (
            TextTotals::default(),
            TextTotals::default(),
            TextTotals::default(),
        );
        for record in self.stream("prompts.jsonl")? {
            if !Self::selected(&record, filters, &window)? {
                continue;
            }
            let text = as_string(record.get("text").unwrap_or(&Value::Null), "prompt.text")?;
            let author = as_string(
                record.get("author_kind").unwrap_or(&Value::Null),
                "prompt.author_kind",
            )?;
            match author {
                "owner_human" | "other_human" => human.add_text(text),
                "agent" | "system" => bot.add_text(text),
                _ => unattributed.add_text(text),
            }
        }
        let (mut linked, mut unlinked) = (TextTotals::default(), TextTotals::default());
        for record in self.stream("messages.jsonl")? {
            let record_type = as_string(
                record.get("record_type").unwrap_or(&Value::Null),
                "message.record_type",
            )?;
            if record_type != "response" {
                continue;
            }
            if !Self::selected(&record, filters, &window)? {
                continue;
            }
            let text = as_string(record.get("text").unwrap_or(&Value::Null), "response.text")?;
            if matches!(record.get("in_reply_to_prompt_id"), Some(Value::String(_))) {
                linked.add_text(text);
            } else {
                unlinked.add_text(text);
            }
        }
        Ok((human, bot, unattributed, linked, unlinked))
    }
}

fn check_page(limit: Option<i64>, tail: Option<i64>) -> Result<(), String> {
    if let Some(limit) = limit {
        if limit < 1 {
            return Err("--limit must be at least 1".to_string());
        }
    }
    if let Some(tail) = tail {
        if tail < 1 {
            return Err("--tail must be at least 1".to_string());
        }
    }
    if limit.is_some() && tail.is_some() {
        return Err("--limit and --tail select opposite ends; choose one".to_string());
    }
    Ok(())
}

/// `parse_ordinal_range`: `N` or inclusive `N-M`.
pub fn parse_ordinal_range(raw: &str) -> Result<(i64, i64), String> {
    let parse = |text: &str| -> Option<i64> {
        if text.is_empty()
            || !text.bytes().all(|byte| byte.is_ascii_digit())
            || text.starts_with('0')
        {
            return None;
        }
        // Ordinals beyond i64 simply name no line.
        Some(text.parse::<i64>().unwrap_or(i64::MAX))
    };
    let (first, last) = match raw.split_once('-') {
        None => match parse(raw) {
            Some(value) => (value, value),
            None => {
                return Err("--range must be a positive ordinal or inclusive N-M range".to_string())
            }
        },
        Some((head, tail)) => {
            if tail.contains('-') {
                return Err("--range must be a positive ordinal or inclusive N-M range".to_string());
            }
            match (parse(head), parse(tail)) {
                (Some(first), Some(last)) => (first, last),
                _ => {
                    return Err(
                        "--range must be a positive ordinal or inclusive N-M range".to_string()
                    )
                }
            }
        }
    };
    if first > last {
        return Err("--range first ordinal must not exceed its last ordinal".to_string());
    }
    Ok((first, last))
}

/// Validate a window once, the way the CLI's filter
/// construction does before any reading.
pub fn check_window(filters: &Filters) -> Result<(), String> {
    parse_window(filters).map(|_| ())
}

impl TranscriptQuery {
    fn prompt_class(record: &Map<String, Value>) -> Result<String, String> {
        let author = as_string(
            record.get("author_kind").unwrap_or(&Value::Null),
            "prompt.author_kind",
        )?;
        Ok(match author {
            "owner_human" | "other_human" => "human".to_string(),
            "agent" | "system" => "bot".to_string(),
            _ => "unclassified".to_string(),
        })
    }

    fn accept_prompt(
        record: &Map<String, Value>,
        filters: &Filters,
        window: &Option<Window>,
        which: &str,
    ) -> Result<bool, String> {
        if !["human", "bot", "all"].contains(&which) {
            return Err("prompt selection must be human, bot, or all".to_string());
        }
        if which != "all" && Self::prompt_class(record)? != which {
            return Ok(false);
        }
        Self::selected(record, filters, window)
    }

    /// Records before the first line at or after the window
    /// end, parsed on demand from the end backwards, stopping
    /// once `tail` records have been accepted (`_tail_matching`
    /// at line rather than member granularity).
    #[allow(clippy::type_complexity)]
    fn tail_accepted(
        &self,
        name: &str,
        end_ms: Option<i64>,
        tail: i64,
        accept: &dyn Fn(&Map<String, Value>) -> Result<bool, String>,
    ) -> Result<Vec<Map<String, Value>>, String> {
        let source = self.line_source(name, false)?;
        let mut end_line = source.lines.len();
        if let Some(end) = end_ms {
            end_line = 0;
            for number in 0..source.lines.len() {
                let record = source.parse(number)?;
                let at = int_literal(
                    record.get("timestamp_ms").unwrap_or(&Value::Null),
                    "transcript record.timestamp_ms",
                )?;
                if dec_cmp(&at, &end.to_string()) != std::cmp::Ordering::Less {
                    break;
                }
                end_line = number + 1;
            }
        }
        let mut accepted: Vec<Map<String, Value>> = Vec::new();
        let mut number = end_line;
        while number > 0 && (accepted.len() as i64) < tail {
            number -= 1;
            let record = source.parse(number)?;
            if accept(&record)? {
                accepted.push(record);
            }
        }
        accepted.reverse();
        Ok(accepted)
    }

    /// `TranscriptQuery.list_prompts` in global timestamp order.
    pub fn list_prompts(
        &self,
        filters: &Filters,
        ordinal_range: Option<(i64, i64)>,
        which: &str,
        limit: Option<i64>,
        tail: Option<i64>,
    ) -> Result<Vec<Map<String, Value>>, String> {
        check_page(limit, tail)?;
        let window = parse_window(filters)?;
        let (start_ms, end_ms) = match &window {
            None => (None, None),
            Some(window) => (window.start, window.end),
        };
        if let (Some(tail), None, None) = (tail, ordinal_range, start_ms) {
            return self.tail_accepted("prompts.jsonl", end_ms, tail, &|record| {
                Self::accept_prompt(record, filters, &window, which)
            });
        }
        let source = self.line_source(
            "prompts.jsonl",
            start_ms.is_none() && ordinal_range.is_none(),
        )?;
        let mut result = Vec::new();
        let mut previous: Option<String> = None;
        let mut started = start_ms.is_none() && ordinal_range.is_none();
        let mut exhausted = false;
        let mut number = 0usize;
        if let Some((first, _)) = ordinal_range {
            number = (first - 1).max(0) as usize;
            started = true;
        }
        while number < source.lines.len() {
            let record = source.parse(number)?;
            if let Some((_, last)) = ordinal_range {
                let ordinal = int_literal(
                    record.get("ordinal").unwrap_or(&Value::Null),
                    "prompt.ordinal",
                )?;
                if dec_cmp(&ordinal, &last.to_string()) == std::cmp::Ordering::Greater {
                    break;
                }
                if ordinal != (number + 1).to_string() {
                    return Err(format!(
                        "{} line {number}: prompt ordinal is {ordinal}, not {}; the projection is not the dense chronological index its manifest declares, so an ordinal range cannot be located by line",
                        source.path,
                        number + 1
                    ));
                }
            }
            if start_ms.is_some() || end_ms.is_some() {
                let at = int_literal(
                    record.get("timestamp_ms").unwrap_or(&Value::Null),
                    "transcript record.timestamp_ms",
                )?;
                if !started {
                    if dec_cmp(&at, &start_ms.unwrap_or(0).to_string()) == std::cmp::Ordering::Less
                    {
                        number += 1;
                        continue;
                    }
                    started = true;
                } else if start_ms.is_some() {
                    if let Some(previous) = &previous {
                        if dec_cmp(&at, previous) == std::cmp::Ordering::Less {
                            return Err(format!(
                                "{} line {number}: records are not ordered by timestamp_ms, so a seeked read would under-return; rebuild the transcript export",
                                source.path
                            ));
                        }
                    }
                }
                previous = Some(at.clone());
                if let Some(end) = end_ms {
                    if dec_cmp(&at, &end.to_string()) != std::cmp::Ordering::Less {
                        break;
                    }
                }
            }
            if Self::accept_prompt(&record, filters, &window, which)? {
                result.push(record);
                if let Some(limit) = limit {
                    if result.len() >= limit as usize {
                        break;
                    }
                }
            }
            number += 1;
            if number >= source.lines.len() {
                exhausted = true;
            }
        }
        if source.lines.is_empty() {
            exhausted = true;
        }
        source.finish(exhausted)?;
        if let Some(tail) = tail {
            let skip = result.len().saturating_sub(tail as usize);
            return Ok(result[skip..].to_vec());
        }
        Ok(result)
    }

    fn message_belongs(
        record: &Map<String, Value>,
        selected_ids: &std::collections::BTreeSet<String>,
    ) -> Result<bool, String> {
        let record_type = as_string(
            record.get("record_type").unwrap_or(&Value::Null),
            "message.record_type",
        )?;
        match record_type {
            "prompt" => {
                let record_id = as_string(
                    record.get("record_id").unwrap_or(&Value::Null),
                    "prompt.record_id",
                )?;
                Ok(selected_ids.contains(record_id))
            }
            "response" => Ok(matches!(
                record.get("in_reply_to_prompt_id"),
                Some(Value::String(prompt_id)) if selected_ids.contains(prompt_id)
            )),
            other => Err(format!(
                "unknown transcript message type {}",
                py_repr(&Value::String(other.to_string()))
            )),
        }
    }

    /// `TranscriptQuery.list_messages`: selected prompts plus
    /// their mechanically associated responses.
    pub fn list_messages(
        &self,
        filters: &Filters,
        ordinal_range: Option<(i64, i64)>,
        which: &str,
        limit: Option<i64>,
        tail: Option<i64>,
        exhaustive: bool,
    ) -> Result<Vec<Map<String, Value>>, String> {
        check_page(limit, tail)?;
        let selected_prompts = self.list_prompts(filters, ordinal_range, which, None, None)?;
        if selected_prompts.is_empty() {
            return Ok(Vec::new());
        }
        // The oracle reads identity, ordinals, and timestamps
        // in three passes; field errors surface in that order.
        let mut selected_ids = std::collections::BTreeSet::new();
        for record in &selected_prompts {
            selected_ids.insert(
                as_string(
                    record.get("record_id").unwrap_or(&Value::Null),
                    "prompt.record_id",
                )?
                .to_string(),
            );
        }
        let mut prompt_ordinals: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for record in &selected_prompts {
            let record_id = as_string(
                record.get("record_id").unwrap_or(&Value::Null),
                "prompt.record_id",
            )?
            .to_string();
            let ordinal = int_literal(
                record.get("ordinal").unwrap_or(&Value::Null),
                "prompt.ordinal",
            )?;
            prompt_ordinals.insert(record_id, ordinal);
        }
        let mut earliest: Option<String> = None;
        for record in &selected_prompts {
            let at = int_literal(
                record.get("timestamp_ms").unwrap_or(&Value::Null),
                "prompt.timestamp_ms",
            )?;
            if earliest
                .as_ref()
                .is_none_or(|current| dec_cmp(&at, current) == std::cmp::Ordering::Less)
            {
                earliest = Some(at);
            }
        }
        let window = parse_window(filters)?;
        let (window_start, window_end) = match &window {
            None => (None, None),
            Some(window) => (window.start, window.end),
        };
        let lower: Option<String> = if exhaustive {
            window_start.map(|value| value.to_string())
        } else {
            match window_start {
                Some(start) => {
                    let earliest = earliest.clone().unwrap_or_default();
                    Some(
                        if dec_cmp(&earliest, &start.to_string()) == std::cmp::Ordering::Less {
                            start.to_string()
                        } else {
                            earliest
                        },
                    )
                }
                None => earliest.clone(),
            }
        };
        let project = |record: &Map<String, Value>| -> Result<Map<String, Value>, String> {
            let mut item = record.clone();
            let record_type = as_string(
                record.get("record_type").unwrap_or(&Value::Null),
                "message.record_type",
            )?;
            if record_type == "response" {
                let ordinal = match record.get("in_reply_to_prompt_id") {
                    Some(Value::String(prompt_id)) => prompt_ordinals
                        .get(prompt_id)
                        .map(|text| {
                            serde_json::Number::from_str(text)
                                .map(Value::Number)
                                .unwrap_or(Value::Null)
                        })
                        .unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                item.insert("prompt_ordinal".to_string(), ordinal);
            }
            Ok(item)
        };
        if let Some(tail) = tail {
            let accepted = self.tail_accepted("messages.jsonl", window_end, tail, &|record| {
                if !Self::message_belongs(record, &selected_ids)? {
                    return Ok(false);
                }
                Self::selected(record, filters, &window)
            })?;
            return accepted.iter().map(&project).collect();
        }
        let source = self.line_source("messages.jsonl", false)?;
        let mut result = Vec::new();
        let mut previous: Option<String> = None;
        let mut started = lower.is_none();
        let mut exhausted = false;
        for number in 0..source.lines.len() {
            let record = source.parse(number)?;
            let at = int_literal(
                record.get("timestamp_ms").unwrap_or(&Value::Null),
                "transcript record.timestamp_ms",
            )?;
            if !started {
                if dec_cmp(&at, lower.as_deref().unwrap_or("0")) == std::cmp::Ordering::Less {
                    continue;
                }
                started = true;
            } else if let Some(previous) = &previous {
                if dec_cmp(&at, previous) == std::cmp::Ordering::Less {
                    return Err(format!(
                        "{} line {number}: records are not ordered by timestamp_ms, so a seeked read would under-return; rebuild the transcript export",
                        source.path
                    ));
                }
            }
            previous = Some(at.clone());
            if let Some(end) = window_end {
                if dec_cmp(&at, &end.to_string()) != std::cmp::Ordering::Less {
                    break;
                }
            }
            let belongs = Self::message_belongs(&record, &selected_ids)?;
            if belongs && Self::selected(&record, filters, &window)? {
                result.push(project(&record)?);
                if let Some(limit) = limit {
                    if result.len() >= limit as usize {
                        break;
                    }
                }
            }
            if number + 1 >= source.lines.len() {
                exhausted = true;
            }
        }
        if source.lines.is_empty() {
            exhausted = true;
        }
        source.finish(exhausted)?;
        Ok(result)
    }
}

/// `ArchiveStats` plus its rendering (`format_stats`).
pub struct ArchiveStats {
    human_prompts: TextTotals,
    bot_prompts: TextTotals,
    unattributed_prompts: TextTotals,
    linked_responses: TextTotals,
    unlinked_responses: TextTotals,
    summary_kinds: Vec<(String, SummaryKindStats)>,
    teams: Vec<String>,
    time_filtered: bool,
    rollup_kinds: Vec<String>,
}

impl ArchiveStats {
    fn all_prompts(&self) -> TextTotals {
        self.human_prompts
            .plus(&self.bot_prompts)
            .plus(&self.unattributed_prompts)
    }

    fn total_responses(&self) -> TextTotals {
        self.linked_responses.plus(&self.unlinked_responses)
    }

    fn generated_summaries(&self) -> TextTotals {
        let mut total = TextTotals::default();
        for (_, item) in &self.summary_kinds {
            total = total.plus(&item.content);
        }
        total
    }

    fn to_mapping(&self) -> Value {
        let mut content = Map::new();
        content.insert(
            "human_prompts".to_string(),
            Value::Object(self.human_prompts.to_map()),
        );
        content.insert(
            "bot_prompts".to_string(),
            Value::Object(self.bot_prompts.to_map()),
        );
        content.insert(
            "unattributed_prompts".to_string(),
            Value::Object(self.unattributed_prompts.to_map()),
        );
        content.insert(
            "all_prompts".to_string(),
            Value::Object(self.all_prompts().to_map()),
        );
        content.insert(
            "mechanically_linked_responses".to_string(),
            Value::Object(self.linked_responses.to_map()),
        );
        content.insert(
            "unlinked_responses".to_string(),
            Value::Object(self.unlinked_responses.to_map()),
        );
        content.insert(
            "total_responses".to_string(),
            Value::Object(self.total_responses().to_map()),
        );
        let generated = self.generated_summaries();
        content.insert(
            "generated_summaries".to_string(),
            Value::Object(generated.to_map()),
        );
        let prompts_and_responses = self.all_prompts().plus(&self.total_responses());
        content.insert(
            "prompts_and_responses".to_string(),
            Value::Object(prompts_and_responses.to_map()),
        );
        content.insert(
            "all_counted_content".to_string(),
            Value::Object(prompts_and_responses.plus(&generated).to_map()),
        );
        let mut by_kind = Map::new();
        let (mut available, mut unavailable) = (0i64, 0i64);
        for (key, item) in &self.summary_kinds {
            by_kind.insert(key.clone(), Value::Object(item.to_map()));
            available += item.available;
            unavailable += item.unavailable;
        }
        let mut summary_availability = Map::new();
        summary_availability.insert("available".to_string(), Value::from(available));
        summary_availability.insert("unavailable".to_string(), Value::from(unavailable));
        summary_availability.insert("total".to_string(), Value::from(available + unavailable));
        summary_availability.insert("by_kind".to_string(), Value::Object(by_kind));
        let mut scope = Map::new();
        scope.insert(
            "teams".to_string(),
            Value::Array(self.teams.iter().cloned().map(Value::String).collect()),
        );
        scope.insert("all_teams".to_string(), Value::Bool(self.teams.is_empty()));
        scope.insert("time_filtered".to_string(), Value::Bool(self.time_filtered));
        scope.insert(
            "rollup_kinds".to_string(),
            Value::Array(
                self.rollup_kinds
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        let mut counting_contract = Map::new();
        for (key, value) in [
            ("words", "Unicode text split on whitespace"),
            (
                "utf8_bytes",
                "UTF-8 encoded text only; serialization framing excluded",
            ),
            (
                "rollup_text",
                "referenced generated Markdown counted verbatim",
            ),
            ("phase_summary_text", "phrase plus newline plus paragraph"),
            (
                "time_unbounded_project_overviews",
                "included only when no time filter is active",
            ),
            (
                "human_prompts",
                "records labeled owner_human or other_human",
            ),
            ("bot_prompts", "records labeled agent or system"),
            (
                "unattributed_prompts",
                "all remaining prompt authorship labels",
            ),
        ] {
            counting_contract.insert(key.to_string(), Value::String(value.to_string()));
        }
        let mut root = Map::new();
        root.insert("schema_version".to_string(), Value::from(1));
        root.insert("command".to_string(), Value::String("stats".to_string()));
        root.insert("scope".to_string(), Value::Object(scope));
        root.insert("content".to_string(), Value::Object(content));
        root.insert(
            "summary_availability".to_string(),
            Value::Object(summary_availability),
        );
        root.insert(
            "counting_contract".to_string(),
            Value::Object(counting_contract),
        );
        Value::Object(root)
    }
}

/// `archive_stats`: verified transcript totals, then summary
/// totals, in the oracle's evaluation order.
pub fn archive_stats(root: &Path, filters: &Filters) -> Result<ArchiveStats, String> {
    // The CLI builds its filters (and so validates the window)
    // before the archive is opened.
    let window = parse_window(filters)?;
    let transcripts = TranscriptQuery::open(root)?;
    let (human, bot, unattributed, linked, unlinked) = transcripts.content_stats(filters)?;
    let timeline = Timeline::open(root)?;
    let summary_kinds = timeline.summary_stats(filters)?;
    Ok(ArchiveStats {
        human_prompts: human,
        bot_prompts: bot,
        unattributed_prompts: unattributed,
        linked_responses: linked,
        unlinked_responses: unlinked,
        summary_kinds,
        teams: filters.teams.clone(),
        time_filtered: window.is_some(),
        rollup_kinds: filters.kinds.clone(),
    })
}

fn commas(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn stats_scope(stats: &ArchiveStats) -> String {
    let teams = if stats.teams.is_empty() {
        "all teams".to_string()
    } else {
        stats.teams.join(", ")
    };
    let time_scope = if stats.time_filtered {
        "filtered time window"
    } else {
        "all time"
    };
    let rollups = if stats.rollup_kinds.is_empty() {
        "all rollup kinds".to_string()
    } else {
        stats.rollup_kinds.join(", ")
    };
    format!("{teams}; {time_scope}; {rollups}")
}

fn stats_content_rows(stats: &ArchiveStats) -> Vec<(String, TextTotals)> {
    vec![
        ("Identified human prompts".to_string(), stats.human_prompts),
        (
            "Identified bot/agent prompts".to_string(),
            stats.bot_prompts,
        ),
        (
            "Unattributed prompt records".to_string(),
            stats.unattributed_prompts,
        ),
        ("All prompt records".to_string(), stats.all_prompts()),
        (
            "Mechanically linked responses".to_string(),
            stats.linked_responses,
        ),
        ("Unlinked responses".to_string(), stats.unlinked_responses),
        ("Total responses".to_string(), stats.total_responses()),
        (
            "Generated summaries".to_string(),
            stats.generated_summaries(),
        ),
        (
            "Prompts + responses".to_string(),
            stats.all_prompts().plus(&stats.total_responses()),
        ),
        (
            "All counted content".to_string(),
            stats
                .all_prompts()
                .plus(&stats.total_responses())
                .plus(&stats.generated_summaries()),
        ),
    ]
}

const STATS_FOOTER: &str = "Words are whitespace-delimited Unicode text. Byte totals are UTF-8 text bytes and exclude CLI/JSON serialization framing; generated rollup Markdown is counted verbatim.";

/// `format_stats` for one stats report.
pub fn format_stats(stats: &ArchiveStats, format: &str) -> Result<String, String> {
    match format {
        "json" => Ok(dump_canonical(&stats.to_mapping())),
        "jsonl" => Ok(format!("{}\n", dump_oneline(&stats.to_mapping()))),
        "markdown" => {
            let mut lines = vec![
                "# Archive content statistics".to_string(),
                String::new(),
                format!("Scope: {}.", stats_scope(stats)),
                String::new(),
                "| Content | Records | Words | UTF-8 bytes |".to_string(),
                "| --- | ---: | ---: | ---: |".to_string(),
            ];
            for (label, totals) in stats_content_rows(stats) {
                lines.push(format!(
                    "| {label} | {} | {} | {} |",
                    commas(totals.records),
                    commas(totals.words),
                    commas(totals.utf8_bytes)
                ));
            }
            lines.extend([
                String::new(),
                "## Summary availability".to_string(),
                String::new(),
                "| Kind | Available | Unavailable | Total |".to_string(),
                "| --- | ---: | ---: | ---: |".to_string(),
            ]);
            let (mut available, mut unavailable) = (0i64, 0i64);
            for (key, item) in &stats.summary_kinds {
                lines.push(format!(
                    "| {} | {} | {} | {} |",
                    title_case(key),
                    commas(item.available),
                    commas(item.unavailable),
                    commas(item.available + item.unavailable)
                ));
                available += item.available;
                unavailable += item.unavailable;
            }
            lines.push(format!(
                "| **All summary slots** | **{}** | **{}** | **{}** |",
                commas(available),
                commas(unavailable),
                commas(available + unavailable)
            ));
            lines.push(String::new());
            lines.push(STATS_FOOTER.to_string());
            Ok(format!("{}\n", lines.join("\n")))
        }
        "text" => {
            let mut lines = vec![
                "Archive content statistics".to_string(),
                format!("Scope: {}", stats_scope(stats)),
                String::new(),
                format!(
                    "{:<34} {:>12} {:>14} {:>16}",
                    "Content", "Records", "Words", "UTF-8 bytes"
                ),
                format!(
                    "{} {} {} {}",
                    "-".repeat(34),
                    "-".repeat(12),
                    "-".repeat(14),
                    "-".repeat(16)
                ),
            ];
            for (label, totals) in stats_content_rows(stats) {
                lines.push(format!(
                    "{:<34} {:>12} {:>14} {:>16}",
                    label,
                    commas(totals.records),
                    commas(totals.words),
                    commas(totals.utf8_bytes)
                ));
            }
            lines.extend([
                String::new(),
                "Summary availability".to_string(),
                format!(
                    "{:<34} {:>12} {:>14} {:>16}",
                    "Kind", "Available", "Unavailable", "Total"
                ),
                format!(
                    "{} {} {} {}",
                    "-".repeat(34),
                    "-".repeat(12),
                    "-".repeat(14),
                    "-".repeat(16)
                ),
            ]);
            let (mut available, mut unavailable) = (0i64, 0i64);
            for (key, item) in &stats.summary_kinds {
                lines.push(format!(
                    "{:<34} {:>12} {:>14} {:>16}",
                    title_case(key),
                    commas(item.available),
                    commas(item.unavailable),
                    commas(item.available + item.unavailable)
                ));
                available += item.available;
                unavailable += item.unavailable;
            }
            lines.push(format!(
                "{:<34} {:>12} {:>14} {:>16}",
                "All summary slots",
                commas(available),
                commas(unavailable),
                commas(available + unavailable)
            ));
            lines.push(String::new());
            lines.push(STATS_FOOTER.to_string());
            Ok(format!("{}\n", lines.join("\n")))
        }
        other => Err(format!(
            "unsupported query output format {}",
            py_repr(&Value::String(other.to_string()))
        )),
    }
}

fn is_search_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{9}'..='\u{d}'
            | '\u{1c}'..='\u{20}'
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// `_compact_text`: whitespace runs collapsed to one space.
fn compact_text(value: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in value.chars() {
        if is_search_whitespace(ch) {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// CPython `str.casefold()` for the compatibility search:
/// the full-folding expansions plus ordinary lowering.
fn py_casefold(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\u{df}' | '\u{1e9e}' => out.push_str("ss"),
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            '\u{130}' => out.push_str("i\u{307}"),
            _ => out.extend(ch.to_lowercase()),
        }
    }
    out
}

fn ascii_lower_utf8(value: &str) -> Vec<u8> {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_uppercase() {
                byte + 32
            } else {
                byte
            }
        })
        .collect()
}

struct SearchLinkContext {
    prompt_excerpts: std::collections::BTreeMap<String, String>,
    response_counts: std::collections::BTreeMap<String, i64>,
}

struct TextMatch {
    compact: Vec<char>,
    ranges: Vec<(usize, usize)>,
    score: i64,
}

struct SearchMatcher {
    compact_needle: Vec<char>,
    parts: Vec<(Vec<char>, bool)>,
    bloom_terms: Vec<String>,
    case_sensitive: bool,
}

fn find_literal_ranges(
    compact: &[char],
    part: &[char],
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<(usize, usize)> {
    let fold = |ch: char| {
        if case_sensitive {
            ch
        } else {
            ch.to_ascii_lowercase()
        }
    };
    let word_char = |ch: char| ch == '_' || ch.is_alphanumeric();
    let mut out = Vec::new();
    if part.is_empty() || part.len() > compact.len() {
        return out;
    }
    let mut start = 0usize;
    while start + part.len() <= compact.len() {
        let matches =
            (0..part.len()).all(|offset| fold(compact[start + offset]) == fold(part[offset]));
        if matches {
            let end = start + part.len();
            let blocked = whole_word
                && ((start > 0 && word_char(compact[start - 1]))
                    || (end < compact.len() && word_char(compact[end])));
            if !blocked {
                out.push((start, end));
                if out.len() == 64 {
                    break;
                }
            }
            start = end;
        } else {
            start += 1;
        }
    }
    out
}

fn smart_parts(needle: &str) -> Vec<(String, bool)> {
    let chars: Vec<char> = needle.chars().collect();
    let mut parts = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == ' ' {
            index += 1;
            continue;
        }
        if chars[index] == '"' {
            // Try the quoted alternative first.
            let mut cursor = index + 1;
            let mut content: Vec<char> = Vec::new();
            let mut closed = false;
            while cursor < chars.len() {
                let ch = chars[cursor];
                if ch == '\\' && cursor + 1 < chars.len() {
                    content.push(ch);
                    content.push(chars[cursor + 1]);
                    cursor += 2;
                    continue;
                }
                if ch == '"' {
                    closed = true;
                    break;
                }
                content.push(ch);
                cursor += 1;
            }
            if closed {
                let raw: String = content.iter().collect();
                let value = raw.replace("\\\"", "\"").replace("\\\\", "\\");
                if !value.is_empty() {
                    parts.push((value, true));
                }
                index = cursor + 1;
                continue;
            }
        }
        let start = index;
        while index < chars.len() && chars[index] != ' ' {
            index += 1;
        }
        let value: String = chars[start..index].iter().collect();
        if !value.is_empty() {
            parts.push((value, false));
        }
    }
    parts
}

fn compile_search_matcher(
    needle: &str,
    match_mode: &str,
    case_sensitive: bool,
) -> Result<SearchMatcher, String> {
    let compact_needle = compact_text(needle);
    if compact_needle.is_empty() {
        return Err("search text must not be empty".to_string());
    }
    let needle_chars: Vec<char> = compact_needle.chars().collect();
    let (parts, bloom_terms): (Vec<(Vec<char>, bool)>, Vec<String>) = match match_mode {
        "literal" | "phrase" => (
            vec![(needle_chars.clone(), false)],
            vec![compact_needle.clone()],
        ),
        "smart" => {
            let raw_parts = smart_parts(&compact_needle);
            if raw_parts.is_empty() {
                return Err("search text must contain a term".to_string());
            }
            let mut parts = Vec::new();
            let mut terms = Vec::new();
            for (part, quoted) in raw_parts {
                let whole = !quoted
                    && part
                        .chars()
                        .all(|ch| ch == '_' || ch == '-' || ch.is_alphanumeric());
                terms.push(part.clone());
                parts.push((part.chars().collect::<Vec<char>>(), whole));
            }
            (parts, terms)
        }
        other => {
            return Err(format!(
                "unsupported search match mode {}",
                py_repr(&Value::String(other.to_string()))
            ))
        }
    };
    Ok(SearchMatcher {
        compact_needle: needle_chars,
        parts,
        bloom_terms,
        case_sensitive,
    })
}

impl SearchMatcher {
    fn match_text(&self, text: &str) -> Option<TextMatch> {
        let compact: Vec<char> = compact_text(text).chars().collect();
        let mut collected: Vec<(usize, usize)> = Vec::new();
        for (part, whole) in &self.parts {
            let candidate = find_literal_ranges(&compact, part, self.case_sensitive, *whole);
            if candidate.is_empty() {
                return None;
            }
            collected.extend(candidate);
        }
        collected.sort();
        collected.dedup();
        collected.truncate(64);
        let first = collected.iter().map(|(start, _)| *start).min().unwrap_or(0);
        let last = collected.iter().map(|(_, end)| *end).max().unwrap_or(0);
        let span = (last as i64 - first as i64).max(1);
        let comparable: Vec<char> = if self.case_sensitive {
            compact.clone()
        } else {
            compact.iter().map(|ch| ch.to_ascii_lowercase()).collect()
        };
        let comparable_needle: Vec<char> = if self.case_sensitive {
            self.compact_needle.clone()
        } else {
            self.compact_needle
                .iter()
                .map(|ch| ch.to_ascii_lowercase())
                .collect()
        };
        let exact = comparable == comparable_needle;
        let score = (if exact { 100_000 } else { 0 })
            + (20_000 - span).max(0)
            + collected.len() as i64 * 100
            - (first as i64).min(10_000);
        Some(TextMatch {
            compact,
            ranges: collected,
            score,
        })
    }
}

fn search_excerpt(matched: &TextMatch) -> Map<String, Value> {
    let first = matched
        .ranges
        .iter()
        .map(|(start, _)| *start)
        .min()
        .unwrap_or(0);
    let start = first.saturating_sub(120);
    let end = (start + 480).min(matched.compact.len());
    let excerpt: String = matched.compact[start..end].iter().collect();
    let mut ranges = Vec::new();
    for (left, right) in &matched.ranges {
        if *right <= start || *left >= end {
            continue;
        }
        ranges.push(Value::Array(vec![
            Value::from(left.saturating_sub(start) as i64),
            Value::from(((*right).min(end) - start) as i64),
        ]));
    }
    let mut out = Map::new();
    out.insert("text".to_string(), Value::String(excerpt));
    out.insert(
        "full_characters".to_string(),
        Value::from(matched.compact.len() as i64),
    );
    out.insert(
        "leading_omitted_characters".to_string(),
        Value::from(start as i64),
    );
    out.insert(
        "trailing_omitted_characters".to_string(),
        Value::from((matched.compact.len() - end) as i64),
    );
    out.insert(
        "is_truncated".to_string(),
        Value::Bool(start > 0 || end < matched.compact.len()),
    );
    out.insert("match_ranges".to_string(), Value::Array(ranges));
    out
}

fn base64_decode(encoded: &str, where_: &str) -> Result<Vec<u8>, String> {
    let invalid = || format!("{where_}.bits_base64: invalid base64");
    let mut table = [255u8; 256];
    for (index, byte) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        table[*byte as usize] = index as u8;
    }
    let bytes = encoded.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(invalid());
    }
    let mut out = Vec::new();
    let chunks = bytes.len() / 4;
    for chunk_index in 0..chunks {
        let chunk = &bytes[chunk_index * 4..chunk_index * 4 + 4];
        let last = chunk_index + 1 == chunks;
        let mut values = [0u8; 4];
        let mut padding = 0;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte == b'=' {
                if !last || index < 2 {
                    return Err(invalid());
                }
                padding += 1;
            } else {
                if padding > 0 || table[*byte as usize] == 255 {
                    return Err(invalid());
                }
                values[index] = table[*byte as usize];
            }
        }
        if padding > 2 {
            return Err(invalid());
        }
        let triple = ((values[0] as u32) << 18)
            | ((values[1] as u32) << 12)
            | ((values[2] as u32) << 6)
            | values[3] as u32;
        out.push((triple >> 16) as u8);
        if padding < 2 {
            out.push((triple >> 8) as u8);
        }
        if padding < 1 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

fn trigram_positions(trigram: &[u8], bit_count: i64, hash_count: i64) -> Vec<i64> {
    let digest = |seed: u32| -> u32 {
        let mut result = seed;
        for byte in trigram {
            result ^= *byte as u32;
            result = result.wrapping_mul(16_777_619);
        }
        result
    };
    let first = digest(2_166_136_261);
    let second = digest(2_166_136_261 ^ 0x9E37_79B9) | 1;
    let mask = (bit_count - 1) as u32;
    (0..hash_count)
        .map(|index| (first.wrapping_add((index as u32).wrapping_mul(second)) & mask) as i64)
        .collect()
}

fn bloom_can_prune(term: &str) -> bool {
    let compact = compact_text(term);
    compact.is_ascii() && ascii_lower_utf8(&compact).len() >= 3
}

fn bloom_might_match(raw: Option<&Value>, terms: &[String], where_: &str) -> Result<bool, String> {
    let Some(raw) = raw else {
        return Ok(true);
    };
    let value = as_object(raw, where_)?;
    let algorithm = as_string(
        value.get("algorithm").unwrap_or(&Value::Null),
        &format!("{where_}.algorithm"),
    )?;
    if algorithm != "ascii-lower-utf8-trigram-fnv1a32-double-v1" {
        return Err(format!(
            "{where_}.algorithm: unsupported value {}",
            py_repr(&Value::String(algorithm.to_string()))
        ));
    }
    let bit_count = as_int(
        value.get("bit_count").unwrap_or(&Value::Null),
        &format!("{where_}.bit_count"),
    )?;
    if bit_count < 64 || (bit_count & (bit_count - 1)) != 0 {
        return Err(format!(
            "{where_}.bit_count: expected a power of two of at least 64"
        ));
    }
    let hash_count = as_int(
        value.get("hash_count").unwrap_or(&Value::Null),
        &format!("{where_}.hash_count"),
    )?;
    if hash_count != 7 {
        return Err(format!("{where_}.hash_count: expected 7"));
    }
    let trigram_count = as_int(
        value.get("trigram_count").unwrap_or(&Value::Null),
        &format!("{where_}.trigram_count"),
    )?;
    if trigram_count < 0 {
        return Err(format!(
            "{where_}.trigram_count: expected a non-negative integer"
        ));
    }
    let encoded = as_string(
        value.get("bits_base64").unwrap_or(&Value::Null),
        &format!("{where_}.bits_base64"),
    )?;
    let bits = base64_decode(encoded, where_)?;
    if bits.len() as i64 * 8 != bit_count {
        return Err(format!(
            "{where_}.bits_base64: decoded {} bits, expected {bit_count}",
            bits.len() * 8
        ));
    }
    for term in terms {
        let compact = compact_text(term);
        if !compact.is_ascii() {
            continue;
        }
        let term_bytes = ascii_lower_utf8(&compact);
        if term_bytes.len() < 3 {
            continue;
        }
        for offset in 0..term_bytes.len() - 2 {
            let trigram = &term_bytes[offset..offset + 3];
            for position in trigram_positions(trigram, bit_count, hash_count) {
                if bits[(position / 8) as usize] & (1 << (position % 8)) == 0 {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn day_bounds(day: &str, where_: &str) -> Result<(i64, i64), String> {
    let parsed = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| {
        format!(
            "{where_}: not a UTC day label: {}",
            py_repr(&Value::String(day.to_string()))
        )
    })?;
    // The label must be exactly the canonical spelling.
    if parsed.format("%Y-%m-%d").to_string() != day {
        return Err(format!(
            "{where_}: not a UTC day label: {}",
            py_repr(&Value::String(day.to_string()))
        ));
    }
    let start_ms = parsed
        .and_hms_opt(0, 0, 0)
        .map(|moment| moment.and_utc().timestamp_millis())
        .unwrap_or(0);
    Ok((start_ms, start_ms + 86_400_000))
}

/// One ranked search page plus result-set metadata
/// (`SearchResults`).
pub struct SearchResults {
    /// The page's items.
    pub items: Vec<Map<String, Value>>,
    /// Matches across the whole result set.
    pub total_matches: i64,
    /// The requested offset.
    pub offset: i64,
    /// The offset as CPython's unbounded `int` spells it.
    pub offset_display: String,
    /// The requested limit.
    pub limit: i64,
    /// The search corpus.
    pub corpus: String,
    /// The match mode.
    pub match_mode: String,
    /// The sort order.
    pub sort: String,
}

impl SearchResults {
    fn truncated(&self) -> bool {
        self.offset > 0 || self.offset + (self.items.len() as i64) < self.total_matches
    }
}

/// `format_search_results` for one search page.
pub fn format_search_results(
    results: &SearchResults,
    command: &str,
    format: &str,
) -> Result<String, String> {
    match format {
        "json" => {
            let mut envelope = Map::new();
            envelope.insert("schema_version".to_string(), Value::from(1));
            envelope.insert("search_schema_version".to_string(), Value::from(1));
            envelope.insert("command".to_string(), Value::String(command.to_string()));
            envelope.insert("corpus".to_string(), Value::String(results.corpus.clone()));
            envelope.insert(
                "match_mode".to_string(),
                Value::String(results.match_mode.clone()),
            );
            envelope.insert("sort".to_string(), Value::String(results.sort.clone()));
            envelope.insert(
                "total_matches".to_string(),
                Value::from(results.total_matches),
            );
            envelope.insert(
                "offset".to_string(),
                serde_json::Number::from_str(&results.offset_display)
                    .map(Value::Number)
                    .unwrap_or(Value::from(results.offset)),
            );
            envelope.insert(
                "returned".to_string(),
                Value::from(results.items.len() as i64),
            );
            envelope.insert("truncated".to_string(), Value::Bool(results.truncated()));
            envelope.insert(
                "items".to_string(),
                Value::Array(results.items.iter().cloned().map(Value::Object).collect()),
            );
            Ok(dump_canonical(&Value::Object(envelope)))
        }
        "jsonl" => Ok(results
            .items
            .iter()
            .map(|item| format!("{}\n", dump_oneline(&Value::Object(item.clone()))))
            .collect()),
        "markdown" => {
            let body = results
                .items
                .iter()
                .map(markdown_item)
                .collect::<Result<Vec<_>, _>>()?
                .join("\n\n");
            Ok(format!(
                "# wrkviz query: {command}\n\n{body}\n\n_Showing {} of {} result(s) from offset {}._\n",
                results.items.len(),
                results.total_matches,
                results.offset_display
            ))
        }
        "text" => {
            let body = results
                .items
                .iter()
                .map(text_item)
                .collect::<Vec<_>>()
                .join("\n\n");
            Ok(format!(
                "Showing {} of {} result(s) from offset {}.\n\n{body}{}",
                results.items.len(),
                results.total_matches,
                results.offset_display,
                if body.is_empty() { "" } else { "\n" }
            ))
        }
        other => Err(format!(
            "unsupported query output format {}",
            py_repr(&Value::String(other.to_string()))
        )),
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
                .collect::<Result<Vec<_>, String>>()?
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

fn markdown_item(item: &Map<String, Value>) -> Result<String, String> {
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
            Some(Value::Number(number)) => lines.push(format!(
                "- {}: `{}`",
                title_case(key),
                py_number_str(number)
            )),
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
            for (index, raw) in transcript.iter().enumerate() {
                let where_ = format!("detail.transcript[{index}]");
                let message = as_object(raw, &where_)?;
                let role = as_string(
                    message.get("role").unwrap_or(&Value::Null),
                    &format!("{where_}.role"),
                )?;
                let at_ms = as_int(
                    message.get("at_ms").unwrap_or(&Value::Null),
                    &format!("{where_}.at_ms"),
                )?;
                let text = as_string(
                    message.get("text").unwrap_or(&Value::Null),
                    &format!("{where_}.text"),
                )?;
                lines.push(format!("#### {role} · {}", timestamp(at_ms)));
                lines.push(String::new());
                lines.push(text.trim_end().to_string());
                lines.push(String::new());
            }
        }
    }
    Ok(lines.join("\n").trim_end().to_string())
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
