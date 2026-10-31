//! Logical-key index for model artifacts, at parity with
//! `py/wrkviz/summary_catalog.py`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::json::{as_array, as_int, as_object, as_string, read_json};
use crate::summary_artifacts::SummaryArtifactProvenance;
use crate::transcript_io::write_json_if_changed;

/// Catalog schema version.
pub const SUMMARY_CATALOG_SCHEMA_VERSION: i64 = 1;

fn validate_relative_path(value: &str) -> Result<String, String> {
    let invalid = value.is_empty()
        || value.starts_with('/')
        || value
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."));
    if invalid {
        return Err(format!(
            "unsafe summary artifact cache path {}",
            crate::json::py_repr_str(value)
        ));
    }
    Ok(value.to_string())
}

/// One immutable artifact and its cache location.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryArtifactReference {
    /// Provenance.
    pub provenance: SummaryArtifactProvenance,
    /// Cache path relative to the summary root.
    pub cache_path: String,
}

impl SummaryArtifactReference {
    /// Construct, validating the path/hash agreement.
    pub fn new(
        provenance: SummaryArtifactProvenance,
        cache_path: &str,
    ) -> Result<Self, String> {
        validate_relative_path(cache_path)?;
        let filename = cache_path.rsplit('/').next().unwrap_or_default();
        if filename != format!("{}.json", provenance.input_hash) {
            return Err(
                "summary artifact cache filename does not match input hash".to_string(),
            );
        }
        Ok(Self {
            provenance,
            cache_path: cache_path.to_string(),
        })
    }

    /// Persisted form.
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("artifact".into(), self.provenance.to_json_obj());
        out.insert(
            "cache_path".into(),
            Value::String(self.cache_path.clone()),
        );
        Value::Object(out)
    }

    /// Strictly parse one catalog record.
    pub fn from_json_obj(value: &Value, where_: &str) -> Result<Self, String> {
        let obj = as_object(value, where_)?;
        if obj.len() != 2 || !obj.contains_key("artifact") || !obj.contains_key("cache_path") {
            return Err(format!("{where_}: invalid summary artifact reference fields"));
        }
        Self::new(
            SummaryArtifactProvenance::from_json_obj(
                obj.get("artifact"),
                &format!("{where_}.artifact"),
            )?,
            validate_relative_path(as_string(
                obj.get("cache_path").unwrap(),
                &format!("{where_}.cache_path"),
            )?)?
            .as_str(),
        )
    }
}

fn record_order(
    record: &SummaryArtifactReference,
) -> (String, String, i64, i64, String, String, String) {
    let provenance = &record.provenance;
    (
        provenance.logical_key.clone(),
        provenance.summarizer_id.clone(),
        provenance.summarizer_version,
        provenance.output_schema_version,
        provenance.model.clone(),
        provenance.generated_at.clone(),
        provenance.artifact_id.clone(),
    )
}

fn ordered_records(
    mut records: Vec<SummaryArtifactReference>,
) -> Vec<SummaryArtifactReference> {
    records.sort_by_key(record_order);
    records
}

/// All indexed model artifacts for one team.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryArtifactCatalog {
    /// Team slug.
    pub team_slug: String,
    /// Canonical records.
    pub records: Vec<SummaryArtifactReference>,
}

impl SummaryArtifactCatalog {
    /// Construct, validating canonical order and uniqueness.
    pub fn new(
        team_slug: &str,
        records: Vec<SummaryArtifactReference>,
    ) -> Result<Self, String> {
        if team_slug.is_empty() {
            return Err("summary artifact catalog team slug must not be empty".to_string());
        }
        let mut ids: Vec<&str> = records
            .iter()
            .map(|item| item.provenance.artifact_id.as_str())
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != total {
            return Err("summary artifact catalog contains duplicate artifact IDs".to_string());
        }
        if records.iter().any(|item| item.provenance.team_slug != team_slug) {
            return Err("summary artifact catalog mixes team slugs".to_string());
        }
        if records != ordered_records(records.clone()) {
            return Err("summary artifact catalog records are not canonical".to_string());
        }
        Ok(Self {
            team_slug: team_slug.to_string(),
            records,
        })
    }

    /// Persisted form with derived aggregate counts.
    pub fn to_json_obj(&self) -> Value {
        let mut versions: BTreeMap<String, i64> = BTreeMap::new();
        let mut models: BTreeMap<String, i64> = BTreeMap::new();
        let mut frontiers: BTreeMap<String, i64> = BTreeMap::new();
        let mut logical: Vec<&str> = Vec::new();
        for record in &self.records {
            let provenance = &record.provenance;
            let version_key = format!(
                "{}@{}/schema-{}",
                provenance.summarizer_id,
                provenance.summarizer_version,
                provenance.output_schema_version
            );
            *versions.entry(version_key).or_default() += 1;
            *models.entry(provenance.model.clone()).or_default() += 1;
            *frontiers
                .entry(provenance.context_coverage.frontier_status.clone())
                .or_default() += 1;
            logical.push(provenance.logical_key.as_str());
        }
        logical.sort_unstable();
        logical.dedup();
        let counts = |map: &BTreeMap<String, i64>| -> Value {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(key.clone(), (*value).into());
            }
            Value::Object(out)
        };
        let mut out = serde_json::Map::new();
        out.insert("schema_version".into(), SUMMARY_CATALOG_SCHEMA_VERSION.into());
        out.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        out.insert("artifact_count".into(), (self.records.len() as i64).into());
        out.insert("logical_key_count".into(), (logical.len() as i64).into());
        out.insert("version_counts".into(), counts(&versions));
        out.insert("model_counts".into(), counts(&models));
        out.insert("frontier_status_counts".into(), counts(&frontiers));
        out.insert(
            "artifacts".into(),
            Value::Array(
                self.records
                    .iter()
                    .map(SummaryArtifactReference::to_json_obj)
                    .collect(),
            ),
        );
        Value::Object(out)
    }

    /// Parse and verify derived aggregate counts.
    pub fn from_json_obj(value: &Value, where_: &str) -> Result<Self, String> {
        let obj = as_object(value, where_)?;
        let expected = [
            "schema_version",
            "team_slug",
            "artifact_count",
            "logical_key_count",
            "version_counts",
            "model_counts",
            "frontier_status_counts",
            "artifacts",
        ];
        if obj.len() != expected.len() || !expected.iter().all(|key| obj.contains_key(*key)) {
            return Err(format!("{where_}: invalid summary artifact catalog fields"));
        }
        if as_int(obj.get("schema_version").unwrap(), &format!("{where_}.schema_version"))?
            != SUMMARY_CATALOG_SCHEMA_VERSION
        {
            return Err(format!("{where_}: unsupported summary artifact catalog schema"));
        }
        let raw = as_array(obj.get("artifacts").unwrap(), &format!("{where_}.artifacts"))?;
        let mut records = Vec::new();
        for (index, item) in raw.iter().enumerate() {
            records.push(SummaryArtifactReference::from_json_obj(
                item,
                &format!("{where_}.artifacts[{index}]"),
            )?);
        }
        let catalog = Self::new(
            as_string(obj.get("team_slug").unwrap(), &format!("{where_}.team_slug"))?,
            ordered_records(records),
        )?;
        let rendered = catalog.to_json_obj();
        let rendered_obj = as_object(&rendered, "catalog")?;
        for key in [
            "artifact_count",
            "logical_key_count",
            "version_counts",
            "model_counts",
            "frontier_status_counts",
        ] {
            if obj.get(key) != rendered_obj.get(key) {
                return Err(format!("{where_}.{key}: derived catalog value mismatch"));
            }
        }
        Ok(catalog)
    }
}

/// An empty canonical catalog.
pub fn empty_summary_catalog(team_slug: &str) -> SummaryArtifactCatalog {
    SummaryArtifactCatalog {
        team_slug: team_slug.to_string(),
        records: Vec::new(),
    }
}

/// Load a catalog, or return empty when absent.
pub fn load_summary_catalog(
    path: &Path,
    team_slug: &str,
) -> Result<SummaryArtifactCatalog, String> {
    if !path.is_file() {
        return Ok(empty_summary_catalog(team_slug));
    }
    let value = read_json(path)?;
    let catalog = SummaryArtifactCatalog::from_json_obj(&value, &path.display().to_string())?;
    if catalog.team_slug != team_slug {
        return Err(format!(
            "{}: summary artifact catalog team mismatch",
            path.display()
        ));
    }
    Ok(catalog)
}

/// Merge validated references by artifact ID and persist.
pub fn merge_summary_catalog(
    path: &Path,
    team_slug: &str,
    additions: &[SummaryArtifactReference],
) -> Result<(SummaryArtifactCatalog, bool), String> {
    let existing = load_summary_catalog(path, team_slug)?;
    let mut by_id: BTreeMap<String, SummaryArtifactReference> = BTreeMap::new();
    for item in existing.records {
        by_id.insert(item.provenance.artifact_id.clone(), item);
    }
    for addition in additions {
        let artifact_id = addition.provenance.artifact_id.clone();
        if let Some(current) = by_id.get(&artifact_id) {
            if current.cache_path != addition.cache_path
                || current.provenance.identity_json_obj() != addition.provenance.identity_json_obj()
            {
                return Err(format!(
                    "artifact {} has conflicting catalog identity",
                    crate::json::py_repr_str(&artifact_id)
                ));
            }
        }
        by_id.insert(artifact_id, addition.clone());
    }
    let catalog = SummaryArtifactCatalog::new(
        team_slug,
        ordered_records(by_id.into_values().collect()),
    )?;
    let changed = write_json_if_changed(path, &catalog.to_json_obj())?;
    Ok((catalog, changed))
}

/// Select the strongest compatible artifact for one logical key.
pub fn select_summary_artifact(
    catalog: &SummaryArtifactCatalog,
    logical_key: &str,
    summarizer_id: &str,
    minimum_output_schema: i64,
    preferred_model: Option<&str>,
) -> Result<Option<SummaryArtifactReference>, String> {
    if minimum_output_schema < 1 {
        return Err("minimum output schema must be positive".to_string());
    }
    let mut candidates: Vec<&SummaryArtifactReference> = catalog
        .records
        .iter()
        .filter(|record| {
            record.provenance.logical_key == logical_key
                && record.provenance.summarizer_id == summarizer_id
                && record.provenance.output_schema_version >= minimum_output_schema
        })
        .collect();
    if let Some(model) = preferred_model {
        let preferred: Vec<&SummaryArtifactReference> = candidates
            .iter()
            .copied()
            .filter(|record| record.provenance.model == model)
            .collect();
        if !preferred.is_empty() {
            candidates = preferred;
        }
    }
    Ok(candidates
        .into_iter()
        .max_by_key(|record| {
            let provenance = &record.provenance;
            (
                provenance.summarizer_version,
                provenance.output_schema_version,
                provenance
                    .context_coverage
                    .coverage_basis_points()
                    .unwrap_or(-1),
                provenance.generated_at.clone(),
                provenance.artifact_id.clone(),
            )
        })
        .cloned())
}

/// Catalog path inside a summary root (helper for callers).
pub fn catalog_path(summary_root: &Path) -> PathBuf {
    summary_root.join("artifacts.json")
}
