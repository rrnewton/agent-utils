//! Writer: create linked worktrees and register a slot.
//!
//! Reproduces the existing `create` mutation for the worktree
//! representation: coordinator/owner authorization, the create
//! plan (repository, branch, start, remote authority, fetch),
//! the CREATE journal lifecycle with progress events, post-
//! provision hooks, the `slot-created` active-state event, and
//! journal completion. Slot-image provisioning, validate-batch
//! seals, and interrupted-create classification stay with the
//! Python edition in this slice: their presence refuses here.
//! Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory, Cfg};
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::{
    acquire_lock, append_event, atomic_write_json, ensure_event_log,
    refuse_partial_state_allow_seals, require_fixture, utc_now,
};
use crate::hold_write::validate_global;
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

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    git(repo, args).is_ok()
}

fn identity_value(identity: &Identity) -> Value {
    json!({
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    })
}

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn assert_caller(identity: &Identity, label: &str) -> Result<()> {
    let mut current: Option<i64> = Some(std::process::id() as i64);
    let mut seen: Vec<i64> = Vec::new();
    for _ in 0..256 {
        let Some(pid) = current else { break };
        if seen.contains(&pid) {
            break;
        }
        seen.push(pid);
        if let Ok(candidate) = read_process_identity(pid) {
            if candidate == *identity {
                return Ok(());
            }
        }
        current = parent_pid(pid);
    }
    Err(format!(
        "{label} PID {} is not in the invoking process ancestry",
        identity.pid
    ))
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

#[derive(Clone)]
struct PlanItem {
    name: String,
    destination: String,
    repository: String,
    branch: String,
    start_point: String,
    remote: String,
    remote_url_sha256: String,
    landed_ref: String,
}

fn plan_obj(item: &PlanItem) -> Value {
    json!({
        "name": item.name,
        "destination": item.destination,
        "repository": item.repository,
        "branch": item.branch,
        "start_point": item.start_point,
        "remote": item.remote,
        "remote_url_sha256": item.remote_url_sha256,
        "landed_ref": item.landed_ref,
    })
}

fn checkout_obj(item: &PlanItem, head: &str) -> Value {
    json!({
        "name": item.name,
        "path": item.destination,
        "repository": item.repository,
        "branch": item.branch,
        "start_point": item.start_point,
        "remote": item.remote,
        "remote_url_sha256": item.remote_url_sha256,
        "landed_ref": item.landed_ref,
        "head": head,
        "containing_remote_refs": [],
        "vcs": "git",
    })
}

fn operation_id() -> String {
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn assert_target_clear(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    items: &[PlanItem],
) -> Result<()> {
    let document = crate::hold_write::active_document(root, cfg, selected)?;
    let mut occupied: Vec<String> = Vec::new();
    if let Some(slots) = document.get("slots").and_then(Value::as_object) {
        for slot in slots.values() {
            if let Some(checkouts) = slot.get("checkouts").and_then(Value::as_array) {
                for checkout in checkouts {
                    if let Some(path) = checkout.get("path").and_then(Value::as_str) {
                        occupied.push(path.to_string());
                    }
                }
            }
        }
    }
    for item in items {
        let target_abs = cfg.root.join(&item.destination);
        for existing in &occupied {
            let existing_abs = cfg.root.join(existing);
            if existing_abs == target_abs
                || existing_abs.starts_with(&target_abs)
                || target_abs.starts_with(&existing_abs)
            {
                return Err(format!(
                    "create target {} overlaps active checkout {existing}",
                    item.destination
                ));
            }
        }
        let repo_cwd = if PathBuf::from(&item.repository).is_absolute() {
            PathBuf::from(&item.repository)
        } else {
            cfg.root.join(&item.repository)
        };
        let listing = git(&repo_cwd, &["worktree", "list", "--porcelain"])?;
        for line in listing.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                if Path::new(path.trim()) == target_abs.as_path() {
                    return Err(format!(
                        "create target {} is already a registered worktree",
                        item.destination
                    ));
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    slot_type: &str,
    coordinator_authorized: bool,
    agent: &str,
    task: &str,
    purpose: &str,
    owner_pid: Option<i64>,
    coordinator_pid: i64,
    repos: &[String],
    remotes_in: &[String],
    remote_urls: &[String],
    branches_in: &[String],
    starts_in: &[String],
    override_disk_floor: bool,
    representation: &str,
    task_scope_unit: Option<&str>,
    task_scope_invocation_id: Option<&str>,
) -> Result<Value> {
    require_fixture()?;
    if !coordinator_authorized {
        return Err("worktree creation requires a coordinator assignment. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to assign the slot; an assigned agent may rerun with --coordinator-authorized and its own --owner-pid".to_string());
    }
    if representation == "image" {
        return Err(
            "slot-image provisioning stays with the Python edition in this slice".to_string(),
        );
    }
    let config = load_config(root, machine_override)?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    if machine_override.is_none() && std::env::var_os("WRKSLOTS_MACHINE").is_none() {
        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        let short = hostname.split('.').next().unwrap_or_default().to_string();
        if selected != short {
            return Err(format!(
                "refusing worktree creation: configured machine shard '{selected}' differs from current host '{short}', and no machine override was supplied"
            ));
        }
    }
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    crate::adopt_write::validate_agent_name(agent)?;
    if task.is_empty() || purpose.is_empty() {
        return Err("task and purpose must be non-empty".to_string());
    }
    let owner = owner_pid.map(read_process_identity).transpose()?;
    let coordinator = read_process_identity(coordinator_pid)?;
    let task_scope = match (task_scope_unit, task_scope_invocation_id) {
        (Some(unit), Some(invocation)) => {
            let Some(owner_ref) = owner.as_ref() else {
                return Err("task scope requires --owner-pid".to_string());
            };
            Some(crate::adopt_write::verify_task_scope(
                owner_ref, unit, invocation,
            )?)
        }
        (None, None) => None,
        _ => {
            return Err(
                "--task-scope-unit and --task-scope-invocation-id must be supplied together"
                    .to_string(),
            );
        }
    };
    // Authorization: owner in caller ancestry, else coordinator in
    // caller ancestry and owner descending from coordinator.
    let authorized = match &owner {
        Some(owner) => {
            assert_caller(owner, "owner").is_ok()
                || (assert_caller(&coordinator, "coordinator").is_ok()
                    && descends_from(owner, &coordinator))
        }
        None => assert_caller(&coordinator, "coordinator").is_ok(),
    };
    if !authorized {
        return Err(match &owner {
            Some(owner) => format!(
                "owner PID {} is not in the invoking process ancestry and does not descend from coordinator PID {}",
                owner.pid, coordinator.pid
            ),
            None => format!(
                "coordinator PID {} is not in the invoking process ancestry",
                coordinator.pid
            ),
        });
    }
    let _global = acquire_lock(
        &cfg.control.join("ACTIVE.global.lock"),
        &cfg.control.join("ACTIVE"),
    )?;
    let _machine = acquire_lock(
        &cfg.control.join(format!("ACTIVE.{selected}.json.lock")),
        &cfg.control.join(format!("ACTIVE.{selected}.json")),
    )?;
    let _config_lock = crate::hold_write::acquire_config_lock(&cfg)?;
    refuse_partial_state_allow_seals(&cfg.control)?;
    if !crate::heartbeat_write::seal_paths(&cfg).is_empty() {
        return Err(
            "validation-batch seals stay with the Python edition in this slice".to_string(),
        );
    }
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
    crate::ensure_states::ensure_states_locked(root, &cfg, &selected)?;
    if slot_type == "agent" && slot == "validate" {
        return Err(
            "slot name 'validate' is reserved for the validation representation directory"
                .to_string(),
        );
    }
    for machine in crate::global_state::discover_machines(&cfg.control)? {
        let archive = crate::archive_state::parse_archive(root, Some(&machine))?;
        if let Some(records) = archive.get("records").and_then(Value::as_array) {
            if records
                .iter()
                .any(|record| record.get("slot").and_then(Value::as_str) == Some(slot))
            {
                return Err(format!(
                    "finished slots are never reused; slot '{slot}' is already archived on {machine}"
                ));
            }
        }
        if machine != selected {
            let (other_active, _) = crate::active_state::parse_state_ordered(root, Some(&machine))?;
            if let Some(slots) = other_active.get("slots").and_then(Value::as_array) {
                if slots
                    .iter()
                    .any(|record| record.get("slot").and_then(Value::as_str) == Some(slot))
                {
                    return Err(format!("slot '{slot}' is already active on {machine}"));
                }
            }
        }
    }
    // Slot/agent free + cap.
    let active = crate::hold_write::active_document(root, &cfg, &selected)?;
    if let Some(slots) = active.get("slots").and_then(Value::as_array) {
        for record in slots {
            if record.get("slot").and_then(Value::as_str) == Some(slot) {
                return Err(format!("slot '{slot}' is already active on {selected}"));
            }
            if slot_type == "agent"
                && record.get("slot_type").and_then(Value::as_str) == Some("agent")
                && record.get("agent").and_then(Value::as_str) == Some(agent)
                && record
                    .get("import_source")
                    .map(Value::is_null)
                    .unwrap_or(true)
            {
                let other = record
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Err(format!(
                    "agent '{agent}' already owns slot '{other}' on {selected}"
                ));
            }
        }
        if let Some(cap) = config.get("max_active_slots").and_then(Value::as_i64) {
            if (slots.len() as i64) >= cap {
                return Err(format!(
                    "machine {selected} already holds {} active slot(s) against max_active_slots={cap}; finish or remove a slot before allocating another",
                    slots.len()
                ));
            }
        }
    }
    // Disk floors (active only when all three thresholds are set).
    let advisory = config.get("disk_advisory_bytes").and_then(Value::as_i64);
    let emergency = config.get("disk_emergency_bytes").and_then(Value::as_i64);
    let provisioning = config
        .get("disk_provisioning_floor_bytes")
        .and_then(Value::as_i64);
    if let (Some(advisory_floor), Some(emergency_floor), Some(provisioning_floor)) =
        (advisory, emergency, provisioning)
    {
        let managed = crate::storage_state::managed_root(&cfg);
        let free = free_bytes(&managed)?;
        if free < advisory_floor {
            eprintln!(
                "WARNING: free space {:.1} GiB is below the advisory floor {:.1} GiB",
                free as f64 / 1073741824.0,
                advisory_floor as f64 / 1073741824.0
            );
        }
        if free < emergency_floor {
            return Err(format!(
                "free space {:.1} GiB is below the emergency floor {:.1} GiB; stop builds, publish durable work, and run 'wrkslots audit' and 'wrkslots clean-caches', then retry. --override-disk-floor does not bypass the emergency floor",
                free as f64 / 1073741824.0,
                emergency_floor as f64 / 1073741824.0
            ));
        }
        if free < provisioning_floor {
            if override_disk_floor {
                eprintln!(
                    "WARNING: --override-disk-floor accepted {:.1} GiB free against the provisioning floor {:.1} GiB",
                    free as f64 / 1073741824.0,
                    provisioning_floor as f64 / 1073741824.0
                );
            } else {
                return Err(format!(
                    "free space {:.1} GiB is below the provisioning floor {:.1} GiB; run 'wrkslots audit' and 'wrkslots clean-caches', then retry. Use --override-disk-floor only after inspecting the pressure",
                    free as f64 / 1073741824.0,
                    provisioning_floor as f64 / 1073741824.0
                ));
            }
        }
    }
    let slot_path = slot_directory(&cfg, slot, slot_type)?;
    if slot_path.exists() || slot_path.is_symlink() {
        return Err(format!("slot path already exists: {}", slot_path.display()));
    }
    // Plan.
    let repositories = assignments(repos, "repository")?;
    let remotes = assignments(remotes_in, "remote")?;
    let trusted_urls = assignments(remote_urls, "trusted remote URL")?;
    let mut branches = assignments(branches_in, "branch")?;
    let starts = assignments(starts_in, "start point")?;
    if repositories.is_empty() {
        return Err("create requires at least one --repo NAME=PATH".to_string());
    }
    if cfg.layout == "flat" && repositories.len() != 1 {
        return Err("flat layout requires exactly one repository per slot".to_string());
    }
    if slot_type == "validate" && branches.is_empty() {
        for name in repositories.keys() {
            branches.insert(
                name.clone(),
                format!("wrkslots/validate/{selected}/{slot}/{name}"),
            );
        }
    }
    if branches.keys().collect::<Vec<_>>() != repositories.keys().collect::<Vec<_>>() {
        return Err("branches must exactly match repositories".to_string());
    }
    for (mapping, label) in [
        (&starts, "start point"),
        (&remotes, "remote"),
        (&trusted_urls, "trusted remote URL"),
    ] {
        for name in mapping.keys() {
            if !repositories.contains_key(name) {
                return Err(format!("{label} supplied for unknown repository: {name}"));
            }
        }
    }
    let default_remote = config
        .get("default_remote")
        .and_then(Value::as_str)
        .unwrap_or("origin");
    let default_landed = config
        .get("default_landed_ref")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let landed_prefix = format!("refs/remotes/{default_remote}/");
    if !default_landed.starts_with(&landed_prefix) {
        return Err(format!(
            "default_landed_ref must be under the configured default remote {default_remote}"
        ));
    }
    let mut plan: Vec<PlanItem> = Vec::new();
    let mut commons: Vec<PathBuf> = Vec::new();
    for (name, raw_repository) in &repositories {
        if raw_repository.starts_with('/') {
            return Err(format!(
                "repository path must be relative to the parent of the project root: {raw_repository}"
            ));
        }
        let (stored_repository, repository, _resolved) =
            crate::active_state::stored_repository_parts(&cfg, raw_repository)?;
        let top = git(&repository, &["rev-parse", "--show-toplevel"])?;
        if top != repository {
            return Err(format!(
                "repository path is not its Git worktree root: {}",
                repository.display()
            ));
        }
        let common = git(&repository, &["rev-parse", "--git-common-dir"])?;
        let common_path = if Path::new(&common).is_absolute() {
            PathBuf::from(common)
        } else {
            repository.join(common)
        };
        if commons.contains(&common_path) {
            return Err("the same Git repository was supplied more than once".to_string());
        }
        commons.push(common_path);
        let branch = branches.get(name).cloned().unwrap_or_default();
        if !git_ok(&repository, &["check-ref-format", "--branch", &branch]) {
            return Err(format!("invalid branch name: {branch}"));
        }
        if git_ok(
            &repository,
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
        ) {
            return Err(format!("local branch already exists: {branch}"));
        }
        let remote = remotes
            .get(name)
            .map(|value| value.as_str())
            .unwrap_or(default_remote);
        let landed_ref = default_landed.replacen(
            "refs/remotes/origin/",
            &format!("refs/remotes/{remote}/"),
            1,
        );
        if !landed_ref.starts_with(&format!("refs/remotes/{remote}/")) {
            return Err(format!(
                "landed ref for {name} must be under refs/remotes/{remote}/"
            ));
        }
        let url = git(&repository, &["remote", "get-url", remote])?;
        if let Some(trusted) = trusted_urls.get(name) {
            if trusted != &url {
                return Err(format!(
                    "remote '{remote}' URL differs from trusted provisioning input"
                ));
            }
        }
        let url_sha = format!("{:x}", Sha256::digest(url.as_bytes()));
        if slot_type != "validate" || !starts.contains_key(name) {
            git(
                &repository,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-auto-maintenance",
                    remote,
                    &format!("+refs/heads/*:refs/remotes/{remote}/*"),
                ],
            )?;
        }
        let start_ref = starts
            .get(name)
            .map(|value| value.as_str())
            .unwrap_or(&landed_ref);
        let start_point = git(
            &repository,
            &["rev-parse", "--verify", &format!("{start_ref}^{{commit}}")],
        )?;
        let slot_dir = slot_directory(&cfg, slot, slot_type)?;
        let destination_abs = if cfg.layout == "flat" {
            slot_dir.clone()
        } else {
            slot_dir.join(name)
        };
        let destination_relative = destination_abs
            .strip_prefix(&cfg.root)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| destination_abs.to_string_lossy().into_owned());
        let destination = cfg.root.join(&destination_relative);
        if destination.exists() || destination.is_symlink() {
            return Err(format!(
                "checkout destination already exists: {}",
                destination.display()
            ));
        }
        // Target-clear: branch not registered to another worktree.
        let listed = git(&repository, &["worktree", "list", "--porcelain"])?;
        for line in listed.lines() {
            if line == format!("branch refs/heads/{branch}") {
                return Err(format!(
                    "branch {branch} is already checked out in another worktree"
                ));
            }
        }
        plan.push(PlanItem {
            name: name.clone(),
            destination: destination_relative,
            repository: stored_repository.clone(),
            branch,
            start_point,
            remote: remote.to_string(),
            remote_url_sha256: url_sha,
            landed_ref,
        });
    }
    plan.sort_by(|left, right| left.name.cmp(&right.name));
    assert_target_clear(root, &cfg, &selected, &plan)?;
    // Journal + worktrees.
    if let Some(parent) = slot_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create slot parent {}: {error}", parent.display()))?;
    }
    let created_at = utc_now();
    let ttl = config
        .get("heartbeat_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(60);
    let hooks: Vec<String> = config
        .get("post_provision_hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let journal_path = cfg.control.join(format!(
        "CREATE.{}.{}.{}.{}.journal",
        selected.chars().count(),
        selected,
        slot.chars().count(),
        slot
    ));
    let mut created: Vec<Value> = Vec::new();
    let op_id = operation_id();
    let make_journal = |created: &Vec<Value>,
                        heartbeat_at: &str,
                        hook_progress: i64,
                        hook_failure: &Value|
     -> Value {
        json!({
            "schema": 2,
            "kind": "create",
            "machine": selected,
            "slot": slot,
            "agent": agent,
            "task": task,
            "purpose": purpose,
            "slot_type": slot_type,
            "created_at": created_at,
            "heartbeat_at": heartbeat_at,
            "heartbeat_ttl_seconds": ttl,
            "owner": owner.as_ref().map(identity_value).unwrap_or(Value::Null),
            "coordinator_lease": identity_value(&coordinator),
            "planned": plan.iter().map(plan_obj).collect::<Vec<_>>(),
            "created": created.clone(),
            "post_provision_hooks": hooks,
            "hook_progress": hook_progress,
            "hook_failure": hook_failure.clone(),
            "task_scope": task_scope.clone().unwrap_or(Value::Null),
            "failure_policy": "leave-for-inspection",
            "operation_id": op_id,
        })
    };
    ensure_event_log(root, &cfg, &selected)?;
    let write_journal = |cfg: &Cfg, payload: &Value| -> Result<()> {
        append_event(
            cfg,
            &selected,
            "operation-progress-recorded",
            json!({
                "slot": slot,
                "operation": "create",
                "journal_path": journal_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                "journal": payload,
            }),
        )?;
        atomic_write_json(&journal_path, payload)
    };
    let mut heartbeat_at = created_at.clone();
    write_journal(
        &cfg,
        &make_journal(&created, &heartbeat_at, 0, &Value::Null),
    )?;
    if cfg.layout == "nested" {
        std::fs::create_dir_all(&slot_path).map_err(|error| {
            format!(
                "cannot create slot directory {}: {error}",
                slot_path.display()
            )
        })?;
    }
    for item in &plan {
        let (_, repository, _) =
            crate::active_state::stored_repository_parts(&cfg, &item.repository)?;
        let destination = cfg.root.join(&item.destination);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create checkout parent {}: {error}",
                    parent.display()
                )
            })?;
        }
        git(
            &repository,
            &[
                "worktree",
                "add",
                "-b",
                &item.branch,
                &destination.to_string_lossy(),
                &item.start_point,
            ],
        )?;
        let head = git(&destination, &["rev-parse", "HEAD"])?;
        created.push(checkout_obj(item, &head));
        write_journal(
            &cfg,
            &make_journal(&created, &heartbeat_at, 0, &Value::Null),
        )?;
    }
    // Post-provision hooks.
    let total = (created.len() * hooks.len()) as i64;
    let mut progress: i64 = 0;
    for (checkout_index, checkout) in created.iter().enumerate() {
        let checkout_name = checkout
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let path = cfg.root.join(
            checkout
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        for (hook_index, hook) in hooks.iter().enumerate() {
            let running = json!({
                "checkout": checkout_name,
                "hook_index": hook_index,
                "command": hook,
                "status": "running",
            });
            write_journal(
                &cfg,
                &make_journal(&created, &heartbeat_at, progress, &running),
            )?;
            let output = Command::new("/bin/sh")
                .args(["-c", hook])
                .current_dir(&path)
                .env(
                    "WRKSLOTS_PROJECT_ROOT",
                    cfg.root.to_string_lossy().into_owned(),
                )
                .env(
                    "WRKSLOTS_SLOT",
                    if cfg.layout == "nested" {
                        slot.to_string()
                    } else {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    },
                )
                .env("WRKSLOTS_CHECKOUT", path.to_string_lossy().into_owned())
                .env("WRKSLOTS_CHECKOUT_NAME", &checkout_name)
                .output();
            let failed = match &output {
                Ok(output) if output.status.success() => None,
                Ok(output) => Some(json!({
                    "checkout": checkout_name,
                    "hook_index": hook_index,
                    "command": hook,
                    "status": "failed",
                    "returncode": output.status.code().unwrap_or(-1),
                    "detail": String::from_utf8_lossy(&output.stderr).trim().chars().take(512).collect::<String>(),
                })),
                Err(error) => Some(json!({
                    "checkout": checkout_name,
                    "hook_index": hook_index,
                    "command": hook,
                    "status": "failed-to-start",
                    "detail": error.to_string(),
                })),
            };
            if let Some(failure) = failed {
                let detail = failure
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                write_journal(
                    &cfg,
                    &make_journal(&created, &heartbeat_at, progress, &failure),
                )?;
                return Err(format!(
                    "post-provision hook {hook_index} for checkout {checkout_name} failed: {detail}. state: PRESERVED -- worktrees were left in place and the create journal records the failure. remedy: fix the hook, then run 'wrkslots recover' to inspect or retry"
                ));
            }
            progress += 1;
            let _ = checkout_index;
        }
    }
    heartbeat_at = utc_now();
    write_journal(
        &cfg,
        &make_journal(&created, &heartbeat_at, total, &Value::Null),
    )?;
    // Re-verify process generations and checkout identity before
    // publication (the journal stays for recovery on refusal).
    if let Some(owner_ref) = &owner {
        let now = read_process_identity(owner_ref.pid)?;
        if now != *owner_ref {
            return Err(
                "owner process generation changed during create; recovery journal is preserved"
                    .to_string(),
            );
        }
        assert_caller(owner_ref, "owner")?;
    }
    let coordinator_now = read_process_identity(coordinator.pid)?;
    if coordinator_now != coordinator {
        return Err(
            "coordinator process generation changed during create; recovery journal is preserved"
                .to_string(),
        );
    }
    for (item, checkout) in plan.iter().zip(created.iter()) {
        let destination = cfg.root.join(&item.destination);
        let head = git(&destination, &["rev-parse", "HEAD"])?;
        let recorded = checkout
            .get("head")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if head != recorded {
            return Err(format!(
                "checkout identity changed for {}; recovery journal is preserved",
                item.name
            ));
        }
        let branch_ref = git(&destination, &["symbolic-ref", "HEAD"])?;
        if branch_ref != format!("refs/heads/{}", item.branch) {
            return Err(format!(
                "checkout branch changed for {}; recovery journal is preserved",
                item.name
            ));
        }
        let url = git(&destination, &["remote", "get-url", &item.remote])?;
        if format!("{:x}", Sha256::digest(url.as_bytes())) != item.remote_url_sha256 {
            return Err(format!(
                "checkout remote changed for {}; recovery journal is preserved",
                item.name
            ));
        }
    }
    // Register the record.
    let state = crate::hold_write::active_document(root, &cfg, &selected)?;
    let previous_revision = state.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let record = json!({
        "slot": slot,
        "agent": agent,
        "task": task,
        "purpose": purpose,
        "slot_type": slot_type,
        "machine": selected,
        "generation": 1,
        "created_at": created_at,
        "heartbeat_at": heartbeat_at,
        "heartbeat_ttl_seconds": ttl,
        "owner": owner.as_ref().map(identity_value).unwrap_or(Value::Null),
        "coordinator_lease": identity_value(&coordinator),
        "coordinator_recovery_note": Value::Null,
        "handoff": Value::Null,
        "checkouts": created,
    });
    let record = if let Some(scope) = &task_scope {
        let mut record = record;
        if let Some(object) = record.as_object_mut() {
            object.insert("task_scope".to_string(), scope.clone());
        }
        record
    } else {
        record
    };
    let mut slots: Vec<Value> = state
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    slots.push(record.clone());
    slots.sort_by(|left, right| {
        left.get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "slot-created",
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
    let mut document = state.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    append_event(
        &cfg,
        &selected,
        "operation-completed",
        json!({
            "slot": slot,
            "operation": "create",
            "journal_path": journal_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        }),
    )?;
    if journal_path.exists() || journal_path.is_symlink() {
        std::fs::remove_file(&journal_path).map_err(|error| {
            format!("cannot remove journal {}: {error}", journal_path.display())
        })?;
    }
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "slot_type": slot_type,
        "agent": agent,
        "generation": 1,
        "owner_process": if owner.is_some() { "bound" } else { "unbound" },
        "checkouts": record.get("checkouts").cloned().unwrap_or(json!([])),
    }))
}

fn free_bytes(path: &Path) -> Result<i64> {
    let mut command = Command::new("df");
    command.args(["-B1", "--output=avail"]).arg(path);
    let output = command
        .output()
        .map_err(|error| format!("cannot measure free space at {}: {error}", path.display()))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(1)
        .and_then(|line| line.trim().parse().ok())
        .ok_or_else(|| format!("cannot measure free space at {}", path.display()))
}
