//! Writer: protect one active slot from cleanup and removal.
//!
//! Reproduces the existing `hold` mutation behind the registry locks:
//! partial-state and journal refusals, global-state validation, the
//! target slot's storage findings, the `slot-held` event, and the
//! atomic HOLD record write. Fixture registries only until the
//! edition cutover (see `ensure_states::require_fixture`).

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, Cfg};
use crate::archive_state::parse_archive;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    acquire_lock, append_event, atomic_write_json, ensure_event_log, refuse_partial_state,
    require_fixture, utc_now,
};
use crate::git_storage_state::parse_storage_git;
use crate::global_state::discover_machines;
use crate::global_state::{event_log_status, load_event_chain, EventLogStatus};
use crate::hold_state::parse_hold;
use crate::journals_state::parse_journals;
use crate::storage_state::parse_storage;
type Result<T> = std::result::Result<T, String>;

/// Validate every machine's snapshot state the way the existing
/// global-state validation does: duplicate slots and agents across
/// machines, duplicate archived slots, and active/archived overlap.
fn validate_global(root: Option<&Path>, cfg: &Cfg) -> Result<()> {
    let mut slot_machine: Vec<(String, String)> = Vec::new();
    let mut agent_slot: Vec<(String, String)> = Vec::new();
    let mut archived: Vec<(String, String)> = Vec::new();
    for machine in discover_machines(&cfg.control)? {
        let (active, order) = parse_state_ordered(root, Some(&machine))?;
        if let Some(slots) = active.get("slots").and_then(Value::as_array) {
            for record in slots {
                let name = record
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some((_, previous)) = slot_machine.iter().find(|(slot, _)| slot == name) {
                    if *previous != machine {
                        return Err(format!(
                            "slot '{name}' is active in both {previous} and {machine}"
                        ));
                    }
                } else {
                    slot_machine.push((name.to_string(), machine.clone()));
                }
            }
        }
        for record in &order {
            if record.slot_type == "agent" && !record.imported {
                if let Some((_, previous)) =
                    agent_slot.iter().find(|(agent, _)| *agent == record.agent)
                {
                    if *previous != record.slot {
                        return Err(format!(
                            "agent '{}' owns both {previous} and {}",
                            record.agent, record.slot
                        ));
                    }
                } else {
                    agent_slot.push((record.agent.clone(), record.slot.clone()));
                }
            }
        }
        let archive = parse_archive(root, Some(&machine))?;
        if let Some(records) = archive.get("records").and_then(Value::as_array) {
            for record in records {
                let name = record
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some((_, previous)) = archived.iter().find(|(slot, _)| slot == name) {
                    if *previous != machine {
                        return Err(format!(
                            "slot '{name}' is archived in both {previous} and {machine}"
                        ));
                    }
                } else {
                    archived.push((name.to_string(), machine.clone()));
                }
            }
        }
    }
    for (name, archive_machine) in &archived {
        if let Some((_, active_machine)) = slot_machine.iter().find(|(slot, _)| slot == name) {
            return Err(format!(
                "slot '{name}' is active on {active_machine} and archived on {archive_machine}"
            ));
        }
    }
    Ok(())
}

/// Take the config-directory, global, and machine locks in the
/// existing order, refusing a configuration changed while starting.
struct MutationLocks {
    _config: ConfigLock,
    _global: crate::ensure_states::Flock,
    _machine: crate::ensure_states::Flock,
}

fn acquire_mutation_locks(
    root: Option<&Path>,
    machine_override: Option<&str>,
    cfg: &Cfg,
    selected: &str,
) -> Result<MutationLocks> {
    let config = acquire_config_lock(cfg)?;
    let (reloaded, reloaded_selected) = load_cfg(root, machine_override)?;
    if reloaded_selected != selected
        || reloaded.root != cfg.root
        || reloaded.control != cfg.control
        || reloaded.layout != cfg.layout
        || reloaded.default_remote != cfg.default_remote
        || reloaded.default_landed_ref != cfg.default_landed_ref
    {
        return Err(
            "configuration changed while this command was starting; rerun so it uses one coherent policy"
                .to_string(),
        );
    }
    let global = acquire_lock(
        &cfg.control.join("ACTIVE.global.lock"),
        &cfg.control.join("ACTIVE"),
    )?;
    let machine = acquire_lock(
        &cfg.control.join(format!("ACTIVE.{selected}.json.lock")),
        &cfg.control.join(format!("ACTIVE.{selected}.json")),
    )?;
    Ok(MutationLocks {
        _config: config,
        _global: global,
        _machine: machine,
    })
}

fn refuse_target_storage(root: Option<&Path>, slot: &str) -> Result<()> {
    for value in [parse_storage(root)?, parse_storage_git(root)?] {
        if let Some(findings) = value.get("findings").and_then(Value::as_array) {
            for finding in findings {
                if finding.get("slot").and_then(Value::as_str) == Some(slot) {
                    let detail = finding
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    return Err(detail.to_string());
                }
            }
        }
    }
    Ok(())
}

struct ConfigLock {
    _file: std::fs::File,
}

fn acquire_config_lock(cfg: &Cfg) -> Result<ConfigLock> {
    let file = std::fs::File::open(&cfg.root).map_err(|error| {
        format!(
            "cannot open configuration lock {}: {error}",
            cfg.root.display()
        )
    })?;
    match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(ConfigLock { _file: file }),
        Err(_) => Err(format!(
            "configuration lock is busy for {}; retry after the other wrkslots mutation exits",
            cfg.root.display()
        )),
    }
}

/// The current hold for one slot: replayed from the event log when the
/// log derives the machine, else the snapshot HOLD record.
fn current_hold(root: Option<&Path>, cfg: &Cfg, selected: &str, slot: &str) -> Result<Value> {
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    match event_log_status(&directory) {
        EventLogStatus::Invalid(message) => return Err(message),
        EventLogStatus::Derived => {
            let events = load_event_chain(&directory, selected)?;
            let mut held: Option<Value> = None;
            for event in &events {
                let payload = event.get("payload").cloned().unwrap_or(Value::Null);
                match event.get("kind").and_then(Value::as_str) {
                    Some("state-imported") => {
                        if let Some(holds) = payload.get("holds").and_then(Value::as_array) {
                            for hold in holds {
                                if hold.get("slot").and_then(Value::as_str) == Some(slot) {
                                    held = Some(hold.clone());
                                }
                            }
                        }
                    }
                    Some("slot-held")
                        if payload.get("slot").and_then(Value::as_str) == Some(slot) =>
                    {
                        held = Some(json!({
                            "schema": 1,
                            "machine": selected,
                            "slot": slot,
                            "held_at": event.get("recorded_at").cloned().unwrap_or(Value::Null),
                            "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
                        }));
                    }
                    Some("slot-hold-released")
                        if payload.get("slot").and_then(Value::as_str) == Some(slot) =>
                    {
                        held = None;
                    }
                    _ => {}
                }
            }
            return Ok(held.unwrap_or(Value::Null));
        }
        EventLogStatus::Snapshot => {}
    }
    parse_hold(root, Some(selected), slot)
}

/// The active document for the selected machine: the snapshot shard,
/// or the imported state when only the event log materializes it.
fn active_document(root: Option<&Path>, cfg: &Cfg, selected: &str) -> Result<Value> {
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    if active_path.symlink_metadata().is_ok() {
        return Ok(parse_state_ordered(root, Some(selected))?.0);
    }
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    if let EventLogStatus::Derived = event_log_status(&directory) {
        let events = load_event_chain(&directory, selected)?;
        let imports: Vec<&Value> = events
            .iter()
            .filter(|event| event.get("kind").and_then(Value::as_str) == Some("state-imported"))
            .collect();
        if imports.len() > 1 {
            return Err("append-only event log imports state more than once".to_string());
        }
        if let Some(event) = imports.first() {
            let raw = event
                .get("payload")
                .and_then(|payload| payload.get("active"))
                .cloned()
                .ok_or_else(|| "state-imported event has no active state".to_string())?;
            return crate::active_state::normalize_active_value(
                cfg,
                &raw,
                selected,
                "state-imported event active",
            );
        }
    }
    Ok(parse_state_ordered(root, Some(selected))?.0)
}

/// Hold the named slot for the given reason, printing the outcome.
pub(crate) fn hold_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    reason: &str,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected): (Cfg, String) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("hold requires a non-empty --reason".to_string());
    }
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
    refuse_target_storage(root, slot)?;
    let active = active_document(root, &cfg, &selected)?;
    let record = active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let generation = record.get("generation").cloned().unwrap_or(json!(0));
    let existing = current_hold(root, &cfg, &selected, slot)?;
    if !existing.is_null() {
        let existing_reason = existing
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if existing_reason != reason {
            return Err(format!(
                "slot {slot} is already held for a different reason: {existing_reason}"
            ));
        }
        return Ok(json!({
            "machine": selected,
            "slot": slot,
            "reason": reason,
            "already_held": true,
        }));
    }
    ensure_event_log(root, &cfg, &selected)?;
    append_event(
        &cfg,
        &selected,
        "slot-held",
        json!({"slot": slot, "generation": generation, "reason": reason}),
    )?;
    let path = cfg.control.join(format!(
        "HOLD.{}.{}.{}.json",
        selected.chars().count(),
        selected,
        slot
    ));
    atomic_write_json(
        &path,
        &json!({
            "schema": 1,
            "machine": selected,
            "slot": slot,
            "held_at": utc_now(),
            "reason": reason,
        }),
    )?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "reason": reason,
        "already_held": false,
    }))
}

fn remove_control_file(path: &Path) -> Result<()> {
    if path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "control file must not be a symlink: {}",
            path.display()
        ));
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("cannot remove control file {}: {error}", path.display()))?;
    let parent = path.parent().unwrap_or(Path::new("."));
    let dir = std::fs::File::open(parent)
        .map_err(|error| format!("cannot remove control file {}: {error}", path.display()))?;
    dir.sync_all()
        .map_err(|error| format!("cannot remove control file {}: {error}", path.display()))?;
    Ok(())
}

fn storage_absent(cfg: &Cfg, record: &Value) -> bool {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let Ok(slot_dir) = crate::active_state::slot_directory(cfg, slot, slot_type) else {
        return false;
    };
    let mut all = vec![slot_dir];
    if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
        for checkout in checkouts {
            if let Some(path) = checkout.get("path").and_then(Value::as_str) {
                if Path::new(path).is_absolute() {
                    return false;
                }
                all.push(cfg.root.join(path));
            }
        }
    }
    all.iter().all(|path| path.symlink_metadata().is_err())
}

/// Release the named slot's hold, printing the outcome.
pub(crate) fn unhold_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected): (Cfg, String) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
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
    let active = active_document(root, &cfg, &selected)?;
    let record = active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned();
    let mut storage_absent_note = false;
    if let Err(error) = refuse_target_storage(root, slot) {
        match &record {
            Some(record) if storage_absent(&cfg, record) => {
                eprintln!(
                    "NOTE: slot {slot} storage is absent ({error}); releasing its hold changes nothing on disk, and recover-absent-agent-rows still proves everything else before retiring the row"
                );
                storage_absent_note = true;
            }
            _ => return Err(error),
        }
    }
    let record =
        record.ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let generation = record.get("generation").cloned().unwrap_or(json!(0));
    let existing = current_hold(root, &cfg, &selected, slot)?;
    if existing.is_null() {
        return Err(format!("slot {slot} is not held"));
    }
    ensure_event_log(root, &cfg, &selected)?;
    append_event(
        &cfg,
        &selected,
        "slot-hold-released",
        json!({"slot": slot, "generation": generation}),
    )?;
    let path = cfg.control.join(format!(
        "HOLD.{}.{}.{}.json",
        selected.chars().count(),
        selected,
        slot
    ));
    remove_control_file(&path)?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "released": true,
        "storage_absent_note": storage_absent_note,
    }))
}
