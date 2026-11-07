//! Writer: the main `recover` dispatcher, plan outcomes.
//!
//! Reproduces the dispatcher's read-only outcomes of the Python
//! command: the coordinator process capture, the flag-combination
//! contract, partial-state refusal, the no-journal refusal, the
//! ownerless-cleanup "already recorded" refusal, and journal
//! schema/machine binding. Before any of those refusals, a scoped
//! create/finish journal exactly one progress step behind its
//! append-only event log is rolled forward to the latest progress
//! (Python `_roll_forward_lagging_scoped_journals`, #1007), and the
//! repair persists even when the rest of the run then refuses.
//! Actually resuming a journal (which writes the recovery-started
//! event and then mutates), recovering partial updates, and
//! starting ownerless validation cleanup stay with the Python
//! edition and refuse explicitly. Fixture registries only until
//! the edition cutover (see `ensure_states::require_fixture`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::active_state::{as_mapping, as_str, load_cfg, Cfg};
use crate::config_validate::validate_name;
use crate::ensure_states::{atomic_write_json, refuse_partial_state, require_fixture};
use crate::global_state::load_event_chain;
use crate::handoff_state::{read_regular_file_identity, strict_json_object};
use crate::hold_write::acquire_mutation_locks;
use crate::journal_state::{
    journal_scoped_path, json_equal, operation_event_journal_path, scoped_operation_identity,
    scoped_operation_opens,
};
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

/// Latest progress for a journal exactly one step behind, else
/// `None` (Python `_scoped_journal_roll_forward_target`).
fn scoped_journal_roll_forward_target(
    cfg: &Cfg,
    path: &Path,
    raw: &Map<String, Value>,
    operation: &str,
) -> Result<Option<Map<String, Value>>> {
    let label = format!("scoped {operation} journal");
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        &format!("{operation} journal.machine"),
    )?
    .to_owned();
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{machine}")), &machine)?;
    let mut recorded: Option<Map<String, Value>> = None;
    let mut completed = false;
    let mut current: Option<String> = None;
    let mut retired: BTreeSet<String> = BTreeSet::new();
    let mut episode: Vec<Map<String, Value>> = Vec::new();
    for event in &events {
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        if kind != "operation-progress-recorded" && kind != "operation-completed" {
            continue;
        }
        let payload = as_mapping(
            event.get("payload").unwrap_or(&Value::Null),
            "append-only event.payload",
        )?;
        let journal = if kind == "operation-progress-recorded" {
            Some(as_mapping(
                payload.get("journal").unwrap_or(&Value::Null),
                "append-only operation journal",
            )?)
        } else {
            None
        };
        if operation_event_journal_path(cfg, &machine, payload, journal)? != path {
            continue;
        }
        if kind == "operation-progress-recorded" {
            let journal = journal.expect("progress journal");
            let identity = scoped_operation_identity(journal, operation);
            if completed && (!scoped_operation_opens(journal, operation) || identity.is_none()) {
                return Err(format!(
                    "{label} path was reused after completion without a new operation"
                ));
            }
            if let Some(identity) = &identity {
                if retired.contains(identity) {
                    return Err(format!(
                        "{label} path was reopened with the identity of a completed operation"
                    ));
                }
            }
            if recorded.is_none() || completed {
                current = identity;
                episode = vec![journal.clone()];
            } else if identity != current {
                return Err(format!("{label} progress changed its operation identity"));
            } else {
                episode.push(journal.clone());
            }
            recorded = Some(journal.clone());
            completed = false;
            continue;
        }
        if recorded.is_none() {
            return Err(format!(
                "scoped {operation} completion has no preceding progress evidence"
            ));
        }
        if let Some(identity) = &current {
            retired.insert(identity.clone());
        }
        completed = true;
        episode = Vec::new();
    }
    let Some(recorded) = recorded else {
        return Err(format!(
            "{label} has no matching append-only progress evidence"
        ));
    };
    if completed
        || json_equal(
            &Value::Object(raw.clone()),
            &Value::Object(recorded.clone()),
        )
    {
        return Ok(None);
    }
    if episode.len() >= 2
        && json_equal(
            &Value::Object(raw.clone()),
            &Value::Object(episode[episode.len() - 2].clone()),
        )
    {
        return Ok(Some(recorded));
    }
    Ok(None)
}

/// Repair scoped journals one step behind their event log
/// (Python `_roll_forward_lagging_scoped_journals`, #1007).
fn roll_forward_lagging_scoped_journals(cfg: &Cfg) -> Result<()> {
    let mut create_paths: Vec<PathBuf> = Vec::new();
    let mut finish_paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.control) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("CREATE.") && name.ends_with(".journal") {
                create_paths.push(entry.path());
            } else if name.starts_with("FINISH.") && name.ends_with(".journal") {
                finish_paths.push(entry.path());
            }
        }
    }
    create_paths.sort();
    finish_paths.sort();
    let mut paths = create_paths;
    paths.extend(finish_paths);
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_regular = path
            .symlink_metadata()
            .map(|meta| !meta.file_type().is_symlink() && meta.is_file())
            .unwrap_or(false);
        if !is_regular {
            continue;
        }
        let tmp_prefix = format!("{name}.tmp.");
        let has_tmp = path
            .parent()
            .and_then(|parent| std::fs::read_dir(parent).ok())
            .map(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.file_name().to_string_lossy().starts_with(&tmp_prefix))
            })
            .unwrap_or(false);
        if has_tmp {
            continue;
        }
        let raw = match read_regular_file_identity(&path, "recovery journal", 1024 * 1024)
            .and_then(|(contents, _identity)| strict_json_object(&contents, "recovery journal"))
        {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let operation = match raw.get("kind").and_then(Value::as_str) {
            Some(operation) if operation == "create" || operation == "finish" => {
                operation.to_owned()
            }
            _ => continue,
        };
        let (Some(machine), Some(slot)) = (
            raw.get("machine").and_then(Value::as_str),
            raw.get("slot").and_then(Value::as_str),
        ) else {
            continue;
        };
        let (machine, slot) = (machine.to_owned(), slot.to_owned());
        if validate_name(&machine, "machine").is_err() || validate_name(&slot, "slot").is_err() {
            continue;
        }
        let expected = journal_scoped_path(
            cfg,
            if operation == "create" {
                "CREATE"
            } else {
                "FINISH"
            },
            &machine,
            &slot,
        );
        if path != expected {
            continue;
        }
        let Some(target) = scoped_journal_roll_forward_target(cfg, &path, &raw, &operation)? else {
            continue;
        };
        atomic_write_json(&path, &Value::Object(target))?;
        println!(
            "rolled forward scoped {operation} journal {name} to latest append-only progress evidence"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_main(
    root: Option<&Path>,
    machine_override: Option<&str>,
    coordinator_pid: Option<i64>,
    slot: Option<&str>,
    coordinator_authorized: bool,
    discard_partial: bool,
    discard_stale_image_copy: bool,
    abort_import: bool,
    legacy_validate_checkout: Option<&str>,
    ownerless_validate_checkout: Option<&str>,
    ownerless_validate_cargo_home: Option<&str>,
    frozen_validate_checkout: Option<&str>,
    completed_record: Option<&str>,
    repository: Option<&str>,
    recovery_note: Option<&str>,
    validation_proof_manifest: Option<&str>,
) -> Result<Value> {
    require_fixture()?;
    let pid = coordinator_pid.ok_or("--coordinator-pid is required")?;
    read_process_identity(pid)?;
    let legacy_requested = legacy_validate_checkout.is_some();
    if legacy_requested
        && (ownerless_validate_checkout.is_some()
            || ownerless_validate_cargo_home.is_some()
            || frozen_validate_checkout.is_some())
    {
        return Err("legacy and ownerless validation path flags cannot be combined".to_string());
    }
    if legacy_requested
        && (completed_record.is_none() || repository.is_none() || recovery_note.is_some())
    {
        return Err(
            "--legacy-validate-checkout requires --completed-record and --repository".to_string(),
        );
    }
    let requested_ownerless = legacy_requested
        || ownerless_validate_checkout.is_some()
        || ownerless_validate_cargo_home.is_some()
        || frozen_validate_checkout.is_some()
        || completed_record.is_some()
        || repository.is_some()
        || recovery_note.is_some()
        || validation_proof_manifest.is_some();
    if slot.is_some() && requested_ownerless {
        return Err(
            "--slot selects an interrupted mutation and cannot be combined with validation cleanup flags"
                .to_string(),
        );
    }
    if discard_stale_image_copy {
        if !coordinator_authorized {
            return Err(
                "--discard-stale-image-copy is the coordinator's decision and requires --coordinator-authorized. state: REFUSED -- nothing was recovered"
                    .to_string(),
            );
        }
        if requested_ownerless {
            return Err(
                "--discard-stale-image-copy applies only to an interrupted removal and cannot be combined with validation cleanup flags"
                    .to_string(),
            );
        }
    }
    if requested_ownerless {
        if !coordinator_authorized {
            return Err(
                "ownerless validation cleanup is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                    .to_string(),
            );
        }
        if abort_import {
            return Err(
                "--abort-import applies only to an interrupted import-existing journal and cannot be combined with validation cleanup flags"
                    .to_string(),
            );
        }
        let targets = [
            ownerless_validate_checkout.is_some() || legacy_requested,
            ownerless_validate_cargo_home.is_some(),
            frozen_validate_checkout.is_some(),
        ];
        if targets.iter().filter(|present| **present).count() != 1 {
            return Err(
                "ownerless validation cleanup requires exactly one target path".to_string(),
            );
        }
        if frozen_validate_checkout.is_some()
            && (completed_record.is_none() || repository.is_none() || recovery_note.is_some())
        {
            return Err(
                "--frozen-validate-checkout requires --completed-record and --repository; provide the terminal run handle and its source checkout"
                    .to_string(),
            );
        }
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    // #1007: roll a lagging scoped journal forward before any other
    // refusal in this run; the repair persists even when the rest
    // of the run then refuses.
    roll_forward_lagging_scoped_journals(&cfg)?;
    refuse_partial_state(&cfg.control)?;
    let _ = discard_partial;
    // Outstanding journals: singleton and scoped journal files.
    let mut journals: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&cfg.control) {
        for entry in entries.flatten() {
            let text = entry.file_name().to_string_lossy().into_owned();
            if (text.starts_with("ACTIVE.")
                || text.starts_with("CREATE.")
                || text.starts_with("FINISH.")
                || text.starts_with("VALIDATE-BATCH-SEAL."))
                && text.ends_with(".journal")
            {
                journals.push(entry.path());
            }
        }
    }
    journals.sort();
    if requested_ownerless {
        if !journals.is_empty() {
            return Err(
                "an interrupted mutation is already recorded. state: REFUSED -- the new validation cleanup did not start. remedy: rerun 'wrkslots recover --coordinator-pid PID' without cleanup flags to finish the recorded operation first"
                    .to_string(),
            );
        }
        return Err(
            "starting ownerless validation cleanup stays with the Python edition for now"
                .to_string(),
        );
    }
    if journals.is_empty() {
        return Err("no interrupted mutation is recorded".to_string());
    }
    // With several journals and no --slot selection, Python refuses
    // asking for one exact slot.
    let chosen: &Path = if let Some(wanted) = slot {
        let mut matches: Vec<&PathBuf> = Vec::new();
        for path in &journals {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(_) => continue,
            };
            if let Ok(raw) = serde_json::from_str::<Value>(&text) {
                if raw.get("slot").and_then(Value::as_str) == Some(wanted) {
                    matches.push(path);
                }
            }
        }
        match matches.as_slice() {
            [only] => only,
            _ => {
                return Err(format!(
                    "no interrupted mutation is recorded for slot {wanted}"
                ));
            }
        }
    } else if journals.len() > 1 {
        return Err(
            "multiple interrupted mutations are present; select one exact slot with '--slot SLOT' or inspect every journal before continuing"
                .to_string(),
        );
    } else {
        &journals[0]
    };
    let text = std::fs::read_to_string(chosen)
        .map_err(|error| format!("cannot read recovery journal: {error}"))?;
    let raw: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read recovery journal: {error}"))?;
    let kind = raw
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if discard_stale_image_copy && kind != "finish" {
        return Err(format!(
            "--discard-stale-image-copy applies only to an interrupted removal of an image-backed agent slot; the recorded journal is {kind:?}"
        ));
    }
    if abort_import && kind != "import-existing" {
        return Err(format!(
            "--abort-import applies only to import-existing journals; the recorded journal is {kind:?}"
        ));
    }
    if raw.get("schema").and_then(Value::as_i64) != Some(2) {
        return Err("unsupported recovery journal schema".to_string());
    }
    let machine = raw
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if machine != selected {
        return Err(format!(
            "journal belongs to machine {machine}; rerun with --machine {machine}"
        ));
    }
    Err(format!(
        "resuming the recorded {kind} recovery stays with the Python edition for now"
    ))
}
