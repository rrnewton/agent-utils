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

use crate::active_state::load_cfg;
use crate::active_state::{
    as_int, as_list, as_mapping, as_str, checkout_from_obj, exact_keys, identity_from_obj,
    is_digest, is_sha, parse_timestamp, record_from_obj, scope_from_obj, validate_ref,
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

fn removal_proof(value: &Value) -> Result<(String, i64)> {
    let label = "validation removal proof";
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
        let (proof_slot, proof_generation) = removal_proof(proof)?;
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
        _ => return Err(format!("unsupported recovery journal kind {kind:?}")),
    }
    Ok(Value::Object(raw.clone()))
}
