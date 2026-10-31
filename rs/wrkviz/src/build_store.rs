//! Where a team's build state lives: `py/wrkviz/build_store.py`
//! at read parity (resolution and discovery; migration is a
//! later command).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::json::{as_object, as_string, read_json};

/// Validate a team slug's grammar.
pub fn validate_team_slug(team_slug: &str) -> Result<(), String> {
    let valid = team_slug.len() <= 64
        && !team_slug.is_empty()
        && team_slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(
            "team slug must be 1-64 lowercase letters/digits separated by single hyphens"
                .to_string(),
        )
    }
}

/// `<archive>.build`, resolved from the archive's real path.
pub fn default_store_root(archive: &Path) -> PathBuf {
    let resolved = crate::json::resolve_path(archive);
    let name = resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    resolved
        .parent()
        .map(|parent| parent.join(format!("{name}.build")))
        .unwrap_or_default()
}

struct Pointer {
    store_root: PathBuf,
    external: bool,
}

fn read_pointer(archive: &Path) -> Result<Option<Pointer>, String> {
    let path = archive.join("build-root.json");
    if !path.is_file() {
        return Ok(None);
    }
    let record = read_json(&path)?;
    let record = as_object(&record, &path.display().to_string())?;
    let layout = as_string(
        record.get("layout").unwrap_or(&Value::Null),
        &format!("{}.layout", path.display()),
    )?;
    if layout != "in-archive-legacy" && layout != "external" {
        return Err(format!(
            "{}: unknown build layout {}",
            path.display(),
            crate::json::py_repr_str(layout)
        ));
    }
    let raw_root = as_string(
        record.get("store_root").unwrap_or(&Value::Null),
        &format!("{}.store_root", path.display()),
    )?;
    let store_root = if raw_root.is_empty() {
        crate::json::resolve_path(archive)
    } else {
        crate::json::resolve_path(&archive.join(raw_root))
    };
    Ok(Some(Pointer {
        store_root,
        external: layout == "external",
    }))
}

fn populated(root: &Path) -> bool {
    ["raw", "payloads"].iter().any(|tenant| {
        let dir = root.join(tenant);
        dir.is_dir()
            && std::fs::read_dir(&dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
    })
}

/// The directory holding one team's build state.
pub fn team_build_root(archive: &Path, team_slug: &str) -> Result<PathBuf, String> {
    validate_team_slug(team_slug)?;
    let legacy = archive.join("teams").join(team_slug);
    if let Some(pointer) = read_pointer(archive)? {
        if pointer.external {
            if populated(&legacy) {
                return Err(format!(
                    "{} says the build store is external, but {} still holds this team's state. That is an interrupted migration: run `timeline migrate-build-state --output {}` to finish it.",
                    archive.join("build-root.json").display(),
                    legacy.display(),
                    archive.display()
                ));
            }
            return Ok(pointer.store_root.join(team_slug));
        }
    }
    if populated(&legacy) {
        return Ok(legacy);
    }
    Ok(default_store_root(archive).join(team_slug))
}

fn candidate_store_roots(archive: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    match read_pointer(archive)? {
        Some(pointer) if pointer.external => roots.push(pointer.store_root),
        _ => {
            let default = default_store_root(archive);
            if default.is_dir() {
                roots.push(default);
            }
        }
    }
    roots.push(archive.join("teams"));
    Ok(roots)
}

/// Every team slug in *archive* that has been ingested.
pub fn ingested_team_slugs(archive: &Path) -> Result<Vec<String>, String> {
    let mut found: Vec<String> = Vec::new();
    for root in candidate_store_roots(archive)? {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir_no_link = path.is_dir()
                && !std::fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
            if is_dir_no_link && path.join("raw").join("team.json").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !found.contains(&name) {
                    found.push(name);
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

fn legacy_team_roots(archive: &Path) -> Vec<PathBuf> {
    let teams = archive.join("teams");
    let mut roots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&teams) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && populated(&path) {
                roots.push(path);
            }
        }
    }
    roots.sort();
    roots
}

/// The store's directory for build state that belongs to no single team.
pub fn shared_build_root(archive: &Path) -> Result<PathBuf, String> {
    if let Some(pointer) = read_pointer(archive)? {
        if pointer.external {
            return Ok(pointer.store_root.join("_shared"));
        }
    }
    if !legacy_team_roots(archive).is_empty() {
        return Ok(archive.join("extracted").join("transcripts"));
    }
    Ok(default_store_root(archive).join("_shared"))
}

/// Resolve a shared-tenant file for reading, accepting either location.
pub fn shared_build_file(archive: &Path, name: &str) -> Result<PathBuf, String> {
    let preferred = shared_build_root(archive)?.join(name);
    if preferred.is_file() {
        return Ok(preferred);
    }
    let legacy = archive.join("extracted").join("transcripts").join(name);
    Ok(if legacy.is_file() { legacy } else { preferred })
}
