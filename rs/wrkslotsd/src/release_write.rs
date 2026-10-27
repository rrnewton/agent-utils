//! Writer: give one slot generation back while its owner runs.
//!
//! Reproduces the existing `release` mutation: owner authorization,
//! the agent-slot and hold guards, handoff publication verification,
//! and the single `owner-released` event (or the already-released
//! outcome). Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory};
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, ensure_event_log, refuse_partial_state_allow_seals, require_fixture,
};
use crate::global_state::{event_log_status, load_event_chain, EventLogStatus};
use crate::handoff_state::{parse_handoff, sidecar_path};
use crate::handoff_write::{committed_project_file, publication, validate_publication};
use crate::heartbeat_write::{
    assert_caller, identity_value, refuse_seal_overlap_action, release_in_effect, seal_paths,
};
use crate::hold_write::{
    acquire_mutation_locks, active_document, current_hold, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

const TAIL: &str = "state: REFUSED -- the slot was not released and its owner still holds it.";

/// Release the named slot generation, printing the outcome.
#[allow(clippy::too_many_arguments)]
pub(crate) fn release_slot(
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
    let owner_value = record.get("owner").cloned().unwrap_or(Value::Null);
    if owner_value.is_null() {
        return Err(format!(
            "slot {slot} has no bound owner process; adopt it first"
        ));
    }
    if identity_value(&supplied) != owner_value {
        let owner = Identity {
            pid: owner_value.get("pid").and_then(Value::as_i64).unwrap_or(0),
            start_ticks: owner_value
                .get("start_ticks")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            boot_id: owner_value
                .get("boot_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            host_id: owner_value
                .get("host_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            cgroup_path: owner_value
                .get("cgroup_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        let (state, detail) = process_state(Some(&owner));
        return Err(format!(
            "owner process generation mismatch for slot {slot}; recorded owner is {state}: {detail}"
        ));
    }
    assert_caller(&supplied, "owner")?;
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    if slot_type != "agent" {
        return Err(format!(
            "release applies only to agent slots; slot {slot} is a {slot_type} slot. {TAIL} remedy: remove a validation slot with 'wrkslots remove {slot} --validate-complete --coordinator-pid <pid> --expected-generation {generation}' once its run completed"
        ));
    }
    refuse_seal_overlap_action(&cfg, &record, "release")?;
    let journals = parse_journals(root)?;
    let journal_names: Vec<String> = journals
        .get("journals")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if seal_paths(&cfg).is_empty() {
        if let Some(first) = journal_names.first() {
            return Err(format!(
                "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                cfg.control.join(first).display()
            ));
        }
    } else if journal_names
        .iter()
        .any(|name| name.contains(&format!(".{}.", slot.chars().count())) && name.contains(slot))
    {
        return Err(format!(
            "release target '{slot}' overlaps private validation finish for '{slot}'"
        ));
    }
    refuse_target_storage(root, slot)?;
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let events = if let EventLogStatus::Derived = event_log_status(&directory) {
        load_event_chain(&directory, &selected)?
    } else {
        Vec::new()
    };
    let hold = current_hold(root, &cfg, &selected, slot)?;
    if !hold.is_null() {
        let reason = hold
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(format!(
            "slot {slot} is held: {reason}. A hold keeps a slot on purpose, so it cannot be released while held. {TAIL} remedy: ask whoever placed the hold to run 'wrkslots unhold {slot}' once it is no longer needed, then retry release"
        ));
    }
    // Resolve the published handoff artifact.
    let path = sidecar_path(&cfg.control, &selected, slot, generation);
    if (path.exists() || path.is_symlink())
        && std::fs::read_to_string(&path)
            .map(|text| {
                serde_json::from_str::<Value>(&text)
                    .map(|value| value.get("schema").and_then(Value::as_i64) == Some(1))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    {
        return Err(format!(
            "cannot release slot {slot}: slot {slot} handoff sidecar schema 1 lacks auditable source provenance and is permanently manual-only; preserve it for inspection rather than replacing or using it as removal authority. {TAIL}"
        ));
    }
    let sidecar: Option<Value> = match parse_handoff(root, machine_override, slot) {
        Ok(value) if value.is_null() => None,
        Ok(value) => Some(value),
        Err(error) => {
            return Err(format!("cannot release slot {slot}: {error}. {TAIL}"));
        }
    };
    validate_publication(&events, slot, generation, &supplied)?;
    let published = publication(&events, slot, generation);
    let slot_dir = slot_directory(&cfg, slot, slot_type)?;
    let legacy_path = slot_dir.join("HANDOFF.md");
    let has_legacy =
        (legacy_path.exists() || legacy_path.is_symlink()) && !committed_project_file(&legacy_path);
    if let Some(value) = &sidecar {
        if has_legacy {
            let (legacy_contents, _) = crate::handoff_state::read_regular_file_identity(
                &legacy_path,
                "slot HANDOFF.md",
                1024 * 1024,
            )?;
            let legacy_sha = format!("{:x}", Sha256::digest(&legacy_contents));
            if value
                .get("contents_utf8")
                .and_then(Value::as_str)
                .map(|text| text.as_bytes())
                != Some(legacy_contents.as_slice())
                || value.get("sha256").and_then(Value::as_str) != Some(legacy_sha.as_str())
            {
                return Err(format!(
                    "cannot release slot {slot}: slot {slot} contains an unread HANDOFF.md: its handoff sidecar disagrees with the legacy file; preserve both files and reconcile their exact contents. {TAIL}"
                ));
            }
        }
        if value.get("source").and_then(Value::as_str) == Some("legacy-slot-file")
            && published.is_some()
        {
            return Err(format!(
                "cannot release slot {slot}: slot {slot} has a owner-authenticated write intent; a legacy projection cannot supersede it. {TAIL}"
            ));
        }
    }
    let artifact = match (&sidecar, &published) {
        (None, Some((_, _, completed))) => {
            let state = if *completed {
                "completed"
            } else {
                "outstanding"
            };
            let remedy = if *completed {
                "restore the exact completed sidecar"
            } else {
                "have the exact owner retry write-handoff or recover its authenticated temp"
            };
            return Err(format!(
                "cannot release slot {slot}: slot {slot} has a {state} owner-authenticated handoff write intent but no canonical sidecar; preserve the slot and {remedy}. {TAIL}"
            ));
        }
        (None, None) => {
            let found = if has_legacy {
                "has only a legacy HANDOFF.md, which names no writer"
            } else {
                "has no handoff sidecar"
            };
            return Err(format!(
                "slot {slot} generation {generation} {found}; release requires a handoff published with write-handoff so the coordinator can read what is left in the slot before removing it. {TAIL} remedy: write your notes to a file outside the slot, run 'wrkslots write-handoff {slot} --agent {record_agent} --owner-pid <owner-pid> --expected-generation {generation} --from-file <notes>', then retry release"
            ));
        }
        (Some(value), _) => value.clone(),
    };
    let (intent_sequence, intended, completed) = match published {
        Some(tuple) => tuple,
        None => {
            return Err(format!(
                "cannot release slot {slot}: slot {slot} handoff sidecar has no matching owner-authenticated publication intent; preserve it for manual review. {TAIL}"
            ));
        }
    };
    let artifact_sha = artifact
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if artifact.get("source").and_then(Value::as_str) != Some("write-handoff") || !completed {
        return Err(format!(
            "cannot release slot {slot}: slot {slot} handoff publication is incomplete; preserve the sidecar and have its exact owner retry write-handoff. {TAIL}"
        ));
    }
    if artifact != intended {
        return Err(format!(
            "slot {slot} handoff publication is incomplete: the sidecar does not match the completed write-handoff intent {intent_sequence}. {TAIL} remedy: rerun the same 'wrkslots write-handoff {slot} --agent {record_agent} --owner-pid <owner-pid> --expected-generation {generation} --from-file <notes>' command with the same file, then retry release"
        ));
    }
    let handoff_read = events
        .iter()
        .rev()
        .find(|event| {
            if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
                return false;
            }
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            payload.get("slot").and_then(Value::as_str) == Some(slot)
                && payload.get("generation").and_then(Value::as_i64) == Some(generation)
        })
        .map(|event| {
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            payload.get("sha256").and_then(Value::as_str) == Some(artifact_sha.as_str())
                && payload.get("source") == artifact.get("source")
                && payload.get("source_path") == artifact.get("source_path")
                && payload.get("source_identity") == artifact.get("source_identity")
        })
        .unwrap_or(false);
    if let Some(previous) = release_in_effect(&cfg, &record, &events)? {
        let payload = previous.get("payload").cloned().unwrap_or(Value::Null);
        let previous_sha = payload
            .get("handoff")
            .and_then(|handoff| handoff.get("sha256"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if previous_sha == artifact_sha {
            return Ok(json!({
                "machine": selected,
                "slot": slot,
                "generation": generation,
                "sha256": artifact_sha,
                "recorded_at": previous.get("recorded_at").cloned().unwrap_or(Value::Null),
                "sequence": previous.get("sequence").cloned().unwrap_or(Value::Null),
                "outcome": "already released",
                "handoff_read": handoff_read,
            }));
        }
    }
    ensure_event_log(root, &cfg, &selected)?;
    let event = append_event(
        &cfg,
        &selected,
        "owner-released",
        json!({
            "slot": slot,
            "generation": generation,
            "agent": record_agent,
            "owner": owner_value,
            "heartbeat_at": record.get("heartbeat_at").cloned().unwrap_or(Value::Null),
            "handoff": {
                "path": path.to_string_lossy(),
                "sha256": artifact_sha,
                "source": "write-handoff",
                "intent_sequence": intent_sequence,
            },
        }),
    )?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "sha256": artifact_sha,
        "recorded_at": event.get("recorded_at").cloned().unwrap_or(Value::Null),
        "sequence": event.get("sequence").cloned().unwrap_or(Value::Null),
        "outcome": "released",
        "handoff_read": handoff_read,
    }))
}
