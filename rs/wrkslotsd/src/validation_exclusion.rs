//! Step 4b-i of the ownerless-validation port (#1089): the
//! exclusion machinery - mount-alias authentication, the trusted
//! root-owned sticky root, the guard create/seal/move sequence,
//! and crash restore - with fixture tests. No deletion path is
//! wired here; `_remove_excluded_validation_tree` and the
//! journaled removal execution land in step 4b-ii.
//!
//! Root-owned steps run through the same `sudo -n -u root`
//! programs the Python edition names (`_run_root_owned_command`),
//! each path root-owned-checked before it runs.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::Cfg;
use crate::validation_recovery::{open_directory_identity, ownerless_validation_paths};

type Result<T> = std::result::Result<T, String>;

/// The production exclusion roots (Python
/// `_OWNERLESS_VALIDATION_EXCLUSION_ROOTS`); tests pass `/tmp`.
pub(crate) const EXCLUSION_ROOTS: &[&str] = &["/data/misc"];

/// A parsed `/proc/self/mountinfo` record: (mount id, major:minor,
/// filesystem root, mount point, fs type, source).
struct MountRecord {
    mount_id: u64,
    dev: String,
    root: PathBuf,
    point: PathBuf,
    fs: String,
    source: String,
}

fn mount_records() -> Result<Vec<MountRecord>> {
    let text = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("cannot read mount layout for validation exclusion: {error}"))?;
    let mut records = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            return Err(
                "mount layout is malformed while preparing validation exclusion".to_string(),
            );
        };
        let fields: Vec<&str> = left.split_whitespace().collect();
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if fields.len() < 5 || right_fields.len() < 2 {
            return Err(
                "mount layout is malformed while preparing validation exclusion".to_string(),
            );
        }
        let mount_id: u64 = fields[0]
            .parse()
            .map_err(|_| "mount layout has a non-numeric mount id".to_string())?;
        let decode = |raw: &str| raw.replace("\\040", " ").replace("\\011", "\t");
        records.push(MountRecord {
            mount_id,
            dev: fields[2].to_string(),
            root: PathBuf::from(decode(fields[3])),
            point: PathBuf::from(decode(fields[4])),
            fs: right_fields[0].to_string(),
            source: right_fields[1].to_string(),
        });
    }
    Ok(records)
}

fn record_for_path<'a>(
    records: &'a [MountRecord],
    path: &Path,
    label: &str,
) -> Result<&'a MountRecord> {
    let matches: Vec<&MountRecord> = records
        .iter()
        .filter(|record| path.starts_with(&record.point))
        .collect();
    if matches.is_empty() {
        return Err(format!(
            "cannot identify the mount containing {label}: {}",
            path.display()
        ));
    }
    let depth = matches
        .iter()
        .map(|record| record.point.components().count())
        .max()
        .unwrap_or_default();
    let closest: Vec<&MountRecord> = matches
        .into_iter()
        .filter(|record| record.point.components().count() == depth)
        .collect();
    if closest.len() != 1 {
        return Err(format!(
            "mount layout is ambiguous for {label}: {}",
            path.display()
        ));
    }
    Ok(closest[0])
}

/// Python `_mount_alias_path` + `_path_alias_on_mount`.
fn alias_on_mount(
    path: &Path,
    destination_root: &Path,
    records: &[MountRecord],
) -> Result<PathBuf> {
    let source = record_for_path(records, path.parent().unwrap_or(path), "validation path")?;
    let destination = record_for_path(records, destination_root, "validation exclusion root")?;
    let alias = if source.mount_id == destination.mount_id {
        path.to_path_buf()
    } else {
        if (source.dev.clone(), source.fs.clone(), source.source.clone())
            != (
                destination.dev.clone(),
                destination.fs.clone(),
                destination.source.clone(),
            )
        {
            return Err("validation exclusion root is on another filesystem".to_string());
        }
        let parent = path.parent().unwrap_or(path);
        let inside = source.root.join(
            parent
                .strip_prefix(&source.point)
                .map_err(|_| "validation path is outside its mount".to_string())?,
        );
        let relative = inside.strip_prefix(&destination.root).map_err(|_| {
            "validation path is outside the filesystem view containing the exclusion root"
                .to_string()
        })?;
        destination.point.join(relative).join(
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    let source_parent = std::fs::symlink_metadata(path.parent().unwrap_or(path))
        .map_err(|error| format!("cannot authenticate validation path mount alias: {error}"))?;
    let alias_parent = std::fs::symlink_metadata(alias.parent().unwrap_or(&alias))
        .map_err(|error| format!("cannot authenticate validation path mount alias: {error}"))?;
    if !source_parent.is_dir()
        || !alias_parent.is_dir()
        || (source_parent.dev(), source_parent.ino()) != (alias_parent.dev(), alias_parent.ino())
    {
        return Err("validation path mount alias names another parent directory".to_string());
    }
    Ok(alias)
}

/// Python `_trusted_validation_exclusion_root`.
fn trusted_root(fenced: &Path, roots: &[PathBuf]) -> Result<(PathBuf, (u64, u64, u64), PathBuf)> {
    let records = mount_records()?;
    let source_mount = record_for_path(
        &records,
        fenced.parent().unwrap_or(fenced),
        "validation path",
    )?;
    let mut direct = Vec::new();
    let mut aliases = Vec::new();
    for candidate in roots {
        let Ok(metadata) = candidate.symlink_metadata() else {
            continue;
        };
        let Ok(resolved) = candidate.canonicalize() else {
            continue;
        };
        if resolved != *candidate
            || !metadata.is_dir()
            || metadata.uid() != 0
            || metadata.gid() != 0
            || metadata.permissions_mode() != 0o1777
        {
            continue;
        }
        let Ok(identity) = open_directory_identity(candidate, "validation exclusion root") else {
            continue;
        };
        let Ok(alias) = alias_on_mount(fenced, candidate, &records) else {
            continue;
        };
        let Ok(destination_mount) =
            record_for_path(&records, candidate, "validation exclusion root")
        else {
            continue;
        };
        if destination_mount.mount_id == source_mount.mount_id {
            direct.push((candidate.clone(), identity, alias));
        } else {
            aliases.push((candidate.clone(), identity, alias));
        }
    }
    let selected = if direct.is_empty() { aliases } else { direct };
    if selected.len() != 1 {
        return Err(format!(
            "validation exclusion requires exactly one trusted root-owned, same-filesystem sticky parent; found {}",
            selected.len()
        ));
    }
    Ok(selected.into_iter().next().expect("one root"))
}

trait PermissionsMode {
    fn permissions_mode(&self) -> u32;
}

impl PermissionsMode for std::fs::Metadata {
    fn permissions_mode(&self) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        self.permissions().mode() & 0o7777
    }
}

/// Python `_ownerless_validation_exclusion_name`.
pub(crate) fn exclusion_name(cfg: &Cfg, fenced: &Path) -> Result<String> {
    let name = fenced
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let token = name.rsplit('.').next().unwrap_or_default();
    if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("fenced validation path has no exclusion identity".to_string());
    }
    let mut hasher = Sha256::new();
    hasher.update(cfg.root.to_string_lossy().as_bytes());
    hasher.update([0u8]);
    hasher.update(fenced.to_string_lossy().as_bytes());
    let context = format!("{:x}", hasher.finalize());
    Ok(format!(
        ".wrkslots-ownerless-validation.{token}.{}",
        &context[..16]
    ))
}

/// One root-owned command output, Python `_root_validation_exclusion_command`.
fn root_command(
    program: &str,
    args: &[String],
    label: &str,
    allow_stdout: bool,
) -> Result<Vec<u8>> {
    root_command_as(program, args, label, allow_stdout, "/usr/bin/sudo")
}

fn root_command_as(
    program: &str,
    args: &[String],
    label: &str,
    allow_stdout: bool,
    sudo: &str,
) -> Result<Vec<u8>> {
    for candidate in [sudo, program] {
        let metadata = std::fs::metadata(candidate).map_err(|error| {
            format!("cannot inspect root-owned executable {candidate}: {error}")
        })?;
        if !metadata.is_file() || metadata.uid() != 0 || metadata.permissions_mode() & 0o022 != 0 {
            return Err(format!("root-owned executable is not trusted: {candidate}"));
        }
    }
    let output = std::process::Command::new(sudo)
        .args(["-n", "-u", "root", "--", program])
        .args(args)
        .output()
        .map_err(|error| format!("verified root context could not {label}: {error}"))?;
    if !output.status.success()
        || !output.stderr.is_empty()
        || (!output.stdout.is_empty() && !allow_stdout)
    {
        let detail = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let detail = if detail.is_empty() {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            detail
        };
        let suffix = if detail.is_empty() {
            format!(" (exit {})", output.status.code().unwrap_or_default())
        } else {
            format!(": {detail}")
        };
        return Err(format!("verified root context could not {label}{suffix}"));
    }
    Ok(output.stdout)
}

fn assert_root_context() -> Result<()> {
    let out = root_command("/usr/bin/id", &["-u".to_string()], "identify itself", true)?;
    if out != b"0\n" {
        return Err("validation exclusion command did not run as uid 0".to_string());
    }
    Ok(())
}

/// (dev, ino) from the root-owned stat program.
fn root_metadata(path: &Path, label: &str) -> Result<(u64, u64, u32, u32, u32)> {
    let out = root_command(
        "/usr/bin/stat",
        &[
            "--format=%d:%i:%f:%u:%g".to_string(),
            "--".to_string(),
            path.to_string_lossy().into_owned(),
        ],
        &format!("inspect {label}"),
        true,
    )?;
    let text = String::from_utf8_lossy(&out);
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() != 5 {
        return Err(format!(
            "verified root context returned malformed {label} metadata"
        ));
    }
    let parse = |index: usize, base: u32| {
        u64::from_str_radix(parts[index], base)
            .map_err(|_| format!("verified root context returned malformed {label} metadata"))
    };
    Ok((
        parse(0, 10)?,
        parse(1, 10)?,
        parse(2, 16)? as u32,
        parse(3, 10)? as u32,
        parse(4, 10)? as u32,
    ))
}

fn root_names(path: &Path) -> Result<Vec<String>> {
    let out = root_command(
        "/usr/bin/find",
        &[
            path.to_string_lossy().into_owned(),
            "-mindepth".to_string(),
            "1".to_string(),
            "-maxdepth".to_string(),
            "1".to_string(),
            "-printf".to_string(),
            "%f\\0".to_string(),
        ],
        "enumerate the validation exclusion guard",
        true,
    )?;
    let text = String::from_utf8(out)
        .map_err(|_| "validation exclusion guard contains a non-UTF-8 name".to_string())?;
    let mut names: Vec<String> = text.split('\0').map(str::to_string).collect();
    if names.last().is_some_and(|name| name.is_empty()) {
        names.pop();
    }
    if names
        .iter()
        .any(|name| name.is_empty() || name.contains('/'))
    {
        return Err("validation exclusion guard contains a malformed name".to_string());
    }
    names.sort();
    Ok(names)
}

/// Python `_ownerless_validation_exclusion_to_obj`.
fn exclusion_obj(
    phase: &str,
    root: &Path,
    root_identity: (u64, u64, u64),
    guard: &Path,
    guard_identity: (u64, u64, u64),
    source_alias: &Path,
    payload_identity: (u64, u64),
) -> Value {
    json!({
        "schema": 1,
        "phase": phase,
        "root": root.to_string_lossy(),
        "root_identity": [root_identity.0, root_identity.1, root_identity.2],
        "guard": guard.to_string_lossy(),
        "guard_identity": [guard_identity.0, guard_identity.1, guard_identity.2],
        "source_alias": source_alias.to_string_lossy(),
        "payload_identity": [payload_identity.0, payload_identity.1],
    })
}

struct ExclusionRecord {
    root: PathBuf,
    root_identity: (u64, u64, u64),
    guard: PathBuf,
    guard_identity: (u64, u64, u64),
    source_alias: PathBuf,
    payload_identity: (u64, u64),
}

fn exclusion_from_obj(cfg: &Cfg, fenced: &Path, value: &Value) -> Result<ExclusionRecord> {
    let Some(raw) = value.as_object() else {
        return Err("validation exclusion record must be an object".to_string());
    };
    if raw.get("schema").and_then(Value::as_i64) != Some(1) {
        return Err("validation exclusion record schema is invalid".to_string());
    }
    let phase = raw.get("phase").and_then(Value::as_str).unwrap_or_default();
    if !matches!(phase, "guard-created" | "payload-moved") {
        return Err(format!("unknown validation exclusion phase {phase:?}"));
    }
    let text = |key: &str| {
        raw.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let identity = |key: &str| -> (u64, u64, u64) {
        let items = raw
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        (
            items.first().and_then(Value::as_u64).unwrap_or_default(),
            items.get(1).and_then(Value::as_u64).unwrap_or_default(),
            items.get(2).and_then(Value::as_u64).unwrap_or_default(),
        )
    };
    let root = PathBuf::from(text("root"));
    let guard = PathBuf::from(text("guard"));
    if guard != root.join(exclusion_name(cfg, fenced)?) {
        return Err("validation exclusion guard does not match its fenced path".to_string());
    }
    let payload_items = raw
        .get("payload_identity")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(ExclusionRecord {
        root,
        root_identity: identity("root_identity"),
        guard,
        guard_identity: identity("guard_identity"),
        source_alias: PathBuf::from(text("source_alias")),
        payload_identity: (
            payload_items
                .first()
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            payload_items
                .get(1)
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        ),
    })
}

/// A begun exclusion: the payload sits behind the sealed guard.
pub(crate) struct Exclusion {
    /// The trusted root.
    pub root: PathBuf,
    /// The guard directory (mode 000 while begun).
    pub guard: PathBuf,
    /// The payload inside the guard.
    pub payload: PathBuf,
    /// The mount alias the payload was moved from.
    pub source_alias: PathBuf,
    payload_file: std::fs::File,
}

impl Exclusion {
    /// The still-open payload descriptor's stable identity.
    pub(crate) fn payload_identity(&self) -> Result<(u64, u64, i64, String)> {
        use std::os::unix::io::AsRawFd as _;

        let label = self.payload.to_string_lossy().into_owned();
        let (kind, hex) =
            crate::stable_identity::fd_file_handle(self.payload_file.as_raw_fd(), &label)?;
        let metadata = rustix::fs::fstat(&self.payload_file)
            .map_err(|error| format!("cannot inspect excluded validation checkout: {error}"))?;
        Ok((metadata.st_dev, metadata.st_ino, i64::from(kind), hex))
    }
}

/// Python `_begin_ownerless_validation_exclusion`.
pub(crate) fn begin_exclusion(
    cfg: &Cfg,
    journal: &mut serde_json::Map<String, Value>,
    roots: &[PathBuf],
) -> Result<Exclusion> {
    let (authorization, _target, fenced, _phase) = ownerless_validation_paths(cfg, journal)?;
    if _target.exists() || _target.is_symlink() {
        return Err(format!(
            "canonical validation path reappeared before cleanup: {}",
            _target.display()
        ));
    }
    if !fenced.is_dir() || fenced.is_symlink() {
        return Err(format!(
            "fenced validation path disappeared before exclusion: {}",
            fenced.display()
        ));
    }
    let (root, root_identity, source_alias) = trusted_root(&fenced, roots)?;
    let guard = root.join(exclusion_name(cfg, &fenced)?);
    let payload = guard.join("checkout");
    let parent_identity = authorization
        .parent_identity
        .ok_or_else(|| "validation exclusion authorization has no parent identity".to_string())?;
    if open_directory_identity(
        fenced.parent().unwrap_or(&fenced),
        "fenced validation checkout",
    )? != parent_identity
    {
        return Err("fenced validation checkout identity changed before cleanup".to_string());
    }
    assert_root_context()?;
    let root_meta = root.symlink_metadata().map_err(|error| {
        format!(
            "cannot open validation exclusion root {}: {error}",
            root.display()
        )
    })?;
    if (root_meta.dev(), root_meta.ino()) != (root_identity.0, root_identity.1)
        || root_meta.uid() != 0
        || root_meta.gid() != 0
        || root_meta.permissions_mode() != 0o1777
    {
        return Err("validation exclusion root identity changed before use".to_string());
    }
    if guard.symlink_metadata().is_ok() {
        return Err(format!(
            "validation exclusion guard already exists: {}",
            guard.display()
        ));
    }
    let payload_file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&fenced)
            .map_err(|error| {
                format!(
                    "cannot safely open fenced validation checkout {}: {error}",
                    fenced.display()
                )
            })?
    };
    let payload_identity_now = open_directory_identity(&fenced, "fenced validation checkout")?;
    if payload_identity_now != authorization.identity {
        return Err("validation checkout identity changed before exclusion".to_string());
    }
    let alias_metadata = source_alias
        .symlink_metadata()
        .map_err(|error| format!("cannot authenticate validation path mount alias: {error}"))?;
    if !alias_metadata.is_dir()
        || (alias_metadata.dev(), alias_metadata.ino())
            != (payload_identity_now.0, payload_identity_now.1)
    {
        return Err("validation checkout mount alias names another inode".to_string());
    }
    root_command(
        "/usr/bin/mkdir",
        &[
            "--mode=0755".to_string(),
            "--".to_string(),
            guard.to_string_lossy().into_owned(),
        ],
        "create the validation exclusion guard",
        false,
    )?;
    let guard_identity = open_directory_identity(&guard, "validation exclusion guard")?;
    let guard_metadata = guard.symlink_metadata().map_err(|error| {
        format!(
            "cannot inspect validation exclusion guard {}: {error}",
            guard.display()
        )
    })?;
    if !guard_metadata.is_dir()
        || guard_metadata.uid() != 0
        || guard_metadata.gid() != 0
        || guard_metadata.permissions_mode() != 0o755
        || guard_identity.2 != root_identity.2
    {
        return Err("validation exclusion guard is not root-owned and private".to_string());
    }
    journal.insert(
        "exclusion".to_string(),
        exclusion_obj(
            "guard-created",
            &root,
            root_identity,
            &guard,
            guard_identity,
            &source_alias,
            (authorization.identity.0, authorization.identity.1),
        ),
    );
    let journal_value = Value::Object(journal.clone());
    crate::remove_write::write_journal(cfg, &cfg.machine, &journal_value)?;
    root_command(
        "/usr/bin/chmod",
        &[
            "000".to_string(),
            "--".to_string(),
            guard.to_string_lossy().into_owned(),
        ],
        "seal the validation exclusion guard",
        false,
    )?;
    let sealed = guard.symlink_metadata().map_err(|error| {
        format!(
            "cannot inspect validation exclusion guard {}: {error}",
            guard.display()
        )
    })?;
    if (sealed.dev(), sealed.ino()) != (guard_identity.0, guard_identity.1)
        || sealed.uid() != 0
        || sealed.gid() != 0
        || sealed.permissions_mode() != 0
    {
        return Err("validation exclusion guard did not become root-owned mode 000".to_string());
    }
    root_command(
        "/usr/bin/mv",
        &[
            "--no-clobber".to_string(),
            "--no-target-directory".to_string(),
            "--".to_string(),
            source_alias.to_string_lossy().into_owned(),
            payload.to_string_lossy().into_owned(),
        ],
        "move the validation checkout behind its exclusion guard",
        false,
    )?;
    let moved = root_metadata(&payload, "excluded validation checkout")?;
    if (moved.0, moved.1) != (authorization.identity.0, authorization.identity.1) {
        return Err("validation checkout identity changed during exclusion".to_string());
    }
    if fenced.exists() || fenced.is_symlink() {
        return Err("validation checkout mount alias remains after exclusion move".to_string());
    }
    if let Some(stable) = &authorization.stable_identity {
        let observed = {
            use std::os::unix::io::AsRawFd as _;
            let label = payload.to_string_lossy().into_owned();
            let (kind, hex) =
                crate::stable_identity::fd_file_handle(payload_file.as_raw_fd(), &label)?;
            let metadata = rustix::fs::fstat(&payload_file)
                .map_err(|error| format!("cannot inspect excluded validation checkout: {error}"))?;
            (metadata.st_dev, metadata.st_ino, i64::from(kind), hex)
        };
        if &observed != stable {
            return Err("validation checkout stable identity changed during exclusion".to_string());
        }
    }
    journal.insert(
        "exclusion".to_string(),
        exclusion_obj(
            "payload-moved",
            &root,
            root_identity,
            &guard,
            guard_identity,
            &source_alias,
            (authorization.identity.0, authorization.identity.1),
        ),
    );
    let journal_value = Value::Object(journal.clone());
    crate::remove_write::write_journal(cfg, &cfg.machine, &journal_value)?;
    Ok(Exclusion {
        root,
        guard,
        payload,
        source_alias,
        payload_file,
    })
}

/// Python `_restore_ownerless_validation_exclusion`.
pub(crate) fn restore_exclusion(
    cfg: &Cfg,
    journal: &mut serde_json::Map<String, Value>,
    roots: &[PathBuf],
) -> Result<()> {
    let (authorization, target, fenced, phase) = ownerless_validation_paths(cfg, journal)?;
    let Some(exclusion_value) = journal.get("exclusion").cloned() else {
        for root in roots {
            let guard = root.join(exclusion_name(cfg, &fenced)?);
            if guard.exists() || guard.is_symlink() {
                return Err(
                    "unrecorded validation exclusion guard exists; preserve it for inspection"
                        .to_string(),
                );
            }
        }
        return Ok(());
    };
    if phase != "removing" {
        return Err("validation exclusion record exists outside the removing phase".to_string());
    }
    let exclusion = exclusion_from_obj(cfg, &fenced, &exclusion_value)?;
    let records = mount_records()?;
    let expected_alias = alias_on_mount(&fenced, &exclusion.root, &records)?;
    if expected_alias != exclusion.source_alias {
        return Err("validation exclusion mount alias changed since it was recorded".to_string());
    }
    assert_root_context()?;
    let root_meta = exclusion.root.symlink_metadata().map_err(|error| {
        format!(
            "cannot open validation exclusion root {}: {error}",
            exclusion.root.display()
        )
    })?;
    if (root_meta.dev(), root_meta.ino()) != (exclusion.root_identity.0, exclusion.root_identity.1)
        || root_meta.uid() != 0
        || root_meta.gid() != 0
        || root_meta.permissions_mode() != 0o1777
    {
        return Err("validation exclusion root identity or permissions changed".to_string());
    }
    let guard_metadata = exclusion.guard.symlink_metadata();
    if guard_metadata.is_err() {
        let candidates: Vec<&PathBuf> = [&target, &fenced]
            .into_iter()
            .filter(|candidate| {
                candidate.is_dir()
                    && !candidate.is_symlink()
                    && open_directory_identity(candidate, "restored validation path").ok()
                        == Some(authorization.identity)
            })
            .collect();
        if candidates.len() != 1 {
            return Err(
                "recorded validation exclusion guard disappeared before recovery".to_string(),
            );
        }
        journal.remove("exclusion");
        let journal_value = Value::Object(journal.clone());
        crate::remove_write::write_journal(cfg, &cfg.machine, &journal_value)?;
        return Ok(());
    }
    let guard_metadata = guard_metadata.expect("guard metadata");
    if (guard_metadata.dev(), guard_metadata.ino())
        != (exclusion.guard_identity.0, exclusion.guard_identity.1)
        || !guard_metadata.is_dir()
        || guard_metadata.uid() != 0
        || guard_metadata.gid() != 0
        || !matches!(guard_metadata.permissions_mode(), 0 | 0o755)
    {
        return Err("validation exclusion guard identity or ownership changed".to_string());
    }
    let names = root_names(&exclusion.guard)?;
    let target_present = target.exists() || target.is_symlink();
    let fenced_present = fenced.exists() || fenced.is_symlink();
    if target_present && fenced_present {
        return Err(
            "canonical and fenced validation paths both exist during exclusion recovery"
                .to_string(),
        );
    }
    if names.is_empty() {
        let restored = if target_present { &target } else { &fenced };
        if restored.is_symlink()
            || !restored.is_dir()
            || open_directory_identity(restored, "restored validation path")?
                != authorization.identity
        {
            return Err("restored validation path identity changed".to_string());
        }
    } else if names == ["checkout"] {
        let payload = exclusion.guard.join("checkout");
        let moved = root_metadata(&payload, "excluded validation checkout")?;
        if (moved.0, moved.1) != exclusion.payload_identity
            || exclusion.payload_identity != (authorization.identity.0, authorization.identity.1)
        {
            return Err("excluded validation checkout identity changed; preserve it".to_string());
        }
        let (destination, destination_alias) = if target_present {
            (fenced.clone(), exclusion.source_alias.clone())
        } else if fenced_present {
            (
                target.clone(),
                alias_on_mount(&target, &exclusion.root, &records)?,
            )
        } else {
            (fenced.clone(), exclusion.source_alias.clone())
        };
        root_command(
            "/usr/bin/mv",
            &[
                "--no-clobber".to_string(),
                "--no-target-directory".to_string(),
                "--".to_string(),
                payload.to_string_lossy().into_owned(),
                destination_alias.to_string_lossy().into_owned(),
            ],
            "restore the excluded validation checkout",
            false,
        )?;
        if !destination.is_dir()
            || destination.is_symlink()
            || open_directory_identity(&destination, "restored validation checkout")?
                != authorization.identity
        {
            return Err(
                "excluded validation checkout was not restored to its exact identity".to_string(),
            );
        }
    } else {
        return Err("validation exclusion state is ambiguous; preserve every path".to_string());
    }
    root_command(
        "/usr/bin/rmdir",
        &[
            "--".to_string(),
            exclusion.guard.to_string_lossy().into_owned(),
        ],
        "remove the empty validation exclusion guard",
        false,
    )?;
    if exclusion.guard.symlink_metadata().is_ok() {
        return Err("validation exclusion guard remains after recovery".to_string());
    }
    journal.remove("exclusion");
    let journal_value = Value::Object(journal.clone());
    crate::remove_write::write_journal(cfg, &cfg.machine, &journal_value)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn git(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn exclusion_begin_restore_and_fence_rollback_match_python() {
        // Fixture registry only: a temp project, a hand-built
        // ownerless-validate journal, and /tmp as the trusted root.
        let base = std::env::temp_dir().join(format!(
            "wrkslotsd-exclusion-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let root = base.join("project");
        std::fs::create_dir_all(root.join("worktrees")).expect("dirs");
        std::fs::write(root.join("live.py"), "# probe\n").expect("live");
        let machine = "node-a";
        std::fs::write(
            root.join(".wrkslots.yml"),
            "schema: 2\nmachine: node-a\nworktrees_dir: worktrees\ndefault_remote: origin\ndefault_landed_ref: refs/remotes/origin/main\nheartbeat_ttl_seconds: 60\nliveness_command: live.py\n",
        )
        .expect("config");
        std::fs::write(
            root.join(format!("worktrees/ACTIVE.{machine}.json")),
            json!({"schema": 2, "machine": machine, "revision": 1, "slots": []}).to_string(),
        )
        .expect("active");
        std::fs::write(
            root.join(format!("worktrees/ARCHIVED.{machine}.json")),
            json!({"schema": 2, "machine": machine, "revision": 1, "records": []}).to_string(),
        )
        .expect("archived");
        let (cfg, _selected) = crate::active_state::load_cfg(Some(&root), None).expect("cfg");
        // A frozen checkout in the sibling frozen directory.
        let frozen_root = crate::validation_recovery::frozen_validation_directory(&cfg);
        std::fs::create_dir_all(&frozen_root).expect("frozen root");
        std::fs::set_permissions(&frozen_root, std::fs::Permissions::from_mode(0o700))
            .expect("frozen mode");
        let target = frozen_root.join("validate-fresh-a");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::write(target.join("file.txt"), "x\n").expect("file");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("mode");
        let identity = crate::validation_recovery::open_directory_identity(
            &target,
            "validation recovery path",
        )
        .expect("identity");
        let parent_identity = crate::validation_recovery::open_directory_identity(
            &frozen_root,
            "frozen validation checkout parent",
        )
        .expect("parent identity");
        let stable =
            crate::validation_recovery::private_cleanup_identity(&target).expect("stable identity");
        let fenced = frozen_root.join(format!(
            ".validate-fresh-a.ownerless-validate.{}",
            "0".repeat(32)
        ));
        let journal = json!({
            "schema": 2,
            "kind": "ownerless-validate-remove",
            "machine": machine,
            "slot": "validate-fresh-a",
            "phase": "removing",
            "fenced": fenced.to_string_lossy(),
            "authorization": {
                "target_kind": "frozen-checkout",
                "path": target.to_string_lossy(),
                "identity": [identity.0, identity.1, identity.2],
                "actor": {"pid": 1, "start_ticks": 1, "boot_id": "fixture", "host_id": "fixture", "cgroup_path": "/"},
                "evidence": {"kind": "terminal-record", "path": "ignored/validate/runs/run-a.json", "sha256": "0".repeat(64), "target": "0".repeat(40)},
                "no_authored_work": true,
                "no_live_use": true,
                "repository": "upstream-repo",
                "head": "0".repeat(40),
                "remote_url_sha256": "0".repeat(64),
                "parent_identity": [parent_identity.0, parent_identity.1, parent_identity.2],
                "stable_identity": [stable.0, stable.1, stable.2, stable.3],
            },
        });
        let mut journal_map = journal.as_object().expect("journal object").clone();
        // Fence the target, as the removal execution does before
        // the exclusion begins.
        std::fs::rename(&target, &fenced).expect("fence");
        let roots = vec![PathBuf::from("/tmp")];
        let exclusion = begin_exclusion(&cfg, &mut journal_map, &roots).expect("begin exclusion");
        assert!(!fenced.exists());
        // The guard is mode 000: the payload is visible only
        // through the still-open descriptor and root-owned stat.
        assert_eq!(
            root_metadata(&exclusion.payload, "excluded validation checkout")
                .expect("payload stat")
                .0,
            identity.0
        );
        assert_eq!(
            journal_map["exclusion"]["phase"],
            Value::String("payload-moved".to_string())
        );
        assert_eq!(
            exclusion.payload_identity().expect("payload identity"),
            stable
        );
        drop(exclusion);
        restore_exclusion(&cfg, &mut journal_map, &roots).expect("restore exclusion");
        assert!(fenced.is_dir());
        assert!(!journal_map.contains_key("exclusion"));
        // Rolling the fence back restores the canonical name and
        // clears the journal file.
        crate::validation_recovery::rollback_validation_fence(&cfg, &journal_map)
            .expect("rollback fence");
        assert!(target.is_dir());
        let _ = git;
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&frozen_root);
    }
}
