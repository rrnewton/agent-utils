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
use crate::global_state::load_event_chain;
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
    let mut file = std::fs::OpenOptions::new()
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
    let Ok(events) = load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), selected)
    else {
        return false;
    };
    events.iter().any(|event| {
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
    })
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
    validate_global(root, &cfg)?;
    let active = active_document(root, &cfg, &selected)?;
    let archives = crate::recover_absent_write::archives_document(&cfg, &selected)?;
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
    for row in &rows {
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
            assert_record_paths(&cfg, &record)?;
            let slot_path = slot_directory(&cfg, &row.slot, slot_type)?;
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
            && matching[0].get("machine").and_then(Value::as_str) == Some(selected.as_str())
            && matching[0].get("generation").and_then(Value::as_i64) == Some(row.generation)
            && matching[0].get("slot_type").and_then(Value::as_str) == Some("validate")
            && matching[0].get("physical_storage").and_then(Value::as_str) == Some("removed")
            && recovery_matches(&cfg, &selected, matching[0], archives_revision, row);
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
