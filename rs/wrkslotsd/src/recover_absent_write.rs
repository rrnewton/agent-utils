//! Writer: recover one exact absent agent row (`recover-absent-agent-row`,
//! and the bounded batch shell `recover-absent-agent-rows`).
//!
//! Reproduces the non-destructive outcomes of the Python commands:
//! the already-recovered recognition (exact archive record plus its
//! recovery event), the interrupted-journal plan outcome
//! (`would-resume`), the differing-journal and
//! neither-active-nor-recovered refusals, generation and record
//! digest validation, and the still-has-physical-storage refusal.
//! The destructive proof path itself (host/process/systemd
//! censuses, submodule salvage, rescue-ref pushes, registration
//! removal) stays with the Python edition and refuses explicitly
//! once storage is proven absent. Fixture registries only until
//! the edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, slot_directory, Cfg};
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{refuse_partial_state, require_fixture};
use crate::finish_write::assert_record_paths;
use crate::global_state::load_event_chain;
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

const BATCH_LIMIT: usize = 128;

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

fn archives_document(cfg: &Cfg, selected: &str) -> Result<Value> {
    let path = cfg.control.join(format!("ARCHIVED.{selected}.json"));
    if !path.exists() {
        return Ok(json!({"schema": 2, "machine": selected, "revision": 0, "records": []}));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read archive state: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("cannot read archive state: {error}"))
}

fn recovery_matches(cfg: &Cfg, selected: &str, slot: &str, generation: i64, sha: &str) -> bool {
    let Ok(events) = load_event_chain(&cfg.control.join(format!("EVENTS.{selected}")), selected)
    else {
        return false;
    };
    events.iter().any(|event| {
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        event.get("kind").and_then(Value::as_str) == Some("archive-state-recorded")
            && payload.get("action").and_then(Value::as_str) == Some("absent-agent-row-recovered")
            && payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload
                .get("record")
                .and_then(|record| record.get("generation"))
                .and_then(Value::as_i64)
                == Some(generation)
            && payload
                .get("evidence")
                .and_then(|evidence| evidence.get("source_record_sha256"))
                .and_then(Value::as_str)
                == Some(sha)
            && payload
                .get("evidence")
                .and_then(|evidence| evidence.get("archive_id"))
                .and_then(Value::as_str)
                == payload
                    .get("record")
                    .and_then(|record| record.get("archive_id"))
                    .and_then(Value::as_str)
    })
}

fn journal_path(cfg: &Cfg, selected: &str) -> PathBuf {
    cfg.control.join(format!("ACTIVE.{selected}.journal"))
}

/// One absent-agent-row decision. Returns the outcome word.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_absent_agent_row(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    expected_generation: i64,
    record_sha256: &str,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    validate_name(slot, "slot")?;
    if !is_digest(record_sha256) {
        return Err("--record-sha256 must be one lowercase SHA-256 digest".to_string());
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    if selected != short_hostname()? {
        return Err("absent agent row recovery must run on the row's machine".to_string());
    }
    let coordinator = if apply {
        if !coordinator_authorized {
            return Err(
                "absent agent row recovery requires --coordinator-authorized with --apply"
                    .to_string(),
            );
        }
        let pid = coordinator_pid.ok_or("--apply requires --coordinator-pid")?;
        Some(read_process_identity(pid)?)
    } else {
        None
    };
    let _ = coordinator;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    // Interrupted-journal outcomes come before any global validation.
    let journal = journal_path(&cfg, &selected);
    if journal.exists() || journal.is_symlink() {
        let text = std::fs::read_to_string(&journal)
            .map_err(|error| format!("cannot read recovery journal: {error}"))?;
        let raw: Value = serde_json::from_str(&text)
            .map_err(|error| format!("cannot read recovery journal: {error}"))?;
        if raw.get("kind").and_then(Value::as_str) != Some("recover-absent-agent-row") {
            return Err(format!(
                "another interrupted mutation is recorded in {}",
                journal.display()
            ));
        }
        let input = raw.get("input").cloned().unwrap_or(Value::Null);
        let same = input.get("slot").and_then(Value::as_str) == Some(slot)
            && input.get("generation").and_then(Value::as_i64) == Some(expected_generation)
            && input.get("record_sha256").and_then(Value::as_str) == Some(record_sha256);
        if !same {
            return Err("requested agent row differs from the interrupted recovery".to_string());
        }
        if !apply {
            return Ok(json!({
                "machine": selected,
                "slot": slot,
                "generation": expected_generation,
                "outcome": "would-resume",
            }));
        }
        return Err(
            "resuming an interrupted absent-agent-row recovery stays with the Python edition for now"
                .to_string(),
        );
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
        .cloned();
    let Some(record) = record else {
        // Already-recovered recognition.
        let archives = archives_document(&cfg, &selected)?;
        let records = archives
            .get("records")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let matching: Vec<&Value> = records
            .iter()
            .filter(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
            .collect();
        let exact = matching.len() == 1
            && matching[0].get("machine").and_then(Value::as_str) == Some(selected.as_str())
            && matching[0].get("generation").and_then(Value::as_i64) == Some(expected_generation)
            && matching[0].get("slot_type").and_then(Value::as_str) == Some("agent")
            && matching[0].get("physical_storage").and_then(Value::as_str) == Some("removed")
            && matching[0]
                .get("validation")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.as_str()
                            == Some(&format!("source ACTIVE record SHA-256: {record_sha256}"))
                    })
                })
            && recovery_matches(&cfg, &selected, slot, expected_generation, record_sha256);
        if exact {
            return Ok(json!({
                "machine": selected,
                "slot": slot,
                "generation": expected_generation,
                "outcome": "already-recovered",
            }));
        }
        return Err(format!(
            "agent row {selected}/{slot} is neither active nor an exact prior recovery"
        ));
    };
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    if slot_type != "agent" {
        return Err(format!("slot {slot} is not an agent row"));
    }
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!("agent row {selected}/{slot} generation changed"));
    }
    let observed = canonical_sha256(&record).map_err(|error| error.to_string())?;
    if observed != record_sha256 {
        return Err(format!(
            "agent row {selected}/{slot} changed: expected {record_sha256}, found {observed}"
        ));
    }
    // Physical-storage proof (the census/salvage proof that follows
    // in Python refuses explicitly below once storage is absent).
    assert_record_paths(&cfg, &record)?;
    let slot_path = slot_directory(&cfg, slot, slot_type)?;
    let mut paths: Vec<PathBuf> = vec![slot_path.clone()];
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
    paths.sort();
    paths.dedup();
    for path in &paths {
        if path.exists() || path.is_symlink() {
            return Err(format!(
                "agent row {slot} still has physical storage at {}",
                path.display()
            ));
        }
    }
    let agent_root = slot_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Ok(entries) = std::fs::read_dir(&agent_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!(".{slot}.fenced.{generation}."))
                || name.starts_with(&format!(".{slot}.ownerless-agent."))
            {
                return Err(format!(
                    "agent row {slot} has a possible fenced path: {}",
                    entry.path().display()
                ));
            }
        }
    }
    Err(format!(
        "absent agent row recovery censuses and salvage for {slot} stay with the Python edition for now"
    ))
}

/// Parse one `SLOT=GENERATION=SHA256` batch row.
fn parse_row(value: &str) -> Result<(String, i64, String)> {
    let parts: Vec<&str> = value.split('=').collect();
    if parts.len() != 3 {
        return Err(format!(
            "absent agent row must be SLOT=GENERATION=SHA256: {value:?}"
        ));
    }
    validate_name(parts[0], "slot")?;
    let generation: i64 = parts[1]
        .parse()
        .map_err(|_| format!("absent agent row generation is not an integer: {value:?}"))?;
    if generation < 1 {
        return Err(format!(
            "absent agent row generation must be positive: {value:?}"
        ));
    }
    if !is_digest(parts[2]) {
        return Err(format!(
            "absent agent row record SHA-256 must be one lowercase digest: {value:?}"
        ));
    }
    Ok((parts[0].to_string(), generation, parts[2].to_string()))
}

/// The bounded batch shell over `recover_absent_agent_row`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_absent_agent_rows(
    root: Option<&Path>,
    machine_override: Option<&str>,
    rows: &[String],
    input: Option<&Path>,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    let mut raw: Vec<String> = rows.to_vec();
    if let Some(path) = input {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read --input {}: {error}", path.display()))?;
        for line in text.lines() {
            let stripped = line.trim();
            if !stripped.is_empty() && !stripped.starts_with('#') {
                raw.push(stripped.to_string());
            }
        }
    }
    if raw.is_empty() {
        return Err(
            "recover-absent-agent-rows needs at least one --row or a non-empty --input".to_string(),
        );
    }
    if raw.len() > BATCH_LIMIT {
        return Err(format!(
            "recover-absent-agent-rows accepts at most {BATCH_LIMIT} rows; run another bounded batch for the remainder"
        ));
    }
    let mut parsed: Vec<(String, i64, String)> = Vec::new();
    for value in &raw {
        parsed.push(parse_row(value)?);
    }
    let mut seen: Vec<&str> = Vec::new();
    for (slot, _generation, _sha) in &parsed {
        if seen.contains(&slot.as_str()) {
            return Err(
                "recover-absent-agent-rows received the same slot more than once".to_string(),
            );
        }
        seen.push(slot.as_str());
    }
    let started = std::time::Instant::now();
    let mut out_rows: Vec<Value> = Vec::new();
    for (slot, generation, sha) in &parsed {
        let item_started = std::time::Instant::now();
        let (outcome, reason, machine) = match recover_absent_agent_row(
            root,
            machine_override,
            slot,
            *generation,
            sha,
            apply,
            coordinator_authorized,
            coordinator_pid,
        ) {
            Ok(value) => (
                value
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                None,
                value
                    .get("machine")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Err(error) => ("refused".to_string(), Some(error), String::new()),
        };
        let output: Vec<String> = if outcome == "refused" {
            Vec::new()
        } else {
            vec![format!(
                "ROW machine={machine} slot={slot} generation={generation} outcome={outcome}"
            )]
        };
        let mut row = json!({
            "slot": slot,
            "generation": generation,
            "outcome": outcome,
            "seconds": (item_started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
            "output": output,
        });
        if let Some(reason) = reason {
            if let Some(object) = row.as_object_mut() {
                object.insert("reason".to_string(), json!(reason));
            }
        }
        out_rows.push(row);
    }
    let count = |name: &str| {
        out_rows
            .iter()
            .filter(|row| row.get("outcome").and_then(Value::as_str) == Some(name))
            .count()
    };
    Ok(json!({
        "schema": 1,
        "batch_limit": BATCH_LIMIT,
        "mode": if apply { "apply" } else { "plan" },
        "requested": parsed.len(),
        "counts": {
            "would-recover": count("would-recover"),
            "would-resume": count("would-resume"),
            "recovered": count("recovered"),
            "already-recovered": count("already-recovered"),
            "refused": count("refused"),
        },
        "rows": out_rows,
        "recovery_required": false,
        "seconds": (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
    }))
}
