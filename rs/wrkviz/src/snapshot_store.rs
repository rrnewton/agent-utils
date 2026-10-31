//! The snapshot pointer (`snapshot-root.json`) and the legacy
//! in-archive snapshot layout: the Rust mirror of
//! `wrkviz.snapshot_store`'s `pointer_summary`, `read_pointer`,
//! `legacy_team_roots`, and `default_store_root`.

use crate::json::{as_object, as_string, read_json, resolve_path};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const POINTER_FILE: &str = "snapshot-root.json";
const LEGACY_DIRECTORY: &str = "source_snapshots";
const DEFAULT_STORE_SUFFIX: &str = ".sources";
const POINTER_KIND: &str = "source-snapshot-location";

struct Pointer {
    layout: String,
    root: Option<String>,
}

fn read_pointer(archive: &Path) -> Result<Option<Pointer>, String> {
    let path = archive.join(POINTER_FILE);
    if path.is_symlink() {
        return Err(format!(
            "{}: snapshot location pointer is a symlink",
            path.display()
        ));
    }
    if !path.is_file() {
        return Ok(None);
    }
    let value = read_json(&path)?;
    let obj = as_object(&value, &path.display().to_string())?.clone();
    let kind = as_string(
        obj.get("kind").unwrap_or(&Value::Null),
        &format!("{}: kind", path.display()),
    )?;
    let version = obj.get("schema_version").unwrap_or(&Value::Null);
    let version_ok = version.as_i64() == Some(1)
        || version.as_f64().is_some_and(|number| number == 1.0)
        || version.as_bool() == Some(true);
    if kind != POINTER_KIND || !version_ok {
        return Err(format!(
            "{}: not a schema-1 {POINTER_KIND} document",
            path.display()
        ));
    }
    let layout = as_string(
        obj.get("layout").unwrap_or(&Value::Null),
        &format!("{}: layout", path.display()),
    )?
    .to_string();
    let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    if layout == "in-archive-legacy" {
        if keys != std::collections::BTreeSet::from(["schema_version", "kind", "layout"]) {
            return Err(format!(
                "{}: unexpected fields for an in-archive layout",
                path.display()
            ));
        }
        return Ok(Some(Pointer { layout, root: None }));
    }
    if layout != "external" {
        return Err(format!(
            "{}: unknown layout {}",
            path.display(),
            crate::json::py_repr_str(&layout)
        ));
    }
    if keys != std::collections::BTreeSet::from(["schema_version", "kind", "layout", "root"]) {
        return Err(format!(
            "{}: unexpected fields for an external layout",
            path.display()
        ));
    }
    let root = as_string(
        obj.get("root").unwrap_or(&Value::Null),
        &format!("{}: root", path.display()),
    )?
    .to_string();
    if root.is_empty() || Path::new(&root).is_absolute() {
        return Err(format!(
            "{}: root must be a non-empty path relative to the archive, so that an archive \
             moved together with its store keeps working",
            path.display()
        ));
    }
    Ok(Some(Pointer {
        layout,
        root: Some(root),
    }))
}

fn populated(path: &Path) -> bool {
    if path.is_symlink() || !path.is_dir() {
        return false;
    }
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

fn legacy_team_roots(archive: &Path) -> Result<Vec<PathBuf>, String> {
    let teams = archive.join("teams");
    if !teams.is_dir() {
        return Ok(Vec::new());
    }
    let read = std::fs::read_dir(&teams).map_err(|error| crate::json::os_error(&error, &teams))?;
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|error| crate::json::os_error(&error, &teams))?;
        entries.push(entry.path());
    }
    entries.sort();
    let mut found = Vec::new();
    for team in entries {
        let meta = std::fs::symlink_metadata(&team);
        let is_real_dir = meta.is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink());
        if !is_real_dir {
            continue;
        }
        let candidate = team.join(LEGACY_DIRECTORY);
        if populated(&candidate) {
            found.push(candidate);
        }
    }
    Ok(found)
}

fn default_store_root(archive: &Path) -> PathBuf {
    let resolved = resolve_path(archive);
    let name = resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    resolved
        .parent()
        .map(|parent| parent.join(format!("{name}{DEFAULT_STORE_SUFFIX}")))
        .unwrap_or_else(|| PathBuf::from(format!("{name}{DEFAULT_STORE_SUFFIX}")))
}

fn tree_size(root: &Path) -> (i64, i64) {
    let mut files = 0i64;
    let mut total = 0i64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                // os.walk lists a symlink-to-dir among dirnames:
                // neither counted nor descended. A symlink to a
                // file (or a broken link) is a filename, sized by
                // lstat.
                match std::fs::metadata(&path) {
                    Ok(target) if target.is_dir() => {}
                    _ => {
                        files += 1;
                        total += meta.len() as i64;
                    }
                }
            } else if meta.is_dir() {
                pending.push(path);
            } else {
                files += 1;
                total += meta.len() as i64;
            }
        }
    }
    (files, total)
}

const STORE_MARKER_FILE: &str = ".snapshot-store.json";
const STORE_GITIGNORE_FILE: &str = ".gitignore";
const MARKER_KIND: &str = "source-snapshot-store";
const STORE_GITIGNORE_TEXT: &str =
    "# Written by wrkviz. This directory is the archive's source-snapshot store:\n\
# vendor logs and SQLite snapshots that ingestion reads and that no published artifact\n\
# contains. `/*` ignores every entry including this file, so a store that lands inside a\n\
# repository is invisible to `git status` rather than being offered as a multi-gigabyte\n\
# addition. Delete this file if you genuinely want the bulk under version control.\n\
/*\n";

/// Where one team's snapshots are, and how that was decided.
#[derive(Clone, Debug)]
pub struct SnapshotLocation {
    /// The archive.
    pub archive: PathBuf,
    /// Team slug.
    pub team_slug: String,
    /// The team's snapshot directory.
    pub root: PathBuf,
    /// The store holding every team, when external.
    pub store_root: Option<PathBuf>,
    /// `external` or `in-archive-legacy`.
    pub layout: String,
    /// How resolution arrived here.
    pub origin: &'static str,
}

impl SnapshotLocation {
    /// The root as a POSIX path relative to the archive.
    pub fn archive_relative(&self) -> String {
        relpath(&self.root, &self.archive)
    }
}

/// Lexical relative path from `base` to `target` (both made
/// absolute first), as a POSIX string.
pub fn relpath(target: &Path, base: &Path) -> String {
    let target = resolve_path(target);
    let base = resolve_path(base);
    let target_parts: Vec<String> = target
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let base_parts: Vec<String> = base
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let mut common = 0;
    while common < target_parts.len()
        && common < base_parts.len()
        && target_parts[common] == base_parts[common]
    {
        common += 1;
    }
    let mut out: Vec<String> = Vec::new();
    for _ in common..base_parts.len() {
        out.push("..".to_string());
    }
    out.extend(target_parts[common..].iter().cloned());
    if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    }
}

fn check_store_root(archive: &Path, store_root: &Path) -> Result<PathBuf, String> {
    let resolved_store = resolve_path(store_root);
    let resolved_archive = resolve_path(archive);
    if resolved_store == resolved_archive {
        return Err(format!(
            "snapshot store {} is the archive itself; the store exists to be outside \
             the published tree",
            store_root.display()
        ));
    }
    if resolved_store.starts_with(&resolved_archive) {
        return Err(format!(
            "snapshot store {} is inside the archive {}; the store exists to be \
             outside the published tree, which is served over HTTP and cloned",
            store_root.display(),
            archive.display()
        ));
    }
    if resolved_archive.starts_with(&resolved_store) {
        return Err(format!(
            "snapshot store {} contains the archive {}; that would make the \
             published tree part of the store",
            store_root.display(),
            archive.display()
        ));
    }
    Ok(resolved_store)
}

fn legacy_team_root(archive: &Path, team_slug: &str) -> PathBuf {
    archive.join("teams").join(team_slug).join(LEGACY_DIRECTORY)
}

fn copied_snapshots_before(archive: &Path) -> bool {
    for root in crate::build_store::candidate_store_roots(archive).unwrap_or_default() {
        if root.is_symlink() || !root.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = path.is_dir()
                && !std::fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
            if is_dir && path.join("raw").join("source-manifest.json").is_file() {
                return true;
            }
        }
    }
    false
}

fn recorded_store(archive: &Path, store_root: &Path) -> Result<PathBuf, String> {
    if store_root.is_dir() {
        return Ok(store_root.to_path_buf());
    }
    let sibling = default_store_root(archive);
    if sibling != store_root && sibling.join(STORE_MARKER_FILE).is_file() {
        return check_store_root(archive, &sibling);
    }
    if copied_snapshots_before(archive) {
        return Err(format!(
            "{} says this archive's snapshots live in {}, and there is no such directory. \
             Snapshots have been copied for this archive before, so they are somewhere; \
             creating an empty store here would lose sight of them. Point at where they \
             are with --snapshot-root <path> (or `snapshot_root` in the project config), \
             or -- if they really are gone -- delete {} to start a fresh store, accepting \
             that the append-only baseline for every team goes with it.",
            archive.join(POINTER_FILE).display(),
            store_root.display(),
            archive.join(POINTER_FILE).display()
        ));
    }
    Ok(store_root.to_path_buf())
}

fn marker_archive(store_root: &Path) -> Result<Option<String>, String> {
    let path = store_root.join(STORE_MARKER_FILE);
    if path.is_symlink() {
        return Err(format!("{}: store marker is a symlink", path.display()));
    }
    if !path.is_file() {
        return Ok(None);
    }
    let value = read_json(&path)?;
    let obj = as_object(&value, &path.display().to_string())?;
    let kind_ok = obj.get("kind").and_then(Value::as_str) == Some(MARKER_KIND);
    if !kind_ok || !crate::json::json_eq_one(obj.get("schema_version")) {
        return Err(format!(
            "{}: not a schema-1 {MARKER_KIND} document",
            path.display()
        ));
    }
    Ok(Some(
        as_string(
            obj.get("archive").unwrap_or(&Value::Null),
            &format!("{}: archive", path.display()),
        )?
        .to_string(),
    ))
}

fn foreign_marker(archive: &Path, store_root: &Path) -> Result<Option<String>, String> {
    let Some(existing) = marker_archive(store_root)? else {
        return Ok(None);
    };
    let expected = relpath(&resolve_path(archive), store_root);
    if existing == expected {
        return Ok(None);
    }
    let rival = resolve_path(&store_root.join(&existing));
    let marker = rival.join(".wrkviz.json");
    let legacy = rival.join(".agent-team-timeline.json");
    if !marker.is_file() && !legacy.is_file() {
        return Ok(None);
    }
    Ok(Some(existing))
}

/// Create the store's own metadata; return files changed.
pub fn ensure_store_root(archive: &Path, store_root: &Path) -> Result<i64, String> {
    let relative_archive = relpath(&resolve_path(archive), store_root);
    if let Some(foreign) = foreign_marker(archive, store_root)? {
        return Err(format!(
            "{} says this store belongs to {foreign}, not to {relative_archive}, and \
             that archive is still there. Two archives sharing one store would collide \
             the moment they share a team slug; give this archive its own \
             --snapshot-root.",
            store_root.join(STORE_MARKER_FILE).display()
        ));
    }
    std::fs::create_dir_all(store_root)
        .map_err(|error| crate::json::os_error(&error, store_root))?;
    let mut marker = Map::new();
    marker.insert("schema_version".into(), 1.into());
    marker.insert("kind".into(), Value::String(MARKER_KIND.to_string()));
    marker.insert("archive".into(), Value::String(relative_archive));
    let mut changed = 0i64;
    if crate::transcript_io::write_json_if_changed(
        &store_root.join(STORE_MARKER_FILE),
        &Value::Object(marker),
    )? {
        changed += 1;
    }
    if crate::transcript_io::write_text_if_changed(
        &store_root.join(STORE_GITIGNORE_FILE),
        STORE_GITIGNORE_TEXT,
    )? {
        changed += 1;
    }
    Ok(changed)
}

/// Create the store and the team directory; return how many
/// files this changed.
pub fn ensure_snapshot_store(location: &SnapshotLocation) -> Result<i64, String> {
    if let Some(store_root) = &location.store_root {
        let changed = ensure_store_root(&location.archive, store_root)?;
        std::fs::create_dir_all(&location.root)
            .map_err(|error| crate::json::os_error(&error, &location.root))?;
        return Ok(changed);
    }
    if let Some(parent) = location.root.parent() {
        std::fs::create_dir_all(parent).map_err(|error| crate::json::os_error(&error, parent))?;
    }
    std::fs::create_dir_all(&location.root)
        .map_err(|error| crate::json::os_error(&error, &location.root))?;
    Ok(0)
}

/// Record the layout durably; return whether anything changed.
pub fn write_pointer(archive: &Path, location: &SnapshotLocation) -> Result<bool, String> {
    let mut value = Map::new();
    value.insert("schema_version".into(), 1.into());
    value.insert("kind".into(), Value::String(POINTER_KIND.to_string()));
    value.insert("layout".into(), Value::String(location.layout.clone()));
    if let Some(store_root) = &location.store_root {
        value.insert(
            "root".into(),
            Value::String(relpath(store_root, &resolve_path(archive))),
        );
    }
    crate::transcript_io::write_json_if_changed(&archive.join(POINTER_FILE), &Value::Object(value))
}

/// Decide where one team's snapshots are. Reads the
/// filesystem; writes nothing.
pub fn resolve_snapshot_root(
    archive: &Path,
    team_slug: &str,
    requested: Option<&Path>,
) -> Result<SnapshotLocation, String> {
    crate::build_store::validate_team_slug(team_slug)?;
    let legacy = legacy_team_root(archive, team_slug);
    let legacy_populated = populated(&legacy);
    let recorded = read_pointer(archive)?;
    if let Some(requested) = requested {
        let store_root = check_store_root(archive, requested)?;
        let occupied = legacy_team_roots(archive)?;
        if legacy_populated || !occupied.is_empty() {
            let where_ = if legacy_populated {
                legacy.display().to_string()
            } else {
                occupied
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let whose = if legacy_populated {
                "this team's"
            } else {
                "this archive's"
            };
            return Err(format!(
                "{where_} still holds {whose} snapshots, so --snapshot-root {} would \
                 split this archive across two layouts. Move them first, all of them \
                 together: wrkviz migrate-snapshots --output {} --snapshot-root {} --move",
                requested.display(),
                archive.display(),
                requested.display()
            ));
        }
        if let Some(pointer) = &recorded {
            if pointer.layout == "external" {
                if let Some(root) = &pointer.root {
                    let existing = resolve_path(&resolve_path(archive).join(root));
                    if existing != store_root && populated(&existing) {
                        return Err(format!(
                            "{} records the store at {}, which is populated, but \
                             --snapshot-root says {}. Move it deliberately: \
                             wrkviz migrate-snapshots --output {} --snapshot-root {} --move",
                            archive.join(POINTER_FILE).display(),
                            existing.display(),
                            store_root.display(),
                            archive.display(),
                            store_root.display()
                        ));
                    }
                }
            }
        }
        return Ok(SnapshotLocation {
            archive: archive.to_path_buf(),
            team_slug: team_slug.to_string(),
            root: store_root.join(team_slug),
            store_root: Some(store_root),
            layout: "external".to_string(),
            origin: "requested",
        });
    }
    if let Some(pointer) = &recorded {
        if pointer.layout == "external" {
            let root = pointer.root.clone().unwrap_or_default();
            let store_root = check_store_root(archive, &resolve_path(archive).join(&root))?;
            if legacy_populated {
                return Err(format!(
                    "{} says this archive's snapshots live in {}, but {} is still \
                     populated. That is an interrupted migration, not a choice: finish \
                     it with wrkviz migrate-snapshots --output {} --move (add --copy if \
                     the store is on another filesystem)",
                    archive.join(POINTER_FILE).display(),
                    store_root.display(),
                    legacy.display(),
                    archive.display()
                ));
            }
            let store_root = recorded_store(archive, &store_root)?;
            return Ok(SnapshotLocation {
                archive: archive.to_path_buf(),
                team_slug: team_slug.to_string(),
                root: store_root.join(team_slug),
                store_root: Some(store_root),
                layout: "external".to_string(),
                origin: "recorded",
            });
        }
        return Ok(SnapshotLocation {
            archive: archive.to_path_buf(),
            team_slug: team_slug.to_string(),
            root: legacy,
            store_root: None,
            layout: "in-archive-legacy".to_string(),
            origin: "recorded",
        });
    }
    if legacy_populated || !legacy_team_roots(archive)?.is_empty() {
        return Ok(SnapshotLocation {
            archive: archive.to_path_buf(),
            team_slug: team_slug.to_string(),
            root: legacy,
            store_root: None,
            layout: "in-archive-legacy".to_string(),
            origin: "in-archive-legacy",
        });
    }
    let store_root = check_store_root(archive, &default_store_root(archive))?;
    Ok(SnapshotLocation {
        archive: archive.to_path_buf(),
        team_slug: team_slug.to_string(),
        root: store_root.join(team_slug),
        store_root: Some(store_root),
        layout: "external".to_string(),
        origin: "default",
    })
}

/// Whether a snapshot layout pointer is recorded.
pub fn pointer_is_recorded(archive: &Path) -> Result<bool, String> {
    Ok(read_pointer(archive)?.is_some())
}

/// Describe an archive's snapshot layout for `inspect`, without
/// resolving a team.
pub fn pointer_summary(archive: &Path) -> Result<Value, String> {
    let recorded = read_pointer(archive)?;
    let legacy = legacy_team_roots(archive)?;
    let layout: String = match &recorded {
        Some(pointer) => pointer.layout.clone(),
        None if !legacy.is_empty() => "in-archive-legacy".to_string(),
        None => "external".to_string(),
    };
    let store_root: Value = if layout == "external" {
        let root = match &recorded {
            Some(Pointer {
                root: Some(root), ..
            }) => resolve_path(&archive.join(root)),
            _ => default_store_root(archive),
        };
        Value::String(root.display().to_string())
    } else {
        Value::Null
    };
    let mut files = 0i64;
    let mut total = 0i64;
    for path in &legacy {
        let (count, size) = tree_size(path);
        files += count;
        total += size;
    }
    let teams: Vec<Value> = legacy
        .iter()
        .filter_map(|path| {
            path.parent()
                .and_then(Path::file_name)
                .map(|name| Value::String(name.to_string_lossy().into_owned()))
        })
        .collect();
    let mut out = Map::new();
    out.insert("layout".to_string(), Value::String(layout));
    out.insert("store_root".to_string(), store_root);
    out.insert("recorded".to_string(), Value::Bool(recorded.is_some()));
    out.insert("in_archive_teams".to_string(), Value::Array(teams));
    out.insert("in_archive_files".to_string(), json!(files));
    out.insert("in_archive_bytes".to_string(), json!(total));
    let _ = out;
    Ok(Value::Object(out))
}
