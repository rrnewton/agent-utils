//! Writer: remove and archive one slot (`remove`).
//!
//! Reproduces the ordinary coordinator-authorized removal: process
//! authorization, partial-state/journal/storage guards, hold and
//! heartbeat/owner checks, the registered-liveness answer, handoff
//! artifact/read checks, salvage of already-published clean
//! checkouts, reclaim evidence, the FINISH journal and path fence,
//! physical worktree removal, and the archive + slot-removed state
//! updates. Gitignored files outside the regenerable caches refuse
//! removal by default; `--archive-ignored` keeps them in a verified
//! deterministic tar beside the salvage bundles and
//! `--discard-ignored` deletes them explicitly, both listed with
//! size and sha256 in the salvage receipt (#1302). Corners that stay
//! with the Python edition for now and
//! refuse explicitly here: host-context removal (runner/proof
//! environment), validate slots, owner consent and attestation,
//! salvage needing a push or local bundle (dirty, unpublished, or
//! drifted work), nested Git repositories inside configured cache
//! directories (their judging and salvage evidence), legacy
//! HANDOFF.md quarantine, and slot images. Fixture registries only
//! until the edition cutover (see `ensure_states::require_fixture`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
use crate::proc_reads;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

/// The mount-table entry mounted exactly at `path`, mirroring the
/// Python `_mount_entry_at` (#1318).
fn mount_entry_at(path: &Path) -> Option<crate::image_mounts::MountEntry> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let entries = crate::image_mounts::parse_mountinfo(&text);
    crate::image_mounts::top_entry(path, &entries).cloned()
}

/// The mount ID of the mount at `path` in this namespace, mirroring
/// Python `mount_id_at` for the bind record written after a relocation.
fn mount_id_at(path: &Path) -> Option<i64> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let target = crate::image_mounts::normalize_absolute(path);
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 5
            && crate::image_mounts::normalize_absolute(Path::new(fields[4])) == target
        {
            if let Ok(id) = fields[0].parse::<i64>() {
                return Some(id);
            }
        }
    }
    None
}

/// Run one privileged mount command, mirroring Python
/// `worktreesimage._run(argv, sudo=True)` (`sudo -n` + argv).
fn run_mount_command(argv: &[String]) -> Result<()> {
    let output = Command::new("sudo")
        .arg("-n")
        .args(argv)
        .output()
        .map_err(|error| format!("{} failed to run: {error}", argv.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    Err(format!("sudo -n {} failed: {detail}", argv.join(" ")))
}

/// Establish the path fence, mirroring Python `_rename_slot_path`
/// (#1318): a mount point cannot be renamed, so a recorded worktrees
/// bind — or an unrecorded worktrees-image subvolume mount, recognised
/// by its mount root — is unmounted, the empty mount point renamed, and
/// the subvolume bound again at the fenced path. Any other mount point
/// fails closed here, naming the mount. Failures keep the existing
/// `cannot establish path fence` refusal shape.
fn fence_slot_path(cfg: &Cfg, slot: &str, slot_path: &Path, fenced_path: &Path) -> Result<()> {
    let records = crate::worktrees_image::load_bind_records(&cfg.control)
        .map_err(|error| format!("cannot read worktrees image record: {error}"))?;
    let entry = mount_entry_at(slot_path);
    let mut bind_key: Option<String> = None;
    if records.contains_key(slot) {
        bind_key = Some(slot.to_string());
    } else if let Some(mount) = &entry {
        let subvolume = mount.root.strip_prefix('/').unwrap_or_default();
        if !subvolume.is_empty()
            && !subvolume.contains('/')
            && crate::worktrees_image::image_mount_dir(&cfg.control)
                .join(subvolume)
                .is_dir()
        {
            bind_key = Some(subvolume.to_string());
        }
    }
    if let Some(key) = bind_key {
        let mount_dir = crate::worktrees_image::image_mount_dir(&cfg.control);
        let subvolume = records
            .get(&key)
            .map(|record| record.subvolume.clone())
            .unwrap_or_else(|| key.clone());
        if !records.contains_key(&key) {
            let source = slot_path.to_string_lossy().into_owned();
            let record = crate::worktrees_image::BindRecord {
                subvolume: subvolume.clone(),
                mount_id: None,
                target: Some(source),
            };
            let record_key = key.clone();
            crate::worktrees_image::update_bind_records(&cfg.control, move |mut current| {
                current.entry(record_key).or_insert(record);
                current
            })
            .map_err(|error| {
                format!(
                    "cannot unmount image mount at {} before the path fence: {error}",
                    slot_path.display()
                )
            })?;
        }
        run_mount_command(&[
            "umount".to_string(),
            slot_path.to_string_lossy().into_owned(),
        ])
        .map_err(|error| {
            format!(
                "cannot unmount image mount at {} before the path fence: {error}",
                slot_path.display()
            )
        })?;
        if let Err(error) = std::fs::rename(slot_path, fenced_path) {
            let _ = run_mount_command(&[
                "mount".to_string(),
                "--bind".to_string(),
                mount_dir.join(&subvolume).to_string_lossy().into_owned(),
                slot_path.to_string_lossy().into_owned(),
            ]);
            return Err(error.to_string());
        }
        run_mount_command(&[
            "mount".to_string(),
            "--bind".to_string(),
            mount_dir.join(&subvolume).to_string_lossy().into_owned(),
            fenced_path.to_string_lossy().into_owned(),
        ])
        .map_err(|error| {
            format!(
                "cannot unmount image mount at {} before the path fence: {error}",
                slot_path.display()
            )
        })?;
        let target = fenced_path.to_string_lossy().into_owned();
        let mount_id = mount_id_at(fenced_path);
        crate::worktrees_image::update_bind_records(&cfg.control, move |mut current| {
            if let Some(record) = current.get_mut(&key) {
                record.mount_id = mount_id;
                record.target = Some(target);
            }
            current
        })?;
        return Ok(());
    }
    if let Some(mount) = entry {
        return Err(format!(
            "cannot fence mounted path {}: it is a mount point ({}); unmount it or record it as an image target before removal",
            slot_path.display(),
            mount.source
        ));
    }
    std::fs::rename(slot_path, fenced_path).map_err(|error| error.to_string())
}

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
        namespace_pid: crate::status_state::optional_namespace_pid(value),
        pidns_inode: crate::status_state::optional_pidns_inode(value),
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let (code, stdout, stderr) = git_unchecked(repo, args)?;
    if code != Some(0) {
        return Err(format!(
            "Git refused in {}: git {}: {}",
            repo.display(),
            args.join(" "),
            stderr.trim()
        ));
    }
    Ok(stdout)
}

/// Run Git as `git` does and return its exit code, stdout and stderr,
/// whatever the exit code.
fn git_unchecked(repo: &Path, args: &[&str]) -> Result<(Option<i32>, String, String)> {
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
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// More refs than this go to a sidecar file in the Python edition (#1256).
const REF_INVENTORY_INLINE_LIMIT: usize = 64;

/// The refs removal deletes from one checkout, as the Python edition's
/// `_ref_inventory` records them (#1256): `scope` is `repository` when the
/// checkout's Git directory is its common directory (every ref listed) and
/// `worktree` for a linked worktree (only refs/worktree/, refs/bisect/ and
/// refs/rewritten/); `head` is the ref HEAD names, or null when detached;
/// `sha256` digests the lines "OBJECT REFNAME\n" in ref-name order. A Git
/// failure is recorded as `error`, never a refusal; an inventory too large
/// to list inline needs the sidecar file only the Python edition writes.
fn ref_inventory(path: &Path, name: &str) -> Result<Value> {
    match ref_inventory_fields(path) {
        Ok(inventory) => {
            let count = inventory
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            if count > REF_INVENTORY_INLINE_LIMIT as u64 {
                return Err(format!(
                    "checkout {name} has {count} refs to inventory; the ref inventory sidecar stays with the Python edition for now"
                ));
            }
            Ok(inventory)
        }
        Err(error) => Ok(json!({ "error": error })),
    }
}

fn ref_inventory_fields(path: &Path) -> Result<Value> {
    let git_directory = PathBuf::from(git(path, &["rev-parse", "--absolute-git-dir"])?.trim());
    let common_raw = git(path, &["rev-parse", "--git-common-dir"])?;
    let common = {
        let raw = common_raw.trim();
        if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            path.join(raw)
        }
    };
    let resolve = |candidate: &Path| {
        std::fs::canonicalize(candidate)
            .map_err(|error| format!("cannot resolve {}: {error}", candidate.display()))
    };
    let scope = if resolve(&git_directory)? == resolve(&common)? {
        "repository"
    } else {
        "worktree"
    };
    let (code, stdout, stderr) = git_unchecked(path, &["symbolic-ref", "--quiet", "HEAD"])?;
    let head = match code {
        Some(0) => Value::String(stdout.trim().to_string()),
        Some(1) => Value::Null,
        _ => return Err(format!("cannot read HEAD: {}", stderr.trim())),
    };
    let mut arguments = vec!["for-each-ref", "--format=%(objectname) %(refname)"];
    if scope == "worktree" {
        arguments.extend(["refs/worktree/", "refs/bisect/", "refs/rewritten/"]);
    }
    let listing = git(path, &arguments)?;
    let mut refs: Vec<(String, String)> = Vec::new();
    for line in listing.lines() {
        let (object, refname) = line.split_once(' ').unwrap_or((line, ""));
        // A full SHA-1 object name, as the Python edition's SHA_RE accepts.
        let full_object = object.len() == 40
            && object
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !full_object || refname.is_empty() {
            return Err(format!("Git listed an unexpected ref line: {line:?}"));
        }
        refs.push((refname.to_string(), object.to_string()));
    }
    refs.sort();
    let text: String = refs
        .iter()
        .map(|(refname, object)| format!("{object} {refname}\n"))
        .collect();
    Ok(json!({
        "scope": scope,
        "head": head,
        "count": refs.len(),
        "sha256": format!("{:x}", Sha256::digest(text.as_bytes())),
        "refs": refs
            .iter()
            .map(|(refname, object)| json!([refname, object]))
            .collect::<Vec<Value>>(),
    }))
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

/// The Python edition's liveness detail line: standard error when it is not
/// empty, else standard output, stripped, and its first line as Python's
/// `splitlines` splits; "no detail" when nothing is left.
fn python_first_line(stdout: &str, stderr: &str) -> String {
    let text = if stderr.is_empty() { stdout } else { stderr };
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .split(|c: char| {
            matches!(
                c,
                '\n' | '\r'
                    | '\u{0b}'
                    | '\u{0c}'
                    | '\u{1c}'
                    | '\u{1d}'
                    | '\u{1e}'
                    | '\u{85}'
                    | '\u{2028}'
                    | '\u{2029}'
            )
        })
        .next()
        .filter(|line| !line.is_empty())
        .unwrap_or("no detail")
        .to_string()
}

/// The action that records an agent removal's dead liveness line (#994); see
/// the Python edition's `REGISTERED_LIVENESS_ACTION`.
const REGISTERED_LIVENESS_ACTION: &str = "registered-liveness-recorded";
const LIVENESS_DETAIL_LIMIT: usize = 1024;

/// The evidence of [`REGISTERED_LIVENESS_ACTION`]: the line escaped to one
/// line and capped at [`LIVENESS_DETAIL_LIMIT`] bytes, never parsed.
fn registered_liveness_evidence(generation: i64, agent: &str, detail: &str) -> Value {
    let mut escaped = String::new();
    for character in detail.chars() {
        let code = character as u32;
        if code < 0x20 || code == 0x7f {
            escaped.push_str(&format!("\\x{code:02x}"));
        } else {
            escaped.push(character);
        }
    }
    let truncated = escaped.len() > LIVENESS_DETAIL_LIMIT;
    if truncated {
        let mut end = LIVENESS_DETAIL_LIMIT;
        while !escaped.is_char_boundary(end) {
            end -= 1;
        }
        escaped.truncate(end);
    }
    json!({
        "generation": generation,
        "agent": agent,
        "registered_liveness": "dead",
        "detail": escaped,
        "detail_truncated": truncated,
    })
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
        // The dead line is recorded in the history (#994), so it is the
        // line the Python edition records.
        Some(0) => ("dead", python_first_line(&stdout, &stderr)),
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
    if state == "unverifiable" {
        if let Some(reanchored) = reanchor_liveness(record) {
            return Ok(reanchored);
        }
    }
    Ok(Liveness {
        state: state.to_string(),
        detail,
    })
}

/// Classify a re-registered pane from agentctl's re-anchor record (#777):
/// the LAST `reanchors` entry in `agents/<name>/agent.json` whose slot
/// and slot_generation match, with a live `current.pid`, is alive.
fn reanchor_liveness(record: &Value) -> Option<Liveness> {
    let agent = record.get("agent").and_then(Value::as_str)?;
    let slot = record.get("slot").and_then(Value::as_str)?;
    let generation = record.get("generation").and_then(Value::as_i64)?;
    let base = match std::env::var_os("WRKSLOTS_AGENTCTL_AGENTS_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/state/agentctl/agents"),
    };
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(base.join(agent).join("agent.json")).ok()?)
            .ok()?;
    let entries = document.get("reanchors").and_then(Value::as_array)?;
    let created_epoch = record
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|moment| moment.timestamp());
    let mut matched: Option<&Value> = None;
    for entry in entries {
        if entry.get("slot").and_then(Value::as_str) != Some(slot) {
            continue;
        }
        match entry.get("slot_generation") {
            Some(Value::Number(number)) => {
                if number.as_i64() != Some(generation) {
                    continue;
                }
            }
            Some(Value::Null) | None => {
                let Some(at) = entry.get("at").and_then(Value::as_f64) else {
                    continue;
                };
                if Some(at as i64) <= created_epoch {
                    continue;
                }
            }
            _ => continue,
        }
        matched = Some(entry);
    }
    let entry = matched?;
    let current = entry.get("current")?;
    let pid = current.get("pid").and_then(Value::as_i64)?;
    crate::status_state::read_process_identity(pid).ok()?;
    Some(Liveness {
        state: "alive".to_string(),
        detail: format!(
            "re-registered: agent re-anchored to pane {} pid {} at {} ({}); bound to the re-anchored process",
            current.get("pane_id").and_then(Value::as_str).unwrap_or_default(),
            pid,
            entry.get("at").cloned().unwrap_or(Value::Null),
            entry.get("reason").and_then(Value::as_str).unwrap_or_default()
        ),
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

/// A live process whose argv or environ names `slot_path` (#993),
/// mirroring the oracle's target-naming match: `(pid, kind, token)`.
fn process_naming_slot(slot_path: &Path) -> Option<(i64, String, String)> {
    let needle = slot_path.to_string_lossy().into_owned();
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut pids: Vec<i64> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i64>().ok())
        .collect();
    pids.sort_unstable();
    for pid in pids {
        if pid == std::process::id() as i64 {
            continue;
        }
        for (kind, name) in [("argv", "cmdline"), ("environ", "environ")] {
            let Ok(raw) = std::fs::read(PathBuf::from("/proc").join(pid.to_string()).join(name))
            else {
                continue;
            };
            for token in raw.split(|byte| *byte == 0) {
                if token.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(token).into_owned();
                let value = text.split('=').nth(1).unwrap_or(&text);
                let hit = value
                    .split(':')
                    .any(|part| part == needle || part.starts_with(&format!("{needle}/")));
                if hit {
                    return Some((pid, kind.to_string(), text));
                }
            }
        }
    }
    None
}

/// One process link as observed by a shared process snapshot.
enum LinkObservation {
    Target(PathBuf),
    NoEvidence,
    Undecided { error: String, device: Option<u64> },
}

struct ProcessLinks {
    pid: i64,
    /// Set when `pid` is a surviving thread read in place of this
    /// leader, which exited while its threads still run.
    leader: Option<i64>,
    links: Vec<(&'static str, LinkObservation)>,
}

impl ProcessLinks {
    /// How a refusal names the holder, as the Python census does.
    fn holder(&self) -> String {
        match self.leader {
            None => format!("process {}", self.pid),
            Some(leader) => format!(
                "thread {} of process {leader} (whose main thread has exited)",
                self.pid
            ),
        }
    }
}

/// One enumeration of `/proc` link targets, captured once and
/// consulted for every slot a caller checks (the audit's shared
/// process census). Capture itself makes no slot decision.
pub(crate) struct ProcessSnapshot {
    processes: Vec<ProcessLinks>,
}

/// Read one /proc link. A link whose process exited (ENOENT or ESRCH)
/// is no evidence and is added to `gone` (#1173 census-vanished-process).
fn observe_link(link: &Path, reader: &mut EntryReader<'_>) -> LinkObservation {
    use std::os::unix::fs::MetadataExt as _;
    match proc_reads::read_link(link) {
        Ok(target) => {
            let shown = target.to_string_lossy().into_owned();
            LinkObservation::Target(PathBuf::from(shown.trim_end_matches(" (deleted)")))
        }
        Err(error) => {
            if proc_reads::is_gone(&error) {
                reader.gone.push((link.to_path_buf(), error));
                return LinkObservation::NoEvidence;
            }
            if matches!(error.raw_os_error(), Some(1 | 13)) {
                return reader.unreadable(link, error);
            }
            // Not a link: no evidence, as in the Python census.
            if error.raw_os_error() == Some(22) {
                return LinkObservation::NoEvidence;
            }
            LinkObservation::Undecided {
                error: format!(
                    "process use is indeterminate because {} failed: {error}",
                    link.display()
                ),
                device: std::fs::metadata(link).ok().map(|metadata| metadata.dev()),
            }
        }
    }
}

fn observation_points_into(
    observation: &LinkObservation,
    slot_path: &Path,
    slot_device: u64,
) -> Result<bool> {
    // Mirrors the Python census (#618): a readlink failure is proven
    // foreign only when stat (which follows the descriptor anyway)
    // lands on a different filesystem device; a same-device target
    // stays indeterminate rather than failing open.
    match observation {
        LinkObservation::Target(target) => Ok(target == slot_path || target.starts_with(slot_path)),
        LinkObservation::NoEvidence => Ok(false),
        LinkObservation::Undecided { error, device } => {
            if device.is_some_and(|device| device != slot_device) {
                Ok(false)
            } else {
                Err(error.clone())
            }
        }
    }
}

#[cfg(test)]
fn link_points_into(link: &Path, slot_path: &Path, slot_device: u64) -> Result<bool> {
    let wait = proc_reads::ExitWait::new();
    let mut reader = EntryReader::new("test link", link, &wait);
    observation_points_into(&observe_link(link, &mut reader), slot_path, slot_device)
}

/// One process's census reads: where they are taken, the reads that
/// found it exited (for the census diagnostics), and how an entry this
/// user may not read is answered.
struct EntryReader<'a> {
    holder: String,
    base: &'a Path,
    owned: Option<bool>,
    gone: Vec<(PathBuf, std::io::Error)>,
    wait: &'a proc_reads::ExitWait,
}

impl<'a> EntryReader<'a> {
    fn new(holder: &str, base: &'a Path, wait: &'a proc_reads::ExitWait) -> Self {
        EntryReader {
            holder: holder.to_string(),
            base,
            owned: None,
            gone: Vec::new(),
            wait,
        }
    }

    /// An entry (link, fd directory or maps) failing with EACCES or
    /// EPERM (#1174, the lead's ruling). A process of this user's uid is
    /// refused to it only once do_exit dropped its mappings, so it is
    /// followed as #1173 follows it: gone or a zombie, at once or within
    /// the snapshot's shared exit wait, uses nothing; still exiting after
    /// that is indeterminate and refuses. A protected (non-dumpable or setuid)
    /// process that is not exiting, or another user's process, is no
    /// evidence, as Python's remove census (lsof's rule) takes it.
    /// Python's audit census reads such a process as root (sudo -n), which
    /// this snapshot cannot: a documented gap.
    fn unreadable(&mut self, path: &Path, error: std::io::Error) -> LinkObservation {
        let owned = *self
            .owned
            .get_or_insert_with(|| proc_reads::owned_by_invoker(self.base));
        if !owned {
            return LinkObservation::NoEvidence;
        }
        let state =
            proc_reads::unreadable_entry_state(&self.holder, self.base, path, &error, self.wait);
        match state {
            proc_reads::UnreadableEntry::Exited => {
                self.gone.push((path.to_path_buf(), error));
                LinkObservation::NoEvidence
            }
            proc_reads::UnreadableEntry::Running => LinkObservation::NoEvidence,
            proc_reads::UnreadableEntry::StillExiting { .. } => LinkObservation::Undecided {
                error: proc_reads::unreadable_refusal(&self.holder, path, &error, &state),
                device: None,
            },
        }
    }
}

/// Most listings of /proc one snapshot takes before it refuses
/// (#1102 wrkslots-census-churn-tolerant). Each listing that is not
/// proven settles at least one more target, so a snapshot of N
/// targets closes within N + 2 listings unless processes keep
/// changing faster than it can read them.
const SNAPSHOT_LISTING_ROUNDS: usize = 64;

/// A process generation's start time (clock ticks after boot) and
/// scheduler state, read from `base`/stat, or None once it has exited.
fn process_stat(base: &Path) -> Option<(u64, char)> {
    let raw = std::fs::read(base.join("stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let mut fields = text[closing + 2..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    let start = fields.nth(18)?.parse().ok()?;
    Some((start, state))
}

fn is_terminal_state(state: char) -> bool {
    matches!(state, 'Z' | 'X')
}

/// The threads, with their start times, that still run in `pid`'s
/// thread group after its leader exited (State Z or X) (#1102 part 2).
/// Such a leader's own entries show nothing and /proc readdir lists
/// none of its threads, yet the threads keep its working directory,
/// descriptors and executable: the process is live for slot use, so
/// each thread is read in its place at /proc/PID/task/TID. A leader
/// that still runs, or a group that is gone, has none. A thread
/// listing that cannot be read refuses, as in the Python census.
fn surviving_threads(proc_root: &Path, pid: i64) -> Result<Vec<(i64, u64)>> {
    let base = proc_root.join(pid.to_string());
    match process_stat(&base) {
        Some((_start, state)) if is_terminal_state(state) => {}
        _ => return Ok(Vec::new()),
    }
    let task = base.join("task");
    let entries = match std::fs::read_dir(&task) {
        Ok(entries) => entries,
        Err(error) if matches!(error.raw_os_error(), Some(2 | 3)) => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "process use is indeterminate because {} failed: {error}",
                task.display()
            ));
        }
    };
    let mut threads = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "process use is indeterminate because {} failed: {error}",
                task.display()
            )
        })?;
        let Some(tid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i64>().ok())
        else {
            continue;
        };
        if tid == pid {
            continue;
        }
        if let Some((start, state)) = process_stat(&entry.path()) {
            if !is_terminal_state(state) {
                threads.push((tid, start));
            }
        }
    }
    Ok(threads)
}

fn list_proc(proc_root: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(proc_root).map_err(|error| {
        format!(
            "process use is indeterminate because {} failed: {error}",
            proc_root.display()
        )
    })?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "process use is indeterminate because {} failed: {error}",
                proc_root.display()
            )
        })?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// Read one process (or a surviving thread of `leader`). Every process
/// one snapshot reads shares its `wait`, so their exits share one
/// deadline (#1174).
fn read_process_links(
    proc_root: &Path,
    pid: i64,
    leader: Option<i64>,
    wait: &proc_reads::ExitWait,
) -> ProcessLinks {
    let base = match leader {
        None => proc_root.join(pid.to_string()),
        Some(leader) => proc_root
            .join(leader.to_string())
            .join("task")
            .join(pid.to_string()),
    };
    let holder = ProcessLinks {
        pid,
        leader,
        links: Vec::new(),
    }
    .holder();
    let mut reader = EntryReader::new(&holder, &base, wait);
    let mut links: Vec<(&'static str, LinkObservation)> = Vec::new();
    for (label, link) in [
        ("working directory", base.join("cwd")),
        ("root directory", base.join("root")),
        ("executable", base.join("exe")),
    ] {
        links.push((label, observe_link(&link, &mut reader)));
    }
    let fd_dir = base.join("fd");
    match proc_reads::read_dir(&fd_dir) {
        Ok(fds) => {
            for fd in fds.flatten() {
                links.push(("descriptor", observe_link(&fd.path(), &mut reader)));
            }
        }
        Err(error) if proc_reads::is_gone(&error) => reader.gone.push((fd_dir, error)),
        Err(error) if matches!(error.raw_os_error(), Some(1 | 13)) => {
            links.push(("descriptor", reader.unreadable(&fd_dir, error)));
        }
        Err(_) => {}
    }
    // Mapped files, as the Python census reads them (#1174
    // rust-remove-census-root-maps): a file mapped from the slot, even one
    // deleted since or whose descriptor was closed, uses it.
    let maps_path = base.join("maps");
    match proc_reads::read_bytes(&maps_path) {
        Ok(maps) => {
            for mapped in mapped_paths(&maps) {
                links.push(("mapping", LinkObservation::Target(mapped)));
            }
        }
        Err(error) if proc_reads::is_gone(&error) => reader.gone.push((maps_path, error)),
        Err(error) if matches!(error.raw_os_error(), Some(1 | 13)) => {
            links.push(("mapping", reader.unreadable(&maps_path, error)));
        }
        Err(error) => links.push((
            "mapping",
            LinkObservation::Undecided {
                error: format!(
                    "process use is indeterminate because {} cannot be read: {error}",
                    maps_path.display()
                ),
                device: None,
            },
        )),
    }
    let gone = std::mem::take(&mut reader.gone);
    let process = ProcessLinks { pid, leader, links };
    proc_reads::note_process_gone(&holder, &base, &gone);
    process
}

/// The distinct absolute file paths in one /proc/PID/maps table, each
/// without the kernel's " (deleted)" suffix. A line has five fields and
/// then the path, which may hold spaces; split as the Python census does.
fn mapped_paths(maps: &[u8]) -> std::collections::BTreeSet<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut paths = std::collections::BTreeSet::new();
    for line in maps.split(|byte| *byte == b'\n') {
        let mut rest = line;
        let mut fields = 0;
        while fields < 5 {
            let start = rest
                .iter()
                .position(|byte| !byte.is_ascii_whitespace())
                .unwrap_or(rest.len());
            rest = &rest[start..];
            let end = rest
                .iter()
                .position(u8::is_ascii_whitespace)
                .unwrap_or(rest.len());
            if end == 0 {
                break;
            }
            rest = &rest[end..];
            fields += 1;
        }
        let start = rest
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let path = rest[start..].trim_ascii_end();
        if fields < 5 || !path.starts_with(b"/") {
            continue;
        }
        let path = path.strip_suffix(b" (deleted)").unwrap_or(path);
        paths.insert(PathBuf::from(std::ffi::OsStr::from_bytes(path)));
    }
    paths
}

/// What one snapshot has read so far, across its listings of /proc.
struct SnapshotReads<'a> {
    proc_root: &'a Path,
    targets: &'a [PathBuf],
    exit_wait: proc_reads::ExitWait,
    devices: Vec<Option<u64>>,
    settled: Vec<bool>,
    read: std::collections::HashMap<i64, u64>,
    processes: Vec<ProcessLinks>,
    unsettling: Vec<i64>,
    added: usize,
}

impl SnapshotReads<'_> {
    /// Keep `process`, read as generation `start`, settling each target
    /// it is the first seen to use (or that it may use).
    fn admit(&mut self, process: ProcessLinks, start: u64) {
        for (index, target) in self.targets.iter().enumerate() {
            let Some(device) = self.devices[index] else {
                continue;
            };
            if self.settled[index] {
                continue;
            }
            let uses = process.links.iter().any(|(_label, observation)| {
                observation_points_into(observation, target, device).unwrap_or(true)
            });
            if uses {
                self.settled[index] = true;
                self.unsettling.push(process.pid);
            }
        }
        self.processes.retain(|known| known.pid != process.pid);
        self.read.insert(process.pid, start);
        self.processes.push(process);
        self.added += 1;
    }

    /// Read every surviving thread of `leader` not read yet.
    ///
    /// A thread that executes a new program takes over the leader's
    /// PID and start time, and its own ID vanishes: a thread listed
    /// here and then read after its execve shows nothing. So once the
    /// threads are read, a leader that runs again is read too, in this
    /// listing, unless that generation was already read while it ran
    /// (that read showed what the group then shared). The check costs
    /// one stat read, and also catches an execve that came before the
    /// thread listing (#1102).
    fn admit_threads(&mut self, leader: i64) -> Result<()> {
        for (tid, start) in surviving_threads(self.proc_root, leader)? {
            if self.read.get(&tid) == Some(&start) {
                continue;
            }
            let thread = read_process_links(self.proc_root, tid, Some(leader), &self.exit_wait);
            self.admit(thread, start);
        }
        let base = self.proc_root.join(leader.to_string());
        if let Some((start, state)) = process_stat(&base) {
            if !is_terminal_state(state) && self.read.get(&leader) != Some(&start) {
                let process = read_process_links(self.proc_root, leader, None, &self.exit_wait);
                self.admit(process, start);
            }
        }
        Ok(())
    }
}

/// Capture one shared process snapshot for `targets`, excluding this
/// process's ancestry and the coordinator. Enumeration failure stays
/// fail-closed.
///
/// One semantics with the Python census (#1102): a process that exits
/// while it is read cannot be using a target, so its missing entries
/// are skipped; a process that appears after a listing must still be
/// seen. After reading every listed process generation (PID and start
/// time) the snapshot lists /proc again and reads each generation it
/// has not read yet. A listing that adds no generation closes the
/// snapshot; otherwise a listing is proven when none of its new
/// generations uses, or may use, a target no earlier generation was
/// already seen using, and two proven listings in a row close it. Unrelated births on a busy host therefore do not keep it
/// open. Exhausting SNAPSHOT_LISTING_ROUNDS refuses. A thread-group
/// leader that exited while its threads still run is live: each
/// surviving thread is read in its place (`surviving_threads`), both
/// when a listing finds the leader already exited and when it exits
/// while it is read (#1102 part 2).
pub(crate) fn capture_process_snapshot(
    coordinator_pid: i64,
    targets: &[PathBuf],
) -> Result<ProcessSnapshot> {
    capture_process_snapshot_at(Path::new("/proc"), coordinator_pid, targets)
}

/// One snapshot over `proc_root` instead of the host `/proc`; a test seam
/// so a census test lists only its own fixture processes and can never
/// trip over an unrelated host process mid-exit (#1249).
pub(crate) fn capture_process_snapshot_at(
    proc_root: &Path,
    coordinator_pid: i64,
    targets: &[PathBuf],
) -> Result<ProcessSnapshot> {
    use std::os::unix::fs::MetadataExt as _;
    let ignored = own_ancestry();
    // A target that cannot be inspected is judged (and refused) by
    // assert_slot_unused_snapshot; here it can settle nothing.
    let devices: Vec<Option<u64>> = targets
        .iter()
        .map(|target| {
            std::fs::metadata(target)
                .ok()
                .map(|metadata| metadata.dev())
        })
        .collect();
    let mut reads = SnapshotReads {
        proc_root,
        targets,
        exit_wait: proc_reads::ExitWait::new(),
        devices,
        settled: vec![false; targets.len()],
        read: std::collections::HashMap::new(),
        processes: Vec::new(),
        unsettling: Vec::new(),
        added: 0,
    };
    let mut proven_rounds = 0;
    for round in 0..SNAPSHOT_LISTING_ROUNDS {
        let names = list_proc(proc_root)?;
        if round == 0 {
            // Test seam (#1084, #1102 wrkslots-census-churn-tolerant): the
            // forced-churn differential makes a listed process vanish and a
            // new one appear between the listing and the reads.
            if let Some(command) = std::env::var_os("WRKSLOTS_TEST_CENSUS_CHURN") {
                let _ = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(command)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        reads.unsettling.clear();
        reads.added = 0;
        for name in names {
            let Ok(pid) = name.parse::<i64>() else {
                continue;
            };
            if ignored.contains(&pid) || pid == coordinator_pid {
                continue;
            }
            let base = proc_root.join(pid.to_string());
            let Some((start, state)) = process_stat(&base) else {
                continue;
            };
            if is_terminal_state(state) {
                // A leader that exited while its threads still run is
                // followed through them, however it was read before.
                reads.admit_threads(pid)?;
                continue;
            }
            if reads.read.get(&pid) == Some(&start) {
                continue;
            }
            let process = read_process_links(proc_root, pid, None, &reads.exit_wait);
            reads.admit(process, start);
            // A leader that exited while it was read showed nothing; its
            // threads still hold what it held, so they are read now, in
            // this listing, not left to one that may never come.
            reads.admit_threads(pid)?;
        }
        if round > 0 && reads.added == 0 {
            // Nothing appeared since the last listing: every later birth
            // descends from a generation already read.
            return Ok(ProcessSnapshot {
                processes: reads.processes,
            });
        }
        if reads.unsettling.is_empty() {
            proven_rounds += 1;
            if proven_rounds >= 2 {
                return Ok(ProcessSnapshot {
                    processes: reads.processes,
                });
            }
        } else {
            proven_rounds = 0;
        }
    }
    let mut unsettling = reads.unsettling;
    unsettling.sort_unstable();
    unsettling.dedup();
    Err(format!(
        "process use is indeterminate because the process snapshot did not close within {SNAPSHOT_LISTING_ROUNDS} listings of /proc (most recently PIDs {})",
        unsettling
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Assert no process in `snapshot` uses `slot_path`.
pub(crate) fn assert_slot_unused_snapshot(
    snapshot: &ProcessSnapshot,
    slot_path: &Path,
) -> Result<()> {
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
    for process in &snapshot.processes {
        let mut via: Option<String> = None;
        for (label, observation) in &process.links {
            if observation_points_into(observation, slot_path, slot_device)? {
                via = Some(label.to_string());
                break;
            }
        }
        if let Some(via) = via {
            return Err(format!(
                "live {} uses slot {} ({via}); stop it and retry",
                process.holder(),
                slot_path.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn assert_slot_unused(slot_path: &Path, coordinator_pid: i64) -> Result<()> {
    let snapshot = capture_process_snapshot(coordinator_pid, &[slot_path.to_path_buf()])?;
    assert_slot_unused_snapshot(&snapshot, slot_path)
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
    // an ignored-file-free status (the gate judged ignored files
    // outside the caches separately, without --ignored).
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
    // One `-v` listing answers both questions (#1284, as in the oracle).
    let listed = git(&path, &["ls-files", "-v"])?;
    if listed.lines().any(|line| {
        line.chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
    }) {
        return Err(format!("checkout {name} has assume-unchanged index state"));
    }
    if listed.lines().any(|line| line.starts_with("S ")) {
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

/// Expand the configured cache globs inside one checkout and judge
/// what removal would reach, failing closed where the oracle's full
/// nested-repository judging has no Rust counterpart yet: a cache
/// path that is (or sits under) a symlink or another mount refuses,
/// and a nested `.git` inside a cache refuses. Ordinary cache
/// contents are regenerable and need no judging.
fn judge_cache_directories(checkout_path: &Path, globs: &[String]) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let patterns: Vec<Vec<String>> = globs
        .iter()
        .map(|pattern| crate::finish_write::glob_parts(pattern))
        .collect();
    let checkout_device = std::fs::metadata(checkout_path)
        .map(|meta| meta.dev())
        .unwrap_or(0);
    // Collect every directory in the checkout (never descending into
    // the checkout's own .git), then keep those a glob names.
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PathBuf> = vec![checkout_path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }
            if directory == checkout_path && entry.file_name() == ".git" {
                continue;
            }
            directories.push(path.clone());
            pending.push(path);
        }
    }
    for candidate in &directories {
        let relative = candidate
            .strip_prefix(checkout_path)
            .unwrap_or(candidate.as_path());
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        if !patterns
            .iter()
            .any(|pattern| crate::finish_write::glob_parts_match(pattern, &parts))
        {
            continue;
        }
        // No symlink component between the checkout root and the
        // cache, and the cache stays on the checkout's device.
        let mut cursor = checkout_path.to_path_buf();
        for part in &parts {
            cursor.push(part);
            let meta = std::fs::symlink_metadata(&cursor).map_err(|error| {
                format!("cannot inspect cache path {}: {error}", cursor.display())
            })?;
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "configured cache path is a symlink: {}",
                    cursor.display()
                ));
            }
            if meta.dev() != checkout_device {
                return Err(format!(
                    "configured cache path crosses a mount: {}",
                    cursor.display()
                ));
            }
        }
        // A nested Git repository inside the cache needs the oracle's
        // judging (disposable evidence or salvage), not deletion.
        let mut pending: Vec<PathBuf> = vec![candidate.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_name() == ".git" {
                    return Err(format!(
                        "cache {} contains nested Git repository at {}; nested-repository judging stays with the Python edition for now",
                        candidate.display(),
                        entry.path().display()
                    ));
                }
                let file_type = match entry.file_type() {
                    Ok(kind) => kind,
                    Err(_) => continue,
                };
                if file_type.is_dir() && !file_type.is_symlink() {
                    pending.push(entry.path());
                }
            }
        }
    }
    Ok(())
}

/// What removal does with ignored files outside the caches (#1302),
/// mirroring the Python edition's ``_IgnoredMode``.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnoredMode {
    Refuse,
    Archive,
    Discard,
}

/// One gitignored regular file or symlink removal would otherwise lose.
#[derive(Clone)]
struct IgnoredFile {
    path: String,
    size: u64,
    sha256: String,
    link_target: Option<String>,
    executable: bool,
}

struct IgnoredContext<'a> {
    machine: &'a str,
    slot: &'a str,
    generation: i64,
    archive_root: &'a Path,
}

const IGNORED_ARCHIVE_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

fn is_cargo_target_dir(path: &Path) -> bool {
    // The Python edition's `_is_cargo_target_dir` (#1042).
    if path.join("CACHEDIR.TAG").is_file() && path.join(".fingerprint").is_dir() {
        return true;
    }
    let profiles = ["debug", "release", "dev", "test", "bench", "check", "doc"];
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let child = entry.path();
        profiles.contains(&entry.file_name().to_string_lossy().as_ref())
            && child.is_dir()
            && !child.is_symlink()
            && child.join(".fingerprint").is_dir()
    })
}

fn ignored_path_is_cache(
    config: &Value,
    checkout_name: &str,
    checkout_path: &Path,
    relative: &str,
    cargo_targets: &mut HashMap<String, bool>,
) -> bool {
    // The Python edition's `_ignored_path_is_cache`: the cache
    // definition `clean-caches` shares.
    if crate::finish_write::cache_globs_for(config, checkout_name)
        .iter()
        .any(|pattern| crate::finish_write::cache_glob_contains_path(pattern, relative))
    {
        return true;
    }
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let repo_policy_empty = config
        .get("repo_cache_globs")
        .and_then(Value::as_array)
        .is_some_and(|groups| groups.is_empty());
    if parts.first() == Some(&"target") && repo_policy_empty {
        return true;
    }
    for end in 1..parts.len() {
        let ancestor = parts[..end].join("/");
        let cached = *cargo_targets
            .entry(ancestor)
            .or_insert_with(|| is_cargo_target_dir(&checkout_path.join(parts[..end].join("/"))));
        if cached {
            return true;
        }
    }
    false
}

fn hash_regular_file(path: &Path, relative: &str, checkout_name: &str) -> Result<(String, u64)> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).map_err(|error| {
        format!("cannot open gitignored file {relative} in checkout {checkout_name}: {error}")
    })?;
    let mut digest = Sha256::new();
    let mut size: u64 = 0;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            format!("cannot read gitignored file {relative} in checkout {checkout_name}: {error}")
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

fn record_ignored_file(
    checkout_name: &str,
    checkout_path: &Path,
    relative: &str,
    found: &mut BTreeMap<String, IgnoredFile>,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let full = checkout_path.join(relative);
    let mut cursor = checkout_path.to_path_buf();
    let components: Vec<&str> = relative.split('/').collect();
    for part in &components[..components.len().saturating_sub(1)] {
        cursor.push(part);
        let metadata = std::fs::symlink_metadata(&cursor).map_err(|error| {
            format!(
                "cannot inspect gitignored path {relative} in checkout {checkout_name}: {error}"
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "gitignored path crosses a symlink in checkout {checkout_name}: {relative}"
            ));
        }
    }
    let metadata = std::fs::symlink_metadata(&full).map_err(|error| {
        format!("cannot inspect gitignored path {relative} in checkout {checkout_name}: {error}")
    })?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(&full).map_err(|error| {
            format!(
                "cannot read gitignored symlink {relative} in checkout {checkout_name}: {error}"
            )
        })?;
        let target_text = target.to_str().ok_or_else(|| {
            format!("gitignored symlink target is not valid UTF-8 in checkout {checkout_name}: {relative}")
        })?;
        found.insert(
            relative.to_string(),
            IgnoredFile {
                path: relative.to_string(),
                size: target_text.len() as u64,
                sha256: format!("{:x}", Sha256::digest(target_text.as_bytes())),
                link_target: Some(target_text.to_string()),
                executable: false,
            },
        );
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(format!(
            "checkout {checkout_name} holds gitignored path {relative} that is not a regular file or symlink; remove it by hand before removal"
        ));
    }
    let (sha256, size) = hash_regular_file(&full, relative, checkout_name)?;
    found.insert(
        relative.to_string(),
        IgnoredFile {
            path: relative.to_string(),
            size,
            sha256,
            link_target: None,
            executable: metadata.permissions().mode() & 0o111 != 0,
        },
    );
    Ok(())
}

fn collect_ignored_directory(
    config: &Value,
    checkout_name: &str,
    checkout_path: &Path,
    relative: &str,
    cargo_targets: &mut HashMap<String, bool>,
    found: &mut BTreeMap<String, IgnoredFile>,
) -> Result<()> {
    let directory = checkout_path.join(relative);
    // A bare mirror or repository directory: its own Git checks judge
    // it; its object store is not ignored worktree files.
    if directory.join("HEAD").is_file()
        && directory.join("objects").is_dir()
        && directory.join("refs").is_dir()
    {
        return Ok(());
    }
    let git_marker = directory.join(".git");
    if git_marker.exists() || git_marker.is_symlink() {
        return collect_ignored_files(
            config,
            checkout_name,
            checkout_path,
            &directory,
            &format!("{relative}/"),
            cargo_targets,
            found,
        );
    }
    let mut entries: Vec<_> = std::fs::read_dir(&directory)
        .map_err(|error| {
            format!(
                "cannot read gitignored directory {relative} in checkout {checkout_name}: {error}"
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "cannot read gitignored directory {relative} in checkout {checkout_name}: {error}"
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" || name.starts_with("HANDOFF") {
            continue;
        }
        let child = format!("{relative}/{name}");
        if ignored_path_is_cache(config, checkout_name, checkout_path, &child, cargo_targets) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            format!("cannot inspect gitignored path {child} in checkout {checkout_name}: {error}")
        })?;
        if file_type.is_dir() {
            collect_ignored_directory(
                config,
                checkout_name,
                checkout_path,
                &child,
                cargo_targets,
                found,
            )?;
        } else {
            record_ignored_file(checkout_name, checkout_path, &child, found)?;
        }
    }
    Ok(())
}

fn collect_ignored_files(
    config: &Value,
    checkout_name: &str,
    checkout_path: &Path,
    repository: &Path,
    prefix: &str,
    cargo_targets: &mut HashMap<String, bool>,
    found: &mut BTreeMap<String, IgnoredFile>,
) -> Result<()> {
    let raw = git(
        repository,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
            ".",
        ],
    )?;
    for name in raw.split('\0') {
        if name.is_empty() {
            continue;
        }
        // HANDOFF* files are the handoff contract's, never salvage
        // evidence (the Python edition's `_collect_ignored_files`).
        if name.rsplit('/').next().unwrap_or(name).starts_with("HANDOFF") {
            continue;
        }
        let relative = format!("{prefix}{name}");
        if ignored_path_is_cache(
            config,
            checkout_name,
            checkout_path,
            &relative,
            cargo_targets,
        ) {
            continue;
        }
        let full = checkout_path.join(&relative);
        let metadata = std::fs::symlink_metadata(&full).map_err(|error| {
            format!(
                "cannot inspect gitignored path {relative} in checkout {checkout_name}: {error}"
            )
        })?;
        if metadata.is_dir() {
            collect_ignored_directory(
                config,
                checkout_name,
                checkout_path,
                &relative,
                cargo_targets,
                found,
            )?;
        } else {
            record_ignored_file(checkout_name, checkout_path, &relative, found)?;
        }
    }
    Ok(())
}

fn ignored_files_outside_caches(
    config: &Value,
    checkout_name: &str,
    checkout_path: &Path,
) -> Result<Vec<IgnoredFile>> {
    let mut found = BTreeMap::new();
    collect_ignored_files(
        config,
        checkout_name,
        checkout_path,
        checkout_path,
        "",
        &mut HashMap::new(),
        &mut found,
    )?;
    Ok(found.into_values().collect())
}

fn ignored_refusal(checkout_name: &str, files: &[IgnoredFile]) -> String {
    let total: u64 = files.iter().map(|item| item.size).sum();
    let mut shown = files
        .iter()
        .take(10)
        .map(|item| format!("{} ({} bytes)", item.path, item.size))
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > 10 {
        shown += &format!(" and {} more", files.len() - 10);
    }
    format!(
        "checkout {checkout_name} holds {} gitignored file(s) outside the known regenerable caches ({total} bytes) that removal would delete: {shown}; rerun with --archive-ignored to keep them in the salvage archive or --discard-ignored to delete them explicitly. state: REFUSED -- no checkout was salvaged or removed",
        files.len()
    )
}

fn sanitize_archive_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches(['-', '.']);
    if trimmed.is_empty() {
        "checkout".to_string()
    } else {
        trimmed.to_string()
    }
}

fn ignored_archive_path(
    archive_root: &Path,
    context: &IgnoredContext<'_>,
    checkout_name: &str,
    files: &[IgnoredFile],
) -> PathBuf {
    let name_digest = format!("{:x}", Sha256::digest(checkout_name.as_bytes()));
    let mut listing = Sha256::new();
    for item in files {
        listing.update(item.path.as_bytes());
        listing.update(b"\x00");
        listing.update(item.size.to_string().as_bytes());
        listing.update(b"\x00");
        listing.update(item.sha256.as_bytes());
        listing.update(b"\x00");
        listing.update(item.link_target.as_deref().unwrap_or("").as_bytes());
        listing.update(b"\x00");
    }
    let listing_digest = format!("{:x}", listing.finalize());
    archive_root
        .join("wrkslots-salvage")
        .join(context.machine)
        .join(context.slot)
        .join(context.generation.to_string())
        .join(format!(
            "ignored-{}-{}-{}.tar",
            sanitize_archive_name(checkout_name),
            &name_digest[..12],
            &listing_digest[..12]
        ))
}

fn prepare_local_salvage_directory(root: &Path, directory: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let euid = std::fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .unwrap_or(0);
    let relative = directory.strip_prefix(root).map_err(|_| {
        format!(
            "local salvage archive directory escapes its root: {}",
            directory.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part.as_os_str());
        match std::fs::create_dir(&current) {
            Ok(()) => {
                let _ = std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "cannot create local salvage archive directory {}: {error}",
                    current.display()
                ));
            }
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "cannot inspect local salvage archive directory {}: {error}",
                current.display()
            )
        })?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != euid
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(format!(
                "local salvage archive directory is not private current-user storage: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn assert_no_symlink_components(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "local salvage archive path escapes its root: {}",
            path.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part.as_os_str());
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "cannot inspect local salvage archive path {}: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "local salvage archive path crosses a symlink: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn parse_octal(field: &[u8]) -> Result<u64> {
    let text: String = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as char)
        .collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(trimmed, 8)
        .map_err(|error| format!("invalid octal field in ignored-files archive: {error}"))
}

fn ustar_header(
    name: &str,
    size: u64,
    mode: u32,
    typeflag: u8,
    linkname: &str,
) -> Result<[u8; 512]> {
    if linkname.len() > 100 {
        return Err("ignored-files archive link target is too long".to_string());
    }
    let (prefix, short_name): (String, String) = if name.len() <= 100 {
        (String::new(), name.to_string())
    } else {
        let components: Vec<&str> = name.split('/').collect();
        let mut split = None;
        for index in 1..components.len() {
            let candidate_prefix = components[..index].join("/");
            let candidate_name = components[index..].join("/");
            if candidate_prefix.len() <= 155 && candidate_name.len() <= 100 {
                split = Some((candidate_prefix, candidate_name));
                break;
            }
        }
        split.ok_or_else(|| "ignored-files archive path is too long".to_string())?
    };
    let mut header = [0u8; 512];
    let write_field = |header: &mut [u8; 512], start: usize, width: usize, bytes: &[u8]| {
        header[start..start + bytes.len()].copy_from_slice(bytes);
        let _ = width;
    };
    write_field(&mut header, 0, 100, short_name.as_bytes());
    write_field(
        &mut header,
        100,
        8,
        format!("{:07o}\0", mode & 0o7777).as_bytes(),
    );
    write_field(&mut header, 108, 8, b"0000000\0");
    write_field(&mut header, 116, 8, b"0000000\0");
    write_field(&mut header, 124, 12, format!("{size:011o}\0").as_bytes());
    write_field(&mut header, 136, 12, b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = typeflag;
    write_field(&mut header, 157, 100, linkname.as_bytes());
    header[257..265].copy_from_slice(b"ustar\x0000");
    write_field(&mut header, 345, 155, prefix.as_bytes());
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    Ok(header)
}

fn verify_ignored_archive(
    tar_path: &Path,
    checkout_name: &str,
    files: &[IgnoredFile],
) -> Result<()> {
    use std::io::Read as _;
    let mut reader = std::fs::File::open(tar_path).map_err(|error| {
        format!(
            "cannot verify ignored-files archive {}: {error}",
            tar_path.display()
        )
    })?;
    let expected: Vec<String> = files
        .iter()
        .map(|item| format!("{checkout_name}/{}", item.path))
        .collect();
    let mut index = 0usize;
    let mut zero_blocks = 0usize;
    loop {
        let mut header = [0u8; 512];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(format!(
                    "ignored-files archive is truncated: {}",
                    tar_path.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "cannot verify ignored-files archive {}: {error}",
                    tar_path.display()
                ));
            }
        }
        if header.iter().all(|byte| *byte == 0) {
            zero_blocks += 1;
            if zero_blocks == 2 {
                break;
            }
            continue;
        }
        if zero_blocks != 0 {
            return Err(format!(
                "ignored-files archive has data after its end blocks: {}",
                tar_path.display()
            ));
        }
        let stored_checksum = parse_octal(&header[148..156])?;
        let mut checksum_header = header;
        checksum_header[148..156].copy_from_slice(b"        ");
        let checksum: u64 = checksum_header.iter().map(|byte| u64::from(*byte)).sum();
        if checksum != stored_checksum {
            return Err(format!(
                "ignored-files archive header checksum does not match: {}",
                tar_path.display()
            ));
        }
        let name_field = &header[0..100];
        let name_end = name_field.iter().position(|byte| *byte == 0).unwrap_or(100);
        let short_name = String::from_utf8_lossy(&name_field[..name_end]).into_owned();
        let prefix_field = &header[345..500];
        let prefix_end = prefix_field
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(155);
        let prefix = String::from_utf8_lossy(&prefix_field[..prefix_end]).into_owned();
        let member_name = if prefix.is_empty() {
            short_name
        } else {
            format!("{prefix}/{short_name}")
        };
        if index >= expected.len() || member_name != expected[index] {
            return Err(format!(
                "ignored-files archive does not list the expected files: {}",
                tar_path.display()
            ));
        }
        let item = &files[index];
        let size = parse_octal(&header[124..136])?;
        let typeflag = header[156];
        if let Some(target) = &item.link_target {
            let link_field = &header[157..257];
            let link_end = link_field.iter().position(|byte| *byte == 0).unwrap_or(100);
            let linkname = String::from_utf8_lossy(&link_field[..link_end]).into_owned();
            if typeflag != b'2' || linkname != *target || size != 0 {
                return Err(format!(
                    "ignored-files archive member is not the expected symlink {member_name}: {}",
                    tar_path.display()
                ));
            }
        } else {
            if typeflag != b'0' && typeflag != 0 {
                return Err(format!(
                    "ignored-files archive member is not a regular file: {member_name} in {}",
                    tar_path.display()
                ));
            }
            let mut digest = Sha256::new();
            let mut remaining = size;
            let mut buffer = [0u8; 1024 * 1024];
            while remaining > 0 {
                let wanted =
                    usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
                let read = reader.read(&mut buffer[..wanted]).map_err(|error| {
                    format!(
                        "cannot verify ignored-files archive {}: {error}",
                        tar_path.display()
                    )
                })?;
                if read == 0 {
                    return Err(format!(
                        "ignored-files archive is truncated: {}",
                        tar_path.display()
                    ));
                }
                digest.update(&buffer[..read]);
                remaining -= read as u64;
            }
            if format!("{:x}", digest.finalize()) != item.sha256 {
                return Err(format!(
                    "ignored-files archive member {member_name} does not match its recorded sha256: {}",
                    tar_path.display()
                ));
            }
        }
        let padding = (512 - (size % 512)) % 512;
        let mut remaining = padding;
        let mut buffer = [0u8; 512];
        while remaining > 0 {
            let wanted =
                usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
            let read = reader.read(&mut buffer[..wanted]).map_err(|error| {
                format!(
                    "cannot verify ignored-files archive {}: {error}",
                    tar_path.display()
                )
            })?;
            if read == 0 {
                return Err(format!(
                    "ignored-files archive is truncated: {}",
                    tar_path.display()
                ));
            }
            remaining -= read as u64;
        }
        index += 1;
    }
    if index != expected.len() {
        return Err(format!(
            "ignored-files archive does not list the expected files: {}",
            tar_path.display()
        ));
    }
    Ok(())
}

fn archive_ignored_files(
    context: &IgnoredContext<'_>,
    checkout_name: &str,
    checkout_path: &Path,
    files: &[IgnoredFile],
) -> Result<Value> {
    use std::io::{Read as _, Seek as _, Write as _};
    let tar_path = ignored_archive_path(context.archive_root, context, checkout_name, files);
    let parent = tar_path.parent().ok_or_else(|| {
        format!(
            "ignored-files archive has no parent: {}",
            tar_path.display()
        )
    })?;
    prepare_local_salvage_directory(context.archive_root, parent)?;
    assert_no_symlink_components(context.archive_root, parent)?;
    if tar_path.exists() || tar_path.is_symlink() {
        verify_ignored_archive(&tar_path, checkout_name, files)?;
    } else {
        let temporary = parent.join(format!(
            ".{}.tmp.{}.{}",
            tar_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let write_result = (|| -> Result<()> {
            let mut output = std::fs::File::create(&temporary).map_err(|error| {
                format!(
                    "cannot create ignored-files archive {}: {error}",
                    temporary.display()
                )
            })?;
            let mut written: u64 = 0;
            for item in files {
                let member_name = format!("{checkout_name}/{}", item.path);
                if let Some(target) = &item.link_target {
                    let header = ustar_header(&member_name, 0, 0o777, b'2', target)?;
                    output.write_all(&header).map_err(|error| {
                        format!(
                            "cannot write ignored-files archive {}: {error}",
                            temporary.display()
                        )
                    })?;
                    written += 512;
                } else {
                    let header = ustar_header(
                        &member_name,
                        item.size,
                        if item.executable { 0o755 } else { 0o644 },
                        b'0',
                        "",
                    )?;
                    output.write_all(&header).map_err(|error| {
                        format!(
                            "cannot write ignored-files archive {}: {error}",
                            temporary.display()
                        )
                    })?;
                    written += 512;
                    let source = checkout_path.join(&item.path);
                    let mut input = std::fs::File::open(&source).map_err(|error| {
                        format!(
                            "cannot open gitignored file {} in checkout {checkout_name}: {error}",
                            item.path
                        )
                    })?;
                    let mut buffer = [0u8; 1024 * 1024];
                    let mut copied: u64 = 0;
                    loop {
                        let read = input.read(&mut buffer).map_err(|error| {
                            format!(
                                "cannot read gitignored file {} in checkout {checkout_name}: {error}",
                                item.path
                            )
                        })?;
                        if read == 0 {
                            break;
                        }
                        output.write_all(&buffer[..read]).map_err(|error| {
                            format!(
                                "cannot write ignored-files archive {}: {error}",
                                temporary.display()
                            )
                        })?;
                        copied += read as u64;
                    }
                    if copied != item.size {
                        return Err(format!(
                            "gitignored file changed while it was archived in checkout {checkout_name}: {}",
                            item.path
                        ));
                    }
                    written += copied;
                    let padding = (512 - (copied % 512)) % 512;
                    if padding > 0 {
                        output
                            .write_all(&vec![0u8; padding as usize])
                            .map_err(|error| {
                                format!(
                                    "cannot write ignored-files archive {}: {error}",
                                    temporary.display()
                                )
                            })?;
                        written += padding;
                    }
                }
            }
            output.write_all(&[0u8; 1024]).map_err(|error| {
                format!(
                    "cannot write ignored-files archive {}: {error}",
                    temporary.display()
                )
            })?;
            written += 1024;
            let record_padding = (10240 - (written % 10240)) % 10240;
            if record_padding > 0 {
                output
                    .write_all(&vec![0u8; record_padding as usize])
                    .map_err(|error| {
                        format!(
                            "cannot write ignored-files archive {}: {error}",
                            temporary.display()
                        )
                    })?;
            }
            output.sync_all().map_err(|error| {
                format!(
                    "cannot fsync ignored-files archive {}: {error}",
                    temporary.display()
                )
            })?;
            let _ = output.seek(std::io::SeekFrom::Start(0));
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temporary, &tar_path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!(
                "cannot publish ignored-files archive {}: {error}",
                tar_path.display()
            ));
        }
        let directory_file = std::fs::File::open(parent)
            .map_err(|error| format!("cannot fsync directory {}: {error}", parent.display()))?;
        directory_file
            .sync_all()
            .map_err(|error| format!("cannot fsync directory {}: {error}", parent.display()))?;
        verify_ignored_archive(&tar_path, checkout_name, files)?;
    }
    let (sha256, size) =
        hash_regular_file(&tar_path, &tar_path.display().to_string(), checkout_name)?;
    let relative = tar_path
        .strip_prefix(context.archive_root)
        .map_err(|_| {
            format!(
                "ignored-files archive escapes its root: {}",
                tar_path.display()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(json!({
        "archive_root": context.archive_root.display().to_string(),
        "path": relative,
        "bytes": size,
        "sha256": sha256,
    }))
}

fn salvage_gate(
    cfg: &Cfg,
    config: &Value,
    checkout: &Value,
    repository: &Path,
    ignored_mode: IgnoredMode,
    ignored_context: &IgnoredContext<'_>,
) -> Result<Value> {
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
    // Gitignored files outside the regenerable caches are judged before
    // anything else (#1302): refuse, archive, or explicitly discard.
    let ignored_files = ignored_files_outside_caches(config, &name, &path)?;
    let mut ignored_archive = Value::Null;
    if !ignored_files.is_empty() {
        match ignored_mode {
            IgnoredMode::Refuse => {
                return Err(ignored_refusal(&name, &ignored_files));
            }
            IgnoredMode::Archive => {
                let total: u64 = ignored_files.iter().map(|item| item.size).sum();
                if total > IGNORED_ARCHIVE_LIMIT_BYTES {
                    return Err(format!(
                        "gitignored files outside the known regenerable caches total {total} bytes, above the {IGNORED_ARCHIVE_LIMIT_BYTES}-byte --archive-ignored limit; preserve them by hand or rerun with --discard-ignored. state: REFUSED -- no checkout was salvaged or removed"
                    ));
                }
                ignored_archive =
                    archive_ignored_files(ignored_context, &name, &path, &ignored_files)?;
            }
            IgnoredMode::Discard => {}
        }
    }
    // Cleanliness by salvage semantics: tracked and untracked entries,
    // ignored files do not dirty a checkout.
    let globs = cache_globs_for(config, &name);
    if !globs.is_empty() {
        crate::finish_write::assert_cache_policy_untracked(&path, &name, &globs)?;
        judge_cache_directories(&path, &globs)?;
    }
    let mut status_args: Vec<String> = vec![
        "status".to_string(),
        "--porcelain=v2".to_string(),
        "--untracked-files=all".to_string(),
        "--".to_string(),
        ".".to_string(),
    ];
    for pattern in &globs {
        status_args.push(format!(":(exclude,glob){pattern}"));
        status_args.push(format!(":(exclude,glob){pattern}/**"));
    }
    let status_refs: Vec<&str> = status_args.iter().map(String::as_str).collect();
    let status = git(&path, &status_refs)?;
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
    let mut receipt = json!({
        "checkout": name,
        "source_head": head,
        "salvage_commit": head,
        "status_sha256": format!("{:x}", Sha256::digest(b"")),
        "disposition": "already-published",
        "remote_ref": Value::Null,
        "excluded_incomplete_history": [],
        "private_history_counts": {
            "named refs": 0,
            "stale pushed": 0,
            "stale fetched recent": 0,
            "stale fetched past prune expiry": 0,
            "dangling, held by source store": 0,
            "dangling, slot-only, kept": 0,
            "dangling, slot-only, past prune expiry": 0,
        },
        "containing_remote_refs": updated
            .get("containing_remote_refs")
            .cloned()
            .unwrap_or(json!([])),
        "ref_inventory": ref_inventory(&path, &name)?,
    });
    if !ignored_files.is_empty() {
        let disposition = match ignored_mode {
            IgnoredMode::Archive => "archived",
            IgnoredMode::Discard => "discarded",
            IgnoredMode::Refuse => unreachable!(),
        };
        if let Some(object) = receipt.as_object_mut() {
            object.insert(
                "ignored_files".to_string(),
                json!(ignored_files
                    .iter()
                    .map(|item| json!({
                        "path": item.path,
                        "bytes": item.size,
                        "sha256": item.sha256,
                        "disposition": disposition,
                    }))
                    .collect::<Vec<_>>()),
            );
            if ignored_mode == IgnoredMode::Archive {
                object.insert("ignored_archive".to_string(), ignored_archive);
            }
        }
    }
    Ok(receipt)
}

fn checkout_registration_gone(repository: &Path, path: &Path) -> bool {
    // A linked worktree whose .git file names an admin directory that no
    // longer exists, and that its repository no longer lists, has no Git
    // registration left to judge (#995, mirroring the Python edition).
    let git_file = path.join(".git");
    let Ok(raw) = std::fs::read_to_string(&git_file) else {
        return false;
    };
    let Some(named) = raw.trim_end_matches('\n').strip_prefix("gitdir: ") else {
        return false;
    };
    let named_path = PathBuf::from(named);
    let admin = if named_path.is_absolute() {
        named_path
    } else {
        path.join(&named_path)
    };
    if admin.exists() {
        return false;
    }
    let Ok(listing) = git(repository, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    !listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|listed| Path::new(listed) == path)
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

/// One removal validated and planned with no mutation locks held (#1287).
struct PreparedRemove {
    record: Value,
    events: Vec<Value>,
    salvage: Vec<Value>,
    answer_state: String,
    answer_detail: String,
    release_present: bool,
    owner_state: String,
    heartbeat_age: i64,
    agent: String,
    latest_read: Option<Value>,
}

/// Lock-free candidate validation for `remove_slot` (#1287): the fleet
/// validation, the state-home reclaim preparation, the storage and
/// handoff gates, the salvage gates and the process-use census all run
/// here, before the mutation locks are taken. The locked commit only
/// re-checks that this slot's record and the event tip are unchanged.
#[allow(clippy::too_many_arguments)]
fn prepare_remove(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    config: &Value,
    slot: &str,
    expected_generation: i64,
    coordinator_pid: i64,
    ignored_mode: IgnoredMode,
) -> Result<PreparedRemove> {
    validate_global(root, cfg)?;
    let active = active_document(root, cfg, selected)?;
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
    crate::reclaim_state::prepare_reclaim(&cfg.control, &slot_type, slot)?;
    refuse_target_storage(root, slot)?;
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), selected)?;
    {
        let hold = current_hold(root, cfg, selected, slot)?;
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
    let _ = owner_detail;
    let release = release_in_effect(cfg, &record, &events)?;
    let answer = liveness_answer(cfg, config, &record)?;
    if release.is_none() {
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
            let lower_detail = answer.detail.to_lowercase();
            let pane_scoped = answer.state == "unverifiable"
                && lower_detail.contains("pane")
                && !lower_detail.contains("archived-record-pane");
            if !pane_scoped {
                return Err(format!(
                    "registered liveness authority is {}: {}; unknown ownership is not a free slot",
                    answer.state, answer.detail
                ));
            }
            let pane_slot_path = slot_directory(cfg, slot, &slot_type)?;
            if let Some((pid, kind, token)) = process_naming_slot(&pane_slot_path) {
                return Err(format!(
                    "the unattributed pane may be using slot {slot}: live process {pid} names the slot in its {kind}: {token}; stop it and retry"
                ));
            }
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
    let slot_path = slot_directory(cfg, slot, &slot_type)?;
    assert_slot_contents(cfg, &record)?;
    let sidecar = load_sidecar_artifact(cfg, selected, slot, &slot_type, generation)?;
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
        let (_stored, repository) = stored_repository_path(cfg, repository_raw)?;
        let ignored_context = IgnoredContext {
            machine: selected,
            slot,
            generation,
            archive_root: &cfg.control,
        };
        salvage.push(salvage_gate(
            cfg,
            config,
            &checkout,
            &repository,
            ignored_mode,
            &ignored_context,
        )?);
    }
    assert_slot_unused(&slot_path, coordinator_pid)?;
    let agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(PreparedRemove {
        record,
        events,
        salvage,
        answer_state: answer.state,
        answer_detail: answer.detail,
        release_present: release.is_some(),
        owner_state,
        heartbeat_age,
        agent,
        latest_read,
    })
}

/// Remove and archive the named slot.
pub(crate) fn remove_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    coordinator_pid: i64,
    expected_generation: i64,
    ignored_mode: IgnoredMode,
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
    crate::freeze::assert_not_frozen(&cfg, &config, "remove")?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
    // Validate and plan with no lock held (#1287); the locked section
    // below only re-checks this slot's record and commits it.
    let prepared = prepare_remove(
        root,
        &cfg,
        &selected,
        &config,
        slot,
        expected_generation,
        coordinator_pid,
        ignored_mode,
    )?;
    let locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    let journals = parse_journals(root)?;
    if let Some(first) = journals
        .get("journals")
        .and_then(Value::as_array)
        .and_then(|names| names.first())
        .and_then(Value::as_str)
    {
        return Err(crate::journals_state::journal_blocker_message(
            &cfg.control,
            first,
        ));
    }
    let active = active_document(root, &cfg, &selected)?;
    // The archive document is (re)read at completion time, after the
    // out-of-lock deletion (#894); reading it here would go stale.
    let archives_path = cfg.control.join(format!("ARCHIVED.{selected}.json"));
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
    if record != prepared.record {
        return Err(format!(
            "slot {slot} changed between the lock-free validation and the registry lock; preserve it and retry"
        ));
    }
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
    // Locked commit (#1287): the slot's record was re-checked above and
    // its event chain must not have grown since the lock-free validation.
    if load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), &selected)?.len()
        != prepared.events.len()
    {
        return Err(format!(
            "slot {slot} changed between the lock-free validation and the registry lock; preserve it and retry"
        ));
    }
    let heartbeat_age = prepared.heartbeat_age;
    let owner_state = prepared.owner_state;
    let release: Option<Value> = if prepared.release_present {
        Some(Value::Null)
    } else {
        None
    };
    let answer = Liveness {
        state: prepared.answer_state,
        detail: prepared.answer_detail,
    };
    let slot_path = slot_directory(&cfg, slot, &slot_type)?;
    // Salvage gate: every checkout already published and clean (gathered
    // lock-free in prepare_remove; the refresh below re-verifies it).
    let salvage = prepared.salvage;
    let agent = prepared.agent;
    let latest_read = prepared.latest_read;
    ensure_event_log(root, &cfg, &selected)?;
    // The dead liveness line, recorded inside this registry-lock hold before
    // salvage, as the Python edition records it (#994).
    let mut active = active;
    if release.is_none() && answer.state == "dead" {
        let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
        let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
        append_event(
            &cfg,
            &selected,
            "active-state-recorded",
            json!({
                "action": REGISTERED_LIVENESS_ACTION,
                "slot": slot,
                "previous_revision": previous_revision,
                "revision": previous_revision + 1,
                "previous_record_sha256": previous_digest,
                "record": record,
                "evidence": registered_liveness_evidence(generation, &agent, &answer.detail),
            }),
        )?;
        if let Some(object) = active.as_object_mut() {
            object.insert("revision".to_string(), json!(previous_revision + 1));
        }
        atomic_write_json(
            &cfg.control.join(format!("ACTIVE.{selected}.json")),
            &active,
        )?;
    }
    let coordinator_obj = crate::status_state::identity_to_value(&coordinator);
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
        // #995: a validate checkout whose registration an interrupted
        // removal already deleted is accepted at its recorded evidence;
        // this writer always journals the finish before fencing, which
        // is the proof the removal began. Agent checkouts keep refusing.
        let checkout_path = cfg.root.join(
            checkout
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if current.get("slot_type").and_then(Value::as_str) == Some("validate")
            && checkout_registration_gone(&repository, &checkout_path)
        {
            final_checkouts.push(checkout);
        } else {
            final_checkouts.push(reclaim_checkout(&cfg, &checkout, &repository)?);
        }
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
    let remover = crate::status_state::read_process_identity(i64::from(std::process::id()))?;
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
        "removing": [],
        "salvage": salvage,
        "validate_complete": false,
        "record": current,
        // The removing process (#1121): a classifier reads the journal as
        // busy while it runs, not interrupted by the removed slot's owner.
        "owner": crate::status_state::identity_to_value(&remover),
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
        fence_slot_path(&cfg, slot, &slot_path, &fenced_path).map_err(|error| {
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
    // The journal and the fence are durable, so the registry lock is
    // released before the deletion I/O (#894): the fenced tree is deleted
    // unlocked, and the lock is re-taken only to record completion.  An
    // interruption from here on leaves the journal for recovery.
    drop(locks);
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
        // #995: record intent before the git call, and skip both the
        // repair and the git call when an interrupted removal already
        // deleted the registration (files are deleted with the fence).
        let registration_gone = current.get("slot_type").and_then(Value::as_str)
            == Some("validate")
            && checkout_registration_gone(&repository, &moved_path);
        let mut removing: Vec<String> = journal
            .get("removing")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !removing.contains(&name) {
            removing.push(name.clone());
        }
        if let Some(object) = journal.as_object_mut() {
            object.insert("removing".to_string(), json!(removing));
        }
        write_journal(&cfg, &selected, &journal)?;
        if !registration_gone {
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
        }
        removed.push(name.clone());
        removing.retain(|item| item != &name);
        if let Some(object) = journal.as_object_mut() {
            object.insert("removed".to_string(), json!(removed));
            object.insert("removing".to_string(), json!(removing));
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
    // Re-take the registry lock only to record completion (#894).  Other
    // slots may have changed while the deletion ran unlocked; this slot's
    // record must still be the fenced one.
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    let archives_before: Value = if archives_path.exists() {
        serde_json::from_str(
            &std::fs::read_to_string(&archives_path)
                .map_err(|error| format!("cannot read archive state: {error}"))?,
        )
        .map_err(|error| format!("cannot read archive state: {error}"))?
    } else {
        json!({"schema": 2, "machine": selected, "revision": 0, "records": []})
    };
    {
        let state_now = active_document(root, &cfg, &selected)?;
        let record_now = state_now
            .get("slots")
            .and_then(Value::as_array)
            .and_then(|slots| {
                slots
                    .iter()
                    .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
            })
            .cloned()
            .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
        if record_now != current {
            return Err(format!(
                "slot {slot} changed during its out-of-lock deletion; preserve the journal and run 'wrkslots recover'"
            ));
        }
    }
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
    let state_home_reclaimed = crate::reclaim_state::reclaim_home(&cfg.control, &slot_type, slot);
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "outcome": "removed and archived",
        "archive_id": archive_id,
        "state_home_reclaimed": state_home_reclaimed,
    }))
}

#[cfg(test)]
mod link_tests {
    use super::{assert_slot_unused_snapshot, capture_process_snapshot_at, link_points_into};

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

    #[test]
    fn shared_snapshot_detects_live_use_and_allows_unused() {
        let base = std::env::temp_dir().join(format!("wrkslots-snap-{}", std::process::id()));
        let slot = base.join("slot");
        let unused = base.join("unused");
        std::fs::create_dir_all(&slot).unwrap();
        std::fs::create_dir_all(&unused).unwrap();
        let slot = slot.canonicalize().unwrap();
        let unused = unused.canonicalize().unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&slot)
            .spawn()
            .unwrap();
        // #1482: a fake /proc holding only this test's child, so an
        // unrelated host process mid-exit can never enter the snapshot.
        let proc_root = base.join("proc");
        std::fs::create_dir_all(&proc_root).unwrap();
        std::os::unix::fs::symlink(
            format!("/proc/{}", child.id()),
            proc_root.join(child.id().to_string()),
        )
        .unwrap();
        let snapshot = capture_process_snapshot_at(
            &proc_root,
            std::process::id() as i64,
            &[slot.clone(), unused.clone()],
        )
        .unwrap();
        // One shared capture serves every slot check in an audit.
        assert!(assert_slot_unused_snapshot(&snapshot, &unused).is_ok());
        let error = assert_slot_unused_snapshot(&snapshot, &slot).unwrap_err();
        assert!(error.contains(&format!("live process {} uses slot", child.id())));
        let error = assert_slot_unused_snapshot(&snapshot, &slot).unwrap_err();
        assert!(error.contains("working directory"));
        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[cfg(test)]
mod mapped_paths_tests {
    use super::mapped_paths;
    use std::path::PathBuf;

    #[test]
    fn mapped_paths_keep_absolute_files_without_the_deleted_suffix() {
        let maps = b"00400000-00452000 r-xp 00000000 08:02 173521 /usr/bin/dbus-daemon\n\
00651000-00652000 r--p 00051000 08:02 173521 /usr/bin/dbus-daemon\n\
7f00-7f01 r--s 00000000 00:2c 42   /slot/a dir/held (deleted)\n\
7ffd-7ffe rw-p 00000000 00:00 0                          [stack]\n\
7ffe-7fff rw-p 00000000 00:00 0\n";
        let found: Vec<PathBuf> = mapped_paths(maps).into_iter().collect();
        assert_eq!(
            found,
            [
                PathBuf::from("/slot/a dir/held"),
                PathBuf::from("/usr/bin/dbus-daemon")
            ]
        );
    }
}

#[cfg(test)]
mod census_gone_tests {
    //! #1173 census-vanished-process: a snapshot read that finds the
    //! process exited is no evidence, and the snapshot completes.
    use super::{assert_slot_unused_snapshot, capture_process_snapshot_at};
    use crate::proc_reads::test_support::{holding_fd, in_cwd, mapping_only, stop, Slot};
    use crate::proc_reads::{inject, reset};

    const ENOENT: i32 = 2;
    const ESRCH: i32 = 3;
    const EACCES: i32 = 13;

    /// A fake /proc listing only the fixture children, each an alias of
    /// its real entry: the census reads them and no unrelated host
    /// process mid-exit can enter the snapshot (#1249).
    fn fake_proc_root(children: &[&std::process::Child]) -> std::path::PathBuf {
        // One root per test: tests in this binary run in parallel
        // threads, and a shared pid-named root lets one test remove
        // another's listing mid-census (#1482).
        let root = std::env::temp_dir().join(format!(
            "wrkslots-census-gone-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            children.first().map_or(0, |child| child.id())
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for child in children.iter() {
            std::os::unix::fs::symlink(
                format!("/proc/{}", child.id()),
                root.join(child.id().to_string()),
            )
            .unwrap();
        }
        root
    }

    fn fake_path(
        root: &std::path::Path,
        child: &std::process::Child,
        entry: &str,
    ) -> std::path::PathBuf {
        root.join(child.id().to_string()).join(entry)
    }

    fn snapshot_with(
        proc_root: &std::path::Path,
        child: &std::process::Child,
        slot: &Slot,
        entries: &[&str],
        errno: i32,
    ) -> (Result<(), String>, Vec<String>) {
        reset();
        for entry in entries {
            inject(&fake_path(proc_root, child, entry), errno);
        }
        let verdict = capture_process_snapshot_at(
            proc_root,
            std::process::id() as i64,
            std::slice::from_ref(&slot.slot),
        )
        .and_then(|snapshot| assert_slot_unused_snapshot(&snapshot, &slot.slot));
        let prefix = format!("census: process {} ", child.id());
        let notes = reset()
            .into_iter()
            .filter(|note| note.starts_with(&prefix))
            .collect();
        (verdict, notes)
    }

    #[test]
    fn a_process_gone_on_each_read_is_skipped_and_noted() {
        let slot = Slot::new("snapshot");
        let children = [
            (in_cwd(&slot), vec!["cwd"]),
            (holding_fd(&slot), vec!["fd/3"]),
            (holding_fd(&slot), vec!["fd"]),
        ];
        let proc_root = fake_proc_root(
            &children
                .iter()
                .map(|(child, _entries)| child)
                .collect::<Vec<_>>(),
        );
        for (child, entries) in &children {
            let (verdict, notes) = snapshot_with(&proc_root, child, &slot, &[], ESRCH);
            let error = verdict.unwrap_err();
            assert!(error.contains("uses slot"), "{entries:?}: {error}");
            assert!(notes.is_empty(), "{notes:?}");
        }
        for (child, entries) in &children {
            for errno in [ENOENT, ESRCH] {
                reset();
                // Every other child's use is hidden too, so the slot is
                // unused exactly when this child's gone reads are skipped.
                for (other, other_entries) in &children {
                    for entry in other_entries {
                        inject(&fake_path(&proc_root, other, entry), errno);
                    }
                }
                let verdict = capture_process_snapshot_at(
                    &proc_root,
                    std::process::id() as i64,
                    std::slice::from_ref(&slot.slot),
                )
                .and_then(|snapshot| assert_slot_unused_snapshot(&snapshot, &slot.slot));
                let prefix = format!("census: process {} ", child.id());
                let notes: Vec<String> = reset()
                    .into_iter()
                    .filter(|note| note.starts_with(&prefix))
                    .collect();
                assert_eq!(verdict, Ok(()), "{entries:?} errno {errno}");
                if errno == ESRCH {
                    assert_eq!(
                        notes,
                        [format!(
                            "census: process {} exited during the census ({}: ESRCH); skipped",
                            child.id(),
                            fake_path(&proc_root, child, entries[0]).display()
                        )]
                    );
                } else {
                    assert!(notes.is_empty(), "{notes:?}");
                }
            }
        }
        // A gone executable read still completes, with the other evidence.
        let (child, _entries) = &children[0];
        for errno in [ENOENT, ESRCH] {
            let (verdict, _notes) = snapshot_with(&proc_root, child, &slot, &["exe"], errno);
            assert!(verdict.unwrap_err().contains("uses slot"));
        }
        let _ = std::fs::remove_dir_all(&proc_root);
        for (child, _entries) in children {
            stop(child);
        }
    }

    /// `stat` with State D and PF_EXITING set: a task stuck in do_exit.
    fn exiting_stat(stat: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(stat).into_owned();
        let closing = text.rfind(") ").unwrap();
        let mut fields: Vec<String> = text[closing + 2..]
            .split_whitespace()
            .map(str::to_string)
            .collect();
        fields[0] = "D".to_string();
        let flags: u64 = fields[6].parse().unwrap();
        fields[6] = (flags | 0x4).to_string();
        format!("{}) {}\n", &text[..closing], fields.join(" ")).into_bytes()
    }

    fn snapshot_verdict(
        proc_root: &std::path::Path,
        slot: &Slot,
        _child: &std::process::Child,
    ) -> Result<(), String> {
        capture_process_snapshot_at(
            proc_root,
            std::process::id() as i64,
            std::slice::from_ref(&slot.slot),
        )
        .and_then(|snapshot| assert_slot_unused_snapshot(&snapshot, &slot.slot))
    }

    #[test]
    fn a_mapping_is_a_use_and_unreadable_maps_follow_the_owner_rule() {
        // #1174: the slot's only user maps a file from it.
        let slot = Slot::new("snapshot-maps");
        let child = mapping_only(&slot, "600");
        // #1482: read this test's census over the fake root, like the
        // gone-on-each-read test, so no host process can enter it. The
        // fake entry aliases the child's real one; fault injection is
        // keyed at the fake paths the census actually reads.
        let proc_root = fake_proc_root(&[&child]);
        let base = proc_root.join(child.id().to_string());
        let maps = base.join("maps");
        reset();
        let error = snapshot_verdict(&proc_root, &slot, &child).unwrap_err();
        assert!(
            error.contains("uses slot") && error.contains("(mapping)"),
            "{error}"
        );

        // Our own process, still in do_exit past the wait: indeterminate.
        reset();
        inject(&maps, EACCES);
        crate::proc_reads::replace(
            &base.join("stat"),
            &exiting_stat(&std::fs::read(base.join("stat")).unwrap()),
        );
        let started = std::time::Instant::now();
        let error = snapshot_verdict(&proc_root, &slot, &child).unwrap_err();
        let notes = reset();
        assert!(started.elapsed() >= crate::proc_reads::CENSUS_EXIT_WAIT);
        assert!(
            error.contains(&format!("{} is unreadable", maps.display())),
            "{error}"
        );
        assert!(
            error.contains(&format!(
                "; process {} is still exiting (PF_EXITING, its memory already released) after a ",
                child.id()
            )),
            "{error}"
        );
        let prefix = format!("census: process {} still exiting after ", child.id());
        let suffix = format!(" ms ({}: EACCES); indeterminate", maps.display());
        assert!(
            notes
                .iter()
                .any(|note| note.starts_with(&prefix) && note.ends_with(&suffix)),
            "{notes:?}"
        );

        // A protected process that is not exiting (non-dumpable, setuid):
        // no evidence, as Python's remove census takes it. Python's audit
        // reads such a process as root; this snapshot cannot (documented
        // gap), so today the slot reads as unused.
        reset();
        inject(&maps, EACCES);
        assert_eq!(snapshot_verdict(&proc_root, &slot, &child), Ok(()));
        let _ = reset();
        let _ = std::fs::remove_dir_all(&proc_root);
        stop(child);

        // Our own process in do_exit that exits within the wait: gone. Read
        // through the per-process reader, so whole-host snapshot time does
        // not decide whether it is still running when its maps are read.
        let mut child = mapping_only(&slot, "0.1");
        let base = std::path::PathBuf::from(format!("/proc/{}", child.id()));
        let maps = base.join("maps");
        reset();
        inject(&maps, EACCES);
        crate::proc_reads::replace_until_exit(
            &base.join("stat"),
            &exiting_stat(&std::fs::read(base.join("stat")).unwrap()),
        );
        let process = super::read_process_links(
            std::path::Path::new("/proc"),
            child.id() as i64,
            None,
            &crate::proc_reads::ExitWait::new(),
        );
        let notes = reset();
        child.wait().unwrap();
        assert!(
            !process
                .links
                .iter()
                .any(|(label, observation)| *label == "mapping"
                    && !matches!(observation, super::LinkObservation::NoEvidence)),
            "an exited process's unreadable maps is no evidence"
        );
        assert!(
            notes.contains(&format!(
                "census: process {} exited during the census ({}: EACCES); skipped",
                child.id(),
                maps.display()
            )),
            "{notes:?}"
        );
    }

    #[test]
    fn processes_exiting_during_the_snapshot_never_refuse_or_hide_a_user() {
        let slot = Slot::new("snapshot-churn");
        let unused = Slot::new("snapshot-churn-unused");
        let user = in_cwd(&slot);
        let running = std::sync::atomic::AtomicBool::new(true);
        // #1482: the census reads the fake root, not the host table.
        // Every churn child is aliased into it before it is waited on,
        // so exactly this test's own processes can enter the snapshot.
        let proc_root = fake_proc_root(&[&user]);
        let running = &running;
        std::thread::scope(|scope| {
            // Bounded churn: a gate runs this beside other steps under a
            // CPU limit, so the children are few and paced.
            let slot_dir = slot.slot.clone();
            for _ in 0..2 {
                let proc_root = proc_root.clone();
                let slot_dir = slot_dir.clone();
                scope.spawn(move || {
                    for _ in 0..100 {
                        if !running.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        let mut child = std::process::Command::new("true")
                            .current_dir(&slot_dir)
                            .spawn()
                            .unwrap();
                        let _ = std::os::unix::fs::symlink(
                            format!("/proc/{}", child.id()),
                            proc_root.join(child.id().to_string()),
                        );
                        child.wait().unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                });
            }
            for _ in 0..5 {
                let snapshot = capture_process_snapshot_at(
                    &proc_root,
                    std::process::id() as i64,
                    &[slot.slot.clone(), unused.slot.clone()],
                )
                .unwrap();
                assert_eq!(assert_slot_unused_snapshot(&snapshot, &unused.slot), Ok(()));
                let error = assert_slot_unused_snapshot(&snapshot, &slot.slot).unwrap_err();
                assert!(error.contains("uses slot"), "{error}");
            }
            running.store(false, std::sync::atomic::Ordering::Relaxed);
        });
        let _ = reset();
        let _ = std::fs::remove_dir_all(&proc_root);
        stop(user);
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
