//! `ACTIVE.<machine>.json` validation parity: read one active-slot shard.
//!
//! Reproduces the existing shard loader's reference mode (no repository
//! existence checks) for a fixture root: strict JSON reading with symlink
//! refusal, exact keys, record identity/handoff/checkout/task-scope/import
//! sub-formats, cross-record uniqueness, and stored-path authority against
//! the validated configuration. Read-only; it never inspects live
//! processes, Git registrations, or checkout existence, so `status`-style
//! liveness and repository diagnoses remain with the existing CLI.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::config_validate::{
    ensure_no_symlink_components, load_config, normpath, refuse_symlink, validate_full_ref,
    validate_layout, validate_name, validate_remote,
};

type Result<T> = std::result::Result<T, String>;

const SCHEMA: i64 = 2;
const HISTORICAL_STATUSES: [&str; 4] = [
    "active",
    "lease-quarantined",
    "owner-lease-revoked",
    "release-requested",
];
const TASK_NOT_RECORDED: &str = "no task recorded in worktree-state.json";
const PURPOSE_NOT_RECORDED: &str = "no purpose recorded in worktree-state.json";

pub(crate) struct Cfg {
    pub(crate) root: PathBuf,
    pub(crate) worktrees: PathBuf,
    pub(crate) control: PathBuf,
    pub(crate) layout: String,
    pub(crate) default_remote: String,
    pub(crate) default_landed_ref: String,
}

#[derive(Clone)]
struct Identity {
    pid: i64,
    start_ticks: i64,
    boot_id: String,
    host_id: String,
    cgroup_path: String,
}

pub(crate) struct CheckoutRec {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) repository: String,
    pub(crate) remote: String,
    pub(crate) landed_ref: String,
}

pub(crate) struct ImportRec {
    pub(crate) row: Map<String, Value>,
}

struct ScopeRec {
    cgroup_path: String,
    boot_id: String,
    leader_pid: i64,
    leader_start_ticks: i64,
}

struct RecordRec {
    slot: String,
    agent: String,
    task: String,
    purpose: String,
    slot_type: String,
    machine: String,
    created_at: String,
    layout: Option<String>,
    owner: Option<Identity>,
    checkouts: Vec<CheckoutRec>,
    import_source: Option<ImportRec>,
    normalized: Value,
}

pub(crate) fn as_mapping<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{label} must be a JSON object"))
}

pub(crate) fn as_list<'a>(value: &'a Value, label: &str) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| format!("{label} must be a JSON array"))
}

pub(crate) fn as_str<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| format!("{label} must be a string"))
}

pub(crate) fn as_int(value: &Value, label: &str, minimum: i64) -> Result<i64> {
    let number = match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok())),
        _ => None,
    };
    match number {
        Some(number) if number >= minimum => Ok(number),
        _ => Err(format!("{label} must be an integer >= {minimum}")),
    }
}

pub(crate) fn exact_keys(
    raw: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result<()> {
    let missing: Vec<&str> = required
        .iter()
        .filter(|key| !raw.contains_key(**key))
        .copied()
        .collect();
    let mut unknown: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|key| !required.contains(key) && !optional.contains(key))
        .collect();
    unknown.sort_unstable();
    if missing.is_empty() && unknown.is_empty() {
        return Ok(());
    }
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing {}", missing.join(", ")));
    }
    if !unknown.is_empty() {
        details.push(format!("unknown {}", unknown.join(", ")));
    }
    Err(format!(
        "{label} has invalid fields: {}",
        details.join("; ")
    ))
}

pub(crate) fn parse_timestamp(value: &str, label: &str) -> Result<()> {
    if crate::schema::parse_timestamp(value, label).is_ok() {
        return Ok(());
    }
    if is_naive_iso(value) {
        Err(format!("timestamp in {label} must include a time zone"))
    } else {
        Err(format!("invalid timestamp in {label}: {value:?}"))
    }
}

fn is_naive_iso(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
        || [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M",
        ]
        .iter()
        .any(|format| chrono::NaiveDateTime::parse_from_str(value, format).is_ok())
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn validate_ref(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.contains("..")
        || value.ends_with('/')
        || value.ends_with('.')
    {
        return Err(format!("invalid {label} {value:?}"));
    }
    Ok(())
}

fn identity_from_obj(value: &Value, label: &str) -> Result<Option<Identity>> {
    if value.is_null() {
        return Ok(None);
    }
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["pid", "start_ticks", "boot_id", "host_id", "cgroup_path"],
        &[],
        label,
    )?;
    let identity = Identity {
        pid: as_int(&raw["pid"], &format!("{label}.pid"), 1)?,
        start_ticks: as_int(&raw["start_ticks"], &format!("{label}.start_ticks"), 1)?,
        boot_id: as_str(&raw["boot_id"], &format!("{label}.boot_id"))?.to_owned(),
        host_id: as_str(&raw["host_id"], &format!("{label}.host_id"))?.to_owned(),
        cgroup_path: as_str(&raw["cgroup_path"], &format!("{label}.cgroup_path"))?.to_owned(),
    };
    if identity.boot_id.is_empty()
        || identity.host_id.is_empty()
        || !identity.cgroup_path.starts_with('/')
    {
        return Err(format!(
            "{label} has an invalid boot, machine, or cgroup identity"
        ));
    }
    Ok(Some(identity))
}

fn handoff_from_obj(value: &Value, label: &str) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["recorded_at", "validation", "limitations", "continuation"],
        &[],
        label,
    )?;
    let recorded_at = as_str(&raw["recorded_at"], &format!("{label}.recorded_at"))?;
    parse_timestamp(recorded_at, &format!("{label}.recorded_at"))?;
    let validation = as_list(&raw["validation"], &format!("{label}.validation"))?;
    for (index, item) in validation.iter().enumerate() {
        as_str(item, &format!("{label}.validation[{index}]"))?;
    }
    for (index, item) in as_list(&raw["limitations"], &format!("{label}.limitations"))?
        .iter()
        .enumerate()
    {
        as_str(item, &format!("{label}.limitations[{index}]"))?;
    }
    let continuation = as_str(&raw["continuation"], &format!("{label}.continuation"))?;
    if validation.is_empty() || continuation.is_empty() {
        return Err(format!(
            "{label} requires validation evidence and an exact continuation"
        ));
    }
    Ok(())
}

pub(crate) fn checkout_from_obj(value: &Value, label: &str) -> Result<CheckoutRec> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
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
            "containing_remote_refs",
            "vcs",
        ],
        &[],
        label,
    )?;
    let name = as_str(&raw["name"], &format!("{label}.name"))?.to_owned();
    let path = as_str(&raw["path"], &format!("{label}.path"))?.to_owned();
    let repository = as_str(&raw["repository"], &format!("{label}.repository"))?.to_owned();
    let branch = as_str(&raw["branch"], &format!("{label}.branch"))?.to_owned();
    let start_point = as_str(&raw["start_point"], &format!("{label}.start_point"))?.to_owned();
    let remote = as_str(&raw["remote"], &format!("{label}.remote"))?.to_owned();
    let digest = as_str(
        &raw["remote_url_sha256"],
        &format!("{label}.remote_url_sha256"),
    )?
    .to_owned();
    let landed_ref = as_str(&raw["landed_ref"], &format!("{label}.landed_ref"))?.to_owned();
    let head = as_str(&raw["head"], &format!("{label}.head"))?.to_owned();
    let mut containing = Vec::new();
    for (index, item) in as_list(
        &raw["containing_remote_refs"],
        &format!("{label}.containing_remote_refs"),
    )?
    .iter()
    .enumerate()
    {
        containing
            .push(as_str(item, &format!("{label}.containing_remote_refs[{index}]"))?.to_owned());
    }
    let vcs = as_str(&raw["vcs"], &format!("{label}.vcs"))?.to_owned();
    validate_name(&name, "checkout name")?;
    validate_remote(&remote)?;
    if !is_digest(&digest) {
        return Err(format!(
            "{label}.remote_url_sha256 must be a lowercase SHA-256"
        ));
    }
    validate_ref(&branch, "branch")?;
    if !is_sha(&start_point) {
        return Err(format!(
            "{label}.start_point must be a full lowercase Git commit"
        ));
    }
    validate_full_ref(&landed_ref, "landed ref")?;
    if vcs != "git" {
        return Err(format!("unsupported VCS in {label}: {vcs:?}"));
    }
    if !is_sha(&head) {
        return Err(format!("{label}.head must be a full lowercase Git commit"));
    }
    let prefix = format!("refs/remotes/{remote}/");
    if containing.iter().any(|item| !item.starts_with(&prefix)) {
        return Err(format!(
            "{label}.containing_remote_refs names an unauthorized ref"
        ));
    }
    Ok(CheckoutRec {
        name,
        path,
        repository,
        remote,
        landed_ref,
    })
}

fn scope_from_obj(value: &Value, label: &str) -> Result<ScopeRec> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "unit",
            "invocation_id",
            "cgroup_path",
            "boot_id",
            "leader_pid",
            "leader_start_ticks",
            "verification",
        ],
        &[],
        label,
    )?;
    let unit = as_str(&raw["unit"], &format!("{label}.unit"))?.to_owned();
    let invocation_id =
        as_str(&raw["invocation_id"], &format!("{label}.invocation_id"))?.to_owned();
    let cgroup_path = as_str(&raw["cgroup_path"], &format!("{label}.cgroup_path"))?.to_owned();
    let boot_id = as_str(&raw["boot_id"], &format!("{label}.boot_id"))?.to_owned();
    let leader_pid = as_int(&raw["leader_pid"], &format!("{label}.leader_pid"), 1)?;
    let leader_start_ticks = as_int(
        &raw["leader_start_ticks"],
        &format!("{label}.leader_start_ticks"),
        1,
    )?;
    let verification = as_str(&raw["verification"], &format!("{label}.verification"))?.to_owned();
    if !unit.ends_with(".scope")
        || unit.contains('/')
        || unit.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(format!("{label}.unit is not a task-scoped systemd unit"));
    }
    if !(invocation_id.len() == 32
        && invocation_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err(format!(
            "{label}.invocation_id is not a canonical systemd invocation ID"
        ));
    }
    let uuid = {
        let parts: Vec<&str> = boot_id.split('-').collect();
        parts.len() == 5
            && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(len, part)| {
                part.len() == *len
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
    };
    if !cgroup_path.starts_with('/')
        || !cgroup_path.ends_with(&format!("/{unit}"))
        || cgroup_path.contains("/../")
        || !uuid
    {
        return Err(format!("{label} has an invalid cgroup path or boot ID"));
    }
    if verification != "systemd-runtime-invocation-symlink-v1" {
        return Err(format!("{label}.verification is unsupported"));
    }
    Ok(ScopeRec {
        cgroup_path,
        boot_id,
        leader_pid,
        leader_start_ticks,
    })
}

pub(crate) fn import_from_obj(value: Option<&Value>, label: &str) -> Result<Option<ImportRec>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "format",
            "path",
            "file_sha256",
            "row_sha256",
            "status",
            "row",
        ],
        &[],
        label,
    )?;
    let format = as_str(&raw["format"], &format!("{label}.format"))?;
    if format != "worktree-state-v3" {
        return Err(format!("{label}.format is unsupported: {format:?}"));
    }
    let path = as_str(&raw["path"], &format!("{label}.path"))?;
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if path.starts_with('/') || parts.is_empty() || parts.contains(&"..") {
        return Err(format!("{label}.path must be project-relative"));
    }
    let file_digest = as_str(&raw["file_sha256"], &format!("{label}.file_sha256"))?;
    let row_digest = as_str(&raw["row_sha256"], &format!("{label}.row_sha256"))?;
    if !is_digest(file_digest) || !is_digest(row_digest) {
        return Err(format!("{label} digests must be lowercase SHA-256 values"));
    }
    let status = as_str(&raw["status"], &format!("{label}.status"))?;
    if !HISTORICAL_STATUSES.contains(&status) {
        return Err(format!("{label}.status is not an active historical state"));
    }
    let row = as_mapping(&raw["row"], &format!("{label}.row"))?.clone();
    let digest =
        crate::canonical_sha256(&Value::Object(row.clone())).map_err(|error| error.to_string())?;
    if digest != row_digest {
        return Err(format!("{label}.row digest does not match its content"));
    }
    if row.get("status").and_then(Value::as_str) != Some(status) {
        return Err(format!("{label}.status does not match its source row"));
    }
    Ok(Some(ImportRec { row }))
}

pub(crate) fn historical_task_and_purpose(
    row: &Map<String, Value>,
    label: &str,
    agent: &str,
) -> Result<(String, String)> {
    let sidecar = as_mapping(
        row.get("owner_sidecar").unwrap_or(&Value::Null),
        &format!("{label}.owner_sidecar"),
    )?;
    let mut writable: Vec<&Map<String, Value>> = Vec::new();
    for (index, item) in as_list(
        row.get("agents").unwrap_or(&Value::Null),
        &format!("{label}.agents"),
    )?
    .iter()
    .enumerate()
    {
        let entry = as_mapping(item, &format!("{label}.agents[{index}]"))?;
        if entry.get("name").and_then(Value::as_str) == Some(agent)
            && entry.get("read_only") == Some(&Value::Bool(false))
        {
            writable.push(entry);
        }
    }
    if writable.is_empty() {
        return Err(format!(
            "{label} does not list owner {agent} as a writable agent"
        ));
    }
    let row_task = match row.get("task") {
        None | Some(Value::Null) => String::new(),
        Some(value) => as_str(value, &format!("{label}.task"))?.to_owned(),
    };
    let owner_task = match sidecar.get("task") {
        None | Some(Value::Null) => String::new(),
        Some(value) => as_str(value, &format!("{label}.owner_sidecar.task"))?.to_owned(),
    };
    let mut writable_tasks: Vec<String> = Vec::new();
    for (index, entry) in writable.iter().enumerate() {
        if let Some(value) = entry.get("task") {
            if !value.is_null() {
                let task = as_str(value, &format!("{label}.writable_agent[{index}].task"))?;
                if !task.is_empty() && !writable_tasks.contains(&task.to_owned()) {
                    writable_tasks.push(task.to_owned());
                }
            }
        }
    }
    if writable_tasks.len() > 1 {
        return Err(format!(
            "{label} records conflicting writable-agent task values"
        ));
    }
    let task = if !row_task.is_empty() {
        row_task
    } else if !owner_task.is_empty() {
        owner_task
    } else {
        writable_tasks
            .first()
            .cloned()
            .unwrap_or_else(|| TASK_NOT_RECORDED.to_owned())
    };
    let purpose = match row.get("purpose") {
        None | Some(Value::Null) => PURPOSE_NOT_RECORDED.to_owned(),
        Some(value) => {
            let purpose = as_str(value, &format!("{label}.purpose"))?;
            if purpose.is_empty() {
                PURPOSE_NOT_RECORDED.to_owned()
            } else {
                purpose.to_owned()
            }
        }
    };
    Ok((task, purpose))
}

fn assert_import_matches(record: &RecordRec, source: &ImportRec, label: &str) -> Result<()> {
    if record.slot_type != "agent"
        || record.layout.as_deref() != Some("nested")
        || record.owner.is_none()
    {
        return Err(format!(
            "{label} historical import must be a nested agent slot with an exact owner"
        ));
    }
    let owner = record.owner.as_ref().expect("checked above");
    let (expected_task, expected_purpose) = historical_task_and_purpose(
        &source.row,
        &format!("{label}.import_source.row"),
        &record.agent,
    )?;
    if expected_task != record.task || expected_purpose != record.purpose {
        return Err(format!(
            "{label} differs from its historical task or purpose"
        ));
    }
    if source.row.get("allocated").and_then(Value::as_str) != Some(record.created_at.as_str()) {
        return Err(format!(
            "{label} differs from its historical allocation time"
        ));
    }
    let sidecar = as_mapping(
        source.row.get("owner_sidecar").unwrap_or(&Value::Null),
        &format!("{label}.import_source.row.owner_sidecar"),
    )?;
    let matches = sidecar.get("supervisor_pid") == Some(&json!(owner.pid))
        && sidecar.get("start_ticks") == Some(&json!(owner.start_ticks))
        && sidecar.get("boot_id").and_then(Value::as_str) == Some(owner.boot_id.as_str())
        && sidecar.get("cgroup_path").and_then(Value::as_str) == Some(owner.cgroup_path.as_str());
    if !matches {
        return Err(format!(
            "{label} owner differs from its historical owner_sidecar"
        ));
    }
    if sidecar.get("slot").and_then(Value::as_str) != Some(record.slot.as_str())
        || sidecar.get("agent").and_then(Value::as_str) != Some(record.agent.as_str())
    {
        return Err(format!(
            "{label} identity differs from its historical owner_sidecar"
        ));
    }
    for checkout in &record.checkouts {
        if source
            .row
            .get(&format!("{}_path", checkout.name))
            .and_then(Value::as_str)
            != Some(checkout.path.as_str())
        {
            return Err(format!(
                "{label} checkout {} differs from its historical path",
                checkout.name
            ));
        }
    }
    Ok(())
}

fn record_from_obj(value: &Value, label: &str) -> Result<RecordRec> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
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
        ],
        &["slot_type", "layout", "import_source", "task_scope"],
        label,
    )?;
    let mut checkouts = Vec::new();
    for (index, item) in as_list(&raw["checkouts"], &format!("{label}.checkouts"))?
        .iter()
        .enumerate()
    {
        checkouts.push(checkout_from_obj(
            item,
            &format!("{label}.checkouts[{index}]"),
        )?);
    }
    let lease = identity_from_obj(
        &raw["coordinator_lease"],
        &format!("{label}.coordinator_lease"),
    )?;
    if lease.is_none() {
        return Err(format!("{label} has no coordinator lease"));
    }
    let slot = as_str(&raw["slot"], &format!("{label}.slot"))?.to_owned();
    let agent = as_str(&raw["agent"], &format!("{label}.agent"))?.to_owned();
    let task = as_str(&raw["task"], &format!("{label}.task"))?.to_owned();
    let purpose = as_str(&raw["purpose"], &format!("{label}.purpose"))?.to_owned();
    let slot_type = match raw.get("slot_type") {
        None => "agent".to_owned(),
        Some(value) => as_str(value, &format!("{label}.slot_type"))?.to_owned(),
    };
    let machine = as_str(&raw["machine"], &format!("{label}.machine"))?.to_owned();
    as_int(&raw["generation"], &format!("{label}.generation"), 1)?;
    let created_at = as_str(&raw["created_at"], &format!("{label}.created_at"))?.to_owned();
    let heartbeat_at = as_str(&raw["heartbeat_at"], &format!("{label}.heartbeat_at"))?.to_owned();
    as_int(
        &raw["heartbeat_ttl_seconds"],
        &format!("{label}.heartbeat_ttl_seconds"),
        1,
    )?;
    let owner = identity_from_obj(&raw["owner"], &format!("{label}.owner"))?;
    let note = match &raw["coordinator_recovery_note"] {
        Value::Null => None,
        value => Some(as_str(value, &format!("{label}.coordinator_recovery_note"))?.to_owned()),
    };
    handoff_from_obj(&raw["handoff"], &format!("{label}.handoff"))?;
    let layout = match raw.get("layout") {
        None => None,
        Some(value) => Some(validate_layout(as_str(value, &format!("{label}.layout"))?)?),
    };
    let import_source =
        import_from_obj(raw.get("import_source"), &format!("{label}.import_source"))?;
    let task_scope = match raw.get("task_scope") {
        None => None,
        Some(value) => Some(scope_from_obj(value, &format!("{label}.task_scope"))?),
    };
    validate_name(&slot, "slot")?;
    validate_name(&agent, "agent")?;
    validate_name(&machine, "machine")?;
    if task.is_empty() || purpose.is_empty() {
        return Err(format!("{label} has an empty task or purpose"));
    }
    if !matches!(slot_type.as_str(), "agent" | "validate") {
        return Err(format!("{label}.slot_type must be one of agent, validate"));
    }
    parse_timestamp(&created_at, &format!("{label}.created_at"))?;
    parse_timestamp(&heartbeat_at, &format!("{label}.heartbeat_at"))?;
    if checkouts.is_empty() && import_source.is_none() {
        return Err(format!("{label} has no checkouts"));
    }
    if checkouts.is_empty() && layout.as_deref() != Some("nested") {
        return Err(format!(
            "{label} has no checkouts but is not a historical nested slot"
        ));
    }
    if note.is_some() && owner.is_some() {
        return Err(format!(
            "{label} has coordinator recovery evidence for a bound owner"
        ));
    }
    if let Some(scope) = &task_scope {
        let scope_label = format!("{label}.task_scope");
        let owner = owner
            .as_ref()
            .ok_or_else(|| format!("{scope_label} has no bound owner process"))?;
        if scope.leader_pid != owner.pid
            || scope.leader_start_ticks != owner.start_ticks
            || scope.boot_id != owner.boot_id
            || scope.cgroup_path != owner.cgroup_path
        {
            return Err(format!(
                "{scope_label} does not match its bound owner process"
            ));
        }
    }
    let mut names: Vec<&str> = checkouts.iter().map(|item| item.name.as_str()).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    if names.len() != total {
        return Err(format!("{label} has duplicate checkout names"));
    }
    let mut normalized = raw.clone();
    normalized
        .entry("slot_type".to_owned())
        .or_insert_with(|| Value::String("agent".to_owned()));
    if import_source.is_none() {
        normalized.remove("import_source");
    }
    let record = RecordRec {
        slot,
        agent,
        task,
        purpose,
        slot_type,
        machine,
        created_at,
        layout,
        owner,
        checkouts,
        import_source,
        normalized: Value::Object(normalized),
    };
    if let Some(source) = &record.import_source {
        assert_import_matches(&record, source, label)?;
    }
    Ok(record)
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

pub(crate) fn slot_directory(cfg: &Cfg, slot: &str, slot_type: &str) -> Result<PathBuf> {
    validate_name(slot, "slot")?;
    if !matches!(slot_type, "agent" | "validate") {
        return Err("slot type must be one of agent, validate".to_owned());
    }
    let base = if slot_type == "validate" {
        managed_worktrees_root(cfg).join("validate")
    } else {
        cfg.worktrees.clone()
    };
    Ok(base.join(slot))
}

pub(crate) fn landed_ref_for_remote(cfg: &Cfg, remote: &str) -> Result<String> {
    let prefix = format!("refs/remotes/{}/", cfg.default_remote);
    let Some(rest) = cfg.default_landed_ref.strip_prefix(&prefix) else {
        return Err(format!("configured landed ref must be under {prefix}"));
    };
    Ok(format!("refs/remotes/{remote}/{rest}"))
}

pub(crate) fn posix_relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("slot path escapes the project root: {}", path.display()))?;
    Ok(relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}

fn stored_path_reference(cfg: &Cfg, raw: &str, label: &str) -> Result<PathBuf> {
    let parts: Vec<&str> = raw.split('/').collect();
    if raw.starts_with('/') || raw.is_empty() || parts.contains(&"..") {
        return Err(format!(
            "{label} must be a relative path inside {}: {raw:?}",
            cfg.root.display()
        ));
    }
    let lexical = normpath(raw);
    if lexical == "." {
        return Err(format!("{label} must not be the project root"));
    }
    Ok(cfg.root.join(&lexical))
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

fn path_is_within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

fn stored_repository_reference(cfg: &Cfg, raw: &str) -> Result<()> {
    let root_parent = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
    let (unresolved, allowed_root) = if raw.starts_with('/') {
        if raw != normpath_absolute(raw) {
            return Err(format!(
                "stored absolute repository path is not canonical: {raw:?}"
            ));
        }
        let candidate = PathBuf::from(raw);
        if candidate == cfg.root
            || candidate == root_parent
            || candidate.file_name().is_none()
            || candidate.parent() != Some(root_parent.as_path())
        {
            return Err(format!(
                "stored absolute repository path must name one direct sibling of the project root, not the project root or another external path: {raw:?}"
            ));
        }
        (candidate, root_parent.clone())
    } else {
        let parts: Vec<&str> = raw
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        let sibling = parts.len() >= 2
            && parts[0] == ".."
            && parts[1..].iter().all(|part| !matches!(*part, "." | ".."));
        if parts.contains(&"..") && !sibling {
            return Err(format!(
                "stored repository path must stay inside {} or one direct sibling, with path components '../NAME' or '../NAME/PATH': {raw:?}",
                cfg.root.display()
            ));
        }
        if sibling {
            let unresolved = root_parent.join(parts[1..].join("/"));
            if unresolved == cfg.root || root_parent.join(parts[1]) == cfg.root {
                return Err(format!(
                    "stored repository path {raw:?} aliases the project root; use a path inside it instead"
                ));
            }
            (unresolved, root_parent.clone())
        } else {
            let lexical = normpath(raw);
            let unresolved = if lexical == "." {
                cfg.root.clone()
            } else {
                cfg.root.join(&lexical)
            };
            (unresolved, cfg.root.clone())
        }
    };
    ensure_no_symlink_components(&allowed_root, &unresolved, "repository")?;
    if path_is_within(&unresolved, &managed_worktrees_root(cfg)) {
        return Err(format!(
            "source repository must be outside the managed worktrees directory: {}",
            unresolved.display()
        ));
    }
    Ok(())
}

fn assert_record_paths(cfg: &Cfg, record: &RecordRec) -> Result<()> {
    let expected_slot = slot_directory(cfg, &record.slot, &record.slot_type)?;
    let layout = record.layout.as_deref().unwrap_or(&cfg.layout);
    if layout == "flat" && record.checkouts.len() != 1 {
        return Err(format!(
            "slot {} has multiple checkouts under flat layout",
            record.slot
        ));
    }
    for checkout in &record.checkouts {
        if checkout.landed_ref != landed_ref_for_remote(cfg, &checkout.remote)? {
            return Err(format!(
                "slot {} checkout {} differs from configured authority",
                record.slot, checkout.name
            ));
        }
        let expected_absolute = if layout == "flat" {
            expected_slot.clone()
        } else {
            expected_slot.join(&checkout.name)
        };
        let expected_relative = posix_relative(&cfg.root, &expected_absolute)?;
        if checkout.path != expected_relative {
            return Err(format!(
                "slot {} checkout {} path does not match its slot: {:?} != {:?}",
                record.slot, checkout.name, checkout.path, expected_relative
            ));
        }
        let actual = stored_path_reference(cfg, &checkout.path, "checkout path")?;
        if actual != expected_absolute {
            return Err(format!(
                "slot {} checkout path identity changed",
                record.slot
            ));
        }
        stored_repository_reference(cfg, &checkout.repository)?;
    }
    Ok(())
}

fn active_from_obj(cfg: &Cfg, value: &Value, selected: &str, label: &str) -> Result<Value> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["schema", "machine", "revision", "slots"],
        &[],
        "active state",
    )?;
    if as_int(&raw["schema"], "active state.schema", 0)? != SCHEMA {
        return Err(format!("unsupported active state schema in {label}"));
    }
    let actual_machine = as_str(&raw["machine"], "active state.machine")?;
    if actual_machine != selected {
        return Err(format!(
            "active state machine mismatch: filename says {selected}, content says {actual_machine}"
        ));
    }
    let mut records = Vec::new();
    for (index, item) in as_list(&raw["slots"], "active state.slots")?
        .iter()
        .enumerate()
    {
        records.push(record_from_obj(
            item,
            &format!("active state.slots[{index}]"),
        )?);
    }
    let mut slot_names: Vec<&str> = records.iter().map(|item| item.slot.as_str()).collect();
    let slot_total = slot_names.len();
    slot_names.sort_unstable();
    slot_names.dedup();
    if slot_names.len() != slot_total {
        return Err(format!("duplicate slot in {label}"));
    }
    let mut agents: Vec<&str> = records
        .iter()
        .filter(|item| item.slot_type == "agent" && item.import_source.is_none())
        .map(|item| item.agent.as_str())
        .collect();
    let agent_total = agents.len();
    agents.sort_unstable();
    agents.dedup();
    if agents.len() != agent_total {
        return Err(format!(
            "one agent owns more than one active slot in {label}"
        ));
    }
    for record in &records {
        if record.machine != selected {
            return Err(format!(
                "slot {} says machine {}, expected shard {selected}",
                record.slot, record.machine
            ));
        }
        assert_record_paths(cfg, record)?;
    }
    let revision = as_int(&raw["revision"], "active state.revision", 0)?;
    let mut slots: Vec<Value> = records.into_iter().map(|item| item.normalized).collect();
    slots.sort_by(|left, right| {
        left.get("slot")
            .and_then(Value::as_str)
            .cmp(&right.get("slot").and_then(Value::as_str))
    });
    Ok(json!({
        "schema": SCHEMA,
        "machine": actual_machine,
        "revision": revision,
        "slots": slots,
    }))
}

pub(crate) fn load_cfg(
    root: Option<&Path>,
    machine_override: Option<&str>,
) -> Result<(Cfg, String)> {
    let config = load_config(root, machine_override)?;
    let text = |key: &str| -> Result<String> {
        config
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("validated configuration is missing {key}"))
    };
    let cfg = Cfg {
        root: PathBuf::from(text("root")?),
        worktrees: PathBuf::from(text("worktrees")?),
        control: PathBuf::from(text("control")?),
        layout: text("layout")?,
        default_remote: text("default_remote")?,
        default_landed_ref: text("default_landed_ref")?,
    };
    let selected = text("machine")?;
    Ok((cfg, selected))
}

/// Read and validate `ACTIVE.<machine>.json` under a project's control
/// directory, printing the normalized state document.
pub(crate) fn parse_state(root: Option<&Path>, machine_override: Option<&str>) -> Result<Value> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    refuse_symlink(&path, "active state")?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing active state: {}", path.display())
        } else {
            format!("cannot read active state {}: {error}", path.display())
        }
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("active state is malformed JSON: {error}"))?;
    active_from_obj(&cfg, &value, &selected, &path.to_string_lossy())
}
