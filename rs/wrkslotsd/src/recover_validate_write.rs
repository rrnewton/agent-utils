//! Writer: recover absent validation rows (`recover-absent-validate-rows`).
//!
//! Reproduces the input contract and the outcomes of the Python
//! command: the bounded JSON input file (schema 1, exact row
//! identities, input digest), the local-machine check,
//! interrupted-batch journal handling, already-recovered
//! recognition bound to the exact archive record and its recovery
//! event, row validation (type, generation, record digest), the
//! scoped hold check, and the still-has-physical-storage refusal.
//! The prove-then-recheck path (#2125) folds the history outside
//! the mutation locks and, only when the O(1) tip proof is
//! unchanged under them, performs the journal write and the
//! recovery writes (archive appends, ACTIVE removal, snapshot
//! readback, journal clear) without re-folding. The host-wide
//! process census and the deleted-bind unmount stay with the
//! Python edition: this edition refuses a deleted-bind row and
//! judges liveness from the scoped evidence it can read.
//! Never-write-narrower rule (#2125): wherever the Python
//! edition (037494e9) would run a check this edition cannot run
//! before writing -- the host-wide /proc census, the deleted-
//! bind unmount and bind-record forget, or the cross-machine
//! unrelated-row fold -- this edition refuses before the
//! journal write and writes nothing; see
//! `refuse_uncovered_write` for the exact gated conditions.
//! Fixture registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory, Cfg};
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::finish_write::assert_record_paths;
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::status_state::{host_id, identity_to_value, read_process_identity};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Row {
    machine: String,
    slot: String,
    generation: i64,
    record_sha256: String,
}

struct Planned {
    row: Row,
    record: Value,
    entry: Value,
}

fn short_hostname() -> Result<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|error| format!("cannot read the machine hostname: {error}"))?;
    let name = raw.trim().split('.').next().unwrap_or_default().to_string();
    validate_name(&name, "machine")?;
    Ok(name)
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn exact_keys(value: &Value, keys: &[&str], label: &str) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} must be a JSON object"))?;
    if object.len() != keys.len() || !keys.iter().all(|key| object.contains_key(*key)) {
        return Err(format!("{label} has unknown or missing fields"));
    }
    Ok(())
}

fn read_input(cfg: &Cfg, raw_path: &Path) -> Result<(String, Vec<Row>)> {
    let path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        cfg.root.join(raw_path)
    };
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|error| format!("cannot read absent-validation-row input: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot read absent-validation-row input: {error}"))?;
    if !metadata.is_file() {
        return Err("absent-validation-row input is not a regular file".to_string());
    }
    let mut contents = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut contents)
        .map_err(|error| format!("cannot read absent-validation-row input: {error}"))?;
    if contents.len() > 1024 * 1024 {
        return Err("absent-validation-row input is too large".to_string());
    }
    let input_sha256 = format!("{:x}", Sha256::digest(&contents));
    let raw: Value = serde_json::from_slice(&contents)
        .map_err(|error| format!("absent-validation-row input is not valid JSON: {error}"))?;
    exact_keys(&raw, &["schema", "rows"], "absent-validation-row input")?;
    if raw.get("schema").and_then(Value::as_i64) != Some(1) {
        return Err("absent-validation-row input schema must be 1".to_string());
    }
    let raw_rows = raw
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if raw_rows.is_empty() {
        return Err("absent-validation-row input must name at least one row".to_string());
    }
    if raw_rows.len() > 1024 {
        return Err("absent-validation-row input names more than 1024 rows".to_string());
    }
    let mut rows: Vec<Row> = Vec::new();
    for (index, value) in raw_rows.iter().enumerate() {
        let label = format!("absent-validation-row input.rows[{index}]");
        exact_keys(
            value,
            &["machine", "slot", "generation", "record_sha256"],
            &label,
        )?;
        let machine = value
            .get("machine")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        validate_name(&machine, "machine")?;
        let slot = value
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        validate_name(&slot, "slot")?;
        let generation = value.get("generation").and_then(Value::as_i64).unwrap_or(0);
        if generation < 1 {
            return Err(format!("{label}.generation must be positive"));
        }
        let record_sha256 = value
            .get("record_sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !is_digest(&record_sha256) {
            return Err(format!(
                "{label}.record_sha256 must be one lowercase SHA-256 digest"
            ));
        }
        if rows
            .iter()
            .any(|row: &Row| row.machine == machine && row.slot == slot)
        {
            return Err(format!(
                "absent-validation-row input repeats {machine}/{slot}"
            ));
        }
        rows.push(Row {
            machine,
            slot,
            generation,
            record_sha256,
        });
    }
    Ok((input_sha256, rows))
}

fn recovery_matches(
    cfg: &Cfg,
    selected: &str,
    archived: &Value,
    _archives_revision: i64,
    row: &Row,
) -> bool {
    let Some(archive_id) = archived.get("archive_id").and_then(Value::as_str) else {
        return false;
    };
    // Streamed (#1931): validate the chain one event at a time and
    // retain nothing; the match semantics are unchanged.
    let Ok(matched) = crate::global_state::event_chain_matches(
        &cfg.control.join(format!("EVENTS.{selected}")),
        selected,
        |event| {
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            let record = payload.get("record").cloned().unwrap_or(Value::Null);
            let evidence = payload.get("evidence").cloned().unwrap_or(Value::Null);
            let evidence_keys: Vec<&String> = evidence
                .as_object()
                .map(|object| object.keys().collect())
                .unwrap_or_default();
            event.get("kind").and_then(Value::as_str) == Some("archive-state-recorded")
                && payload.get("action").and_then(Value::as_str)
                    == Some("absent-validation-row-recovered")
                && payload.get("slot").and_then(Value::as_str) == Some(row.slot.as_str())
                && record.get("archive_id").and_then(Value::as_str) == Some(archive_id)
                && record.get("slot").and_then(Value::as_str) == Some(row.slot.as_str())
                && record.get("machine").and_then(Value::as_str) == Some(selected)
                && record.get("generation").and_then(Value::as_i64) == Some(row.generation)
                && record.get("slot_type").and_then(Value::as_str) == Some("validate")
                && record.get("physical_storage").and_then(Value::as_str) == Some("removed")
                && evidence_keys.len() == 3
                && evidence.get("archive_id").and_then(Value::as_str) == Some(archive_id)
                && evidence.get("source_record_sha256").and_then(Value::as_str)
                    == Some(row.record_sha256.as_str())
                && evidence.get("validation_outcome").and_then(Value::as_str) == Some("unknown")
        },
    ) else {
        return false;
    };
    matched
}

/// Whether `path` is covered by a deleted-subvolume bind (#1771):
/// its device is 0, or a mountinfo root ending in `//deleted` mounts
/// at or above it (or it is the slot directory holding such a mount).
/// Such a bind is absent storage, never physical storage, but this
/// edition never unmounts it: the caller refuses and names the Python
/// edition, which unmounts the stale bind before reconciling.
fn deleted_bind_covers(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.dev() == 0 {
            return true;
        }
    }
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    mountinfo_covers_deleted_bind(path, &text)
}

fn mountinfo_covers_deleted_bind(path: &Path, text: &str) -> bool {
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 || !fields[3].ends_with("//deleted") {
            continue;
        }
        let decoded = fields[4].replace("\\040", " ");
        let mount_point = Path::new(&decoded);
        if path == mount_point || path.starts_with(mount_point) || mount_point.starts_with(path) {
            return true;
        }
    }
    false
}

fn refusal_payload(
    root: Option<&Path>,
    machine_override: Option<&str>,
    input: &Path,
    message: &str,
) -> Option<Value> {
    // Python emits the outcome payload (recovered rows marked,
    // the rest refused with the message) before re-raising.
    let (cfg, selected) = load_cfg(root, machine_override).ok()?;
    let (_sha, rows) = read_input(&cfg, input).ok()?;
    let archives = crate::recover_absent_write::archives_document(&cfg, &selected).ok()?;
    let archives_revision = archives
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let archive_records = archives
        .get("records")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    for row in &rows {
        let matching: Vec<&Value> = archive_records
            .iter()
            .filter(|item| item.get("slot").and_then(Value::as_str) == Some(row.slot.as_str()))
            .collect();
        let exact = matching.len() == 1
            && matching[0].get("machine").and_then(Value::as_str) == Some(selected.as_str())
            && matching[0].get("generation").and_then(Value::as_i64) == Some(row.generation)
            && matching[0].get("slot_type").and_then(Value::as_str) == Some("validate")
            && matching[0].get("physical_storage").and_then(Value::as_str) == Some("removed")
            && recovery_matches(&cfg, &selected, matching[0], archives_revision, row);
        out.push(if exact {
            json!({
                "machine": row.machine,
                "slot": row.slot,
                "generation": row.generation,
                "outcome": "already-recovered",
                "detail": "exact archive already exists",
            })
        } else {
            json!({
                "machine": row.machine,
                "slot": row.slot,
                "generation": row.generation,
                "outcome": "refused",
                "detail": message,
            })
        });
    }
    Some(json!({"schema": 1, "rows": out}))
}

/// Bounded re-reads before the locked fallback (#2125), as in
/// the Python edition's `_HISTORY_RECHECK_ATTEMPTS`.
const HISTORY_RECHECK_ATTEMPTS: usize = 3;

/// The cheap proof a prove-then-recheck mutation compares (#2125).
///
/// Owner rule: only constant-time work runs while holding one of
/// the registry locks; anything linear in the history happens ahead
/// of time, outside the locks, and is validated under them by an
/// O(1) generation/tip check, redone if it changed. Later lock work
/// (#2127 per-slot locks) must reuse this pattern, not re-derive it.
///
/// Per machine: the event index's tip (sequence and digest), both
/// snapshot revisions, and the on-disk journal names. Reading it
/// reads, per machine, the index file, the one sequence-addressed
/// last event file, and both snapshot files -- never a listing of
/// the log, never a fold -- so comparing it under the mutation
/// locks is constant time on a long history. `None` when any piece
/// cannot be read; the caller then takes the ordinary locked path,
/// exactly as before #2125. Mirrors the Python edition's
/// `_history_tip_proof`.
/// One machine's (name, tip sequence, tip digest, ACTIVE revision,
/// ARCHIVED revision) plus the on-disk journal names (#2125).
type HistoryProof = (Vec<(String, u64, String, i64, i64)>, Vec<String>);

fn history_proof(root: Option<&Path>, cfg: &Cfg) -> Option<HistoryProof> {
    let machines = crate::global_state::discover_machines(&cfg.control).ok()?;
    let mut proof = Vec::new();
    for machine in &machines {
        let directory = cfg.control.join(format!("EVENTS.{machine}"));
        let index_text =
            std::fs::read_to_string(cfg.control.join(format!("EVENT-INDEX.{machine}.json")))
                .ok()?;
        let index: Value = serde_json::from_str(&index_text).ok()?;
        if index.get("schema").and_then(Value::as_i64) != Some(1)
            || index.get("machine").and_then(Value::as_str) != Some(machine.as_str())
        {
            return None;
        }
        let tip = index.get("tip")?;
        let sequence = tip.get("sequence").and_then(Value::as_i64)?;
        let digest = tip.get("sha256").and_then(Value::as_str)?;
        if sequence < 1 {
            return None;
        }
        let last_text =
            std::fs::read_to_string(directory.join(format!("{sequence:020}.json"))).ok()?;
        let last: Value = serde_json::from_str(&last_text).ok()?;
        if last.get("sha256").and_then(Value::as_str) != Some(digest) {
            return None;
        }
        let (count, tip) = (sequence as u64, digest.to_string());
        let (active, _order) =
            crate::active_state::parse_state_ordered(root, Some(machine)).ok()?;
        let active_revision = active.get("revision").and_then(Value::as_i64)?;
        if index.get("active_revision").and_then(Value::as_i64) != Some(active_revision) {
            return None;
        }
        let archive = crate::archive_state::parse_archive(root, Some(machine)).ok()?;
        let archive_revision = archive.get("revision").and_then(Value::as_i64).unwrap_or(0);
        proof.push((
            machine.clone(),
            count,
            tip,
            active_revision,
            archive_revision,
        ));
    }
    let mut journals: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&cfg.control).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".journal") || name.contains(".journal.tmp.") {
            journals.push(name);
        }
    }
    journals.sort();
    Some((proof, journals))
}

/// Save-then-compare binding (#2125): whether the current tip
/// proof still equals the proof saved before the locks. O(1) per
/// machine -- index file, one sequence-addressed last event,
/// both snapshot revisions, journal names; never a listing of an
/// event directory, never a fold. Mirrors comparing two
/// `_history_tip_proof` values in the Python edition.
pub(crate) fn history_proof_matches(root: Option<&Path>, cfg: &Cfg, saved: &HistoryProof) -> bool {
    history_proof(root, cfg).as_ref() == Some(saved)
}

/// Read one sequence-addressed event and verify it against its
/// chain predecessor, as the Python `_event_from_path` does for
/// the scoped hold check. `Err` means unprovable (missing file,
/// broken chain, bad digest); the caller falls back.
fn read_scoped_event(cfg: &Cfg, machine: &str, sequence: i64) -> Result<Value> {
    let directory = cfg.control.join(format!("EVENTS.{machine}"));
    let mut previous = "0".repeat(64);
    if sequence > 1 {
        let prior_text =
            std::fs::read_to_string(directory.join(format!("{:020}.json", sequence - 1)))
                .map_err(|error| format!("cannot read append-only event: {error}"))?;
        let prior: Value = serde_json::from_str(&prior_text)
            .map_err(|error| format!("append-only event is malformed JSON: {error}"))?;
        previous = prior
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or("append-only event.sha256 missing")?
            .to_string();
    }
    let text = std::fs::read_to_string(directory.join(format!("{sequence:020}.json")))
        .map_err(|error| format!("cannot read append-only event: {error}"))?;
    let event: Value = serde_json::from_str(&text)
        .map_err(|error| format!("append-only event is malformed JSON: {error}"))?;
    if event.get("machine").and_then(Value::as_str) != Some(machine) {
        return Err("append-only event belongs to another machine".to_string());
    }
    if event.get("sequence").and_then(Value::as_i64) != Some(sequence) {
        return Err("append-only event sequence does not match its filename".to_string());
    }
    if event.get("previous_sha256").and_then(Value::as_str) != Some(previous.as_str()) {
        return Err("append-only event hash chain is broken".to_string());
    }
    let recorded = event
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "append-only event.sha256 missing".to_string())?
        .to_string();
    let mut core = event
        .as_object()
        .cloned()
        .ok_or_else(|| "append-only event must be a JSON object".to_string())?;
    core.remove("sha256");
    let digest = canonical_sha256(&Value::Object(core)).map_err(|error| error.to_string())?;
    if recorded != digest {
        return Err("append-only event digest does not match its content".to_string());
    }
    Ok(event)
}

/// One slot's hold from the index's sequence-addressed events
/// (#2039/#2125), mirroring `_scoped_slot_hold`: the latest of its
/// imported, held and hold-released events decides, and only those
/// events (plus chain predecessors) are read. Both holder
/// spellings are honoured: an imported hold record and a
/// `slot-held` event. `Err` means unprovable.
fn scoped_slot_hold(cfg: &Cfg, machine: &str, slot: &str, entry: &Value) -> Result<Option<Value>> {
    let mut hold: Option<Value> = None;
    let mut best: i64 = -1;
    if let Some(imported) = entry.get("imported").and_then(Value::as_i64) {
        let event = read_scoped_event(cfg, machine, imported)?;
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if let Some(holds) = payload.get("holds").and_then(Value::as_array) {
            for item in holds {
                if item.get("slot").and_then(Value::as_str) == Some(slot) {
                    hold = Some(item.clone());
                    best = imported;
                }
            }
        }
    }
    if let Some(held) = entry.get("held").and_then(Value::as_i64) {
        if held > best {
            let event = read_scoped_event(cfg, machine, held)?;
            let payload = event.get("payload").cloned().unwrap_or(Value::Null);
            hold = Some(json!({
                "schema": 1,
                "machine": machine,
                "slot": slot,
                "held_at": event.get("recorded_at").cloned().unwrap_or(Value::Null),
                "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
            }));
            best = held;
        }
    }
    if let Some(released) = entry.get("hold_released").and_then(Value::as_i64) {
        if released > best {
            hold = None;
        }
    }
    Ok(hold)
}

/// The scoped global-hold check (#2125): the target slot ids come
/// from the registry snapshot's rows, and each hold is judged from
/// its few sequence-addressed events instead of a fold of the
/// whole log. Anything unprovable (no usable index, a slot absent
/// from the index, an unreadable addressed event) takes the
/// streamed fold below, which is also sequence-verified but
/// retains only the named slots -- never a full state replay.
/// Mirrors `_assert_absent_validate_rows_not_held`.
fn scoped_hold_exists(
    _root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    slots: &[String],
) -> Result<Option<(String, String)>> {
    // Try the O(1) scoped path first.
    let index_path = cfg.control.join(format!("EVENT-INDEX.{selected}.json"));
    let scoped = (|| -> Option<Vec<(String, Option<Value>)>> {
        let text = std::fs::read_to_string(&index_path).ok()?;
        let index: Value = serde_json::from_str(&text).ok()?;
        if index.get("schema").and_then(Value::as_i64) != Some(1)
            || index.get("machine").and_then(Value::as_str) != Some(selected)
        {
            return None;
        }
        let slots_index = index.get("slots")?.as_object()?;
        let mut out = Vec::new();
        for slot in slots {
            let entry = slots_index.get(slot)?;
            let hold = scoped_slot_hold(cfg, selected, slot, entry).ok()?;
            out.push((slot.clone(), hold));
        }
        Some(out)
    })();
    if let Some(holds) = scoped {
        for (slot, hold) in holds {
            if let Some(hold) = hold {
                let reason = hold
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                return Ok(Some((slot, reason)));
            }
        }
        return Ok(None);
    }
    // Empty-log snapshot branch (as in the Python streamed path):
    // with no event files the HOLD snapshot file decides.
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let has_events = std::fs::read_dir(&directory)
        .map(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.len() == 25 && name.ends_with(".json")
            })
        })
        .unwrap_or(false);
    if !has_events {
        for slot in slots {
            let hold_path = cfg.control.join(format!(
                "HOLD.{}.{}.{}.json",
                selected.chars().count(),
                selected,
                slot
            ));
            if hold_path.exists() || hold_path.is_symlink() {
                let text = std::fs::read_to_string(&hold_path)
                    .map_err(|error| format!("cannot read slot hold: {error}"))?;
                let hold: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("slot hold is malformed JSON: {error}"))?;
                let reason = hold
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                return Ok(Some((slot.clone(), reason)));
            }
        }
        return Ok(None);
    }
    // Streamed fallback: fold only the hold-relevant kinds.
    let events = crate::global_state::load_event_chain_filtered(
        &directory,
        selected,
        &["state-imported", "slot-held", "slot-hold-released"],
    )?;
    let wanted: std::collections::BTreeSet<String> = slots.iter().cloned().collect();
    let holds = crate::hold_write::holds_from_events(&events, selected, &wanted);
    for slot in slots {
        if let Some(hold) = holds.get(slot) {
            if !hold.is_null() {
                let reason = hold
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                return Ok(Some((slot.clone(), reason)));
            }
        }
    }
    Ok(None)
}

fn assert_rows_not_held(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    slots: &[String],
) -> Result<()> {
    if let Some((slot, reason)) = scoped_hold_exists(root, cfg, selected, slots)? {
        return Err(format!(
            "slot {slot} is held: {reason}; run 'wrkslots unhold {slot}' only when the hold is no longer needed"
        ));
    }
    Ok(())
}

fn journal_path(cfg: &Cfg, selected: &str) -> PathBuf {
    cfg.control.join(format!("ACTIVE.{selected}.journal"))
}

/// The absent-validate archive entry, mirroring
/// `_absent_validate_archive_entry`.
fn absent_validate_archive_entry(record: &Value, row: &Row, finished_at: &str) -> Value {
    let get = |key: &str| record.get(key).cloned().unwrap_or(Value::Null);
    let mut entry = json!({
        "archive_id": format!(
            "{}:{}:{}:{}",
            record.get("machine").and_then(Value::as_str).unwrap_or_default(),
            row.slot,
            row.generation,
            finished_at
        ),
        "slot": row.slot,
        "agent": get("agent"),
        "task": get("task"),
        "purpose": get("purpose"),
        "slot_type": get("slot_type"),
        "machine": get("machine"),
        "generation": row.generation,
        "created_at": get("created_at"),
        "finished_at": finished_at,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": [
            "registry reconciliation confirmed physical storage absent",
            format!("source ACTIVE record SHA-256: {}", row.record_sha256),
        ],
        "limitations": ["validation outcome unknown"],
        "continuation": "no storage remains; consult retained validation records by run identity",
        "salvage": [],
        "checkouts": record.get("checkouts").cloned().unwrap_or(json!([])),
    });
    if let Some(layout) = record.get("layout") {
        if !layout.is_null() {
            entry["layout"] = layout.clone();
        }
    }
    if let Some(source) = record.get("import_source") {
        if !source.is_null() {
            entry["import_source"] = source.clone();
        }
    }
    entry
}

fn recovery_evidence(row: &Row, entry: &Value) -> Value {
    json!({
        "archive_id": entry.get("archive_id").cloned().unwrap_or(Value::Null),
        "source_record_sha256": row.record_sha256,
        "validation_outcome": "unknown",
    })
}

/// `_write_journal` for the absent-validate batch: an
/// `operation-progress-recorded` event plus the atomic file.
fn write_batch_journal(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    journal: &Value,
) -> Result<()> {
    ensure_event_log(root, cfg, selected)?;
    append_event(
        cfg,
        selected,
        "operation-progress-recorded",
        json!({
            "slot": journal.get("slot").cloned().unwrap_or(Value::Null),
            "operation": journal.get("kind").cloned().unwrap_or(Value::Null),
            "journal_path": format!("ACTIVE.{selected}.journal"),
            "journal": journal,
        }),
    )?;
    atomic_write_json(&journal_path(cfg, selected), journal)
}

/// `_clear_journal`: an `operation-completed` event, then unlink.
fn clear_batch_journal(
    _root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    journal: &Value,
) -> Result<()> {
    append_event(
        cfg,
        selected,
        "operation-completed",
        json!({
            "slot": journal.get("slot").cloned().unwrap_or(Value::Null),
            "operation": journal.get("kind").cloned().unwrap_or(Value::Null),
            "journal_path": format!("ACTIVE.{selected}.journal"),
        }),
    )?;
    let path = journal_path(cfg, selected);
    if path.exists() || path.is_symlink() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("cannot remove control file {}: {error}", path.display()))?;
        if let Some(parent) = path.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
    }
    Ok(())
}

/// Whether no interrupted-mutation journal is outstanding, judged
/// without the locks; the locked path re-judges journals itself.
fn journals_absent(root: Option<&Path>) -> bool {
    match crate::journals_state::parse_journals(root) {
        Ok(enumerated) => enumerated
            .get("journals")
            .and_then(Value::as_array)
            .is_some_and(|names| names.is_empty()),
        Err(_) => false,
    }
}

/// Fail-closed gate for conditions this edition cannot prove
/// as safely as the Python edition (037494e9) before writing
/// (#2125, never write with narrower safety than Python). The
/// Python path (`_plan_absent_validate_rows` ->
/// `_assert_absent_validate_rows_safe`, `_recover_absent_validate_rows`)
/// runs, before its journal/archive/ACTIVE writes: the host-wide
/// /proc process census (`_assert_absent_validate_processes_unrelated`,
/// retained-handle and run-evidence judgements), the deleted-bind
/// unmount (`_retire_deleted_subvolume_binds`) and bind-record
/// forget (`_forget_absent_validate_binds`), and the cross-machine
/// unrelated-row fold (`_global_rows` over every machine, judged
/// after the writes from all machines' states). This edition has
/// no host-wide census, never unmounts or forgets binds, and its
/// readback covers only the selected machine, so it refuses --
/// naming the Python edition -- when any of these holds:
///
/// * a planned slot has a worktrees-image bind record (Python
///   would unmount a stale deleted bind and forget the record);
/// * any row (ACTIVE slot or ARCHIVED record) is present on a
///   non-selected machine (Python folds every machine and proves
///   no unrelated row changed; this edition cannot);
/// * a planned row needs the census: it has no recorded owner
///   generation, an incomplete owner generation (missing pid,
///   start ticks, boot or host identity), or a retained
///   validation handle naming it under `ignored/validate/runs`
///   (Python judges those handles and the host process table).
///
/// Covered rows -- a complete owner generation this edition can
/// prove dead from scoped /proc identity, no bind record, no
/// retained handle, and no other machine's rows -- keep the
/// proven writing path unchanged. Called from `classify_rows`
/// and again at the top of `execute_recovery`, before the
/// journal write, so a refusal writes nothing: no journal, no
/// event appends, no snapshot writes.
fn refuse_uncovered_write(cfg: &Cfg, selected: &str, planned: &[Planned]) -> Result<()> {
    // Cross-machine unrelated-row fold. Enumerate shards
    // directly (not `discover_machines`, whose shard-consistency
    // error would mask which machine's rows block the write).
    let mut machines: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&cfg.control).map_err(|error| {
        format!(
            "cannot enumerate machines for the cross-machine check; reconcile it with the Python edition, which folds every machine before writing: {error}"
        )
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        for prefix in ["ACTIVE.", "ARCHIVED."] {
            if let Some(machine) = name
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(".json"))
            {
                if !machine.is_empty() && !machines.contains(&machine.to_string()) {
                    machines.push(machine.to_string());
                }
            }
        }
    }
    machines.sort();
    for machine in &machines {
        if machine == selected {
            continue;
        }
        for (label, file, key) in [
            ("ACTIVE", format!("ACTIVE.{machine}.json"), "slots"),
            ("ARCHIVED", format!("ARCHIVED.{machine}.json"), "records"),
        ] {
            let path = cfg.control.join(&file);
            if !path.exists() && !path.is_symlink() {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|error| {
                format!(
                    "cannot read {label} rows on non-selected machine {machine} for the cross-machine check; reconcile it with the Python edition, which folds every machine before writing: {error}"
                )
            })?;
            let doc: Value = serde_json::from_str(&text).map_err(|error| {
                format!(
                    "cannot read {label} rows on non-selected machine {machine} for the cross-machine check; reconcile it with the Python edition, which folds every machine before writing: {error}"
                )
            })?;
            let count = doc.get(key).and_then(Value::as_array).map_or(0, Vec::len);
            if count > 0 {
                return Err(format!(
                    "rows on non-selected machine {machine} are present in state; reconcile it with the Python edition, which folds every machine and proves no unrelated row changed before writing"
                ));
            }
        }
    }
    // Deleted-bind unmount / bind-record forget.
    let binds = crate::worktrees_image::load_bind_records(&cfg.control).map_err(|error| {
        format!(
            "cannot read worktrees-image bind records; reconcile it with the Python edition, which unmounts and forgets binds before writing: {error}"
        )
    })?;
    for item in planned {
        if binds.contains_key(&item.row.slot) {
            return Err(format!(
                "validation row {} has a worktrees-image bind record; reconcile it with the Python edition, which unmounts the stale bind and forgets the bind record before writing",
                item.row.slot
            ));
        }
    }
    // Host-wide /proc census.
    for item in planned {
        let owner = item.record.get("owner");
        let complete = owner.is_some_and(|identity| {
            identity
                .get("pid")
                .and_then(Value::as_i64)
                .is_some_and(|pid| pid >= 1)
                && identity
                    .get("start_ticks")
                    .and_then(Value::as_i64)
                    .is_some()
                && identity
                    .get("boot_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
                && identity
                    .get("host_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
        });
        if !complete {
            return Err(format!(
                "validation row {} has no complete recorded owner generation; proving it dead needs the host-wide process census; reconcile it with the Python edition, which runs that census before writing",
                item.row.slot
            ));
        }
        let handles = cfg.root.join("ignored").join("validate").join("runs");
        if handles.is_dir() {
            let entries = std::fs::read_dir(&handles).map_err(|error| {
                format!(
                    "cannot read retained validation handles for validation row {}; proving it dead needs the host-wide process census; reconcile it with the Python edition, which runs that census before writing: {error}",
                    item.row.slot
                )
            })?;
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if text.contains(&item.row.slot) {
                        return Err(format!(
                            "validation row {} is named by a retained validation handle; proving it dead needs the host-wide process census; reconcile it with the Python edition, which runs that census before writing",
                            item.row.slot
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Classify every input row against the folded history, shared
/// by the prove-then-recheck rounds (no locks held) and the
/// ordinary locked path (#2125). Returns the rows that still need
/// recovery, the already-recovered outcomes, and the folded
/// documents the recovery writes advance. No census runs here;
/// the apply path judges holds and storage before writing.
fn classify_rows(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    rows: &[Row],
) -> Result<(Vec<Planned>, Vec<Value>, Value, Value)> {
    validate_global(root, cfg)?;
    let active = active_document(root, cfg, selected)?;
    let archives = crate::recover_absent_write::archives_document(cfg, selected)?;
    let archives_revision = archives
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let archive_records = archives
        .get("records")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut planned: Vec<Planned> = Vec::new();
    let mut already: Vec<Value> = Vec::new();
    for row in rows {
        if row.machine != selected {
            return Err(format!(
                "every input row must name selected machine {selected}; use --machine for another shard"
            ));
        }
        let record = active
            .get("slots")
            .and_then(Value::as_array)
            .and_then(|slots| {
                slots.iter().find(|item| {
                    item.get("slot").and_then(Value::as_str) == Some(row.slot.as_str())
                })
            })
            .cloned();
        if let Some(record) = record {
            let slot_type = record
                .get("slot_type")
                .and_then(Value::as_str)
                .unwrap_or("agent");
            if slot_type != "validate" {
                return Err(format!("slot {} is not a validation row", row.slot));
            }
            let generation = record
                .get("generation")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if generation != row.generation {
                return Err(format!(
                    "validation row {}/{} generation changed",
                    row.machine, row.slot
                ));
            }
            let observed = canonical_sha256(&record).map_err(|error| error.to_string())?;
            if observed != row.record_sha256 {
                return Err(format!(
                    "validation row {}/{} changed: expected {}, found {observed}",
                    row.machine, row.slot, row.record_sha256
                ));
            }
            // Both active and archived is never a plan.
            if archive_records
                .iter()
                .any(|item| item.get("slot").and_then(Value::as_str) == Some(row.slot.as_str()))
            {
                return Err(format!(
                    "validation row {} is both active and archived",
                    row.slot
                ));
            }
            assert_record_paths(cfg, &record)?;
            let slot_path = slot_directory(cfg, &row.slot, slot_type)?;
            let mut paths: Vec<PathBuf> = vec![slot_path];
            for checkout in record
                .get("checkouts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let raw_path = checkout
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                paths.push(cfg.root.join(raw_path));
            }
            paths.dedup();
            if paths.iter().any(|path| deleted_bind_covers(path)) {
                return Err(format!(
                    "validation row {} sits on a deleted-subvolume bind; reconcile it with the Python edition, which unmounts the stale bind first",
                    row.slot
                ));
            }
            for path in &paths {
                if path.exists() || path.is_symlink() {
                    return Err(format!(
                        "validation row {} still has physical storage at {}",
                        row.slot,
                        path.display()
                    ));
                }
            }
            let entry = absent_validate_archive_entry(&record, row, &utc_now());
            planned.push(Planned {
                row: row.clone(),
                record,
                entry,
            });
            continue;
        }
        let matching: Vec<&Value> = archive_records
            .iter()
            .filter(|item| item.get("slot").and_then(Value::as_str) == Some(row.slot.as_str()))
            .collect();
        let exact = matching.len() == 1
            && matching[0].get("machine").and_then(Value::as_str) == Some(selected)
            && matching[0].get("generation").and_then(Value::as_i64) == Some(row.generation)
            && matching[0].get("slot_type").and_then(Value::as_str) == Some("validate")
            && matching[0].get("physical_storage").and_then(Value::as_str) == Some("removed")
            && recovery_matches(cfg, selected, matching[0], archives_revision, row);
        if !exact {
            return Err(format!(
                "validation row {}/{} is neither active nor an exact prior absent-row recovery",
                row.machine, row.slot
            ));
        }
        already.push(json!({
            "machine": row.machine,
            "slot": row.slot,
            "generation": row.generation,
            "outcome": "already-recovered",
            "detail": "exact archive already exists",
        }));
    }
    // Scoped hold check over exactly the planned slots (#2125).
    if !planned.is_empty() {
        let slots: Vec<String> = planned.iter().map(|item| item.row.slot.clone()).collect();
        assert_rows_not_held(root, cfg, selected, &slots)?;
        refuse_uncovered_write(cfg, selected, &planned)?;
    }
    Ok((planned, already, active, archives))
}

fn plan_outcomes(
    rows: &[Row],
    planned: &[Planned],
    already: &[Value],
    input_sha256: &str,
) -> Value {
    let planned_keys: Vec<(String, i64)> = planned
        .iter()
        .map(|item| (item.row.slot.clone(), item.row.generation))
        .collect();
    let mut out: Vec<Value> = Vec::new();
    for row in rows {
        if planned_keys.contains(&(row.slot.clone(), row.generation)) {
            out.push(json!({
                "machine": row.machine, "slot": row.slot, "generation": row.generation,
                "outcome": "planned",
                "detail": format!("read-only plan input_sha256={input_sha256}"),
            }));
        } else if let Some(found) = already
            .iter()
            .find(|item| item.get("slot").and_then(Value::as_str) == Some(row.slot.as_str()))
        {
            out.push(found.clone());
        }
    }
    json!({"schema": 1, "rows": out})
}

fn recovered_outcomes(rows: &[Row], planned: &[Planned], already: &[Value]) -> Vec<Value> {
    let planned_keys: Vec<(String, i64)> = planned
        .iter()
        .map(|item| (item.row.slot.clone(), item.row.generation))
        .collect();
    let mut out: Vec<Value> = Vec::new();
    for row in rows {
        if planned_keys.contains(&(row.slot.clone(), row.generation)) {
            out.push(json!({
                "machine": row.machine, "slot": row.slot, "generation": row.generation,
                "outcome": "recovered", "detail": "archived with validation outcome unknown",
            }));
        } else if let Some(found) = already
            .iter()
            .find(|item| item.get("slot").and_then(Value::as_str) == Some(row.slot.as_str()))
        {
            out.push(json!({
                "machine": found.get("machine").cloned().unwrap_or(Value::Null),
                "slot": found.get("slot").cloned().unwrap_or(Value::Null),
                "generation": found.get("generation").cloned().unwrap_or(Value::Null),
                "outcome": "already-recovered", "detail": "exact archive already existed",
            }));
        }
    }
    out
}

fn with_lock_seconds(mut payload: Value, started: std::time::Instant) -> Value {
    let seconds = started.elapsed().as_secs_f64();
    let rounded = (seconds * 1000.0).round() / 1000.0;
    if let Some(object) = payload.as_object_mut() {
        object.insert("lock_seconds".to_string(), json!(rounded));
    }
    payload
}

/// Perform the recovery writes for `planned` under the mutation
/// locks (#2125): journal, archive appends, ACTIVE removal, both
/// snapshot readbacks, journal clear. The caller has already
/// proven the tip unchanged; every append here is the only change
/// since, so the readback compares snapshots, never a re-fold.
/// On failure the journal is released only when the tip proof
/// captured just after the journal write still matches (#1554,
/// judged by the proof, never a replay under the locks).
#[allow(clippy::too_many_arguments)]
fn execute_recovery(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
    rows: &[Row],
    planned: &[Planned],
    already: &[Value],
    active: &Value,
    archives: &Value,
    input_sha256: &str,
    coordinator: &Value,
) -> Result<Vec<Value>> {
    // Never-write-narrower gate (#2125): before the journal
    // write, refuse every condition the Python edition proves
    // with its census, bind unmount/forget, or cross-machine
    // fold and this edition cannot. A refusal here writes
    // nothing -- no journal, no event or snapshot appends.
    refuse_uncovered_write(cfg, selected, planned)?;
    // Owner-liveness gate: a recorded owner that is exactly the
    // live process refuses; anything unreadable is dead evidence.
    for item in planned {
        for key in ["owner", "coordinator_lease"] {
            if let Some(identity) = item.record.get(key) {
                if let Some(pid) = identity.get("pid").and_then(Value::as_i64) {
                    if let Ok(live) = read_process_identity(pid) {
                        if crate::status_state::identity_matches_value(&live, identity) {
                            return Err(format!(
                                "validation row {} owner is still alive",
                                item.row.slot
                            ));
                        }
                    }
                }
            }
        }
    }
    let mut journal = json!({
        "schema": 2,
        "kind": "recover-absent-validate-rows",
        "machine": selected,
        "host_id": host_id()?,
        "slot": "absent-validate-rows",
        "input_sha256": input_sha256,
        "cursor": 0,
        "actor": coordinator,
        "items": planned.iter().map(|item| json!({
            "input": {
                "machine": item.row.machine, "slot": item.row.slot,
                "generation": item.row.generation,
                "record_sha256": item.row.record_sha256,
            },
            "record": item.record,
            "archive_entry": item.entry,
        })).collect::<Vec<_>>(),
    });
    write_batch_journal(root, cfg, selected, &journal)?;
    let journal_proof = history_proof(root, cfg);
    let result = (|| -> Result<()> {
        let mut archive_revision = archives
            .get("revision")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let mut archive_records = archives
            .get("records")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Bounded commits (#1701): chunks of 64, cursor per chunk.
        let mut index = 0usize;
        while index < planned.len() {
            let end = (index + 64).min(planned.len());
            for item in &planned[index..end] {
                let previous = archive_revision;
                archive_revision += 1;
                append_event(
                    cfg,
                    selected,
                    "archive-state-recorded",
                    json!({
                        "action": "absent-validation-row-recovered",
                        "slot": item.row.slot,
                        "previous_revision": previous,
                        "revision": archive_revision,
                        "record": item.entry,
                        "evidence": recovery_evidence(&item.row, &item.entry),
                    }),
                )?;
                archive_records.push(item.entry.clone());
            }
            index = end;
            journal["cursor"] = json!(index);
            write_batch_journal(root, cfg, selected, &journal)?;
        }
        let archive_doc = json!({
            "schema": 2, "machine": selected,
            "revision": archive_revision, "records": archive_records,
        });
        atomic_write_json(
            &cfg.control.join(format!("ARCHIVED.{selected}.json")),
            &archive_doc,
        )?;
        // ACTIVE removal, one event per row.
        let mut active_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
        let removed: Vec<String> = planned.iter().map(|item| item.row.slot.clone()).collect();
        for item in planned {
            let previous = active_revision;
            active_revision += 1;
            append_event(
                cfg,
                selected,
                "active-state-recorded",
                json!({
                    "action": "absent-validation-row-recovered",
                    "slot": item.row.slot,
                    "previous_revision": previous,
                    "revision": active_revision,
                    "previous_record_sha256": canonical_sha256(&item.record).map_err(|error| error.to_string())?,
                    "record": Value::Null,
                    "evidence": recovery_evidence(&item.row, &item.entry),
                }),
            )?;
        }
        let active_slots: Vec<Value> = active
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|slot| {
                !removed.contains(
                    &slot
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let active_doc = json!({
            "schema": 2, "machine": selected,
            "revision": active_revision, "slots": active_slots,
        });
        atomic_write_json(
            &cfg.control.join(format!("ARCHIVED.{selected}.json")),
            &archive_doc,
        )?;
        atomic_write_json(
            &cfg.control.join(format!("ACTIVE.{selected}.json")),
            &active_doc,
        )?;
        // Prove the writes landed by reading both snapshots back.
        let (readback_active, _order) =
            crate::active_state::parse_state_ordered(root, Some(selected))?;
        if readback_active != active_doc {
            return Err(
                "absent-validation-row recovery ACTIVE readback differs from its write".to_string(),
            );
        }
        let readback_archive = crate::archive_state::parse_archive(root, Some(selected))?;
        let mut expected_archive = archive_doc.clone();
        if let Some(object) = expected_archive.as_object_mut() {
            object.insert(
                "quarantined_records".to_string(),
                readback_archive
                    .get("quarantined_records")
                    .cloned()
                    .unwrap_or(json!([])),
            );
        }
        if readback_archive != expected_archive {
            return Err(
                "absent-validation-row recovery ARCHIVED readback differs from its write"
                    .to_string(),
            );
        }
        Ok(())
    })();
    if let Err(message) = result {
        // Release only when no event was appended after the journal.
        let unchanged = match &journal_proof {
            Some(saved) => history_proof_matches(root, cfg, saved),
            None => false,
        };
        if unchanged {
            let _ = clear_batch_journal(root, cfg, selected, &journal);
        }
        return Err(message);
    }
    clear_batch_journal(root, cfg, selected, &journal)?;
    Ok(recovered_outcomes(rows, planned, already))
}

/// Recover absent validation rows named by the input file. On
/// refusal, also returns the outcome payload Python prints first.
pub(crate) fn recover_absent_validate_rows_wrapped(
    root: Option<&Path>,
    machine_override: Option<&str>,
    input: &Path,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> (Option<Value>, Result<Value>) {
    match recover_absent_validate_rows(
        root,
        machine_override,
        input,
        apply,
        coordinator_authorized,
        coordinator_pid,
    ) {
        Ok(value) => (None, Ok(value)),
        Err(message) => (
            refusal_payload(root, machine_override, input, &message),
            Err(message),
        ),
    }
}

/// Recover absent validation rows named by the input file.
pub(crate) fn recover_absent_validate_rows(
    root: Option<&Path>,
    machine_override: Option<&str>,
    input: &Path,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let (input_sha256, rows) = read_input(&cfg, input)?;
    let local = short_hostname()?;
    if selected != local {
        return Err(format!(
            "absent validation rows belong to {selected}, but liveness evidence is local to {local}; run this command on {selected}"
        ));
    }
    let coordinator = if apply {
        if !coordinator_authorized {
            return Err(
                "absent validation row recovery requires --coordinator-authorized with --apply"
                    .to_string(),
            );
        }
        let pid = coordinator_pid.ok_or("--apply requires --coordinator-pid")?;
        Some(identity_to_value(&read_process_identity(pid)?))
    } else {
        None
    };
    // Prove-then-recheck (#2125): fold the history and judge every
    // row WITHOUT the mutation locks, then take them and write only
    // when the cheap proof shows the history has not moved since
    // the fold. Owner rule: only constant-time work (the proof
    // comparison) runs under the locks before the writes; the
    // linear fold stays outside and is validated under them by
    // the O(1) tip check, redone if it changed. A moved history is
    // re-read (bounded rounds); an unprovable one, an outstanding
    // journal (the locked path below judges journals, as before),
    // or exhausted rounds take the ordinary locked path unchanged.
    for _round in 0..HISTORY_RECHECK_ATTEMPTS {
        let Some(proof) = history_proof(root, &cfg) else {
            break;
        };
        if !journals_absent(root) {
            break;
        }
        let classified = classify_rows(root, &cfg, &selected, &rows);
        if let Err(message) = &classified {
            if history_proof_matches(root, &cfg, &proof) {
                return Err(message.clone());
            }
            continue;
        }
        let (planned, already, active, archives) = classified.unwrap();
        if !apply {
            let lock_started = std::time::Instant::now();
            let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
            if !history_proof_matches(root, &cfg, &proof) {
                continue;
            }
            refuse_partial_state(&cfg.control)?;
            let _ = lock_started;
            return Ok(plan_outcomes(&rows, &planned, &already, &input_sha256));
        }
        let lock_started = std::time::Instant::now();
        let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
        if !history_proof_matches(root, &cfg, &proof) {
            continue;
        }
        refuse_partial_state(&cfg.control)?;
        if planned.is_empty() {
            let payload =
                json!({"schema": 1, "rows": recovered_outcomes(&rows, &planned, &already)});
            return Ok(with_lock_seconds(payload, lock_started));
        }
        let coordinator = coordinator.clone().unwrap_or(Value::Null);
        let outcomes = execute_recovery(
            root,
            &cfg,
            &selected,
            &rows,
            &planned,
            &already,
            &active,
            &archives,
            &input_sha256,
            &coordinator,
        )?;
        let payload = json!({"schema": 1, "rows": outcomes});
        return Ok(with_lock_seconds(payload, lock_started));
    }
    let lock_started = std::time::Instant::now();
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    // Interrupted-batch journal outcomes.
    {
        let enumerated = crate::journals_state::parse_journals(root)?;
        let names = enumerated
            .get("journals")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if names.len() > 1 {
            return Err("multiple interrupted mutations require inspection".to_string());
        }
        if let Some(name) = names.first().and_then(Value::as_str) {
            let path: PathBuf = cfg.control.join(name);
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // An event-pending journal with no file on disk:
                    // Python falls back to the pending mapping; the
                    // kind/input judgement cannot be made here.
                    return Err(format!(
                        "another interrupted mutation is recorded in {}; run wrkslots recover",
                        path.display()
                    ));
                }
                Err(error) => {
                    return Err(format!("cannot read recovery journal: {error}"));
                }
            };
            let raw: Value = serde_json::from_str(&text)
                .map_err(|error| format!("cannot read recovery journal: {error}"))?;
            if raw.get("kind").and_then(Value::as_str) != Some("recover-absent-validate-rows") {
                return Err(format!(
                    "another interrupted mutation is recorded in {}; run wrkslots recover",
                    path.display()
                ));
            }
            if raw.get("input_sha256").and_then(Value::as_str) != Some(input_sha256.as_str()) {
                return Err(
                    "absent-validation-row input differs from the interrupted batch".to_string(),
                );
            }
            return Err(
                "resuming an interrupted absent-validation-row batch stays with the Python edition for now"
                    .to_string(),
            );
        }
    }
    // Ordinary locked path: classify under the locks, then the
    // same writes. `lock_seconds` is observed from the final
    // snapshot save, never self-asserted.
    let (planned, already, active, archives) = classify_rows(root, &cfg, &selected, &rows)?;
    if !apply {
        return Ok(plan_outcomes(&rows, &planned, &already, &input_sha256));
    }
    if planned.is_empty() {
        let payload = json!({"schema": 1, "rows": recovered_outcomes(&rows, &planned, &already)});
        return Ok(with_lock_seconds(payload, lock_started));
    }
    let coordinator = coordinator.clone().unwrap_or(Value::Null);
    let outcomes = execute_recovery(
        root,
        &cfg,
        &selected,
        &rows,
        &planned,
        &already,
        &active,
        &archives,
        &input_sha256,
        &coordinator,
    )?;
    let payload = json!({"schema": 1, "rows": outcomes});
    Ok(with_lock_seconds(payload, lock_started))
}

#[cfg(test)]
mod tests {
    use super::{
        history_proof_matches, mountinfo_covers_deleted_bind, refuse_uncovered_write,
        scoped_hold_exists, with_lock_seconds, HistoryProof, Planned, Row,
    };
    use crate::active_state::Cfg;
    use crate::canonical::canonical_sha256;
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};

    fn test_cfg(control: PathBuf) -> Cfg {
        let root = control.parent().unwrap_or(&control).to_path_buf();
        Cfg {
            root: root.clone(),
            worktrees: control.clone(),
            control,
            layout: "nested".to_string(),
            default_remote: "origin".to_string(),
            default_landed_ref: "refs/remotes/origin/main".to_string(),
            machine: "node-a".to_string(),
            harness_repo: None,
            eden_mount_root: None,
            eden_backing_root: None,
            eden_backing_paths: Vec::new(),
            validation_exclusion_roots: Vec::new(),
            remote_ssh_commands: Vec::new(),
        }
    }

    fn write_event(
        cfg: &Cfg,
        machine: &str,
        sequence: i64,
        previous: &str,
        kind: &str,
        payload: Value,
    ) -> String {
        let directory = cfg.control.join(format!("EVENTS.{machine}"));
        std::fs::create_dir_all(&directory).unwrap();
        let core = json!({
            "schema": 1, "machine": machine, "sequence": sequence,
            "previous_sha256": previous,
            "recorded_at": "2026-01-01T00:00:00+00:00",
            "kind": kind, "payload": payload,
        });
        let digest = canonical_sha256(&core).unwrap();
        let mut event = core.as_object().unwrap().clone();
        event.insert("sha256".to_string(), json!(digest));
        std::fs::write(
            directory.join(format!("{sequence:020}.json")),
            serde_json::to_string(&Value::Object(event)).unwrap(),
        )
        .unwrap();
        digest
    }

    fn temp_control(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wrkslotsd-2125-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scoped_hold_needs_only_addressed_events_despite_poison() {
        // Listdir poison (#2125): a corrupt non-addressed event must
        // not affect the O(1) scoped read, which touches only the
        // held event and its predecessor.
        let control = temp_control("poison");
        let cfg = test_cfg(control.clone());
        let zeros = "0".repeat(64);
        let first = write_event(
            &cfg,
            "node-a",
            1,
            &zeros,
            "state-imported",
            json!({"active": {"revision": 0}, "holds": []}),
        );
        let _held = write_event(
            &cfg,
            "node-a",
            2,
            &first,
            "slot-held",
            json!({"slot": "slot-a", "reason": "probe hold"}),
        );
        // Poison: overwrite event 1 with a bad digest. A full fold
        // would refuse; the scoped read of event 2 still verifies
        // against event 1's file only as a predecessor digest source,
        // so corrupt event 1's own digest breaks the chain check --
        // use a different poison: an extra invalid file the folder
        // would list but the scoped reader never opens.
        let directory = control.join("EVENTS.node-a");
        std::fs::write(directory.join("00000000000000000099.json"), "{not json").unwrap();
        std::fs::write(
            control.join("EVENT-INDEX.node-a.json"),
            serde_json::to_string(&json!({
                "schema": 1, "machine": "node-a",
                "tip": {"sequence": 2, "sha256": _held},
                "active_revision": 0,
                "slots": {"slot-a": {"held": 2}},
            }))
            .unwrap(),
        )
        .unwrap();
        let found = scoped_hold_exists(None, &cfg, "node-a", &["slot-a".to_string()]).unwrap();
        assert_eq!(
            found,
            Some(("slot-a".to_string(), "probe hold".to_string()))
        );
        let _ = std::fs::remove_dir_all(&control);
    }

    #[test]
    fn scoped_hold_honours_imported_spelling_and_release_order() {
        let control = temp_control("spelling");
        let cfg = test_cfg(control.clone());
        let zeros = "0".repeat(64);
        let imported = write_event(
            &cfg,
            "node-a",
            1,
            &zeros,
            "state-imported",
            json!({"holds": [{"slot": "slot-a", "reason": "imported hold"}]}),
        );
        let _ = &imported;
        std::fs::write(
            control.join("EVENT-INDEX.node-a.json"),
            serde_json::to_string(&json!({
                "schema": 1, "machine": "node-a",
                "tip": {"sequence": 1, "sha256": imported},
                "active_revision": 0,
                "slots": {"slot-a": {"imported": 1}},
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            scoped_hold_exists(None, &cfg, "node-a", &["slot-a".to_string()]).unwrap(),
            Some(("slot-a".to_string(), "imported hold".to_string()))
        );
        let _ = std::fs::remove_dir_all(&control);
    }

    #[test]
    fn missing_index_entry_falls_back_to_streamed_hold() {
        // #2125b: absence from the index is unprovable, so the
        // streamed fold still finds the hold and refuses.
        let control = temp_control("fallback");
        let cfg = test_cfg(control.clone());
        let zeros = "0".repeat(64);
        let first = write_event(
            &cfg,
            "node-a",
            1,
            &zeros,
            "state-imported",
            json!({"active": {"revision": 0}, "holds": []}),
        );
        let held = write_event(
            &cfg,
            "node-a",
            2,
            &first,
            "slot-held",
            json!({"slot": "slot-a", "reason": "streamed hold"}),
        );
        std::fs::write(
            control.join("EVENT-INDEX.node-a.json"),
            serde_json::to_string(&json!({
                "schema": 1, "machine": "node-a",
                "tip": {"sequence": 2, "sha256": held},
                "active_revision": 0, "slots": {},
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            scoped_hold_exists(None, &cfg, "node-a", &["slot-a".to_string()]).unwrap(),
            Some(("slot-a".to_string(), "streamed hold".to_string()))
        );
        let _ = std::fs::remove_dir_all(&control);
    }

    #[test]
    fn proof_mismatch_and_lock_seconds_are_observed() {
        // Failover on proof mismatch: a saved proof never matches a
        // registry with no readable history, so the caller re-reads.
        let control = temp_control("proof");
        let cfg = test_cfg(control.clone());
        let saved: HistoryProof = (
            vec![("node-a".to_string(), 1, "a".repeat(64), 0, 0)],
            vec![],
        );
        assert!(!history_proof_matches(None, &cfg, &saved));
        let payload = with_lock_seconds(json!({"schema": 1}), std::time::Instant::now());
        let seconds = payload.get("lock_seconds").and_then(Value::as_f64).unwrap();
        assert!(seconds >= 0.0);
        let _ = std::fs::remove_dir_all(&control);
    }

    fn planned_with_owner(owner: Value) -> Planned {
        Planned {
            row: Row {
                machine: "node-a".to_string(),
                slot: "slot-a".to_string(),
                generation: 1,
                record_sha256: "a".repeat(64),
            },
            record: json!({"slot": "slot-a", "owner": owner}),
            entry: json!({}),
        }
    }

    fn complete_owner() -> Value {
        json!({"pid": 999999, "start_ticks": 1, "boot_id": "boot", "host_id": "host"})
    }

    fn gate_cfg(tag: &str) -> (PathBuf, Cfg) {
        let base = temp_control(tag);
        let control = base.join("worktrees");
        std::fs::create_dir_all(&control).unwrap();
        let cfg = test_cfg(control);
        (base, cfg)
    }

    #[test]
    fn uncovered_bind_record_refuses() {
        let (base, cfg) = gate_cfg("gate-bind");
        let control = cfg.control.clone();
        std::fs::write(
            control.join(".worktrees-image.json"),
            json!({"binds": {"slot-a": "/subvol/slot-a"}}).to_string(),
        )
        .unwrap();
        let err = refuse_uncovered_write(&cfg, "node-a", &[planned_with_owner(complete_owner())])
            .unwrap_err();
        assert!(err.contains("bind record"), "{err}");
        assert!(err.contains("Python edition"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn uncovered_cross_machine_row_refuses() {
        let (base, cfg) = gate_cfg("gate-cross");
        let control = cfg.control.clone();
        std::fs::write(
            control.join("ACTIVE.node-b.json"),
            json!({"schema": 2, "machine": "node-b", "revision": 1, "slots": [{"slot": "slot-b"}]})
                .to_string(),
        )
        .unwrap();
        let err = refuse_uncovered_write(&cfg, "node-a", &[planned_with_owner(complete_owner())])
            .unwrap_err();
        assert!(err.contains("non-selected machine node-b"), "{err}");
        assert!(err.contains("Python edition"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn uncovered_census_cases_refuse() {
        // No recorded owner generation.
        let (base, cfg) = gate_cfg("gate-census-owner");
        let err =
            refuse_uncovered_write(&cfg, "node-a", &[planned_with_owner(Value::Null)]).unwrap_err();
        assert!(err.contains("host-wide process census"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
        // Retained handle naming the row.
        let (base, cfg) = gate_cfg("gate-census-handle");
        let runs = base.join("ignored").join("validate").join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(
            runs.join("run-1.json"),
            json!({"slot": "slot-a"}).to_string(),
        )
        .unwrap();
        let err = refuse_uncovered_write(&cfg, "node-a", &[planned_with_owner(complete_owner())])
            .unwrap_err();
        assert!(err.contains("retained validation handle"), "{err}");
        assert!(err.contains("host-wide process census"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn covered_row_passes_uncovered_gate() {
        let (base, cfg) = gate_cfg("gate-covered");
        assert!(
            refuse_uncovered_write(&cfg, "node-a", &[planned_with_owner(complete_owner())]).is_ok()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn deleted_root_covers_mount_slot_and_checkout_but_not_live_binds() {
        let table = "\
41 30 0:41 /deadbind//deleted /slots/validate/deadbind/product rw - btrfs /dev/loop1 rw\n\
42 30 0:41 /live /slots/validate/live/product rw - btrfs /dev/loop1 rw\n";
        let checkout = Path::new("/slots/validate/deadbind/product");
        assert!(mountinfo_covers_deleted_bind(checkout, table));
        assert!(mountinfo_covers_deleted_bind(
            Path::new("/slots/validate/deadbind"),
            table
        ));
        assert!(!mountinfo_covers_deleted_bind(
            Path::new("/slots/validate/live/product"),
            table
        ));
        assert!(!mountinfo_covers_deleted_bind(
            Path::new("/slots/validate/other"),
            table
        ));
    }
}
