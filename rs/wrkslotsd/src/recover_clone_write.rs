//! Writer: recover one ownerless standalone clone
//! (`recover-ownerless-agent-clone`), plan outcomes.
//!
//! Reproduces the read-only outcomes of the Python command that do
//! not depend on the full clone inventory: the local-machine check,
//! interrupted-journal resume planning with the journal validated
//! as the exact durable plan, registry and hold collisions, target
//! binding, and the root clone's identity facts (a standalone Git
//! directory rather than a linked worktree, an existing HEAD, a
//! clean checkout). The inventory-bound plan and the destructive
//! apply path (rescue pushes, fencing, removal, process/systemd
//! censuses) stay with the Python edition and refuse explicitly.
//! Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{
    as_int, as_list, as_mapping, as_str, exact_keys, identity_from_obj, load_cfg, parse_timestamp,
    slot_directory,
};
use crate::config_validate::validate_name;
use crate::ensure_states::{refuse_partial_state, require_fixture};
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

fn short_hostname() -> Result<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|error| format!("cannot read the machine hostname: {error}"))?;
    let name = raw.trim().split('.').next().unwrap_or_default().to_string();
    validate_name(&name, "machine")?;
    Ok(name)
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

/// (st_dev, st_ino, mount id), mirroring `_open_directory_identity`.
fn directory_identity(path: &Path) -> Result<(i64, i64, i64)> {
    let file = std::fs::File::open(path).map_err(|error| {
        format!(
            "cannot safely open ownerless clone {}: {error}",
            path.display()
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect ownerless clone: {error}"))?;
    if !metadata.is_dir() {
        return Err(format!(
            "ownerless clone is not a directory: {}",
            path.display()
        ));
    }
    use std::os::unix::io::AsRawFd;
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
        .map_err(|error| format!("cannot determine mount identity: {error}"))?;
    let mount_id = info
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:"))
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .ok_or("cannot determine mount identity: mnt_id is absent")?;
    Ok((metadata.dev() as i64, metadata.ino() as i64, mount_id))
}

fn target_inside(cfg: &crate::active_state::Cfg, raw_path: &str) -> Result<(String, PathBuf)> {
    // Mirrors _ownerless_agent_target over _relative_inside.
    let agent_root = slot_directory(cfg, "slot", "agent")?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if Path::new(raw_path).is_absolute()
        || raw_path.is_empty()
        || Path::new(raw_path)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(format!(
            "ownerless agent clone must be a relative path inside {}: {raw_path:?}",
            cfg.root.display()
        ));
    }
    let mut lexical = PathBuf::new();
    for part in Path::new(raw_path).components() {
        if let std::path::Component::Normal(name) = part {
            lexical.push(name);
        }
    }
    if lexical.as_os_str().is_empty() {
        return Err("ownerless agent clone must not be the project root".to_string());
    }
    let target = cfg.root.join(&lexical);
    {
        let mut current = cfg.root.clone();
        for part in lexical.components() {
            current.push(part.as_os_str());
            if current.is_symlink() {
                return Err(format!(
                    "ownerless agent clone crosses a symlink: {}",
                    current.display()
                ));
            }
        }
    }
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    if target.parent() != Some(agent_root.as_path()) || name == "ignored" {
        return Err(format!(
            "ownerless agent clone must be one direct child of {}, excluding ignored",
            agent_root.display()
        ));
    }
    validate_name(&name, "slot")?;
    Ok((lexical.to_string_lossy().into_owned(), target))
}

fn journal_resume(
    cfg: &crate::active_state::Cfg,
    selected: &str,
    relative: &str,
    target: &Path,
    name: &str,
    apply: bool,
) -> Result<Option<String>> {
    let control = cfg.control.join(format!("ACTIVE.{selected}.journal"));
    // Enumerate outstanding journals through the control directory.
    let mut journals: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.control) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let text = file_name.to_string_lossy();
            if text.starts_with("ACTIVE.") && text.ends_with(".journal") {
                journals.push(entry.path());
            }
        }
    }
    journals.sort();
    let Some(first) = journals.first() else {
        return Ok(None);
    };
    if journals.len() != 1 || *first != control {
        return Err(format!(
            "another interrupted mutation is recorded in {}",
            first.display()
        ));
    }
    let text = match std::fs::read_to_string(first) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "another interrupted mutation is recorded in {}",
                first.display()
            ));
        }
        Err(error) => return Err(format!("cannot read recovery journal: {error}")),
    };
    let raw: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read recovery journal: {error}"))?;
    let raw = as_mapping(&raw, "ownerless clone journal")?;
    if raw.get("kind").and_then(Value::as_str) != Some("ownerless-clone-remove") {
        return Err(format!(
            "another interrupted mutation is recorded in {}",
            first.display()
        ));
    }
    // Full journal validation, mirroring _ownerless_clone_journal_inputs.
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "slot",
            "phase",
            "fenced",
            "authorization",
            "rescue",
            "record",
        ],
        &[],
        "ownerless clone journal",
    )?;
    if as_int(raw.get("schema").unwrap_or(&Value::Null), "schema", 0)? != 2
        || raw.get("machine").and_then(Value::as_str) != Some(selected)
    {
        return Err("ownerless clone journal identity is invalid".to_string());
    }
    let authorization = as_mapping(
        raw.get("authorization").unwrap_or(&Value::Null),
        "ownerless clone authorization",
    )?;
    exact_keys(
        authorization,
        &[
            "path",
            "identity",
            "actor",
            "remote_urls",
            "inventory_sha256",
            "handoff_sha256s",
            "discard_ignored",
            "recorded_at",
        ],
        &[],
        "ownerless clone authorization",
    )?;
    let auth_path = as_str(
        authorization.get("path").unwrap_or(&Value::Null),
        "ownerless clone path",
    )?;
    if auth_path != relative {
        return Err("requested ownerless path differs from the interrupted recovery".to_string());
    }
    let identity_items = as_list(
        authorization.get("identity").unwrap_or(&Value::Null),
        "ownerless clone directory identity",
    )?;
    if identity_items.len() != 3 {
        return Err("ownerless clone journal is invalid".to_string());
    }
    let mut identity = [0i64; 3];
    for (slot_ref, item) in identity.iter_mut().zip(identity_items) {
        *slot_ref = as_int(item, "ownerless clone directory identity", i64::MIN)?;
    }
    if identity_from_obj(
        authorization.get("actor").unwrap_or(&Value::Null),
        "ownerless clone recovery actor",
    )?
    .is_none()
    {
        return Err("ownerless clone journal is invalid".to_string());
    }
    let urls = as_mapping(
        authorization.get("remote_urls").unwrap_or(&Value::Null),
        "ownerless clone remote URLs",
    )?;
    for url in urls.values() {
        as_str(url, "ownerless clone remote URL")?;
    }
    let digest = as_str(
        authorization
            .get("inventory_sha256")
            .unwrap_or(&Value::Null),
        "ownerless clone inventory digest",
    )?;
    if !is_digest(digest) {
        return Err("ownerless clone journal is invalid".to_string());
    }
    for item in as_list(
        authorization.get("handoff_sha256s").unwrap_or(&Value::Null),
        "ownerless clone handoff digests",
    )? {
        if !is_digest(as_str(item, "ownerless clone handoff digest")?) {
            return Err("ownerless clone journal is invalid".to_string());
        }
    }
    parse_timestamp(
        as_str(
            authorization.get("recorded_at").unwrap_or(&Value::Null),
            "ownerless clone recorded_at",
        )?,
        "ownerless clone recorded_at",
    )?;
    if !authorization
        .get("discard_ignored")
        .is_some_and(Value::is_boolean)
    {
        return Err("ownerless clone journal is invalid".to_string());
    }
    if raw.get("slot").and_then(Value::as_str) != Some(name) {
        return Err("ownerless clone journal is invalid".to_string());
    }
    let fenced_raw = as_str(
        raw.get("fenced").unwrap_or(&Value::Null),
        "ownerless clone fenced path",
    )?;
    let fenced = cfg.root.join(fenced_raw);
    let fenced_name = fenced
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!(".{name}.ownerless-clone.");
    let fenced_ok = fenced.parent() == target.parent()
        && fenced_name.starts_with(&prefix)
        && fenced_name.len() == prefix.len() + 32
        && fenced_name[prefix.len()..]
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase());
    if !fenced_ok {
        return Err("ownerless clone journal is invalid".to_string());
    }
    let phase = as_str(
        raw.get("phase").unwrap_or(&Value::Null),
        "ownerless clone journal.phase",
    )?;
    if !matches!(
        phase,
        "prepared" | "rescued" | "fenced" | "removing" | "removed"
    ) {
        return Err("ownerless clone journal is invalid".to_string());
    }
    for (index, value) in as_list(
        raw.get("rescue").unwrap_or(&Value::Null),
        "ownerless clone journal.rescue",
    )?
    .iter()
    .enumerate()
    {
        as_mapping(value, &format!("ownerless clone journal.rescue[{index}]"))?;
    }
    if let Some(record) = raw.get("record") {
        if !record.is_null() {
            as_mapping(record, "ownerless clone journal.record")?;
        }
    }
    let target_present = target.exists() || target.is_symlink();
    let fenced_present = fenced.exists() || fenced.is_symlink();
    if target_present && fenced_present {
        return Err("canonical and fenced ownerless clone paths both exist".to_string());
    }
    let active: Option<&Path> = if target_present {
        Some(target)
    } else if fenced_present {
        Some(&fenced)
    } else {
        None
    };
    if phase == "removed" && active.is_some() {
        return Err("ownerless clone reappeared after removal was recorded".to_string());
    }
    if let Some(active_path) = active {
        if active_path.is_symlink()
            || !active_path.is_dir()
            || directory_identity(active_path)? != (identity[0], identity[1], identity[2])
        {
            return Err("ownerless clone identity changed; preserve it".to_string());
        }
    }
    if apply {
        return Err(
            "resuming an interrupted ownerless clone recovery stays with the Python edition for now"
                .to_string(),
        );
    }
    Ok(Some(phase.to_string()))
}

/// Plan (or explicitly refuse) ownerless standalone clone recovery.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_ownerless_agent_clone(
    root: Option<&Path>,
    machine_override: Option<&str>,
    raw_path: &str,
    remote_urls: &[(String, String)],
    handoff_sha256s: &[String],
    discard_ignored: bool,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    if selected != short_hostname()? {
        return Err("ownerless clone recovery must run on the clone's machine".to_string());
    }
    for digest in handoff_sha256s {
        if !is_digest(digest) {
            return Err("--handoff-sha256 must be one lowercase SHA-256 digest".to_string());
        }
    }
    let mut seen_keys: Vec<&str> = Vec::new();
    for (key, url) in remote_urls {
        if key.is_empty() || url.is_empty() {
            return Err(format!("--remote-url must be KEY=URL, got {key}={url:?}"));
        }
        if seen_keys.contains(&key.as_str()) {
            return Err(format!("duplicate --remote-url for Git directory {key:?}"));
        }
        seen_keys.push(key.as_str());
        if url.starts_with('-') {
            return Err(format!("remote URL cannot begin with '-': {url:?}"));
        }
        if url.chars().any(|ch| ch.is_control()) {
            return Err("remote URL contains control characters".to_string());
        }
    }
    if apply {
        if !coordinator_authorized {
            return Err(
                "ownerless agent clone recovery requires --coordinator-authorized with --apply"
                    .to_string(),
            );
        }
        let pid = coordinator_pid.ok_or("--apply requires --coordinator-pid")?;
        read_process_identity(pid)?;
    }
    let (relative, target) = target_inside(&cfg, raw_path)?;
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    if let Some(phase) = journal_resume(&cfg, &selected, &relative, &target, &name, apply)? {
        return Ok(json!({
            "machine": selected,
            "path": target.to_string_lossy(),
            "outcome": "planned",
            "detail": "resume",
            "phase": phase,
        }));
    }
    validate_global(root, &cfg)?;
    let active = active_document(root, &cfg, &selected)?;
    if let Some(slots) = active.get("slots").and_then(Value::as_array) {
        for record in slots {
            let slot = record
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if slot == name {
                return Err(format!(
                    "ownerless path name collides with active slot {slot}"
                ));
            }
            for checkout in record
                .get("checkouts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let recorded = cfg.root.join(
                    checkout
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                if recorded == target
                    || recorded.starts_with(&target)
                    || target.starts_with(&recorded)
                {
                    return Err(format!(
                        "ownerless path overlaps registered checkout {slot}/{}",
                        checkout
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }
    let archives = crate::recover_absent_write::archives_document(&cfg, &selected)?;
    if let Some(records) = archives.get("records").and_then(Value::as_array) {
        for record in records {
            if record.get("slot").and_then(Value::as_str) == Some(name.as_str()) {
                return Err(format!(
                    "ownerless path name collides with archived slot {name}"
                ));
            }
        }
    }
    let hold_path = cfg
        .control
        .join(format!("HOLD.{}.{selected}.{name}.json", selected.len()));
    if hold_path.exists() || hold_path.is_symlink() {
        return Err(format!(
            "slot name {name} is held; unhold it before removal"
        ));
    }
    // Root clone identity facts.
    if target.is_symlink() || !target.is_dir() {
        return Err(format!(
            "ownerless clone is absent or unsafe: {}",
            target.display()
        ));
    }
    {
        let mut current = cfg.root.clone();
        let relative_path = target.strip_prefix(&cfg.root).unwrap_or(target.as_path());
        for part in relative_path.components() {
            current.push(part.as_os_str());
            if current.is_symlink() {
                return Err(format!(
                    "ownerless clone crosses a symlink: {}",
                    current.display()
                ));
            }
        }
    }
    if target.join(".git").exists() && !target.join(".git").is_dir() {
        return Err(
            "ownerless clone root is a linked worktree, not a standalone clone".to_string(),
        );
    }
    let git_dir_raw = git(&target, &["rev-parse", "--git-dir"])?;
    let git_dir = {
        let raw = git_dir_raw.trim();
        if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            target.join(raw)
        }
    };
    if git_dir != target.join(".git") && git_dir != target {
        return Err(format!(
            "ownerless clone root is not a standalone clone: {}",
            target.display()
        ));
    }
    git(&target, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    // Cleanliness with the clone proof's tree configuration pinned.
    let status = {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&target)
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "core.ignoreStat=false",
                "-c",
                "core.ignoreCase=false",
                "-c",
                "core.precomposeUnicode=false",
                "-c",
                "core.checkStat=default",
                "-c",
                "core.trustctime=true",
                "-c",
                "core.symlinks=true",
                "status",
                "--porcelain",
                "--untracked-files=all",
            ])
            .output()
            .map_err(|error| format!("cannot run git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git status failed in {}: {}",
                target.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    if !status.trim().is_empty() {
        let first = status.lines().next().unwrap_or_default().to_string();
        return Err(format!("ownerless clone is not clean: {first}"));
    }
    // HANDOFF digest binding for a root-level HANDOFF.md, if present.
    let handoff_path = target.join("HANDOFF.md");
    if handoff_path.is_file() && !handoff_path.is_symlink() {
        let contents = std::fs::read(&handoff_path)
            .map_err(|error| format!("cannot read ownerless clone HANDOFF.md: {error}"))?;
        let observed = format!("{:x}", Sha256::digest(&contents));
        if !handoff_sha256s.iter().any(|digest| digest == &observed) {
            return Err(format!(
                "ownerless clone contains unread HANDOFF.md; read it and rerun with --handoff-sha256 {observed}"
            ));
        }
    }
    let _ = discard_ignored;
    if apply {
        return Err(
            "ownerless agent clone recovery stays with the Python edition for now".to_string(),
        );
    }
    Err(
        "the ownerless clone inventory and its digest-bound plan stay with the Python edition for now"
            .to_string(),
    )
}
