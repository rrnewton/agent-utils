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

pub(crate) fn planned_from_obj(value: &Value, label: &str) -> Result<()> {
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
        return Err(format!("unknown finish journal phase '{phase}'"));
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
            "unknown ownerless validation exclusion phase '{phase}'"
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
        return Err(format!("unknown ownerless agent phase '{phase}'"));
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

fn ownerless_clone_authorization(value: &Value) -> Result<(String, (i64, i64, i64))> {
    let label = "ownerless clone authorization";
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "path",
            "identity",
            "actor",
            "remote_urls",
            "inventory_sha256",
            "handoff_sha256s",
            "discard_ignored",
            "recorded_at",
        ],
        &[],
        label,
    )?;
    let mut identity = Vec::new();
    for item in as_list(&raw["identity"], "ownerless clone directory identity")? {
        identity.push(as_int(item, "ownerless clone directory identity", 0)?);
    }
    if identity.len() != 3 {
        return Err("ownerless clone directory identity must have three fields".to_owned());
    }
    if identity_from_obj(&raw["actor"], "ownerless clone recovery actor")?.is_none() {
        return Err("ownerless clone recovery has no actor".to_owned());
    }
    for url in as_mapping(&raw["remote_urls"], "ownerless clone remote URLs")?.values() {
        as_str(url, "ownerless clone remote URL")?;
    }
    let digest = as_str(&raw["inventory_sha256"], "ownerless clone inventory digest")?;
    let mut handoffs_valid = true;
    for item in as_list(&raw["handoff_sha256s"], "ownerless clone handoff digests")? {
        let handoff = as_str(item, "ownerless clone handoff digest")?;
        handoffs_valid &= is_digest(handoff);
    }
    if !is_digest(digest) || !handoffs_valid {
        return Err("ownerless clone digests are invalid".to_owned());
    }
    let recorded_at = as_str(&raw["recorded_at"], "ownerless clone recorded_at")?;
    parse_timestamp(recorded_at, "ownerless clone recorded_at")?;
    if !raw["discard_ignored"].is_boolean() {
        return Err("ownerless clone discard_ignored must be boolean".to_owned());
    }
    let path = as_str(&raw["path"], "ownerless clone path")?.to_owned();
    Ok((path, (identity[0], identity[1], identity[2])))
}

fn ownerless_clone_shape(cfg: &Cfg, raw: &Map<String, Value>, journal_machine: &str) -> Result<()> {
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
            "rescue",
            "record",
        ],
        &[],
        "ownerless clone journal",
    )?;
    if as_int(&raw["schema"], "ownerless clone journal.schema", 0)? != SCHEMA
        || raw["kind"] != Value::String("ownerless-clone-remove".to_owned())
        || raw["machine"].as_str() != Some(journal_machine)
    {
        return Err("ownerless clone journal identity is invalid".to_owned());
    }
    let (auth_path, identity) = ownerless_clone_authorization(&raw["authorization"])?;
    let (_relative, target) =
        crate::config_validate::relative_inside(&cfg.root, &auth_path, "ownerless agent worktree")?;
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
        return Err("ownerless clone journal slot differs from its path".to_owned());
    }
    let (_relative, fenced) = crate::config_validate::relative_inside(
        &cfg.root,
        as_str(&raw["fenced"], "ownerless clone fenced path")?,
        "fenced path",
    )?;
    let fenced_name = fenced
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!(".{target_name}.ownerless-clone.");
    let tail = fenced_name.strip_prefix(&prefix).unwrap_or_default();
    if fenced.parent() != target.parent()
        || !fenced_name.starts_with(&prefix)
        || !(tail.len() == 32
            && tail
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err("ownerless clone fenced path does not match its target".to_owned());
    }
    let phase = as_str(&raw["phase"], "ownerless clone journal.phase")?;
    if !matches!(
        phase,
        "prepared" | "rescued" | "fenced" | "removing" | "removed"
    ) {
        return Err(format!("unknown ownerless clone phase '{phase}'"));
    }
    for (index, item) in as_list(&raw["rescue"], "ownerless clone journal.rescue")?
        .iter()
        .enumerate()
    {
        as_mapping(item, &format!("ownerless clone journal.rescue[{index}]"))?;
    }
    if !raw["record"].is_null() {
        as_mapping(&raw["record"], "ownerless clone journal.record")?;
    }
    let present = |path: &Path| path.symlink_metadata().is_ok();
    let target_present = present(&target);
    let fenced_present = present(&fenced);
    if target_present && fenced_present {
        return Err("canonical and fenced ownerless clone paths both exist".to_owned());
    }
    let active = if target_present {
        Some(target)
    } else if fenced_present {
        Some(fenced)
    } else {
        None
    };
    if phase == "removed" && active.is_some() {
        return Err("ownerless clone reappeared after removal was recorded".to_owned());
    }
    if let Some(active) = &active {
        if active.is_symlink()
            || !active.is_dir()
            || open_directory_identity(active, "ownerless clone")? != identity
        {
            return Err("ownerless clone identity changed; preserve it".to_owned());
        }
    }
    Ok(())
}

fn host_id() -> Result<String> {
    let mut failures: Vec<String> = Vec::new();
    for candidate in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        match std::fs::read_to_string(candidate) {
            Ok(text) => {
                let value = text.trim();
                if !value.is_empty() {
                    return Ok(value.to_owned());
                }
                failures.push(format!("{candidate}: empty"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("{candidate}: {error}")),
        }
    }
    let detail = if failures.is_empty() {
        String::new()
    } else {
        format!(" ({})", failures.join("; "))
    };
    Err(format!(
        "cannot establish the stable machine identity{detail}"
    ))
}

fn ownerless_cache_shape(cfg: &Cfg, raw: &Map<String, Value>, journal_machine: &str) -> Result<()> {
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "host_id",
            "slot",
            "phase",
            "source",
            "destination",
            "fenced",
            "identity",
            "actor",
        ],
        &[],
        "ownerless agent cache journal",
    )?;
    if as_int(&raw["schema"], "ownerless agent cache journal.schema", 0)? != SCHEMA
        || raw["kind"] != Value::String("ownerless-agent-cache-relocate".to_owned())
        || raw["machine"].as_str() != Some(journal_machine)
        || raw["slot"].as_str() != Some("ignored")
    {
        return Err("ownerless agent cache journal identity is invalid".to_owned());
    }
    if as_str(&raw["host_id"], "ownerless agent cache host_id")? != host_id()? {
        return Err("ownerless agent cache journal belongs to another host identity".to_owned());
    }
    let expected_source = cfg.worktrees.join("ignored");
    let expected_destination = cfg
        .root
        .join("ignored")
        .join("validate")
        .join("cache")
        .join("wrkslots-agent-ignored");
    let (_relative, source) = crate::config_validate::relative_inside(
        &cfg.root,
        as_str(&raw["source"], "ownerless agent cache source")?,
        "cache source",
    )?;
    let (_relative, destination) = crate::config_validate::relative_inside(
        &cfg.root,
        as_str(&raw["destination"], "ownerless agent cache destination")?,
        "cache destination",
    )?;
    let (_relative, fenced) = crate::config_validate::relative_inside(
        &cfg.root,
        as_str(&raw["fenced"], "ownerless agent cache fence")?,
        "cache fence",
    )?;
    if source != expected_source || destination != expected_destination {
        return Err("ownerless agent cache journal names a non-canonical path".to_owned());
    }
    let prefix = ".ignored.ownerless-agent-cache.";
    let fenced_name = fenced
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tail = fenced_name.strip_prefix(prefix).unwrap_or_default();
    if fenced.parent() != source.parent()
        || !fenced_name.starts_with(prefix)
        || !(tail.len() == 32
            && tail
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err("ownerless agent cache fence is invalid".to_owned());
    }
    let mut identity = Vec::new();
    for item in as_list(&raw["identity"], "ownerless agent cache identity")? {
        identity.push(as_int(item, "ownerless agent cache identity", 0)?);
    }
    if identity.len() != 3 {
        return Err("ownerless agent cache identity must have three fields".to_owned());
    }
    if identity_from_obj(&raw["actor"], "ownerless agent cache actor")?.is_none() {
        return Err("ownerless agent cache journal has no actor".to_owned());
    }
    let phase = as_str(&raw["phase"], "ownerless agent cache phase")?;
    if !matches!(phase, "prepared" | "fenced" | "relocated") {
        return Err(format!("unknown ownerless agent cache phase '{phase}'"));
    }
    let present = |path: &PathBuf| path.symlink_metadata().is_ok();
    let candidates = [source.clone(), fenced.clone(), destination.clone()];
    let present_paths: Vec<&PathBuf> = candidates.iter().filter(|path| present(path)).collect();
    if present_paths.len() > 1 {
        return Err("more than one ownerless agent cache path exists; preserve all".to_owned());
    }
    let active = present_paths.first().map(|path| (*path).clone());
    if let Some(active) = &active {
        if active.is_symlink()
            || !active.is_dir()
            || open_directory_identity(active, "ownerless agent cache")?
                != (identity[0], identity[1], identity[2])
        {
            return Err("ownerless agent cache identity changed; preserve it".to_owned());
        }
    }
    let effective_phase = if phase != "relocated" && active.as_ref() == Some(&destination) {
        "relocated"
    } else {
        phase
    };
    if effective_phase == "relocated" && active.as_ref() != Some(&destination) {
        return Err("relocated ownerless agent cache is missing from its destination".to_owned());
    }
    Ok(())
}

fn git_ref_name(value: &str, label: &str) -> Result<()> {
    let parts: Vec<&str> = value.split('/').collect();
    let invalid = !value.starts_with("refs/")
        || parts.len() < 2
        || parts
            .iter()
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || value.contains("..")
        || value.contains("@{")
        || value.ends_with('.')
        || value
            .chars()
            .any(|c| (c as u32) < 0x20 || (c as u32) == 0x7f)
        || value.chars().any(|c| " ~^:?*[\\".contains(c));
    if invalid {
        return Err(format!("invalid full {label} {value:?}"));
    }
    Ok(())
}

struct RowItem {
    machine: String,
    slot: String,
    generation: i64,
    record_sha256: String,
}

fn absent_row_identity(value: &Value, label: &str) -> Result<RowItem> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["machine", "slot", "generation", "record_sha256"],
        &[],
        label,
    )?;
    let digest = as_str(&raw["record_sha256"], &format!("{label}.record_sha256"))?;
    if !is_digest(digest) {
        return Err(format!(
            "{label}.record_sha256 must be one lowercase SHA-256 digest"
        ));
    }
    Ok(RowItem {
        machine: validate_name(
            as_str(&raw["machine"], &format!("{label}.machine"))?,
            "machine",
        )?,
        slot: validate_name(as_str(&raw["slot"], &format!("{label}.slot"))?, "slot")?,
        generation: as_int(&raw["generation"], &format!("{label}.generation"), 1)?,
        record_sha256: digest.to_owned(),
    })
}

type Witness = (String, String, String, Option<String>);

fn witness_from_obj(value: &Value, label: &str) -> Result<Witness> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["checkout", "branch_ref", "state", "head"],
        &[],
        label,
    )?;
    let checkout = as_str(&raw["checkout"], &format!("{label}.checkout"))?;
    if checkout.is_empty() {
        return Err(format!("{label}.checkout must not be empty"));
    }
    let branch_ref = as_str(&raw["branch_ref"], &format!("{label}.branch_ref"))?;
    if git_ref_name(branch_ref, &format!("{label}.branch_ref")).is_err() {
        return Err(format!("{label}.branch_ref is invalid"));
    }
    if !branch_ref.starts_with("refs/heads/") {
        return Err(format!("{label}.branch_ref must name refs/heads"));
    }
    let state = as_str(&raw["state"], &format!("{label}.state"))?;
    let head = match state {
        "absent" => {
            if !raw["head"].is_null() {
                return Err(format!(
                    "{label}.head must be null when the branch is absent"
                ));
            }
            None
        }
        "present" => {
            let head = as_str(&raw["head"], &format!("{label}.head"))?;
            if !is_sha(head) {
                return Err(format!("{label}.head must be one full object SHA"));
            }
            Some(head.to_owned())
        }
        _ => {
            return Err(format!("{label}.state must be 'absent' or 'present'"));
        }
    };
    Ok((
        checkout.to_owned(),
        branch_ref.to_owned(),
        state.to_owned(),
        head,
    ))
}

fn record_checkouts(record: &crate::active_state::RecordRec) -> Vec<(String, String, String)> {
    record
        .normalized
        .get("checkouts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        item.get("branch")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        item.get("head")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn rescue_ref(
    record: &crate::active_state::RecordRec,
    checkout: &(String, String, String),
    root: &str,
) -> String {
    format!(
        "{}/{}/{}/{}-{}-{}",
        root,
        record.machine,
        record.slot,
        checkout.0.replace('/', "-"),
        record.generation,
        &checkout.2[..12.min(checkout.2.len())]
    )
}

fn planned_receipts(
    record: &crate::active_state::RecordRec,
    checkouts: &[(String, String, String)],
    witnesses: &[Witness],
    include_witness: bool,
    rescue_root: Option<&str>,
    containment: &[(String, String, String)],
) -> Result<Vec<Value>> {
    if witnesses.len() != checkouts.len() {
        return Err("absent-agent branch witness count differs from checkout count".to_owned());
    }
    if !containment.is_empty() && !include_witness {
        return Err("absent-agent remote containment requires branch witnesses".to_owned());
    }
    let mut receipts = Vec::new();
    for (checkout, witness) in checkouts.iter().zip(witnesses) {
        let existing = containment.iter().find(|(name, _, _)| name == &checkout.0);
        let mut receipt = match existing {
            None => serde_json::json!({
                "checkout": checkout.0,
                "source_head": checkout.2,
                "salvage_commit": checkout.2,
                "status_sha256": Value::Null,
                "disposition": "salvaged",
                "remote_ref": rescue_ref(record, checkout, rescue_root.unwrap_or("refs/rescue/wrkslots")),
                "containing_remote_refs": [],
                "working_tree_contents": "unknown-storage-absent-before-recovery",
            }),
            Some((_, remote_ref, tip)) => serde_json::json!({
                "checkout": checkout.0,
                "source_head": checkout.2,
                "salvage_commit": checkout.2,
                "status_sha256": Value::Null,
                "disposition": "already-on-remote",
                "remote_ref": remote_ref,
                "remote_ref_tip": tip,
                "containing_remote_refs": [remote_ref],
                "working_tree_contents": "unknown-storage-absent-before-recovery",
            }),
        };
        if include_witness {
            receipt["local_branch"] = serde_json::json!({
                "checkout": witness.0,
                "branch_ref": witness.1,
                "state": witness.2,
                "head": witness.3,
            });
            receipt["local_branch_rescue"] = Value::String(match &witness.3 {
                None => "not-applicable-absent".to_owned(),
                Some(head) if *head == checkout.2 => "same-commit-as-recorded-head".to_owned(),
                Some(_) => "no-separate-ref-left-untouched".to_owned(),
            });
        }
        receipts.push(receipt);
    }
    Ok(receipts)
}

fn branch_validation(receipt: &Map<String, Value>) -> Result<String> {
    let checkout = as_str(
        receipt.get("checkout").unwrap_or(&Value::Null),
        "absent-agent receipt.checkout",
    )?;
    let witness = witness_from_obj(
        receipt.get("local_branch").unwrap_or(&Value::Null),
        "absent-agent receipt.local_branch",
    )?;
    let observed = witness.3.as_deref().unwrap_or("absent");
    let disposition = as_str(
        receipt.get("local_branch_rescue").unwrap_or(&Value::Null),
        "absent-agent receipt.local_branch_rescue",
    )?;
    let already = receipt.get("disposition").and_then(Value::as_str) == Some("already-on-remote");
    let remote_ref = receipt
        .get("remote_ref")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source_head = receipt
        .get("source_head")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let detail = if disposition == "not-applicable-absent" && witness.3.is_none() {
        if already {
            format!(
                "had no tip; the recorded HEAD was already contained in existing remote ref {remote_ref}"
            )
        } else {
            "had no tip to include in the recorded-HEAD rescue ref".to_owned()
        }
    } else if disposition == "same-commit-as-recorded-head"
        && witness.3.as_deref() == receipt.get("source_head").and_then(Value::as_str)
    {
        if already {
            format!(
                "named the recorded HEAD, which existing remote ref {remote_ref} already contained"
            )
        } else {
            "named the same commit as the recorded HEAD rescue ref".to_owned()
        }
    } else if disposition == "no-separate-ref-left-untouched" && witness.3.is_some() {
        if already {
            format!(
                "received no rescue ref; only the recorded HEAD {source_head} was checked against existing remote ref {remote_ref}"
            )
        } else {
            format!(
                "received no separate rescue ref; the recorded-HEAD rescue ref targets {source_head}"
            )
        }
    } else {
        return Err("absent-agent local branch rescue disposition is inconsistent".to_owned());
    };
    Ok(format!(
        "checkout {checkout}: recovery issued no source-repository command targeting local branch {} for mutation; checks immediately before and after registration reconciliation matched witnessed state {observed}, then that witness expired when the removal marker became durable and asserts no later branch state; {detail}",
        witness.1
    ))
}

fn module_validation(receipt: &Map<String, Value>) -> Result<String> {
    let commits = as_list(
        receipt.get("unpublished_commits").unwrap_or(&Value::Null),
        "submodule receipt commits",
    )?;
    let prefix = format!(
        "checkout {} submodule {}: ",
        receipt
            .get("checkout")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        receipt
            .get("submodule")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    match receipt.get("disposition").and_then(Value::as_str) {
        Some("already-published") => Ok(format!(
            "{prefix}every commit the submodule repository Git kept for the checkout stored itself, or HEAD or a ref there named, was reachable from a remote-tracking ref there or in the shared submodule repository; nothing was salvaged"
        )),
        Some("salvaged") => Ok(format!(
            "{prefix}{} commit(s) outside every remote-tracking ref were pushed to rescue refs on the submodule's remote and read back before the registration was removed",
            commits.len()
        )),
        _ => Ok(format!(
            "{prefix}{} commit(s) outside every remote-tracking ref were bundled with complete history into {} (SHA-256 {}), which was cloned afresh and fsck-checked before the registration was removed",
            commits.len(),
            receipt.get("archive_bundle").and_then(Value::as_str).unwrap_or_default(),
            receipt.get("archive_bundle_sha256").and_then(Value::as_str).unwrap_or_default()
        )),
    }
}

fn absent_archive_entry(
    record: &crate::active_state::RecordRec,
    item: &RowItem,
    finished_at: &str,
    receipts: &[Value],
    submodules: &[Value],
) -> Result<Value> {
    let branch_flags: Vec<bool> = receipts
        .iter()
        .map(|receipt| {
            receipt
                .as_object()
                .is_some_and(|map| map.contains_key("local_branch"))
        })
        .collect();
    if branch_flags.iter().any(|flag| *flag) && !branch_flags.iter().all(|flag| *flag) {
        return Err("absent-agent receipts mix legacy and branch-witness shapes".to_owned());
    }
    let has_witnesses = branch_flags.first().copied().unwrap_or(false);
    let distinct = receipts.iter().any(|receipt| {
        receipt.get("local_branch_rescue").and_then(Value::as_str)
            == Some("no-separate-ref-left-untouched")
    });
    let already = receipts.iter().any(|receipt| {
        receipt.get("disposition").and_then(Value::as_str) == Some("already-on-remote")
    });
    if already && !has_witnesses {
        return Err("absent-agent remote containment requires branch witnesses".to_owned());
    }
    let mut validation: Vec<Value> = vec![
        Value::String(
            "registry reconciliation confirmed physical storage externally absent".to_owned(),
        ),
        Value::String(format!(
            "source ACTIVE record SHA-256: {}",
            item.record_sha256
        )),
    ];
    for receipt in receipts {
        let map = receipt
            .as_object()
            .ok_or_else(|| "absent-agent receipt must be a JSON object".to_owned())?;
        let checkout = map
            .get("checkout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source_head = map
            .get("source_head")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let remote_ref = map
            .get("remote_ref")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let salvage = map
            .get("salvage_commit")
            .and_then(Value::as_str)
            .unwrap_or_default();
        validation.push(Value::String(
            if map.get("disposition").and_then(Value::as_str) == Some("already-on-remote") {
                let tip = map.get("remote_ref_tip").and_then(Value::as_str).unwrap_or_default();
                format!("checkout {checkout}: recorded slot HEAD {source_head} was already contained in existing remote ref {remote_ref} at tip {tip}; recovery pushed nothing and re-verified the containment with git merge-base --is-ancestor against a freshly fetched copy of that ref")
            } else if has_witnesses {
                format!("checkout {checkout}: recorded slot HEAD {source_head} preserved at remote rescue ref {remote_ref} and read back at {salvage}")
            } else {
                format!("checkout {checkout}: remote rescue ref {remote_ref} read back at {salvage}")
            },
        ));
    }
    if has_witnesses {
        for receipt in receipts {
            let map = receipt
                .as_object()
                .ok_or_else(|| "absent-agent receipt must be a JSON object".to_owned())?;
            validation.push(Value::String(branch_validation(map)?));
        }
    }
    for receipt in submodules {
        let map = receipt
            .as_object()
            .ok_or_else(|| "absent-agent receipt must be a JSON object".to_owned())?;
        validation.push(Value::String(module_validation(map)?));
    }
    let mut limitations: Vec<Value> = vec![Value::String(
        "working-tree contents were absent before recovery; tracked and ordinary untracked content and HANDOFF could not be inspected or salvaged; ignored content could not be inspected and would not have been uploaded"
            .to_owned(),
    )];
    if has_witnesses {
        limitations.push(Value::String(
            "local branch witnesses cover only registration reconciliation; no branch state after the durable removal marker is asserted".to_owned(),
        ));
    }
    if distinct {
        limitations.push(Value::String(
            "local branch tips distinct from the recorded slot HEAD were left untouched in source repositories; no separate rescue refs were created to target those exact tips".to_owned(),
        ));
    }
    if already {
        limitations.push(Value::String(
            "a recorded HEAD found in an existing remote ref stays preserved only while that ref, which recovery neither created nor protects, still contains it".to_owned(),
        ));
    }
    if !submodules.is_empty() {
        limitations.push(Value::String(format!(
            "submodule repositories Git kept for the checkouts under <common>/worktrees/<id>/modules were moved intact, reflogs included, to <common>/wrkslots-retained-modules/{}+{}+{}+<id>/modules before each registration was removed, and recovery never deletes them; at the final census every commit their HEADs, refs and reflogs named and every commit they stored themselves, whether named or not, was compared with their remote-tracking refs and those of the shared repository, and each reachable from none of those was salvaged; none they read through objects/info/alternates was left both unreachable from those and unsalvaged; a commit written there later, by a process already holding a handle inside them, is kept there but recorded nowhere; a commit found in a remote-tracking ref stays preserved only while that remote still holds it",
            record.slot,
            record.generation,
            &item.record_sha256[..16]
        )));
    }
    let continuation = if already {
        "physical storage was already absent; inspect each checkout's verified remote ref, either a rescue ref this recovery pushed or an existing ref that already contained the recorded HEAD, and re-read every local branch ref because its historical witness has expired"
    } else if has_witnesses {
        "physical storage was already absent; inspect the verified recorded-HEAD rescue refs and re-read every local branch ref because its historical witness has expired"
    } else {
        "physical storage was already absent; inspect the verified rescue refs"
    };
    let mut salvage: Vec<Value> = receipts.to_vec();
    salvage.extend(submodules.iter().cloned());
    let normalized = record
        .normalized
        .as_object()
        .ok_or_else(|| "absent-agent record must be a JSON object".to_owned())?;
    let mut entry = serde_json::json!({
        "archive_id": format!("{}:{}:{}:{}", record.machine, record.slot, record.generation, finished_at),
        "slot": record.slot,
        "agent": record.agent,
        "task": record.task,
        "purpose": record.purpose,
        "slot_type": "agent",
        "machine": record.machine,
        "generation": normalized["generation"],
        "created_at": record.created_at,
        "finished_at": finished_at,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": validation,
        "limitations": limitations,
        "continuation": continuation,
        "salvage": salvage,
        "checkouts": normalized["checkouts"],
    });
    if let Some(layout) = normalized.get("layout") {
        entry["layout"] = layout.clone();
    }
    if let Some(source) = normalized.get("import_source") {
        entry["import_source"] = source.clone();
    }
    Ok(entry)
}

fn module_commit_refs(map: &Map<String, Value>, key: &str, label: &str) -> Result<Vec<String>> {
    let mut commits = Vec::new();
    for (index, value) in as_list(
        map.get(key).unwrap_or(&Value::Null),
        &format!("{label}.{key}"),
    )?
    .iter()
    .enumerate()
    {
        let row_label = format!("{label}.{key}[{index}]");
        let entry = as_mapping(value, &row_label)?;
        exact_keys(entry, &["commit", "ref"], &[], &row_label)?;
        commits.push(as_str(&entry["commit"], &format!("{row_label}.commit"))?.to_owned());
        let reference = as_str(&entry["ref"], &format!("{row_label}.ref"))?;
        crate::config_validate::validate_full_ref(reference, &format!("{row_label}.ref"))?;
    }
    Ok(commits)
}

fn submodule_salvage(raw: &Map<String, Value>, checkout_names: &[String]) -> Result<Vec<Value>> {
    let Some(value) = raw.get("submodule_salvage") else {
        return Ok(Vec::new());
    };
    let entries = as_list(value, "absent-agent-row journal.submodule_salvage")?;
    if entries.is_empty() {
        return Err("absent-agent-row journal.submodule_salvage is empty".to_owned());
    }
    let mut receipts: Vec<Value> = Vec::new();
    let mut last: Option<(usize, String)> = None;
    for (index, entry_value) in entries.iter().enumerate() {
        let label = format!("absent-agent-row journal.submodule_salvage[{index}]");
        let entry = as_mapping(entry_value, &label)?;
        let disposition = as_str(
            entry.get("disposition").unwrap_or(&Value::Null),
            &format!("{label}.disposition"),
        )?;
        let extras: &[&str] = match disposition {
            "already-published" => &[],
            "salvaged" => &["remote_url_sha256", "remote_refs"],
            "archived-local" => &[
                "archive_root",
                "archive_receipt",
                "archive_receipt_sha256",
                "archive_bundle",
                "archive_bundle_sha256",
                "archive_bundle_bytes",
                "archive_refs",
                "complete_history",
                "archive_reason",
                "remote_failure",
            ],
            _ => {
                return Err(format!("{label}.disposition {disposition:?} is unknown"));
            }
        };
        let mut required: Vec<&str> = vec![
            "checkout",
            "submodule",
            "submodule_repository",
            "shared_repository",
            "unpublished_commits",
            "disposition",
        ];
        required.extend(extras.iter().copied());
        exact_keys(entry, &required, &[], &label)?;
        let checkout = as_str(&entry["checkout"], &format!("{label}.checkout"))?;
        let submodule = as_str(&entry["submodule"], &format!("{label}.submodule"))?;
        as_str(
            &entry["submodule_repository"],
            &format!("{label}.submodule_repository"),
        )?;
        if !entry["shared_repository"].is_null() {
            as_str(
                &entry["shared_repository"],
                &format!("{label}.shared_repository"),
            )?;
        }
        let position = checkout_names.iter().position(|name| name == checkout);
        if position.is_none() || submodule.is_empty() {
            return Err(format!("{label} names no recorded checkout and submodule"));
        }
        let mut commits = Vec::new();
        for (commit_index, commit) in as_list(
            &entry["unpublished_commits"],
            &format!("{label}.unpublished_commits"),
        )?
        .iter()
        .enumerate()
        {
            commits.push(
                as_str(
                    commit,
                    &format!("{label}.unpublished_commits[{commit_index}]"),
                )?
                .to_owned(),
            );
        }
        let mut sorted = commits.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if commits.iter().any(|commit| !is_sha(commit))
            || commits != sorted
            || (disposition == "already-published") != commits.is_empty()
        {
            return Err(format!(
                "{label}.unpublished_commits is invalid for {disposition}"
            ));
        }
        if disposition == "salvaged" {
            let digest = as_str(
                &entry["remote_url_sha256"],
                &format!("{label}.remote_url_sha256"),
            )?;
            if !is_digest(digest) || module_commit_refs(entry, "remote_refs", &label)? != commits {
                return Err(format!("{label} remote refs differ from its commits"));
            }
        } else if disposition == "archived-local" {
            for key in [
                "archive_root",
                "archive_receipt",
                "archive_bundle",
                "archive_reason",
            ] {
                as_str(&entry[key], &format!("{label}.{key}"))?;
            }
            for key in ["archive_receipt_sha256", "archive_bundle_sha256"] {
                let digest = as_str(&entry[key], &format!("{label}.{key}"))?;
                if !is_digest(digest) {
                    return Err(format!("{label}.{key} must be one SHA-256 digest"));
                }
            }
            as_int(
                &entry["archive_bundle_bytes"],
                &format!("{label}.archive_bundle_bytes"),
                1,
            )?;
            if !entry["remote_failure"].is_null() {
                as_str(&entry["remote_failure"], &format!("{label}.remote_failure"))?;
            }
            if entry["complete_history"] != Value::Bool(true)
                || module_commit_refs(entry, "archive_refs", &label)? != commits
            {
                return Err(format!("{label} archive refs differ from its commits"));
            }
        }
        let position = (position.expect("checked above"), submodule.to_owned());
        if last.as_ref().is_some_and(|previous| position <= *previous) {
            return Err(
                "absent-agent-row submodule receipts are not distinct and in checkout order"
                    .to_owned(),
            );
        }
        last = Some(position);
        receipts.push(entry_value.clone());
    }
    Ok(receipts)
}

fn absent_agent_shape(raw: &Map<String, Value>, journal_machine: &str) -> Result<()> {
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "host_id",
            "slot",
            "phase",
            "actor",
            "input",
            "record",
            "preserved",
            "removed",
            "archive_entry",
        ],
        &[
            "branch_witnesses",
            "remote_containment",
            "submodule_salvage",
        ],
        "absent-agent-row journal",
    )?;
    if as_int(&raw["schema"], "absent-agent-row journal.schema", 0)? != SCHEMA
        || raw["kind"] != Value::String("recover-absent-agent-row".to_owned())
        || raw["machine"].as_str() != Some(journal_machine)
    {
        return Err("absent-agent-row journal identity is invalid".to_owned());
    }
    let item = absent_row_identity(&raw["input"], "absent-agent-row journal.input")?;
    if item.machine != journal_machine || raw["slot"].as_str() != Some(item.slot.as_str()) {
        return Err("absent-agent-row journal row identity is invalid".to_owned());
    }
    if as_str(&raw["host_id"], "absent-agent-row journal.host_id")?.is_empty() {
        return Err("absent-agent-row journal has no stable host identity".to_owned());
    }
    if identity_from_obj(&raw["actor"], "absent-agent-row journal.actor")?.is_none() {
        return Err("absent-agent-row journal has no actor".to_owned());
    }
    let phase = as_str(&raw["phase"], "absent-agent-row journal.phase")?;
    if !matches!(phase, "prepared" | "preserved" | "registrations-removed") {
        return Err(format!("unknown absent-agent-row phase '{phase}'"));
    }
    let record = record_from_obj(&raw["record"], "absent-agent-row journal.record")?;
    if record.slot_type != "agent" {
        return Err(format!("slot {} is not an agent row", item.slot));
    }
    if record.machine != item.machine || record.generation != item.generation {
        return Err(format!(
            "agent row {}/{} generation changed",
            item.machine, item.slot
        ));
    }
    let observed =
        crate::canonical_sha256(&record.normalized).map_err(|error| error.to_string())?;
    if observed != item.record_sha256 {
        return Err(format!(
            "agent row {}/{} changed: expected {}, found {observed}",
            item.machine, item.slot, item.record_sha256
        ));
    }
    let checkouts = record_checkouts(&record);
    let checkout_names: Vec<String> = checkouts.iter().map(|item| item.0.clone()).collect();
    let legacy = !raw.contains_key("branch_witnesses");
    let witnesses: Vec<Witness> = if legacy {
        checkouts
            .iter()
            .map(|checkout| {
                (
                    checkout.0.clone(),
                    format!("refs/heads/{}", checkout.1),
                    "present".to_owned(),
                    Some(checkout.2.clone()),
                )
            })
            .collect()
    } else {
        let mut parsed = Vec::new();
        for (index, value) in as_list(
            &raw["branch_witnesses"],
            "absent-agent-row journal.branch_witnesses",
        )?
        .iter()
        .enumerate()
        {
            parsed.push(witness_from_obj(
                value,
                &format!("absent-agent-row journal.branch_witnesses[{index}]"),
            )?);
        }
        parsed
    };
    if witnesses.len() != checkouts.len() {
        return Err("absent-agent-row branch witness count differs from checkout count".to_owned());
    }
    for (checkout, witness) in checkouts.iter().zip(&witnesses) {
        if witness.0 != checkout.0 || witness.1 != format!("refs/heads/{}", checkout.1) {
            return Err(format!(
                "absent-agent-row branch witness differs from checkout {}",
                checkout.0
            ));
        }
    }
    let containment: Vec<(String, String, String)> = match raw.get("remote_containment") {
        None => Vec::new(),
        Some(value) => {
            if legacy {
                return Err(
                    "absent-agent-row remote containment requires branch witnesses".to_owned(),
                );
            }
            let entries = as_list(value, "absent-agent-row journal.remote_containment")?;
            if entries.is_empty() {
                return Err("absent-agent-row journal.remote_containment is empty".to_owned());
            }
            let mut parsed: Vec<(String, String, String)> = Vec::new();
            for (index, entry_value) in entries.iter().enumerate() {
                let label = format!("absent-agent-row journal.remote_containment[{index}]");
                let entry = as_mapping(entry_value, &label)?;
                exact_keys(entry, &["checkout", "remote_ref", "tip"], &[], &label)?;
                let name = as_str(&entry["checkout"], &format!("{label}.checkout"))?.to_owned();
                let reference = as_str(&entry["remote_ref"], &format!("{label}.remote_ref"))?;
                let tip = as_str(&entry["tip"], &format!("{label}.tip"))?;
                if git_ref_name(reference, &format!("{label}.remote_ref")).is_err() {
                    return Err(format!("{label}.remote_ref is invalid"));
                }
                if !is_sha(tip) {
                    return Err(format!("{label}.tip must be one full object SHA"));
                }
                if !checkout_names.contains(&name)
                    || parsed.iter().any(|(seen, _, _)| *seen == name)
                {
                    return Err(format!(
                        "{label}.checkout must name one distinct recorded checkout"
                    ));
                }
                parsed.push((name, reference.to_owned(), tip.to_owned()));
            }
            let ordered: Vec<String> = checkout_names
                .iter()
                .filter(|name| parsed.iter().any(|(seen, _, _)| seen == *name))
                .cloned()
                .collect();
            if parsed
                .iter()
                .map(|(name, _, _)| name.clone())
                .collect::<Vec<_>>()
                != ordered
            {
                return Err(
                    "absent-agent-row remote containment is not in checkout order".to_owned(),
                );
            }
            parsed
        }
    };
    let submodules = submodule_salvage(raw, &checkout_names)?;
    let mut receipts: Vec<Value> = Vec::new();
    for (index, value) in as_list(&raw["preserved"], "absent-agent-row journal.preserved")?
        .iter()
        .enumerate()
    {
        as_mapping(
            value,
            &format!("absent-agent-row journal.preserved[{index}]"),
        )?;
        receipts.push(value.clone());
    }
    let mut removed: Vec<String> = Vec::new();
    for (index, value) in as_list(&raw["removed"], "absent-agent-row journal.removed")?
        .iter()
        .enumerate()
    {
        removed
            .push(as_str(value, &format!("absent-agent-row journal.removed[{index}]"))?.to_owned());
    }
    if receipts.len() > checkouts.len() || removed.len() > checkouts.len() {
        return Err("absent-agent-row journal progress exceeds its checkout count".to_owned());
    }
    let receipt_names: Vec<String> = receipts
        .iter()
        .map(|receipt| match receipt.get("checkout") {
            Some(Value::String(name)) => name.clone(),
            _ => "None".to_owned(),
        })
        .collect();
    if receipt_names
        .iter()
        .any(|name| !checkout_names.contains(name))
        || removed.iter().any(|name| !checkout_names.contains(name))
    {
        return Err("absent-agent-row journal names an unrelated checkout".to_owned());
    }
    let archive_entry = as_mapping(
        &raw["archive_entry"],
        "absent-agent-row journal.archive_entry",
    )?;
    let finished_at = as_str(
        archive_entry.get("finished_at").unwrap_or(&Value::Null),
        "absent-agent archive.finished_at",
    )?;
    parse_timestamp(finished_at, "absent-agent archive.finished_at")?;
    let canonical =
        |value: &Value| crate::canonical::canonical_json(value).map_err(|error| error.to_string());
    let mut expected_receipts: Vec<Value> = Vec::new();
    let mut matched = false;
    for root in ["refs/rescue/wrkslots", "refs/heads/rescue/wrkslots"] {
        let planned = planned_receipts(
            &record,
            &checkouts,
            &witnesses,
            !legacy,
            Some(root),
            &containment,
        )?;
        let expected = absent_archive_entry(&record, &item, finished_at, &planned, &submodules)?;
        if canonical(&Value::Object(archive_entry.clone()))? == canonical(&expected)? {
            expected_receipts = planned;
            matched = true;
            break;
        }
    }
    if !matched {
        return Err("absent-agent-row archive entry differs from its exact inputs".to_owned());
    }
    for (index, receipt) in receipts.iter().enumerate() {
        if canonical(receipt)? != canonical(&expected_receipts[index])? {
            return Err("absent-agent-row preserved receipts are not a durable prefix".to_owned());
        }
    }
    if removed != checkout_names[..removed.len()] {
        return Err("absent-agent-row removed registrations are not a durable prefix".to_owned());
    }
    if !removed.is_empty() && (phase == "prepared" || receipts.len() != checkouts.len()) {
        return Err(
            "absent-agent-row removed registrations precede durable remote preservation".to_owned(),
        );
    }
    if matches!(phase, "preserved" | "registrations-removed") && receipts.len() != checkouts.len() {
        return Err(
            "absent-agent-row journal phase precedes complete remote preservation".to_owned(),
        );
    }
    if phase == "registrations-removed" && removed.len() != checkouts.len() {
        return Err(
            "absent-agent-row journal phase precedes complete registration removal".to_owned(),
        );
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
        return Err(format!("unknown ownerless validation phase '{phase}'"));
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

/// Shape-validate one journal object against its file name,
/// exactly as the oracle's `_validate_journal_shape`: schema,
/// machine/kind/slot agreement with the name, then the
/// kind-specific key sets and sub-objects. Returns
/// (machine, kind, slot).
pub(crate) fn validate_journal_shape(
    cfg: &Cfg,
    name: &str,
    raw: &Map<String, Value>,
) -> Result<(String, String, String)> {
    if as_int(
        raw.get("schema").unwrap_or(&Value::Null),
        "recovery journal.schema",
        0,
    )? != SCHEMA
    {
        return Err("unsupported recovery journal schema".to_owned());
    }
    let machine = validate_name(
        as_str(
            raw.get("machine").unwrap_or(&Value::Null),
            "recovery journal.machine",
        )?,
        "machine",
    )?;
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
            ownerless_validate_shape(cfg, raw, &machine)?;
        }
        "recover-absent-validate-rows" => {
            absent_validate_shape(raw, &machine)?;
        }
        "ownerless-agent-remove" => {
            ownerless_agent_shape(cfg, raw, &machine)?;
        }
        "ownerless-clone-remove" => {
            ownerless_clone_shape(cfg, raw, &machine)?;
        }
        "ownerless-agent-cache-relocate" => {
            ownerless_cache_shape(cfg, raw, &machine)?;
        }
        "recover-absent-agent-row" => {
            absent_agent_shape(raw, &machine)?;
        }
        _ => return Err(format!("unsupported recovery journal kind '{kind}'")),
    }
    Ok((machine, kind, slot))
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
    validate_journal_shape(&cfg, name, raw)?;
    Ok(value)
}

// ---------------------------------------------------------------------
// Provenance: shape-validated journals checked against the
// append-only operation evidence, as `_validated_cache_journal`
// and `_all_journal_cache_slots` do. A journal that fails here
// refuses the whole audit, as in the oracle.

use std::collections::{BTreeMap, BTreeSet};

use crate::active_state::identity_to_obj;
use crate::canonical::canonical_json;
use crate::handoff_state::{read_regular_file_identity, strict_json_object};

/// One outstanding recovery journal, validated against its
/// append-only evidence.
pub(crate) struct ValidatedJournal {
    pub(crate) slot: String,
    pub(crate) machine: String,
    pub(crate) state: String,
    /// The validated journal object; cache planning reads its
    /// checkouts from here.
    #[allow(dead_code)]
    pub(crate) raw: Value,
}

fn journal_legacy_path(cfg: &Cfg, machine: &str) -> PathBuf {
    cfg.control.join(format!("ACTIVE.{machine}.journal"))
}

fn journal_scoped_path(cfg: &Cfg, operation: &str, machine: &str, slot: &str) -> PathBuf {
    cfg.control.join(format!(
        "{operation}.{}.{}.{}.{}.journal",
        machine.chars().count(),
        machine,
        slot.chars().count(),
        slot
    ))
}

fn json_equal(left: &Value, right: &Value) -> bool {
    match (canonical_json(left), canonical_json(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// The path one append-only operation event claims for its
/// journal, with the oracle's identity checks.
fn operation_event_journal_path(
    cfg: &Cfg,
    machine: &str,
    payload: &Map<String, Value>,
    journal: Option<&Map<String, Value>>,
) -> Result<PathBuf> {
    let Some(raw_name) = payload.get("journal_path").filter(|value| !value.is_null()) else {
        return Ok(journal_legacy_path(cfg, machine));
    };
    let name = as_str(raw_name, "append-only operation journal_path")?;
    if name.contains('/') || name.ends_with('/') || matches!(name, "." | "..") {
        return Err("append-only operation journal_path is not a filename".to_owned());
    }
    let operation = as_str(
        payload.get("operation").unwrap_or(&Value::Null),
        "append-only operation kind",
    )?;
    let slot = validate_name(
        as_str(
            payload.get("slot").unwrap_or(&Value::Null),
            "append-only operation slot",
        )?,
        "slot",
    )?;
    let expected = if operation == "create" && name.starts_with("CREATE.") {
        journal_scoped_path(cfg, "CREATE", machine, &slot)
    } else if operation == "finish" && name.starts_with("FINISH.") {
        journal_scoped_path(cfg, "FINISH", machine, &slot)
    } else {
        journal_legacy_path(cfg, machine)
    };
    let selected = cfg.control.join(name);
    if selected != expected {
        return Err(
            "append-only operation journal_path does not match its operation identity".to_owned(),
        );
    }
    if let Some(journal) = journal {
        if journal.get("kind").and_then(Value::as_str) != Some(operation)
            || journal.get("slot").and_then(Value::as_str) != Some(slot.as_str())
            || journal.get("machine").and_then(Value::as_str) != Some(machine)
        {
            return Err(
                "append-only operation event does not match its embedded journal".to_owned(),
            );
        }
    }
    Ok(selected)
}

/// Pending journal evidence from one machine's append-only log:
/// progress records a journal, a matching completion clears it.
fn pending_operations(
    cfg: &Cfg,
    machine: &str,
    events: &[Value],
) -> Result<BTreeMap<PathBuf, Map<String, Value>>> {
    let mut pending: BTreeMap<PathBuf, Map<String, Value>> = BTreeMap::new();
    for event in events {
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        let payload = as_mapping(
            event.get("payload").unwrap_or(&Value::Null),
            "append-only event.payload",
        )?;
        if kind == "operation-progress-recorded" {
            let journal = as_mapping(
                payload.get("journal").unwrap_or(&Value::Null),
                "append-only operation journal",
            )?;
            let path = operation_event_journal_path(cfg, machine, payload, Some(journal))?;
            for (existing_path, existing) in &pending {
                if *existing_path != path
                    && existing.get("kind") == journal.get("kind")
                    && existing.get("slot") == journal.get("slot")
                {
                    return Err(
                        "append-only history has the same pending operation under multiple journal paths"
                            .to_owned(),
                    );
                }
            }
            pending.insert(path, journal.clone());
        } else if kind == "operation-completed" {
            let path = operation_event_journal_path(cfg, machine, payload, None)?;
            if let Some(current) = pending.get(&path) {
                if payload.get("slot") == current.get("slot")
                    && payload.get("operation") == current.get("kind")
                {
                    pending.remove(&path);
                } else {
                    return Err(
                        "append-only completion does not match its pending operation".to_owned(),
                    );
                }
            }
        }
    }
    Ok(pending)
}

fn pending_journal_for_path(
    cfg: &Cfg,
    events_by_machine: &BTreeMap<String, Vec<Value>>,
    path: &Path,
) -> Result<Option<Map<String, Value>>> {
    let mut matches: Vec<Map<String, Value>> = Vec::new();
    for (machine, events) in events_by_machine {
        let pending = pending_operations(cfg, machine, events)?;
        if let Some(journal) = pending.get(path) {
            matches.push(journal.clone());
        }
    }
    if matches.len() > 1 {
        return Err(format!(
            "multiple append-only histories claim pending journal {}",
            path.display()
        ));
    }
    Ok(matches.into_iter().next())
}

fn scoped_operation_identity(journal: &Map<String, Value>, operation: &str) -> Option<String> {
    let field = if operation == "finish" {
        "fenced"
    } else {
        "operation_id"
    };
    journal
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn scoped_operation_opens(journal: &Map<String, Value>, operation: &str) -> bool {
    if operation == "finish" {
        journal.get("phase").and_then(Value::as_str) == Some("prepared")
    } else {
        journal
            .get("created")
            .is_some_and(|created| created.as_array().is_some_and(|items| items.is_empty()))
            && journal.get("hook_progress").is_some_and(|value| {
                matches!(value, Value::Bool(false)) || value.as_f64() == Some(0.0)
            })
    }
}

/// Whether a scoped journal's append-only episode completed,
/// refusing any disagreement with that evidence.
fn scoped_operation_completed(
    cfg: &Cfg,
    path: &Path,
    raw: &Map<String, Value>,
    operation: &str,
    events: &[Value],
) -> Result<bool> {
    let label = format!("scoped {operation} journal");
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        &format!("{operation} journal.machine"),
    )?;
    validate_name(
        as_str(
            raw.get("slot").unwrap_or(&Value::Null),
            &format!("{operation} journal.slot"),
        )?,
        "slot",
    )?;
    let mut recorded: Option<Map<String, Value>> = None;
    let mut current: Option<String> = None;
    let mut retired: BTreeSet<String> = BTreeSet::new();
    let mut completed = false;
    for event in events {
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
        if operation_event_journal_path(cfg, machine, payload, journal)? != path {
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
            } else if identity != current {
                return Err(format!("{label} progress changed its operation identity"));
            }
            recorded = Some(journal.clone());
            completed = false;
        } else {
            if recorded.is_none() {
                return Err(format!(
                    "scoped {operation} completion has no preceding progress evidence"
                ));
            }
            if let Some(identity) = current.take() {
                retired.insert(identity);
            }
            completed = true;
        }
    }
    let Some(recorded) = recorded else {
        return Err(format!(
            "{label} has no matching append-only progress evidence"
        ));
    };
    if !json_equal(&Value::Object(raw.clone()), &Value::Object(recorded)) {
        return Err(format!(
            "{label} differs from append-only progress evidence"
        ));
    }
    Ok(completed)
}

/// The exact ACTIVE row a completed scoped create produced.
fn completed_create_expected(config: &Value, raw: &Map<String, Value>) -> Result<Value> {
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        "create journal.machine",
    )?
    .to_owned();
    let slot = validate_name(
        as_str(
            raw.get("slot").unwrap_or(&Value::Null),
            "create journal.slot",
        )?,
        "slot",
    )?;
    let agent = validate_name(
        as_str(
            raw.get("agent").unwrap_or(&Value::Null),
            "create journal.agent",
        )?,
        "agent",
    )?;
    let default_slot_type = Value::String("agent".to_owned());
    let slot_type = as_str(
        raw.get("slot_type").unwrap_or(&default_slot_type),
        "create journal.slot_type",
    )?;
    if !matches!(slot_type, "agent" | "validate") {
        return Err("create journal has an invalid slot type".to_owned());
    }
    let planned = as_list(
        raw.get("planned").unwrap_or(&Value::Null),
        "create journal.planned",
    )?;
    let created = as_list(
        raw.get("created").unwrap_or(&Value::Null),
        "create journal.created",
    )?;
    for (index, item) in planned.iter().enumerate() {
        planned_from_obj(item, &format!("create journal.planned[{index}]"))?;
    }
    for (index, item) in created.iter().enumerate() {
        checkout_from_obj(item, &format!("create journal.created[{index}]"))?;
    }
    if created.len() != planned.len() {
        return Err("completed create journal has an incomplete checkout plan".to_owned());
    }
    for (plan, checkout) in planned.iter().zip(created.iter()) {
        let plan = as_mapping(plan, "create journal.planned")?;
        let checkout = as_mapping(checkout, "create journal.created")?;
        let fields = |map: &Map<String, Value>, keys: &[&str]| -> Vec<String> {
            keys.iter()
                .map(|key| {
                    map.get(*key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        };
        let checkout_fields = fields(
            checkout,
            &[
                "name",
                "path",
                "repository",
                "branch",
                "start_point",
                "remote",
                "remote_url_sha256",
                "landed_ref",
                "head",
            ],
        );
        let plan_fields = fields(
            plan,
            &[
                "name",
                "destination",
                "repository",
                "branch",
                "start_point",
                "remote",
                "remote_url_sha256",
                "landed_ref",
                "start_point",
            ],
        );
        if checkout_fields != plan_fields {
            return Err("completed create journal differs from its checkout plan".to_owned());
        }
    }
    let mut hooks: Vec<String> = Vec::new();
    for (index, item) in as_list(
        raw.get("post_provision_hooks")
            .unwrap_or(&Value::Array(Vec::new())),
        "create journal.post_provision_hooks",
    )?
    .iter()
    .enumerate()
    {
        let hook = as_str(
            item,
            &format!("create journal.post_provision_hooks[{index}]"),
        )?;
        validate_hook(hook)?;
        hooks.push(hook.to_owned());
    }
    let config_hooks: Vec<String> = config
        .get("post_provision_hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if hooks != config_hooks {
        return Err("create journal post-provision hooks differ from configuration".to_owned());
    }
    let hook_progress = as_int(
        raw.get("hook_progress").unwrap_or(&Value::from(0)),
        "create journal.hook_progress",
        0,
    )?;
    if hook_progress != (planned.len() * hooks.len()) as i64
        || !raw.get("hook_failure").is_none_or(Value::is_null)
    {
        return Err("completed create journal has unfinished post-provision hooks".to_owned());
    }
    let heartbeat_ttl = as_int(
        raw.get("heartbeat_ttl_seconds").unwrap_or(&Value::Null),
        "create journal.heartbeat_ttl_seconds",
        1,
    )?;
    let config_ttl = config
        .get("heartbeat_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if heartbeat_ttl != config_ttl {
        return Err("create journal heartbeat TTL differs from configuration".to_owned());
    }
    let owner = identity_from_obj(
        raw.get("owner").unwrap_or(&Value::Null),
        "create journal.owner",
    )?;
    let task_scope = if raw.contains_key("task_scope") {
        Some(scope_from_obj(
            raw.get("task_scope").unwrap_or(&Value::Null),
            "create journal.task_scope",
        )?)
    } else {
        None
    };
    if let Some(scope) = &task_scope {
        let Some(owner) = &owner else {
            return Err("create journal.task_scope has no bound owner process".to_owned());
        };
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
    let coordinator = identity_from_obj(
        raw.get("coordinator_lease").unwrap_or(&Value::Null),
        "create journal.coordinator_lease",
    )?;
    if coordinator.is_none() {
        return Err("create journal has no coordinator lease".to_owned());
    }
    let created_at = as_str(
        raw.get("created_at").unwrap_or(&Value::Null),
        "create journal.created_at",
    )?;
    parse_timestamp(created_at, "create journal.created_at")?;
    let heartbeat_at = as_str(
        raw.get("heartbeat_at").unwrap_or(&Value::Null),
        "create journal.heartbeat_at",
    )?;
    parse_timestamp(heartbeat_at, "create journal.heartbeat_at")?;
    let task = as_str(
        raw.get("task").unwrap_or(&Value::Null),
        "create journal.task",
    )?;
    let purpose = as_str(
        raw.get("purpose").unwrap_or(&Value::Null),
        "create journal.purpose",
    )?;
    if task.is_empty() || purpose.is_empty() {
        return Err("create journal has an empty task or purpose".to_owned());
    }
    let mut checkouts: Vec<Value> = Vec::new();
    for item in created {
        let checkout = as_mapping(item, "create journal.created")?;
        let mut object = Map::new();
        for key in [
            "name",
            "path",
            "repository",
            "branch",
            "start_point",
            "remote",
            "remote_url_sha256",
            "landed_ref",
            "head",
            "containing_remote_refs",
            "vcs",
        ] {
            object.insert(
                key.to_owned(),
                checkout.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        checkouts.push(Value::Object(object));
    }
    let mut expected = serde_json::json!({
        "slot": slot,
        "agent": agent,
        "task": task,
        "purpose": purpose,
        "slot_type": slot_type,
        "machine": machine,
        "generation": 1,
        "created_at": created_at,
        "heartbeat_at": heartbeat_at,
        "heartbeat_ttl_seconds": heartbeat_ttl,
        "owner": identity_to_obj(&owner),
        "coordinator_lease": identity_to_obj(&coordinator),
        "coordinator_recovery_note": Value::Null,
        "handoff": Value::Null,
        "checkouts": checkouts,
    });
    if task_scope.is_some() {
        expected.as_object_mut().expect("record object").insert(
            "task_scope".to_owned(),
            raw.get("task_scope").cloned().unwrap_or(Value::Null),
        );
    }
    Ok(expected)
}

fn scoped_create_completed(
    cfg: &Cfg,
    config: &Value,
    path: &Path,
    raw: &Map<String, Value>,
    events_by_machine: &BTreeMap<String, Vec<Value>>,
    records: &[Value],
) -> Result<()> {
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        "create journal.machine",
    )?
    .to_owned();
    let slot = validate_name(
        as_str(
            raw.get("slot").unwrap_or(&Value::Null),
            "create journal.slot",
        )?,
        "slot",
    )?;
    if path != journal_scoped_path(cfg, "CREATE", &machine, &slot) {
        return Err("scoped create journal filename does not match its target".to_owned());
    }
    let events = events_by_machine
        .get(&machine)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !scoped_operation_completed(cfg, path, raw, "create", events)? {
        return Ok(());
    }
    let expected = completed_create_expected(config, raw)?;
    let matches: Vec<&Value> = records
        .iter()
        .filter(|record| {
            record.get("machine").and_then(Value::as_str) == Some(machine.as_str())
                && record.get("slot").and_then(Value::as_str) == Some(slot.as_str())
        })
        .collect();
    if matches.len() != 1 {
        return Err("completed scoped create journal has no exact durable ACTIVE row".to_owned());
    }
    let actual = crate::active_state::record_from_obj(matches[0], "active record")?;
    if !json_equal(&expected, &actual.normalized) {
        return Err("completed scoped create journal has no exact durable ACTIVE row".to_owned());
    }
    Ok(())
}

fn validate_journal_provenance(
    cfg: &Cfg,
    config: &Value,
    path: &Path,
    raw: &Map<String, Value>,
    events_by_machine: &BTreeMap<String, Vec<Value>>,
    records: &[Value],
) -> Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (machine, kind, _slot) = validate_journal_shape(cfg, &name, raw)?;
    if path == journal_legacy_path(cfg, &machine) {
        if let Some(pending) = pending_journal_for_path(cfg, events_by_machine, path)? {
            if !json_equal(&Value::Object(raw.clone()), &Value::Object(pending)) {
                return Err(
                    "standalone recovery journal differs from append-only operation evidence"
                        .to_owned(),
                );
            }
        }
        return Ok(());
    }
    match kind.as_str() {
        "create" => scoped_create_completed(cfg, config, path, raw, events_by_machine, records),
        "finish" => {
            let slot = validate_name(
                as_str(
                    raw.get("slot").unwrap_or(&Value::Null),
                    "finish journal.slot",
                )?,
                "slot",
            )?;
            if path != journal_scoped_path(cfg, "FINISH", &machine, &slot) {
                return Err("scoped finish journal filename does not match its target".to_owned());
            }
            let events = events_by_machine
                .get(&machine)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            scoped_operation_completed(cfg, path, raw, "finish", events)?;
            Ok(())
        }
        _ => Err("only create and finish journals may use a scoped journal path".to_owned()),
    }
}

/// Every outstanding journal in the registry, shape- and
/// provenance-validated, keyed by slot.
pub(crate) fn validated_journals(
    cfg: &Cfg,
    config: &Value,
    events_by_machine: &BTreeMap<String, Vec<Value>>,
    records: &[Value],
) -> Result<BTreeMap<String, ValidatedJournal>> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    {
        use std::os::unix::ffi::OsStrExt as _;
        let listing = std::fs::read_dir(&cfg.control).map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                cfg.control.display()
            )
        })?;
        for entry in listing {
            let entry = entry.map_err(|error| format!("cannot list control directory: {error}"))?;
            let bytes = entry.file_name();
            let bytes = bytes.as_bytes();
            if (bytes.starts_with(b"ACTIVE.")
                || bytes.starts_with(b"CREATE.")
                || bytes.starts_with(b"FINISH."))
                && bytes.ends_with(b".journal")
            {
                paths.insert(entry.path());
            }
        }
    }
    for (machine, events) in events_by_machine {
        for path in pending_operations(cfg, machine, events)?.into_keys() {
            paths.insert(path);
        }
    }
    // Validate every journal first; duplicates are detected
    // only after all of them pass, as in the oracle.
    let mut validated: Vec<(String, String, String, Map<String, Value>)> = Vec::new();
    for path in &paths {
        let raw = if path.symlink_metadata().is_ok() {
            let (contents, _identity) =
                read_regular_file_identity(path, "cache recovery journal", 16 * 1024 * 1024)?;
            strict_json_object(&contents, "cache recovery journal")?
        } else {
            match pending_journal_for_path(cfg, events_by_machine, path)? {
                Some(journal) => journal,
                None => {
                    return Err(format!(
                        "journal has neither standalone nor append-only evidence: {}",
                        path.display()
                    ));
                }
            }
        };
        validate_journal_provenance(cfg, config, path, &raw, events_by_machine, records)?;
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
        if !matches!(kind.as_str(), "create" | "import-existing" | "finish") {
            return Err(format!("unsupported cache recovery journal kind '{kind}'"));
        }
        let slot = validate_name(
            as_str(
                raw.get("slot").unwrap_or(&Value::Null),
                "recovery journal.slot",
            )?,
            "slot",
        )?;
        validated.push((slot, machine, kind, raw));
    }
    let mut journals: BTreeMap<String, ValidatedJournal> = BTreeMap::new();
    let mut machines_by_slot: BTreeMap<String, String> = BTreeMap::new();
    for (slot, machine, kind, raw) in validated {
        if let Some(previous) = machines_by_slot.get(&slot) {
            return Err(format!(
                "slot '{slot}' has recovery journals on both {previous} and {machine}"
            ));
        }
        machines_by_slot.insert(slot.clone(), machine.clone());
        journals.insert(
            slot.clone(),
            ValidatedJournal {
                slot,
                machine,
                state: format!("{kind}-journal"),
                raw: Value::Object(raw),
            },
        );
    }
    Ok(journals)
}
