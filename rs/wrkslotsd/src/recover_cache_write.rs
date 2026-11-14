//! Writers: ownerless agent cache relocation plans
//! (`recover-ownerless-agent-cache`) and the ownerless validation
//! batch input contract (`recover-ownerless-validate-batch`,
//! `classify-ownerless-validate-batch`).
//!
//! For the cache, Rust reproduces the read-only plan outcomes: the
//! local-machine check, interrupted-journal resume planning,
//! registry collision, destination-exists refusal, and the exact
//! cache tree shape (validate/cache/rust-script, no symlinks, no
//! authored-work markers). The relocation itself, and the
//! process/systemd censuses in the tree proof, stay with the
//! Python edition and refuse explicitly. For the validation batch
//! commands, Rust reproduces the argument-shape contract and then
//! refuses explicitly: their shared process census and recovery
//! stay with the Python edition. Fixture registries only until
//! the edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::config_validate::validate_name;
use crate::ensure_states::{refuse_partial_state, require_fixture};
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

const BATCH_LIMIT: usize = 8;

fn short_hostname() -> Result<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|error| format!("cannot read the machine hostname: {error}"))?;
    let name = raw.trim().split('.').next().unwrap_or_default().to_string();
    validate_name(&name, "machine")?;
    Ok(name)
}

fn cache_paths(cfg: &crate::active_state::Cfg) -> (PathBuf, PathBuf) {
    let source = cfg.worktrees.join("ignored");
    let destination = cfg
        .root
        .join("ignored")
        .join("validate")
        .join("cache")
        .join("wrkslots-agent-ignored");
    (source, destination)
}

fn assert_cache_tree(source: &Path) -> Result<()> {
    if source.is_symlink() || !source.is_dir() {
        return Err(format!(
            "ownerless agent cache path is absent or unsafe: {}",
            source.display()
        ));
    }
    let expected = source.join("validate").join("cache").join("rust-script");
    if !expected.is_dir() || expected.is_symlink() {
        return Err(
            "ownerless agent cache must contain the exact validate/cache/rust-script tree"
                .to_string(),
        );
    }
    fn walk(dir: &Path, root: &Path) -> Result<()> {
        let entries = std::fs::read_dir(dir)
            .map_err(|error| format!("cannot enumerate ownerless agent cache: {error}"))?;
        let mut names: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|error| format!("cannot enumerate ownerless agent cache: {error}"))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_symlink() {
                return Err(format!(
                    "ownerless agent cache contains a symlink: {}",
                    path.display()
                ));
            }
            if name == ".git" || name == "HANDOFF.md" {
                return Err(format!(
                    "ownerless agent cache contains authored-work marker {}",
                    path.display()
                ));
            }
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "cannot inspect ownerless agent cache entry {}: {error}",
                    path.display()
                )
            })?;
            if !metadata.is_dir() && !metadata.is_file() {
                return Err(format!(
                    "ownerless agent cache contains a special file: {}",
                    path.display()
                ));
            }
            names.push(name);
            if metadata.is_dir() {
                walk(&path, root)?;
            }
        }
        names.sort();
        if dir == root && names != ["validate"] {
            return Err("ownerless agent cache root contains an unexpected entry".to_string());
        }
        if dir == root.join("validate") && names != ["cache"] {
            return Err(
                "ownerless agent cache validate directory contains an unexpected entry".to_string(),
            );
        }
        if dir == root.join("validate").join("cache") && names != ["rust-script"] {
            return Err(
                "ownerless agent cache cache directory contains an unexpected entry".to_string(),
            );
        }
        Ok(())
    }
    walk(source, source)
}

/// Plan (or explicitly refuse) ownerless agent cache relocation.
pub(crate) fn recover_ownerless_agent_cache(
    root: Option<&Path>,
    machine_override: Option<&str>,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    if selected != short_hostname()? {
        return Err("ownerless agent cache recovery must run on the cache's machine".to_string());
    }
    if apply {
        if !coordinator_authorized {
            return Err(
                "ownerless agent cache recovery requires --coordinator-authorized with --apply"
                    .to_string(),
            );
        }
        let pid = coordinator_pid.ok_or("--apply requires --coordinator-pid")?;
        read_process_identity(pid)?;
    }
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    // Interrupted-journal outcomes (kind match only, as in Python's
    // read-only path; full journal validation gates the apply path).
    {
        let mut journals: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&cfg.control) {
            for entry in entries.flatten() {
                let text = entry.file_name().to_string_lossy().into_owned();
                if text.starts_with("ACTIVE.") && text.ends_with(".journal") {
                    journals.push(entry.path());
                }
            }
        }
        journals.sort();
        if let Some(first) = journals.first() {
            let text = std::fs::read_to_string(first)
                .map_err(|error| format!("cannot read recovery journal: {error}"))?;
            let raw: Value = serde_json::from_str(&text)
                .map_err(|error| format!("cannot read recovery journal: {error}"))?;
            if raw.get("kind").and_then(Value::as_str) != Some("ownerless-agent-cache-relocate") {
                return Err(format!(
                    "another interrupted mutation is recorded in {}",
                    first.display()
                ));
            }
            if raw.get("schema").and_then(Value::as_i64) != Some(2)
                || raw.get("machine").and_then(Value::as_str) != Some(selected.as_str())
                || raw.get("slot").and_then(Value::as_str) != Some("ignored")
            {
                return Err("ownerless agent cache journal is invalid".to_string());
            }
            if apply {
                return Err(
                    "resuming an interrupted ownerless agent cache recovery stays with the Python edition for now"
                        .to_string(),
                );
            }
            return Ok(json!({
                "machine": selected,
                "slot": "ignored",
                "outcome": "planned",
                "detail": "resume",
            }));
        }
    }
    validate_global(root, &cfg)?;
    let (source, destination) = cache_paths(&cfg);
    let active = active_document(root, &cfg, &selected)?;
    if let Some(slots) = active.get("slots").and_then(Value::as_array) {
        for record in slots {
            let slot = record
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if slot == "ignored" {
                return Err("ownerless path name collides with active slot ignored".to_string());
            }
            for checkout in record
                .get("checkouts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let recorded = cfg.root.join(
                    checkout
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                if recorded == source
                    || recorded.starts_with(&source)
                    || source.starts_with(&recorded)
                {
                    return Err(format!(
                        "ownerless path overlaps registered checkout {slot}/{}",
                        checkout
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }
    let archives = crate::recover_absent_write::archives_document(&cfg, &selected)?;
    if let Some(records) = archives.get("records").and_then(Value::as_array) {
        for record in records {
            if record.get("slot").and_then(Value::as_str) == Some("ignored") {
                return Err("ownerless path name collides with archived slot ignored".to_string());
            }
        }
    }
    if destination.exists() || destination.is_symlink() {
        return Err(format!(
            "ownerless agent cache destination already exists: {}",
            destination.display()
        ));
    }
    assert_cache_tree(&source)?;
    if apply {
        return Err(
            "ownerless agent cache recovery stays with the Python edition for now".to_string(),
        );
    }
    Ok(json!({
        "machine": selected,
        "slot": "ignored",
        "outcome": "planned",
        "detail": destination.to_string_lossy(),
    }))
}

fn batch_shape(
    command: &str,
    checkouts: &[String],
    frozen_checkouts: &[String],
    records: &[String],
    repositories: &[String],
    proofs: &[String],
) -> Result<bool> {
    if (!checkouts.is_empty()) == (!frozen_checkouts.is_empty()) {
        return Err(format!(
            "{command} requires exactly one of --checkout or --frozen-validate-checkout{}",
            if command == "recover-ownerless-validate-batch" {
                "; choose the matching validation storage kind and retry"
            } else {
                ""
            }
        ));
    }
    let frozen = !frozen_checkouts.is_empty();
    let selected = if frozen { frozen_checkouts } else { checkouts };
    if selected.len() != records.len() || records.len() != repositories.len() {
        return Err(format!(
            "{command} requires the same number of {} arguments",
            if command == "recover-ownerless-validate-batch" {
                "--checkout, --completed-record, and --repository"
            } else {
                "checkout, completed-record, and repository"
            }
        ));
    }
    if !proofs.is_empty() && proofs.len() != selected.len() {
        return Err(format!(
            "{command} requires {}",
            if command == "recover-ownerless-validate-batch" {
                "one --validation-proof-manifest for each checkout when any proof is supplied"
            } else {
                "one proof per checkout"
            }
        ));
    }
    if selected.len() > BATCH_LIMIT {
        return Err(format!("{command} accepts at most {BATCH_LIMIT} checkouts"));
    }
    let mut seen: Vec<&str> = Vec::new();
    for checkout in selected {
        if seen.contains(&checkout.as_str()) {
            return Err(format!(
                "{command} received {}",
                if command == "recover-ownerless-validate-batch" {
                    "the same checkout more than once"
                } else {
                    "a duplicate checkout"
                }
            ));
        }
        seen.push(checkout.as_str());
    }
    Ok(frozen)
}

/// Refuse ownerless validation batch recovery after its input contract.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_ownerless_validate_batch(
    root: Option<&Path>,
    machine_override: Option<&str>,
    checkouts: &[String],
    frozen_checkouts: &[String],
    records: &[String],
    repositories: &[String],
    proofs: &[String],
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    batch_shape(
        "recover-ownerless-validate-batch",
        checkouts,
        frozen_checkouts,
        records,
        repositories,
        proofs,
    )?;
    if !coordinator_authorized {
        return Err(
            "ownerless validation cleanup is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                .to_string(),
        );
    }
    let pid = coordinator_pid.ok_or("ownerless validation cleanup requires --coordinator-pid")?;
    read_process_identity(pid)?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let _ = (cfg, selected);
    Err("ownerless validation batch recovery stays with the Python edition for now".to_string())
}

/// Python `_cmd_classify_ownerless_validate_batch`: classify
/// retained validation checkouts without locks or writes.
/// Proof-manifest classification stays with the Python edition.
pub(crate) fn classify_ownerless_validate_batch(
    root: Option<&Path>,
    machine_override: Option<&str>,
    checkouts: &[String],
    frozen_checkouts: &[String],
    records: &[String],
    repositories: &[String],
    proofs: &[String],
) -> Result<Value> {
    require_fixture()?;
    let frozen = batch_shape(
        "classify-ownerless-validate-batch",
        checkouts,
        frozen_checkouts,
        records,
        repositories,
        proofs,
    )?;
    if !proofs.is_empty() {
        return Err(
            "ownerless validation batch proof classification stays with the Python edition for now"
                .to_string(),
        );
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let config = crate::config_validate::load_config(root, machine_override)?;
    crate::ensure_states::refuse_partial_state_allow_seals(&cfg.control)?;
    // Python `_validate_batch_seal_journals`: any seal journal
    // blocks admission before classification.
    if let Ok(entries) = std::fs::read_dir(&cfg.control) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("VALIDATE-BATCH-SEAL.") && name.ends_with(".journal") {
                return Err(
                    "an interrupted validation-batch seal is recorded; recover it before admission"
                        .to_string(),
                );
            }
        }
    }
    crate::hold_write::validate_global(root, &cfg)?;
    let before_rows = crate::validation_recovery::load_global_rows(&cfg)?;
    let before_paths = crate::classify_journals::outstanding_journal_paths(&cfg)?;
    let create_rows = crate::classify_journals::create_journal_rows(&cfg, &config)?;
    let target_kind = if frozen {
        "frozen-checkout"
    } else {
        "checkout"
    };
    let selected_checkouts: &[String] = if frozen { frozen_checkouts } else { checkouts };
    let mut results: Vec<Value> = Vec::new();
    type PreparedRow = (
        String,
        String,
        String,
        PathBuf,
        crate::validation_recovery::PrivateIdentity,
    );
    let mut prepared: Vec<PreparedRow> = Vec::new();
    for (index, checkout) in selected_checkouts.iter().enumerate() {
        let prepared_row = (|| -> Result<(PathBuf, (u64, u64, i64, String))> {
            let (_relative, target, _allowed) =
                crate::validation_recovery::validation_recovery_target(
                    &cfg,
                    checkout,
                    target_kind,
                )?;
            if create_rows.iter().any(|row| {
                crate::validation_recovery::path_is_within_pub(&target, &row.slot_path)
                    || crate::validation_recovery::path_is_within_pub(&row.slot_path, &target)
            }) {
                return Err("validation path overlaps an interrupted create".to_string());
            }
            let managed = if frozen {
                Some(crate::validation_recovery::frozen_validation_directory(
                    &cfg,
                ))
            } else {
                None
            };
            let identity = crate::validation_recovery::private_cleanup_path_identity(
                &cfg,
                &target,
                &[0o700],
                managed.as_deref(),
            )?;
            Ok((target, identity))
        })();
        match prepared_row {
            Ok((target, identity)) => prepared.push((
                checkout.clone(),
                records[index].clone(),
                repositories[index].clone(),
                target,
                identity,
            )),
            Err(error) => results.push(json!({
                "blocks_entry": true,
                "checkout": checkout,
                "reason": error,
                "state": "could-not-classify",
            })),
        }
    }
    let duplicate_identity = (0..prepared.len()).any(|index| {
        ((index + 1)..prepared.len()).any(|other| prepared[other].4 == prepared[index].4)
    });
    if duplicate_identity {
        return Err(
            "classify-ownerless-validate-batch resolved multiple checkouts to one directory"
                .to_string(),
        );
    }
    let census_count = i64::from(!prepared.is_empty());
    let snapshot = if prepared.is_empty() {
        None
    } else {
        let targets: Vec<PathBuf> = prepared.iter().map(|row| row.3.clone()).collect();
        match crate::remove_write::capture_process_snapshot(i64::from(std::process::id()), &targets)
        {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                results.extend(prepared.iter().map(|row| {
                    json!({
                        "blocks_entry": true,
                        "checkout": row.0,
                        "reason": error,
                        "state": "could-not-classify",
                    })
                }));
                prepared.clear();
                None
            }
        }
    };
    let batch = snapshot.map(|snapshot| {
        let pairs: Vec<((u64, u64, i64, String), PathBuf)> = prepared
            .iter()
            .map(|row| (row.4.clone(), row.3.clone()))
            .collect();
        crate::validation_recovery::BatchContext::new(&pairs, snapshot)
    });
    let actor_identity = crate::status_state::read_process_identity(i64::from(std::process::id()))
        .map(|identity| crate::status_state::identity_to_value(&identity))?;
    for (checkout, record, repository, target, _identity) in &prepared {
        let classified = (|| -> Result<Value> {
            if let Some(batch) = &batch {
                batch.assert_unused(&cfg, target, false)?;
            }
            let mut frozen_result: Option<Value> = None;
            if frozen {
                // The record file and its recorded target, as the
                // disposition inputs.
                let record_path = cfg.root.join(record);
                let contents = std::fs::read(&record_path).map_err(|error| {
                    format!(
                        "cannot read frozen validation run-record {}: {error}",
                        record_path.display()
                    )
                })?;
                let record_obj = crate::validation_recovery::strict_json_object_pub(
                    &contents,
                    "frozen validation run-record",
                )?;
                let record_target = record_obj
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let digest = {
                    use sha2::Digest as _;
                    format!("{:x}", sha2::Sha256::digest(&contents))
                };
                let repository_path =
                    crate::validation_recovery::repository_path(&cfg, repository, true)?.1;
                let (disposition, historical_schema, evidence) =
                    crate::validation_recovery::frozen_no_proof_disposition(
                        &cfg,
                        &record_path,
                        target,
                        &repository_path,
                        &digest,
                        &record_target,
                        &|path| {
                            crate::remove_write::assert_slot_unused(
                                path,
                                i64::from(std::process::id()),
                            )
                        },
                    )?;
                match disposition {
                    crate::validation_recovery::FrozenDisposition::CurrentIncompleteRetained => {
                        return Ok(json!({
                            "blocks_entry": false,
                            "checkout": checkout,
                            "reason": "current incomplete frozen validation is exact and inactive; the checkout and no-result remain retained",
                            "state": "current-incomplete-retained",
                        }));
                    }
                    crate::validation_recovery::FrozenDisposition::HistoricalNonblocking => {
                        return Ok(json!({
                            "blocks_entry": false,
                            "checkout": checkout,
                            "reason": format!(
                                "historical schema {} validation is exact and inactive; it remains retained",
                                historical_schema.unwrap_or_default()
                            ),
                            "state": "historical-retained",
                        }));
                    }
                    crate::validation_recovery::FrozenDisposition::BlocksEntry => {
                        return Err(
                            "frozen validation has no typed removal proof and cannot be classified as historical"
                                .to_string(),
                        );
                    }
                    _ => {
                        frozen_result = evidence;
                    }
                }
            }
            crate::validation_recovery::ownerless_validation_journal(
                &cfg,
                &config,
                target_kind,
                checkout,
                Some(record),
                Some(repository),
                frozen_result.as_ref(),
                &actor_identity,
            )?;
            Ok(json!({
                "blocks_entry": false,
                "checkout": checkout,
                "reason": "terminal validation is exact and unused; read-only classification left it retained",
                "state": "terminal-retained",
            }))
        })();
        match classified {
            Ok(row) => results.push(row),
            Err(error) => results.push(json!({
                "blocks_entry": true,
                "checkout": checkout,
                "reason": error,
                "state": "could-not-classify",
            })),
        }
    }
    let after_rows = crate::validation_recovery::load_global_rows(&cfg)?;
    if before_rows != after_rows {
        return Err("active registry changed during validation classification".to_string());
    }
    let after_paths = crate::classify_journals::outstanding_journal_paths(&cfg)?;
    if before_paths != after_paths
        || create_rows
            .iter()
            .map(|row| row.journal.clone())
            .collect::<Vec<_>>()
            != after_paths
                .iter()
                .filter(|path| create_rows.iter().any(|row| &row.journal == *path))
                .cloned()
                .collect::<Vec<_>>()
    {
        return Err("recovery journal set changed before validation classification".to_string());
    }
    // Re-classify to prove the create rows are stable.
    let stable_rows = crate::classify_journals::create_journal_rows(&cfg, &config)?;
    if stable_rows.iter().map(|row| &row.obj).collect::<Vec<_>>()
        != create_rows.iter().map(|row| &row.obj).collect::<Vec<_>>()
    {
        return Err("create journal changed while it was classified".to_string());
    }
    Ok(json!({
        "classifications": results,
        "create_journals": create_rows.iter().map(|row| row.obj.clone()).collect::<Vec<_>>(),
        "process_censuses": census_count,
        "requested": selected_checkouts.len(),
        "schema": 1,
    }))
}
