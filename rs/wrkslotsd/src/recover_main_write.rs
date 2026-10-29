//! Writer: the main `recover` dispatcher, plan outcomes.
//!
//! Reproduces the dispatcher's read-only outcomes of the Python
//! command: the coordinator process capture, the flag-combination
//! contract, partial-state refusal, the no-journal refusal, the
//! ownerless-cleanup "already recorded" refusal, and journal
//! schema/machine binding. Actually resuming a journal (which
//! writes the recovery-started event and then mutates), recovering
//! partial updates, and starting ownerless validation cleanup stay
//! with the Python edition and refuse explicitly. Fixture
//! registries only until the edition cutover (see
//! `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::active_state::load_cfg;
use crate::config_validate::validate_name;
use crate::ensure_states::{refuse_partial_state, require_fixture};
use crate::hold_write::acquire_mutation_locks;
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

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
