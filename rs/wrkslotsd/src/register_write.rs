//! Writer: register already-created live worktrees (`register`).
//!
//! Reproduces the Python mutation behind the registry locks:
//! coordinator authorization, the existing-worktree Git identity
//! checks (worktree root, common directory, registration, branch,
//! single fetch/push URL authority), the slot-contents check, the
//! owner/coordinator generation rechecks, and the `slot-registered`
//! active-state event. Fixture registries only until the edition
//! cutover (see `ensure_states::require_fixture`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory, stored_repository_parts};
use crate::adopt_write::{validate_agent_name, verify_task_scope};
use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, validate_name, validate_remote};
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, ensure_states_locked, refuse_partial_state,
    require_fixture, utc_now,
};
use crate::global_state::discover_machines;
use crate::heartbeat_write::{assert_caller, identity_value};
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::journals_state::parse_journals;
use crate::status_state::{read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

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
    let output = command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_raw(repo: &Path, args: &[&str]) -> (i32, String, String) {
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
    match command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .output()
    {
        Ok(output) => (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        Err(_) => (-1, String::new(), String::new()),
    }
}

fn common_directory(repository: &Path) -> Result<PathBuf> {
    let text = git(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(text);
    if common.is_symlink() || !common.is_dir() {
        return Err(format!(
            "Git reported an unsafe common directory: {}",
            common.display()
        ));
    }
    Ok(common)
}

fn repository_root(repository: &Path) -> Result<PathBuf> {
    let text = git(repository, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(text);
    if root.is_symlink() || !root.is_dir() {
        return Err(format!(
            "Git reported an unsafe repository root: {}",
            root.display()
        ));
    }
    Ok(root)
}

fn verify_existing_worktree(repository: &Path, checkout: &Path) -> Result<String> {
    let source_common = common_directory(repository)?;
    let checkout_root = repository_root(checkout)?;
    let absolute = if checkout.is_absolute() {
        checkout.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(checkout)
    };
    if checkout_root != absolute {
        return Err(format!(
            "checkout path is not its Git worktree root: {} (root {})",
            checkout.display(),
            checkout_root.display()
        ));
    }
    if common_directory(checkout)? != source_common {
        return Err(format!(
            "checkout {} does not belong to repository {}",
            checkout.display(),
            repository.display()
        ));
    }
    let listing = git(repository, &["worktree", "list", "--porcelain"])?;
    let listed = listing.lines().any(|line| {
        line.strip_prefix("worktree ")
            .map(|path| Path::new(path.trim()) == absolute.as_path())
            .unwrap_or(false)
    });
    if !listed {
        return Err(format!(
            "Git does not list {} as a worktree of {}",
            checkout.display(),
            repository.display()
        ));
    }
    let head = git(checkout, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    if head.len() != 40 && head.len() != 64 || !head.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!(
            "checkout HEAD is not a full commit: {}",
            checkout.display()
        ));
    }
    Ok(head)
}

fn branch_of(checkout: &Path) -> Result<String> {
    let (code, stdout, stderr) = git_raw(checkout, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    if code == 1 && stdout.is_empty() && stderr.is_empty() {
        return Err(format!(
            "checkout must be on its registered branch, not detached: {}",
            checkout.display()
        ));
    }
    if code != 0 {
        return Err(format!(
            "cannot read the branch of checkout {}: git symbolic-ref failed",
            checkout.display()
        ));
    }
    let branch = stdout.trim().to_string();
    crate::active_state::validate_ref(&branch, "branch")?;
    Ok(branch)
}

fn remote_url_sha256(checkout: &Path, remote: &str) -> Result<String> {
    validate_remote(remote)?;
    let (code, stdout, _stderr) = git_raw(
        checkout,
        &[
            "config",
            "--null",
            "--get-all",
            &format!("remote.{remote}.pushurl"),
        ],
    );
    if code != 0 && code != 1 {
        return Err(format!(
            "cannot determine whether remote '{remote}' has a push URL"
        ));
    }
    let values: Vec<&str> = stdout.split('\0').filter(|v| !v.is_empty()).collect();
    if !values.is_empty() {
        return Err(format!(
            "remote '{remote}' has a configured push URL; remove it so fetch and push authority agree"
        ));
    }
    let fetched = git(checkout, &["remote", "get-url", "--all", remote])?;
    let urls: Vec<&str> = fetched.lines().collect();
    if urls.len() != 1 || urls[0].is_empty() {
        return Err(format!(
            "remote '{remote}' must have exactly one non-empty fetch URL"
        ));
    }
    let pushed = git(checkout, &["remote", "get-url", "--push", "--all", remote])?;
    let push_urls: Vec<&str> = pushed.lines().collect();
    if push_urls != urls {
        return Err(format!(
            "remote '{remote}' effective push URL differs from its single fetch URL; remove pushInsteadOf or other push-only redirection"
        ));
    }
    let url = urls[0];
    if url.starts_with('-') {
        return Err(format!(
            "remote '{remote}' URL cannot begin with '-' for isolated transport"
        ));
    }
    Ok(format!("{:x}", Sha256::digest(url.as_bytes())))
}

fn assignments(values: &[String], label: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for value in values {
        let Some((name, item)) = value.split_once('=') else {
            return Err(format!("{label} must be NAME=VALUE: {value}"));
        };
        validate_name(name, "checkout")?;
        if out.insert(name.to_string(), item.to_string()).is_some() {
            return Err(format!("{label} supplied more than once for {name}"));
        }
    }
    Ok(out)
}

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn descends_from(process: &Identity, ancestor: &Identity) -> bool {
    if read_process_identity(process.pid)
        .map(|now| now != *process)
        .unwrap_or(true)
    {
        return false;
    }
    let mut current: Option<i64> = Some(process.pid);
    let mut seen: Vec<i64> = Vec::new();
    for _ in 0..256 {
        let Some(pid) = current else { break };
        if seen.contains(&pid) {
            break;
        }
        seen.push(pid);
        if read_process_identity(pid)
            .map(|candidate| candidate == *ancestor)
            .unwrap_or(false)
        {
            return true;
        }
        current = parent_pid(pid);
    }
    false
}

/// Register the named slot's existing worktrees, printing the outcome.
#[allow(clippy::too_many_arguments)]
pub(crate) fn register_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    slot_type: Option<&str>,
    coordinator_authorized: bool,
    agent: &str,
    task: &str,
    purpose: &str,
    owner_pid: i64,
    coordinator_pid: i64,
    verified_live: bool,
    repos: &[String],
    remotes_in: &[String],
    remote_urls: &[String],
    task_scope_unit: Option<&str>,
    task_scope_invocation_id: Option<&str>,
) -> Result<String> {
    require_fixture()?;
    if !coordinator_authorized {
        return Err("worktree registration is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized".to_string());
    }
    let slot_type = match slot_type {
        Some(value @ ("agent" | "validate")) => value.to_string(),
        _ => {
            return Err("worktree registration must record whether this is an agent or validate slot. state: REFUSED -- no worktree or lifecycle record was changed. remedy: rerun with --slot-type agent for authored work, or --slot-type validate for a disposable validation checkout whose evidence lives outside the slot".to_string())
        }
    };
    let config = load_config(root, machine_override)?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    if machine_override.is_none() && std::env::var_os("WRKSLOTS_MACHINE").is_none() {
        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        let short = hostname.split('.').next().unwrap_or_default().to_string();
        if selected != short {
            return Err(format!(
                "refusing worktree registration: configured machine shard '{selected}' differs from current host '{short}', and no machine override was supplied"
            ));
        }
    }
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    validate_agent_name(agent)?;
    if task.is_empty() || purpose.is_empty() {
        return Err("task and purpose must be non-empty".to_string());
    }
    if !verified_live {
        return Err("registration requires --verified-live and a live --owner-pid".to_string());
    }
    let owner = read_process_identity(owner_pid)?;
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
    let task_scope = match (task_scope_unit, task_scope_invocation_id) {
        (Some(unit), Some(invocation)) => Some(verify_task_scope(&owner, unit, invocation)?),
        (None, None) => None,
        _ => {
            return Err(
                "--task-scope-unit and --task-scope-invocation-id must be supplied together"
                    .to_string(),
            )
        }
    };
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        if let Some(first) = names.first().and_then(Value::as_str) {
            return Err(format!(
                "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                cfg.control.join(first).display()
            ));
        }
    }
    validate_global(root, &cfg)?;
    // The slot and agent must be free on every machine (Python
    // `_assert_agent_and_slot_free`, including the active-slot cap).
    let ttl = config
        .get("heartbeat_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cap = config.get("max_active_slots").and_then(Value::as_i64);
    let mut local_active: Vec<String> = Vec::new();
    for machine in discover_machines(&cfg.control)? {
        let (machine_active, _order) =
            crate::active_state::parse_state_ordered(root, Some(&machine))?;
        if let Some(slots) = machine_active.get("slots").and_then(Value::as_array) {
            for record in slots {
                let name = record
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if machine == selected {
                    local_active.push(name.to_string());
                }
                if name == slot {
                    return Err(format!("slot '{slot}' is already active on {machine}"));
                }
                if slot_type == "agent"
                    && record.get("slot_type").and_then(Value::as_str) == Some("agent")
                    && record.get("import_source").is_none_or(Value::is_null)
                    && record.get("agent").and_then(Value::as_str) == Some(agent)
                {
                    return Err(format!(
                        "agent '{agent}' already owns slot '{name}' on {machine}"
                    ));
                }
            }
        }
        let archive = crate::archive_state::parse_archive(root, Some(&machine))?;
        if let Some(records) = archive.get("records").and_then(Value::as_array) {
            for record in records {
                if record.get("slot").and_then(Value::as_str) == Some(slot) {
                    return Err(format!(
                        "slot '{slot}' is archived on {machine}; finished slots are never reused"
                    ));
                }
            }
        }
    }
    if let Some(cap) = cap {
        if (local_active.len() as i64) >= cap {
            let mut sorted = local_active.clone();
            sorted.sort();
            return Err(format!(
                "machine {selected} already holds {} active slot(s) against max_active_slots={cap}; finish or remove a slot before allocating another. Active: {}",
                local_active.len(),
                sorted.join(", ")
            ));
        }
    }
    ensure_states_locked(root, &cfg, &selected)?;
    let active = active_document(root, &cfg, &selected)?;
    // Existing checkouts (Python `_existing_checkouts`).
    let repositories = assignments(repos, "repository")?;
    let remotes = assignments(remotes_in, "remote")?;
    let remote_url_map = assignments(remote_urls, "trusted remote URL")?;
    if repositories.is_empty() {
        return Err("at least one --repo NAME=PATH is required".to_string());
    }
    if cfg.layout == "flat" && repositories.len() != 1 {
        return Err("flat layout requires exactly one repository per slot".to_string());
    }
    for (mapping, label) in [
        (&remotes, "remote"),
        (&remote_url_map, "trusted remote URL"),
    ] {
        let unknown: Vec<&String> = mapping
            .keys()
            .filter(|key| !repositories.contains_key(*key))
            .collect();
        if !unknown.is_empty() {
            let names: Vec<&str> = unknown.iter().map(|key| key.as_str()).collect();
            return Err(format!(
                "{label} supplied for unknown repository: {}",
                names.join(", ")
            ));
        }
    }
    struct Checkout {
        name: String,
        path: String,
        repository: String,
        branch: String,
        head: String,
        remote: String,
        remote_url_sha256: String,
        landed_ref: String,
        destination: PathBuf,
        repository_path: PathBuf,
    }
    let mut checkouts: Vec<Checkout> = Vec::new();
    for (name, raw_repository) in &repositories {
        let (repository_relative, unresolved, _allowed) =
            stored_repository_parts(&cfg, raw_repository)?;
        let repository_path = std::fs::canonicalize(&unresolved).unwrap_or(unresolved.clone());
        let slot_dir = slot_directory(&cfg, slot, &slot_type)?;
        let destination = if cfg.layout == "flat" {
            slot_dir.clone()
        } else {
            slot_dir.join(name)
        };
        let destination_relative = destination
            .strip_prefix(&cfg.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let metadata = destination.symlink_metadata();
        if metadata
            .map(|meta| !meta.is_dir() || meta.file_type().is_symlink())
            .unwrap_or(true)
        {
            return Err(format!(
                "existing checkout is missing or unsafe: {}",
                destination.display()
            ));
        }
        let head = verify_existing_worktree(&repository_path, &destination)?;
        let branch = branch_of(&destination)?;
        let remote = validate_remote(
            remotes
                .get(name)
                .map(String::as_str)
                .unwrap_or(&cfg.default_remote),
        )?;
        let digest = match remote_url_map.get(name) {
            None => remote_url_sha256(&destination, &remote)?,
            Some(expected) => {
                let observed = remote_url_sha256(&destination, &remote)?;
                // Recompute the URL check through the authority digest:
                // compare against the expected URL directly.
                let expected_digest = format!("{:x}", Sha256::digest(expected.as_bytes()));
                if observed != expected_digest {
                    return Err(format!(
                        "remote '{remote}' URL differs from trusted provisioning input"
                    ));
                }
                observed
            }
        };
        let landed_ref = crate::active_state::landed_ref_for_remote(&cfg, &remote)?;
        if !landed_ref.starts_with(&format!("refs/remotes/{remote}/")) {
            return Err(format!(
                "landed ref for {name} must be under refs/remotes/{remote}/"
            ));
        }
        checkouts.push(Checkout {
            name: name.clone(),
            path: destination_relative,
            repository: repository_relative,
            branch,
            head,
            remote,
            remote_url_sha256: digest,
            landed_ref,
            destination,
            repository_path,
        });
    }
    checkouts.sort_by(|left, right| left.name.cmp(&right.name));
    // Slot contents (Python `_assert_slot_contents`).
    let slot_dir = slot_directory(&cfg, slot, &slot_type)?;
    let slot_metadata = slot_dir.symlink_metadata();
    if slot_metadata
        .map(|meta| !meta.is_dir() || meta.file_type().is_symlink())
        .unwrap_or(true)
    {
        return Err(format!(
            "slot directory is missing or unsafe: {}",
            slot_dir.display()
        ));
    }
    if cfg.layout != "flat" {
        let expected: Vec<&str> = checkouts.iter().map(|item| item.name.as_str()).collect();
        let mut actual: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(&slot_dir).map_err(|error| {
            format!(
                "cannot inspect slot directory {}: {error}",
                slot_dir.display()
            )
        })?;
        for entry in entries.flatten() {
            actual.push(entry.file_name().to_string_lossy().into_owned());
        }
        let mut extras: Vec<&str> = actual
            .iter()
            .map(String::as_str)
            .filter(|name| {
                !expected.contains(name) && !(slot_type == "agent" && *name == "HANDOFF.md")
            })
            .collect();
        let mut missing: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|name| !actual.iter().any(|item| item == name))
            .collect();
        extras.sort();
        missing.sort();
        if !extras.is_empty() || !missing.is_empty() {
            let mut details: Vec<String> = Vec::new();
            if !extras.is_empty() {
                details.push(format!("unexpected entries {}", extras.join(", ")));
            }
            if !missing.is_empty() {
                details.push(format!("missing checkouts {}", missing.join(", ")));
            }
            return Err(format!(
                "slot directory does not match its record: {}",
                details.join("; ")
            ));
        }
    }
    for checkout in &checkouts {
        let head = verify_existing_worktree(&checkout.repository_path, &checkout.destination)?;
        let branch = branch_of(&checkout.destination)?;
        let digest = remote_url_sha256(&checkout.destination, &checkout.remote)?;
        if head != checkout.head
            || branch != checkout.branch
            || digest != checkout.remote_url_sha256
        {
            return Err(format!(
                "checkout {} identity changed before state publication; preserve it for inspection",
                checkout.name
            ));
        }
    }
    if read_process_identity(owner.pid)? != owner {
        return Err("owner process generation changed during registration".to_string());
    }
    let confirmed_coordinator = read_process_identity(coordinator.pid)?;
    if confirmed_coordinator != coordinator {
        return Err("coordinator process generation changed during registration".to_string());
    }
    assert_caller(&confirmed_coordinator, "coordinator")?;
    // Owner authorization: caller ancestry, else coordinator descent
    // plus a working directory inside the slot.
    if assert_caller(&owner, "owner").is_err() {
        if !descends_from(&owner, &coordinator) {
            return Err(format!(
                "owner PID {} does not descend from coordinator PID {}",
                owner.pid, coordinator.pid
            ));
        }
        let cwd = std::fs::read_link(format!("/proc/{}/cwd", owner.pid))
            .map_err(|error| format!("cannot inspect owner working directory: {error}"))?;
        let cwd_text = cwd.to_string_lossy().into_owned();
        let cwd_clean = cwd_text.strip_suffix(" (deleted)").unwrap_or(&cwd_text);
        if !Path::new(cwd_clean).starts_with(&slot_dir) {
            return Err(format!(
                "owner PID {} is outside the invoking process ancestry and does not have its working directory inside slot {}",
                owner.pid,
                slot_dir.display()
            ));
        }
    }
    let now = utc_now();
    let checkout_values: Vec<Value> = checkouts
        .iter()
        .map(|checkout| {
            json!({
                "name": checkout.name,
                "path": checkout.path,
                "repository": checkout.repository,
                "branch": checkout.branch,
                "start_point": checkout.head,
                "remote": checkout.remote,
                "remote_url_sha256": checkout.remote_url_sha256,
                "landed_ref": checkout.landed_ref,
                "head": checkout.head,
                "containing_remote_refs": [],
                "vcs": "git",
            })
        })
        .collect();
    let checkout_count = checkout_values.len();
    let mut record_object = serde_json::Map::new();
    record_object.insert("slot".to_string(), json!(slot));
    record_object.insert("agent".to_string(), json!(agent));
    record_object.insert("task".to_string(), json!(task));
    record_object.insert("purpose".to_string(), json!(purpose));
    record_object.insert("slot_type".to_string(), json!(slot_type));
    record_object.insert("machine".to_string(), json!(selected));
    record_object.insert("generation".to_string(), json!(1));
    record_object.insert("created_at".to_string(), json!(now));
    record_object.insert("heartbeat_at".to_string(), json!(now));
    record_object.insert("heartbeat_ttl_seconds".to_string(), json!(ttl));
    record_object.insert("owner".to_string(), identity_value(&owner));
    record_object.insert(
        "coordinator_lease".to_string(),
        identity_value(&coordinator),
    );
    record_object.insert("coordinator_recovery_note".to_string(), Value::Null);
    record_object.insert("handoff".to_string(), Value::Null);
    record_object.insert("checkouts".to_string(), json!(checkout_values));
    if let Some(scope) = &task_scope {
        record_object.insert("task_scope".to_string(), scope.clone());
    }
    let record = Value::Object(record_object);
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if slots
        .iter()
        .any(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
    {
        return Err(format!("slot '{slot}' is already active"));
    }
    slots.push(record.clone());
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    let _ = previous_digest;
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "slot-registered",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": Value::Null,
            "record": record,
            "evidence": {
                "slot_type": slot_type,
                "coordinator_authorized": true,
                "coordinator": identity_value(&coordinator),
            },
        }),
    )?;
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    validate_global(root, &cfg)?;
    Ok(format!(
        "registered slot={slot} agent={agent} generation=1 checkouts={checkout_count}\n"
    ))
}
