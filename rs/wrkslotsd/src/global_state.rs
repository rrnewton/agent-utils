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
            Some((stem, rest)) => is_event_stem(stem) && !rest.is_empty() && !rest.contains('\n'),
            None => false,
        }
    } else {
        match name.strip_suffix(".json") {
            Some(stem) => is_event_stem(stem),
            None => false,
        }
    }
}

/// Validate one machine's complete event chain the way the existing
/// event loader does (filename sequence, exact keys, schema, machine,
/// sequence, hash chain, and content digest), returning the loader's
/// first refusal message.
pub(crate) fn validate_event_chain(directory: &Path, machine: &str) -> Result<()> {
    load_event_chain(directory, machine).map(|_| ())
}

/// Like `validate_event_chain`, but also return the event documents.
pub(crate) fn load_event_chain(directory: &Path, machine: &str) -> Result<Vec<Value>> {
    use crate::active_state::{as_int, as_mapping, as_str, parse_timestamp};

    let mut names: Vec<String> = Vec::new();
    if directory.exists() {
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("cannot list event log {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("cannot list event log {}: {error}", directory.display())
            })?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    let mut complete: Vec<String> = names
        .into_iter()
        .filter(|name| is_event_name(name, false))
        .collect();
    complete.sort();
    let mut events: Vec<Value> = Vec::new();
    let mut previous = "0".repeat(64);
    for (index, name) in complete.iter().enumerate() {
        let expected = index + 1;
        let path = directory.join(name);
        if *name != format!("{expected:020}.json") {
            return Err(format!(
                "append-only event log has a sequence gap at {}; preserve the log and run 'wrkslots doctor'",
                path.display()
            ));
        }
        if path
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(format!(
                "append-only event must not be a symlink: {}",
                path.display()
            ));
        }
        let text = std::fs::read_to_string(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!("missing append-only event: {}", path.display())
            } else {
                format!("cannot read append-only event {}: {error}", path.display())
            }
        })?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("append-only event is malformed JSON: {error}"))?;
        let raw = as_mapping(&value, "append-only event")?;
        let required = [
            "schema",
            "machine",
            "sequence",
            "previous_sha256",
            "recorded_at",
            "kind",
            "payload",
            "sha256",
        ];
        let mut missing: Vec<&str> = required
            .iter()
            .filter(|key| !raw.contains_key(**key))
            .copied()
            .collect();
        missing.sort_unstable();
        let mut unknown: Vec<&str> = raw
            .keys()
            .map(String::as_str)
            .filter(|key| !required.contains(key))
            .collect();
        unknown.sort_unstable();
        if !missing.is_empty() || !unknown.is_empty() {
            let mut details = Vec::new();
            if !missing.is_empty() {
                details.push(format!("missing {}", missing.join(", ")));
            }
            if !unknown.is_empty() {
                details.push(format!("unknown {}", unknown.join(", ")));
            }
            return Err(format!(
                "append-only event has invalid fields: {}",
                details.join("; ")
            ));
        }
        if as_int(&raw["schema"], "append-only event.schema", 0)? != 1 {
            return Err(format!(
                "unsupported append-only event schema in {}",
                path.display()
            ));
        }
        if as_str(&raw["machine"], "append-only event.machine")? != machine {
            return Err(format!(
                "append-only event belongs to another machine: {}",
                path.display()
            ));
        }
        if as_int(&raw["sequence"], "append-only event.sequence", 0)? != expected as i64 {
            return Err(format!(
                "append-only event sequence does not match its filename: {}",
                path.display()
            ));
        }
        if as_str(&raw["previous_sha256"], "append-only event.previous_sha256")? != previous {
            return Err(format!(
                "append-only event hash chain is broken at {}",
                path.display()
            ));
        }
        parse_timestamp(
            as_str(&raw["recorded_at"], "append-only event.recorded_at")?,
            "append-only event.recorded_at",
        )?;
        as_str(&raw["kind"], "append-only event.kind")?;
        as_mapping(&raw["payload"], "append-only event.payload")?;
        let recorded = as_str(&raw["sha256"], "append-only event.sha256")?;
        let mut core = raw.clone();
        core.remove("sha256");
        let digest = crate::canonical::canonical_sha256(&Value::Object(core))
            .map_err(|error| error.to_string())?;
        if recorded != digest {
            return Err(format!(
                "append-only event digest does not match its content: {}",
                path.display()
            ));
        }
        previous = recorded.to_owned();
        events.push(value);
    }
    Ok(events)
}

/// How a machine's event log classifies under the log directory's entry
/// rules: complete event files derive state, in-progress `.tmp.` files
/// do not, and any other entry is invalid with the loader's message.
pub(crate) enum EventLogStatus {
    Snapshot,
    Derived,
    Invalid(String),
}

pub(crate) fn event_log_status(directory: &Path) -> EventLogStatus {
    match directory.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return EventLogStatus::Invalid(format!(
                "append-only event log is not a real directory: {}",
                directory.display()
            ));
        }
        Ok(_) => {}
        Err(_) => return EventLogStatus::Snapshot,
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            return EventLogStatus::Invalid(format!(
                "cannot list event log {}: {error}",
                directory.display()
            ));
        }
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Err(error) => {
                return EventLogStatus::Invalid(format!(
                    "cannot list event log {}: {error}",
                    directory.display()
                ));
            }
        }
    }
    let unexpected: Vec<&String> = names
        .iter()
        .filter(|name| !is_event_name(name, false) && !is_event_name(name, true))
        .collect();
    if let Some(first) = unexpected.iter().min() {
        return EventLogStatus::Invalid(format!(
            "append-only event log contains an unexpected entry: {}; preserve it and run 'wrkslots doctor' before changing slot state",
            directory.join(first).display()
        ));
    }
    if names.iter().any(|name| is_event_name(name, false)) {
        EventLogStatus::Derived
    } else {
        EventLogStatus::Snapshot
    }
}

/// The state bootstrap's refusal when an in-progress write of the
/// first event file is present, if any: it would append that file, and
/// its atomic-write leftover check names the first such leftover.
pub(crate) fn partial_write_refusal(directory: &Path) -> Option<String> {
    let first = "00000000000000000001.json";
    let prefix = format!("{first}.tmp.");
    let entries = std::fs::read_dir(directory).ok()?;
    let mut leftovers: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix) && is_event_name(name, true))
        .collect();
    leftovers.sort();
    leftovers.first().map(|name| {
        format!(
            "partial state update exists for {first}: {}; inspect it and use 'wrkslots recover'",
            directory.join(name).display()
        )
    })
}

fn event_log_derived(directory: &Path) -> Result<bool> {
    match event_log_status(directory) {
        EventLogStatus::Snapshot => Ok(false),
        EventLogStatus::Derived => Ok(true),
        EventLogStatus::Invalid(message) => Err(message),
    }
}

/// Discover the registry's machines from its shard and event-log
/// names, refusing unexpected names and a shard set that differs
/// without append-only history. Sorted like the existing discovery.
pub(crate) fn discover_machines(control: &Path) -> Result<Vec<String>> {
    let mut active_machines: BTreeSet<String> = BTreeSet::new();
    let mut archive_machines: BTreeSet<String> = BTreeSet::new();
    let mut event_machines: BTreeSet<String> = BTreeSet::new();
    let entries = std::fs::read_dir(control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            control.display()
        )
    })?;
    let mut names: Vec<(String, std::path::PathBuf, bool)> = Vec::new();
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
    Ok(machines.into_iter().collect())
}

/// Validate every machine's ACTIVE and ARCHIVED shards as one registry,
/// printing each machine's normalized pair.
pub(crate) fn parse_global(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let machines = discover_machines(&cfg.control)?;
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
