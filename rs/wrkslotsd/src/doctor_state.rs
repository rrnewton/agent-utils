//! Doctor findings composition parity (snapshot scope).
//!
//! Composes the tolerant per-record status diagnosis with the
//! operational findings the existing doctor derives without authorizing
//! mutation: invalid holds, dead owners, unfinished journals,
//! interrupted atomic writes, and an active-slot cap breach. Registry
//! storage inconsistencies, slot-image attachment findings, and
//! event-derived owner releases stay with the existing `doctor`.
//! Read-only.

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::config_validate::load_config;
use crate::journals_state::parse_journals;
use crate::status_state::status_records;

type Result<T> = std::result::Result<T, String>;

const REMEDY: &str = "inspect the named condition; diagnosis alone authorizes no mutation";

fn note(kind: &str, slot: Option<&str>, machine: Option<&str>, detail: String) -> Value {
    json!({
        "kind": kind,
        "scope": "operational",
        "slot": slot,
        "slot_type": Value::Null,
        "machine": machine,
        "checkout": Value::Null,
        "detail": detail,
        "remedy": REMEDY,
    })
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            match rest.strip_prefix(part) {
                Some(stripped) => rest = stripped,
                None => return false,
            }
        } else if index == parts.len() - 1 {
            if !rest.ends_with(part) {
                return false;
            }
            rest = &rest[..rest.len() - part.len()];
        } else {
            match rest.find(part) {
                Some(position) => rest = &rest[position + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn partial_leftovers(control: &Path) -> Result<Vec<String>> {
    let patterns = [
        "ACTIVE.*.json.tmp.*",
        "ARCHIVED.*.json.tmp.*",
        "ACTIVE.*.journal.tmp.*",
        "CREATE.*.journal.tmp.*",
        "FINISH.*.journal.tmp.*",
    ];
    let entries = std::fs::read_dir(control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            control.display()
        )
    })?;
    let mut names: Vec<(String, bool, std::path::PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                control.display()
            )
        })?;
        let path = entry.path();
        let is_real_dir = path
            .symlink_metadata()
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        names.push((
            entry.file_name().to_string_lossy().into_owned(),
            is_real_dir,
            path,
        ));
    }
    let mut leftovers: Vec<String> = Vec::new();
    for pattern in patterns {
        let mut matched: Vec<String> = names
            .iter()
            .filter(|(name, _, _)| glob_match(pattern, name))
            .map(|(name, _, _)| name.clone())
            .collect();
        matched.sort();
        leftovers.extend(matched);
    }
    let mut event_dirs: Vec<&(String, bool, std::path::PathBuf)> = names
        .iter()
        .filter(|(name, is_dir, _)| name.starts_with("EVENTS.") && *is_dir)
        .collect();
    event_dirs.sort();
    for (_, _, directory) in event_dirs {
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("cannot list event log {}: {error}", directory.display()))?;
        let mut matched: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("cannot list event log {}: {error}", directory.display())
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if glob_match("*.json.tmp.*", &name) {
                matched.push(name);
            }
        }
        matched.sort();
        leftovers.extend(matched);
    }
    Ok(leftovers)
}

/// Diagnose the registry the way the existing doctor does: tolerant
/// status records plus operational findings. Scoped to the selected
/// machine unless `all_machines`, like the existing command.
pub(crate) fn parse_doctor(
    root: Option<&Path>,
    machine_override: Option<&str>,
    all_machines: bool,
) -> Result<Value> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    let cap = config
        .get("max_active_slots")
        .cloned()
        .unwrap_or(Value::Null);
    let (all_machines_list, all_records) = status_records(root, true)?;
    let (machines, records): (Vec<String>, Vec<Value>) = if all_machines {
        (all_machines_list, all_records)
    } else {
        (
            all_machines_list
                .into_iter()
                .filter(|machine| machine == &selected)
                .collect(),
            all_records
                .into_iter()
                .filter(|record| {
                    record.get("machine").and_then(Value::as_str) == Some(selected.as_str())
                })
                .collect(),
        )
    };
    let mut findings: Vec<Value> = Vec::new();
    for record in &records {
        let slot = record.get("slot").and_then(Value::as_str);
        let machine = record.get("machine").and_then(Value::as_str);
        if let Some(error) = record.get("hold_error").and_then(Value::as_str) {
            findings.push(note("invalid-hold", slot, machine, error.to_string()));
        }
        if record.get("owner_state").and_then(Value::as_str) == Some("dead") {
            let detail = record
                .get("owner_detail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            findings.push(note(
                "dead-owner",
                slot,
                machine,
                format!(
                    "recorded owner is not running ({detail}); the slot is held by a row whose owner has exited"
                ),
            ));
        }
    }
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        for name in names {
            if let Some(name) = name.as_str() {
                findings.push(note(
                    "unfinished-journal",
                    None,
                    None,
                    format!("recovery required: {name}"),
                ));
            }
        }
    }
    for name in partial_leftovers(&cfg.control)? {
        findings.push(note(
            "partial-atomic-update",
            None,
            None,
            format!("interrupted write left {name}; preserve it and run 'wrkslots recover'"),
        ));
    }
    if let Some(cap_value) = cap.as_i64() {
        let mine = records
            .iter()
            .filter(|record| record.get("machine").and_then(Value::as_str) == Some(&selected))
            .count();
        if mine as i64 > cap_value {
            findings.push(note(
                "cap-breach",
                None,
                Some(&selected),
                format!(
                    "{mine} active slot(s) against max_active_slots={cap_value} (excess {})",
                    mine as i64 - cap_value
                ),
            ));
        }
    }
    Ok(json!({
        "machines": machines,
        "active": records,
        "max_active_slots": cap,
        "findings": findings,
    }))
}
