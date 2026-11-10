//! All-machine ACTIVE/ARCHIVED validation parity.
//!
//! Composes the per-machine shard readers the way the existing global
//! state validation does in reference mode: discover the machine shards
//! and event-log directories in the control directory, refuse a shard
//! set that differs without append-only history, validate every shard,
//! then apply the cross-machine uniqueness and active-versus-archived
//! overlap rules. Snapshot mode only: a machine with a non-empty event
//! log derives its state, and derivation stays with `rebuild`.
//! Read-only. The status and doctor readers publish only a read whose
//! registry content identity did not change across it
//! (`consistent_read`, #1200 rust-status-consistency), as the Python
//! readers' phased read does (#1172).

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

/// Machine a journal filename names, or None when it names none
/// (#873; Python `_journal_machine`).  `ACTIVE.<machine>.journal`
/// carries the machine directly; a scoped `CREATE.`/`FINISH.` name
/// embeds it length-prefixed
/// (`CREATE.<len>.<machine>.<len>.<slot>.journal`), which parses
/// unambiguously even when the machine itself contains dots.  A legacy
/// unscoped CREATE./FINISH. journal names no machine.
pub(crate) fn journal_machine(name: &str) -> Option<String> {
    if let Some(machine) = strip_affixes(name, "ACTIVE.", ".journal") {
        return machine_name(machine);
    }
    for prefix in ["CREATE.", "FINISH."] {
        let Some(rest) = name.strip_prefix(prefix) else {
            continue;
        };
        let (digits, rest) = rest.split_once('.')?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let length: usize = digits.parse().ok()?;
        if rest.len() < length || !rest.is_char_boundary(length) {
            return None;
        }
        let (machine, remainder) = rest.split_at(length);
        let remainder = remainder.strip_prefix('.')?;
        let (slot_digits, slot) = remainder.split_once('.')?;
        if slot_digits.is_empty() || !slot_digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let slot_length: usize = slot_digits.parse().ok()?;
        let slot_name = slot.strip_suffix(".journal")?;
        if slot_name.len() != slot_length {
            return None;
        }
        return machine_name(machine);
    }
    None
}

/// Shards recorded by other machines that are present on disk (#873;
/// Python `_foreign_shards`), sorted by path.
pub(crate) fn foreign_shards(
    control: &Path,
    local: &str,
) -> Result<Vec<(std::path::PathBuf, String)>> {
    let mut result: std::collections::BTreeMap<std::path::PathBuf, String> =
        std::collections::BTreeMap::new();
    let entries = std::fs::read_dir(control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            control.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                control.display()
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let machine = if let Some(machine) = strip_affixes(&name, "ACTIVE.", ".json") {
            machine_name(machine)
        } else if let Some(machine) = strip_affixes(&name, "ARCHIVED.", ".json") {
            machine_name(machine)
        } else if name.ends_with(".journal")
            && (name.starts_with("ACTIVE.")
                || name.starts_with("CREATE.")
                || name.starts_with("FINISH."))
        {
            journal_machine(&name)
        } else if name.starts_with("EVENTS.") {
            machine_name(strip_affixes(&name, "EVENTS.", "").unwrap_or_default())
        } else {
            None
        };
        if let Some(machine) = machine {
            if machine != local {
                result.insert(path, machine);
            }
        }
    }
    Ok(result.into_iter().collect())
}

/// NOTE records naming each foreign shard a default read skipped
/// (#873; Python `_foreign_shard_notes`).
pub(crate) fn foreign_shard_notes(control: &Path, local: &str) -> Result<Vec<Value>> {
    Ok(foreign_shards(control, local)?
        .into_iter()
        .map(|(path, machine)| {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| {
                    chrono::DateTime::from_timestamp(
                        duration.as_secs() as i64,
                        duration.subsec_nanos(),
                    )
                })
                // Whole seconds, as in the Python reader: both editions
                // print the same instant for the same file.
                .map(|moment| moment.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            json!({
                "machine": machine,
                "modified": modified,
                "path": path.to_string_lossy(),
            })
        })
        .collect())
}

/// Human NOTE lines for foreign shards a default read skipped.
pub(crate) fn foreign_shard_note_lines(notes: &[Value]) -> String {
    let mut out = String::new();
    for note in notes {
        out.push_str(&format!(
            "NOTE: foreign shard {} (machine {}, modified {}) was not read; \
             pass --all-machines to include it\n",
            note.get("path").and_then(Value::as_str).unwrap_or_default(),
            note.get("machine")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            note.get("modified")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ));
    }
    out
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
            // Derived machines validate through the chain fold, like
            // the Python global reader.
            let active = crate::hold_write::active_document(root, &cfg, machine)?;
            let order = active
                .get("slots")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|slot| crate::active_state::RecordOrder {
                    slot: slot
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    agent: slot
                        .get("agent")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    slot_type: slot
                        .get("slot_type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    imported: true,
                })
                .collect();
            actives.push(active);
            orders.push(order);
            continue;
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
        if event_log_derived(&cfg.control.join(format!("EVENTS.{machine}")))? {
            archives.push(crate::recover_absent_write::archives_document(
                &cfg, machine,
            )?);
        } else {
            archives.push(parse_archive(root, Some(machine))?);
        }
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

/// How many times a reader repeats a measurement whose registry changed
/// under it before it measures under the shared registry lock, as the
/// Python readers' `_PHASED_READ_RETRIES` (#1172).
const PHASED_READ_RETRIES: usize = 2;

fn content_digest(path: &Path) -> String {
    use sha2::Digest as _;
    match std::fs::read(path) {
        Ok(bytes) => sha2::Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        Err(error) => format!("unreadable: {error}"),
    }
}

fn sorted_names(directory: &Path) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    Ok(names)
}

fn is_complete_event(name: &str) -> bool {
    name.len() == 25
        && name.ends_with(".json")
        && name.as_bytes()[..20].iter().all(u8::is_ascii_digit)
}

/// Content identity of everything a registry reader reports from, as
/// the Python `_registry_read_identity` (#1172): every regular control
/// file except lock files is hashed, and so are the quarantine
/// directory's journals and every slot image record; an append-only
/// event directory is identified by its complete-event count, the last
/// event's digest (which chains every earlier event) and any partial
/// event write. Contents, not file times or inode numbers.
pub(crate) fn registry_read_identity(control: &Path) -> Result<Vec<Value>> {
    let names = sorted_names(control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            control.display()
        )
    })?;
    let mut parts: Vec<Value> = Vec::new();
    for name in names {
        if name.ends_with(".lock") {
            continue;
        }
        let path = control.join(&name);
        let Ok(metadata) = path.symlink_metadata() else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map(|target| target.to_string_lossy().into_owned())
                .unwrap_or_default();
            parts.push(json!([name, "symlink", target]));
        } else if name.starts_with("EVENTS.") && metadata.is_dir() {
            match sorted_names(&path) {
                Ok(events) => {
                    let (complete, partial): (Vec<String>, Vec<String>) =
                        events.into_iter().partition(|item| is_complete_event(item));
                    let last = complete.last();
                    parts.push(json!([
                        name,
                        complete.len(),
                        last,
                        last.map(|last| content_digest(&path.join(last))),
                        partial,
                    ]));
                }
                Err(error) => parts.push(json!([name, format!("unreadable: {error}")])),
            }
        } else if name == "quarantine" && metadata.is_dir() {
            let journals: Vec<Value> = sorted_names(&path)
                .unwrap_or_default()
                .into_iter()
                .filter(|item| !item.starts_with('.') && item.ends_with(".journal"))
                .map(|item| json!([item, content_digest(&path.join(&item))]))
                .collect();
            parts.push(json!([name, journals]));
        } else if name == crate::image_record::IMAGES_DIRECTORY && metadata.is_dir() {
            let mut records: Vec<Value> = Vec::new();
            for outer in sorted_names(&path).unwrap_or_default() {
                if outer.starts_with('.') || !path.join(&outer).is_dir() {
                    continue;
                }
                for inner in sorted_names(&path.join(&outer)).unwrap_or_default() {
                    let record = path
                        .join(&outer)
                        .join(&inner)
                        .join(crate::image_record::RECORD_NAME);
                    if inner.starts_with('.') || !record.exists() {
                        continue;
                    }
                    records.push(json!([
                        format!("{outer}/{inner}/{}", crate::image_record::RECORD_NAME),
                        content_digest(&record),
                    ]));
                }
            }
            parts.push(json!([name, records]));
        } else if metadata.is_file() {
            parts.push(json!([name, content_digest(&path)]));
        }
    }
    Ok(parts)
}

/// Run a registry reader so that it reports the registry at one instant
/// (#1200 rust-status-consistency), as the Python readers' phased read
/// does (#1172). `measure` runs lock-free between two reads of the
/// registry content identity, and its result (a report or a refusal) is
/// published only when the identity did not change across it. Otherwise
/// it is measured again, and after PHASED_READ_RETRIES repeats the last
/// measurement runs under the shared registry lock, which no writer can
/// hold at the same time. The Python reader re-measures only the rows
/// that changed; this repeats the whole read, with the same report.
pub(crate) fn consistent_read<T>(
    control: &Path,
    mut measure: impl FnMut() -> Result<T>,
) -> Result<T> {
    for attempt in 0..=PHASED_READ_RETRIES {
        let before = registry_read_identity(control)?;
        let measured = measure();
        if attempt == 0 {
            change_registry_for_test();
        }
        if registry_read_identity(control)? == before {
            return measured;
        }
    }
    let _lock = crate::ensure_states::acquire_shared_lock(
        &control.join("ACTIVE.global.lock"),
        &control.join("ACTIVE"),
    )?;
    measure()
}

/// Test seam (#1200): run `WRKSLOTS_TEST_REGISTRY_CHANGE` once, after a
/// reader's first measurement and before it checks the registry
/// identity, so a differential can land a mutation mid-read.
fn change_registry_for_test() {
    if let Some(command) = std::env::var_os("WRKSLOTS_TEST_REGISTRY_CHANGE") {
        let _ = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod read_identity_tests {
    use super::*;

    fn control(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wrkslots-identity-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("EVENTS.host")).unwrap();
        std::fs::create_dir_all(dir.join("slot-images/agent/a")).unwrap();
        std::fs::create_dir_all(dir.join("quarantine")).unwrap();
        std::fs::write(dir.join("ACTIVE.host.json"), b"{}").unwrap();
        std::fs::write(dir.join("ACTIVE.global.lock"), b"").unwrap();
        std::fs::write(dir.join("EVENTS.host/00000000000000000001.json"), b"1").unwrap();
        std::fs::write(dir.join("slot-images/agent/a/IMAGE.json"), b"{}").unwrap();
        std::fs::write(dir.join("quarantine/x.journal"), b"j").unwrap();
        dir
    }

    #[test]
    fn the_identity_sees_every_reported_content_but_not_locks() {
        let dir = control("contents");
        let base = registry_read_identity(&dir).unwrap();
        assert_eq!(registry_read_identity(&dir).unwrap(), base);
        // A lock file's content is not reported from.
        std::fs::write(dir.join("ACTIVE.global.lock"), b"held").unwrap();
        assert_eq!(registry_read_identity(&dir).unwrap(), base);
        for (path, content) in [
            ("ACTIVE.host.json", &b"{\"slots\": []}"[..]),
            ("EVENTS.host/00000000000000000002.json", b"2"),
            ("EVENTS.host/00000000000000000003.json.tmp", b"partial"),
            ("slot-images/agent/a/IMAGE.json", b"{\"size\": 1}"),
            ("quarantine/x.journal", b"changed"),
        ] {
            let before = registry_read_identity(&dir).unwrap();
            std::fs::write(dir.join(path), content).unwrap();
            assert_ne!(registry_read_identity(&dir).unwrap(), before, "{path}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_that_saw_a_change_is_measured_again() {
        let dir = control("phased");
        let mut calls = 0;
        let report = consistent_read(&dir, || {
            calls += 1;
            if calls == 1 {
                // A writer lands while the first measurement runs.
                std::fs::write(dir.join("ACTIVE.host.json"), b"{\"slots\": [1]}").unwrap();
                return Ok("mixed");
            }
            Ok("current")
        })
        .unwrap();
        assert_eq!((report, calls), ("current", 2));
        // A registry that changes on every measurement is read once more
        // under the shared lock, after the bounded repeats.
        let mut calls = 0;
        let report = consistent_read(&dir, || {
            calls += 1;
            std::fs::write(dir.join("ACTIVE.host.json"), format!("{{\"n\": {calls}}}")).unwrap();
            Ok(calls)
        })
        .unwrap();
        assert_eq!(report, PHASED_READ_RETRIES + 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
