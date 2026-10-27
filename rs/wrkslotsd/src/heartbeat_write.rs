//! Writer: refresh one slot's heartbeat for its exact owner.
//!
//! Reproduces the existing `heartbeat` mutation: caller-process
//! capture and ancestry proof, owner authorization, the seal and
//! cleanup-journal disjointness guards, the owner-release refusal,
//! and the `active-state-recorded` event plus ACTIVE shard rewrite
//! at the next revision. Fixture registries only until the edition
//! cutover (see `ensure_states::require_fixture`).

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, Cfg};
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state_allow_seals,
    require_fixture, utc_now,
};
use crate::global_state::{event_log_status, load_event_chain, EventLogStatus};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn assert_caller(identity: &Identity, label: &str) -> Result<()> {
    let mut current: Option<i64> = Some(std::process::id() as i64);
    let mut seen: Vec<i64> = Vec::new();
    for _ in 0..256 {
        let Some(pid) = current else { break };
        if seen.contains(&pid) {
            break;
        }
        seen.push(pid);
        if let Ok(candidate) = read_process_identity(pid) {
            if candidate == *identity {
                return Ok(());
            }
        }
        current = parent_pid(pid);
    }
    Err(format!(
        "{label} PID {} is not in the invoking process ancestry",
        identity.pid
    ))
}

fn identity_value(identity: &Identity) -> Value {
    json!({
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    })
}

fn record_identity(record: &Value, key: &str) -> Option<Identity> {
    let value = record.get(key)?;
    if value.is_null() {
        return None;
    }
    Some(Identity {
        pid: value.get("pid")?.as_i64()?,
        start_ticks: value.get("start_ticks")?.as_i64()?,
        boot_id: value.get("boot_id")?.as_str()?.to_string(),
        host_id: value.get("host_id")?.as_str()?.to_string(),
        cgroup_path: value.get("cgroup_path")?.as_str()?.to_string(),
    })
}

fn seal_paths(cfg: &Cfg) -> Vec<std::path::PathBuf> {
    let mut seals: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.control) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("VALIDATE-BATCH-SEAL.") && name.ends_with(".journal") {
                seals.push(entry.path());
            }
        }
    }
    seals.sort();
    seals
}

fn refuse_seal_overlap(cfg: &Cfg, record: &Value) -> Result<()> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let requested = crate::active_state::slot_directory(cfg, slot, slot_type)?;
    for path in seal_paths(cfg) {
        let text = std::fs::read_to_string(&path).map_err(|error| {
            format!(
                "cannot read validation-batch seal journal {}: {error}",
                path.display()
            )
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            format!(
                "validation-batch seal journal {} is invalid: {error}",
                path.display()
            )
        })?;
        let machine = value
            .get("machine")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected = cfg
            .control
            .join(format!("VALIDATE-BATCH-SEAL.{machine}.journal"));
        if path != expected {
            return Err(
                "validation-batch seal journal filename does not match its machine".to_string(),
            );
        }
        let targets = value
            .get("targets")
            .and_then(Value::as_array)
            .ok_or_else(|| "validation-batch seal journal.targets is not a list".to_string())?;
        for target in targets {
            let sealed = target
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let generation = target.get("generation").cloned().unwrap_or(json!(0));
            let target_path = target
                .get("path")
                .and_then(Value::as_str)
                .map(|raw| cfg.root.join(raw));
            let overlaps = sealed == slot
                || target_path
                    .as_ref()
                    .map(|target| requested.starts_with(target) || target.starts_with(&requested))
                    .unwrap_or(false);
            if overlaps {
                return Err(format!(
                    "heartbeat target '{slot}' overlaps validation cleanup seal {} target '{sealed}' generation {generation}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
            }
        }
    }
    Ok(())
}

fn release_in_effect(cfg: &Cfg, record: &Value, events: &[Value]) -> Result<Option<Value>> {
    if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
        return Ok(None);
    }
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut release: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("owner-released") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) != Some(slot)
            || payload.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            continue;
        }
        let mut keys: Vec<&str> = payload
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort();
        if keys
            != [
                "agent",
                "generation",
                "heartbeat_at",
                "handoff",
                "owner",
                "slot",
            ]
        {
            return Err("owner-released event payload has the wrong keys".to_string());
        }
        release = Some(event.clone());
    }
    let Some(event) = release else {
        return Ok(None);
    };
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
    // Void rules: agent, owner, or heartbeat changed; handoff
    // publication superseded or incomplete; sidecar missing/changed.
    if payload.get("agent").and_then(Value::as_str) != record.get("agent").and_then(Value::as_str) {
        return Ok(None);
    }
    let owner = record_identity(record, "owner");
    let released_owner = payload.get("owner").cloned().unwrap_or(Value::Null);
    if owner.map(|identity| identity_value(&identity)) != Some(released_owner) {
        return Ok(None);
    }
    if payload.get("heartbeat_at").and_then(Value::as_str)
        != record.get("heartbeat_at").and_then(Value::as_str)
    {
        return Ok(None);
    }
    let handoff = payload.get("handoff").cloned().unwrap_or(Value::Null);
    let intent_sequence = handoff
        .get("intent_sequence")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut intents: Vec<i64> = Vec::new();
    let mut completed: Vec<i64> = Vec::new();
    let mut latest_artifact = Value::Null;
    for event in events {
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) != Some(slot)
            || payload.get("generation").and_then(Value::as_i64) != Some(generation)
        {
            continue;
        }
        match event.get("kind").and_then(Value::as_str) {
            Some("handoff-write-intended") => {
                if let Some(sequence) = event.get("sequence").and_then(Value::as_i64) {
                    intents.push(sequence);
                    if sequence >= intents.iter().max().copied().unwrap_or(0) {
                        latest_artifact = payload.get("artifact").cloned().unwrap_or(Value::Null);
                    }
                }
            }
            Some("handoff-write-completed") => {
                if let Some(sequence) = payload.get("intent_sequence").and_then(Value::as_i64) {
                    completed.push(sequence);
                }
            }
            _ => {}
        }
    }
    let latest = intents.iter().max().copied();
    if latest != Some(intent_sequence) || !completed.contains(&intent_sequence) {
        return Ok(None);
    }
    let handoff_path = handoff
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sidecar_path = if Path::new(handoff_path).is_absolute() {
        std::path::PathBuf::from(handoff_path)
    } else {
        cfg.root.join(handoff_path)
    };
    let Ok(sidecar_text) = std::fs::read_to_string(&sidecar_path) else {
        return Ok(None);
    };
    let Ok(sidecar) = serde_json::from_str::<Value>(&sidecar_text) else {
        return Ok(None);
    };
    if sidecar.get("sha256") != handoff.get("sha256")
        || sidecar.get("source").and_then(Value::as_str) != Some("write-handoff")
        || sidecar != latest_artifact
    {
        return Ok(None);
    }
    Ok(Some(event))
}

/// Refresh the named slot's heartbeat, printing the outcome.
pub(crate) fn heartbeat(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    let supplied = read_process_identity(owner_pid)?;
    assert_caller(&supplied, "owner")?;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state_allow_seals(&cfg.control)?;
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
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if record_agent != agent {
        return Err(format!(
            "ownership mismatch for slot {slot}: expected {record_agent}, got {agent}"
        ));
    }
    let owner = record_identity(&record, "owner")
        .ok_or_else(|| format!("slot {slot} has no bound owner process; adopt it first"))?;
    if supplied != owner {
        let (state, detail) = process_state(Some(&owner));
        return Err(format!(
            "owner process generation mismatch for slot {slot}; recorded owner is {state}: {detail}"
        ));
    }
    assert_caller(&supplied, "owner")?;
    refuse_seal_overlap(&cfg, &record)?;
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let sealed = matches!(event_log_status(&directory), EventLogStatus::Derived)
        && std::fs::read_dir(&cfg.control)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("VALIDATE-BATCH-SEAL.")
                })
            })
            .unwrap_or(false);
    if !sealed {
        let journals = parse_journals(root)?;
        if let Some(names) = journals.get("journals").and_then(Value::as_array) {
            if let Some(first) = names.first().and_then(Value::as_str) {
                return Err(format!(
                    "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                    cfg.control.join(first).display()
                ));
            }
        }
    }
    refuse_target_storage(root, slot)?;
    if let EventLogStatus::Derived = event_log_status(&directory) {
        let events = load_event_chain(&directory, &selected)?;
        if let Some(event) = release_in_effect(&cfg, &record, &events)? {
            let recorded_at = event
                .get("recorded_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let sequence = event.get("sequence").cloned().unwrap_or(json!(0));
            return Err(format!(
                "slot {slot} generation {generation} was released by its owner at {recorded_at} (event {sequence}), which stopped this generation's heartbeat. state: REFUSED -- the heartbeat was not renewed and the slot stays released for removal. remedy: stop using this slot and leave it; if more work is needed, create a new slot with 'wrkslots create'"
            ));
        }
    }
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let mut updated = record.clone();
    let stamp = utc_now();
    if let Some(object) = updated.as_object_mut() {
        object.insert("heartbeat_at".to_string(), json!(stamp));
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = updated.clone();
        }
    }
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "ttl-renewed",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": updated,
            "evidence": {"heartbeat_at": stamp},
        }),
    )?;
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "heartbeat_at": stamp,
    }))
}
