//! Writer: record coordinator recovery evidence for an unbound slot.
//!
//! Reproduces the existing `recover-unbound-owner` mutation: a
//! coordinator records recovery evidence (note, validation,
//! limitations, continuation) and freshly verified checkouts for a
//! historical agent slot with no recorded owner, after the
//! registered liveness authority reports the agent dead. It does
//! not invent or replace an owner identity. Fixture registries
//! only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::finish_write::{
    assert_checkout_safe, assert_record_paths, assert_slot_contents, stored_repository_path,
};
use crate::heartbeat_write::assert_caller;
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::remove_write::liveness_answer;
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

/// Record coordinator recovery evidence for the named slot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_unbound_owner(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    coordinator_pid: i64,
    expected_generation: i64,
    recovery_note: &str,
    validation: &[String],
    limitations: &[String],
) -> Result<Value> {
    require_fixture()?;
    if recovery_note.trim().is_empty() {
        return Err("coordinator recovery requires a non-empty --recovery-note".to_string());
    }
    if validation.is_empty() || validation.iter().any(|item| item.trim().is_empty()) {
        return Err("coordinator recovery requires non-empty --validation evidence".to_string());
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
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
    let coordinator_now = read_process_identity(coordinator_pid).map_err(|_| {
        format!("coordinator PID {coordinator_pid} exited before the locked recovery boundary")
    })?;
    if coordinator_now != coordinator {
        return Err(format!(
            "coordinator PID {coordinator_pid} generation changed before the locked recovery boundary"
        ));
    }
    assert_caller(&coordinator, "coordinator")?;
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    if slot_type != "agent" {
        return Err(format!(
            "recover-unbound-owner for slot type '{slot_type}' stays with the Python edition for now"
        ));
    }
    if !record
        .get("owner")
        .cloned()
        .unwrap_or(Value::Null)
        .is_null()
    {
        return Err(format!(
            "slot {slot} has a historical owner; coordinator recovery cannot replace it"
        ));
    }
    let answer = liveness_answer(&cfg, &config, &record)?;
    if answer.state == "alive" {
        return Err(format!(
            "registered liveness authority reports owner alive: {}",
            answer.detail
        ));
    }
    if answer.state != "dead" {
        return Err(format!(
            "registered liveness authority is {}: {}; unknown ownership is not a free slot",
            answer.state, answer.detail
        ));
    }
    assert_record_paths(&cfg, &record)?;
    assert_slot_contents(&cfg, &record)?;
    let mut final_checkouts: Vec<Value> = Vec::new();
    for checkout in record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_stored, repository) = stored_repository_path(&cfg, repository_raw)?;
        final_checkouts.push(assert_checkout_safe(&cfg, &config, &checkout, &repository)?);
    }
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let mut updated = record.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("checkouts".to_string(), json!(final_checkouts));
        object.insert(
            "coordinator_recovery_note".to_string(),
            json!(recovery_note),
        );
        object.insert(
            "handoff".to_string(),
            json!({
                "recorded_at": utc_now(),
                "validation": validation,
                "limitations": limitations,
                "continuation": format!(
                    "wrkslots remove {slot} --coordinator-pid <current-coordinator-pid> --expected-generation {generation}"
                ),
            }),
        );
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
            "action": "owner-recovery-recorded",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": updated,
            "evidence": {"note": recovery_note},
        }),
    )?;
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    atomic_write_json(
        &cfg.control.join(format!("ACTIVE.{selected}.json")),
        &document,
    )?;
    validate_global(root, &cfg)?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "outcome": "recorded coordinator recovery evidence",
    }))
}
