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
        } else if kind == "operation-completed" || kind == "operation-quarantined" {
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

fn identity_from_value(value: Option<&Value>) -> Option<crate::status_state::Identity> {
    let map = value?.as_object()?;
    Some(crate::status_state::Identity {
        pid: map.get("pid")?.as_i64()?,
        start_ticks: map.get("start_ticks")?.as_i64()?,
        boot_id: map.get("boot_id")?.as_str()?.to_string(),
        host_id: map.get("host_id")?.as_str()?.to_string(),
        cgroup_path: map.get("cgroup_path")?.as_str()?.to_string(),
    })
}

fn journal_actor_identities(raw: &serde_json::Map<String, Value>) -> Vec<crate::status_state::Identity> {
    let mut identities: Vec<crate::status_state::Identity> = Vec::new();
    let mut push = |value: Option<&Value>| {
        if let Some(identity) = identity_from_value(value) {
            if !identities.contains(&identity) {
                identities.push(identity);
            }
        }
    };
    match raw.get("kind").and_then(Value::as_str) {
        Some("create") => {
            push(raw.get("owner"));
            push(raw.get("coordinator_lease"));
        }
        Some("import-existing") => {
            push(raw.get("record").and_then(|record| record.get("owner")));
        }
        _ => {}
    }
    push(raw.get("actor"));
    push(raw.get("owner"));
    push(
        raw.get("authorization")
            .and_then(|authorization| authorization.get("actor")),
    );
    identities
}

fn journal_since(raw: &serde_json::Map<String, Value>, path: &Path) -> Option<String> {
    for field in ["finished_at", "created_at", "recorded_at"] {
        if let Some(value) = raw.get(field).and_then(Value::as_str) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(format_unix_utc(seconds))
}

fn format_unix_utc(total_seconds: i64) -> String {
    let days = total_seconds.div_euclid(86_400);
    let seconds = total_seconds.rem_euclid(86_400);
    let (hour, minute, second) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 { month_prime + 3 } else { month_prime - 9 } as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

/// Refusal text for an outstanding journal, by actor liveness (#1367).
///
/// Mirrors the Python `_assert_no_journal` classification: a live actor is
/// busy, a journal with no recorded actor generation is legacy, and every
/// other journal is interrupted.
pub(crate) fn journal_blocker_message(control: &Path, name: &str) -> String {
    let path = control.join(name);
    let interrupted = format!(
        "interrupted mutation recorded in {}; run 'wrkslots recover' first",
        path.display()
    );
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return interrupted;
    };
    let Ok(raw) = serde_json::from_str::<Value>(&contents) else {
        return interrupted;
    };
    let Some(raw) = raw.as_object() else {
        return interrupted;
    };
    let identities = journal_actor_identities(raw);
    if identities.is_empty() {
        return format!(
            "interrupted mutation recorded in {}; unknown actor (legacy journal); \
             if no wrkslots process is running, run 'wrkslots recover' first",
            path.display()
        );
    }
    // A create or import journal records its slot owner and coordinator,
    // not the process writing it; never its busy actor (#1367).
    let actor_is_writer = !matches!(
        raw.get("kind").and_then(Value::as_str),
        Some("create") | Some("import-existing")
    );
    for identity in &identities {
        let (state, _detail) = crate::status_state::process_state(Some(identity));
        if actor_is_writer && state == "live" {
            let kind = raw
                .get("kind")
                .and_then(Value::as_str)
                .map(|value| format!("'{value}'"))
                .unwrap_or_else(|| "None".to_string());
            return match journal_since(raw, &path) {
                Some(since) => format!(
                    "busy: live {kind} mutation {name} by pid {} since {since}; retry",
                    identity.pid
                ),
                None => format!(
                    "busy: live {kind} mutation {name} by pid {}; retry",
                    identity.pid
                ),
            };
        }
    }
    interrupted
}
