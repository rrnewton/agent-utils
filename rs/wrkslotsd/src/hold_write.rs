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
    if let EventLogStatus::Derived = event_log_status(&directory) {
        let events = load_event_chain(&directory, selected)?;
        let mut held: Option<Value> = None;
        for event in &events {
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            if payload.get("slot").and_then(Value::as_str) != Some(slot) {
                continue;
            }
            match event.get("kind").and_then(Value::as_str) {
                Some("slot-held") => {
                    held = Some(json!({
                        "schema": 1,
                        "machine": selected,
                        "slot": slot,
                        "held_at": event.get("recorded_at").cloned().unwrap_or(Value::Null),
                        "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
                    }));
                }
                Some("slot-hold-released") => held = None,
                _ => {}
            }
        }
        return Ok(held.unwrap_or(Value::Null));
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
    let _config = acquire_config_lock(&cfg)?;
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
    let _global = acquire_lock(
        &cfg.control.join("ACTIVE.global.lock"),
        &cfg.control.join("ACTIVE"),
    )?;
    let _machine = acquire_lock(
        &cfg.control.join(format!("ACTIVE.{selected}.json.lock")),
        &cfg.control.join(format!("ACTIVE.{selected}.json")),
    )?;
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
    let mut active_slots: Vec<(String, String)> = Vec::new();
    for machine in discover_machines(&cfg.control)? {
        let (active, _order) = parse_state_ordered(root, Some(&machine))?;
        if let Some(slots) = active.get("slots").and_then(Value::as_array) {
            for record in slots {
                if let Some(name) = record.get("slot").and_then(Value::as_str) {
                    active_slots.push((name.to_string(), machine.clone()));
                }
            }
        }
        let archive = parse_archive(root, Some(&machine))?;
        if let Some(records) = archive.get("records").and_then(Value::as_array) {
            for record in records {
                if let Some(name) = record.get("slot").and_then(Value::as_str) {
                    if let Some((_, active_machine)) =
                        active_slots.iter().find(|(slot, _)| slot == name)
                    {
                        return Err(format!(
                            "slot '{name}' is active on {active_machine} and archived on {machine}"
                        ));
                    }
                }
            }
        }
    }
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
