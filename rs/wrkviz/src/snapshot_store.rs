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

const COPY_STAGING_DIRECTORY: &str = ".migrating";

fn is_link(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn device_of(path: &Path) -> Result<u64, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let mut probe = path.to_path_buf();
        loop {
            if probe.exists() {
                return std::fs::metadata(&probe)
                    .map(|meta| meta.dev())
                    .map_err(|error| crate::json::os_error(&error, &probe));
            }
            match probe.parent() {
                Some(parent) if parent != probe => {
                    probe = parent.to_path_buf();
                }
                _ => {
                    return Err(format!("no existing ancestor for {}", path.display()));
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0)
    }
}

fn fsync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let file =
            std::fs::File::open(path).map_err(|error| crate::json::os_error(&error, path))?;
        file.sync_all()
            .map_err(|error| crate::json::os_error(&error, path))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|error| crate::json::os_error(&error, target))?;
    let entries =
        std::fs::read_dir(source).map_err(|error| crate::json::os_error(&error, source))?;
    let mut items: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    items.sort();
    for item in items {
        let dest = target.join(item.file_name().unwrap_or_default());
        if is_link(&item) {
            #[cfg(unix)]
            {
                let link_target = std::fs::read_link(&item)
                    .map_err(|error| crate::json::os_error(&error, &item))?;
                std::os::unix::fs::symlink(&link_target, &dest)
                    .map_err(|error| crate::json::os_error(&error, &dest))?;
            }
        } else if item.is_dir() {
            copy_tree(&item, &dest)?;
        } else if item.is_file() {
            std::fs::copy(&item, &dest).map_err(|error| crate::json::os_error(&error, &dest))?;
        }
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<(), String> {
    if is_link(path) || path.is_file() {
        return std::fs::remove_file(path).map_err(|error| crate::json::os_error(&error, path));
    }
    if !path.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(path).map_err(|error| crate::json::os_error(&error, path))?;
    for entry in entries.flatten() {
        remove_tree(&entry.path())?;
    }
    std::fs::remove_dir(path).map_err(|error| crate::json::os_error(&error, path))
}

/// One team's snapshot relocation, described before anything moves.
#[derive(Clone, Debug)]
pub struct SnapshotTeamMigration {
    /// Team slug.
    pub team_slug: String,
    /// Source directory.
    pub source: PathBuf,
    /// Target directory.
    pub target: PathBuf,
    /// File count.
    pub files: i64,
    /// Byte count.
    pub bytes: i64,
    /// Same device.
    pub same_device: bool,
    /// Refusal, if any.
    pub refusal: Option<String>,
}

/// What migrate-snapshots would do, and what it refuses to do.
#[derive(Clone, Debug)]
pub struct SnapshotMigrationPlan {
    /// Archive.
    pub archive: PathBuf,
    /// Store root.
    pub store_root: PathBuf,
    /// Teams.
    pub teams: Vec<SnapshotTeamMigration>,
    /// Already external.
    pub already_external: bool,
}

impl SnapshotMigrationPlan {
    fn movable(&self) -> Vec<&SnapshotTeamMigration> {
        self.teams
            .iter()
            .filter(|team| team.refusal.is_none())
            .collect()
    }

    fn total_files(&self) -> i64 {
        self.movable().iter().map(|team| team.files).sum()
    }

    fn total_bytes(&self) -> i64 {
        self.movable().iter().map(|team| team.bytes).sum()
    }

    /// JSON form of the plan.
    pub fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "archive".into(),
            Value::String(self.archive.display().to_string()),
        );
        out.insert(
            "store_root".into(),
            Value::String(self.store_root.display().to_string()),
        );
        out.insert(
            "already_external".into(),
            Value::Bool(self.already_external),
        );
        out.insert(
            "teams".into(),
            Value::Array(
                self.teams
                    .iter()
                    .map(|team| {
                        let mut item = Map::new();
                        item.insert("team".into(), Value::String(team.team_slug.clone()));
                        item.insert(
                            "source".into(),
                            Value::String(team.source.display().to_string()),
                        );
                        item.insert(
                            "target".into(),
                            Value::String(team.target.display().to_string()),
                        );
                        item.insert("files".into(), team.files.into());
                        item.insert("bytes".into(), team.bytes.into());
                        item.insert("same_device".into(), Value::Bool(team.same_device));
                        item.insert(
                            "refusal".into(),
                            team.refusal.clone().map_or(Value::Null, Value::String),
                        );
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

/// What migrate-snapshots --move actually did.
#[derive(Clone, Debug)]
pub struct SnapshotMigrationResult {
    /// The plan.
    pub plan: SnapshotMigrationPlan,
    /// Teams relocated by rename.
    pub moved: Vec<String>,
    /// Teams relocated by copy-verify-delete.
    pub copied: Vec<String>,
    /// Pointer changed.
    pub pointer_changed: bool,
}

impl SnapshotMigrationResult {
    /// JSON form of the result.
    pub fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert("plan".into(), self.plan.to_json_obj());
        out.insert(
            "moved".into(),
            Value::Array(
                self.moved
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "copied".into(),
            Value::Array(
                self.copied
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert("pointer_changed".into(), Value::Bool(self.pointer_changed));
        Value::Object(out)
    }
}

/// Describe the snapshot move without performing any part of it.
pub fn plan_snapshot_migration(
    archive: &Path,
    requested: Option<&Path>,
) -> Result<SnapshotMigrationPlan, String> {
    let recorded = read_pointer(archive)?;
    let store_root = if let Some(requested) = requested {
        check_store_root(archive, requested)?
    } else if let Some(pointer) = &recorded {
        if pointer.layout == "external" {
            let root = pointer.root.clone().unwrap_or_default();
            check_store_root(archive, &resolve_path(archive).join(root))?
        } else {
            check_store_root(archive, &default_store_root(archive))?
        }
    } else {
        check_store_root(archive, &default_store_root(archive))?
    };
    let foreign = if store_root.is_dir() {
        foreign_marker(archive, &store_root)?
    } else {
        None
    };
    let expected_archive = relpath(&resolve_path(archive), &store_root);
    let store_refusal = foreign.as_ref().map(|other| {
        format!(
            "{} says this store belongs to {other}, not to {expected_archive}, and that archive is still there",
            store_root.join(STORE_MARKER_FILE).display()
        )
    });
    let store_device = device_of(&store_root)?;
    let mut teams = Vec::new();
    for source in legacy_team_roots(archive)? {
        let team_slug = source
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        let target = store_root.join(&team_slug);
        let (files, total) = tree_size(&source);
        let mut refusal = store_refusal.clone();
        if refusal.is_none() && populated(&target) {
            refusal = Some(format!(
                "{target} already holds snapshots; merging two trees is not something this command will guess at",
                target = target.display()
            ));
        }
        if refusal.is_none() {
            if let Err(error) = crate::build_store::validate_team_slug(&team_slug) {
                refusal = Some(format!("{}: {error}", source.display()));
            }
        }
        let source_device = std::fs::metadata(&source)
            .map(|meta| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt as _;
                    meta.dev()
                }
                #[cfg(not(unix))]
                {
                    0
                }
            })
            .unwrap_or(0);
        teams.push(SnapshotTeamMigration {
            team_slug,
            same_device: source_device == store_device,
            source,
            target,
            files,
            bytes: total,
            refusal,
        });
    }
    Ok(SnapshotMigrationPlan {
        archive: archive.to_path_buf(),
        store_root,
        teams,
        already_external: recorded
            .as_ref()
            .is_some_and(|pointer| pointer.layout == "external"),
    })
}

fn copy_across_devices(source: &Path, target: &Path) -> Result<(), String> {
    let staging = target
        .parent()
        .unwrap_or(target)
        .join(COPY_STAGING_DIRECTORY)
        .join(target.file_name().unwrap_or_default());
    if staging.exists() {
        remove_tree(&staging)?;
    }
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent).map_err(|error| crate::json::os_error(&error, parent))?;
    }
    copy_tree(source, &staging)?;
    let before = tree_size(source);
    let after = tree_size(&staging);
    if before != after {
        let _ = remove_tree(&staging);
        return Err(format!(
            "copy of {} to {} does not match: ({}, {}) files/bytes became ({}, {}); the original has been left alone",
            source.display(),
            target.display(),
            before.0,
            before.1,
            after.0,
            after.1,
        ));
    }
    std::fs::rename(&staging, target).map_err(|error| crate::json::os_error(&error, &staging))?;
    fsync_directory(target.parent().unwrap_or(target))?;
    remove_tree(source)
}

/// Relocate an existing archive's snapshots out of the published tree.
pub fn migrate_snapshots(
    archive: &Path,
    requested: Option<&Path>,
    moving: bool,
    copy_across_devices: bool,
) -> Result<SnapshotMigrationResult, String> {
    let plan = plan_snapshot_migration(archive, requested)?;
    if !moving {
        return Ok(SnapshotMigrationResult {
            plan,
            moved: Vec::new(),
            copied: Vec::new(),
            pointer_changed: false,
        });
    }
    let blocked: Vec<&SnapshotTeamMigration> = plan
        .teams
        .iter()
        .filter(|team| team.refusal.is_some())
        .collect();
    if !blocked.is_empty() {
        let list = blocked
            .iter()
            .map(|team| {
                format!(
                    "{}: {}",
                    team.team_slug,
                    team.refusal.as_deref().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        return Err(format!("refusing to migrate; nothing was moved:\n  {list}"));
    }
    let cross: Vec<&SnapshotTeamMigration> = plan
        .movable()
        .into_iter()
        .filter(|team| !team.same_device)
        .collect();
    if !cross.is_empty() && !copy_across_devices {
        let list = cross
            .iter()
            .map(|team| team.team_slug.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{} is on a different filesystem from {list}, so this cannot be a rename. Pass --copy to accept a copy-verify-delete instead: unlike a rename it is interruptible, and while it runs the bytes exist twice.",
            plan.store_root.display()
        ));
    }
    ensure_store_root(archive, &plan.store_root)?;
    let pointer_changed = write_pointer(
        archive,
        &SnapshotLocation {
            archive: archive.to_path_buf(),
            team_slug: String::new(),
            root: plan.store_root.clone(),
            store_root: Some(plan.store_root.clone()),
            layout: "external".into(),
            origin: "requested",
        },
    )?;
    let mut moved = Vec::new();
    let mut copied = Vec::new();
    for team in plan.movable() {
        if team.same_device {
            std::fs::rename(&team.source, &team.target)
                .map_err(|error| crate::json::os_error(&error, &team.source))?;
            moved.push(team.team_slug.clone());
        } else {
            copy_across_devices_impl(&team.source, &team.target)?;
            copied.push(team.team_slug.clone());
        }
        if let Some(parent) = team.source.parent() {
            fsync_directory(parent)?;
        }
        fsync_directory(&plan.store_root)?;
    }
    Ok(SnapshotMigrationResult {
        plan,
        moved,
        copied,
        pointer_changed,
    })
}

fn copy_across_devices_impl(source: &Path, target: &Path) -> Result<(), String> {
    copy_across_devices(source, target)
}

/// Render the plan or the outcome as operator text.
pub fn format_snapshot_migration_report(result: &SnapshotMigrationResult, moved: bool) -> String {
    let plan = &result.plan;
    let mut lines: Vec<String> = vec![
        format!("archive:    {}", plan.archive.display()),
        format!("store root: {}", plan.store_root.display()),
        String::new(),
    ];
    if plan.teams.is_empty() {
        lines.push(format!(
            "No team keeps snapshots inside the archive.{}",
            if plan.already_external {
                String::new()
            } else {
                " Recording the external layout is all this would do.".to_string()
            }
        ));
    }
    for team in &plan.teams {
        let mut verb = "would move".to_string();
        if moved {
            verb = if result.moved.contains(&team.team_slug) {
                "moved".into()
            } else {
                "copied".into()
            };
        }
        if team.refusal.is_some() {
            verb = "REFUSED".into();
        }
        let mut line = format!(
            "{}: {verb} {} file(s), {} byte(s)\n    from {}\n      to {}",
            team.team_slug,
            team.files,
            team.bytes,
            team.source.display(),
            team.target.display()
        );
        if !team.same_device {
            line.push_str("\n    (different filesystem: copy, not rename)");
        }
        if let Some(refusal) = &team.refusal {
            line.push_str(&format!("\n    refusal: {refusal}"));
        }
        lines.push(line);
    }
    lines.push(String::new());
    if moved {
        lines.push(format!(
            "moved {} team(s), copied {}; {} now records the external layout",
            result.moved.len(),
            result.copied.len(),
            plan.archive.join(POINTER_FILE).display()
        ));
    } else {
        lines.push(format!(
            "dry run: {} file(s), {} byte(s) would leave the published tree. Re-run with --move to do it.",
            plan.total_files(),
            plan.total_bytes()
        ));
    }
    format!("{}\n", lines.join("\n"))
}
