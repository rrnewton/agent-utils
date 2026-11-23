//! Writer: recover absent validation rows (`recover-absent-validate-rows`).
//!
//! Reproduces the input contract and the non-destructive outcomes
//! of the Python command: the bounded JSON input file (schema 1,
//! exact row identities, input digest), the local-machine check,
//! interrupted-batch journal refusals, already-recovered
//! recognition bound to the exact archive record and its recovery
//! event, row validation (type, generation, record digest), and
//! the still-has-physical-storage refusal. The validation census
//! plan/apply path stays with the Python edition and refuses
//! explicitly. Fixture registries only until the edition cutover
//! (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory, Cfg};
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{refuse_partial_state, require_fixture};
use crate::finish_write::assert_record_paths;
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

struct Row {
    machine: String,
    slot: String,
    generation: i64,
    record_sha256: String,
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

/// The history-folding outcome computation, shared by the
/// prove-then-recheck rounds (no locks held) and the ordinary
/// locked path (#2125).
fn recover_outcomes(root: Option<&Path>, cfg: &Cfg, selected: &str, rows: &[Row]) -> Result<Value> {
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
    let mut outcomes: Vec<Value> = Vec::new();
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
            return Err(format!(
                "absent validation row recovery censuses for {} stay with the Python edition for now",
                row.slot
            ));
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
        outcomes.push(json!({
            "machine": row.machine,
            "slot": row.slot,
            "generation": row.generation,
            "outcome": "already-recovered",
            "detail": "exact archive already exists",
        }));
    }
    Ok(json!({"schema": 1, "rows": outcomes}))
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
    if apply {
        if !coordinator_authorized {
            return Err(
                "absent validation row recovery requires --coordinator-authorized with --apply"
                    .to_string(),
            );
        }
        let pid = coordinator_pid.ok_or("--apply requires --coordinator-pid")?;
        read_process_identity(pid)?;
    }
    // Prove-then-recheck (#2125): fold the history and judge every
    // row WITHOUT the mutation locks, then take them and return the
    // prepared outcome only when the cheap proof shows the history
    // has not moved since the fold. A moved history is re-read
    // (bounded rounds); an unprovable one, an outstanding journal
    // (the locked path below judges journals, as before), or
    // exhausted rounds take the ordinary locked path unchanged.
    for _round in 0..HISTORY_RECHECK_ATTEMPTS {
        let Some(proof) = history_proof(root, &cfg) else {
            break;
        };
        if !journals_absent(root) {
            break;
        }
        let prepared = recover_outcomes(root, &cfg, &selected, &rows);
        if let Err(message) = &prepared {
            if history_proof(root, &cfg).as_ref() == Some(&proof) {
                return Err(message.clone());
            }
            continue;
        }
        let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
        if history_proof(root, &cfg).as_ref() != Some(&proof) {
            continue;
        }
        refuse_partial_state(&cfg.control)?;
        return prepared;
    }
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
    recover_outcomes(root, &cfg, &selected, &rows)
}

#[cfg(test)]
mod tests {
    use super::mountinfo_covers_deleted_bind;
    use std::path::Path;

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
