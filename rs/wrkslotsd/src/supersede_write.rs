//! Writer: supersede a handoff-read retirement entry and rebind the
//! slot to its live agent (`supersede-handoff`).
//!
//! Reproduces the Python mutation behind the registry locks: the
//! coordinator authorization, the handoff-read candidate check, the
//! live-owner process-tree slot-use proof (cwd/root/exe links, one
//! descriptor, one mapping, mounts), the generation advance, and the
//! `handoff-superseded` active-state event. Fixture registries only
//! until the edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, slot_directory};
use crate::adopt_write::validate_agent_name;
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::global_state::load_event_chain;
use crate::heartbeat_write::{assert_caller, identity_value};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::retirement_queue::latest_handoff_read;
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn descendant_pids(root_pid: i64) -> Result<Vec<i64>> {
    let mut children: Vec<(i64, i64)> = Vec::new();
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate host processes: {error}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if let (Ok(pid), Some(ppid)) = (name.parse::<i64>(), parent_pid(name.parse().unwrap_or(0)))
        {
            children.push((ppid, pid));
        }
    }
    let mut found: Vec<i64> = vec![root_pid];
    let mut queue: Vec<i64> = vec![root_pid];
    while let Some(current) = queue.pop() {
        if found.len() > 4096 {
            break;
        }
        for (ppid, pid) in &children {
            if *ppid == current {
                found.push(*pid);
                queue.push(*pid);
            }
        }
    }
    Ok(found)
}

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

fn link_target(path: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_link(path).ok()?;
    let text = raw.to_string_lossy().into_owned();
    let text = text.strip_suffix(" (deleted)").unwrap_or(&text).to_string();
    let target = PathBuf::from(&text);
    if target.is_absolute() {
        Some(target)
    } else {
        Some(path.parent().unwrap_or(Path::new("/")).join(target))
    }
}

fn process_uses_slot(pid: i64, slot_path: &Path) -> Result<Vec<String>> {
    let pid_dir = PathBuf::from(format!("/proc/{pid}"));
    let mut uses: Vec<String> = Vec::new();
    for name in ["cwd", "root", "exe"] {
        if let Some(target) = link_target(&pid_dir.join(name)) {
            if within(&target, slot_path) {
                uses.push(format!("{name}={}", target.display()));
            }
        }
    }
    let fd_dir = pid_dir.join("fd");
    match std::fs::read_dir(&fd_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Some(target) = link_target(&entry.path()) {
                    if within(&target, slot_path) {
                        uses.push(format!(
                            "fd/{}={}",
                            entry.file_name().to_string_lossy(),
                            target.display()
                        ));
                        break;
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(format!(
                "process use is indeterminate because {} is unreadable: {error}",
                fd_dir.display()
            ))
        }
        Err(error) => {
            return Err(format!(
                "process use is indeterminate because {} cannot be read: {error}",
                fd_dir.display()
            ))
        }
    }
    let maps_path = pid_dir.join("maps");
    match std::fs::read_to_string(&maps_path) {
        Ok(maps) => {
            let slot_text = slot_path.to_string_lossy();
            for line in maps.lines() {
                if !line.contains(slot_text.as_ref()) {
                    continue;
                }
                let whitespace_fields: Vec<&str> = {
                    let mut out: Vec<&str> = Vec::new();
                    let mut rest = line;
                    for _ in 0..5 {
                        let trimmed = rest.trim_start();
                        match trimmed.find(char::is_whitespace) {
                            Some(at) => {
                                out.push(&trimmed[..at]);
                                rest = &trimmed[at..];
                            }
                            None => {
                                out.push(trimmed);
                                rest = "";
                                break;
                            }
                        }
                    }
                    out.push(rest.trim());
                    out
                };
                if whitespace_fields.len() == 6 {
                    let mapped = whitespace_fields[5]
                        .strip_suffix(" (deleted)")
                        .unwrap_or(whitespace_fields[5]);
                    let mapped_path = Path::new(mapped);
                    if mapped_path.is_absolute() && within(mapped_path, slot_path) {
                        uses.push(format!("map={mapped}"));
                        break;
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(format!(
                "process use is indeterminate because {} is unreadable: {error}",
                maps_path.display()
            ))
        }
        Err(error) => {
            return Err(format!(
                "process use is indeterminate because {} cannot be read: {error}",
                maps_path.display()
            ))
        }
    }
    if let Ok(mountinfo) = std::fs::read_to_string(pid_dir.join("mountinfo")) {
        for line in mountinfo.lines() {
            let Some((left, _right)) = line.split_once(" - ") else {
                continue;
            };
            let fields: Vec<&str> = left.split_whitespace().collect();
            for candidate in [fields.get(3), fields.get(4)].into_iter().flatten() {
                let decoded = candidate
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\012", "\n")
                    .replace("\\134", "\\");
                let path = Path::new(&decoded);
                if within(path, slot_path) {
                    uses.push(format!("mount={decoded}"));
                    break;
                }
            }
            if uses.iter().any(|item| item.starts_with("mount=")) {
                break;
            }
        }
    }
    Ok(uses)
}

/// Supersede the named slot's handoff-read entry, printing the outcome.
#[allow(clippy::too_many_arguments)]
pub(crate) fn supersede_handoff(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
    coordinator_pid: i64,
    coordinator_authorized: bool,
    reason: &str,
) -> Result<String> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    validate_agent_name(agent)?;
    if !coordinator_authorized {
        return Err("handoff supersession is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized".to_string());
    }
    if reason.trim().is_empty() {
        return Err("handoff supersession requires a non-empty --reason".to_string());
    }
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
    let successor = read_process_identity(owner_pid)?;
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
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!(
            "stale generation for slot {slot}: expected {expected_generation}, current {generation}"
        ));
    }
    if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
        let slot_type = record
            .get("slot_type")
            .and_then(Value::as_str)
            .unwrap_or("agent");
        return Err(format!(
            "slot {slot} is a {slot_type} slot; supersede-handoff applies to agent slots"
        ));
    }
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let events = load_event_chain(&directory, &selected)?;
    let latest = latest_handoff_read(&events, slot, generation)?;
    let Some(latest) = latest.filter(|read| read.enqueued) else {
        return Err(format!(
            "slot {slot} is not a handoff-read retirement candidate; there is no handoff-read entry to supersede"
        ));
    };
    let slot_dir = slot_directory(&cfg, slot, "agent")?;
    let mut slot_use: Option<String> = None;
    for pid in descendant_pids(successor.pid)? {
        let found = process_uses_slot(pid, &slot_dir)?;
        if let Some(first) = found.into_iter().next() {
            slot_use = Some(format!("pid {pid}: {first}"));
            break;
        }
    }
    let Some(slot_use) = slot_use else {
        return Err(format!(
            "no process in new owner PID {}'s tree uses slot {slot} ({}); supersede-handoff requires a live owner that is using the slot. remedy: have the new owner's lane run with a working directory or open file inside the slot, then rerun",
            successor.pid,
            slot_dir.display()
        ));
    };
    if read_process_identity(successor.pid)? != successor {
        return Err("owner process generation changed during supersede-handoff".to_string());
    }
    let released = events.iter().any(|event| {
        if event.get("kind").and_then(Value::as_str) != Some("owner-released") {
            return false;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
    });
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let previous_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let previous_owner = record.get("owner").cloned().unwrap_or(Value::Null);
    let mut superseded = record.clone();
    if let Some(object) = superseded.as_object_mut() {
        object.insert("agent".to_string(), json!(agent));
        object.insert("owner".to_string(), identity_value(&successor));
        object.insert("generation".to_string(), json!(generation + 1));
        object.insert("heartbeat_at".to_string(), json!(utc_now()));
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "handoff-superseded",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": superseded,
            "evidence": {
                "agent": agent,
                "coordinator": identity_value(&coordinator),
                "owner": identity_value(&successor),
                "previous_agent": previous_agent,
                "previous_owner": previous_owner,
                "reason": reason,
                "slot_use": slot_use,
                "superseded_handoff_read_sequence": latest.sequence,
                "superseded_handoff_sha256": latest.sha256,
                "voided_owner_release": released,
            },
        }),
    )?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = superseded.clone();
        }
    }
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    validate_global(root, &cfg)?;
    Ok(format!(
        "superseded slot={slot} agent={agent} generation={} owner_pid={} previous_agent={previous_agent}\n",
        generation + 1,
        successor.pid
    ))
}
