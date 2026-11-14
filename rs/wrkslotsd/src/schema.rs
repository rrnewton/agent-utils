use std::collections::BTreeSet;
use std::path::{Component, Path};

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use serde_json::{Map, Value};

use crate::canonical::canonical_sha256;
use crate::evidence::ScopeIdentity;
use crate::replay::validate_name;
use crate::ObserverError;

/// Required and optional keys of each record the observer schema and the
/// writer/reader schemas both validate (#1517). `active_state.rs` uses
/// these same tables, and a test below compares them against the Python
/// writer in `py/wrkslots/cli.py`, so the three cannot drift apart.
pub(crate) const ACTIVE_RECORD_REQUIRED: &[&str] = &[
    "slot",
    "agent",
    "task",
    "purpose",
    "machine",
    "generation",
    "created_at",
    "heartbeat_at",
    "heartbeat_ttl_seconds",
    "owner",
    "coordinator_lease",
    "coordinator_recovery_note",
    "handoff",
    "checkouts",
];
pub(crate) const ACTIVE_RECORD_OPTIONAL: &[&str] = &[
    "slot_type",
    "layout",
    "import_source",
    "task_scope",
    // Recorded by create for harness-checkout slots (#1299).
    "harness_checkout_reason",
];
pub(crate) const ARCHIVE_RECORD_REQUIRED: &[&str] = &[
    "archive_id",
    "slot",
    "agent",
    "task",
    "purpose",
    "machine",
    "generation",
    "created_at",
    "finished_at",
    "mode",
    "actor",
    "physical_storage",
    "validation",
    "limitations",
    "continuation",
    "checkouts",
];
pub(crate) const ARCHIVE_RECORD_OPTIONAL: &[&str] =
    &["slot_type", "salvage", "layout", "import_source"];
pub(crate) const IDENTITY_REQUIRED: &[&str] =
    &["pid", "start_ticks", "boot_id", "host_id", "cgroup_path"];
pub(crate) const IDENTITY_OPTIONAL: &[&str] = &["namespace_pid", "pidns_inode"];
pub(crate) const HANDOFF_REQUIRED: &[&str] =
    &["recorded_at", "validation", "limitations", "continuation"];
pub(crate) const HANDOFF_OPTIONAL: &[&str] = &["discarded_unaccounted"];
pub(crate) const CHECKOUT_REQUIRED: &[&str] = &[
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
];
// Create records provisioned output on the checkout (#1369); the observer
// accepts the additive field.
pub(crate) const CHECKOUT_OPTIONAL: &[&str] = &["provisioned"];
pub(crate) const IMPORT_SOURCE_REQUIRED: &[&str] = &[
    "format",
    "path",
    "file_sha256",
    "row_sha256",
    "status",
    "row",
];
pub(crate) const TASK_SCOPE_REQUIRED: &[&str] = &[
    "unit",
    "invocation_id",
    "cgroup_path",
    "boot_id",
    "leader_pid",
    "leader_start_ticks",
    "verification",
];

const ACTIVE_STATUSES: &[&str] = &[
    "active",
    "lease-quarantined",
    "owner-lease-revoked",
    "release-requested",
];
const HISTORICAL_TASK_NOT_RECORDED: &str = "no task recorded in worktree-state.json";
const HISTORICAL_PURPOSE_NOT_RECORDED: &str = "no purpose recorded in worktree-state.json";

#[derive(Clone, Debug)]
pub(crate) struct ActiveRecordMeta {
    pub(crate) slot: String,
    pub(crate) agent: String,
    pub(crate) slot_type: String,
    pub(crate) generation: u64,
    pub(crate) imported: bool,
    pub(crate) heartbeat_at: String,
    pub(crate) heartbeat_ttl_seconds: u64,
    pub(crate) task_scope: Option<ScopeIdentity>,
    pub(crate) checkouts: Vec<(String, String)>,
}

pub(crate) struct ValidatedActiveRecord {
    pub(crate) meta: ActiveRecordMeta,
    pub(crate) normalized: Value,
}

impl ActiveRecordMeta {
    pub(crate) fn has_unique_agent(&self) -> bool {
        self.slot_type == "agent" && !self.imported
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ArchiveRecordMeta {
    pub(crate) archive_id: String,
    pub(crate) slot: String,
    pub(crate) generation: u64,
    pub(crate) slot_type: String,
    pub(crate) physical_storage: String,
}

#[derive(Clone, Debug)]
struct Identity {
    pid: u64,
    start_ticks: u64,
    boot_id: String,
    cgroup_path: String,
}

#[derive(Clone, Debug)]
struct Checkout {
    name: String,
    path: String,
}

#[derive(Clone, Debug)]
struct ImportSource {
    row: Map<String, Value>,
}

pub(crate) fn validate_active_record(
    value: &Value,
    machine: &str,
    label: &str,
) -> Result<ValidatedActiveRecord, ObserverError> {
    let record = object(value, label)?;
    exact_keys_optional(
        record,
        ACTIVE_RECORD_REQUIRED,
        ACTIVE_RECORD_OPTIONAL,
        label,
    )?;
    let slot = named_string(&record["slot"], &format!("{label}.slot"))?;
    let agent = named_string(&record["agent"], &format!("{label}.agent"))?;
    nonempty_string(&record["task"], &format!("{label}.task"))?;
    nonempty_string(&record["purpose"], &format!("{label}.purpose"))?;
    if string(&record["machine"], &format!("{label}.machine"))? != machine {
        return Err(invalid(format!("{label} belongs to another machine")));
    }
    let generation = positive(&record["generation"], &format!("{label}.generation"))?;
    parse_timestamp(
        string(&record["created_at"], &format!("{label}.created_at"))?,
        &format!("{label}.created_at"),
    )?;
    let heartbeat_at = string(&record["heartbeat_at"], &format!("{label}.heartbeat_at"))?;
    parse_timestamp(heartbeat_at, &format!("{label}.heartbeat_at"))?;
    let heartbeat_ttl_seconds = positive(
        &record["heartbeat_ttl_seconds"],
        &format!("{label}.heartbeat_ttl_seconds"),
    )?;
    let owner = validate_identity(&record["owner"], &format!("{label}.owner"))?;
    if validate_identity(
        &record["coordinator_lease"],
        &format!("{label}.coordinator_lease"),
    )?
    .is_none()
    {
        return Err(invalid(format!("{label} has no coordinator lease")));
    }
    match &record["coordinator_recovery_note"] {
        Value::Null => {}
        Value::String(_) if owner.is_none() => {}
        Value::String(_) => {
            return Err(invalid(format!(
                "{label} has coordinator recovery evidence for a bound owner"
            )))
        }
        _ => {
            return Err(invalid(format!(
                "{label}.coordinator_recovery_note must be a string or null"
            )))
        }
    }
    validate_handoff(&record["handoff"], &format!("{label}.handoff"))?;
    let checkouts = validate_checkouts(&record["checkouts"], label)?;
    let slot_type = optional_string(record, "slot_type", "agent", label)?;
    if !matches!(slot_type, "agent" | "validate") {
        return Err(invalid(format!(
            "{label}.slot_type must be agent or validate"
        )));
    }
    let layout = optional_layout(record, label)?;
    let import_source = validate_import_source(record.get("import_source"), label)?;
    let task_scope = record
        .get("task_scope")
        .map(|value| {
            serde_json::from_value::<ScopeIdentity>(value.clone())
                .map_err(|error| invalid(format!("{label}.task_scope is invalid: {error}")))
                .and_then(|identity| {
                    identity.validate(&format!("{label}.task_scope"))?;
                    Ok(identity)
                })
        })
        .transpose()?;
    if let Some(scope) = &task_scope {
        let owner = owner
            .as_ref()
            .ok_or_else(|| invalid(format!("{label}.task_scope has no bound owner process")))?;
        if (scope.leader_pid, scope.leader_start_ticks) != (owner.pid, owner.start_ticks)
            || scope.boot_id != owner.boot_id
            || scope.cgroup_path != owner.cgroup_path
        {
            return Err(invalid(format!(
                "{label}.task_scope does not match its bound owner process"
            )));
        }
    }
    if checkouts.is_empty() && import_source.is_none() {
        return Err(invalid(format!("{label} has no checkouts")));
    }
    if checkouts.is_empty() && layout.as_deref() != Some("nested") {
        return Err(invalid(format!(
            "{label} has no checkouts but is not an explicit historical nested slot"
        )));
    }
    if layout.as_deref() == Some("flat") && checkouts.len() != 1 {
        return Err(invalid(format!(
            "{label} has multiple checkouts under flat layout"
        )));
    }
    if let Some(source) = &import_source {
        validate_active_import(
            record,
            owner.as_ref(),
            &checkouts,
            source,
            &slot,
            &agent,
            slot_type,
            layout.as_deref(),
            label,
        )?;
    }
    let mut normalized = record.clone();
    normalized
        .entry("slot_type".to_owned())
        .or_insert_with(|| Value::String("agent".to_owned()));
    if import_source.is_none() {
        normalized.remove("import_source");
    }
    Ok(ValidatedActiveRecord {
        meta: ActiveRecordMeta {
            slot,
            agent,
            slot_type: slot_type.to_owned(),
            generation,
            imported: import_source.is_some(),
            heartbeat_at: heartbeat_at.to_owned(),
            heartbeat_ttl_seconds,
            task_scope,
            checkouts: checkouts
                .iter()
                .map(|checkout| (checkout.name.clone(), checkout.path.clone()))
                .collect(),
        },
        normalized: Value::Object(normalized),
    })
}

pub(crate) fn validate_archive_record(
    value: &Value,
    machine: &str,
    label: &str,
) -> Result<ArchiveRecordMeta, ObserverError> {
    let record = object(value, label)?;
    exact_keys_optional(
        record,
        ARCHIVE_RECORD_REQUIRED,
        ARCHIVE_RECORD_OPTIONAL,
        label,
    )?;
    let slot = named_string(&record["slot"], &format!("{label}.slot"))?;
    let agent = named_string(&record["agent"], &format!("{label}.agent"))?;
    nonempty_string(&record["task"], &format!("{label}.task"))?;
    nonempty_string(&record["purpose"], &format!("{label}.purpose"))?;
    if string(&record["machine"], &format!("{label}.machine"))? != machine {
        return Err(invalid(format!("{label} belongs to another machine")));
    }
    let generation = positive(&record["generation"], &format!("{label}.generation"))?;
    parse_timestamp(
        string(&record["created_at"], &format!("{label}.created_at"))?,
        &format!("{label}.created_at"),
    )?;
    let finished_at = string(&record["finished_at"], &format!("{label}.finished_at"))?;
    parse_timestamp(finished_at, &format!("{label}.finished_at"))?;
    let archive_id = string(&record["archive_id"], &format!("{label}.archive_id"))?;
    let expected_archive_id = format!("{machine}:{slot}:{generation}:{finished_at}");
    if archive_id != expected_archive_id {
        return Err(invalid(format!(
            "{label}.archive_id does not match its record"
        )));
    }
    if string(&record["mode"], &format!("{label}.mode"))? != "remove" {
        return Err(invalid(format!("{label}.mode is invalid")));
    }
    if string(&record["actor"], &format!("{label}.actor"))? != "coordinator" {
        return Err(invalid(format!("{label}.actor is invalid")));
    }
    let physical_storage = string(
        &record["physical_storage"],
        &format!("{label}.physical_storage"),
    )?;
    if physical_storage != "removed" {
        return Err(invalid(format!(
            "{label} does not record removed physical storage"
        )));
    }
    let validation = string_array(&record["validation"], &format!("{label}.validation"))?;
    if validation.is_empty() {
        return Err(invalid(format!("{label} has no validation evidence")));
    }
    string_array(&record["limitations"], &format!("{label}.limitations"))?;
    nonempty_string(&record["continuation"], &format!("{label}.continuation"))?;
    let slot_type = optional_string(record, "slot_type", "agent", label)?;
    if !matches!(slot_type, "agent" | "validate") {
        return Err(invalid(format!(
            "{label}.slot_type must be agent or validate"
        )));
    }
    let layout = optional_layout(record, label)?;
    if let Some(salvage) = record.get("salvage") {
        for (index, item) in array(salvage, &format!("{label}.salvage"))?
            .iter()
            .enumerate()
        {
            object(item, &format!("{label}.salvage[{index}]"))?;
        }
    }
    let checkouts = validate_checkouts(&record["checkouts"], label)?;
    let import_source = validate_import_source(record.get("import_source"), label)?;
    if checkouts.is_empty() && (import_source.is_none() || layout.as_deref() == Some("flat")) {
        return Err(invalid(format!("{label} has no checkouts")));
    }
    if layout.as_deref() == Some("flat") && checkouts.len() != 1 {
        return Err(invalid(format!(
            "{label} has multiple checkouts under flat layout"
        )));
    }
    if let Some(source) = &import_source {
        validate_archive_import(record, source, &slot, &agent, layout.as_deref(), label)?;
    }
    Ok(ArchiveRecordMeta {
        archive_id: archive_id.to_owned(),
        slot,
        generation,
        slot_type: slot_type.to_owned(),
        physical_storage: physical_storage.to_owned(),
    })
}

fn validate_identity(value: &Value, label: &str) -> Result<Option<Identity>, ObserverError> {
    if value.is_null() {
        return Ok(None);
    }
    let identity = object(value, label)?;
    exact_keys_optional(identity, IDENTITY_REQUIRED, IDENTITY_OPTIONAL, label)?;
    let pid = positive(&identity["pid"], &format!("{label}.pid"))?;
    let start_ticks = positive(&identity["start_ticks"], &format!("{label}.start_ticks"))?;
    let boot_id = nonempty_string(&identity["boot_id"], &format!("{label}.boot_id"))?;
    nonempty_string(&identity["host_id"], &format!("{label}.host_id"))?;
    let cgroup_path = string(&identity["cgroup_path"], &format!("{label}.cgroup_path"))?;
    if !cgroup_path.starts_with('/') {
        return Err(invalid(format!("{label} has an invalid cgroup identity")));
    }
    Ok(Some(Identity {
        pid,
        start_ticks,
        boot_id: boot_id.to_owned(),
        cgroup_path: cgroup_path.to_owned(),
    }))
}

fn validate_handoff(value: &Value, label: &str) -> Result<(), ObserverError> {
    if value.is_null() {
        return Ok(());
    }
    let handoff = object(value, label)?;
    exact_keys_optional(handoff, HANDOFF_REQUIRED, HANDOFF_OPTIONAL, label)?;
    if let Some(discarded) = handoff.get("discarded_unaccounted") {
        for (index, item) in array(discarded, &format!("{label}.discarded_unaccounted"))?
            .iter()
            .enumerate()
        {
            digest(item, 40, &format!("{label}.discarded_unaccounted[{index}]"))?;
        }
    }
    parse_timestamp(
        string(&handoff["recorded_at"], &format!("{label}.recorded_at"))?,
        &format!("{label}.recorded_at"),
    )?;
    let validation = string_array(&handoff["validation"], &format!("{label}.validation"))?;
    string_array(&handoff["limitations"], &format!("{label}.limitations"))?;
    let continuation = string(&handoff["continuation"], &format!("{label}.continuation"))?;
    if validation.is_empty() || continuation.is_empty() {
        return Err(invalid(format!(
            "{label} requires validation evidence and an exact continuation"
        )));
    }
    Ok(())
}

fn validate_checkouts(value: &Value, parent: &str) -> Result<Vec<Checkout>, ObserverError> {
    let values = array(value, &format!("{parent}.checkouts"))?;
    let mut result = Vec::with_capacity(values.len());
    let mut names = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        let label = format!("{parent}.checkouts[{index}]");
        let checkout = object(value, &label)?;
        exact_keys_optional(checkout, CHECKOUT_REQUIRED, CHECKOUT_OPTIONAL, &label)?;
        let name = named_string(&checkout["name"], &format!("{label}.name"))?;
        if !names.insert(name.clone()) {
            return Err(invalid(format!("{parent} has duplicate checkout names")));
        }
        let path = string(&checkout["path"], &format!("{label}.path"))?;
        validate_relative_path(path, &format!("{label}.path"))?;
        string(&checkout["repository"], &format!("{label}.repository"))?;
        let branch = string(&checkout["branch"], &format!("{label}.branch"))?;
        let remote = string(&checkout["remote"], &format!("{label}.remote"))?;
        let landed_ref = string(&checkout["landed_ref"], &format!("{label}.landed_ref"))?;
        let containing = string_array(
            &checkout["containing_remote_refs"],
            &format!("{label}.containing_remote_refs"),
        )?;
        let vcs = string(&checkout["vcs"], &format!("{label}.vcs"))?;
        if vcs != "git" && vcs != "eden" {
            return Err(invalid(format!("unsupported VCS in {label}")));
        }
        digest(
            &checkout["start_point"],
            40,
            &format!("{label}.start_point"),
        )?;
        digest(&checkout["head"], 40, &format!("{label}.head"))?;
        if vcs == "eden" {
            if branch != "eden" || remote != "eden" {
                return Err(invalid(format!("{label} Eden markers must be 'eden'")));
            }
            if landed_ref != "refs/remotes/eden/main" {
                return Err(invalid(format!(
                    "{label} Eden landed ref must be the fixed marker"
                )));
            }
            if !containing.is_empty() {
                return Err(invalid(format!(
                    "{label} Eden rows name no containing refs"
                )));
            }
            result.push(Checkout {
                name,
                path: path.to_owned(),
            });
            continue;
        }
        validate_ref(branch, &format!("{label}.branch"))?;
        validate_remote(remote, &format!("{label}.remote"))?;
        digest(
            &checkout["remote_url_sha256"],
            64,
            &format!("{label}.remote_url_sha256"),
        )?;
        validate_full_ref(landed_ref, &format!("{label}.landed_ref"))?;
        let prefix = format!("refs/remotes/{remote}/");
        if containing
            .iter()
            .any(|reference| !reference.starts_with(&prefix))
        {
            return Err(invalid(format!(
                "{label}.containing_remote_refs names an unauthorized ref"
            )));
        }
        result.push(Checkout {
            name,
            path: path.to_owned(),
        });
    }
    Ok(result)
}

fn validate_import_source(
    value: Option<&Value>,
    parent: &str,
) -> Result<Option<ImportSource>, ObserverError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let label = format!("{parent}.import_source");
    let source = object(value, &label)?;
    exact_keys(source, IMPORT_SOURCE_REQUIRED, &label)?;
    if string(&source["format"], &format!("{label}.format"))? != "worktree-state-v3" {
        return Err(invalid(format!("{label}.format is unsupported")));
    }
    validate_relative_path(
        string(&source["path"], &format!("{label}.path"))?,
        &format!("{label}.path"),
    )?;
    digest(&source["file_sha256"], 64, &format!("{label}.file_sha256"))?;
    let row_sha256 = digest(&source["row_sha256"], 64, &format!("{label}.row_sha256"))?;
    let status = string(&source["status"], &format!("{label}.status"))?;
    if !ACTIVE_STATUSES.contains(&status) {
        return Err(invalid(format!("{label}.status is not active")));
    }
    let row = object(&source["row"], &format!("{label}.row"))?.clone();
    if canonical_sha256(&Value::Object(row.clone()))? != row_sha256 {
        return Err(invalid(format!(
            "{label}.row digest does not match its content"
        )));
    }
    if row.get("status") != Some(&Value::String(status.to_owned())) {
        return Err(invalid(format!(
            "{label}.status does not match its source row"
        )));
    }
    Ok(Some(ImportSource { row }))
}

#[allow(clippy::too_many_arguments)]
fn validate_active_import(
    record: &Map<String, Value>,
    owner: Option<&Identity>,
    checkouts: &[Checkout],
    source: &ImportSource,
    slot: &str,
    agent: &str,
    slot_type: &str,
    layout: Option<&str>,
    label: &str,
) -> Result<(), ObserverError> {
    let owner = owner.ok_or_else(|| invalid(format!("{label} import has no exact owner")))?;
    if slot_type != "agent" || layout != Some("nested") {
        return Err(invalid(format!(
            "{label} historical import must be an explicit nested agent slot"
        )));
    }
    let (task, purpose) = historical_task_and_purpose(&source.row, agent, label)?;
    if record["task"] != Value::String(task) || record["purpose"] != Value::String(purpose) {
        return Err(invalid(format!(
            "{label} differs from its historical task or purpose"
        )));
    }
    if source.row.get("allocated") != record.get("created_at") {
        return Err(invalid(format!(
            "{label} differs from its historical allocation time"
        )));
    }
    let sidecar = object(
        source
            .row
            .get("owner_sidecar")
            .ok_or_else(|| invalid(format!("{label} source row has no owner_sidecar")))?,
        &format!("{label}.import_source.row.owner_sidecar"),
    )?;
    if sidecar.get("supervisor_pid") != Some(&Value::from(owner.pid))
        || sidecar.get("start_ticks") != Some(&Value::from(owner.start_ticks))
        || sidecar.get("boot_id") != Some(&Value::String(owner.boot_id.clone()))
        || sidecar.get("cgroup_path") != Some(&Value::String(owner.cgroup_path.clone()))
        || sidecar.get("slot") != Some(&Value::String(slot.to_owned()))
        || sidecar.get("agent") != Some(&Value::String(agent.to_owned()))
    {
        return Err(invalid(format!(
            "{label} differs from its historical owner identity"
        )));
    }
    for checkout in checkouts {
        if source.row.get(&format!("{}_path", checkout.name))
            != Some(&Value::String(checkout.path.clone()))
        {
            return Err(invalid(format!(
                "{label} checkout {} differs from its historical path",
                checkout.name
            )));
        }
    }
    Ok(())
}

fn validate_archive_import(
    record: &Map<String, Value>,
    source: &ImportSource,
    slot: &str,
    agent: &str,
    layout: Option<&str>,
    label: &str,
) -> Result<(), ObserverError> {
    if layout == Some("flat") {
        return Err(invalid(format!(
            "{label} historical import cannot use flat layout"
        )));
    }
    let (task, purpose) = historical_task_and_purpose(&source.row, agent, label)?;
    let sidecar = object(
        source
            .row
            .get("owner_sidecar")
            .ok_or_else(|| invalid(format!("{label} source row has no owner_sidecar")))?,
        &format!("{label}.import_source.row.owner_sidecar"),
    )?;
    if record["task"] != Value::String(task)
        || record["purpose"] != Value::String(purpose)
        || source.row.get("allocated") != record.get("created_at")
        || sidecar.get("slot") != Some(&Value::String(slot.to_owned()))
        || sidecar.get("agent") != Some(&Value::String(agent.to_owned()))
    {
        return Err(invalid(format!(
            "{label} differs from its historical source row"
        )));
    }
    Ok(())
}

fn historical_task_and_purpose(
    row: &Map<String, Value>,
    agent: &str,
    label: &str,
) -> Result<(String, String), ObserverError> {
    let sidecar = object(
        row.get("owner_sidecar")
            .ok_or_else(|| invalid(format!("{label} source row has no owner_sidecar")))?,
        &format!("{label}.owner_sidecar"),
    )?;
    let agents = array(
        row.get("agents")
            .ok_or_else(|| invalid(format!("{label} source row has no agents")))?,
        &format!("{label}.agents"),
    )?;
    let mut writable_tasks = BTreeSet::new();
    let mut found_writable = false;
    for (index, item) in agents.iter().enumerate() {
        let item = object(item, &format!("{label}.agents[{index}]"))?;
        if item.get("name") == Some(&Value::String(agent.to_owned()))
            && item.get("read_only") == Some(&Value::Bool(false))
        {
            found_writable = true;
            if let Some(value) = item.get("task") {
                if !value.is_null() {
                    let task = string(value, &format!("{label}.agents[{index}].task"))?;
                    if !task.is_empty() {
                        writable_tasks.insert(task.to_owned());
                    }
                }
            }
        }
    }
    if !found_writable {
        return Err(invalid(format!(
            "{label} does not list owner {agent} as a writable agent"
        )));
    }
    if writable_tasks.len() > 1 {
        return Err(invalid(format!(
            "{label} records conflicting writable-agent task values"
        )));
    }
    let row_task = nullable_string(row.get("task"), &format!("{label}.task"))?;
    let owner_task = nullable_string(sidecar.get("task"), &format!("{label}.owner_sidecar.task"))?;
    let task = if !row_task.is_empty() {
        row_task
    } else if !owner_task.is_empty() {
        owner_task
    } else {
        writable_tasks
            .into_iter()
            .next()
            .unwrap_or_else(|| HISTORICAL_TASK_NOT_RECORDED.to_owned())
    };
    let purpose = match row.get("purpose") {
        None | Some(Value::Null) => HISTORICAL_PURPOSE_NOT_RECORDED.to_owned(),
        Some(value) => {
            let value = string(value, &format!("{label}.purpose"))?;
            if value.is_empty() {
                HISTORICAL_PURPOSE_NOT_RECORDED.to_owned()
            } else {
                value.to_owned()
            }
        }
    };
    Ok((task, purpose))
}

pub(crate) fn parse_timestamp(value: &str, label: &str) -> Result<(), ObserverError> {
    parse_timestamp_instant(value, label).map(|_| ())
}

/// Parse a timestamp accepted by [`parse_timestamp`] into the UTC instant that
/// CPython's `datetime.fromisoformat` assigns to it, so ages are computed from
/// exactly the strings that replay admits.
pub(crate) fn parse_timestamp_instant(
    value: &str,
    label: &str,
) -> Result<DateTime<Utc>, ObserverError> {
    python_aware_iso_instant(value)
        .ok_or_else(|| invalid(format!("invalid or timezone-naive timestamp in {label}")))
}

/// Validate the supported `datetime.fromisoformat` grammar explicitly instead
/// of composing Chrono format strings. In particular, Chrono's permissive
/// `%#z` accepts lowercase `z` and malformed short offsets that CPython rejects.
/// Values are extracted only after the grammar accepts the string.
fn python_aware_iso_instant(value: &str) -> Option<DateTime<Utc>> {
    let date_length = python_iso_date_length(value)?;
    let separator = value
        .get(date_length..)
        .and_then(|tail| tail.chars().next())?;
    let time_and_zone = value.get(date_length + separator.len_utf8()..)?;
    let zone_start = time_and_zone.find(['+', '-', 'Z'])?;
    let (time, zone) = time_and_zone.split_at(zone_start);
    if !(valid_iso_time(time) && valid_iso_offset(zone)) {
        return None;
    }
    let date = python_iso_date(value.get(..date_length)?)?;
    let (hour, minute, second, microsecond) = python_clock_value(time.as_bytes())?;
    let offset_microseconds = if zone == "Z" {
        0
    } else if let Some(body) = zone.strip_prefix('-') {
        -python_offset_microseconds(body)?
    } else {
        python_offset_microseconds(zone.strip_prefix('+')?)?
    };
    let local = date.and_hms_micro_opt(hour, minute, second, microsecond)?;
    Some(
        local
            .checked_sub_signed(Duration::microseconds(offset_microseconds))?
            .and_utc(),
    )
}

fn python_offset_microseconds(body: &str) -> Option<i64> {
    let (hours, minutes, seconds, microseconds) = python_clock_value(body.as_bytes())?;
    let whole_seconds = i64::from(hours) * 3_600 + i64::from(minutes) * 60 + i64::from(seconds);
    // CPython's `tzinfo_from_isoformat_results` returns UTC for a zero
    // whole-second offset and discards its fractional seconds.
    if whole_seconds == 0 {
        return Some(0);
    }
    Some(whole_seconds * 1_000_000 + i64::from(microseconds))
}

/// Mirror the value assignment of CPython's `parse_hh_mm_ss_ff` for a clock the
/// grammar has already accepted: a component followed by one final character
/// ends the clock with no fraction, and a fraction keeps its first six digits.
fn python_clock_value(value: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let mut components = [0_u32; 3];
    let mut position = 0;
    let mut has_separator = true;
    for (index, component) in components.iter_mut().enumerate() {
        *component = ascii_decimal(value.get(position..position + 2)?)?;
        position += 2;
        let separator = value.get(position).copied();
        position += 1;
        if index == 0 {
            has_separator = separator == Some(b':');
        }
        if position >= value.len() {
            return Some((components[0], components[1], components[2], 0));
        }
        match separator {
            Some(b':') if has_separator => {}
            Some(b'.' | b',') => break,
            _ if !has_separator => position -= 1,
            _ => return None,
        }
    }
    let digits = value.len().checked_sub(position)?.min(6);
    let fraction = ascii_decimal(value.get(position..position + digits)?)?;
    let scale = 10_u32.pow(u32::try_from(6 - digits).ok()?);
    Some((
        components[0],
        components[1],
        components[2],
        fraction * scale,
    ))
}

fn python_iso_date_length(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let length = if bytes.len() == 7 {
        7
    } else if bytes.len() < 8 {
        return None;
    } else if bytes.get(4) == Some(&b'-') {
        if bytes.get(5) == Some(&b'W') {
            if bytes.get(8) == Some(&b'-') {
                if bytes.len() == 9 {
                    return None;
                }
                if bytes.get(10).is_some_and(u8::is_ascii_digit) {
                    // Match CPython's documented best-effort resolution of
                    // `YYYY-Www-##`: the hyphen is treated as the separator.
                    8
                } else {
                    10
                }
            } else {
                8
            }
        } else {
            10
        }
    } else if bytes.get(4) == Some(&b'W') {
        let mut offset = 7;
        while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if offset < 9 {
            offset
        } else if offset % 2 == 0 {
            7
        } else {
            8
        }
    } else {
        8
    };
    if !value.is_char_boundary(length) {
        return None;
    }
    python_iso_date(value.get(..length)?).map(|_| length)
}

fn python_iso_date(value: &str) -> Option<NaiveDate> {
    let bytes = value.as_bytes();
    let year = bytes.get(..4).and_then(ascii_decimal)?;
    if year == 0 {
        return None;
    }

    if bytes.get(4) == Some(&b'-') && bytes.get(5) == Some(&b'W') {
        let week = bytes.get(6..8).and_then(ascii_decimal)?;
        let weekday = if bytes.get(8) == Some(&b'-') {
            bytes.get(9..10).and_then(ascii_decimal)?
        } else {
            1
        };
        if !matches!(bytes.len(), 8 | 10) {
            return None;
        }
        iso_week_date(year, week, weekday)
    } else if bytes.get(4) == Some(&b'W') {
        let week = bytes.get(5..7).and_then(ascii_decimal)?;
        let (length, weekday) = if bytes.get(7).is_some_and(u8::is_ascii_digit) {
            (8, bytes.get(7..8).and_then(ascii_decimal)?)
        } else {
            (7, 1)
        };
        if bytes.len() != length {
            return None;
        }
        iso_week_date(year, week, weekday)
    } else if bytes.get(4) == Some(&b'-') && bytes.get(7) == Some(&b'-') {
        let month = bytes.get(5..7).and_then(ascii_decimal)?;
        let day = bytes.get(8..10).and_then(ascii_decimal)?;
        if bytes.len() != 10 {
            return None;
        }
        NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)
    } else {
        let month = bytes.get(4..6).and_then(ascii_decimal)?;
        let day = bytes.get(6..8).and_then(ascii_decimal)?;
        if bytes.len() != 8 {
            return None;
        }
        NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)
    }
}

fn iso_week_date(year: u32, week: u32, weekday: u32) -> Option<NaiveDate> {
    let weekday = match weekday {
        1 => Weekday::Mon,
        2 => Weekday::Tue,
        3 => Weekday::Wed,
        4 => Weekday::Thu,
        5 => Weekday::Fri,
        6 => Weekday::Sat,
        7 => Weekday::Sun,
        _ => return None,
    };
    NaiveDate::from_isoywd_opt(i32::try_from(year).ok()?, week, weekday)
        .filter(|date| (1..=9_999).contains(&date.year()))
}

fn valid_iso_time(value: &str) -> bool {
    let Some((hour, minute, second)) = python_iso_local_time_components(value) else {
        return false;
    };
    hour < 24 && minute < 60 && second < 60
}

fn valid_iso_offset(value: &str) -> bool {
    if value == "Z" {
        return true;
    }
    let Some(body) = value.strip_prefix(['+', '-']) else {
        return false;
    };
    let Some((hours, minutes, seconds)) = python_iso_offset_components(body) else {
        return false;
    };
    hours * 3_600 + minutes * 60 + seconds < 24 * 3_600
}

fn python_iso_local_time_components(value: &str) -> Option<(u32, u32, u32)> {
    if let Some(components) = iso_clock_with_fraction(value, true) {
        return Some(components);
    }
    let bytes = value.as_bytes();
    if bytes.len() > 6 && bytes[..6].iter().all(u8::is_ascii_digit) {
        let fraction = value.get(6..)?;
        if valid_python_local_fraction(fraction) {
            return exact_iso_clock(value.get(..6)?);
        }
    }
    if value.len() >= 9
        && value.get(2..3) == Some(":")
        && value.get(5..6) == Some(":")
        && value.get(8..9) == Some(":")
        && value.get(9..).is_some_and(valid_python_local_fraction)
    {
        return exact_iso_clock(value.get(..8)?);
    }
    let (&trailing, prefix) = bytes.split_last()?;
    if trailing.is_ascii() {
        return exact_iso_clock(std::str::from_utf8(prefix).ok()?);
    }
    None
}

fn python_iso_offset_components(value: &str) -> Option<(u32, u32, u32)> {
    if let Some(components) = iso_clock_with_fraction(value, false) {
        return Some(components);
    }
    let bytes = value.as_bytes();
    if bytes.len() >= 8 && bytes.iter().all(u8::is_ascii_digit) {
        return exact_iso_clock(value.get(..6)?);
    }
    if value.len() >= 10
        && value.get(2..3) == Some(":")
        && value.get(5..6) == Some(":")
        && value.get(8..9) == Some(":")
        && value.get(9..).is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return exact_iso_clock(value.get(..8)?);
    }
    None
}

fn iso_clock_with_fraction(value: &str, allow_empty_fraction: bool) -> Option<(u32, u32, u32)> {
    let Some(offset) = value.find(['.', ',']) else {
        return exact_iso_clock(value);
    };
    let digits = value.get(offset + 1..)?;
    let valid_fraction = if allow_empty_fraction {
        valid_python_local_fraction(digits)
    } else {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    };
    if !valid_fraction {
        return None;
    }
    exact_iso_clock(value.get(..offset)?)
}

fn valid_python_local_fraction(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_digit())
        || value
            .as_bytes()
            .get(..6)
            .is_some_and(|digits| digits.iter().all(u8::is_ascii_digit))
}

fn exact_iso_clock(value: &str) -> Option<(u32, u32, u32)> {
    let bytes = value.as_bytes();
    let (hour, minute, second) = match bytes.len() {
        2 => (ascii_decimal(bytes)?, 0, 0),
        4 => (
            ascii_decimal(bytes.get(..2)?)?,
            ascii_decimal(bytes.get(2..4)?)?,
            0,
        ),
        5 if bytes.get(2) == Some(&b':') => (
            ascii_decimal(bytes.get(..2)?)?,
            ascii_decimal(bytes.get(3..5)?)?,
            0,
        ),
        6 => (
            ascii_decimal(bytes.get(..2)?)?,
            ascii_decimal(bytes.get(2..4)?)?,
            ascii_decimal(bytes.get(4..6)?)?,
        ),
        8 if bytes.get(2) == Some(&b':') && bytes.get(5) == Some(&b':') => (
            ascii_decimal(bytes.get(..2)?)?,
            ascii_decimal(bytes.get(3..5)?)?,
            ascii_decimal(bytes.get(6..8)?)?,
        ),
        _ => return None,
    };
    Some((hour, minute, second))
}

fn ascii_decimal(value: &[u8]) -> Option<u32> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    value.iter().try_fold(0_u32, |result, digit| {
        result.checked_mul(10)?.checked_add(u32::from(digit - b'0'))
    })
}

fn validate_remote(value: &str, label: &str) -> Result<(), ObserverError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 128
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
        || value.ends_with('/')
        || value.contains("..")
    {
        return Err(invalid(format!("invalid {label} {value:?}")));
    }
    Ok(())
}

fn validate_ref(value: &str, label: &str) -> Result<(), ObserverError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.contains("..")
        || value.ends_with('/')
        || value.ends_with('.')
    {
        return Err(invalid(format!("invalid {label} {value:?}")));
    }
    Ok(())
}

fn validate_full_ref(value: &str, label: &str) -> Result<(), ObserverError> {
    validate_ref(value, label)?;
    let parts = value.split('/').collect::<Vec<_>>();
    if !value.starts_with("refs/")
        || parts.len() < 3
        || parts
            .iter()
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || value.contains("@{")
        || value.chars().any(|character| {
            let codepoint = u32::from(character);
            codepoint < 32 || codepoint == 127 || " ~^:?*[\\".contains(character)
        })
    {
        return Err(invalid(format!("invalid full {label} {value:?}")));
    }
    Ok(())
}

fn validate_relative_path(value: &str, label: &str) -> Result<(), ObserverError> {
    let path = Path::new(value);
    let components = path.components().collect::<Vec<_>>();
    if path.is_absolute()
        || components.is_empty()
        || components
            .iter()
            .all(|component| *component == Component::CurDir)
        || components.contains(&Component::ParentDir)
    {
        return Err(invalid(format!("{label} must be project-relative")));
    }
    Ok(())
}

fn optional_layout(
    record: &Map<String, Value>,
    label: &str,
) -> Result<Option<String>, ObserverError> {
    record
        .get("layout")
        .map(|value| {
            let layout = string(value, &format!("{label}.layout"))?;
            if !matches!(layout, "nested" | "flat") {
                return Err(invalid(format!("{label}.layout is invalid")));
            }
            Ok(layout.to_owned())
        })
        .transpose()
}

fn optional_string<'a>(
    record: &'a Map<String, Value>,
    key: &str,
    default: &'a str,
    label: &str,
) -> Result<&'a str, ObserverError> {
    record.get(key).map_or(Ok(default), |value| {
        string(value, &format!("{label}.{key}"))
    })
}

fn nullable_string(value: Option<&Value>, label: &str) -> Result<String, ObserverError> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(value) => string(value, label).map(str::to_owned),
    }
}

pub(crate) fn exact_keys(
    value: &Map<String, Value>,
    required: &[&str],
    label: &str,
) -> Result<(), ObserverError> {
    exact_keys_optional(value, required, &[], label)
}

pub(crate) fn exact_keys_optional(
    value: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result<(), ObserverError> {
    let actual = value.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let required = required.iter().copied().collect::<BTreeSet<_>>();
    let permitted = required
        .iter()
        .copied()
        .chain(optional.iter().copied())
        .collect::<BTreeSet<_>>();
    let missing = required.difference(&actual).copied().collect::<Vec<_>>();
    let unknown = actual.difference(&permitted).copied().collect::<Vec<_>>();
    if !missing.is_empty() || !unknown.is_empty() {
        return Err(invalid(format!(
            "{label} has invalid fields: missing {missing:?}; unknown {unknown:?}"
        )));
    }
    Ok(())
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>, ObserverError> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{label} must be a JSON object")))
}

fn array<'a>(value: &'a Value, label: &str) -> Result<&'a [Value], ObserverError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid(format!("{label} must be a JSON array")))
}

fn string<'a>(value: &'a Value, label: &str) -> Result<&'a str, ObserverError> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("{label} must be a string")))
}

fn nonempty_string<'a>(value: &'a Value, label: &str) -> Result<&'a str, ObserverError> {
    let value = string(value, label)?;
    if value.is_empty() {
        return Err(invalid(format!("{label} must not be empty")));
    }
    Ok(value)
}

fn named_string(value: &Value, label: &str) -> Result<String, ObserverError> {
    let value = string(value, label)?;
    validate_name(value, label)?;
    Ok(value.to_owned())
}

fn positive(value: &Value, label: &str) -> Result<u64, ObserverError> {
    let value = value
        .as_u64()
        .ok_or_else(|| invalid(format!("{label} must be a positive integer")))?;
    if value == 0 {
        return Err(invalid(format!("{label} must be a positive integer")));
    }
    Ok(value)
}

fn string_array<'a>(value: &'a Value, label: &str) -> Result<Vec<&'a str>, ObserverError> {
    array(value, label)?
        .iter()
        .enumerate()
        .map(|(index, value)| string(value, &format!("{label}[{index}]")))
        .collect()
}

fn digest<'a>(value: &'a Value, length: usize, label: &str) -> Result<&'a str, ObserverError> {
    let value = string(value, label)?;
    if value.len() != length
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(invalid(format!("{label} is not a lowercase digest")));
    }
    Ok(value)
}

fn invalid(message: impl Into<String>) -> ObserverError {
    ObserverError::invalid(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(checkout_extra: Value) -> Value {
        let mut checkout = json!({
            "name": "repo",
            "path": "repo",
            "repository": "../upstream",
            "branch": "main",
            "start_point": "a".repeat(40),
            "remote": "origin",
            "remote_url_sha256": "b".repeat(64),
            "landed_ref": "refs/remotes/origin/main",
            "head": "a".repeat(40),
            "containing_remote_refs": ["refs/remotes/origin/main"],
            "vcs": "git",
        });
        if let (Some(target), Some(extra)) = (checkout.as_object_mut(), checkout_extra.as_object())
        {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        json!({
            "slot": "slot-a",
            "agent": "agent-a",
            "task": "task",
            "purpose": "testing",
            "machine": "node-a",
            "generation": 1,
            "created_at": "2026-11-07T00:00:00Z",
            "heartbeat_at": "2026-11-07T00:00:00Z",
            "heartbeat_ttl_seconds": 60,
            "owner": null,
            "coordinator_lease": {
                "pid": 1,
                "start_ticks": 1,
                "boot_id": "boot",
                "host_id": "host",
                "cgroup_path": "/",
            },
            "coordinator_recovery_note": null,
            "handoff": null,
            "checkouts": [checkout],
        })
    }

    #[test]
    fn active_record_accepts_provisioned_checkouts() {
        // Writers since fe29e325 record create-hook output on the
        // checkout; the observer must not fail a whole rebuild.
        let value = record(json!({"provisioned": [{"path": "out.o"}]}));
        validate_active_record(&value, "node-a", "record").expect("provisioned accepted");
    }

    #[test]
    fn active_record_accepts_harness_checkout_reason() {
        let mut value = record(json!({}));
        value
            .as_object_mut()
            .expect("object")
            .insert("harness_checkout_reason".to_string(), json!("suite setup"));
        validate_active_record(&value, "node-a", "record").expect("reason accepted");
    }

    #[test]
    fn active_record_still_refuses_unknown_checkout_fields() {
        let value = record(json!({"surprise": 1}));
        assert!(validate_active_record(&value, "node-a", "record").is_err());
    }

    fn record_with_handoff(handoff: Value) -> Value {
        let mut value = record(json!({}));
        value
            .as_object_mut()
            .expect("object")
            .insert("handoff".to_string(), handoff);
        value
    }

    #[test]
    fn active_record_accepts_discarded_unaccounted_handoff() {
        // Writers since c88e65a5 record finish-discarded commits on the
        // handoff (#1364); the observer must not fail a whole rebuild.
        let value = record_with_handoff(json!({
            "recorded_at": "2026-11-07T00:00:00Z",
            "validation": ["checked"],
            "limitations": [],
            "continuation": "continue",
            "discarded_unaccounted": ["a".repeat(40)],
        }));
        validate_active_record(&value, "node-a", "record").expect("discarded accepted");
    }

    #[test]
    fn active_record_refuses_invalid_discarded_unaccounted() {
        let value = record_with_handoff(json!({
            "recorded_at": "2026-11-07T00:00:00Z",
            "validation": ["checked"],
            "limitations": [],
            "continuation": "continue",
            "discarded_unaccounted": ["not-a-sha"],
        }));
        assert!(validate_active_record(&value, "node-a", "record").is_err());
    }

    fn key_set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    /// The `(required, optional)` key sets a Python `_exact_keys` call in
    /// `function` declares, resolving a `fields = {...}` local.
    fn python_key_sets(source: &str, function: &str) -> (BTreeSet<String>, BTreeSet<String>) {
        let start = source
            .find(&format!("def {function}("))
            .unwrap_or_else(|| panic!("{function} not found in Python writer"));
        let body = &source[start..];
        let body = &body[..body.find("\ndef ").unwrap_or(body.len())];
        let call = body
            .find("_exact_keys(")
            .unwrap_or_else(|| panic!("{function} has no _exact_keys call"));
        let args = &body[call..];
        let args = &args[..args.find("\n    )").unwrap_or(args.len())];
        let mut parts: Vec<&str> = Vec::new();
        let mut depth = 0_i32;
        let mut part_start = 0_usize;
        for (index, byte) in args.bytes().enumerate() {
            match byte {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => depth -= 1,
                b',' if depth == 1 => {
                    parts.push(&args[part_start..index]);
                    part_start = index + 1;
                }
                _ => {}
            }
        }
        parts.push(&args[part_start..]);
        let resolve = |part: &str| -> BTreeSet<String> {
            let part = part.trim().trim_start_matches("_exact_keys(").trim();
            if part == "set()" {
                return BTreeSet::new();
            }
            let set_text = if let Some(open) = part.find('{') {
                &part[open..]
            } else {
                // A bare name: `fields = {...}` earlier in the function.
                let name = part.trim_end_matches(',').trim();
                let assign = body
                    .find(&format!("{name} = {{"))
                    .unwrap_or_else(|| panic!("{function}: cannot resolve {name}"));
                &body[assign..]
            };
            let close = set_text.find('}').expect("set closes");
            set_text[..close]
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect()
        };
        // parts[0] is the mapping argument; required and optional follow.
        (resolve(parts[1]), resolve(parts[2]))
    }

    #[test]
    fn shared_key_tables_match_python_writer() {
        // Fails when the observer schema and the Python writer disagree
        // on a shared record's keys (#1517). The Rust reader uses these
        // same constants, so it is covered by construction.
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../py/wrkslots/cli.py"
        ))
        .expect("read Python writer");
        let cases: [(&str, &[&str], &[&str]); 7] = [
            (
                "_record_from_obj",
                ACTIVE_RECORD_REQUIRED,
                ACTIVE_RECORD_OPTIONAL,
            ),
            (
                "_archive_record_from_obj",
                ARCHIVE_RECORD_REQUIRED,
                ARCHIVE_RECORD_OPTIONAL,
            ),
            ("_identity_from_obj", IDENTITY_REQUIRED, IDENTITY_OPTIONAL),
            ("_handoff_from_obj", HANDOFF_REQUIRED, HANDOFF_OPTIONAL),
            ("_checkout_from_obj", CHECKOUT_REQUIRED, CHECKOUT_OPTIONAL),
            ("_import_source_from_obj", IMPORT_SOURCE_REQUIRED, &[]),
            ("_task_scope_from_obj", TASK_SCOPE_REQUIRED, &[]),
        ];
        for (function, required, optional) in cases {
            let (python_required, python_optional) = python_key_sets(&source, function);
            assert_eq!(python_required, key_set(required), "{function} required");
            assert_eq!(python_optional, key_set(optional), "{function} optional");
        }
    }

    #[test]
    fn observer_and_reader_agree_on_active_record_keys() {
        // Both validators must accept or reject the same key mutations.
        let base = record(json!({}));
        assert!(validate_active_record(&base, "node-a", "record").is_ok());
        assert!(crate::active_state::record_from_obj(&base, "record").is_ok());
        let mut unknown = base.clone();
        unknown
            .as_object_mut()
            .expect("object")
            .insert("surprise".to_string(), json!(1));
        assert!(validate_active_record(&unknown, "node-a", "record").is_err());
        assert!(crate::active_state::record_from_obj(&unknown, "record").is_err());
        let mut missing = base.clone();
        missing.as_object_mut().expect("object").remove("task");
        assert!(validate_active_record(&missing, "node-a", "record").is_err());
        assert!(crate::active_state::record_from_obj(&missing, "record").is_err());
        let discarded = record_with_handoff(json!({
            "recorded_at": "2026-11-07T00:00:00Z",
            "validation": ["checked"],
            "limitations": [],
            "continuation": "continue",
            "discarded_unaccounted": ["a".repeat(40)],
        }));
        assert!(validate_active_record(&discarded, "node-a", "record").is_ok());
        assert!(crate::active_state::record_from_obj(&discarded, "record").is_ok());
    }
}
