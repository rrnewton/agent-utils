//! Writer: hand a slot with a provably dead owner to its successor.
//!
//! Reproduces the existing `transfer-owner` mutation: generation
//! and agent checks, the already-owned outcome, the provably-dead
//! recorded-owner requirement, the released-generation refusal
//! (with a coordinator-authorized `--reopen` rebind starting a new
//! generation), unregistered nested worktrees warned not refused,
//! successor authorization by caller ancestry or by residence in
//! the slot's own cgroup box, and the `owner-transferred` (or
//! `owner-reopened`) active-state event plus ACTIVE shard rewrite.
//! Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::adopt_write::validate_agent_name;
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::global_state::load_event_chain;
use crate::heartbeat_write::{assert_caller, release_in_effect};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage_except_nested, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

fn identity_value(identity: &Identity) -> Value {
    json!({
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    })
}

fn record_identity(record: &Value) -> Option<Identity> {
    let value = record.get("owner")?;
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

fn systemd_escape(value: &str) -> Option<String> {
    let output = Command::new("systemd-escape")
        .arg("--")
        .arg(value)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn cgroup_in_slot_box(cgroup_path: &str, slot_type: &str, slot: &str) -> bool {
    let label = if slot_type == "agent" {
        slot.to_string()
    } else {
        format!("{slot_type}:{slot}")
    };
    let Some(escaped) = systemd_escape(&label) else {
        return false;
    };
    let own = format!("wrkslots-{escaped}.slice");
    cgroup_path
        .split('/')
        .any(|component| component == own || component.ends_with(&format!("-{own}")))
}

/// Transfer the named slot to a live successor owner.
#[allow(clippy::too_many_arguments)]
pub(crate) fn transfer_owner(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
    reopen: bool,
    coordinator_pid: Option<i64>,
    coordinator_authorized: bool,
    reason: Option<&str>,
) -> Result<Value> {
    let reason_text = reason.unwrap_or("").trim().to_string();
    let coordinator = if reopen {
        if !coordinator_authorized {
            return Err(
                "released generation reopen is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                    .to_string(),
            );
        }
        let Some(pid) = coordinator_pid else {
            return Err("transfer-owner --reopen requires --coordinator-pid".to_string());
        };
        if reason_text.is_empty() {
            return Err("transfer-owner --reopen requires a non-empty --reason".to_string());
        }
        let identity = read_process_identity(pid)?;
        assert_caller(&identity, "coordinator")?;
        Some(identity)
    } else {
        None
    };
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    validate_agent_name(agent)?;
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
    let nested = refuse_target_storage_except_nested(root, slot)?;
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
        .unwrap_or_default()
        .to_string();
    if !reopen && record_agent != agent {
        return Err(format!(
            "ownership mismatch for slot {slot}: expected {record_agent}, got {agent}"
        ));
    }
    let existing = record_identity(&record);
    if !reopen {
        if let Some(owner) = &existing {
            if *owner == successor {
                return Ok(json!({
                    "machine": selected,
                    "slot": slot,
                    "agent": agent,
                    "generation": generation,
                    "owner_pid": successor.pid,
                    "already_transferred": true,
                }));
            }
        }
    }
    let (previous_state, detail) = process_state(existing.as_ref());
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), &selected)?;
    let release_event = release_in_effect(&cfg, &record, &events)?;
    if reopen {
        if release_event.is_none() {
            return Err(format!(
                "slot {slot} generation {generation} is not a released generation in effect; use plain 'wrkslots transfer-owner' without --reopen for a dead-owner transfer"
            ));
        }
    } else {
        if existing.is_some() && previous_state != "dead" {
            return Err(format!(
                "slot {slot} owner is {previous_state}: {detail}; transfer-owner only replaces an owner that is provably dead"
            ));
        }
        if release_event.is_some() {
            return Err(format!(
                "slot {slot} generation {generation} was released by its owner; a released generation is waiting for removal and is not transferred. remedy: a coordinator may rebind it with 'wrkslots transfer-owner {slot} --agent <agent> --owner-pid <pid> --expected-generation {generation} --reopen --coordinator-authorized --coordinator-pid <pid> --reason <why>'"
            ));
        }
    }
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let basis = match assert_caller(&successor, "owner") {
        Ok(()) => "caller-ancestry",
        Err(_) => {
            if cgroup_in_slot_box(&successor.cgroup_path, &slot_type, slot) {
                "slot-box"
            } else {
                return Err(format!(
                    "owner PID {} is neither in the invoking process ancestry nor inside slot {slot}'s own box (control group {})",
                    successor.pid, successor.cgroup_path
                ));
            }
        }
    };
    let confirmed = read_process_identity(successor.pid)?;
    if confirmed != successor {
        return Err("owner process generation changed during transfer-owner".to_string());
    }
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let new_generation = if reopen { generation + 1 } else { generation };
    let mut transferred = record.clone();
    if let Some(object) = transferred.as_object_mut() {
        object.insert("owner".to_string(), identity_value(&successor));
        object.insert("heartbeat_at".to_string(), json!(utc_now()));
        if reopen {
            object.insert("agent".to_string(), json!(agent));
            object.insert("generation".to_string(), json!(new_generation));
        }
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = transferred.clone();
        }
    }
    let mut evidence = json!({
        "owner": identity_value(&successor),
        "previous_owner": existing.as_ref().map(identity_value).unwrap_or(Value::Null),
        "previous_owner_state": previous_state,
        "basis": basis,
    });
    if reopen {
        let release_sequence = release_event
            .as_ref()
            .and_then(|event| event.get("sequence"))
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(object) = evidence.as_object_mut() {
            object.insert("previous_agent".to_string(), json!(record_agent));
            object.insert("previous_generation".to_string(), json!(generation));
            object.insert(
                "coordinator".to_string(),
                coordinator
                    .as_ref()
                    .map(identity_value)
                    .unwrap_or(Value::Null),
            );
            object.insert("coordinator_authorized".to_string(), json!(true));
            object.insert("reason".to_string(), json!(reason_text));
            object.insert("release_sequence".to_string(), release_sequence);
        }
    }
    if !nested.is_empty() {
        if let Some(object) = evidence.as_object_mut() {
            object.insert("unregistered_nested_worktrees".to_string(), json!(nested));
        }
    }
    for detail in &nested {
        eprintln!(
            "WARNING: unregistered nested worktree retained during transfer-owner: {detail}. state: RETAINED -- ownership did not touch it and removal still refuses it"
        );
    }
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": if reopen { "owner-reopened" } else { "owner-transferred" },
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": transferred,
            "evidence": evidence,
        }),
    )?;
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    validate_global(root, &cfg)?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "agent": agent,
        "generation": new_generation,
        "owner_pid": successor.pid,
        "basis": basis,
        "previous_owner_state": previous_state,
        "already_transferred": false,
        "reopened": reopen,
        "unregistered_nested_worktrees": nested,
    }))
}

#[cfg(test)]
mod tests {
    use super::cgroup_in_slot_box;

    // Mirrors the Python `_cgroup_in_slot_box` cases in
    // test_owner_transfer.py, including systemd-escaped hyphens.
    #[test]
    fn slot_box_matches_own_slice_only() {
        let boxed = "/user.slice/user-1.slice/user@1.service/app.slice/app-wrkslots.slice/app-wrkslots-kvm\\x2d20261012.slice/wrkslots-run-kvm\\x2d20261012-1073417.scope";
        assert!(cgroup_in_slot_box(boxed, "agent", "kvm-20261012"));
        assert!(cgroup_in_slot_box(
            "/wrkslots.slice/wrkslots-slot01.slice/x.scope",
            "agent",
            "slot01"
        ));
        for slot in ["kvm", "20261012", "kvm-2026101"] {
            assert!(!cgroup_in_slot_box(boxed, "agent", slot), "{slot}");
        }
        assert!(!cgroup_in_slot_box(
            "/app.slice/app-wrkslots-box-kvm\\x2d20261012.slice/a.scope",
            "agent",
            "kvm-20261012"
        ));
        assert!(!cgroup_in_slot_box(
            "/app.slice/herdr-server.service",
            "agent",
            "kvm-20261012"
        ));
        assert!(!cgroup_in_slot_box(
            "/user.slice/wrkslots-slot-b.slice/run.scope",
            "agent",
            "slot-a"
        ));
    }
}
