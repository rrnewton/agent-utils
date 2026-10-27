//! All-machine ACTIVE/ARCHIVED validation parity.
//!
//! Composes the per-machine shard readers the way the existing global
//! state validation does in reference mode: discover the machine shards
//! and event-log directories in the control directory, refuse a shard
//! set that differs without append-only history, validate every shard,
//! then apply the cross-machine uniqueness and active-versus-archived
//! overlap rules. Snapshot mode only: a machine with a non-empty event
//! log derives its state, and derivation stays with `rebuild`.
//! Read-only; the lock-based overlap re-read the Python reader performs
//! is not reproduced, so an overlap is reported from the first read.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::archive_state::parse_archive;

type Result<T> = std::result::Result<T, String>;

fn machine_name(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return None;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return None;
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(text.to_owned())
}

fn strip_affixes<'a>(name: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    name.strip_prefix(prefix)?.strip_suffix(suffix)
}

fn python_list(values: &BTreeSet<String>) -> String {
    let items: Vec<String> = values.iter().map(|item| format!("'{item}'")).collect();
    format!("[{}]", items.join(", "))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn is_event_stem(stem: &str) -> bool {
    stem.len() == 20 && stem.bytes().all(|b| b.is_ascii_digit())
}

fn is_event_name(name: &str, partial: bool) -> bool {
    if partial {
        match name.split_once(".json.tmp.") {
            Some((stem, rest)) => is_event_stem(stem) && !rest.is_empty(),
            None => false,
        }
    } else {
        match name.strip_suffix(".json") {
            Some(stem) => is_event_stem(stem),
            None => false,
        }
    }
}

/// Whether the machine's state is derived from its event log, applying
/// the log directory's entry classification: complete event files derive,
/// in-progress `.tmp.` files do not, and any other entry refuses.
fn event_log_derived(directory: &Path) -> Result<bool> {
    if !directory.exists() {
        return Ok(false);
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot list event log {}: {error}", directory.display()))?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("cannot list event log {}: {error}", directory.display()))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    let unexpected: Vec<&String> = names
        .iter()
        .filter(|name| !is_event_name(name, false) && !is_event_name(name, true))
        .collect();
    if let Some(first) = unexpected.iter().min() {
        return Err(format!(
            "append-only event log contains an unexpected entry: {}; preserve it and run 'wrkslots doctor' before changing slot state",
            directory.join(first).display()
        ));
    }
    Ok(names.iter().any(|name| is_event_name(name, false)))
}

/// Validate every machine's ACTIVE and ARCHIVED shards as one registry,
/// printing each machine's normalized pair.
pub(crate) fn parse_global(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let mut active_machines: BTreeSet<String> = BTreeSet::new();
    let mut archive_machines: BTreeSet<String> = BTreeSet::new();
    let mut event_machines: BTreeSet<String> = BTreeSet::new();
    let entries = std::fs::read_dir(&cfg.control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            cfg.control.display()
        )
    })?;
    let mut names: Vec<(String, std::path::PathBuf, bool)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                cfg.control.display()
            )
        })?;
        let path = entry.path();
        let is_real_dir = path
            .symlink_metadata()
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        names.push((
            entry.file_name().to_string_lossy().into_owned(),
            path,
            is_real_dir,
        ));
    }
    names.sort();
    for (name, path, is_real_dir) in &names {
        let shown = path.display();
        if let Some(rest) = name.strip_prefix("ACTIVE.") {
            if rest.ends_with(".json") {
                match machine_name(strip_affixes(name, "ACTIVE.", ".json").unwrap_or_default()) {
                    Some(machine) => {
                        active_machines.insert(machine);
                    }
                    None => {
                        return Err(format!("unexpected active-state filename: {shown}"));
                    }
                }
            }
        } else if let Some(rest) = name.strip_prefix("ARCHIVED.") {
            if rest.ends_with(".json") {
                match machine_name(strip_affixes(name, "ARCHIVED.", ".json").unwrap_or_default()) {
                    Some(machine) => {
                        archive_machines.insert(machine);
                    }
                    None => {
                        return Err(format!("unexpected archive-state filename: {shown}"));
                    }
                }
            }
        } else if name.starts_with("EVENTS.") {
            match machine_name(strip_affixes(name, "EVENTS.", "").unwrap_or_default()) {
                Some(machine) => {
                    if !is_real_dir {
                        return Err(format!(
                            "append-only event log is not a real directory: {shown}"
                        ));
                    }
                    event_machines.insert(machine);
                }
                None => {
                    return Err(format!(
                        "unexpected append-only event directory name: {shown}"
                    ));
                }
            }
        }
    }
    let differing: BTreeSet<String> = active_machines
        .symmetric_difference(&archive_machines)
        .filter(|machine| !event_machines.contains(*machine))
        .cloned()
        .collect();
    if !differing.is_empty() {
        return Err(format!(
            "ACTIVE and ARCHIVED machine shards differ without append-only history: \
             active={} archived={} events={}",
            python_list(&active_machines),
            python_list(&archive_machines),
            python_list(&event_machines)
        ));
    }
    let machines: BTreeSet<String> = active_machines
        .union(&archive_machines)
        .chain(event_machines.iter())
        .cloned()
        .collect();
    let mut actives: Vec<Value> = Vec::new();
    let mut orders: Vec<Vec<crate::active_state::RecordOrder>> = Vec::new();
    for machine in &machines {
        if event_log_derived(&cfg.control.join(format!("EVENTS.{machine}")))? {
            return Err(format!(
                "machine state for {machine} is derived from the append-only event log; use rebuild instead of snapshot validation"
            ));
        }
        let (active, order) = crate::active_state::parse_state_ordered(root, Some(machine))?;
        actives.push(active);
        orders.push(order);
    }
    let mut slot_owner: Vec<(&str, &str)> = Vec::new();
    let mut agent_owner: Vec<(&str, &str)> = Vec::new();
    for (active, order) in actives.iter().zip(orders.iter()) {
        let machine = str_field(active, "machine").unwrap_or_default();
        for record in order {
            let slot = record.slot.as_str();
            if let Some((_, previous)) = slot_owner.iter().find(|(name, _)| *name == slot) {
                if *previous != machine {
                    return Err(format!(
                        "slot '{slot}' is active in both {previous} and {machine}"
                    ));
                }
            } else {
                slot_owner.push((slot, machine));
            }
            if record.slot_type == "agent" && !record.imported {
                let agent = record.agent.as_str();
                if let Some((_, previous)) = agent_owner.iter().find(|(name, _)| *name == agent) {
                    if *previous != slot {
                        return Err(format!("agent '{agent}' owns both {previous} and {slot}"));
                    }
                } else {
                    agent_owner.push((agent, slot));
                }
            }
        }
    }
    let mut archives: Vec<Value> = Vec::new();
    for machine in &machines {
        archives.push(parse_archive(root, Some(machine))?);
    }
    let mut archived_owner: Vec<(&str, &str)> = Vec::new();
    for archive in &archives {
        let machine = str_field(archive, "machine").unwrap_or_default();
        if let Some(records) = archive.get("records").and_then(Value::as_array) {
            for record in records {
                let slot = str_field(record, "slot").unwrap_or_default();
                if let Some((_, previous)) = archived_owner.iter().find(|(name, _)| *name == slot) {
                    if *previous != machine {
                        return Err(format!(
                            "slot '{slot}' is archived in both {previous} and {machine}"
                        ));
                    }
                } else {
                    archived_owner.push((slot, machine));
                }
            }
        }
    }
    for archive in &archives {
        let archive_machine = str_field(archive, "machine").unwrap_or_default();
        if let Some(records) = archive.get("records").and_then(Value::as_array) {
            for record in records {
                let slot = str_field(record, "slot").unwrap_or_default();
                if let Some((_, active_machine)) = slot_owner.iter().find(|(name, _)| *name == slot)
                {
                    return Err(format!(
                        "slot '{slot}' is active on {active_machine} and archived on {archive_machine}"
                    ));
                }
            }
        }
    }
    let states: Vec<Value> = machines
        .iter()
        .enumerate()
        .map(|(index, machine)| {
            json!({
                "machine": machine,
                "active": actives[index],
                "archive": archives[index],
            })
        })
        .collect();
    Ok(json!({
        "machines": machines.iter().collect::<Vec<_>>(),
        "states": states,
    }))
}
