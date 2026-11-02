//! Reader: classify outstanding create journals (`classify-create-journals`).
//!
//! Reproduces the Python edition's read-only classification: every
//! outstanding journal must be a create journal whose target is
//! unrelated to the requested slot, with process liveness, planned /
//! created checkout agreement, and worktree dirtiness inspected.
//! No locks, authorization, or writes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::active_state::{as_int, as_list, as_mapping, as_str, load_cfg, slot_directory, Cfg};
use crate::batch_text::sort_keys;
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::refuse_partial_state_allow_seals;
use crate::git_storage_state::git_run;
use crate::global_state::load_event_chain;
use crate::journal_state::validate_journal_shape;
use crate::status_state::process_state;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Row {
    machine: String,
    slot: String,
    agent: String,
    slot_type: String,
    slot_path: PathBuf,
    planned: Vec<Value>,
    created: Vec<Value>,
    observed: Vec<Value>,
    dirty: Vec<String>,
    owner_detail: String,
    owner_state: String,
    coordinator_detail: String,
    coordinator_state: String,
    state: String,
    journal: PathBuf,
    identity: FileIdentity,
}

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

type FileIdentity = (u64, u64, u64, String);

fn read_identity(path: &Path) -> Result<(Vec<u8>, FileIdentity)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("cannot read recovery journal {}: {e}", path.display()))?;
    if meta.len() > 1024 * 1024 {
        return Err(format!("recovery journal {} is too large", path.display()));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("cannot read recovery journal {}: {e}", path.display()))?;
    let digest = {
        use sha2::{Digest as _, Sha256};
        format!("{:x}", Sha256::digest(&bytes))
    };
    Ok((bytes, (meta.dev(), meta.ino(), meta.len(), digest)))
}

fn strict_obj(bytes: &[u8]) -> Result<Map<String, Value>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "recovery journal is not valid UTF-8".to_string())?;
    let value: Value = serde_json::from_str(text)
        .map_err(|e| format!("recovery journal is malformed JSON: {e}"))?;
    as_mapping(&value, "recovery journal").cloned()
}

fn pending_ops(control: &Path, machine: &str) -> Result<Vec<(String, Value)>> {
    let events = load_event_chain(&control.join(format!("EVENTS.{machine}")), machine)?;
    let mut pending: Vec<(String, Value)> = Vec::new();
    for event in &events {
        let payload = as_mapping(
            event.get("payload").unwrap_or(&Value::Null),
            "append-only event.payload",
        )?;
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        if kind == "operation-progress-recorded" {
            let journal = payload.get("journal").cloned().unwrap_or(Value::Null);
            let name = payload
                .get("journal_path")
                .and_then(Value::as_str)
                .unwrap_or("ACTIVE.journal")
                .to_string();
            if let Some(slot) = pending.iter_mut().find(|(n, _)| *n == name) {
                slot.1 = journal;
            } else {
                pending.push((name, journal));
            }
        } else if kind == "operation-completed" {
            let name = payload
                .get("journal_path")
                .and_then(Value::as_str)
                .unwrap_or("ACTIVE.journal")
                .to_string();
            if let Some(index) = pending.iter().position(|(n, _)| *n == name) {
                pending.remove(index);
            }
        }
    }
    Ok(pending)
}

fn outstanding(control: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(control) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with("ACTIVE.")
                || name.starts_with("CREATE.")
                || name.starts_with("FINISH."))
                && name.ends_with(".journal")
            {
                paths.insert(entry.path());
            }
            if name.starts_with("EVENTS.") && entry.path().is_dir() {
                let machine = name.trim_start_matches("EVENTS.").to_string();
                for (journal_name, _) in pending_ops(control, &machine)? {
                    paths.insert(control.join(journal_name));
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn create_journal_path(cfg: &Cfg, slot: &str, machine: &str) -> PathBuf {
    let machine_part = format!("{}.{}", machine.chars().count(), machine);
    let slot_part = format!("{}.{}", slot.chars().count(), slot);
    cfg.control
        .join(format!("CREATE.{machine_part}.{slot_part}.journal"))
}

/// Episode machine over append-only progress/completion evidence for
/// one scoped create journal path (Python `_scoped_operation_journal_completed`).
fn scoped_completed(cfg: &Cfg, path: &Path, raw: &Map<String, Value>) -> Result<bool> {
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        "create journal.machine",
    )?;
    let slot = validate_name(
        as_str(
            raw.get("slot").unwrap_or(&Value::Null),
            "create journal.slot",
        )?,
        "slot",
    )?;
    if path != create_journal_path(cfg, &slot, machine) {
        return Err("scoped create journal filename does not match its target".to_string());
    }
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{machine}")), machine)?;
    let mut recorded: Option<Value> = None;
    let mut completed = false;
    let mut current: Option<String> = None;
    let mut retired: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for event in &events {
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        if kind != "operation-progress-recorded" && kind != "operation-completed" {
            continue;
        }
        let payload = as_mapping(
            event.get("payload").unwrap_or(&Value::Null),
            "append-only event.payload",
        )?;
        let event_name = match payload.get("journal_path").and_then(Value::as_str) {
            Some(name) => name.to_string(),
            None => {
                let operation = payload
                    .get("operation")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let event_slot = payload
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if operation == "create" {
                    format!(
                        "CREATE.{}.{}.{}.{}.journal",
                        machine.chars().count(),
                        machine,
                        event_slot.chars().count(),
                        event_slot
                    )
                } else if operation == "finish" {
                    format!(
                        "FINISH.{}.{}.{}.{}.journal",
                        machine.chars().count(),
                        machine,
                        event_slot.chars().count(),
                        event_slot
                    )
                } else {
                    format!("ACTIVE.{machine}.journal")
                }
            }
        };
        if cfg.control.join(&event_name) != path {
            continue;
        }
        if kind == "operation-progress-recorded" {
            let journal = payload.get("journal").cloned().unwrap_or(Value::Null);
            let identity = journal
                .get("operation_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let opens = journal
                .get("created")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
                && journal.get("hook_progress").and_then(Value::as_i64) == Some(0);
            if completed && !(opens && identity.is_some()) {
                return Err(
                    "scoped create journal path was reused after completion without a new operation"
                        .to_string(),
                );
            }
            if let Some(id) = &identity {
                if retired.contains(id) {
                    return Err(
                        "scoped create journal path was reopened with the identity of a completed operation"
                            .to_string(),
                    );
                }
            }
            if recorded.is_none() || completed {
                current = identity.clone();
            } else if identity != current {
                return Err(
                    "scoped create journal progress changed its operation identity".to_string(),
                );
            }
            recorded = Some(journal);
            completed = false;
        } else {
            if recorded.is_none() {
                return Err(
                    "scoped create completion has no preceding progress evidence".to_string(),
                );
            }
            if let Some(id) = &current {
                retired.insert(id.clone());
            }
            completed = true;
        }
    }
    let Some(recorded) = recorded else {
        return Err(
            "scoped create journal has no matching append-only progress evidence".to_string(),
        );
    };
    if recorded != Value::Object(raw.clone()) {
        return Err("scoped create journal differs from append-only progress evidence".to_string());
    }
    Ok(completed)
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    git_run(repo, args)
}

fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let output = git_run(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    );
    match output {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn listed_worktrees(repo: &Path) -> Result<BTreeSet<PathBuf>> {
    let out = git_text(repo, &["worktree", "list", "--porcelain"])?;
    let mut set = BTreeSet::new();
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            set.insert(PathBuf::from(path));
        }
    }
    Ok(set)
}

fn expected_checkout(
    cfg: &Cfg,
    slot: &str,
    name: &str,
    slot_type: &str,
) -> Result<(String, PathBuf)> {
    let dir = slot_directory(cfg, slot, slot_type)?;
    let destination = if cfg.layout == "flat" {
        dir.clone()
    } else {
        dir.join(name)
    };
    let relative = destination
        .strip_prefix(&cfg.root)
        .unwrap_or(&destination)
        .to_string_lossy()
        .replace('\\', "/");
    Ok((relative, destination))
}

fn field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn classify_one(
    cfg: &Cfg,
    config: &Value,
    path: &Path,
    states: &[(String, Vec<Value>)],
    archives: &[Vec<Value>],
) -> Result<Row> {
    if !path.exists() && path.symlink_metadata().is_err() {
        return Err(format!(
            "recovery journal {} is missing while append-only history still names it",
            path.display()
        ));
    }
    let (bytes, identity) = read_identity(path)?;
    let raw = strict_obj(&bytes)?;
    let (machine_s, kind, _slot_s) = validate_journal_shape(
        cfg,
        path.file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default(),
        &raw,
    )?;
    if kind != "create" {
        return Err(format!(
            "interrupted {kind:?} mutation {} cannot coexist with an operation that permits only verified disjoint create journals",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    // Provenance: scoped create completion from append-only episodes.
    let completed = scoped_completed(cfg, path, &raw)?;
    let owner = crate::status_state::identity_from_value(raw.get("owner").unwrap_or(&Value::Null));
    let coordinator = crate::status_state::identity_from_value(
        raw.get("coordinator_lease").unwrap_or(&Value::Null),
    );
    let (owner_state, owner_detail) = process_state(owner.as_ref());
    let (coordinator_state, coordinator_detail) = process_state(coordinator.as_ref());
    if !completed {
        if owner_state == "indeterminate" || coordinator_state == "indeterminate" {
            return Err(format!(
                "create journal for slot {:?} process state is indeterminate: owner={owner_state} ({owner_detail}); coordinator={coordinator_state} ({coordinator_detail})",
                raw.get("slot").and_then(Value::as_str).unwrap_or_default()
            ));
        }
        if owner_state != coordinator_state || !matches!(owner_state.as_str(), "live" | "dead") {
            return Err(format!(
                "create journal for slot {:?} has disagreeing process state: owner={owner_state} ({owner_detail}); coordinator={coordinator_state} ({coordinator_detail})",
                raw.get("slot").and_then(Value::as_str).unwrap_or_default()
            ));
        }
    }
    let machine = machine_s;
    let slot = field(&Value::Object(raw.clone()), "slot");
    let agent = validate_name(&field(&Value::Object(raw.clone()), "agent"), "agent")?.to_string();
    let slot_type = raw
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    if !matches!(slot_type.as_str(), "agent" | "validate") {
        return Err("create journal has an invalid slot type".to_string());
    }
    if !completed
        && states
            .iter()
            .any(|(_, slots)| slots.iter().any(|r| field(r, "slot") == slot))
    {
        return Err(format!(
            "create journal for slot {slot:?} has already reached ACTIVE publication"
        ));
    }
    if archives
        .iter()
        .any(|records| records.iter().any(|r| field(r, "slot") == slot))
    {
        return Err(format!(
            "create journal for slot {slot:?} conflicts with an archive"
        ));
    }
    let planned = as_list(
        raw.get("planned").unwrap_or(&Value::Null),
        "create journal.planned",
    )?
    .clone();
    let created = as_list(
        raw.get("created").unwrap_or(&Value::Null),
        "create journal.created",
    )?
    .clone();
    if planned.is_empty() || {
        let mut names: Vec<String> = planned.iter().map(|p| field(p, "name")).collect();
        names.sort();
        names.dedup();
        names.len() != planned.len()
    } {
        return Err("create journal has an empty or duplicate checkout plan".to_string());
    }
    {
        let mut names: Vec<String> = created.iter().map(|p| field(p, "name")).collect();
        let planned_names: Vec<String> = planned.iter().map(|p| field(p, "name")).collect();
        if names.len() != created.len() && {
            names.sort();
            names.dedup();
            true
        } {
            return Err("create journal has duplicate completed checkouts".to_string());
        }
        let prefix: Vec<String> = planned_names.iter().take(created.len()).cloned().collect();
        let created_names: Vec<String> = created.iter().map(|p| field(p, "name")).collect();
        if created_names != prefix {
            return Err(
                "create journal's completed checkouts are not a planned prefix".to_string(),
            );
        }
    }
    let hooks: Vec<String> = config
        .get("post_provision_hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let journal_hooks: Vec<String> = raw
        .get("post_provision_hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if journal_hooks != hooks {
        return Err("create journal post-provision hooks differ from configuration".to_string());
    }
    if raw.get("failure_policy").and_then(Value::as_str) != Some("leave-for-inspection") {
        return Err("create journal has an unsupported hook failure policy".to_string());
    }
    let progress = as_int(
        raw.get("hook_progress").unwrap_or(&Value::Null),
        "create journal.hook_progress",
        0,
    )?;
    if progress > (created.len() * hooks.len()) as i64 {
        return Err("create journal ran hooks for an unrecorded checkout".to_string());
    }
    let slot_path = slot_directory(cfg, &slot, &slot_type)?;
    let mut observed: Vec<Value> = Vec::new();
    let mut dirty: Vec<String> = Vec::new();
    let mut expected_entries: BTreeSet<String> = BTreeSet::new();
    for item in &planned {
        let name = field(item, "name");
        let (expected_path, destination) = expected_checkout(cfg, &slot, &name, &slot_type)?;
        if field(item, "destination") != expected_path {
            return Err(format!(
                "create journal destination for {name} does not match slot {slot}"
            ));
        }
        let (_rel, repository) =
            crate::finish_write::stored_repository_path(cfg, &field(item, "repository"))?;
        if field(item, "landed_ref")
            != crate::active_state::landed_ref_for_remote(cfg, &field(item, "remote"))?
        {
            return Err(format!(
                "create journal target for {name} differs from configured authority"
            ));
        }
        // Remote digest census.
        let digest = {
            use sha2::{Digest as _, Sha256};
            let url = git_text(&repository, &["remote", "get-url", &field(item, "remote")])
                .unwrap_or_default();
            format!("{:x}", Sha256::digest(url.trim().as_bytes()))
        };
        if digest != field(item, "remote_url_sha256") {
            return Err(format!("create journal remote changed for {name}"));
        }
        let recorded = created.iter().find(|c| field(c, "name") == name);
        let Some(recorded) = recorded else {
            if destination.exists() || destination.symlink_metadata().is_ok() {
                return Err(format!(
                    "create journal has an unrecorded path at {}",
                    destination.display()
                ));
            }
            if listed_worktrees(&repository)?.contains(&destination) {
                return Err(format!(
                    "create journal has an unrecorded Git worktree at {}",
                    destination.display()
                ));
            }
            if branch_exists(&repository, &field(item, "branch"))? {
                return Err(format!(
                    "create journal has an unrecorded branch {:?}",
                    field(item, "branch")
                ));
            }
            continue;
        };
        let expected = (
            field(item, "destination"),
            field(item, "repository"),
            field(item, "branch"),
            field(item, "start_point"),
            field(item, "remote"),
            field(item, "remote_url_sha256"),
            field(item, "landed_ref"),
            field(item, "start_point"),
        );
        let actual = (
            field(recorded, "path"),
            field(recorded, "repository"),
            field(recorded, "branch"),
            field(recorded, "start_point"),
            field(recorded, "remote"),
            field(recorded, "remote_url_sha256"),
            field(recorded, "landed_ref"),
            field(recorded, "head"),
        );
        if actual != expected {
            return Err(format!(
                "create journal's completed checkout {name} differs from its plan"
            ));
        }
        if destination
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
            || !destination.is_dir()
        {
            return Err(format!(
                "create journal checkout is missing or unsafe: {}",
                destination.display()
            ));
        }
        let observed_head =
            crate::finish_write::verify_existing_worktree(&repository, &destination)?;
        let observed_branch = git_text(
            &destination,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )
        .map_err(|_| {
            format!(
                "checkout must be on its registered branch, not detached: {}",
                destination.display()
            )
        })?;
        if observed_branch.trim() != field(recorded, "branch") {
            return Err(format!(
                "create journal checkout {name} changed its registered branch"
            ));
        }
        let mut entry = recorded.clone();
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("head".to_string(), Value::String(observed_head));
            obj.insert("containing_remote_refs".to_string(), json!([]));
        }
        observed.push(entry);
        // Dirtiness with the configuration's cache-glob exclusions.
        let globs = crate::finish_write::cache_globs_for(config, &name);
        let mut args: Vec<String> = vec![
            "status".into(),
            "--porcelain=v2".into(),
            "--untracked-files=all".into(),
            "--ignored=matching".into(),
        ];
        args.push(".".into());
        for pattern in &globs {
            args.push(format!(":(exclude,glob){pattern}"));
            args.push(format!(":(exclude,glob){pattern}/**"));
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let status = git_text(&destination, &arg_refs)?;
        if !status.trim().is_empty() {
            dirty.push(name.clone());
        }
        expected_entries.insert(name);
    }
    if slot_path.exists() || slot_path.symlink_metadata().is_ok() {
        if slot_path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
            || !slot_path.is_dir()
        {
            return Err(format!(
                "create journal slot path is unsafe: {}",
                slot_path.display()
            ));
        }
        if cfg.layout == "nested" {
            let entries: BTreeSet<String> = std::fs::read_dir(&slot_path)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            if entries != expected_entries {
                return Err(format!(
                    "create journal slot {slot:?} contains unrecorded paths"
                ));
            }
        }
    } else if !created.is_empty() {
        return Err(format!(
            "create journal has checkouts below missing slot {slot:?}"
        ));
    }
    for (state_machine, slots) in states {
        for record in slots {
            if completed && *state_machine == machine && field(record, "slot") == slot {
                continue;
            }
            let active_path = slot_directory(
                cfg,
                &field(record, "slot"),
                {
                    let t = field(record, "slot_type");
                    if t.is_empty() {
                        "agent".to_string()
                    } else {
                        t
                    }
                }
                .as_str(),
            )?;
            if within(&slot_path, &active_path) || within(&active_path, &slot_path) {
                return Err(format!(
                    "create journal slot {slot:?} overlaps active slot {state_machine}/{}",
                    field(record, "slot")
                ));
            }
        }
    }
    // Stable re-read: file must not have changed while classified.
    let (rebound, rebound_identity) = read_identity(path)?;
    if rebound_identity != identity || strict_obj(&rebound)? != raw {
        return Err(format!(
            "create journal changed while it was classified: {}",
            path.display()
        ));
    }
    let state = if completed {
        "completed-create".to_string()
    } else {
        format!("{owner_state}-incomplete-create")
    };
    Ok(Row {
        machine,
        slot,
        agent,
        slot_type,
        slot_path,
        planned,
        created,
        observed,
        dirty,
        owner_detail,
        owner_state,
        coordinator_detail,
        coordinator_state,
        state,
        journal: path.to_path_buf(),
        identity,
    })
}

/// Classify outstanding create journals for the requested slot.
pub(crate) fn classify_create_journals(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    slot_type: &str,
    agent: &str,
    format_json: bool,
) -> Result<String> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    if !matches!(slot_type, "agent" | "validate") {
        return Err("slot type must be one of agent, validate".to_string());
    }
    let slot = validate_name(slot, "slot")?.to_string();
    let agent = validate_name(agent, "agent")?.to_string();
    refuse_partial_state_allow_seals(&cfg.control)?;
    crate::hold_write::validate_global(root, &cfg)?;
    // Load every machine's ACTIVE/ARCHIVED documents.
    let mut states: Vec<(String, Vec<Value>)> = Vec::new();
    let mut archives: Vec<Vec<Value>> = Vec::new();
    for machine in crate::global_state::discover_machines(&cfg.control)? {
        let active_path = cfg.control.join(format!("ACTIVE.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&active_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                states.push((
                    machine.clone(),
                    doc.get("slots")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        let archive_path = cfg.control.join(format!("ARCHIVED.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&archive_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                archives.push(
                    doc.get("records")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
    }
    let paths = outstanding(&cfg.control)?;
    let mut rows: Vec<Row> = Vec::new();
    for path in &paths {
        // Only create journals are classified; other kinds refuse in classify_one.
        rows.push(classify_one(&cfg, &config, path, &states, &archives)?);
    }
    {
        let mut seen_slots: BTreeSet<(String, String)> = BTreeSet::new();
        let mut seen_agents: BTreeSet<String> = BTreeSet::new();
        for row in &rows {
            if !seen_slots.insert((row.machine.clone(), row.slot.clone())) {
                return Err(format!(
                    "multiple interrupted creates claim {}/{}",
                    row.machine, row.slot
                ));
            }
            if row.slot_type == "agent" && !seen_agents.insert(row.agent.clone()) {
                return Err(format!(
                    "multiple interrupted creates claim agent {:?}",
                    row.agent
                ));
            }
        }
    }
    let target = slot_directory(&cfg, &slot, slot_type)?;
    for row in &rows {
        if row.slot == slot {
            return Err(format!(
                "interrupted create already targets slot {slot:?}; recover that exact slot"
            ));
        }
        if slot_type == "agent" && row.slot_type == "agent" && row.agent == agent {
            return Err(format!(
                "interrupted create already belongs to agent {agent:?}; recover it first"
            ));
        }
        if within(&target, &row.slot_path) || within(&row.slot_path, &target) {
            return Err(format!(
                "requested slot path {} overlaps interrupted create {}/{}",
                target.display(),
                row.machine,
                row.slot
            ));
        }
    }
    let journals: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "agent": row.agent,
                "checkouts": row.observed.iter().map(|c| json!({
                    "branch": field(c, "branch"),
                    "dirty": row.dirty.contains(&field(c, "name")),
                    "head": field(c, "head"),
                    "name": field(c, "name"),
                    "path": field(c, "path"),
                })).collect::<Vec<_>>(),
                "coordinator": row.coordinator_detail,
                "coordinator_state": row.coordinator_state,
                "created": row.created.len(),
                "dirty_checkouts": row.dirty,
                "journal": row.journal.to_string_lossy(),
                "journal_identity": {
                    "device": row.identity.0,
                    "inode": row.identity.1,
                    "sha256": row.identity.3,
                    "size": row.identity.2,
                },
                "machine": row.machine,
                "owner": row.owner_detail,
                "owner_state": row.owner_state,
                "planned": row.planned.len(),
                "registry_state": if row.state == "completed-create" { "exact-active" } else { "absent" },
                "slot": row.slot,
                "slot_path": row.slot_path.to_string_lossy(),
                "slot_type": row.slot_type,
                "state": row.state,
            })
        })
        .collect();
    if format_json {
        let payload = json!({
            "blocking": false,
            "journals": journals,
            "requested": {
                "agent": agent,
                "slot": slot,
                "slot_path": target.to_string_lossy(),
                "slot_type": slot_type,
            },
            "schema": 2,
        });
        return Ok(format!(
            "{}\n",
            crate::batch_text::py_compact_pub(&sort_keys(&payload))
        ));
    }
    let live = rows
        .iter()
        .filter(|r| r.state == "live-incomplete-create")
        .count();
    let completed = rows
        .iter()
        .filter(|r| r.state == "completed-create")
        .count();
    let dead = rows
        .iter()
        .filter(|r| r.state == "dead-incomplete-create")
        .count();
    let mut out = format!(
        "NONBLOCKING: journals={} live-incomplete-create={live} completed-create={completed} dead-incomplete-create={dead}; all are unrelated to requested {slot_type} slot {slot}\n",
        rows.len()
    );
    for row in &rows {
        let dirty = if row.dirty.is_empty() {
            "none".to_string()
        } else {
            row.dirty.join(",")
        };
        out.push_str(&format!(
            "JOURNAL: state={} machine={} slot={} agent={} dirty_checkouts={} journal={}\n",
            row.state,
            row.machine,
            row.slot,
            row.agent,
            dirty,
            row.journal.display()
        ));
    }
    Ok(out)
}
