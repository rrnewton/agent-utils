//! Writer: record one coordinator read of a slot's handoff.
//!
//! Reproduces the existing `read-handoff` mutation: coordinator
//! capture, artifact resolution (sidecar or legacy file, with the
//! legacy projection write), the `handoff-read` retirement-queue
//! event, and the already-queued outcome. The existing command
//! displays the handoff between two lock phases and rechecks the
//! record; this port records the read in one lock phase, so the
//! displayed-change refusals cannot fire. Fixture registries only
//! until the edition cutover (see `ensure_states::require_fixture`).

use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory};
use crate::config_validate::validate_name;
use crate::ensure_states::{append_event, ensure_event_log, refuse_partial_state, require_fixture};
use crate::global_state::{event_log_status, load_event_chain, EventLogStatus};
use crate::handoff_state::{identity_to_obj, read_regular_file_identity, sidecar_path};
use crate::handoff_write::{committed_project_file, validate_publication, write_noreplace};
use crate::heartbeat_write::{assert_caller, identity_value};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, storage_absent, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

const LIMIT: usize = 1024 * 1024;

/// Record a coordinator read of the named slot's handoff.
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_handoff(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    coordinator_pid: i64,
    coordinator_authorized: bool,
    adopt_legacy_change: bool,
    expected_generation: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    let reader = read_process_identity(coordinator_pid)?;
    assert_caller(&reader, "coordinator")?;
    if adopt_legacy_change {
        return Err("--adopt-legacy-change is unavailable because handoff sidecars are immutable once published; preserve both artifacts for manual review".to_string());
    }
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
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    if slot_type != "agent" {
        return Err(format!(
            "slot {slot} has type validate and cannot carry an agent handoff. state: REFUSED -- no event was recorded. remedy: inspect its run evidence under the project's validation evidence directory"
        ));
    }
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let absent = storage_absent(&cfg, &record);
    if !absent {
        refuse_target_storage(root, slot)?;
    }
    if absent {
        let parent = slot_directory(&cfg, slot, slot_type)?
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_default();
        if let Ok(entries) = std::fs::read_dir(&parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!(".{slot}.fenced."))
                    || name.starts_with(&format!(".{slot}.ownerless-agent."))
                {
                    return Err(format!(
                        "slot {slot} has fenced storage beside its absent directory: {}",
                        entry.path().display()
                    ));
                }
            }
        }
    }
    if expected_generation.is_some() {
        return Err("--expected-generation applies only with the unavailable --adopt-legacy-change compatibility flag".to_string());
    }
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let events = match event_log_status(&directory) {
        EventLogStatus::Derived => load_event_chain(&directory, &selected)?,
        EventLogStatus::Invalid(message) => return Err(message),
        EventLogStatus::Snapshot => Vec::new(),
    };
    validate_publication(&events, slot, generation, &reader_or_owner(&record))?;
    let published = crate::handoff_write::publication(&events, slot, generation);
    let slot_dir = slot_directory(&cfg, slot, slot_type)?;
    let sidecar = match load_sidecar(&cfg, &selected, slot, generation) {
        Ok(value) => value,
        Err(error) => {
            return Err(format!(
                "{error}. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"
            ));
        }
    };
    if let Some(value) = &sidecar {
        match value.get("source").and_then(Value::as_str) {
            Some("write-handoff") => match &published {
                Some((_, intended, completed)) if *completed && intended == value => {}
                Some((_, _, false)) => {
                    return Err(format!(
                        "slot {slot} handoff publication is incomplete; preserve the sidecar and have its exact owner retry write-handoff. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"
                    ));
                }
                _ => {
                    return Err(format!(
                        "slot {slot} handoff sidecar has no matching owner-authenticated publication intent; preserve it for manual review. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"
                    ));
                }
            },
            Some("legacy-slot-file") if published.is_some() => {
                return Err(format!(
                    "slot {slot} has a owner-authenticated write intent; legacy sidecar recovery cannot supersede it. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"
                ));
            }
            _ => {}
        }
    } else if let Some((_, _, completed)) = &published {
        let state = if *completed {
            "completed"
        } else {
            "outstanding"
        };
        return Err(format!(
            "slot {slot} has a {state} owner-authenticated handoff write intent but no canonical sidecar; preserve the slot. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"
        ));
    }
    let legacy_path = slot_dir.join("HANDOFF.md");
    let has_legacy =
        (legacy_path.exists() || legacy_path.is_symlink()) && !committed_project_file(&legacy_path);
    let artifact: Value = match (&sidecar, has_legacy) {
        (Some(value), true) => {
            let (legacy_contents, _) =
                read_regular_file_identity(&legacy_path, "slot HANDOFF.md", LIMIT)
                    .map_err(|error| format!("{error}. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"))?;
            let legacy_sha = format!("{:x}", Sha256::digest(&legacy_contents));
            if value
                .get("contents_utf8")
                .and_then(Value::as_str)
                .map(|text| text.as_bytes())
                != Some(legacy_contents.as_slice())
                || value.get("sha256").and_then(Value::as_str) != Some(legacy_sha.as_str())
            {
                return Err(format!(
                    "slot {slot} contains an unread HANDOFF.md: its handoff sidecar disagrees with the legacy file; preserve both files and reconcile their exact contents. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"
                ));
            }
            value.clone()
        }
        (Some(value), false) => value.clone(),
        (None, true) => {
            let (legacy_contents, identity) =
                read_regular_file_identity(&legacy_path, "slot HANDOFF.md", LIMIT)
                    .map_err(|error| format!("{error}. state: REFUSED -- no read was recorded; preserve both the slot and any handoff sidecar"))?;
            let contents_text = String::from_utf8(legacy_contents.clone()).map_err(|_| {
                format!("slot {slot} handoff cannot be read as UTF-8. state: REFUSED -- no read was recorded. remedy: preserve and repair the handoff file")
            })?;
            json!({
                "schema": 2,
                "machine": selected,
                "slot": slot,
                "generation": generation,
                "recorded_at": crate::ensure_states::utc_now(),
                "source": "legacy-slot-file",
                "source_path": std::fs::canonicalize(&legacy_path)
                    .unwrap_or_else(|_| legacy_path.clone())
                    .to_string_lossy(),
                "source_identity": identity_to_obj(&identity),
                "contents_utf8": contents_text,
                "sha256": format!("{:x}", Sha256::digest(&legacy_contents)),
            })
        }
        (None, false) => {
            if absent {
                return Err(format!(
                    "slot {slot} directory is absent and has no handoff sidecar"
                ));
            }
            return Err(format!(
                "slot {slot} has no handoff sidecar or legacy HANDOFF.md. state: REFUSED -- no read was recorded. remedy: continue the reclaim checks; absence of a handoff is not evidence that the owner is dead"
            ));
        }
    };
    if absent && sidecar.is_none() {
        return Err(format!(
            "slot {slot} directory is absent and has no handoff sidecar"
        ));
    }
    let contents = artifact
        .get("contents_utf8")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sha = artifact
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let latest_read = events.iter().rev().find(|event| {
        if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
            return false;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
    });
    if let Some(event) = &latest_read {
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        let sha_field = payload
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if sha_field.len() != 64
            || !sha_field.chars().all(|ch| ch.is_ascii_hexdigit())
            || !payload
                .get("retirement_enqueued")
                .map(Value::is_boolean)
                .unwrap_or(false)
            || !matches!(
                payload.get("source").and_then(Value::as_str),
                Some("write-handoff") | Some("legacy-slot-file")
            )
        {
            return Err(format!(
                "handoff-read event for slot {slot} generation {generation} is malformed"
            ));
        }
    }
    let latest_matches = latest_read
        .map(|event| {
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            payload.get("sha256").and_then(Value::as_str) == Some(sha.as_str())
                && payload.get("source") == artifact.get("source")
                && payload.get("source_path") == artifact.get("source_path")
                && payload.get("source_identity") == artifact.get("source_identity")
        })
        .unwrap_or(false);
    let already_queued = latest_matches
        && latest_read
            .and_then(|event| event.get("payload"))
            .and_then(|payload| payload.get("retirement_enqueued"))
            .and_then(Value::as_bool)
            == Some(true);
    let payload = json!({
        "slot": slot,
        "generation": generation,
        "sha256": sha,
        "contents_utf8": contents,
        "source": artifact.get("source").cloned().unwrap_or(Value::Null),
        "source_path": artifact.get("source_path").cloned().unwrap_or(Value::Null),
        "source_identity": artifact.get("source_identity").cloned().unwrap_or(Value::Null),
        "reader": identity_value(&reader),
        "coordinator_authorized": coordinator_authorized,
    });
    let projection_needed = artifact.get("source").and_then(Value::as_str)
        == Some("legacy-slot-file")
        && sidecar.is_none();
    if projection_needed && !latest_matches {
        ensure_event_log(root, &cfg, &selected)?;
        let mut first = payload.clone();
        if let Some(object) = first.as_object_mut() {
            object.insert("retirement_enqueued".to_string(), json!(false));
        }
        append_event(&cfg, &selected, "handoff-read", first)?;
    }
    if projection_needed {
        let path = sidecar_path(&cfg.control, &selected, slot, generation);
        write_noreplace(&path, &artifact)?;
        let published = load_sidecar(&cfg, &selected, slot, generation)?.unwrap_or(Value::Null);
        if published != artifact {
            return Err("handoff sidecar changed after its no-replace publication".to_string());
        }
    }
    if !already_queued {
        ensure_event_log(root, &cfg, &selected)?;
        let mut queued = payload.clone();
        if let Some(object) = queued.as_object_mut() {
            object.insert("retirement_enqueued".to_string(), json!(true));
        }
        append_event(&cfg, &selected, "handoff-read", queued)?;
    }
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "sha256": sha,
        "contents_utf8": contents,
        "source": artifact.get("source").cloned().unwrap_or(Value::Null),
        "already_queued": already_queued,
    }))
}

fn load_sidecar(
    cfg: &crate::active_state::Cfg,
    selected: &str,
    slot: &str,
    generation: i64,
) -> Result<Option<Value>> {
    let path = sidecar_path(&cfg.control, selected, slot, generation);
    if !path.exists() && !path.is_symlink() {
        return Ok(None);
    }
    let (bytes, _) = read_regular_file_identity(&path, "handoff sidecar", LIMIT * 6 + 64 * 1024)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("handoff sidecar {} is invalid: not UTF-8", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("handoff sidecar {} is invalid: {error}", path.display()))?;
    if value.get("schema").and_then(Value::as_i64) == Some(1) {
        return Err(format!(
            "slot {slot} handoff sidecar schema 1 lacks auditable source provenance and is permanently manual-only; preserve it for inspection"
        ));
    }
    let mut keys: Vec<&str> = value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort();
    if keys
        != [
            "contents_utf8",
            "generation",
            "machine",
            "recorded_at",
            "schema",
            "sha256",
            "slot",
            "source",
            "source_identity",
            "source_path",
        ]
    {
        return Err(format!(
            "handoff sidecar {} has the wrong keys",
            path.display()
        ));
    }
    if value.get("machine").and_then(Value::as_str) != Some(selected)
        || value.get("slot").and_then(Value::as_str) != Some(slot)
        || value.get("generation").and_then(Value::as_i64) != Some(generation)
    {
        return Err(format!(
            "handoff sidecar identity does not match slot generation: {}",
            path.display()
        ));
    }
    let contents_text = value
        .get("contents_utf8")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let digest = format!("{:x}", Sha256::digest(contents_text.as_bytes()));
    if value.get("sha256").and_then(Value::as_str) != Some(digest.as_str()) {
        return Err(format!(
            "handoff sidecar {} digest does not match its content",
            path.display()
        ));
    }
    Ok(Some(value))
}

fn reader_or_owner(record: &Value) -> crate::status_state::Identity {
    let owner = record.get("owner").cloned().unwrap_or(Value::Null);
    crate::status_state::Identity {
        pid: owner.get("pid").and_then(Value::as_i64).unwrap_or(0),
        start_ticks: owner
            .get("start_ticks")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        boot_id: owner
            .get("boot_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        host_id: owner
            .get("host_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        cgroup_path: owner
            .get("cgroup_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}
