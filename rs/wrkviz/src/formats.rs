//! The single source of truth for the wrkviz schema-3 archive records.
//!
//! These types define the timeline-v3 bootstrap (`data/timeline-v3.json`)
//! and its shard catalogue, as the writers emit them. The JSON Schemas
//! under `validation/schemas/` are generated from these types and
//! checked in. Readers in both editions accept documents the schema
//! refuses (unknown keys, omitted writer-only fields); the schema is
//! the writer contract, and the readers add the cross-field rules a
//! schema cannot express (shard coverage, codec agreement, team
//! coverage, search-stream completeness).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn schema_three(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "integer", "const": 3}))
        .expect("schema-three schema is valid")
}

fn bootstrap_kind(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "string", "const": "timeline-v3-bootstrap"}))
        .expect("bootstrap kind schema is valid")
}

fn const_container(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "string", "const": "multi-member-gzip"}))
        .expect("container schema is valid")
}

fn const_timestamp_key(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "string", "const": "at_ms"}))
        .expect("timestamp key schema is valid")
}

fn const_record_kind_key(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "string", "const": "record_kind"}))
        .expect("record kind key schema is valid")
}

fn const_index_suffix(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "string", "const": ".index.jsonl"}))
        .expect("index suffix schema is valid")
}

/// The codec block of the bootstrap; every value is fixed by the format.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Codec {
    /// Container format; always `multi-member-gzip`.
    #[schemars(schema_with = "const_container")]
    pub container: String,
    /// Compression level the writer used.
    pub level: i64,
    /// Target chunk size in bytes.
    pub target_chunk_bytes: u64,
    /// Record key carrying the timestamp; always `at_ms`.
    #[schemars(schema_with = "const_timestamp_key")]
    pub timestamp_key: String,
    /// Record key carrying the kind; always `record_kind`.
    #[schemars(schema_with = "const_record_kind_key")]
    pub record_kind_key: String,
    /// Index file suffix; always `.index.jsonl`.
    #[schemars(schema_with = "const_index_suffix")]
    pub index_suffix: String,
}

/// One shard entry in a stream's catalogue.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShardCatalogEntry {
    /// Stream this shard belongs to.
    pub stream: String,
    /// Team slug, or null for team-less streams.
    pub team: Option<String>,
    /// UTC day label, or null.
    pub day: Option<String>,
    /// Archive-relative shard path.
    pub path: String,
    /// Archive-relative index path.
    pub index_path: String,
    /// Record count.
    #[schemars(range(min = 0))]
    pub records: i64,
    /// Gzip member count.
    #[schemars(range(min = 0))]
    pub members: i64,
    /// Compressed size in bytes.
    #[schemars(range(min = 0))]
    pub c_bytes: i64,
    /// Uncompressed size in bytes.
    #[schemars(range(min = 0))]
    pub u_bytes: i64,
    /// SHA-256 of the compressed bytes, lowercase hex.
    #[schemars(length(equal = 64))]
    pub c_sha256: String,
    /// SHA-256 of the uncompressed bytes, lowercase hex.
    #[schemars(length(equal = 64))]
    pub u_sha256: String,
    /// Whether member timestamps are sorted.
    pub timestamps_sorted: bool,
    /// Earliest timestamp in milliseconds, or null.
    pub t0: Option<i64>,
    /// Latest timestamp in milliseconds, or null.
    pub t1: Option<i64>,
    /// End of the shard's time range (exclusive), or null.
    pub t_end_exclusive: Option<i64>,
    /// Per-record-kind `[first_line, count]` ranges, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_ranges: Option<BTreeMap<String, [u64; 2]>>,
    /// Per-record-kind counts, when ranges are not recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<BTreeMap<String, i64>>,
}

/// One stream section of the bootstrap.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamSection {
    /// How shards are addressed (per team/day, time range, or line range).
    pub addressing: String,
    /// Sort order description.
    pub sort: String,
    /// Record kinds carried by the stream.
    pub record_kinds: Vec<String>,
    /// The stream's shards.
    pub shards: Vec<ShardCatalogEntry>,
}

/// The stream sections of the bootstrap. The three base streams are
/// always present; the three search streams appear together or not at all.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Streams {
    /// Timeline event shards.
    pub timeline: StreamSection,
    /// Spine shards.
    pub spine: StreamSection,
    /// Activity bin shards.
    pub bins: StreamSection,
    /// Search record shards, when the search corpus is published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<StreamSection>,
    /// Search bloom shards, with `search`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_bloom: Option<StreamSection>,
    /// Search link shards, with `search`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_links: Option<StreamSection>,
}

/// The schema-3 bootstrap: `data/timeline-v3.json` in an archive.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineV3Bootstrap {
    /// Format schema; always 3.
    #[schemars(schema_with = "schema_three")]
    pub schema_version: u64,
    /// Document kind; always `timeline-v3-bootstrap`.
    #[schemars(schema_with = "bootstrap_kind")]
    pub kind: String,
    /// When the generation was built, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<Value>,
    /// Digest of the source timeline, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Value>,
    /// Display timezone, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_timezone: Option<Value>,
    /// Where the display timezone came from, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_timezone_source: Option<Value>,
    /// Covered time range, when recorded (shape owned by the timeline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Value>,
    /// Build statistics, when recorded (shape owned by the timeline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<Value>,
    /// Artifact catalogue path, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_catalog_path: Option<Value>,
    /// Glossary path, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glossary_path: Option<Value>,
    /// Codec block.
    pub codec: Codec,
    /// Team records copied from the source timeline.
    #[schemars(length(min = 1))]
    pub teams: Vec<TeamEntry>,
    /// Stream sections.
    pub streams: Streams,
}

/// One team record in the bootstrap: a slug plus the fields the
/// source timeline recorded for the team.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct TeamEntry {
    /// Team slug.
    pub slug: String,
    /// Other fields copied from the source timeline.
    #[serde(flatten)]
    #[schemars(schema_with = "extra_fields_schema")]
    pub extra: Map<String, Value>,
}

fn extra_fields_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "additionalProperties": true
    }))
    .expect("extra-fields schema is valid")
}

/// Remove the `null` alternative for a field accepted only when
/// absent-or-valued, never null.
fn denull_stream(schema: &mut Value, name: &str) {
    let Some(properties) = schema["$defs"]["Streams"]["properties"].as_object_mut() else {
        return;
    };
    let Some(field) = properties.get_mut(name) else {
        return;
    };
    if let Some(any_of) = field.get("anyOf").and_then(Value::as_array).cloned() {
        let non_null: Vec<Value> = any_of
            .into_iter()
            .filter(|branch| branch.get("type") != Some(&Value::String("null".to_owned())))
            .collect();
        if non_null.len() == 1 {
            let only = non_null.into_iter().next().expect("one branch");
            if let Some(obj) = field.as_object_mut() {
                obj.remove("anyOf");
                if let Value::Object(branch) = only {
                    for (key, value) in branch {
                        obj.insert(key, value);
                    }
                }
            }
        }
    }
}

/// The generated schema of each format, keyed by file stem.
#[must_use]
pub fn generated_schemas() -> Vec<(&'static str, Value)> {
    let mut bootstrap = serde_json::to_value(schemars::schema_for!(TimelineV3Bootstrap))
        .expect("timeline-v3 bootstrap schema serializes");
    for name in ["search", "search_bloom", "search_links"] {
        denull_stream(&mut bootstrap, name);
    }
    vec![("wrkviz-timeline-v3-bootstrap", bootstrap)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_refuses_unknown_fields_and_generates_consts() {
        let mut value = serde_json::json!({
            "schema_version": 3, "kind": "timeline-v3-bootstrap",
            "codec": {"container": "multi-member-gzip", "level": 6,
                      "target_chunk_bytes": 1024, "timestamp_key": "at_ms",
                      "record_kind_key": "record_kind", "index_suffix": ".index.jsonl"},
            "teams": [{"slug": "alpha"}],
            "streams": {
                "timeline": {"addressing": "per-team-utc-day", "sort": "s", "record_kinds": [], "shards": []},
                "spine": {"addressing": "per-team-line-range", "sort": "s", "record_kinds": [], "shards": []},
                "bins": {"addressing": "single-shard-time-range", "sort": "s", "record_kinds": [], "shards": []}
            }
        });
        assert!(serde_json::from_value::<TimelineV3Bootstrap>(value.clone()).is_ok());
        value["bogus"] = Value::Bool(true);
        assert!(serde_json::from_value::<TimelineV3Bootstrap>(value).is_err());
        let schema = generated_schemas()
            .into_iter()
            .next()
            .expect("one schema")
            .1;
        assert_eq!(schema["properties"]["schema_version"]["const"], 3);
        assert_eq!(
            schema["$defs"]["Codec"]["properties"]["container"]["const"],
            "multi-member-gzip"
        );
    }

    #[test]
    fn every_format_generates_a_schema() {
        assert_eq!(generated_schemas().len(), 1);
    }
}
