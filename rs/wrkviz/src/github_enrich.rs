//! Discover evidenced pull references and cache bounded metadata.
//! The Rust mirror of `wrkviz.github_enrich`.

use crate::github_metadata::{
    fetch_pull_request, load_pull_request_metadata_cache, save_pull_request_metadata_cache,
    GhApiTransport, HttpTransport, PullRequestKey, DEFAULT_TIMEOUT_SECONDS,
};
use crate::json::{as_array, as_object, read_json};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn valid_team_slug(slug: &str) -> bool {
    // [a-z0-9]+(?:-[a-z0-9]+)*
    !slug.is_empty()
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Observable result of one conditional metadata refresh.
pub struct PullMetadataReport {
    /// Evidenced references.
    pub references: i64,
    /// Distinct pulls.
    pub distinct_pulls: i64,
    /// Fetched count.
    pub fetched: i64,
    /// Unchanged count.
    pub not_modified: i64,
    /// Failures.
    pub failures: Vec<String>,
    /// Cache path.
    pub cache_path: String,
}

/// The durable per-team pull metadata cache path.
pub fn pull_metadata_path(archive: &Path, team_slug: &str) -> Result<PathBuf, String> {
    if team_slug.chars().count() > 64 || !valid_team_slug(team_slug) {
        return Err(
            "team slug must be 1-64 lowercase letters/digits separated by single hyphens".into(),
        );
    }
    Ok(archive
        .join("teams")
        .join(team_slug)
        .join("summary_data")
        .join("github")
        .join("pulls.json"))
}

fn entry_keys(value: &Value, where_: &str) -> Result<BTreeSet<PullRequestKey>, String> {
    let item = as_object(value, where_)?;
    let text = item
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let mut keys = BTreeSet::new();
    for reference in crate::build_assets::find_pull_request_references(text) {
        if let Ok(key) = PullRequestKey::new(&reference.repository, reference.number) {
            keys.insert(key);
        }
    }
    if let Some(raw) = item.get("pull_requests") {
        if !raw.is_null() {
            for (index, reference) in as_array(raw, &format!("{where_}.pull_requests"))?
                .iter()
                .enumerate()
            {
                let obj = as_object(reference, &format!("{where_}.pull_requests[{index}]"))?;
                let repository = crate::json::as_string(
                    obj.get("repository").unwrap_or(&Value::Null),
                    &format!("{where_}.pull_requests[{index}].repository"),
                )?;
                let number = obj
                    .get("number")
                    .and_then(|value| value.as_i64())
                    .ok_or_else(|| {
                        format!("{where_}.pull_requests[{index}].number: expected an integer")
                    })?;
                keys.insert(PullRequestKey::new(repository, number)?);
            }
        }
    }
    Ok(keys)
}

/// Read generated phase details; return occurrence and unique keys.
pub fn discover_pull_request_keys(archive: &Path) -> Result<(i64, Vec<PullRequestKey>), String> {
    let details_root = archive.join("data").join("details");
    if !details_root.is_dir() {
        return Err(format!(
            "no generated phase details at {}; run build before GitHub enrichment",
            details_root.display()
        ));
    }
    let mut occurrences: i64 = 0;
    let mut keys = BTreeSet::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&details_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    for path in paths {
        let value = read_json(&path)?;
        let detail = as_object(&value, &path.display().to_string())?;
        for field_name in ["work_summary", "transcript"] {
            let entries = as_array(
                detail.get(field_name).unwrap_or(&Value::Null),
                &format!("{}.{field_name}", path.display()),
            )?;
            for (index, raw_entry) in entries.iter().enumerate() {
                let found = entry_keys(
                    raw_entry,
                    &format!("{}.{field_name}[{index}]", path.display()),
                )?;
                occurrences += found.len() as i64;
                keys.extend(found);
            }
        }
    }
    Ok((occurrences, keys.into_iter().collect()))
}

/// Conditionally refresh every evidenced pull.
pub fn enrich_pull_request_metadata(
    archive: &Path,
    team_slug: &str,
    timeout_seconds: f64,
    fetched_at: &str,
) -> Result<PullMetadataReport, String> {
    enrich_with_transport(
        archive,
        team_slug,
        timeout_seconds,
        fetched_at,
        &GhApiTransport,
    )
}

/// Enrich with an explicit transport (the fake-provider seam).
pub fn enrich_with_transport(
    archive: &Path,
    team_slug: &str,
    timeout_seconds: f64,
    fetched_at: &str,
    transport: &dyn HttpTransport,
) -> Result<PullMetadataReport, String> {
    let (references, keys) = discover_pull_request_keys(archive)?;
    let cache_path = pull_metadata_path(archive, team_slug)?;
    let mut cache = load_pull_request_metadata_cache(&cache_path)?;
    let mut fetched = 0i64;
    let mut not_modified = 0i64;
    let mut failures = Vec::new();
    for key in &keys {
        let cached = cache.get(key);
        match fetch_pull_request(key, cached, fetched_at, timeout_seconds, transport) {
            Ok(result) => {
                if result.not_modified {
                    not_modified += 1;
                    continue;
                }
                cache.put(result.metadata);
                if let Err(error) = save_pull_request_metadata_cache(&cache_path, &cache) {
                    failures.push(format!("{}: {error}", key.cache_key()));
                    continue;
                }
                fetched += 1;
            }
            Err(error) => {
                failures.push(format!("{}: {error}", key.cache_key()));
            }
        }
    }
    Ok(PullMetadataReport {
        references,
        distinct_pulls: keys.len() as i64,
        fetched,
        not_modified,
        failures,
        cache_path: cache_path.display().to_string(),
    })
}

/// Default-timeout marker for CLI wiring.
pub const TIMEOUT: f64 = DEFAULT_TIMEOUT_SECONDS;
