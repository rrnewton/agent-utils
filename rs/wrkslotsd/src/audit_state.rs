//! Reader: audit census (`parse-audit`).
//!
//! Reproduces the Python edition's read-only audit payload for
//! agent rows: per-record verdicts (HELD / DELETABLE / RECOVERABLE /
//! BLOCKED / UNKNOWN) with the oracle's reasons, unregistered
//! on-disk directory rows, interrupted-journal rows, validation-batch
//! seal evidence, stray Git registrations, cache byte accounting,
//! and the disk/gate summary. Validation-row liveness and the
//! bounded/cache-state census machinery stay with the Python
//! edition for now and refuse by name. Read-only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{
    as_int, as_list, as_mapping, as_str, exact_keys, load_cfg, parse_timestamp, slot_directory, Cfg,
};
use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, refuse_symlink, relative_inside, validate_name};
use crate::ensure_states::refuse_partial_state_allow_seals;
use crate::finish_write::{
    cache_globs_for, glob_parts, glob_parts_match, stored_path, stored_repository_path,
    verify_existing_worktree,
};
use crate::global_state::{discover_machines, load_event_chain};
use crate::handoff_state::{load_sidecar_artifact, sidecar_path};
use crate::handoff_write::publication;
use crate::hold_write::{active_document, current_hold, validate_global};
use crate::remove_write::{assert_slot_unused, liveness_answer};
use crate::status_state::process_state;

type Result<T> = std::result::Result<T, String>;

struct JournalSlot {
    slot: String,
    machine: String,
    state: String,
}

fn journal_slots(cfg: &Cfg) -> Result<BTreeMap<String, JournalSlot>> {
    let mut found: BTreeMap<String, JournalSlot> = BTreeMap::new();
    let entries = std::fs::read_dir(&cfg.control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            cfg.control.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot list control directory: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let machine = ["ACTIVE.", "CREATE.", "FINISH."].iter().find_map(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(".journal"))
        });
        let Some(machine) = machine else { continue };
        if machine.is_empty() {
            continue;
        }
        let text = std::fs::read_to_string(entry.path())
            .map_err(|error| format!("cannot read recovery journal {name}: {error}"))?;
        let raw: Value = serde_json::from_str(&text)
            .map_err(|error| format!("recovery journal {name} is malformed JSON: {error}"))?;
        let kind = raw.get("kind").and_then(Value::as_str).unwrap_or_default();
        if !matches!(kind, "create" | "finish" | "import-existing") {
            continue;
        }
        let slot = raw
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        validate_name(&slot, "slot")?;
        if let Some(previous) = found.get(&slot) {
            return Err(format!(
                "slot '{slot}' has recovery journals on both {} and {machine}",
                previous.machine
            ));
        }
        found.insert(
            slot.clone(),
            JournalSlot {
                slot,
                machine: machine.to_string(),
                state: format!("{kind}-journal"),
            },
        );
    }
    Ok(found)
}

/// The latest owner release for one generation and why it is void,
/// mirroring `_owner_release_state` / `_owner_release_void_reason`.
fn release_state(
    cfg: &Cfg,
    record: &Value,
    events: &[Value],
) -> Result<(Option<Value>, Option<String>)> {
    if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
        return Ok((None, None));
    }
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut latest: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("owner-released") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
        {
            latest = Some(event.clone());
        }
    }
    let Some(event) = latest else {
        return Ok((None, None));
    };
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
    let handoff = payload.get("handoff").cloned().unwrap_or(Value::Null);
    let release = json!({
        "sequence": event.get("sequence").cloned().unwrap_or(Value::Null),
        "recorded_at": event.get("recorded_at").cloned().unwrap_or(Value::Null),
        "heartbeat_at": payload.get("heartbeat_at").cloned().unwrap_or(Value::Null),
        "owner": payload.get("owner").cloned().unwrap_or(Value::Null),
        "agent": payload.get("agent").cloned().unwrap_or(Value::Null),
        "handoff_path": handoff.get("path").cloned().unwrap_or(Value::Null),
        "sha256": handoff.get("sha256").cloned().unwrap_or(Value::Null),
        "intent_sequence": handoff.get("intent_sequence").cloned().unwrap_or(Value::Null),
    });
    let void = release_void_reason(cfg, record, events, &payload, &handoff)?;
    Ok((Some(release), void))
}

fn release_void_reason(
    cfg: &Cfg,
    record: &Value,
    events: &[Value],
    payload: &Value,
    handoff: &Value,
) -> Result<Option<String>> {
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let release_agent = payload
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if release_agent != record_agent {
        return Ok(Some(format!(
            "the slot now belongs to agent {record_agent}, not {release_agent}"
        )));
    }
    if payload.get("owner") != record.get("owner") {
        return Ok(Some(
            "the recorded owner process generation changed after the release".to_string(),
        ));
    }
    let released_heartbeat = payload
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_heartbeat = record
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if released_heartbeat != current_heartbeat {
        return Ok(Some(format!(
            "the heartbeat was renewed after the release (released at heartbeat {released_heartbeat}, now {current_heartbeat})"
        )));
    }
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let intent_sequence = handoff.get("intent_sequence").and_then(Value::as_i64);
    let published = publication(events, slot, generation);
    let publication_ok = match &published {
        Some((sequence, _artifact, completed)) => Some(*sequence) == intent_sequence && *completed,
        None => false,
    };
    if !publication_ok {
        return Ok(Some(
            "the handoff publication changed after the release".to_string(),
        ));
    }
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let artifact = match load_sidecar_artifact(cfg, machine, slot, slot_type, generation) {
        Ok(value) => value,
        Err(error) => {
            return Ok(Some(format!(
                "the released handoff sidecar cannot be read: {error}"
            )));
        }
    };
    let Some(artifact) = artifact else {
        return Ok(Some("the released handoff sidecar is missing".to_string()));
    };
    let expected_path = sidecar_path(&cfg.control, machine, slot, generation);
    let published_artifact = published
        .as_ref()
        .map(|(_sequence, artifact, _done)| artifact);
    if artifact.value.get("source").and_then(Value::as_str) != Some("write-handoff")
        || handoff.get("path").and_then(Value::as_str)
            != Some(expected_path.to_string_lossy().as_ref())
        || artifact.value.get("sha256") != handoff.get("sha256")
        || published_artifact != Some(&artifact.value)
    {
        return Ok(Some(
            "the handoff sidecar no longer matches the released bytes".to_string(),
        ));
    }
    Ok(None)
}

fn release_obj(release: &Value, void_reason: Option<&str>) -> Value {
    let recorded_at = release
        .get("recorded_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({
        "state": if void_reason.is_none() { "released" } else { "void" },
        "summary": match void_reason {
            None => format!("RELEASED by live owner at {recorded_at}"),
            Some(reason) => format!("release at {recorded_at} no longer applies: {reason}"),
        },
        "event_sequence": release.get("sequence").cloned().unwrap_or(Value::Null),
        "recorded_at": release.get("recorded_at").cloned().unwrap_or(Value::Null),
        "heartbeat_at": release.get("heartbeat_at").cloned().unwrap_or(Value::Null),
        "owner": release.get("owner").cloned().unwrap_or(Value::Null),
        "handoff_sha256": release.get("sha256").cloned().unwrap_or(Value::Null),
        "void_reason": void_reason,
    })
}

/// Latest handoff-read payload for one generation (audit's read gate).
fn latest_handoff_read(events: &[Value], record: &Value) -> Option<Value> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut latest: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
        {
            latest = Some(payload);
        }
    }
    latest
}

/// The oracle's `_assert_handoff_read` as an audit reason producer.
fn handoff_read_reason(
    cfg: &Cfg,
    record: &Value,
    events: &[Value],
    slot_path: &Path,
) -> Option<String> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sidecar = load_sidecar_artifact(cfg, machine, slot, slot_type, generation);
    let latest = latest_handoff_read(events, record);
    match sidecar {
        Err(error) => Some(error),
        Ok(None) => {
            let legacy = slot_path.join("HANDOFF.md");
            if legacy.exists() || legacy.is_symlink() {
                // Legacy artifacts need the file-identity projection the
                // finish/remove slices still leave to Python for
                // quarantine; audit treats an unread legacy file like
                // the oracle's unread refusal below.
                let read_matches = latest.as_ref().is_some_and(|read| {
                    read.get("source").and_then(Value::as_str) == Some("legacy-slot-file")
                });
                if !read_matches {
                    return Some(format!(
                        "slot {slot} contains an unread HANDOFF.md or handoff sidecar. state: REFUSED -- no checkout was salvaged or removed. remedy: run 'wrkslots read-handoff {slot} --coordinator-authorized --coordinator-pid <current-coordinator-pid>' and read its output, then retry remove"
                    ));
                }
                return None;
            }
            if latest.is_some() {
                return Some(format!(
                    "slot {slot} handoff artifact disappeared after it was read. state: REFUSED -- no checkout was salvaged or removed. remedy: restore the exact generation-bound sidecar or legacy HANDOFF.md"
                ));
            }
            None
        }
        Ok(Some(artifact)) => {
            let matches = latest.as_ref().is_some_and(|read| {
                read.get("sha256") == artifact.value.get("sha256")
                    && read.get("source") == artifact.value.get("source")
                    && read.get("source_path") == artifact.value.get("source_path")
                    && read.get("source_identity") == artifact.value.get("source_identity")
            });
            if !matches {
                return Some(format!(
                    "slot {slot} contains an unread HANDOFF.md or handoff sidecar. state: REFUSED -- no checkout was salvaged or removed. remedy: run 'wrkslots read-handoff {slot} --coordinator-authorized --coordinator-pid <current-coordinator-pid>' and read its output, then retry remove"
                ));
            }
            None
        }
    }
}

/// Allocated bytes (st_blocks * 512) of every cache directory the
/// configured globs name inside one record's checkouts, with the
/// fail-closed cache errors audit reports.
fn cache_accounting(cfg: &Cfg, config: &Value, record: &Value) -> (Option<i64>, Option<String>) {
    use std::os::unix::fs::MetadataExt as _;
    let checkouts = record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut total: i64 = 0;
    for checkout in &checkouts {
        let name = checkout
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let globs = cache_globs_for(config, name);
        if globs.is_empty() {
            continue;
        }
        let raw_path = checkout
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let checkout_path = match stored_path(cfg, raw_path, "checkout path") {
            Ok(path) => path,
            Err(error) => return (None, Some(error)),
        };
        if !checkout_path.is_dir() {
            continue;
        }
        let patterns: Vec<Vec<String>> = globs.iter().map(|item| glob_parts(item)).collect();
        // Enumerate directories like the remove slice's expansion.
        let mut directories: Vec<PathBuf> = Vec::new();
        let mut pending: Vec<PathBuf> = vec![checkout_path.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() || !kind.is_dir() {
                    continue;
                }
                if directory == checkout_path && entry.file_name() == ".git" {
                    continue;
                }
                directories.push(entry.path());
                pending.push(entry.path());
            }
        }
        let checkout_device = std::fs::metadata(&checkout_path)
            .map(|meta| meta.dev())
            .unwrap_or(0);
        for candidate in &directories {
            let relative = candidate.strip_prefix(&checkout_path).unwrap_or(candidate);
            let parts: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            if !patterns
                .iter()
                .any(|pattern| glob_parts_match(pattern, &parts))
            {
                continue;
            }
            match allocated_bytes(candidate, checkout_device) {
                Ok(bytes) => total += bytes,
                Err(error) => return (None, Some(error)),
            }
        }
    }
    (Some(total), None)
}

fn allocated_bytes(path: &Path, device: u64) -> Result<i64> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect cache entry {}: {error}", path.display()))?;
    if meta.dev() != device {
        return Err(format!(
            "cache path crossed onto another filesystem: {}",
            path.display()
        ));
    }
    let mut total = (meta.blocks() * 512) as i64;
    if !meta.is_dir() {
        return Ok(total);
    }
    let entries = std::fs::read_dir(path)
        .map_err(|error| format!("cannot inspect cache entry {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect cache entry: {error}"))?;
        if entry.file_name() == ".git" {
            return Err(format!(
                "cache {} contains nested Git repository at {}; nested-repository judging stays with the Python edition for now",
                path.display(),
                entry.path().display()
            ));
        }
        total += allocated_bytes(&entry.path(), device)?;
    }
    Ok(total)
}

/// The record digest the oracle publishes: `_event_digest` of
/// `_record_to_obj` (the stored object, with `slot_type` defaulted).
fn record_digest(record: &Value) -> Result<String> {
    let mut obj = record.clone();
    if obj.get("slot_type").is_none() {
        obj.as_object_mut()
            .expect("record object")
            .insert("slot_type".to_string(), json!("agent"));
    }
    canonical_sha256(&obj).map_err(|error| error.to_string())
}

fn heartbeat_diagnosis(record: &Value) -> Result<(i64, bool)> {
    let stamp = record
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    parse_timestamp(stamp, "heartbeat_at")?;
    let parsed = chrono::DateTime::parse_from_rfc3339(stamp)
        .map_err(|error| format!("heartbeat timestamp is invalid: {error}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let age = now - parsed.timestamp();
    let ttl = record
        .get("heartbeat_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let age = age.max(0);
    Ok((age, age > ttl))
}

/// Journal and stray-registration overrides for one registered row,
/// in the oracle's order: an interrupted journal blocks the row,
/// then (for DELETABLE/BLOCKED rows with the directory present) stray
/// Git registrations block it too.
fn apply_overrides(
    row: &mut Value,
    journals: &BTreeMap<String, JournalSlot>,
    strays: &BTreeMap<(String, String), Vec<String>>,
    slot: &str,
    slot_type: &str,
    slot_absent: bool,
    sealed_generation: Option<i64>,
) {
    if let Some(journal) = journals.get(slot) {
        let state_text = journal.state.replace('-', " ");
        let row_reasons = row
            .get_mut("reasons")
            .and_then(Value::as_array_mut)
            .expect("reasons list");
        row_reasons.insert(
            0,
            json!(format!(
                "interrupted {state_text} requires wrkslots recover"
            )),
        );
        row.as_object_mut()
            .expect("row object")
            .insert("verdict".to_string(), json!("BLOCKED"));
    }
    if let Some(sealed_generation) = sealed_generation {
        let active = row.get("generation").and_then(Value::as_i64).unwrap_or(0);
        let (reason, verdict) = if sealed_generation == active {
            (
                "interrupted validation-batch seal requires wrkslots recover".to_string(),
                "BLOCKED",
            )
        } else {
            (
                format!(
                    "validation-batch seal cannot be attributed to the active generation: sealed={sealed_generation} active={active}"
                ),
                "UNKNOWN",
            )
        };
        let row_reasons = row
            .get_mut("reasons")
            .and_then(Value::as_array_mut)
            .expect("reasons list");
        row_reasons.insert(0, json!(reason));
        row.as_object_mut()
            .expect("row object")
            .insert("verdict".to_string(), json!(verdict));
    }
    let verdict = row
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if matches!(verdict.as_str(), "DELETABLE" | "BLOCKED") && !slot_absent {
        if let Some(stray) = strays.get(&(slot_type.to_string(), slot.to_string())) {
            let row_reasons = row
                .get_mut("reasons")
                .and_then(Value::as_array_mut)
                .expect("reasons list");
            row_reasons.extend(stray.iter().map(|item| json!(item)));
            row.as_object_mut()
                .expect("row object")
                .insert("verdict".to_string(), json!("BLOCKED"));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn audit_record(
    root: Option<&Path>,
    cfg: &Cfg,
    config: &Value,
    record: &Value,
    events: &[Value],
    journals: &BTreeMap<String, JournalSlot>,
    strays: &BTreeMap<(String, String), Vec<String>>,
    sealed_generation: Option<i64>,
) -> Result<(Value, bool)> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let owner_identity =
        crate::status_state::identity_from_value(record.get("owner").unwrap_or(&Value::Null));
    let (owner_state, owner_detail) = process_state(owner_identity.as_ref());
    let liveness = liveness_answer(cfg, config, record)?;
    let (heartbeat_age, heartbeat_expired) = heartbeat_diagnosis(record)?;
    let mut agent_running = !(liveness.state == "dead" && owner_state == "dead");
    let hold = current_hold(root, cfg, &machine, &slot)?;
    let (release, void_reason) = release_state(cfg, record, events)?;
    let release_honoured = release.is_some()
        && void_reason.is_none()
        && matches!(owner_state.as_str(), "live" | "dead")
        && record.get("owner").is_some_and(|owner| !owner.is_null());
    let release_fields = match &release {
        Some(release) => release_obj(release, void_reason.as_deref()),
        None => Value::Null,
    };
    let slot_path = slot_directory(cfg, &slot, &slot_type)?;
    let slot_absent = !slot_path.exists() && !slot_path.is_symlink();
    let (cache_bytes, cache_error) = cache_accounting(cfg, config, record);
    let (cache_bytes_value, cache_status, cache_error) = if slot_absent {
        (json!(0), "complete", None)
    } else {
        match (&cache_bytes, &cache_error) {
            (Some(bytes), None) => (json!(bytes), "complete", None),
            _ => (Value::Null, "error", cache_error),
        }
    };
    let record_sha256 = record_digest(record)?;
    let base = |verdict: &str, reasons: Vec<String>| -> Value {
        let mut row = json!({
            "slot": slot,
            "machine": machine,
            "agent": record.get("agent").cloned().unwrap_or(Value::Null),
            "slot_type": slot_type,
            "generation": generation,
            "record_sha256": record_sha256,
            "verdict": verdict,
            "reasons": reasons,
            "owner_state": owner_state,
            "liveness_state": liveness.state,
            "heartbeat_age_seconds": heartbeat_age,
            "heartbeat_expired": heartbeat_expired,
            "cache_bytes": cache_bytes_value.clone(),
            "cache_error": cache_error.clone(),
            "cache_status": cache_status,
        });
        if let Some(release) = &release {
            let _ = release;
            row.as_object_mut()
                .expect("row object")
                .insert("owner_release".to_string(), release_fields.clone());
        }
        row
    };
    if !hold.is_null() {
        let reason = hold
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut row = base("HELD", vec![reason]);
        apply_overrides(
            &mut row,
            journals,
            strays,
            &slot,
            &slot_type,
            slot_absent,
            sealed_generation,
        );
        return Ok((row, agent_running));
    }
    let mut reasons: Vec<String> = Vec::new();
    let mut removal_only_reasons: Vec<String> = Vec::new();
    if !release_honoured {
        if !heartbeat_expired {
            let ttl = record
                .get("heartbeat_ttl_seconds")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            reasons.push(format!(
                "heartbeat age {heartbeat_age}s has not exceeded the {ttl}s time-to-live"
            ));
        }
        if liveness.state != "dead" {
            reasons.push(format!(
                "liveness is {}: {}",
                liveness.state, liveness.detail
            ));
        }
        if record.get("owner").is_none_or(|owner| owner.is_null()) {
            reasons
                .push("no owner process generation is recorded, so death is unknown".to_string());
            removal_only_reasons.push(reasons.last().expect("just pushed").clone());
        } else if owner_state != "dead" {
            reasons.push(format!("recorded owner is {owner_state}: {owner_detail}"));
        }
    }
    if !slot_absent {
        if let Err(error) = crate::finish_write::assert_slot_contents(cfg, record) {
            reasons.push(error);
        } else {
            if let Some(reason) = handoff_read_reason(cfg, record, events, &slot_path) {
                reasons.push(reason);
            }
            let checkouts = record
                .get("checkouts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for checkout in &checkouts {
                let name = checkout
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let raw_path = checkout
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let raw_repo = checkout
                    .get("repository")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let outcome = (|| -> Result<()> {
                    let path = stored_path(cfg, raw_path, "checkout path")?;
                    let (_rel, repository) = stored_repository_path(cfg, raw_repo)?;
                    verify_existing_worktree(&repository, &path)?;
                    let branch = {
                        let output = std::process::Command::new("git")
                            .args([
                                "-C",
                                &path.to_string_lossy(),
                                "symbolic-ref",
                                "--quiet",
                                "--short",
                                "HEAD",
                            ])
                            .output()
                            .map_err(|error| format!("cannot execute Git: {error}"))?;
                        if !output.status.success() {
                            return Err(format!("checkout {name} branch changed"));
                        }
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    };
                    if branch
                        != checkout
                            .get("branch")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    {
                        return Err(format!("checkout {name} branch changed"));
                    }
                    let (authority, _url) = crate::finish_write::remote_authority(
                        &path,
                        checkout
                            .get("remote")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )?;
                    if authority
                        != checkout
                            .get("remote_url_sha256")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    {
                        return Err(format!("checkout {name} remote URL changed"));
                    }
                    Ok(())
                })();
                if let Err(error) = outcome {
                    reasons.push(error);
                }
            }
        }
    }
    if slot_absent {
        reasons.retain(|reason| !removal_only_reasons.contains(reason));
    }
    if slot_absent && release_honoured && owner_state != "dead" {
        reasons.push(format!(
            "recorded owner is {owner_state}: {owner_detail}; the recover-absent commands require the recorded owner generation to be dead"
        ));
    }
    if (release_honoured
        || (heartbeat_expired && liveness.state == "dead" && owner_state == "dead"))
        && reasons.is_empty()
        && !slot_absent
    {
        if let Err(error) = assert_slot_unused(&slot_path, std::process::id() as i64) {
            reasons.push(error);
            agent_running = true;
        }
    }
    if slot_absent {
        let row_digest = record_digest(record)?;
        let plan =
            format!("wrkslots recover-absent-agent-rows --row {slot}={generation}={row_digest}");
        reasons.push(format!(
            "slot directory is absent: {}; nothing occupies the path, so this row is retired by recovery, not removal: on machine {machine}, run '{plan}' for the read-only plan, then repeat it with '--apply --coordinator-authorized --coordinator-pid <coordinator-pid>'",
            slot_path.display()
        ));
    }
    if let Some(error) = &cache_error {
        if !slot_absent {
            reasons.push(format!("cache inspection failed: {error}"));
        }
    }
    let verdict = if slot_absent {
        if reasons.len() == 1 {
            "RECOVERABLE"
        } else {
            "BLOCKED"
        }
    } else if reasons.is_empty() {
        "DELETABLE"
    } else {
        "BLOCKED"
    };
    let mut row = base(verdict, reasons);
    apply_overrides(
        &mut row,
        journals,
        strays,
        &slot,
        &slot_type,
        slot_absent,
        sealed_generation,
    );
    Ok((row, agent_running))
}

type SealTargets = Vec<(String, i64, PathBuf)>;
type SealErrors = Vec<(Option<String>, String, String)>;

struct SealTarget {
    machine: String,
    slot: String,
    generation: i64,
    path: PathBuf,
}

/// The machine a seal journal filename names, when it matches the
/// oracle's `VALIDATE-BATCH-SEAL.<machine>.journal` pattern.
fn seal_machine_from_name(name: &str) -> Option<String> {
    let middle = name
        .strip_prefix("VALIDATE-BATCH-SEAL.")?
        .strip_suffix(".journal")?;
    if middle.is_empty() || middle.len() > 64 {
        return None;
    }
    let mut chars = middle.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !middle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    Some(middle.to_string())
}

fn seal_actor(value: &Value, label: &str) -> Result<()> {
    if value.is_null() {
        return Err("validation-batch seal journal has no actor".to_string());
    }
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["pid", "start_ticks", "boot_id", "host_id", "cgroup_path"],
        &[],
        label,
    )?;
    as_int(&raw["pid"], &format!("{label}.pid"), 1)?;
    as_int(&raw["start_ticks"], &format!("{label}.start_ticks"), 1)?;
    let boot_id = as_str(&raw["boot_id"], &format!("{label}.boot_id"))?;
    let host_id = as_str(&raw["host_id"], &format!("{label}.host_id"))?;
    let cgroup_path = as_str(&raw["cgroup_path"], &format!("{label}.cgroup_path"))?;
    if boot_id.is_empty() || host_id.is_empty() || !cgroup_path.starts_with('/') {
        return Err(format!(
            "{label} has an invalid boot, machine, or cgroup identity"
        ));
    }
    Ok(())
}

fn cleanup_identity(value: &Value, label: &str) -> Result<()> {
    let values = as_list(value, label)?;
    if values.len() == 3 {
        for (index, item) in values.iter().enumerate() {
            as_int(item, &format!("{label}[{index}]"), 1)?;
        }
        return Ok(());
    }
    if values.len() != 4 {
        return Err(format!(
            "{label} must contain dev, ino, file-handle type, and file-handle bytes"
        ));
    }
    as_int(&values[0], &format!("{label}[0]"), 1)?;
    as_int(&values[1], &format!("{label}[1]"), 1)?;
    as_int(&values[2], &format!("{label}[2]"), 0)?;
    let handle = as_str(&values[3], &format!("{label}[3]"))?;
    if handle.is_empty()
        || handle.len() > 256
        || handle.len() % 2 != 0
        || !handle
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return Err(format!(
            "{label}[3] must be bounded lowercase hexadecimal bytes"
        ));
    }
    Ok(())
}

fn cleanup_target(cfg: &Cfg, value: &Value, label: &str) -> Result<(String, i64, PathBuf)> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["slot", "generation", "path", "original_mode", "identity"],
        &[],
        label,
    )?;
    let slot = validate_name(as_str(&raw["slot"], &format!("{label}.slot"))?, "slot")?;
    let generation = as_int(&raw["generation"], &format!("{label}.generation"), 1)?;
    let (_relative, path) = relative_inside(
        &cfg.root,
        as_str(&raw["path"], &format!("{label}.path"))?,
        &format!("{label}.path"),
    )?;
    if path != slot_directory(cfg, &slot, "validate")? {
        return Err(format!(
            "{label}.path does not match validation slot {slot}"
        ));
    }
    let original_mode = as_int(&raw["original_mode"], &format!("{label}.original_mode"), 0)?;
    if original_mode > 0o7777 {
        return Err(format!("{label}.original_mode exceeds permission bits"));
    }
    cleanup_identity(&raw["identity"], &format!("{label}.identity"))?;
    Ok((slot, generation, path))
}

/// One seal journal, validated like `_validate_batch_seal_journal`.
fn seal_journal(cfg: &Cfg, path: &Path, value: &Value) -> Result<(String, SealTargets)> {
    let label = "validation-batch seal journal";
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "created_at",
            "actor",
            "targets",
        ],
        &[],
        label,
    )?;
    if as_int(&raw["schema"], &format!("{label}.schema"), 0)? != 1 {
        return Err("unsupported validation-batch seal journal schema".to_string());
    }
    if raw["kind"] != Value::String("validate-batch-seal".to_string()) {
        return Err("validation-batch seal journal has the wrong operation kind".to_string());
    }
    let machine = validate_name(
        as_str(&raw["machine"], &format!("{label}.machine"))?,
        "machine",
    )?;
    let expected = cfg
        .control
        .join(format!("VALIDATE-BATCH-SEAL.{machine}.journal"));
    if path != expected {
        return Err(
            "validation-batch seal journal filename does not match its machine".to_string(),
        );
    }
    parse_timestamp(
        as_str(&raw["created_at"], &format!("{label}.created_at"))?,
        &format!("{label}.created_at"),
    )?;
    seal_actor(&raw["actor"], &format!("{label}.actor"))?;
    let items = as_list(&raw["targets"], &format!("{label}.targets"))?;
    let mut targets: Vec<(String, i64, PathBuf)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        targets.push(cleanup_target(
            cfg,
            item,
            &format!("{label}.targets[{index}]"),
        )?);
    }
    if targets.len() > 8 {
        return Err("validation-batch seal journal exceeds the batch limit".to_string());
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    if targets
        .iter()
        .any(|(slot, _generation, _path)| !seen.insert(slot.as_str()))
    {
        return Err("validation-batch seal journal repeats a slot".to_string());
    }
    Ok((machine, targets))
}

fn read_seal_json(path: &Path) -> Result<Value> {
    refuse_symlink(path, "validation-batch seal journal")?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing validation-batch seal journal: {}", path.display())
        } else {
            format!(
                "cannot read validation-batch seal journal {}: {error}",
                path.display()
            )
        }
    })?;
    serde_json::from_str(&text)
        .map_err(|error| format!("validation-batch seal journal is malformed JSON: {error}"))
}

/// Seal targets and per-journal errors, in the oracle's order: one
/// bad journal hides neither the other journals nor the rows.
fn seal_evidence(cfg: &Cfg) -> (Vec<SealTarget>, SealErrors) {
    let mut targets: Vec<SealTarget> = Vec::new();
    let mut errors: Vec<(Option<String>, String, String)> = Vec::new();
    for path in crate::heartbeat_write::seal_paths(cfg) {
        let name = path
            .file_name()
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        let machine = seal_machine_from_name(&name);
        let parsed = read_seal_json(&path).and_then(|value| seal_journal(cfg, &path, &value));
        match parsed {
            Ok((parsed_machine, parsed_targets)) => {
                targets.extend(parsed_targets.into_iter().map(
                    |(slot, generation, target_path)| SealTarget {
                        machine: parsed_machine.clone(),
                        slot,
                        generation,
                        path: target_path,
                    },
                ));
            }
            Err(error) => errors.push((machine, name, error)),
        }
    }
    (targets, errors)
}

/// The audit payload (`{"schema": 2, ...}`), minus the metrics phase
/// timings the oracle measures live.
pub(crate) fn parse_audit(root: Option<&Path>, machine_override: Option<&str>) -> Result<Value> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    refuse_partial_state_allow_seals(&cfg.control)?;
    validate_global(root, &cfg)?;
    let journals = journal_slots(&cfg)?;
    let machines = discover_machines(&cfg.control)?;
    let mut records: Vec<Value> = Vec::new();
    let mut events_by_machine: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for machine in &machines {
        let document = active_document(root, &cfg, machine)?;
        if let Some(slots) = document.get("slots").and_then(Value::as_array) {
            records.extend(slots.iter().cloned());
        }
        let directory = cfg.control.join(format!("EVENTS.{machine}"));
        let events = if directory.is_dir() {
            load_event_chain(&directory, machine)?
        } else {
            Vec::new()
        };
        events_by_machine.insert(machine.clone(), events);
    }
    if records
        .iter()
        .any(|record| record.get("slot_type").and_then(Value::as_str) == Some("validate"))
    {
        return Err(
            "audit validation-row liveness stays with the Python edition for now".to_string(),
        );
    }
    let (seal_targets, seal_errors) = seal_evidence(&cfg);
    let mut seal_by_slot: BTreeMap<(String, String), (i64, PathBuf)> = BTreeMap::new();
    for target in &seal_targets {
        seal_by_slot.insert(
            (target.machine.clone(), target.slot.clone()),
            (target.generation, target.path.clone()),
        );
    }
    let mut represented_seals: BTreeSet<(String, String, i64)> = BTreeSet::new();
    // Stray Git registrations inside the managed root.
    let mut strays: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let git_doc = crate::git_storage_state::parse_storage_git(root)?;
    if let Some(findings) = git_doc.get("findings").and_then(Value::as_array) {
        for finding in findings {
            if finding.get("kind").and_then(Value::as_str) == Some("git-registration-without-row") {
                let key = (
                    finding
                        .get("slot_type")
                        .and_then(Value::as_str)
                        .unwrap_or("agent")
                        .to_string(),
                    finding
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                strays.entry(key).or_default().push(
                    finding
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
    }
    // On-disk slot directories.
    let validate_root = crate::storage_state::managed_root(&cfg).join("validate");
    let mut on_disk: BTreeSet<(String, String)> = BTreeSet::new();
    for (slot_type, root_path) in [
        ("agent", cfg.worktrees.clone()),
        ("validate", validate_root.clone()),
    ] {
        if !root_path.is_dir() {
            continue;
        }
        let listing = std::fs::read_dir(&root_path).map_err(|error| {
            format!(
                "cannot enumerate worktree slots in {}: {error}",
                root_path.display()
            )
        })?;
        for entry in listing {
            let entry =
                entry.map_err(|error| format!("cannot enumerate worktree slots: {error}"))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("EVENTS.") {
                continue;
            }
            // The validate-slots directory itself is not an agent slot.
            if slot_type == "agent" && path == validate_root {
                continue;
            }
            if path.is_dir() && !path.is_symlink() {
                on_disk.insert((slot_type.to_string(), name));
            }
        }
    }
    let registered: BTreeSet<(String, String)> = records
        .iter()
        .map(|record| {
            (
                record
                    .get("slot_type")
                    .and_then(Value::as_str)
                    .unwrap_or("agent")
                    .to_string(),
                record
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    let mut rows: Vec<Value> = Vec::new();
    let mut running_agent_count: i64 = 0;
    for record in &records {
        let machine = record
            .get("machine")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slot = record
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let sealed = seal_by_slot.get(&(machine.to_string(), slot.to_string()));
        let events = events_by_machine.get(machine).cloned().unwrap_or_default();
        let (row, running) = audit_record(
            root,
            &cfg,
            &config,
            record,
            &events,
            &journals,
            &strays,
            sealed.map(|(generation, _path)| *generation),
        )?;
        running_agent_count += i64::from(running);
        if let Some((generation, _path)) = sealed {
            represented_seals.insert((machine.to_string(), slot.to_string(), *generation));
        }
        rows.push(row);
    }
    for (slot_type, slot) in on_disk.difference(&registered) {
        let journal = journals.get(slot);
        let sealed = seal_by_slot.get(&(selected.clone(), slot.clone()));
        let (reason, owner_state) = if let Some(journal) = journal {
            (
                format!(
                    "interrupted {} requires wrkslots recover",
                    journal.state.replace('-', " ")
                ),
                journal.state.clone(),
            )
        } else if sealed.is_some() {
            (
                "interrupted validation-batch seal requires wrkslots recover".to_string(),
                "validation-batch-seal".to_string(),
            )
        } else if slot_type == "validate" {
            (
                "validation directory has no active registry row; preserve and inspect it, and use import-existing only for a demonstrably live owner; destructive ownerless Git worktree recovery is disabled".to_string(),
                "unregistered".to_string(),
            )
        } else {
            (
                "directory has no active registry row; inspect or import it".to_string(),
                "unregistered".to_string(),
            )
        };
        let (machine_value, generation_value) = if let Some(journal) = journal {
            (json!(journal.machine), Value::Null)
        } else if let Some((generation, _path)) = sealed {
            (json!(selected), json!(generation))
        } else {
            (Value::Null, Value::Null)
        };
        // A validation directory is not a cache subject, so the
        // oracle's census reports it complete with no bytes.
        let (cache_bytes, cache_error, cache_status) = if slot_type == "validate" {
            (json!(0), Value::Null, "complete")
        } else {
            (
                Value::Null,
                json!(format!(
                    "unregistered slot {slot} contains no Git checkouts"
                )),
                "error",
            )
        };
        rows.push(json!({
            "slot": slot,
            "slot_type": slot_type,
            "machine": machine_value,
            "agent": Value::Null,
            "generation": generation_value,
            "record_sha256": Value::Null,
            "verdict": "BLOCKED",
            "reasons": [reason],
            "owner_state": owner_state,
            "liveness_state": "unverifiable",
            "cache_bytes": cache_bytes,
            "cache_error": cache_error,
            "cache_status": cache_status,
        }));
        if let Some((generation, _path)) = sealed {
            represented_seals.insert((selected.clone(), slot.clone(), *generation));
        }
    }
    for journal in journals.values() {
        let known = registered.iter().any(|(_type, slot)| slot == &journal.slot)
            || on_disk.iter().any(|(_type, slot)| slot == &journal.slot);
        if known {
            continue;
        }
        rows.push(json!({
            "slot": journal.slot,
            "slot_type": Value::Null,
            "machine": journal.machine,
            "agent": Value::Null,
            "generation": Value::Null,
            "record_sha256": Value::Null,
            "verdict": "BLOCKED",
            "reasons": [format!(
                "interrupted {} requires wrkslots recover",
                journal.state.replace('-', " ")
            )],
            "owner_state": journal.state,
            "liveness_state": "unverifiable",
            "cache_bytes": 0,
            "cache_error": Value::Null,
            "cache_status": "complete",
        }));
    }
    for target in &seal_targets {
        if represented_seals.contains(&(
            target.machine.clone(),
            target.slot.clone(),
            target.generation,
        )) {
            continue;
        }
        rows.push(json!({
            "slot": target.slot,
            "slot_type": "validate",
            "machine": target.machine,
            "agent": Value::Null,
            "generation": target.generation,
            "record_sha256": Value::Null,
            "verdict": "BLOCKED",
            "reasons": [format!(
                "interrupted validation-batch seal requires wrkslots recover; recorded path={}",
                target.path.display()
            )],
            "owner_state": "validation-batch-seal",
            "liveness_state": "unverifiable",
            "cache_bytes": 0,
            "cache_error": Value::Null,
            "cache_status": "complete",
        }));
    }
    for (machine, journal_name, error) in &seal_errors {
        rows.push(json!({
            "slot": journal_name,
            "slot_type": "validate",
            "machine": machine,
            "agent": Value::Null,
            "generation": Value::Null,
            "record_sha256": Value::Null,
            "verdict": "UNKNOWN",
            "reasons": [format!(
                "validation-batch seal evidence is unreadable: {error}"
            )],
            "owner_state": "validation-batch-seal",
            "liveness_state": "unverifiable",
            "cache_bytes": 0,
            "cache_error": Value::Null,
            "cache_status": "complete",
        }));
    }
    rows.sort_by(|a, b| {
        a.get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("slot").and_then(Value::as_str).unwrap_or_default())
    });
    let worktree_count = on_disk.len();
    let disk = disk_status(&cfg, &config);
    let attention: Vec<String> = first_seen_names(
        rows.iter()
            .filter(|row| {
                row.get("verdict").and_then(Value::as_str) == Some("DELETABLE")
                    || row.get("verdict").and_then(Value::as_str) == Some("RECOVERABLE")
                    || row.get("owner_state").and_then(Value::as_str) == Some("unregistered")
                    || row
                        .get("reasons")
                        .and_then(Value::as_array)
                        .is_some_and(|reasons| {
                            reasons.iter().any(|reason| {
                                reason
                                    .as_str()
                                    .is_some_and(|text| text.starts_with("interrupted "))
                            })
                        })
            })
            .map(|row| {
                row.get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    );
    let attention_set: BTreeSet<&String> = attention.iter().collect();
    let unknown: Vec<String> = first_seen_names(
        rows.iter()
            .filter(|row| {
                let verdict = row
                    .get("verdict")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                verdict == "UNKNOWN"
                    || (verdict == "BLOCKED"
                        && row.get("heartbeat_expired").and_then(Value::as_bool) == Some(true)
                        && (!matches!(
                            row.get("owner_state").and_then(Value::as_str),
                            Some("live" | "dead")
                        ) || !matches!(
                            row.get("liveness_state").and_then(Value::as_str),
                            Some("alive" | "dead")
                        ) || !row.get("cache_error").is_some_and(|value| value.is_null())
                            || row.get("cache_status").and_then(Value::as_str) == Some("partial")
                            || row.get("reasons").and_then(Value::as_array).is_some_and(
                                |reasons| {
                                    reasons.iter().any(|reason| {
                                        reason.as_str().is_some_and(|text| {
                                            let lower = text.to_lowercase();
                                            ["cannot", "failed", "unavailable", "unknown"]
                                                .iter()
                                                .any(|token| lower.contains(token))
                                        })
                                    })
                                },
                            )))
            })
            .map(|row| {
                row.get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    )
    .into_iter()
    .filter(|name| !attention_set.contains(name))
    .collect();
    let (gate_state, summary) = if !attention.is_empty() {
        (
            "actionable",
            format!(
                "{} worktree slot(s) need coordinator attention",
                attention.len()
            ),
        )
    } else if !unknown.is_empty() {
        (
            "unknown",
            format!(
                "could not determine reclaim state for {} worktree slot(s)",
                unknown.len()
            ),
        )
    } else {
        (
            "ok",
            "no worktree slot currently needs coordinator attention".to_string(),
        )
    };
    Ok(json!({
        "schema": 2,
        "worktree_count": worktree_count,
        "running_agent_count": running_agent_count,
        "leak": worktree_count as i64 > running_agent_count,
        "attention_count": attention.len(),
        "attention_slots": attention,
        "unknown_count": unknown.len(),
        "unknown_slots": unknown,
        "state": gate_state,
        "summary": summary,
        "remote_refs_refreshed": false,
        "disk": disk,
        "slots": rows,
    }))
}

fn first_seen_names(names: Vec<String>) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for name in names {
        if seen.insert(name.clone()) {
            ordered.push(name);
        }
    }
    ordered
}

fn disk_status(cfg: &Cfg, config: &Value) -> Value {
    let free = free_bytes(&cfg.worktrees);
    let advisory = config.get("disk_advisory_bytes").and_then(Value::as_i64);
    let provisioning = config
        .get("disk_provisioning_floor_bytes")
        .and_then(Value::as_i64);
    let emergency = config.get("disk_emergency_bytes").and_then(Value::as_i64);
    let state = match (advisory, provisioning, emergency) {
        (Some(advisory), Some(provisioning), Some(emergency)) => {
            if free < emergency {
                "emergency"
            } else if free < provisioning {
                "provisioning-blocked"
            } else if free < advisory {
                "advisory"
            } else {
                "healthy"
            }
        }
        _ => "unconfigured",
    };
    json!({
        "free_bytes": free,
        "state": state,
        "advisory_bytes": advisory,
        "provisioning_floor_bytes": provisioning,
        "emergency_bytes": emergency,
    })
}

fn free_bytes(path: &Path) -> i64 {
    rustix::fs::statvfs(path)
        .map(|stat| (stat.f_bavail * stat.f_frsize) as i64)
        .unwrap_or(0)
}
