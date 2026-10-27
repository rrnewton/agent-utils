//! Outstanding recovery-journal listing parity.
//!
//! Reproduces the existing outstanding-journal enumeration: the
//! ACTIVE/CREATE/FINISH journal files in the control directory, plus
//! the journal paths still pending in each machine's append-only
//! history (an operation-progress event without its matching
//! completion). Event chains are validated as they are folded, so a
//! corrupt log refuses with the event loader's message. Read-only;
//! journal contents are not validated here (see `parse-journal`).

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::{as_mapping, as_str, load_cfg};
use crate::config_validate::validate_name;
use crate::global_state::{event_log_status, load_event_chain, EventLogStatus};

type Result<T> = std::result::Result<T, String>;

fn journal_path_name(
    control: &Path,
    machine: &str,
    payload: &serde_json::Map<String, Value>,
    journal: Option<&serde_json::Map<String, Value>>,
) -> Result<String> {
    let default = format!("ACTIVE.{machine}.journal");
    let name = match payload.get("journal_path") {
        None | Some(Value::Null) => return Ok(default),
        Some(value) => as_str(value, "append-only operation journal_path")?,
    };
    if name.contains('/') || name == "." || name == ".." {
        return Err("append-only operation journal_path is not a filename".to_string());
    }
    let operation = as_str(
        payload.get("operation").unwrap_or(&Value::Null),
        "append-only operation kind",
    )?;
    let slot = validate_name(
        as_str(
            payload.get("slot").unwrap_or(&Value::Null),
            "append-only operation slot",
        )?,
        "slot",
    )?;
    let slot_part = format!("{}.{}", slot.chars().count(), slot);
    let expected = if operation == "create" && name.starts_with("CREATE.") {
        format!(
            "CREATE.{}.{}.{}.journal",
            machine.chars().count(),
            machine,
            slot_part
        )
    } else if operation == "finish" && name.starts_with("FINISH.") {
        format!(
            "FINISH.{}.{}.{}.journal",
            machine.chars().count(),
            machine,
            slot_part
        )
    } else {
        default.clone()
    };
    let _ = control;
    if name != expected {
        return Err(
            "append-only operation journal_path does not match its operation identity".to_string(),
        );
    }
    if let Some(journal) = journal {
        if journal.get("kind").and_then(Value::as_str) != Some(operation)
            || journal.get("slot").and_then(Value::as_str) != Some(slot.as_str())
            || journal.get("machine").and_then(Value::as_str) != Some(machine)
        {
            return Err(
                "append-only operation event does not match its embedded journal".to_string(),
            );
        }
    }
    Ok(name.to_string())
}

fn pending_journals(control: &Path, machine: &str, events: &[Value]) -> Result<Vec<String>> {
    let mut pending: Vec<(String, Value)> = Vec::new();
    for event in events {
        let payload_value = event.get("payload").cloned().unwrap_or(Value::Null);
        let payload = as_mapping(&payload_value, "append-only event.payload")?;
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        if kind == "operation-progress-recorded" {
            let journal_value = payload.get("journal").cloned().unwrap_or(Value::Null);
            let journal = as_mapping(&journal_value, "append-only operation journal")?;
            let name = journal_path_name(control, machine, payload, Some(journal))?;
            let identity = (
                journal.get("kind").cloned().unwrap_or(Value::Null),
                journal.get("slot").cloned().unwrap_or(Value::Null),
            );
            for (existing_name, existing) in &pending {
                let existing_identity = (
                    existing.get("kind").cloned().unwrap_or(Value::Null),
                    existing.get("slot").cloned().unwrap_or(Value::Null),
                );
                if *existing_name != name && existing_identity == identity {
                    return Err(
                        "append-only history has the same pending operation under multiple journal paths"
                            .to_string(),
                    );
                }
            }
            if let Some(slot) = pending.iter_mut().find(|(n, _)| *n == name) {
                slot.1 = journal_value;
            } else {
                pending.push((name, journal_value));
            }
        } else if kind == "operation-completed" {
            let name = journal_path_name(control, machine, payload, None)?;
            if let Some(index) = pending.iter().position(|(n, _)| *n == name) {
                let current = &pending[index].1;
                if payload.get("slot") == current.get("slot")
                    && payload.get("operation") == current.get("kind")
                {
                    pending.remove(index);
                } else {
                    return Err(
                        "append-only completion does not match its pending operation".to_string(),
                    );
                }
            }
        }
    }
    Ok(pending.into_iter().map(|(name, _)| name).collect())
}

/// List the outstanding recovery-journal file names for the registry.
pub(crate) fn parse_journals(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut machines: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&cfg.control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            cfg.control.display()
        )
    })?;
    let mut event_dirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                cfg.control.display()
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let journal_file = ["ACTIVE.", "CREATE.", "FINISH."].iter().any(|prefix| {
            name.starts_with(prefix)
                && name.ends_with(".journal")
                && name.len() >= prefix.len() + ".journal".len()
        });
        if journal_file {
            names.insert(name);
        } else if let Some(machine) = name.strip_prefix("EVENTS.") {
            if machine.is_empty() {
                continue;
            }
            let path = entry.path();
            if path
                .symlink_metadata()
                .map(|meta| meta.is_dir())
                .unwrap_or(false)
            {
                validate_name(machine, "machine")?;
                event_dirs.push((machine.to_string(), path));
            }
        }
    }
    event_dirs.sort();
    for (machine, directory) in &event_dirs {
        machines.push(machine.clone());
        match event_log_status(directory) {
            EventLogStatus::Invalid(message) => return Err(message),
            EventLogStatus::Snapshot => {}
            EventLogStatus::Derived => {
                let events = load_event_chain(directory, machine)?;
                for name in pending_journals(&cfg.control, machine, &events)? {
                    names.insert(name);
                }
            }
        }
    }
    Ok(json!({
        "machines": machines,
        "journals": names.iter().collect::<Vec<_>>(),
    }))
}
