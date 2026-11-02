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

/// The Python `doctor` command's output, envelope and human form.
///
/// Unlike the scoped [`parse_doctor`], this composes every finding the
/// Python doctor reports — registry storage inconsistencies first,
/// then per-record operational notes including event-derived owner
/// releases, slot-image attachment findings, journals, interrupted
/// atomic writes, and the cap breach — and the full envelope
/// (`schema`, roots, `event_counts`).
pub(crate) fn doctor_cli(
    root: Option<&Path>,
    machine_override: Option<&str>,
    all_machines: bool,
    human: bool,
) -> Result<String> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    let cap = config
        .get("max_active_slots")
        .cloned()
        .unwrap_or(Value::Null);
    let (all_machine_names, records_all) = status_records(root, true)?;
    // The Python doctor validates the global state first (active and
    // archived overlap, duplicate slots or agents refuse), as
    // `status-text` does.
    crate::hold_write::validate_global(root, &cfg)?;
    let machines: Vec<String> = if all_machines {
        all_machine_names.clone()
    } else {
        all_machine_names
            .iter()
            .filter(|machine| *machine == &selected)
            .cloned()
            .collect()
    };
    // Event chains per machine: release lookups and the envelope's
    // event counts both read them, like the Python doctor.
    let mut events_by_machine: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for machine in &all_machine_names {
        let dir = cfg.control.join(format!("EVENTS.{machine}"));
        let events = if dir.is_dir() {
            crate::global_state::load_event_chain(&dir, machine)?
        } else {
            Vec::new()
        };
        events_by_machine.insert(machine.clone(), events);
    }
    // Release lookups are computed once per record; a malformed
    // release event refuses the doctor, as the Python edition's
    // release-state loading raises instead of skipping it.
    let mut releases: std::collections::HashMap<(String, String), Value> =
        std::collections::HashMap::new();
    for record in &records_all {
        let (Some(machine), Some(slot)) = (
            record.get("machine").and_then(Value::as_str),
            record.get("slot").and_then(Value::as_str),
        ) else {
            continue;
        };
        let events = events_by_machine
            .get(machine)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let (release, void) = crate::audit_state::release_state(&cfg, record, events)?;
        if let Some(release) = release {
            releases.insert(
                (machine.to_string(), slot.to_string()),
                crate::audit_state::release_obj(&release, void.as_deref()),
            );
        }
    }
    let release_for = |record: &Value| -> Option<Value> {
        let machine = record.get("machine").and_then(Value::as_str)?;
        let slot = record.get("slot").and_then(Value::as_str)?;
        releases
            .get(&(machine.to_string(), slot.to_string()))
            .cloned()
    };
    // Findings, in the Python doctor's composition order.
    let mut findings: Vec<Value> =
        crate::status_state::storage_inconsistencies(root, &records_all)?;
    for record in &records_all {
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
        if let Some(release) = release_for(record) {
            let state = release
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let summary = release
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let sequence = release
                .get("event_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if state == "released" {
                let generation = record
                    .get("generation")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                findings.push(note(
                    "owner-released",
                    slot,
                    machine,
                    format!(
                        "{summary} (event {sequence}, generation {generation}); remove no longer waits for the owner's exit or the heartbeat time-to-live, and still requires the handoff read and an unused slot"
                    ),
                ));
            } else {
                let recorded_at = release
                    .get("recorded_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let void_reason = release
                    .get("void_reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                findings.push(note(
                    "owner-release-void",
                    slot,
                    machine,
                    format!(
                        "release at {recorded_at} (event {sequence}) no longer applies: {void_reason}"
                    ),
                ));
            }
        }
    }
    findings.extend(crate::storage_state::image_findings(&cfg, &selected));
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
        let mine = records_all
            .iter()
            .filter(|record| {
                record.get("machine").and_then(Value::as_str) == Some(selected.as_str())
            })
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
    if !all_machines {
        findings.retain(|item| {
            item.get("machine").is_none_or(|machine| {
                machine.is_null() || machine.as_str() == Some(selected.as_str())
            })
        });
    }
    let mut records: Vec<Value> = Vec::new();
    for mut record in records_all {
        if !machines
            .iter()
            .any(|machine| Some(machine.as_str()) == record.get("machine").and_then(Value::as_str))
        {
            continue;
        }
        if let Some(release) = release_for(&record) {
            if let Some(object) = record.as_object_mut() {
                object.insert("owner_release".to_string(), release);
            }
        }
        records.push(record);
    }
    if !human {
        let event_counts: serde_json::Map<String, Value> = machines
            .iter()
            .map(|machine| {
                (
                    machine.clone(),
                    json!(events_by_machine.get(machine).map_or(0, Vec::len)),
                )
            })
            .collect();
        let envelope = json!({
            "schema": 2,
            "project_root": cfg.root.to_string_lossy(),
            "worktrees_dir": cfg.worktrees.to_string_lossy(),
            "control_dir": cfg.control.to_string_lossy(),
            "layout": cfg.layout,
            "machines": machines,
            "event_counts": event_counts,
            "active": records,
            "max_active_slots": cap,
            "findings": findings,
        });
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope)
                .map_err(|error| format!("cannot encode doctor: {error}"))?
        ));
    }
    let cap_text = if cap.is_null() {
        "none".to_string()
    } else {
        cap.to_string()
    };
    let mut out = format!(
        "project={} worktrees={} control={} layout={} active={} cap={} findings={}\n",
        cfg.root.display(),
        cfg.worktrees.display(),
        cfg.control.display(),
        cfg.layout,
        records.len(),
        cap_text,
        findings.len(),
    );
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for item in &findings {
        if let Some(kind) = item.get("kind").and_then(Value::as_str) {
            *counts.entry(kind.to_string()).or_default() += 1;
        }
    }
    for (kind, count) in &counts {
        out.push_str(&format!("  {count:4}  {kind}\n"));
    }
    for item in &findings {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default();
        let slot = text("slot");
        out.push_str(&format!(
            "{}: {}: {}\n",
            text("kind"),
            if slot.is_empty() { "-" } else { slot },
            text("detail"),
        ));
    }
    Ok(out)
}
