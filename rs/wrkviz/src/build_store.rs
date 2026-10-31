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

/// The candidate build-store roots for *archive*.
pub fn candidate_store_roots(archive: &Path) -> Result<Vec<PathBuf>, String> {
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

/// Tenants of one team's build state, in order.
pub const TENANTS: [&str; 2] = ["raw", "payloads"];
/// The store entry for shared build state.
pub const SHARED_TENANT: &str = "_shared";
/// Shared-tenant files by in-archive path.
pub const SHARED_FILES: [&str; 1] = ["extracted/transcripts/occurrences.jsonl"];

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn tree_size(root: &Path) -> (i64, i64) {
    let mut files = 0i64;
    let mut total = 0i64;
    fn visit(dir: &Path, files: &mut i64, total: &mut i64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        items.sort();
        for item in items {
            if item.is_dir() && !is_symlink(&item) {
                visit(&item, files, total);
            } else if item.is_file() && !is_symlink(&item) {
                *files += 1;
                *total += item
                    .symlink_metadata()
                    .map(|meta| meta.len() as i64)
                    .unwrap_or(0);
            }
        }
    }
    visit(root, &mut files, &mut total);
    (files, total)
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
    // shutil.copytree(symlinks=True)
    std::fs::create_dir_all(target).map_err(|error| crate::json::os_error(&error, target))?;
    let entries =
        std::fs::read_dir(source).map_err(|error| crate::json::os_error(&error, source))?;
    let mut items: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    items.sort();
    for item in items {
        let name = item.file_name().unwrap_or_default();
        let dest = target.join(name);
        if is_symlink(&item) {
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

fn digest_file(path: &Path) -> Result<String, String> {
    use sha2::Digest as _;
    let bytes = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    Ok(sha2::Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn copy_tenant_across_devices(source: &Path, target: &Path, verify: bool) -> Result<(), String> {
    if target.exists() {
        return Err(format!(
            "refusing to overwrite existing build state: {}",
            target.display()
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| crate::json::os_error(&error, parent))?;
    }
    copy_tree(source, target)?;
    if verify {
        for (source_path, target_path) in tenant_files(source, target) {
            if digest_file(&source_path)? != digest_file(&target_path)? {
                // shutil cleanup: remove the partial copy, then refuse.
                let _ = remove_dir_all(target);
                return Err(format!(
                    "copy verification failed for {} -> {}",
                    source_path.display(),
                    target_path.display()
                ));
            }
        }
    }
    remove_dir_all(source)?;
    fsync_directory(target.parent().unwrap_or(target))
}

fn tenant_files(source: &Path, target: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    fn visit(
        dir: &Path,
        source_root: &Path,
        target_root: &Path,
        pairs: &mut Vec<(PathBuf, PathBuf)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        items.sort();
        for item in items {
            if item.is_dir() && !is_symlink(&item) {
                visit(&item, source_root, target_root, pairs);
            } else if item.is_file() && !is_symlink(&item) {
                if let Ok(relative) = item.strip_prefix(source_root) {
                    pairs.push((item.clone(), target_root.join(relative)));
                }
            }
        }
    }
    visit(source, source, target, &mut pairs);
    pairs
}

fn remove_dir_all(path: &Path) -> Result<(), String> {
    if is_symlink(path) || path.is_file() {
        return std::fs::remove_file(path).map_err(|error| crate::json::os_error(&error, path));
    }
    if !path.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(path).map_err(|error| crate::json::os_error(&error, path))?;
    for entry in entries.flatten() {
        remove_dir_all(&entry.path())?;
    }
    std::fs::remove_dir(path).map_err(|error| crate::json::os_error(&error, path))
}

/// One team's relocation.
#[derive(Clone, Debug)]
pub struct TeamMigration {
    /// Team slug.
    pub team_slug: String,
    /// Source root.
    pub source: PathBuf,
    /// Target root.
    pub target: PathBuf,
    /// Tenant names present.
    pub tenants: Vec<String>,
    /// File count.
    pub files: i64,
    /// Byte count.
    pub bytes: i64,
    /// Same device.
    pub same_device: bool,
}

/// The whole relocation plan.
#[derive(Clone, Debug)]
pub struct BuildMigrationPlan {
    /// Archive.
    pub archive: PathBuf,
    /// Store root.
    pub store_root: PathBuf,
    /// Teams.
    pub teams: Vec<TeamMigration>,
    /// Shared files (source, target, bytes).
    pub shared: Vec<(PathBuf, PathBuf, i64)>,
}

impl BuildMigrationPlan {
    fn files(&self) -> i64 {
        self.teams.iter().map(|team| team.files).sum::<i64>() + self.shared.len() as i64
    }

    fn bytes(&self) -> i64 {
        self.teams.iter().map(|team| team.bytes).sum::<i64>()
            + self.shared.iter().map(|(_, _, size)| size).sum::<i64>()
    }
}

/// What a build-state migration did.
#[derive(Clone, Debug)]
pub struct BuildMigrationResult {
    /// The plan.
    pub plan: BuildMigrationPlan,
    /// Applied moves.
    pub moved: Vec<TeamMigration>,
    /// Pointer written.
    pub pointer_written: bool,
}

/// Plan moving every team's build state into the store.
pub fn plan_build_migration(
    archive: &Path,
    requested: Option<&Path>,
) -> Result<BuildMigrationPlan, String> {
    let store_root = crate::json::resolve_path(
        &requested.map_or_else(|| default_store_root(archive), Path::to_path_buf),
    );
    let mut migrations = Vec::new();
    for source in legacy_team_roots(archive) {
        let tenants: Vec<String> = TENANTS
            .iter()
            .filter(|tenant| {
                let dir = source.join(tenant);
                dir.is_dir()
                    && !is_symlink(&dir)
                    && std::fs::read_dir(&dir)
                        .map(|mut entries| entries.next().is_some())
                        .unwrap_or(false)
            })
            .map(|tenant| tenant.to_string())
            .collect();
        let (mut files, mut total) = (0i64, 0i64);
        for tenant in &tenants {
            let (tenant_files, tenant_bytes) = tree_size(&source.join(tenant));
            files += tenant_files;
            total += tenant_bytes;
        }
        let name = source
            .file_name()
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        migrations.push(TeamMigration {
            team_slug: name.clone(),
            source: source.clone(),
            target: store_root.join(&name),
            tenants,
            files,
            bytes: total,
            same_device: device_of(&source)? == device_of(&store_root)?,
        });
    }
    let mut shared = Vec::new();
    for relative in SHARED_FILES {
        let source = archive.join(relative);
        if source.is_file() && !is_symlink(&source) {
            let size = source
                .symlink_metadata()
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            shared.push((
                source,
                store_root
                    .join(SHARED_TENANT)
                    .join(Path::new(relative).file_name().unwrap_or_default()),
                size,
            ));
        }
    }
    Ok(BuildMigrationPlan {
        archive: archive.to_path_buf(),
        store_root,
        teams: migrations,
        shared,
    })
}

fn write_build_pointer(archive: &Path, store_root: &Path) -> Result<bool, String> {
    let relative = pathdiff_relative(store_root, archive);
    let mut record = serde_json::Map::new();
    record.insert("schema_version".into(), 1.into());
    record.insert("layout".into(), Value::String("external".into()));
    record.insert("store_root".into(), Value::String(relative));
    crate::transcript_io::write_text_if_changed(
        &archive.join("build-root.json"),
        &crate::json::dump_canonical(&Value::Object(record)),
    )
}

fn pathdiff_relative(target: &Path, base: &Path) -> String {
    // os.path.relpath semantics over resolved paths.
    let target_resolved = crate::json::resolve_path(target);
    let base_resolved = crate::json::resolve_path(base);
    let target_parts: Vec<String> = target_resolved
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let base_parts: Vec<String> = base_resolved
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
    let mut out: Vec<String> = vec!["..".to_string(); base_parts.len() - common];
    out.extend(target_parts[common..].iter().cloned());
    if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    }
}

/// Move every team's build state into the store (dry-run by default).
pub fn migrate_build_state(
    archive: &Path,
    requested: Option<&Path>,
    apply: bool,
) -> Result<BuildMigrationResult, String> {
    let plan = plan_build_migration(archive, requested)?;
    if !apply {
        return Ok(BuildMigrationResult {
            plan,
            moved: Vec::new(),
            pointer_written: false,
        });
    }
    // verify=True tenants: none today (PAID_TENANTS is empty).
    let mut moved = Vec::new();
    for team in &plan.teams {
        std::fs::create_dir_all(&team.target)
            .map_err(|error| crate::json::os_error(&error, &team.target))?;
        for tenant in &team.tenants {
            let source = team.source.join(tenant);
            let target = team.target.join(tenant);
            if team.same_device {
                if target.exists() {
                    return Err(format!(
                        "refusing to overwrite existing build state: {}",
                        target.display()
                    ));
                }
                std::fs::rename(&source, &target)
                    .map_err(|error| crate::json::os_error(&error, &source))?;
            } else {
                copy_tenant_across_devices(&source, &target, false)?;
            }
        }
        fsync_directory(&team.target)?;
        moved.push(team.clone());
    }
    for (source, target, _size) in &plan.shared {
        if target.exists() {
            return Err(format!(
                "refusing to overwrite existing build state: {}",
                target.display()
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| crate::json::os_error(&error, parent))?;
        }
        if device_of(source)? == device_of(target.parent().unwrap_or(target))? {
            std::fs::rename(source, target)
                .map_err(|error| crate::json::os_error(&error, source))?;
        } else {
            std::fs::copy(source, target).map_err(|error| crate::json::os_error(&error, target))?;
            std::fs::remove_file(source).map_err(|error| crate::json::os_error(&error, source))?;
        }
        fsync_directory(target.parent().unwrap_or(target))?;
    }
    std::fs::create_dir_all(&plan.store_root)
        .map_err(|error| crate::json::os_error(&error, &plan.store_root))?;
    let gitignore = plan.store_root.join(".gitignore");
    if !gitignore.is_file() {
        std::fs::write(&gitignore, "/*\n")
            .map_err(|error| crate::json::os_error(&error, &gitignore))?;
    }
    let pointer_written = write_build_pointer(archive, &plan.store_root)?;
    Ok(BuildMigrationResult {
        plan,
        moved,
        pointer_written,
    })
}

fn grouped(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}{out}", if value < 0 { "-" } else { "" })
}

/// Render a build-state migration report.
pub fn format_build_migration_report(result: &BuildMigrationResult, applied: bool) -> String {
    let plan = &result.plan;
    let verb = if applied { "Moved" } else { "Would move" };
    if plan.teams.is_empty() && plan.shared.is_empty() {
        return format!(
            "No build state inside the archive; nothing to migrate.\nStore: {}\n",
            plan.store_root.display()
        );
    }
    let mut lines = vec![format!(
        "{verb} build state for {} team(s) ({} files, {} bytes) into {}",
        plan.teams.len(),
        grouped(plan.files()),
        grouped(plan.bytes()),
        plan.store_root.display()
    )];
    for (source, _target, size) in &plan.shared {
        let relative = source
            .strip_prefix(&plan.archive)
            .map(|item| item.display().to_string())
            .unwrap_or_else(|_| source.display().to_string());
        lines.push(format!(
            "  {SHARED_TENANT}: {relative} ({} bytes)",
            grouped(*size)
        ));
    }
    for team in &plan.teams {
        let crossing = if team.same_device {
            String::new()
        } else {
            "  [cross-device copy+verify]".to_string()
        };
        lines.push(format!(
            "  {}: {} ({} files, {} bytes){crossing}",
            team.team_slug,
            team.tenants.join("+"),
            grouped(team.files),
            grouped(team.bytes)
        ));
    }
    if !applied {
        lines.push(String::new());
        lines.push("Dry run. Re-run with --move to perform the move.".to_string());
    }
    format!("{}\n", lines.join("\n"))
}
