//! Writer: remove and archive one slot (`remove`).
//!
//! Reproduces the ordinary coordinator-authorized removal: process
//! authorization, partial-state/journal/storage guards, hold and
//! heartbeat/owner checks, the registered-liveness answer, handoff
//! artifact/read checks, salvage of already-published clean
//! checkouts, reclaim evidence, the FINISH journal and path fence,
//! physical worktree removal, and the archive + slot-removed state
//! updates. Corners that stay with the Python edition for now and
//! refuse explicitly here: host-context removal (runner/proof
//! environment), validate slots, owner consent and attestation,
//! salvage needing a push or local bundle (dirty, unpublished, or
//! drifted work), cache directories with configured globs (nested
//! repository judging), legacy HANDOFF.md quarantine, and slot
//! images. Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory, Cfg};
use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::finish_write::{
    assert_record_paths, assert_slot_contents, cache_globs_for, fetch_remote, record_layout,
    remote_authority, stored_repository_path,
};
use crate::global_state::load_event_chain;
use crate::handoff_state::{
    load_sidecar_artifact, read_regular_file_identity, sidecar_path, validate_sidecar,
    SIDECAR_BYTES_LIMIT,
};
use crate::heartbeat_write::{assert_caller, release_in_effect};
use crate::hold_write::{
    acquire_mutation_locks, active_document, current_hold, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

fn identity_of(value: &Value) -> Identity {
    Identity {
        pid: value.get("pid").and_then(Value::as_i64).unwrap_or(0),
        start_ticks: value
            .get("start_ticks")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        boot_id: value
            .get("boot_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        host_id: value
            .get("host_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        cgroup_path: value
            .get("cgroup_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-replace-objects",
            "-c",
            "core.useReplaceRefs=false",
            "-C",
        ])
        .arg(repo)
        .args(args)
        .env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with("GIT_") {
            command.env(key, value);
        }
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git refused in {}: git {}: {}",
            repo.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn heartbeat_expired(record: &Value) -> Result<(bool, bool, i64)> {
    let heartbeat_at = record
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let stamp = crate::schema::parse_timestamp_instant(
        heartbeat_at,
        &format!(
            "slot {} heartbeat",
            record
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
    )
    .map_err(|error| error.to_string())?;
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let age = ((now_millis - stamp.timestamp_millis()) as f64 / 1000.0).max(0.0);
    let ttl = record
        .get("heartbeat_ttl_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Ok((true, age > ttl, age as i64))
}

pub(crate) struct Liveness {
    pub(crate) state: String,
    pub(crate) detail: String,
}

pub(crate) fn liveness_answer(cfg: &Cfg, config: &Value, record: &Value) -> Result<Liveness> {
    // The registered-liveness protocol: exit 0 is dead; exit 1 is alive
    // only when the whole output is one line binding agent=<agent> and
    // rc=1; exit 2 is unregistered only for the bound
    // reason=no-agentctl-record line; anything else is unverifiable.
    // The command runs with the exact one-subject environment.
    if config
        .get("liveness_batch_command")
        .is_some_and(|value| !value.is_null())
    {
        return Ok(Liveness {
            state: "unverifiable".to_string(),
            detail: "batch liveness authority stays with the Python edition for now".to_string(),
        });
    }
    let Some(raw) = config.get("liveness_command").and_then(Value::as_str) else {
        return Ok(Liveness {
            state: "unverifiable".to_string(),
            detail: "config has no liveness command".to_string(),
        });
    };
    let agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let command_path = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        cfg.root.join(raw)
    };
    let mut command = if raw.ends_with(".py") {
        let mut command = Command::new("python3");
        command.arg(&command_path).arg(agent);
        command
    } else {
        let mut command = Command::new(&command_path);
        command.arg(agent);
        command
    };
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let slot_path = crate::active_state::slot_directory(cfg, slot, slot_type)
        .unwrap_or_else(|_| cfg.root.join(slot));
    let owner = record.get("owner").cloned().unwrap_or(Value::Null);
    let owner_field = |key: &str| -> String {
        match owner.get(key) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            _ => String::new(),
        }
    };
    command
        .current_dir(&cfg.root)
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/bin:/bin")
        .env(
            "WRKSLOTS_PROJECT_ROOT",
            cfg.root.to_string_lossy().into_owned(),
        )
        .env("WRKSLOTS_SLOT", slot)
        .env("WRKSLOTS_SLOT_TYPE", slot_type)
        .env(
            "WRKSLOTS_SLOT_PATH",
            slot_path.to_string_lossy().into_owned(),
        )
        .env("WRKSLOTS_AGENT", agent)
        .env(
            "WRKSLOTS_TASK",
            record
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .env(
            "WRKSLOTS_MACHINE",
            record
                .get("machine")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .env(
            "WRKSLOTS_GENERATION",
            record
                .get("generation")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .to_string(),
        )
        .env("WRKSLOTS_OWNER_PID", owner_field("pid"))
        .env("WRKSLOTS_OWNER_START_TICKS", owner_field("start_ticks"))
        .env("WRKSLOTS_OWNER_BOOT_ID", owner_field("boot_id"))
        .env("WRKSLOTS_OWNER_CGROUP", owner_field("cgroup_path"));
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return Ok(Liveness {
                state: "unverifiable".to_string(),
                detail: format!("registered liveness command could not run: {error}"),
            });
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined_lines: Vec<String> = format!("{stdout}\n{stderr}")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .collect();
    let first = combined_lines
        .first()
        .cloned()
        .unwrap_or_else(|| "no detail".to_string());
    let bound = |rc: &str, reason: Option<&str>| -> Option<String> {
        if combined_lines.len() != 1 {
            return None;
        }
        let tokens: Vec<&str> = combined_lines[0].split_whitespace().collect();
        let agents: Vec<&str> = tokens
            .iter()
            .filter(|token| token.starts_with("agent="))
            .cloned()
            .collect();
        if agents != vec![format!("agent={agent}").as_str()] {
            return None;
        }
        let rcs: Vec<&str> = tokens
            .iter()
            .filter(|token| token.starts_with("rc="))
            .cloned()
            .collect();
        if rcs != vec![rc] {
            return None;
        }
        if let Some(reason) = reason {
            let reasons: Vec<&str> = tokens
                .iter()
                .filter(|token| token.starts_with("reason="))
                .cloned()
                .collect();
            if reasons != vec![reason] {
                return None;
            }
        }
        Some(combined_lines[0].clone())
    };
    let (state, detail) = match output.status.code() {
        Some(0) => ("dead", first),
        Some(1) => match bound("rc=1", None) {
            Some(line) => ("alive", line),
            None => (
                "unverifiable",
                "registered liveness command exited 1 without a single output line binding the agent to rc=1"
                    .to_string(),
            ),
        },
        Some(2) => match bound("rc=2", Some("reason=no-agentctl-record")) {
            Some(line) => ("unregistered", line),
            None => ("unverifiable", first),
        },
        other => (
            "unverifiable",
            format!(
                "registered liveness command returned unexpected rc {}: {first}",
                other.unwrap_or(-1)
            ),
        ),
    };
    Ok(Liveness {
        state: state.to_string(),
        detail,
    })
}

fn own_ancestry() -> BTreeSet<i64> {
    let mut result = BTreeSet::new();
    let mut current = std::process::id() as i64;
    loop {
        result.insert(current);
        let Ok(raw) = std::fs::read(format!("/proc/{current}/stat")) else {
            break;
        };
        let text = String::from_utf8_lossy(&raw).into_owned();
        let Some(closing) = text.rfind(") ") else {
            break;
        };
        let Some(ppid) = text[closing + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<i64>().ok())
        else {
            break;
        };
        if ppid <= 0 || result.contains(&ppid) {
            break;
        }
        current = ppid;
    }
    result
}

fn link_points_into(link: &Path, slot_path: &Path, slot_device: u64) -> Result<bool> {
    // Mirrors the Python census (#618): a readlink failure is proven
    // foreign only when stat (which follows the descriptor anyway)
    // lands on a different filesystem device; a same-device target
    // stays indeterminate rather than failing open.
    use std::os::unix::fs::MetadataExt as _;
    match std::fs::read_link(link) {
        Ok(target) => {
            let shown = target.to_string_lossy().into_owned();
            let target = PathBuf::from(shown.trim_end_matches(" (deleted)"));
            Ok(target == slot_path || target.starts_with(slot_path))
        }
        Err(error) => {
            // Errnos the Python census treats as no evidence.
            if matches!(error.raw_os_error(), Some(1 | 2 | 3 | 13 | 22)) {
                return Ok(false);
            }
            match std::fs::metadata(link) {
                Ok(metadata) => {
                    if metadata.dev() != slot_device {
                        Ok(false)
                    } else {
                        Err(format!(
                            "process use is indeterminate because {} failed: {error}",
                            link.display()
                        ))
                    }
                }
                Err(_) => Err(format!(
                    "process use is indeterminate because {} failed: {error}",
                    link.display()
                )),
            }
        }
    }
}

fn assert_slot_unused(slot_path: &Path, coordinator_pid: i64) -> Result<()> {
    let cwd = std::env::current_dir()
        .map_err(|error| format!("cannot establish the current working directory: {error}"))?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    if cwd == slot_path || cwd.starts_with(slot_path) {
        return Err(format!(
            "the wrkslots process is running from inside slot {}; change directory and retry",
            slot_path.display()
        ));
    }
    let slot_device = {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(slot_path)
            .map(|metadata| metadata.dev())
            .map_err(|error| {
                format!(
                    "process use is indeterminate because the slot cannot be inspected: {}: {error}",
                    slot_path.display()
                )
            })?
    };
    let ignored = own_ancestry();
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| format!("process use is indeterminate because /proc failed: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("process use is indeterminate because /proc failed: {error}")
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(pid) = name.parse::<i64>() else {
            continue;
        };
        if ignored.contains(&pid) || pid == coordinator_pid {
            continue;
        }
        let base = PathBuf::from("/proc").join(&name);
        let mut via: Option<String> = None;
        for (label, link) in [
            ("working directory", base.join("cwd")),
            ("executable", base.join("exe")),
        ] {
            if link_points_into(&link, slot_path, slot_device)? {
                via = Some(label.to_string());
                break;
            }
        }
        if via.is_none() {
            if let Ok(fds) = std::fs::read_dir(base.join("fd")) {
                for fd in fds.flatten() {
                    if link_points_into(&fd.path(), slot_path, slot_device)? {
                        via = Some("descriptor".to_string());
                        break;
                    }
                }
            }
        }
        if let Some(via) = via {
            return Err(format!(
                "live process {pid} uses slot {} ({via}); stop it and retry",
                slot_path.display()
            ));
        }
    }
    Ok(())
}

fn repair_registration(repository: &Path, checkout_path: &Path) -> Result<()> {
    let git_file = checkout_path.join(".git");
    let metadata = std::fs::symlink_metadata(&git_file).map_err(|error| {
        format!(
            "checkout has no safe .git file to repair: {}: {error}",
            git_file.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "checkout has no safe .git file to repair: {}",
            git_file.display()
        ));
    }
    let raw = std::fs::read_to_string(&git_file).map_err(|error| {
        format!(
            "cannot read checkout .git file {}: {error}",
            git_file.display()
        )
    })?;
    let line = raw.strip_suffix('\n').unwrap_or(&raw);
    let Some(named) = line.strip_prefix("gitdir: ") else {
        return Err(format!(
            "checkout has an invalid .git file: {}",
            git_file.display()
        ));
    };
    if line.contains('\n') || line.contains('\0') {
        return Err(format!(
            "checkout has an invalid .git file: {}",
            git_file.display()
        ));
    }
    let named_path = PathBuf::from(named);
    let admin = if named_path.is_absolute() {
        named_path
    } else {
        checkout_path.join(&named_path)
    };
    let admin = admin.canonicalize().map_err(|error| {
        format!(
            "checkout .git file names an unavailable Git directory: {}: {error}",
            git_file.display()
        )
    })?;
    let common_out = git(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(common_out.trim());
    let common = if common.is_absolute() {
        common
    } else {
        repository.join(&common)
    };
    let common = common.canonicalize().unwrap_or(common);
    if admin.parent() != Some(common.join("worktrees").as_path()) || !admin.is_dir() {
        return Err(format!(
            "checkout .git file {} names {}, which is not a linked-worktree directory of {}; refusing to repair it",
            git_file.display(),
            admin.display(),
            repository.display()
        ));
    }
    let commondir = admin.join("commondir");
    if commondir.exists() || commondir.is_symlink() {
        let named_common = std::fs::read_to_string(&commondir)
            .map_err(|error| format!("cannot read linked-worktree commondir: {error}"))?;
        let resolved = admin
            .join(named_common.trim())
            .canonicalize()
            .unwrap_or_default();
        if resolved != common {
            return Err(format!(
                "linked-worktree directory {} belongs to another repository; refusing to repair it",
                admin.display()
            ));
        }
    }
    let desired = format!(
        "{}\n",
        git_file
            .canonicalize()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| git_file.to_string_lossy().into_owned())
    );
    let link = admin.join("gitdir");
    if link.is_symlink() {
        return Err(format!(
            "linked-worktree gitdir is a symlink; refusing to repair it: {}",
            link.display()
        ));
    }
    let current = std::fs::read_to_string(&link).unwrap_or_default();
    if current != desired {
        // Atomic replace beside the destination; never write through
        // a symlink or truncate the existing file in place.
        let temporary = admin.join(format!(".gitdir.wrkslots-{}", operation_id()?));
        std::fs::write(&temporary, &desired)
            .map_err(|error| format!("cannot repair linked-worktree gitdir: {error}"))?;
        if let Err(error) = std::fs::rename(&temporary, &link) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("cannot repair linked-worktree gitdir: {error}"));
        }
    }
    let listing = git(repository, &["worktree", "list", "--porcelain"])?;
    let absolute = checkout_path
        .canonicalize()
        .unwrap_or_else(|_| checkout_path.to_path_buf());
    if !listing.lines().any(|line| {
        line.strip_prefix("worktree ")
            .map(|path| {
                *path.trim() == *absolute.to_string_lossy()
                    || Path::new(path.trim()) == checkout_path
            })
            .unwrap_or(false)
    }) {
        return Err(format!(
            "Git did not repair worktree registration for {}",
            checkout_path.display()
        ));
    }
    Ok(())
}

fn salvage_checkout(cfg: &Cfg, checkout: &Value, repository: &Path) -> Result<Value> {
    // Salvage safety: the reclaim identity checks plus graft/shallow
    // and index-flag checks, then publication = the HEAD is contained
    // in at least one remote ref after a scratch-namespace fetch.
    // Unlike finish, salvage needs neither landed-ref ancestry nor
    // an ignored-file-free status (the caller checked cleanliness
    // without --ignored).
    let name = checkout
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let updated = reclaim_checkout(cfg, checkout, repository)?;
    let raw_path = checkout
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = cfg.root.join(raw_path);
    let common_out = git(&path, &["rev-parse", "--git-common-dir"])?;
    let common = {
        let raw = common_out.trim();
        if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            path.join(raw)
        }
    };
    for marker in ["info/grafts", "shallow"] {
        let candidate = common.join(marker);
        if candidate.exists() || candidate.is_symlink() {
            return Err(format!(
                "checkout {name} uses graft or shallow history: {}",
                candidate.display()
            ));
        }
    }
    let assume = git(&path, &["ls-files", "-v"])?;
    if assume.lines().any(|line| {
        line.chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
    }) {
        return Err(format!("checkout {name} has assume-unchanged index state"));
    }
    let skip = git(&path, &["ls-files", "-t"])?;
    if skip.lines().any(|line| line.starts_with("S ")) {
        return Err(format!("checkout {name} has skip-worktree index state"));
    }
    let head = updated
        .get("head")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let remote = checkout
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let landed_ref = checkout
        .get("landed_ref")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (authority_sha, url) = remote_authority(&path, &remote)?;
    fetch_remote(&path, &remote, &landed_ref, &url)?;
    let (after_sha, _after_url) = remote_authority(&path, &remote)?;
    if after_sha != authority_sha {
        return Err(format!(
            "remote {remote:?} changed after authority was captured; preserve the checkout and restore its recorded remote"
        ));
    }
    let refs_output = git(
        &path,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "--contains",
            &head,
            &format!("refs/remotes/{remote}/"),
        ],
    )?;
    let remote_refs: Vec<String> = refs_output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if remote_refs.is_empty() {
        return Err(format!(
            "checkout {name} HEAD {head} is not contained in any {remote} ref"
        ));
    }
    let mut result = updated;
    if let Some(object) = result.as_object_mut() {
        object.insert("containing_remote_refs".to_string(), json!(remote_refs));
    }
    Ok(result)
}

/// Whether `git submodule status` output names an initialized submodule. A line starting with
/// `-` is a submodule that was never initialized: an empty directory with nothing to salvage,
/// as the Python edition's `submodule foreach` (initialized only) also treats it.
fn has_initialized_submodule(status: &str) -> bool {
    status
        .lines()
        .any(|line| !line.trim().is_empty() && !line.starts_with('-'))
}

fn salvage_gate(cfg: &Cfg, config: &Value, checkout: &Value, repository: &Path) -> Result<Value> {
    let name = checkout
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let raw_path = checkout
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = cfg.root.join(raw_path);
    // Cleanliness by salvage semantics: tracked and untracked entries,
    // ignored files do not dirty a checkout.
    let globs = cache_globs_for(config, &name);
    if !globs.is_empty() {
        return Err(format!(
            "configured cache policy for checkout {name} needs nested-repository judging; remove stays with the Python edition for now"
        ));
    }
    let status = git(
        &path,
        &[
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )?;
    if !status.is_empty() {
        return Err(format!(
            "checkout {name} has uncommitted work; salvage stays with the Python edition for now"
        ));
    }
    let submodules = git(&path, &["submodule", "status", "--recursive"])?;
    if has_initialized_submodule(&submodules) {
        return Err(format!(
            "checkout {name} has initialized submodules; submodule salvage stays with the Python edition for now"
        ));
    }
    let updated = salvage_checkout(cfg, checkout, repository).map_err(|error| {
        format!(
            "checkout {name} is not fully published; salvage stays with the Python edition for now: {error}"
        )
    })?;
    let head = updated
        .get("head")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(json!({
        "checkout": name,
        "source_head": head,
        "salvage_commit": head,
        "status_sha256": format!("{:x}", Sha256::digest(b"")),
        "disposition": "already-published",
        "remote_ref": Value::Null,
        "containing_remote_refs": updated
            .get("containing_remote_refs")
            .cloned()
            .unwrap_or(json!([])),
    }))
}

fn reclaim_checkout(cfg: &Cfg, checkout: &Value, repository: &Path) -> Result<Value> {
    // The reclaim refresh: worktree identity, branch, unfinished
    // operations, ordinary history and index, and the recorded remote
    // URL digest. Containing refs stay as recorded; only HEAD moves.
    let name = checkout
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let raw_path = checkout
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = cfg.root.join(raw_path);
    let top = git(&path, &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim()) != path {
        return Err(format!(
            "checkout path is not its Git worktree root: {}",
            path.display()
        ));
    }
    let head_out = git(&path, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head = head_out.trim().to_string();
    if head.len() != 40 || !head.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!(
            "checkout HEAD is not a full commit: {}",
            path.display()
        ));
    }
    let branch_out = git(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = branch_out.trim();
    let recorded_branch = checkout
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if branch != recorded_branch {
        return Err(format!(
            "checkout {name} branch changed: expected {recorded_branch}, found {branch}; preserve it and update the record before reclaim"
        ));
    }
    for state_name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
    ] {
        let out = git(&path, &["rev-parse", "--git-path", state_name])?;
        let candidate = {
            let raw = out.trim();
            if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                path.join(raw)
            }
        };
        if candidate.exists() || candidate.is_symlink() {
            return Err(format!(
                "checkout {name} has an unfinished Git operation at {}; complete or abort it before reclaim",
                candidate.display()
            ));
        }
    }
    let replacements = git(
        &path,
        &["for-each-ref", "--format=%(refname)", "refs/replace/"],
    )?;
    if !replacements.trim().is_empty() {
        return Err(format!(
            "checkout contains replacement refs: {}",
            replacements.lines().next().unwrap_or_default()
        ));
    }
    let remote = checkout
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url_out = git(&path, &["remote", "get-url", "--all", remote])?;
    let urls: Vec<&str> = url_out.lines().collect();
    if urls.len() != 1 {
        return Err(format!(
            "remote {remote:?} must have exactly one non-empty fetch URL"
        ));
    }
    let digest = format!("{:x}", Sha256::digest(urls[0].as_bytes()));
    if digest
        != checkout
            .get("remote_url_sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
    {
        return Err(format!(
            "checkout {name} remote {remote} URL changed; restore the recorded remote before reclaim"
        ));
    }
    let _ = repository;
    let mut updated = checkout.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("head".to_string(), json!(head));
    }
    Ok(updated)
}

fn latest_handoff_read(events: &[Value], record: &Value) -> Result<Option<Value>> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut latest: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) != Some(slot)
            || payload.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            continue;
        }
        let digest = payload
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if digest.len() != 64 || !digest.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')) {
            return Err("handoff-read event sha256 is not a SHA-256 digest".to_string());
        }
        let source = payload.get("source").cloned().unwrap_or(Value::Null);
        let source_path = payload.get("source_path").cloned().unwrap_or(Value::Null);
        let source_identity = payload
            .get("source_identity")
            .cloned()
            .unwrap_or(Value::Null);
        let present = [
            !source.is_null(),
            !source_path.is_null(),
            !source_identity.is_null(),
        ];
        if present.iter().any(|flag| *flag) && !present.iter().all(|flag| *flag) {
            return Err("handoff-read event has incomplete source provenance".to_string());
        }
        if present[0] {
            let source_name = source.as_str().unwrap_or_default();
            if !matches!(source_name, "write-handoff" | "legacy-slot-file") {
                return Err("handoff-read event source is invalid".to_string());
            }
            if source_identity.get("sha256").and_then(Value::as_str) != Some(digest) {
                return Err("handoff-read source digest does not match its content".to_string());
            }
        }
        latest = Some(json!({
            "sha256": digest,
            "source": source,
            "source_path": source_path,
            "source_identity": source_identity,
            "sequence": event.get("sequence").cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(latest)
}

fn write_journal(cfg: &Cfg, selected: &str, journal: &Value) -> Result<()> {
    ensure_event_log(None, cfg, selected)?;
    append_event(
        cfg,
        selected,
        "operation-progress-recorded",
        json!({
            "slot": journal.get("slot").cloned().unwrap_or(Value::Null),
            "operation": journal.get("kind").cloned().unwrap_or(Value::Null),
            "journal_path": format!("ACTIVE.{selected}.journal"),
            "journal": journal,
        }),
    )?;
    atomic_write_json(
        &cfg.control.join(format!("ACTIVE.{selected}.journal")),
        journal,
    )
}

fn clear_journal(cfg: &Cfg, selected: &str, journal: &Value) -> Result<()> {
    ensure_event_log(None, cfg, selected)?;
    append_event(
        cfg,
        selected,
        "operation-completed",
        json!({
            "slot": journal.get("slot").cloned().unwrap_or(Value::Null),
            "operation": journal.get("kind").cloned().unwrap_or(Value::Null),
            "journal_path": format!("ACTIVE.{selected}.journal"),
        }),
    )?;
    let path = cfg.control.join(format!("ACTIVE.{selected}.journal"));
    if path.exists() || path.is_symlink() {
        std::fs::remove_file(&path).map_err(|error| {
            format!("cannot clear operation journal {}: {error}", path.display())
        })?;
    }
    Ok(())
}

fn operation_id() -> Result<String> {
    use std::io::Read as _;
    let mut bytes = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("cannot draw a removal fence id: {error}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The publication gate the oracle's `_resolve_handoff_artifact`
/// applies before a sidecar may act as removal authority: a
/// write-handoff sidecar must byte-match a completed owner-authenticated
/// write intent; a legacy projection must not supersede an intent and
/// needs its own provenance-bearing read.
fn assert_sidecar_publication(
    events: &[Value],
    slot: &str,
    generation: i64,
    artifact: &Value,
) -> Result<()> {
    let publication = crate::handoff_write::publication(events, slot, generation);
    if artifact.get("source").and_then(Value::as_str) == Some("write-handoff") {
        match publication {
            Some((_sequence, intended, completed)) if intended == *artifact => {
                if !completed {
                    return Err(format!(
                        "slot {slot} handoff publication is incomplete; preserve the sidecar and have its exact owner retry write-handoff"
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "slot {slot} handoff sidecar has no matching owner-authenticated publication intent; preserve it for manual review"
                ));
            }
        }
        return Ok(());
    }
    if let Some((_sequence, _intended, completed)) = publication {
        let state = if completed {
            "completed"
        } else {
            "outstanding"
        };
        return Err(format!(
            "slot {slot} has a {state} owner-authenticated write intent; legacy sidecar recovery cannot supersede it"
        ));
    }
    let matched = events.iter().any(|event| {
        if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
            return false;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
            && payload.get("sha256") == artifact.get("sha256")
            && payload.get("contents_utf8") == artifact.get("contents_utf8")
            && payload.get("source").and_then(Value::as_str) == Some("legacy-slot-file")
            && payload.get("source_path") == artifact.get("source_path")
            && payload.get("source_identity") == artifact.get("source_identity")
    });
    if !matched {
        return Err(format!(
            "slot {slot} projected legacy sidecar has no matching provenance-bearing read; preserve it for manual review"
        ));
    }
    Ok(())
}

fn read_matches_artifact(read: &Value, artifact: &Value) -> bool {
    read.get("sha256") == artifact.get("sha256")
        && read.get("source") == artifact.get("source")
        && read.get("source_path") == artifact.get("source_path")
        && read.get("source_identity") == artifact.get("source_identity")
}

/// Move an archived slot's read handoff sidecar to durable
/// identity-bound retired storage, mirroring the Python
/// `_remove_handoff_sidecar_after_archive`. Idempotent: an existing
/// retired artifact is validated against the recorded read digest.
#[allow(clippy::too_many_arguments)]
fn retire_handoff_sidecar(
    cfg: &Cfg,
    selected: &str,
    slot: &str,
    slot_type: &str,
    generation: i64,
    recorded_digest: Option<&str>,
) -> Result<()> {
    let sidecar = sidecar_path(&cfg.control, selected, slot, generation);
    let name = sidecar
        .file_name()
        .map(|item| item.to_string_lossy().into_owned())
        .unwrap_or_default();
    let digest = format!("{:x}", Sha256::digest(name.as_bytes()));
    let prefix = format!("HANDOFF-RETIRED.{}.", &digest[..16]);
    let mut retired_paths: Vec<PathBuf> = std::fs::read_dir(&cfg.control)
        .map_err(|error| format!("cannot list control directory: {error}"))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|item| item.to_string_lossy().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    retired_paths.sort();
    if retired_paths.len() > 1 {
        return Err(format!(
            "multiple retired handoff sidecars exist for slot {slot}"
        ));
    }
    if let Some(retired) = retired_paths.into_iter().next() {
        if sidecar.exists() || sidecar.is_symlink() {
            return Err(format!(
                "handoff sidecar was replaced during archived cleanup for slot {slot}; preserve both {} and {}",
                sidecar.display(),
                retired.display()
            ));
        }
        let retired_name = retired
            .file_name()
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parts: Vec<&str> = retired_name[prefix.len()..].split('.').collect();
        let lower_hex = |text: &str| {
            !text.is_empty() && text.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f'))
        };
        let parsed = (parts.len() == 5
            && parts[4] == "json"
            && lower_hex(parts[0])
            && lower_hex(parts[1])
            && parts[2].len() == 64
            && lower_hex(parts[2])
            && parts[3].len() == 64
            && lower_hex(parts[3]))
        .then(|| {
            (
                u64::from_str_radix(parts[0], 16).ok(),
                u64::from_str_radix(parts[1], 16).ok(),
                parts[2].to_string(),
                parts[3].to_string(),
            )
        });
        let Some((Some(device), Some(inode), name_digest, name_file_hash)) = parsed else {
            return Err(format!(
                "invalid retired handoff sidecar: {}",
                retired.display()
            ));
        };
        let (contents, identity) =
            read_regular_file_identity(&retired, "retired handoff sidecar", SIDECAR_BYTES_LIMIT)?;
        if identity.device != device
            || identity.inode != inode
            || recorded_digest != Some(name_digest.as_str())
            || identity.sha256 != name_file_hash
        {
            return Err(format!(
                "retired handoff sidecar identity changed: {}",
                retired.display()
            ));
        }
        let artifact = validate_sidecar(
            cfg, selected, slot, slot_type, generation, &sidecar, &contents, identity,
        )?;
        if recorded_digest != artifact.value.get("sha256").and_then(Value::as_str) {
            return Err(format!(
                "retired handoff sidecar does not match the exact read for slot {slot}; preserve it and run 'wrkslots recover'"
            ));
        }
        return Ok(());
    }
    let Some(artifact) = load_sidecar_artifact(cfg, selected, slot, slot_type, generation)? else {
        return Ok(());
    };
    let artifact_sha = artifact
        .value
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if recorded_digest != Some(artifact_sha.as_str()) {
        return Err(format!(
            "handoff sidecar changed before archived cleanup for slot {slot}; preserve it and run 'wrkslots recover'"
        ));
    }
    let identity = &artifact.storage_identity;
    let retired = cfg.control.join(format!(
        "{prefix}{:x}.{:x}.{}.{}.json",
        identity.device, identity.inode, artifact_sha, identity.sha256
    ));
    if retired.exists() || retired.is_symlink() {
        return Err(format!(
            "retired handoff destination already exists for slot {slot}: {}",
            retired.display()
        ));
    }
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        &sidecar,
        rustix::fs::CWD,
        &retired,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        format!(
            "cannot quarantine handoff sidecar {}: {error}",
            sidecar.display()
        )
    })?;
    let directory = std::fs::File::open(&cfg.control)
        .map_err(|error| format!("cannot fsync directory {}: {error}", cfg.control.display()))?;
    directory
        .sync_all()
        .map_err(|error| format!("cannot fsync directory {}: {error}", cfg.control.display()))?;
    let (contents, retired_identity) =
        read_regular_file_identity(&retired, "retired handoff sidecar", SIDECAR_BYTES_LIMIT)?;
    let retired_artifact = validate_sidecar(
        cfg,
        selected,
        slot,
        slot_type,
        generation,
        &sidecar,
        &contents,
        retired_identity,
    )?;
    if retired_artifact.storage_identity.device != identity.device
        || retired_artifact.storage_identity.inode != identity.inode
        || retired_artifact.storage_identity.size != identity.size
        || retired_artifact.storage_identity.sha256 != identity.sha256
        || retired_artifact.value.get("sha256").and_then(Value::as_str)
            != Some(artifact_sha.as_str())
    {
        return Err(format!(
            "retired handoff sidecar identity changed at its archive boundary for slot {slot}; preserved it for inspection"
        ));
    }
    if sidecar.exists() || sidecar.is_symlink() {
        return Err(format!(
            "handoff sidecar pathname was replaced during cleanup for slot {slot}; preserved both identities for recovery"
        ));
    }
    Ok(())
}

/// Remove and archive the named slot.
pub(crate) fn remove_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    coordinator_pid: i64,
    expected_generation: i64,
) -> Result<Value> {
    require_fixture()?;
    let host_context = [
        "WRKSLOTS_REMOVE_RUNNER_PID",
        "WRKSLOTS_REMOVE_COORDINATOR_START_TICKS",
        "WRKSLOTS_REMOVE_PROOF_FD",
    ]
    .iter()
    .filter(|name| std::env::var_os(name).is_some())
    .count();
    if host_context == 3 {
        return Err("host-context removal stays with the Python edition for now".to_string());
    }
    if host_context != 0 {
        return Err(
            "host-context removal requires runner PID, coordinator generation, and proof FD"
                .to_string(),
        );
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    let journals = parse_journals(root)?;
    if let Some(first) = journals
        .get("journals")
        .and_then(Value::as_array)
        .and_then(|names| names.first())
        .and_then(Value::as_str)
    {
        return Err(format!(
            "interrupted mutation recorded in {}; run 'wrkslots recover' first",
            cfg.control.join(first).display()
        ));
    }
    validate_global(root, &cfg)?;
    let active = active_document(root, &cfg, &selected)?;
    let archives_path = cfg.control.join(format!("ARCHIVED.{selected}.json"));
    let archives_before: Value = if archives_path.exists() {
        serde_json::from_str(
            &std::fs::read_to_string(&archives_path)
                .map_err(|error| format!("cannot read archive state: {error}"))?,
        )
        .map_err(|error| format!("cannot read archive state: {error}"))?
    } else {
        json!({"schema": 2, "machine": selected, "revision": 0, "records": []})
    };
    let record = active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!(
            "stale generation for slot {slot}: expected {expected_generation}, current {generation}"
        ));
    }
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    if slot_type != "agent" {
        return Err(format!(
            "remove for slot type '{slot_type}' stays with the Python edition for now"
        ));
    }
    // Re-verify the coordinator generation under the mutation locks.
    let coordinator_now = read_process_identity(coordinator_pid).map_err(|_| {
        format!("coordinator PID {coordinator_pid} exited before the locked removal boundary")
    })?;
    if coordinator_now != coordinator {
        return Err(format!(
            "coordinator PID {coordinator_pid} generation changed before the locked removal boundary"
        ));
    }
    assert_caller(&coordinator, "coordinator")?;
    refuse_target_storage(root, slot)?;
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), &selected)?;
    {
        let hold = current_hold(root, &cfg, &selected, slot)?;
        if !hold.is_null() {
            return Err(format!(
                "slot {slot} is held: {}",
                hold.get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ));
        }
    }
    let (_present, expired, heartbeat_age) = heartbeat_expired(&record)?;
    let owner_value = record.get("owner").cloned().unwrap_or(Value::Null);
    let owner_absent = owner_value.is_null();
    let owner_identity: Option<Identity> = if owner_absent {
        None
    } else {
        Some(identity_of(&owner_value))
    };
    let (owner_state, owner_detail) = process_state(owner_identity.as_ref());
    let owner_gone = owner_absent || owner_state == "dead";
    let release = release_in_effect(&cfg, &record, &events)?;
    let agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let answer = liveness_answer(&cfg, &config, &record)?;
    if release.is_none() {
        // Both authorities must agree the owner is gone: the exact
        // recorded process generation dead (or no owner recorded) AND
        // the registered liveness answer "dead", with the heartbeat
        // TTL expired. Every other combination refuses.
        if owner_state == "live" {
            return Err(
                "owner consent and attested removal stay with the Python edition for now"
                    .to_string(),
            );
        }
        if answer.state == "alive" {
            return Err(format!(
                "registered liveness authority reports owner alive: {}; let the owner finish or stop, then rerun remove after the time-to-live expires",
                answer.detail
            ));
        }
        if answer.state != "dead" {
            if answer.state == "unregistered" && owner_state == "dead" && !owner_absent {
                return Err(
                    "coordinator-attested removal of an unregistered agent stays with the Python edition for now"
                        .to_string(),
                );
            }
            return Err(format!(
                "registered liveness authority is {}: {}; unknown ownership is not a free slot",
                answer.state, answer.detail
            ));
        }
        if owner_state != "dead" && !owner_absent {
            return Err(format!(
                "remove requires a proven-dead recorded owner; owner is {owner_state}: {owner_detail}"
            ));
        }
        if !expired {
            return Err(format!(
                "heartbeat for slot {slot} has not expired; wait for its TTL or have the owner stop cleanly"
            ));
        }
    }
    let _ = owner_gone;
    let _ = agent;
    let slot_path = slot_directory(&cfg, slot, &slot_type)?;
    assert_slot_contents(&cfg, &record)?;
    // Handoff artifact gates: no uncommitted handoff, and a present
    // artifact must have been read with matching provenance.
    let sidecar = load_sidecar_artifact(&cfg, &selected, slot, &slot_type, generation)?;
    let legacy_handoff = slot_path.join("HANDOFF.md");
    if legacy_handoff.exists() || legacy_handoff.is_symlink() {
        return Err(
            "legacy HANDOFF.md quarantine stays with the Python edition for now".to_string(),
        );
    }
    let latest_read = latest_handoff_read(&events, &record)?;
    if sidecar.is_none() {
        if let Some((_sequence, _intended, completed)) =
            crate::handoff_write::publication(&events, slot, generation)
        {
            let (state, remedy) = if completed {
                ("completed", "restore the exact completed sidecar")
            } else {
                (
                    "outstanding",
                    "have the exact owner retry write-handoff or recover its authenticated temp",
                )
            };
            return Err(format!(
                "slot {slot} has a {state} owner-authenticated handoff write intent but no canonical sidecar; preserve the slot and {remedy}"
            ));
        }
    }
    if let Some(artifact) = &sidecar {
        assert_sidecar_publication(&events, slot, generation, &artifact.value)?;
    }
    match (&sidecar, &latest_read) {
        (Some(artifact), Some(read)) if read_matches_artifact(read, &artifact.value) => {}
        (Some(_), _) => {
            return Err(format!(
                "slot {slot} contains an unread HANDOFF.md or handoff sidecar. state: REFUSED -- no checkout was salvaged or removed. remedy: run 'wrkslots read-handoff {slot} --coordinator-authorized --coordinator-pid <current-coordinator-pid>' and read its output, then retry remove"
            ));
        }
        (None, Some(_)) => {
            return Err(format!(
                "slot {slot} handoff artifact disappeared after it was read; restore the exact generation-bound sidecar or legacy HANDOFF.md"
            ));
        }
        (None, None) => {}
    }
    // Salvage gate: every checkout already published and clean.
    let mut salvage: Vec<Value> = Vec::new();
    for checkout in record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_stored, repository) = stored_repository_path(&cfg, repository_raw)?;
        salvage.push(salvage_gate(&cfg, &config, &checkout, &repository)?);
    }
    if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
        for checkout in checkouts {
            let moved = salvage
                .iter()
                .find(|item| {
                    item.get("checkout").and_then(Value::as_str)
                        == checkout.get("name").and_then(Value::as_str)
                })
                .cloned()
                .unwrap_or(Value::Null);
            let _ = moved;
        }
    }
    // Use check and registry consistency before any event is written.
    assert_slot_unused(&slot_path, coordinator_pid)?;
    ensure_event_log(root, &cfg, &selected)?;
    let coordinator_obj = json!({
        "pid": coordinator.pid,
        "start_ticks": coordinator.start_ticks,
        "boot_id": coordinator.boot_id,
        "host_id": coordinator.host_id,
        "cgroup_path": coordinator.cgroup_path,
    });
    append_event(
        &cfg,
        &selected,
        "reclaim-started",
        json!({
            "slot": slot,
            "generation": generation,
            "actor": coordinator_obj,
            "runner": coordinator_obj,
            "handoff_writer": Value::Null,
            "coordinator_authorized": true,
            "owner_state": owner_state,
            "registered_liveness": answer.state,
            "heartbeat_age_seconds": heartbeat_age,
            "heartbeat_ttl_seconds": record
                .get("heartbeat_ttl_seconds")
                .cloned()
                .unwrap_or(json!(0)),
            "validate_complete": false,
            "live_validate_owner": false,
            "salvage_archive_root": Value::Null,
        }),
    )?;
    // Evidence handoff when finish never recorded one.
    let mut current = record.clone();
    if current
        .get("handoff")
        .cloned()
        .unwrap_or(Value::Null)
        .is_null()
    {
        let validation: Vec<String> = salvage
            .iter()
            .map(|item| {
                format!(
                    "checkout {}: already-published commit {} on an existing remote ref",
                    item.get("checkout")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    item.get("salvage_commit")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
            .collect();
        let continuation = format!(
            "wrkslots remove {slot} --coordinator-pid <current-coordinator-pid> --expected-generation {generation}"
        );
        if let Some(object) = current.as_object_mut() {
            object.insert(
                "handoff".to_string(),
                json!({
                    "recorded_at": utc_now(),
                    "validation": validation,
                    "limitations": [],
                    "continuation": continuation,
                }),
            );
        }
        let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
        let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
        append_event(
            &cfg,
            &selected,
            "active-state-recorded",
            json!({
                "action": "reclaim-evidence-recorded",
                "slot": slot,
                "previous_revision": previous_revision,
                "revision": previous_revision + 1,
                "previous_record_sha256": previous_digest,
                "record": current,
                "evidence": {"salvage": salvage},
            }),
        )?;
        let mut slots: Vec<Value> = active
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in slots.iter_mut() {
            if item.get("slot").and_then(Value::as_str) == Some(slot) {
                *item = current.clone();
            }
        }
        let mut document = active.clone();
        if let Some(object) = document.as_object_mut() {
            object.insert("revision".to_string(), json!(previous_revision + 1));
            object.insert("slots".to_string(), json!(slots));
        }
        atomic_write_json(
            &cfg.control.join(format!("ACTIVE.{selected}.json")),
            &document,
        )?;
    }
    // Reclaim preconditions: refreshed checkout evidence before fencing.
    assert_record_paths(&cfg, &current)?;
    assert_slot_contents(&cfg, &current)?;
    let mut final_checkouts: Vec<Value> = Vec::new();
    for checkout in current
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_stored, repository) = stored_repository_path(&cfg, repository_raw)?;
        final_checkouts.push(reclaim_checkout(&cfg, &checkout, &repository)?);
    }
    // Salvage evidence must still match after the refresh.
    for item in &salvage {
        let name = item
            .get("checkout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let refreshed_item = final_checkouts
            .iter()
            .find(|checkout| checkout.get("name").and_then(Value::as_str) == Some(name))
            .cloned()
            .unwrap_or(Value::Null);
        if refreshed_item.get("head").and_then(Value::as_str)
            != item.get("source_head").and_then(Value::as_str)
        {
            return Err(format!(
                "checkout {name} salvage evidence no longer matches; preserve it and retry"
            ));
        }
    }
    let mut refreshed = current.clone();
    if let Some(object) = refreshed.as_object_mut() {
        object.insert("checkouts".to_string(), json!(final_checkouts));
    }
    {
        let state_now = active_document(root, &cfg, &selected)?;
        let previous_revision = state_now
            .get("revision")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let previous_record = state_now
            .get("slots")
            .and_then(Value::as_array)
            .and_then(|slots| {
                slots
                    .iter()
                    .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
            })
            .cloned()
            .unwrap_or(Value::Null);
        let previous_digest =
            canonical_sha256(&previous_record).map_err(|error| error.to_string())?;
        append_event(
            &cfg,
            &selected,
            "active-state-recorded",
            json!({
                "action": "checkout-evidence-refreshed",
                "slot": slot,
                "previous_revision": previous_revision,
                "revision": previous_revision + 1,
                "previous_record_sha256": previous_digest,
                "record": refreshed,
                "evidence": {},
            }),
        )?;
        let mut slots: Vec<Value> = state_now
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in slots.iter_mut() {
            if item.get("slot").and_then(Value::as_str) == Some(slot) {
                *item = refreshed.clone();
            }
        }
        let mut document = state_now.clone();
        if let Some(object) = document.as_object_mut() {
            object.insert("revision".to_string(), json!(previous_revision + 1));
            object.insert("slots".to_string(), json!(slots));
        }
        atomic_write_json(
            &cfg.control.join(format!("ACTIVE.{selected}.json")),
            &document,
        )?;
    }
    let current = refreshed;
    // FINISH journal (prepared), then the path fence.
    let finished_at = utc_now();
    let archive_id = format!("{selected}:{slot}:{generation}:{finished_at}");
    let layout = record_layout(&cfg, &current)?;
    let mode = std::fs::symlink_metadata(&slot_path)
        .map(|metadata| {
            use std::os::unix::fs::MetadataExt as _;
            metadata.mode() & 0o7777
        })
        .map_err(|error| {
            format!(
                "cannot inspect slot permissions before fencing: {}: {error}",
                slot_path.display()
            )
        })?;
    let fenced_name = format!(".{slot}.fenced.{generation}.{}", operation_id()?);
    let fenced_relative = slot_path
        .parent()
        .map(|parent| {
            parent
                .join(&fenced_name)
                .strip_prefix(&cfg.root)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| fenced_name.clone())
        })
        .unwrap_or_else(|| fenced_name.clone());
    let mut journal = json!({
        "schema": 2,
        "kind": "finish",
        "machine": selected,
        "slot": slot,
        "mode": "remove",
        "actor": "coordinator",
        "finished_at": finished_at,
        "archive_id": archive_id,
        "phase": "prepared",
        "fenced": fenced_relative,
        "original_mode": mode,
        "removed": [],
        "salvage": salvage,
        "validate_complete": false,
        "record": current,
    });
    write_journal(&cfg, &selected, &journal)?;
    let fenced_path = cfg.root.join(
        journal
            .get("fenced")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if slot_path.exists() || slot_path.is_symlink() {
        if fenced_path.exists() || fenced_path.is_symlink() {
            return Err(format!(
                "both canonical and fenced slot paths exist for {slot}"
            ));
        }
        std::fs::rename(&slot_path, &fenced_path).map_err(|error| {
            format!(
                "cannot establish path fence {} -> {}: {error}",
                slot_path.display(),
                fenced_path.display()
            )
        })?;
    }
    if !fenced_path.is_dir() || fenced_path.is_symlink() {
        return Err(format!(
            "fenced slot is missing or unsafe: {}",
            fenced_path.display()
        ));
    }
    if slot_path.exists() || slot_path.is_symlink() {
        return Err(format!(
            "canonical slot still exists after path fence: {}",
            slot_path.display()
        ));
    }
    if let Some(object) = journal.as_object_mut() {
        object.insert("phase".to_string(), json!("fenced"));
    }
    write_journal(&cfg, &selected, &journal)?;
    // Contents re-check at the fenced path.
    {
        let expected: BTreeSet<String> = current
            .get("checkouts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if layout == "nested" {
            let actual: BTreeSet<String> = std::fs::read_dir(&fenced_path)
                .map_err(|error| {
                    format!(
                        "cannot inspect fenced slot {}: {error}",
                        fenced_path.display()
                    )
                })?
                .filter_map(|entry| {
                    entry
                        .ok()
                        .map(|item| item.file_name().to_string_lossy().into_owned())
                })
                .collect();
            if actual != expected {
                return Err(format!(
                    "fenced slot contents changed: expected {expected:?}, found {actual:?}"
                ));
            }
        }
    }
    assert_slot_unused(&fenced_path, coordinator_pid)?;
    // Physical removal per checkout, journaling each step.
    let mut removed: Vec<String> = Vec::new();
    for checkout in current
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let name = checkout
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_stored, repository) = stored_repository_path(&cfg, repository_raw)?;
        let moved_path = if layout == "flat" {
            fenced_path.clone()
        } else {
            fenced_path.join(&name)
        };
        if moved_path.is_symlink() {
            return Err(format!(
                "interrupted fenced checkout became a symlink: {}",
                moved_path.display()
            ));
        }
        repair_registration(&repository, &moved_path)?;
        git(
            &repository,
            &[
                "worktree",
                "remove",
                "--force",
                "--",
                &moved_path.to_string_lossy(),
            ],
        )?;
        removed.push(name);
        if let Some(object) = journal.as_object_mut() {
            object.insert("removed".to_string(), json!(removed));
        }
        write_journal(&cfg, &selected, &journal)?;
    }
    if fenced_path.exists() {
        std::fs::remove_dir_all(&fenced_path).map_err(|error| {
            format!(
                "cannot remove fenced slot directory {}: {error}",
                fenced_path.display()
            )
        })?;
    }
    if let Some(object) = journal.as_object_mut() {
        object.insert("phase".to_string(), json!("removed"));
    }
    write_journal(&cfg, &selected, &journal)?;
    // State update: archive first, then delete the active record.
    let handoff = current.get("handoff").cloned().unwrap_or(Value::Null);
    let mut entry = json!({
        "archive_id": archive_id,
        "slot": slot,
        "agent": current.get("agent").cloned().unwrap_or(Value::Null),
        "task": current.get("task").cloned().unwrap_or(Value::Null),
        "purpose": current.get("purpose").cloned().unwrap_or(Value::Null),
        "slot_type": slot_type,
        "machine": selected,
        "generation": generation,
        "created_at": current.get("created_at").cloned().unwrap_or(Value::Null),
        "finished_at": finished_at,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": handoff.get("validation").cloned().unwrap_or(json!([])),
        "limitations": handoff.get("limitations").cloned().unwrap_or(json!([])),
        "continuation": handoff.get("continuation").cloned().unwrap_or(Value::Null),
        "salvage": salvage,
        "checkouts": current.get("checkouts").cloned().unwrap_or(json!([])),
    });
    if let Some(layout_value) = current.get("layout").cloned() {
        if let Some(object) = entry.as_object_mut() {
            object.insert("layout".to_string(), layout_value);
        }
    }
    if let Some(source) = current.get("import_source").cloned() {
        if let Some(object) = entry.as_object_mut() {
            object.insert("import_source".to_string(), source);
        }
    }
    let archive_records = archives_before
        .get("records")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if archive_records
        .iter()
        .any(|item| item.get("archive_id").and_then(Value::as_str) == Some(archive_id.as_str()))
    {
        return Err(format!(
            "archive_id {archive_id} already exists with different content"
        ));
    }
    let previous_archive_revision = archives_before
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    append_event(
        &cfg,
        &selected,
        "archive-state-recorded",
        json!({
            "action": "slot-archived",
            "slot": slot,
            "previous_revision": previous_archive_revision,
            "revision": previous_archive_revision + 1,
            "record": entry,
            "evidence": {"archive_id": archive_id},
        }),
    )?;
    let mut updated_records = archive_records.clone();
    updated_records.push(entry.clone());
    let mut archive_document = archives_before.clone();
    if let Some(object) = archive_document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_archive_revision + 1));
        object.insert("records".to_string(), json!(updated_records));
    }
    atomic_write_json(&archives_path, &archive_document)?;
    // Delete the active record.
    let state_now = active_document(root, &cfg, &selected)?;
    let previous_revision = state_now
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let previous_record = state_now
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let previous_digest = canonical_sha256(&previous_record).map_err(|error| error.to_string())?;
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "slot-removed",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": Value::Null,
            "evidence": {"archive_id": archive_id},
        }),
    )?;
    let remaining: Vec<Value> = state_now
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("slot").and_then(Value::as_str) != Some(slot))
        .collect();
    let mut document = state_now.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(remaining));
    }
    atomic_write_json(
        &cfg.control.join(format!("ACTIVE.{selected}.json")),
        &document,
    )?;
    retire_handoff_sidecar(
        &cfg,
        &selected,
        slot,
        &slot_type,
        generation,
        latest_read
            .as_ref()
            .and_then(|read| read.get("sha256"))
            .and_then(Value::as_str),
    )?;
    clear_journal(&cfg, &selected, &journal)?;
    validate_global(root, &cfg)?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "outcome": "removed and archived",
        "archive_id": archive_id,
    }))
}

#[cfg(test)]
mod link_tests {
    use super::link_points_into;

    #[test]
    fn link_points_into_cases() {
        let base = std::env::temp_dir().join(format!("wrkslots-link-{}", std::process::id()));
        let slot = base.join("slot");
        let outside = base.join("outside");
        std::fs::create_dir_all(&slot).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let device = {
            use std::os::unix::fs::MetadataExt as _;
            std::fs::metadata(&slot).unwrap().dev()
        };
        let into = base.join("into");
        std::os::unix::fs::symlink(&slot, &into).unwrap();
        assert!(link_points_into(&into, &slot, device).unwrap());
        let away = base.join("away");
        std::os::unix::fs::symlink(&outside, &away).unwrap();
        assert!(!link_points_into(&away, &slot, device).unwrap());
        assert!(!link_points_into(&base.join("missing"), &slot, device).unwrap());
        // A regular file is not a link: EINVAL is no evidence.
        let regular = base.join("regular");
        std::fs::write(&regular, b"x").unwrap();
        assert!(!link_points_into(&regular, &slot, device).unwrap());
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[cfg(test)]
mod submodule_tests {
    use super::has_initialized_submodule;

    #[test]
    fn uninitialized_submodules_do_not_block_removal() {
        assert!(!has_initialized_submodule(""));
        assert!(!has_initialized_submodule(
            "-56714bf4884cdc74b4a0b9d9e2a7b8427bd24fd7 ext/minibeads\n"
        ));
        assert!(has_initialized_submodule(
            " 56714bf4884cdc74b4a0b9d9e2a7b8427bd24fd7 ext/minibeads (v0.29.0)\n"
        ));
        assert!(has_initialized_submodule(
            "-1111111111111111111111111111111111111111 a\n+2222222222222222222222222222222222222222 b (heads/main)\n"
        ));
        assert!(has_initialized_submodule(
            "U3333333333333333333333333333333333333333 c\n"
        ));
    }
}
