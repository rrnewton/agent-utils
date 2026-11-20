//! Writer: hand a slot with a provably dead owner to its successor.
//!
//! Reproduces the existing `transfer-owner` mutation: generation
//! and agent checks, the already-owned outcome, the provably-dead
//! recorded-owner requirement, the released-generation refusal
//! (with a coordinator-authorized `--reopen` rebind starting a new
//! generation), unregistered nested worktrees warned not refused,
//! successor authorization by caller ancestry or by residence in
//! the slot's own cgroup box, and the `owner-transferred` (or
//! `owner-reopened`) active-state event plus ACTIVE shard rewrite.
//! Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::adopt_write::validate_agent_name;
use sha2::{Digest, Sha256};

use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::finish_write::{
    cache_globs_for, stored_path, stored_repository_path, verify_existing_worktree,
};
use crate::global_state::load_event_chain;
use crate::heartbeat_write::{assert_caller, release_in_effect};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage_except_nested, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

fn identity_value(identity: &Identity) -> Value {
    crate::status_state::identity_to_value(identity)
}

fn record_identity(record: &Value) -> Option<Identity> {
    let value = record.get("owner")?;
    if value.is_null() {
        return None;
    }
    Some(Identity {
        pid: value.get("pid")?.as_i64()?,
        start_ticks: value.get("start_ticks")?.as_i64()?,
        boot_id: value.get("boot_id")?.as_str()?.to_string(),
        host_id: value.get("host_id")?.as_str()?.to_string(),
        cgroup_path: value.get("cgroup_path")?.as_str()?.to_string(),
        namespace_pid: crate::status_state::optional_namespace_pid(value),
        pidns_inode: crate::status_state::optional_pidns_inode(value),
    })
}

fn systemd_escape(value: &str) -> Option<String> {
    let output = Command::new("systemd-escape")
        .arg("--")
        .arg(value)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn cgroup_in_slot_box(cgroup_path: &str, slot_type: &str, slot: &str) -> bool {
    let label = if slot_type == "agent" {
        slot.to_string()
    } else {
        format!("{slot_type}:{slot}")
    };
    let Some(escaped) = systemd_escape(&label) else {
        return false;
    };
    let own = format!("wrkslots-{escaped}.slice");
    cgroup_path
        .split('/')
        .any(|component| component == own || component.ends_with(&format!("-{own}")))
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "Git refused in {}: git {}{}",
            repo.display(),
            args.join(" "),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_text_scrubbed(
    repo: &Path,
    args: &[std::ffi::OsString],
    stdin_bytes: Option<&[u8]>,
    allowed: &[i32],
) -> Result<Vec<u8>> {
    // The ignore decision must not be caller-controlled (#1870):
    // strip every GIT_* knob and the user config locations, disable
    // the user excludes file, and turn off fsmonitor and the
    // untracked cache, so only tracked .gitignore rules can make a
    // path count as ignored.
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .arg("-c")
        .arg("core.excludesFile=/dev/null")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("core.untrackedCache=false");
    for (key, _value) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(&key);
        }
    }
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", "/dev/null")
        .env("XDG_CONFIG_HOME", "/dev/null")
        .env("LC_ALL", "C")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if stdin_bytes.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if let Some(bytes) = stdin_bytes {
        use std::io::Write as _;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(bytes);
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    let code = output.status.code().unwrap_or(-1);
    if !allowed.contains(&code) {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "Git refused the clean-tree check in {}: git exited {code}{}",
            repo.display(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(output.stdout)
}

fn os_args(parts: &[&str]) -> Vec<std::ffi::OsString> {
    parts.iter().map(std::ffi::OsString::from).collect()
}

/// Split NUL-terminated porcelain-v2 status into blocking entries
/// and ignored candidates (the `! ` entries), as raw bytes.
fn split_status_lines(status: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut blocking: Vec<Vec<u8>> = Vec::new();
    let mut candidates: Vec<Vec<u8>> = Vec::new();
    for entry in status.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        if let Some(path) = entry.strip_prefix(b"! ".as_slice()) {
            candidates.push(path.to_vec());
        } else {
            blocking.push(entry.to_vec());
        }
    }
    (blocking, candidates)
}

/// The candidates ignored by a TRACKED .gitignore rule, and only
/// those (mirrors `_tracked_ignored_paths`): check-ignore attributes
/// each candidate, a rule counts only when its source is a
/// .gitignore tracked in the index, and a hidden edit (an
/// assume-unchanged or skip-worktree bit, or a blob that differs
/// from the index) refuses outright.
fn tracked_ignored_paths(repo: &Path, candidates: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
    use std::os::unix::ffi::OsStrExt as _;

    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin_bytes: Vec<u8> = Vec::new();
    for name in candidates {
        stdin_bytes.extend_from_slice(name);
        stdin_bytes.push(0);
    }
    let attributed = git_text_scrubbed(
        repo,
        &os_args(&["check-ignore", "-v", "-z", "--stdin"]),
        Some(&stdin_bytes),
        &[0, 1],
    )?;
    let fields: Vec<&[u8]> = attributed.split(|byte| *byte == 0).collect();
    let mut sources: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut index = 0;
    while index + 3 < fields.len() + 1 && index + 3 <= fields.len() {
        if index + 3 >= fields.len() {
            break;
        }
        let source = fields[index];
        if source == b".gitignore" || source.ends_with(b"/.gitignore") {
            sources.push((fields[index + 3].to_vec(), source.to_vec()));
        }
        index += 4;
    }
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let mut unique_sources: Vec<Vec<u8>> = sources.iter().map(|(_, s)| s.clone()).collect();
    unique_sources.sort();
    unique_sources.dedup();
    let mut ls_args = os_args(&["--literal-pathspecs", "ls-files", "-v", "-z", "--"]);
    for source in &unique_sources {
        ls_args.push(std::ffi::OsStr::from_bytes(source).to_os_string());
    }
    let listed = git_text_scrubbed(repo, &ls_args, None, &[0])?;
    let mut tags: std::collections::HashMap<Vec<u8>, Vec<u8>> = std::collections::HashMap::new();
    for entry in listed.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        if let Some(at) = entry.iter().position(|byte| *byte == b' ') {
            tags.insert(entry[at + 1..].to_vec(), entry[..at].to_vec());
        }
    }
    let mut staged_args = os_args(&["--literal-pathspecs", "ls-files", "-s", "-z", "--"]);
    for source in &unique_sources {
        staged_args.push(std::ffi::OsStr::from_bytes(source).to_os_string());
    }
    let staged = git_text_scrubbed(repo, &staged_args, None, &[0])?;
    let mut index_blobs: std::collections::HashMap<Vec<u8>, Vec<u8>> =
        std::collections::HashMap::new();
    for entry in staged.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        if let Some(at) = entry.iter().position(|byte| *byte == b'\t') {
            let (meta, name) = (&entry[..at], &entry[at + 1..]);
            let parts: Vec<&[u8]> = meta.split(|byte| *byte == b' ').collect();
            if parts.len() >= 2 {
                index_blobs.insert(name.to_vec(), parts[1].to_vec());
            }
        }
    }
    for source in &unique_sources {
        let Some(tag) = tags.get(source) else {
            continue;
        };
        let hidden = tag == b"S" || tag.iter().all(|byte| byte.is_ascii_lowercase());
        let shown = String::from_utf8_lossy(source);
        if hidden {
            return Err(format!(
                "ignore rules in {shown} are hidden by an assume-unchanged or skip-worktree bit; a slot changes agent only with its ignore rules visible"
            ));
        }
        let mut blob_args = os_args(&["hash-object", "--no-filters", "--"]);
        blob_args.push(std::ffi::OsStr::from_bytes(source).to_os_string());
        let blob = git_text_scrubbed(repo, &blob_args, None, &[0])?;
        if blob.trim_ascii() != index_blobs.get(source).map(Vec::as_slice).unwrap_or(b"") {
            return Err(format!(
                "ignore rules in {shown} differ from the index; a slot changes agent only with a clean tree"
            ));
        }
    }
    let tracked: std::collections::HashSet<&[u8]> = tags.keys().map(Vec::as_slice).collect();
    Ok(sources
        .into_iter()
        .filter(|(_, source)| tracked.contains(source.as_slice()))
        .map(|(name, _)| name)
        .collect())
}

/// Refuse a rename while any present checkout carries uncommitted work.
///
/// Gitignored files are not authored work and carry no attribution
/// (#1870): they are routinely the slot's protected evidence, so they
/// transfer with the slot unchanged and are returned for the caller's
/// output instead of refusing the rename.
fn assert_checkouts_clean_for_rebind(
    root: Option<&Path>,
    machine_override: Option<&str>,
    cfg: &crate::active_state::Cfg,
    record: &Value,
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut ignored: Vec<(String, Vec<u8>)> = Vec::new();
    let config = load_config(root, machine_override)?;
    for checkout in record
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
        let raw_path = checkout
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let path = stored_path(cfg, raw_path, "checkout path")?;
        if !path.exists() {
            continue;
        }
        let raw_repository = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_relative, repository) = stored_repository_path(cfg, raw_repository)?;
        verify_existing_worktree(&repository, &path)?;
        let branch = git_text(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?
            .trim()
            .to_string();
        let expected_branch = checkout
            .get("branch")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if branch != expected_branch {
            return Err(format!(
                "checkout {name} branch changed: expected {expected_branch}, found {branch}"
            ));
        }
        let globs = cache_globs_for(&config, &name);
        let mut status_args = os_args(&[
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
            "--",
            ".",
        ]);
        for pattern in &globs {
            status_args.push(std::ffi::OsString::from(format!(
                ":(exclude,glob){pattern}"
            )));
            status_args.push(std::ffi::OsString::from(format!(
                ":(exclude,glob){pattern}/**"
            )));
        }
        let status = git_text_scrubbed(&path, &status_args, None, &[0])?;
        if !status.is_empty() {
            let (mut blocking, candidates) = split_status_lines(&status);
            let kept = tracked_ignored_paths(&path, &candidates)?;
            for candidate in &candidates {
                if !kept.contains(candidate) {
                    let mut line = b"! ".to_vec();
                    line.extend_from_slice(candidate);
                    blocking.push(line);
                }
            }
            if let Some(first) = blocking.first() {
                let shown = String::from_utf8_lossy(first);
                return Err(format!(
                    "checkout {name} is dirty or has untracked/ignored files ({shown}); a slot changes agent only with a clean tree: finish or discard the changes first"
                ));
            }
            ignored.extend(
                candidates
                    .into_iter()
                    .map(|candidate| (name.clone(), candidate)),
            );
        }
    }
    Ok(ignored)
}

fn find_slot_record(active: &Value, slot: &str, selected: &str) -> Result<Value> {
    active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))
}

struct TransferJudgement {
    generation: i64,
    record_agent: String,
    rename: bool,
    target_agent: String,
    existing: Option<Identity>,
    previous_state: String,
    basis: String,
    release_event: Option<Value>,
    ignored_transferred: Vec<(String, Vec<u8>)>,
}

enum TransferDecision {
    Already { generation: i64 },
    Proceed(Box<TransferJudgement>),
}

/// Single-record transfer refusals, judged identically before the
/// locks (lock-free) and again under them (#1975). Only this record,
/// its release history and live process facts are judged here.
#[allow(clippy::too_many_arguments)]
fn judge_transfer(
    root: Option<&Path>,
    machine_override: Option<&str>,
    cfg: &crate::active_state::Cfg,
    selected: &str,
    record: &Value,
    successor: &Identity,
    agent: &str,
    to_agent: Option<&str>,
    reopen: bool,
    expected_generation: i64,
) -> Result<TransferDecision> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!(
            "stale generation for slot {slot}: expected {expected_generation}, current {generation}"
        ));
    }
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let rename = to_agent.is_some_and(|target| target != record_agent.as_str());
    let target_agent = to_agent.unwrap_or(agent).to_string();
    if record_agent != agent && (to_agent.is_some() || !reopen) {
        return Err(format!(
            "ownership mismatch for slot {slot}: expected {record_agent}, got {agent}"
        ));
    }
    let existing = record_identity(record);
    if !reopen && !rename {
        if let Some(owner) = &existing {
            if *owner == *successor {
                return Ok(TransferDecision::Already { generation });
            }
        }
    }
    let (previous_state, detail) = process_state(existing.as_ref());
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), selected)?;
    let release_event = release_in_effect(cfg, record, &events)?;
    if reopen {
        if release_event.is_none() {
            return Err(format!(
                "slot {slot} generation {generation} is not a released generation in effect; use plain 'wrkslots transfer-owner' without --reopen for a dead-owner transfer"
            ));
        }
    } else {
        if rename && (existing.is_none() || previous_state != "dead") {
            return Err(format!(
                "slot {slot} owner is {previous_state}: {detail}; transfer-owner only replaces an owner that is provably dead"
            ));
        }
        if existing.is_some() && previous_state != "dead" {
            return Err(format!(
                "slot {slot} owner is {previous_state}: {detail}; transfer-owner only replaces an owner that is provably dead"
            ));
        }
        if release_event.is_some() {
            return Err(format!(
                "slot {slot} generation {generation} was released by its owner; a released generation is waiting for removal and is not transferred. remedy: a coordinator may rebind it with 'wrkslots transfer-owner {slot} --agent <agent> --owner-pid <pid> --expected-generation {generation} --reopen --coordinator-authorized --coordinator-pid <pid> --reason <why>'"
            ));
        }
    }
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let basis = match assert_caller(successor, "owner") {
        Ok(()) => "caller-ancestry".to_string(),
        Err(_) => {
            if cgroup_in_slot_box(&successor.cgroup_path, &slot_type, &slot) {
                "slot-box".to_string()
            } else {
                return Err(format!(
                    "owner PID {} is neither in the invoking process ancestry nor inside slot {slot}'s own box (control group {})",
                    successor.pid, successor.cgroup_path
                ));
            }
        }
    };
    let confirmed = read_process_identity(successor.pid)?;
    if confirmed != *successor {
        return Err("owner process generation changed during transfer-owner".to_string());
    }
    let ignored_transferred = if rename {
        assert_checkouts_clean_for_rebind(root, machine_override, cfg, record)?
    } else {
        Vec::new()
    };
    Ok(TransferDecision::Proceed(Box::new(TransferJudgement {
        generation,
        record_agent,
        rename,
        target_agent,
        existing,
        previous_state: previous_state.to_string(),
        basis,
        release_event,
        ignored_transferred,
    })))
}

/// Transfer the named slot to a live successor owner.
#[allow(clippy::too_many_arguments)]
pub(crate) fn transfer_owner(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    to_agent: Option<&str>,
    owner_pid: i64,
    expected_generation: i64,
    reopen: bool,
    coordinator_pid: Option<i64>,
    coordinator_authorized: bool,
    reason: Option<&str>,
) -> Result<Value> {
    let reason_text = reason.unwrap_or("").trim().to_string();
    let coordinator = if reopen {
        if !coordinator_authorized {
            return Err(
                "released generation reopen is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                    .to_string(),
            );
        }
        let Some(pid) = coordinator_pid else {
            return Err("transfer-owner --reopen requires --coordinator-pid".to_string());
        };
        if reason_text.is_empty() {
            return Err("transfer-owner --reopen requires a non-empty --reason".to_string());
        }
        let identity = read_process_identity(pid)?;
        assert_caller(&identity, "coordinator")?;
        Some(identity)
    } else {
        None
    };
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    validate_agent_name(agent)?;
    if let Some(target) = to_agent {
        validate_name(target, "agent")?;
        validate_agent_name(target)?;
    }
    let successor = read_process_identity(owner_pid)?;
    // Lock-free prepare (#1975, following the Python edition and this
    // edition's heartbeat): fleet validation, the storage scan, dead
    // findings and the single-record judgement run holding no lock.
    // This edition has no registry-identity compare yet, so the commit
    // under the locks re-judges only the touched record (generation,
    // agent, owner, release, successor) and never trusts the prepare.
    refuse_partial_state(&cfg.control)?;
    validate_global(root, &cfg)?;
    let nested = refuse_target_storage_except_nested(root, slot)?;
    crate::hold_write::report_dead_findings(root, slot)?;
    let prepared_active = active_document(root, &cfg, &selected)?;
    let prepared_record = find_slot_record(&prepared_active, slot, &selected)?;
    match judge_transfer(
        root,
        machine_override,
        &cfg,
        &selected,
        &prepared_record,
        &successor,
        agent,
        to_agent,
        reopen,
        expected_generation,
    )? {
        TransferDecision::Already { generation } => {
            return Ok(json!({
                "machine": selected,
                "slot": slot,
                "agent": agent,
                "generation": generation,
                "owner_pid": successor.pid,
                "already_transferred": true,
            }));
        }
        TransferDecision::Proceed(_) => {}
    }
    // Commit: only the touched record is re-judged before the write.
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        if let Some(first) = names.first().and_then(Value::as_str) {
            return Err(crate::journals_state::journal_blocker_message(
                &cfg.control,
                first,
            ));
        }
    }
    let active = active_document(root, &cfg, &selected)?;
    let record = find_slot_record(&active, slot, &selected)?;
    let judged = match judge_transfer(
        root,
        machine_override,
        &cfg,
        &selected,
        &record,
        &successor,
        agent,
        to_agent,
        reopen,
        expected_generation,
    )? {
        TransferDecision::Already { generation } => {
            return Ok(json!({
                "machine": selected,
                "slot": slot,
                "agent": agent,
                "generation": generation,
                "owner_pid": successor.pid,
                "already_transferred": true,
            }));
        }
        TransferDecision::Proceed(judged) => judged,
    };
    let TransferJudgement {
        generation,
        record_agent,
        rename,
        target_agent,
        existing,
        previous_state,
        basis,
        release_event,
        ignored_transferred,
    } = *judged;
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let new_generation = if reopen || rename {
        generation + 1
    } else {
        generation
    };
    let mut transferred = record.clone();
    if let Some(object) = transferred.as_object_mut() {
        object.insert("owner".to_string(), identity_value(&successor));
        object.insert("heartbeat_at".to_string(), json!(utc_now()));
        if reopen || rename {
            object.insert("agent".to_string(), json!(target_agent));
            object.insert("generation".to_string(), json!(new_generation));
        }
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = transferred.clone();
        }
    }
    let mut evidence = json!({
        "owner": identity_value(&successor),
        "previous_owner": existing.as_ref().map(identity_value).unwrap_or(Value::Null),
        "previous_owner_state": previous_state,
        "basis": basis,
    });
    if reopen {
        let release_sequence = release_event
            .as_ref()
            .and_then(|event| event.get("sequence"))
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(object) = evidence.as_object_mut() {
            object.insert("previous_agent".to_string(), json!(record_agent));
            object.insert("previous_generation".to_string(), json!(generation));
            object.insert(
                "coordinator".to_string(),
                coordinator
                    .as_ref()
                    .map(identity_value)
                    .unwrap_or(Value::Null),
            );
            object.insert("coordinator_authorized".to_string(), json!(true));
            object.insert("reason".to_string(), json!(reason_text));
            object.insert("release_sequence".to_string(), release_sequence);
        }
    } else if rename {
        if let Some(object) = evidence.as_object_mut() {
            object.insert("previous_agent".to_string(), json!(record_agent));
            object.insert("previous_generation".to_string(), json!(generation));
        }
    }
    if !nested.is_empty() {
        if let Some(object) = evidence.as_object_mut() {
            object.insert("unregistered_nested_worktrees".to_string(), json!(nested));
        }
    }
    for detail in &nested {
        eprintln!(
            "WARNING: unregistered nested worktree retained during transfer-owner: {detail}. state: RETAINED -- ownership did not touch it and removal still refuses it"
        );
    }
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": if reopen { "owner-reopened" } else { "owner-transferred" },
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": transferred,
            "evidence": evidence,
        }),
    )?;
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    let mut result = json!({
        "machine": selected,
        "slot": slot,
        "agent": target_agent,
        "generation": new_generation,
        "owner_pid": successor.pid,
        "basis": basis,
        "previous_owner_state": previous_state,
        "already_transferred": false,
        "reopened": reopen,
        "unregistered_nested_worktrees": nested,
    });
    if !ignored_transferred.is_empty() {
        // The ignored set that transferred with the slot (#1870),
        // recorded in the command output only -- not a persisted field.
        let mut listing: Vec<u8> = Vec::new();
        for (index, (checkout, path)) in ignored_transferred.iter().enumerate() {
            if index > 0 {
                listing.push(b'\n');
            }
            listing.extend_from_slice(checkout.as_bytes());
            listing.push(0);
            listing.extend_from_slice(path);
        }
        let digest = format!("{:x}", Sha256::digest(&listing));
        if let Some(object) = result.as_object_mut() {
            object.insert(
                "ignored_transferred".to_string(),
                json!({"count": ignored_transferred.len(), "sha256": digest}),
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{cgroup_in_slot_box, split_status_lines};

    #[test]
    fn status_lines_split_blocking_from_ignored_candidates() {
        let status = b"1 .M N... 100644 100644 100644 abc def file.txt\0? new.txt\0! .buckconfig.local\0! ignored/\0! p q/.gitignore\0";
        let (blocking, candidates) = split_status_lines(status);
        assert_eq!(blocking.len(), 2);
        assert!(blocking[0].windows(8).any(|w| w == b"file.txt"));
        assert!(blocking[1].starts_with(b"? "));
        assert_eq!(
            candidates,
            vec![
                b".buckconfig.local".to_vec(),
                b"ignored/".to_vec(),
                b"p q/.gitignore".to_vec()
            ]
        );
        let (blocking, candidates) = split_status_lines(b"");
        assert!(blocking.is_empty() && candidates.is_empty());
    }

    // Mirrors the Python `_cgroup_in_slot_box` cases in
    // test_owner_transfer.py, including systemd-escaped hyphens.
    #[test]
    fn slot_box_matches_own_slice_only() {
        let boxed = "/user.slice/user-1.slice/user@1.service/app.slice/app-wrkslots.slice/app-wrkslots-kvm\\x2d20261012.slice/wrkslots-run-kvm\\x2d20261012-1073417.scope";
        assert!(cgroup_in_slot_box(boxed, "agent", "kvm-20261012"));
        assert!(cgroup_in_slot_box(
            "/wrkslots.slice/wrkslots-slot01.slice/x.scope",
            "agent",
            "slot01"
        ));
        for slot in ["kvm", "20261012", "kvm-2026101"] {
            assert!(!cgroup_in_slot_box(boxed, "agent", slot), "{slot}");
        }
        assert!(!cgroup_in_slot_box(
            "/app.slice/app-wrkslots-box-kvm\\x2d20261012.slice/a.scope",
            "agent",
            "kvm-20261012"
        ));
        assert!(!cgroup_in_slot_box(
            "/app.slice/herdr-server.service",
            "agent",
            "kvm-20261012"
        ));
        assert!(!cgroup_in_slot_box(
            "/user.slice/wrkslots-slot-b.slice/run.scope",
            "agent",
            "slot-a"
        ));
    }
}
