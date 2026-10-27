//! Recovery-journal shape validation parity for create/import/finish.
//!
//! Reproduces the existing journal shape validator (which inspects no
//! journal targets) for the three primary journal kinds: exact keys,
//! identity and filename agreement, planned/created checkout shapes for
//! create, the embedded record for import and finish, finish phase and
//! archive-id derivation, and the finish journal's optional evidence
//! bundles. Other journal kinds still refuse here as unsupported.
//! Read-only; provenance against append-only evidence is not checked.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use sha2::{Digest as _, Sha256};

use crate::active_state::load_cfg;
use crate::active_state::{
    as_int, as_list, as_mapping, as_str, checkout_from_obj, exact_keys, identity_from_obj,
    is_digest, is_sha, parse_timestamp, record_from_obj, scope_from_obj, validate_ref, Cfg,
};
use crate::config_validate::{
    refuse_symlink, validate_full_ref, validate_hook, validate_name, validate_remote,
};

type Result<T> = std::result::Result<T, String>;

const SCHEMA: i64 = 2;

fn planned_from_obj(value: &Value, label: &str) -> Result<()> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "name",
            "destination",
            "repository",
            "branch",
            "start_point",
            "remote",
            "remote_url_sha256",
            "landed_ref",
        ],
        &[],
        label,
    )?;
    let name = as_str(&raw["name"], &format!("{label}.name"))?;
    as_str(&raw["destination"], &format!("{label}.destination"))?;
    as_str(&raw["repository"], &format!("{label}.repository"))?;
    let branch = as_str(&raw["branch"], &format!("{label}.branch"))?;
    let start_point = as_str(&raw["start_point"], &format!("{label}.start_point"))?;
    let remote = as_str(&raw["remote"], &format!("{label}.remote"))?;
    let digest = as_str(
        &raw["remote_url_sha256"],
        &format!("{label}.remote_url_sha256"),
    )?;
    let landed_ref = as_str(&raw["landed_ref"], &format!("{label}.landed_ref"))?;
    validate_name(name, "checkout name")?;
    validate_remote(remote)?;
    if !is_digest(digest) {
        return Err(format!(
            "{label}.remote_url_sha256 must be a lowercase SHA-256"
        ));
    }
    validate_ref(branch, "branch")?;
    if !is_sha(start_point) {
        return Err(format!(
            "{label}.start_point must be a full lowercase Git commit"
        ));
    }
    validate_full_ref(landed_ref, "landed ref")?;
    Ok(())
}

fn create_shape(raw: &Map<String, Value>) -> Result<()> {
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "slot",
            "agent",
            "task",
            "purpose",
            "created_at",
            "heartbeat_at",
            "heartbeat_ttl_seconds",
            "owner",
            "coordinator_lease",
            "planned",
            "created",
        ],
        &[
            "slot_type",
            "task_scope",
            "post_provision_hooks",
            "hook_progress",
            "hook_failure",
            "failure_policy",
            "operation_id",
        ],
        "create journal",
    )?;
    validate_name(as_str(&raw["agent"], "create journal.agent")?, "agent")?;
    if as_str(&raw["task"], "create journal.task")?.is_empty()
        || as_str(&raw["purpose"], "create journal.purpose")?.is_empty()
    {
        return Err("create journal has an empty task or purpose".to_owned());
    }
    parse_timestamp(
        as_str(&raw["created_at"], "create journal.created_at")?,
        "create journal.created_at",
    )?;
    parse_timestamp(
        as_str(&raw["heartbeat_at"], "create journal.heartbeat_at")?,
        "create journal.heartbeat_at",
    )?;
    as_int(
        &raw["heartbeat_ttl_seconds"],
        "create journal.heartbeat_ttl_seconds",
        1,
    )?;
    let owner = identity_from_obj(&raw["owner"], "create journal.owner")?;
    let task_scope = match raw.get("task_scope") {
        None => None,
        Some(value) => Some(scope_from_obj(value, "create journal.task_scope")?),
    };
    if let Some(scope) = &task_scope {
        let owner = owner
            .as_ref()
            .ok_or_else(|| "create journal.task_scope has no bound owner process".to_owned())?;
        if scope.leader_pid != owner.pid
            || scope.leader_start_ticks != owner.start_ticks
            || scope.boot_id != owner.boot_id
            || scope.cgroup_path != owner.cgroup_path
        {
            return Err(
                "create journal.task_scope does not match its bound owner process".to_owned(),
            );
        }
    }
    if identity_from_obj(
        &raw["coordinator_lease"],
        "create journal.coordinator_lease",
    )?
    .is_none()
    {
        return Err("create journal has no coordinator lease".to_owned());
    }
    let planned = as_list(&raw["planned"], "create journal.planned")?;
    if planned.is_empty() {
        return Err("create journal contains no planned checkouts".to_owned());
    }
    for (index, item) in planned.iter().enumerate() {
        planned_from_obj(item, &format!("create journal.planned[{index}]"))?;
    }
    for (index, item) in as_list(&raw["created"], "create journal.created")?
        .iter()
        .enumerate()
    {
        checkout_from_obj(item, &format!("create journal.created[{index}]"))?;
    }
    let slot_type = match raw.get("slot_type") {
        None => "agent",
        Some(value) => as_str(value, "create journal.slot_type")?,
    };
    if !matches!(slot_type, "agent" | "validate") {
        return Err("create journal has an invalid slot type".to_owned());
    }
    if let Some(value) = raw.get("hook_progress") {
        as_int(value, "create journal.hook_progress", 0)?;
    }
    if let Some(hooks) = raw.get("post_provision_hooks") {
        for (index, hook) in as_list(hooks, "create journal.post_provision_hooks")?
            .iter()
            .enumerate()
        {
            validate_hook(as_str(
                hook,
                &format!("create journal.post_provision_hooks[{index}]"),
            )?)?;
        }
    }
    if let Some(failure) = raw.get("hook_failure") {
        if !failure.is_null() {
            as_mapping(failure, "create journal.hook_failure")?;
        }
    }
    if let Some(policy) = raw.get("failure_policy") {
        as_str(policy, "create journal.failure_policy")?;
    }
    if let Some(operation) = raw.get("operation_id") {
        let operation = as_str(operation, "create journal.operation_id")?;
        if !(operation.len() == 32
            && operation
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        {
            return Err("create journal has an invalid operation_id".to_owned());
        }
    }
    Ok(())
}

fn regular_identity(raw: &Map<String, Value>, label: &str) -> Result<()> {
    let parts = as_list(
        raw.get("identity").unwrap_or(&Value::Null),
        &format!("{label}.identity"),
    )?;
    if parts.len() != 2 {
        return Err(format!("{label}.identity must contain device and inode"));
    }
    let digest = as_str(
        raw.get("sha256").unwrap_or(&Value::Null),
        &format!("{label}.sha256"),
    )?;
    if !is_digest(digest) {
        return Err(format!(
            "{label}.sha256 must be 64 lowercase hexadecimal digits"
        ));
    }
    as_int(&parts[0], &format!("{label}.identity[0]"), 0)?;
    as_int(&parts[1], &format!("{label}.identity[1]"), 1)?;
    as_int(
        raw.get("size").unwrap_or(&Value::Null),
        &format!("{label}.size"),
        0,
    )?;
    Ok(())
}

fn removal_proof(value: &Value, label: &str) -> Result<(String, i64)> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "manifest",
            "schema",
            "kind",
            "slot",
            "generation",
            "checkout",
            "artifacts",
        ],
        &[],
        label,
    )?;
    let manifest = as_mapping(&raw["manifest"], &format!("{label}.manifest"))?;
    exact_keys(
        manifest,
        &["path", "identity", "size", "sha256"],
        &[],
        &format!("{label}.manifest"),
    )?;
    if as_int(&raw["schema"], &format!("{label}.schema"), 0)? != 1 {
        return Err("unsupported validation removal proof schema".to_owned());
    }
    if as_str(&raw["kind"], &format!("{label}.kind"))? != "validation-removal-proof" {
        return Err("validation removal proof has the wrong kind".to_owned());
    }
    let slot = validate_name(as_str(&raw["slot"], &format!("{label}.slot"))?, "slot")?;
    let generation = as_int(&raw["generation"], &format!("{label}.generation"), 1)?;
    as_str(&raw["checkout"], &format!("{label}.checkout"))?;
    let artifacts_label = format!("{label}.artifacts");
    let mut roles: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for (index, item) in as_list(&raw["artifacts"], &artifacts_label)?
        .iter()
        .enumerate()
    {
        let row_label = format!("{artifacts_label}[{index}]");
        let row = as_mapping(item, &row_label)?;
        exact_keys(
            row,
            &["role", "path", "identity", "size", "sha256"],
            &[],
            &row_label,
        )?;
        roles.push(as_str(&row["role"], &format!("{row_label}.role"))?.to_owned());
        paths.push(as_str(&row["path"], &format!("{row_label}.path"))?.to_owned());
        regular_identity(row, &row_label)?;
    }
    let mut unique = roles.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != roles.len() {
        return Err("validation removal proof repeats an artifact role".to_owned());
    }
    let mut sorted = roles.clone();
    sorted.sort_unstable();
    if sorted
        != [
            "producer-schema",
            "run-record",
            "scorecard-handoff",
            "service-result",
        ]
    {
        return Err(
            "validation removal proof must name exactly run-record, service-result, producer-schema, and scorecard-handoff artifacts"
                .to_owned(),
        );
    }
    let mut unique_paths = paths.clone();
    unique_paths.sort_unstable();
    unique_paths.dedup();
    if unique_paths.len() != paths.len() {
        return Err("validation removal proof repeats an artifact path".to_owned());
    }
    as_str(&manifest["path"], &format!("{label}.manifest.path"))?;
    regular_identity(manifest, &format!("{label}.manifest"))?;
    Ok((slot, generation))
}

fn nested_evidence(value: &Value, label: &str) -> Result<()> {
    const REQUIRED: [&str; 6] = [
        "checkout",
        "path",
        "head",
        "remote_ref",
        "ignored_entries",
        "ignored_sample",
    ];
    const SALVAGE: [&str; 5] = [
        "salvage_bundle",
        "salvage_bundle_sha256",
        "salvage_receipt",
        "salvage_commit",
        "salvage_reason",
    ];
    for (index, item) in as_list(value, label)?.iter().enumerate() {
        let item_label = format!("{label}[{index}]");
        let row = as_mapping(item, &item_label)?;
        let mut optional: Vec<&str> = SALVAGE.to_vec();
        optional.push("contents_judged");
        exact_keys(row, &REQUIRED, &optional, &item_label)?;
        let salvage_present: Vec<&str> = SALVAGE
            .iter()
            .filter(|key| row.contains_key(**key))
            .copied()
            .collect();
        if !salvage_present.is_empty() && salvage_present.len() != SALVAGE.len() {
            return Err(format!("{item_label} records an incomplete salvage bundle"));
        }
        if let Some(judged) = row.get("contents_judged") {
            if judged != &Value::Bool(false) || !salvage_present.is_empty() {
                return Err(format!(
                    "{item_label}.contents_judged must be false and only on a repository that was not salvaged"
                ));
            }
        }
        for key in &salvage_present {
            if *key == "salvage_commit" && row[*key].is_null() {
                continue;
            }
            as_str(&row[*key], &format!("{item_label}.{key}"))?;
        }
        as_str(&row["checkout"], &format!("{item_label}.checkout"))?;
        as_str(&row["path"], &format!("{item_label}.path"))?;
        for key in ["head", "remote_ref"] {
            if !row[key].is_null() {
                as_str(&row[key], &format!("{item_label}.{key}"))?;
            }
        }
        as_int(
            &row["ignored_entries"],
            &format!("{item_label}.ignored_entries"),
            0,
        )?;
        for (sample_index, sample) in as_list(
            &row["ignored_sample"],
            &format!("{item_label}.ignored_sample"),
        )?
        .iter()
        .enumerate()
        {
            as_str(
                sample,
                &format!("{item_label}.ignored_sample[{sample_index}]"),
            )?;
        }
    }
    Ok(())
}

fn private_census(value: &Value, label: &str) -> Result<()> {
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
    let handle = as_str(&values[3], &format!("{label}[3]"))?;
    if handle.is_empty()
        || handle.len() > 256
        || handle.len() % 2 != 0
        || !handle
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(format!(
            "{label}[3] must be bounded lowercase hexadecimal bytes"
        ));
    }
    as_int(&values[0], &format!("{label}[0]"), 1)?;
    as_int(&values[1], &format!("{label}[1]"), 1)?;
    as_int(&values[2], &format!("{label}[2]"), 0)?;
    Ok(())
}

fn finish_shape(
    raw: &Map<String, Value>,
    machine: &str,
    slot: &str,
    is_finish_path: bool,
) -> Result<()> {
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "slot",
            "mode",
            "actor",
            "finished_at",
            "archive_id",
            "phase",
            "fenced",
            "removed",
            "record",
        ],
        &[
            "salvage",
            "validate_complete",
            "original_mode",
            "private_census_identity",
            "validation_removal_proof",
            "nested_git_evidence",
            "archive_nested_cache_repositories",
        ],
        "finish journal",
    )?;
    let record = record_from_obj(&raw["record"], "finish journal.record")?;
    if record.machine != machine || record.slot != slot {
        return Err("finish journal record does not match its shard or slot".to_owned());
    }
    if is_finish_path
        && (record.slot_type != "validate"
            || raw.get("validate_complete") != Some(&Value::Bool(true))
            || raw.contains_key("validation_removal_proof"))
    {
        return Err(
            "scoped finish journal is not a direct completed-validation removal".to_owned(),
        );
    }
    if as_str(&raw["mode"], "finish journal.mode")? != "remove"
        || as_str(&raw["actor"], "finish journal.actor")? != "coordinator"
    {
        return Err("finish journal actor does not match its operation".to_owned());
    }
    let finished_at = as_str(&raw["finished_at"], "finish journal.finished_at")?;
    parse_timestamp(finished_at, "finish journal.finished_at")?;
    let expected = format!(
        "{machine}:{}:{}:{}",
        record.slot, record.generation, finished_at
    );
    if as_str(&raw["archive_id"], "finish journal.archive_id")? != expected {
        return Err("finish journal archive_id does not match its record".to_owned());
    }
    let phase = as_str(&raw["phase"], "finish journal.phase")?;
    if !matches!(phase, "prepared" | "fenced" | "removed") {
        return Err(format!("unknown finish journal phase {phase:?}"));
    }
    as_str(&raw["fenced"], "finish journal.fenced")?;
    for (index, item) in as_list(&raw["removed"], "finish journal.removed")?
        .iter()
        .enumerate()
    {
        as_str(item, &format!("finish journal.removed[{index}]"))?;
    }
    if let Some(salvage) = raw.get("salvage") {
        for (index, item) in as_list(salvage, "finish journal.salvage")?
            .iter()
            .enumerate()
        {
            as_mapping(item, &format!("finish journal.salvage[{index}]"))?;
        }
    }
    if let Some(complete) = raw.get("validate_complete") {
        if !complete.is_boolean() {
            return Err("finish journal.validate_complete must be boolean".to_owned());
        }
    }
    if let Some(flag) = raw.get("archive_nested_cache_repositories") {
        if flag != &Value::Bool(true) || record.slot_type != "agent" {
            return Err(
                "finish journal.archive_nested_cache_repositories must be true and only for an agent slot"
                    .to_owned(),
            );
        }
    }
    if let Some(evidence) = raw.get("nested_git_evidence") {
        nested_evidence(evidence, "finish journal.nested_git_evidence")?;
        if record.slot_type != "agent" {
            return Err(
                "finish journal records nested Git evidence for a non-agent slot".to_owned(),
            );
        }
    }
    if let Some(mode) = raw.get("original_mode") {
        if as_int(mode, "finish journal.original_mode", 0)? > 0o7777 {
            return Err("finish journal.original_mode exceeds permission bits".to_owned());
        }
    }
    if let Some(identity) = raw.get("private_census_identity") {
        private_census(identity, "finish journal.private_census_identity")?;
        if !raw.contains_key("original_mode") {
            return Err("finish journal private census identity has no original mode".to_owned());
        }
    }
    if let Some(proof) = raw.get("validation_removal_proof") {
        let (proof_slot, proof_generation) =
            removal_proof(proof, "finish journal.validation_removal_proof")?;
        if record.slot_type != "validate"
            || record.checkouts.len() != 1
            || proof_slot != record.slot
            || proof_generation != record.generation
            || !raw.contains_key("private_census_identity")
        {
            return Err(
                "finish journal validation removal proof does not match one privately sealed validation checkout"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

fn validation_authorization(value: &Value) -> Result<(String, String)> {
    let label = "validation recovery authorization";
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "target_kind",
            "path",
            "identity",
            "actor",
            "evidence",
            "no_authored_work",
            "no_live_use",
            "repository",
            "head",
            "remote_url_sha256",
            "parent_identity",
        ],
        &[
            "stable_identity",
            "removal_proof",
            "proof_original_mode",
            "proof_private_identity",
        ],
        label,
    )?;
    let actor = identity_from_obj(&raw["actor"], "validation recovery actor")?;
    if actor.is_none()
        || raw["no_authored_work"] != Value::Bool(true)
        || raw["no_live_use"] != Value::Bool(true)
    {
        return Err("validation recovery authorization is incomplete".to_owned());
    }
    let identity = as_list(&raw["identity"], "validation path identity")?;
    for item in identity {
        as_int(item, "validation path identity", 0)?;
    }
    let parent_len = match &raw["parent_identity"] {
        Value::Null => None,
        value => {
            let items = as_list(value, "validation parent identity")?;
            for item in items {
                as_int(item, "validation parent identity", 0)?;
            }
            Some(items.len())
        }
    };
    if identity.len() != 3 || parent_len.is_some_and(|len| len != 3) {
        return Err("validation recovery directory identity must have three fields".to_owned());
    }
    if let Some(stable) = raw.get("stable_identity") {
        if !stable.is_null() {
            private_census(stable, "validation stable path identity")?;
        }
    }
    let has_proof = match raw.get("removal_proof") {
        None | Some(Value::Null) => false,
        Some(proof) => {
            removal_proof(proof, "validation recovery removal proof")?;
            true
        }
    };
    let proof_mode = match raw.get("proof_original_mode") {
        None | Some(Value::Null) => None,
        Some(value) => Some(as_int(value, "validation recovery proof original mode", 0)?),
    };
    if proof_mode.is_some_and(|mode| mode > 0o7777) {
        return Err("validation recovery proof original mode exceeds permission bits".to_owned());
    }
    let has_proof_identity = match raw.get("proof_private_identity") {
        None | Some(Value::Null) => false,
        Some(value) => {
            private_census(value, "validation recovery proof private identity")?;
            true
        }
    };
    if !has_proof && (proof_mode.is_some() || has_proof_identity) {
        return Err("validation recovery has a proof seal without a proof".to_owned());
    }
    if proof_mode.is_some() != has_proof_identity {
        return Err("validation recovery proof seal is incomplete".to_owned());
    }
    let target_kind = as_str(&raw["target_kind"], "validation target kind")?.to_owned();
    let path = as_str(&raw["path"], "validation path")?.to_owned();
    as_mapping(&raw["evidence"], "validation recovery evidence")?;
    for (key, field_label) in [
        ("repository", "validation repository"),
        ("head", "validation HEAD"),
        ("remote_url_sha256", "validation remote digest"),
    ] {
        if !raw[key].is_null() {
            as_str(&raw[key], field_label)?;
        }
    }
    Ok((target_kind, path))
}

fn managed_worktrees_root(cfg: &Cfg) -> PathBuf {
    if cfg
        .worktrees
        .file_name()
        .is_some_and(|name| name == "slots")
    {
        return cfg
            .worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
    }
    cfg.worktrees.clone()
}

fn normpath_absolute(raw: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

fn frozen_directory(cfg: &Cfg) -> PathBuf {
    let root_name = cfg
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    cfg.root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join(format!(".{root_name}-frozen-validate"))
}

fn recovery_target(cfg: &Cfg, raw_path: &str, target_kind: &str) -> Result<PathBuf> {
    if target_kind == "frozen-checkout" {
        if !raw_path.starts_with('/') {
            return Err(
                "frozen validation checkout is not an absolute path; pass the exact absolute checkout path and retry"
                    .to_owned(),
            );
        }
        if raw_path.split('/').any(|part| part == "..") {
            return Err(
                "frozen validation checkout path contains '..'; pass its normalized exact absolute path and retry"
                    .to_owned(),
            );
        }
        let canonical = normpath_absolute(raw_path);
        if raw_path != canonical {
            return Err(format!(
                "frozen validation checkout must use its exact canonical absolute spelling; pass {canonical:?} and retry"
            ));
        }
        let target = PathBuf::from(&canonical);
        let frozen_root = frozen_directory(cfg);
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if target.parent() != Some(frozen_root.as_path()) || !name.starts_with("validate-fresh-") {
            return Err(format!(
                "frozen checkout path is outside the exact sibling validation directory: {}; preserve it or move it under {}/validate-fresh-* before retrying",
                target.display(),
                frozen_root.display()
            ));
        }
        validate_name(&name, "validation recovery path name")?;
        let allowed_root = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
        crate::config_validate::ensure_no_symlink_components(
            &allowed_root,
            &target,
            "frozen validation recovery path",
        )?;
        return Ok(target);
    }
    let (_relative, target) =
        crate::config_validate::relative_inside(&cfg.root, raw_path, "validation recovery path")?;
    let validate_root = managed_worktrees_root(cfg).join("validate");
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let allowed = match target_kind {
        "checkout" => {
            parent == validate_root
                || (parent == cfg.root.join("ignored") && name.starts_with("validate-fresh-"))
        }
        "cargo-home" => {
            name.starts_with("validate-cargo-")
                && (parent == validate_root
                    || parent
                        == cfg
                            .root
                            .join("ignored")
                            .join("validate")
                            .join("cargo-homes"))
        }
        _ => {
            return Err(format!(
                "unknown validation recovery target kind {target_kind:?}"
            ))
        }
    };
    if !allowed {
        return Err(format!(
            "{target_kind} path is outside the exact validation directories: {}",
            target.display()
        ));
    }
    validate_name(&name, "validation recovery path name")?;
    Ok(target)
}

fn directory_identity(value: &Value, label: &str) -> Result<()> {
    let fields = as_list(value, label)?;
    if fields.len() != 3 {
        return Err(format!("{label} must contain device, inode, and mount id"));
    }
    for (index, item) in fields.iter().enumerate() {
        as_int(item, &format!("{label}[{index}]"), 1)?;
    }
    Ok(())
}

fn exclusion_name(cfg: &Cfg, fenced: &Path) -> Result<String> {
    let name = fenced
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let token = name.rsplit('.').next().unwrap_or_default();
    if !(token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err("fenced validation path has no exclusion identity".to_owned());
    }
    let mut bytes = cfg.root.to_string_lossy().into_owned().into_bytes();
    bytes.push(0);
    bytes.extend(fenced.to_string_lossy().into_owned().into_bytes());
    let context = format!("{:x}", Sha256::digest(&bytes));
    Ok(format!(
        ".wrkslots-ownerless-validation.{token}.{}",
        &context[..16]
    ))
}

fn exclusion_from_obj(cfg: &Cfg, fenced: &Path, value: &Value) -> Result<()> {
    let label = "ownerless validation exclusion";
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "schema",
            "phase",
            "root",
            "root_identity",
            "guard",
            "guard_identity",
            "source_alias",
            "payload_identity",
        ],
        &[],
        label,
    )?;
    if as_int(&raw["schema"], &format!("{label}.schema"), 0)? != 1 {
        return Err("ownerless validation exclusion schema is unsupported".to_owned());
    }
    let phase = as_str(&raw["phase"], &format!("{label}.phase"))?;
    if !matches!(phase, "guard-created" | "payload-moved") {
        return Err(format!(
            "unknown ownerless validation exclusion phase {phase:?}"
        ));
    }
    let root = as_str(&raw["root"], &format!("{label}.root"))?;
    let guard = as_str(&raw["guard"], &format!("{label}.guard"))?;
    let source_alias = as_str(&raw["source_alias"], &format!("{label}.source_alias"))?;
    if root != "/data/misc" || !root.starts_with('/') {
        return Err("ownerless validation exclusion paths are not canonical".to_owned());
    }
    let expected_guard = PathBuf::from(root).join(exclusion_name(cfg, fenced)?);
    if guard != expected_guard.to_string_lossy()
        || !source_alias.starts_with('/')
        || source_alias.split('/').any(|part| part == "..")
        || [root, guard, source_alias]
            .iter()
            .any(|path| path.contains(['\n', '\0']))
    {
        return Err("ownerless validation exclusion paths are not canonical".to_owned());
    }
    let payload = as_list(
        &raw["payload_identity"],
        &format!("{label}.payload_identity"),
    )?;
    if payload.len() != 2 {
        return Err(format!(
            "{label} payload identity must contain device and inode"
        ));
    }
    for (index, item) in payload.iter().enumerate() {
        as_int(item, &format!("{label}.payload_identity[{index}]"), 1)?;
    }
    directory_identity(&raw["root_identity"], &format!("{label}.root_identity"))?;
    directory_identity(&raw["guard_identity"], &format!("{label}.guard_identity"))?;
    Ok(())
}

struct AgentAuthorization {
    path: String,
    identity: (i64, i64, i64),
    repository: String,
}

fn ownerless_agent_authorization(value: &Value) -> Result<AgentAuthorization> {
    let label = "ownerless agent authorization";
    let raw = as_mapping(value, label)?;
    const REQUIRED: [&str; 10] = [
        "path",
        "identity",
        "actor",
        "repository",
        "head",
        "branch",
        "remote",
        "remote_url_sha256",
        "handoff_sha256",
        "recorded_at",
    ];
    let mut keys: Vec<&str> = raw.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut pre_handoff: Vec<&str> = REQUIRED
        .iter()
        .filter(|key| **key != "handoff_sha256")
        .copied()
        .collect();
    pre_handoff.sort_unstable();
    let mut owned: Map<String, Value>;
    let raw = if keys == pre_handoff {
        owned = raw.clone();
        owned.insert("handoff_sha256".to_owned(), Value::Null);
        &owned
    } else {
        raw
    };
    exact_keys(raw, &REQUIRED, &[], label)?;
    let mut identity = Vec::new();
    for item in as_list(&raw["identity"], "ownerless agent directory identity")? {
        identity.push(as_int(item, "ownerless agent directory identity", 0)?);
    }
    if identity.len() != 3 {
        return Err("ownerless agent directory identity must have three fields".to_owned());
    }
    if identity_from_obj(&raw["actor"], "ownerless agent recovery actor")?.is_none() {
        return Err("ownerless agent recovery has no actor".to_owned());
    }
    let head = as_str(&raw["head"], "ownerless agent HEAD")?;
    let digest = as_str(&raw["remote_url_sha256"], "ownerless agent remote digest")?;
    if !is_sha(head) || !is_digest(digest) {
        return Err("ownerless agent Git identity is invalid".to_owned());
    }
    if !raw["handoff_sha256"].is_null() {
        let handoff = as_str(&raw["handoff_sha256"], "ownerless agent HANDOFF.md digest")?;
        if !is_digest(handoff) {
            return Err("ownerless agent HANDOFF.md digest is invalid".to_owned());
        }
    }
    let recorded_at = as_str(&raw["recorded_at"], "ownerless agent recorded_at")?;
    parse_timestamp(recorded_at, "ownerless agent recorded_at")?;
    let path = as_str(&raw["path"], "ownerless agent path")?.to_owned();
    let repository = as_str(&raw["repository"], "ownerless agent repository")?.to_owned();
    if !raw["branch"].is_null() {
        validate_ref(as_str(&raw["branch"], "ownerless agent branch")?, "branch")?;
    }
    validate_remote(as_str(&raw["remote"], "ownerless agent remote")?)?;
    Ok(AgentAuthorization {
        path,
        identity: (identity[0], identity[1], identity[2]),
        repository,
    })
}

fn open_directory_identity(path: &Path, label: &str) -> Result<(i64, i64, i64)> {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::io::AsRawFd as _;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot safely open {label} {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot safely open {label} {}: {error}", path.display()))?;
    let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
        .map_err(|error| format!("cannot determine mount identity for {label}: {error}"))?;
    for line in fdinfo.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        if key == "mnt_id" {
            let value: i64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("invalid mount identity for {label}: {raw:?}"))?;
            if value > 0 {
                return Ok((metadata.dev() as i64, metadata.ino() as i64, value));
            }
        }
    }
    Err(format!(
        "cannot determine mount identity for {label}: mnt_id is absent"
    ))
}

fn repository_resolved(cfg: &Cfg, raw: &str) -> Result<PathBuf> {
    if raw.starts_with('/') {
        return Err(format!(
            "repository path must be relative to {}: {raw:?}; use a path inside the project root or ../NAME[/PATH] components inside one direct sibling",
            cfg.root.display()
        ));
    }
    let parts: Vec<&str> = raw
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let sibling = parts.len() >= 2
        && parts[0] == ".."
        && parts[1..].iter().all(|part| !matches!(*part, "." | ".."));
    if parts.contains(&"..") && !sibling {
        return Err(format!(
            "repository path must stay inside {} or one direct sibling, with path components '../NAME' or '../NAME/PATH'; other parent traversal is refused: {raw:?}",
            cfg.root.display()
        ));
    }
    let root_parent = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
    let (stored, unresolved, allowed_root) = if sibling {
        if root_parent.join(parts[1]) == cfg.root {
            return Err(format!(
                "repository path {raw:?} aliases the project root; use a path inside it instead"
            ));
        }
        (
            crate::config_validate::normpath(raw),
            root_parent.join(parts[1..].join("/")),
            root_parent.clone(),
        )
    } else {
        let lexical = crate::config_validate::normpath(raw);
        let unresolved = if lexical == "." {
            cfg.root.clone()
        } else {
            cfg.root.join(&lexical)
        };
        (lexical, unresolved, cfg.root.clone())
    };
    crate::config_validate::ensure_no_symlink_components(&allowed_root, &unresolved, "repository")?;
    let absolute = unresolved.canonicalize().map_err(|_| {
        format!(
            "source repository {stored:?} does not exist or cannot be resolved: {}",
            unresolved.display()
        )
    })?;
    if !absolute.is_dir() {
        return Err(format!(
            "repository does not exist or is unsafe: {}",
            absolute.display()
        ));
    }
    Ok(absolute)
}

fn ownerless_agent_shape(cfg: &Cfg, raw: &Map<String, Value>, journal_machine: &str) -> Result<()> {
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "slot",
            "phase",
            "fenced",
            "authorization",
            "salvage",
        ],
        &[],
        "ownerless agent journal",
    )?;
    if as_int(&raw["schema"], "ownerless agent journal.schema", 0)? != SCHEMA
        || raw["kind"] != Value::String("ownerless-agent-remove".to_owned())
        || raw["machine"].as_str() != Some(journal_machine)
    {
        return Err("ownerless agent journal identity is invalid".to_owned());
    }
    let authorization = ownerless_agent_authorization(&raw["authorization"])?;
    let (_relative, target) = crate::config_validate::relative_inside(
        &cfg.root,
        &authorization.path,
        "ownerless agent worktree",
    )?;
    let target_name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if target.parent() != Some(cfg.worktrees.as_path()) || target_name == "ignored" {
        return Err(format!(
            "ownerless agent worktree must be one direct child of {}, excluding ignored",
            cfg.worktrees.display()
        ));
    }
    validate_name(&target_name, "ownerless agent worktree name")?;
    if raw["slot"].as_str() != Some(target_name.as_str()) {
        return Err("ownerless agent journal slot differs from its path".to_owned());
    }
    let (_relative, fenced) = crate::config_validate::relative_inside(
        &cfg.root,
        as_str(&raw["fenced"], "ownerless agent fenced path")?,
        "fenced path",
    )?;
    let fenced_name = fenced
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!(".{target_name}.ownerless-agent.");
    let tail = fenced_name.strip_prefix(&prefix).unwrap_or_default();
    if fenced.parent() != target.parent()
        || !fenced_name.starts_with(&prefix)
        || !(tail.len() == 32
            && tail
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err("ownerless agent fenced path does not match its target".to_owned());
    }
    let phase = as_str(&raw["phase"], "ownerless agent journal.phase")?;
    if !matches!(phase, "prepared" | "preserved" | "fenced" | "removed") {
        return Err(format!("unknown ownerless agent phase {phase:?}"));
    }
    let present = |path: &Path| path.symlink_metadata().is_ok();
    let target_present = present(&target);
    let fenced_present = present(&fenced);
    if target_present && fenced_present {
        return Err("canonical and fenced ownerless agent paths both exist".to_owned());
    }
    let active = if target_present {
        Some(target.clone())
    } else if fenced_present {
        Some(fenced.clone())
    } else {
        None
    };
    if phase == "removed" && active.is_some() {
        return Err("ownerless agent worktree reappeared after removal was recorded".to_owned());
    }
    if let Some(active) = &active {
        if active.is_symlink()
            || !active.is_dir()
            || open_directory_identity(active, "ownerless agent worktree")?
                != authorization.identity
        {
            return Err("ownerless agent worktree identity changed; preserve it".to_owned());
        }
    }
    let repository = repository_resolved(cfg, &authorization.repository)?;
    if repository == target || repository.starts_with(&target) {
        return Err("ownerless agent source repository is inside the removal target".to_owned());
    }
    for (index, item) in as_list(&raw["salvage"], "ownerless agent journal.salvage")?
        .iter()
        .enumerate()
    {
        as_mapping(item, &format!("ownerless agent journal.salvage[{index}]"))?;
    }
    Ok(())
}

fn absent_validate_shape(raw: &Map<String, Value>, journal_machine: &str) -> Result<()> {
    const LABEL: &str = "absent-validation-row journal";
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "host_id",
            "slot",
            "input_sha256",
            "cursor",
            "actor",
            "items",
        ],
        &[],
        LABEL,
    )?;
    if as_int(&raw["schema"], &format!("{LABEL}.schema"), 0)? != SCHEMA
        || raw["kind"] != Value::String("recover-absent-validate-rows".to_owned())
        || raw["machine"].as_str() != Some(journal_machine)
        || raw["slot"].as_str() != Some("absent-validate-rows")
    {
        return Err(format!("{LABEL} identity is invalid"));
    }
    let input_digest = as_str(&raw["input_sha256"], &format!("{LABEL}.input_sha256"))?;
    if !is_digest(input_digest) {
        return Err(format!("{LABEL} input digest is invalid"));
    }
    if identity_from_obj(&raw["actor"], &format!("{LABEL}.actor"))?.is_none() {
        return Err(format!("{LABEL} has no actor"));
    }
    if as_str(&raw["host_id"], &format!("{LABEL}.host_id"))?.is_empty() {
        return Err(format!("{LABEL} has no stable host identity"));
    }
    let mut seen: Vec<(String, String)> = Vec::new();
    let items = as_list(&raw["items"], &format!("{LABEL}.items"))?;
    for (index, entry) in items.iter().enumerate() {
        let label = format!("{LABEL}.items[{index}]");
        let entry = as_mapping(entry, &label)?;
        exact_keys(entry, &["input", "record", "archive_entry"], &[], &label)?;
        let input = as_mapping(&entry["input"], &format!("{label}.input"))?;
        exact_keys(
            input,
            &["machine", "slot", "generation", "record_sha256"],
            &[],
            &format!("{label}.input"),
        )?;
        let item_machine = validate_name(
            as_str(&input["machine"], &format!("{label}.input.machine"))?,
            "machine",
        )?;
        let item_slot = validate_name(
            as_str(&input["slot"], &format!("{label}.input.slot"))?,
            "slot",
        )?;
        let item_generation = as_int(
            &input["generation"],
            &format!("{label}.input.generation"),
            1,
        )?;
        let record_digest = as_str(
            &input["record_sha256"],
            &format!("{label}.input.record_sha256"),
        )?;
        if item_machine != journal_machine || !is_digest(record_digest) {
            return Err(format!("{label}.input identity is invalid"));
        }
        let record = record_from_obj(&entry["record"], &format!("{label}.record"))?;
        if record.slot_type != "validate" {
            return Err(format!("slot {item_slot} is not a validation row"));
        }
        if record.machine != item_machine || record.generation != item_generation {
            return Err(format!(
                "validation row {item_machine}/{item_slot} generation changed"
            ));
        }
        let digest =
            crate::canonical_sha256(&record.normalized).map_err(|error| error.to_string())?;
        if digest != record_digest {
            return Err(format!(
                "validation row {item_machine}/{item_slot} changed: expected {record_digest}, found {digest}"
            ));
        }
        let archive_entry = as_mapping(&entry["archive_entry"], &format!("{label}.archive_entry"))?;
        let finished_at = as_str(
            archive_entry.get("finished_at").unwrap_or(&Value::Null),
            &format!("{label}.archive_entry.finished_at"),
        )?;
        parse_timestamp(finished_at, &format!("{label}.archive_entry.finished_at"))?;
        let normalized = record
            .normalized
            .as_object()
            .ok_or_else(|| format!("{label}.record must be a JSON object"))?;
        let mut expected = serde_json::json!({
            "archive_id": format!("{}:{}:{}:{}", record.machine, record.slot, record.generation, finished_at),
            "slot": record.slot,
            "agent": record.agent,
            "task": record.task,
            "purpose": record.purpose,
            "slot_type": record.slot_type,
            "machine": record.machine,
            "generation": normalized["generation"],
            "created_at": record.created_at,
            "finished_at": finished_at,
            "mode": "remove",
            "actor": "coordinator",
            "physical_storage": "removed",
            "validation": [
                "registry reconciliation confirmed physical storage absent",
                format!("source ACTIVE record SHA-256: {record_digest}"),
            ],
            "limitations": ["validation outcome unknown"],
            "continuation": "no storage remains; consult retained validation records by run identity",
            "salvage": [],
            "checkouts": normalized["checkouts"],
        });
        if let Some(layout) = normalized.get("layout") {
            expected["layout"] = layout.clone();
        }
        if let Some(source) = normalized.get("import_source") {
            expected["import_source"] = source.clone();
        }
        let expected_canonical =
            crate::canonical::canonical_json(&expected).map_err(|error| error.to_string())?;
        let actual_canonical =
            crate::canonical::canonical_json(&Value::Object(archive_entry.clone()))
                .map_err(|error| error.to_string())?;
        if actual_canonical != expected_canonical {
            return Err(format!(
                "{label}.archive_entry differs from its exact input row"
            ));
        }
        seen.push((item_machine, item_slot));
    }
    if seen.is_empty() {
        return Err(format!("{LABEL} has no items"));
    }
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != seen.len() {
        return Err(format!("{LABEL} repeats a row"));
    }
    if as_int(&raw["cursor"], &format!("{LABEL}.cursor"), 0)? > seen.len() as i64 {
        return Err(format!("{LABEL} cursor exceeds its item count"));
    }
    Ok(())
}

fn ownerless_validate_shape(
    cfg: &Cfg,
    raw: &Map<String, Value>,
    journal_machine: &str,
) -> Result<()> {
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "slot",
            "phase",
            "fenced",
            "authorization",
        ],
        &["exclusion"],
        "ownerless validation journal",
    )?;
    let phase = as_str(&raw["phase"], "ownerless validation journal.phase")?;
    if !matches!(phase, "prepared" | "fenced" | "removing" | "removed") {
        return Err(format!("unknown ownerless validation phase {phase:?}"));
    }
    as_str(&raw["fenced"], "ownerless validation journal.fenced")?;
    let (target_kind, auth_path) = validation_authorization(&raw["authorization"])?;
    if let Some(exclusion) = raw.get("exclusion") {
        if as_int(&raw["schema"], "ownerless validation journal.schema", 0)? != SCHEMA
            || raw["kind"] != Value::String("ownerless-validate-remove".to_owned())
            || raw["machine"].as_str() != Some(journal_machine)
        {
            return Err("ownerless validation journal identity is invalid".to_owned());
        }
        let target = recovery_target(cfg, &auth_path, &target_kind)?;
        let target_name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if as_str(&raw["slot"], "ownerless validation slot")? != target_name {
            return Err("ownerless validation journal slot differs from its path".to_owned());
        }
        let raw_fenced = as_str(&raw["fenced"], "fenced validation path")?;
        let fenced = if target_kind == "frozen-checkout" {
            if !raw_fenced.starts_with('/') {
                return Err("fenced frozen validation path must be absolute".to_owned());
            }
            let fenced = PathBuf::from(normpath_absolute(raw_fenced));
            if fenced.parent() != Some(frozen_directory(cfg).as_path()) {
                return Err("fenced frozen validation path has the wrong parent".to_owned());
            }
            let allowed_root = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
            crate::config_validate::ensure_no_symlink_components(
                &allowed_root,
                &fenced,
                "fenced frozen validation path",
            )?;
            fenced
        } else {
            crate::config_validate::relative_inside(&cfg.root, raw_fenced, "fenced path")?.1
        };
        let prefix = format!(".{target_name}.ownerless-validate.");
        let fenced_name = fenced
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tail = fenced_name.strip_prefix(&prefix).unwrap_or_default();
        if fenced.parent() != target.parent()
            || !fenced_name.starts_with(&prefix)
            || !(tail.len() == 32
                && tail
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        {
            return Err("fenced validation path does not match its target".to_owned());
        }
        exclusion_from_obj(cfg, &fenced, exclusion)?;
    }
    Ok(())
}

/// Read and shape-validate one recovery journal in the control directory,
/// printing it as stored.
pub(crate) fn parse_journal(
    root: Option<&Path>,
    machine_override: Option<&str>,
    name: &str,
) -> Result<Value> {
    if name.is_empty() || name.contains(['/', '\\']) || matches!(name, "." | "..") {
        return Err("journal name must be a file name inside the control directory".to_owned());
    }
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let path: PathBuf = cfg.control.join(name);
    refuse_symlink(&path, "recovery journal")?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing recovery journal: {}", path.display())
        } else {
            format!("cannot read recovery journal {}: {error}", path.display())
        }
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("recovery journal is malformed JSON: {error}"))?;
    let raw = as_mapping(&value, "recovery journal")?;
    if as_int(
        raw.get("schema").unwrap_or(&Value::Null),
        "recovery journal.schema",
        0,
    )? != SCHEMA
    {
        return Err("unsupported recovery journal schema".to_owned());
    }
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        "recovery journal.machine",
    )?
    .to_owned();
    let kind = as_str(
        raw.get("kind").unwrap_or(&Value::Null),
        "recovery journal.kind",
    )?
    .to_owned();
    let slot = validate_name(
        as_str(
            raw.get("slot").unwrap_or(&Value::Null),
            "recovery journal.slot",
        )?,
        "slot",
    )?;
    let mut expected = vec![format!("ACTIVE.{machine}.journal")];
    if kind == "create" {
        expected.push(format!(
            "CREATE.{}.{}.{}.{}.journal",
            machine.chars().count(),
            machine,
            slot.chars().count(),
            slot
        ));
    }
    if kind == "finish" {
        expected.push(format!(
            "FINISH.{}.{}.{}.{}.journal",
            machine.chars().count(),
            machine,
            slot.chars().count(),
            slot
        ));
    }
    if !expected.iter().any(|candidate| candidate == name) {
        return Err(
            "recovery journal filename does not match its machine, operation, and slot".to_owned(),
        );
    }
    match kind.as_str() {
        "create" => create_shape(raw)?,
        "import-existing" => {
            exact_keys(
                raw,
                &["schema", "kind", "machine", "slot", "record"],
                &[],
                "import journal",
            )?;
            let record = record_from_obj(&raw["record"], "import journal.record")?;
            if record.machine != machine || record.slot != slot {
                return Err("import journal record does not match its shard or slot".to_owned());
            }
        }
        "finish" => finish_shape(
            raw,
            &machine,
            &slot,
            expected.len() > 1 && name == expected[1],
        )?,
        "legacy-validate-remove" => {
            exact_keys(
                raw,
                &[
                    "schema",
                    "kind",
                    "machine",
                    "slot",
                    "phase",
                    "checkout",
                    "repository",
                    "completed_record",
                    "completed_record_sha256",
                    "head",
                    "actor",
                    "coordinator_authorized",
                ],
                &[],
                "legacy validate removal journal",
            )?;
            for field in [
                "phase",
                "checkout",
                "repository",
                "completed_record",
                "completed_record_sha256",
                "head",
            ] {
                as_str(
                    &raw[field],
                    &format!("legacy validate removal journal.{field}"),
                )?;
            }
            if identity_from_obj(&raw["actor"], "legacy validate removal journal.actor")?.is_none()
            {
                return Err("legacy validate removal journal has no actor".to_owned());
            }
            if !raw["coordinator_authorized"].is_boolean() {
                return Err(
                    "legacy validate removal journal.coordinator_authorized must be boolean"
                        .to_owned(),
                );
            }
        }
        "ownerless-validate-remove" => {
            ownerless_validate_shape(&cfg, raw, &machine)?;
        }
        "recover-absent-validate-rows" => {
            absent_validate_shape(raw, &machine)?;
        }
        "ownerless-agent-remove" => {
            ownerless_agent_shape(&cfg, raw, &machine)?;
        }
        _ => return Err(format!("unsupported recovery journal kind {kind:?}")),
    }
    Ok(Value::Object(raw.clone()))
}
