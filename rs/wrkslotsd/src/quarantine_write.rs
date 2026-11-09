//! Writer: set aside one stranded create journal recover itself refuses.
//!
//! Mirrors `_cmd_quarantine_journal` (#1086c): a read-only eligibility
//! plan (both owners proven dead, stable bytes, age bound, a named
//! classification refusal recover shares), a dry run by default, and
//! with `--yes` echoing the dry run's path and sha256, an
//! `operation-quarantined` tombstone event appended in the same
//! journaled step as the move into `quarantine/`. Fixture registries
//! only until the edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::active_state::{as_int, as_list, as_str, load_cfg, slot_directory, Cfg};
use crate::batch_text::sort_keys;
use crate::classify_journals::{
    absolute_path, create_journal_path, create_journal_state, journal_age_seconds, read_identity,
    strict_obj,
};
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::{append_event, ensure_event_log, refuse_partial_state, require_fixture};
use crate::git_storage_state::git_run;
use crate::global_state::load_event_chain;
use crate::hold_write::{acquire_mutation_locks, validate_global};
use crate::journal_state::validate_journal_shape;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;
type States = Vec<(String, Vec<Value>)>;
type Archives = Vec<Vec<Value>>;

const MIN_AGE_SECONDS: u64 = 24 * 60 * 60;
const REASON_LIMIT: usize = 500;

fn quarantine_directory(cfg: &Cfg) -> PathBuf {
    cfg.control.join("quarantine")
}

/// Best-effort census rows for quarantined journals, mirroring
/// `_quarantined_journal_rows`; shared with doctor and audit.
pub(crate) fn quarantined_rows(cfg: &Cfg) -> Vec<Value> {
    let directory = quarantine_directory(cfg);
    let mut rows: Vec<Value> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return rows;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().ends_with(".journal"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        let parsed: Option<Value> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let text = |key: &str| -> Option<String> {
            parsed
                .as_ref()
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        rows.push(json!({
            "age_seconds": journal_age_seconds(&path),
            "agent": text("agent"),
            "journal": absolute_path(&path).to_string_lossy(),
            "kind": text("kind").unwrap_or_else(|| "unknown".to_string()),
            "machine": text("machine"),
            "slot": text("slot"),
        }));
    }
    rows
}

fn quarantine_event_for(cfg: &Cfg, path: &Path, sha256: &str) -> Option<Value> {
    let events = load_event_chain(
        &cfg.control.join(format!("EVENTS.{}", cfg.machine)),
        &cfg.machine,
    )
    .ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    events.into_iter().find(|event| {
        if event.get("kind").and_then(Value::as_str) != Some("operation-quarantined") {
            return false;
        }
        let Some(payload) = event.get("payload") else {
            return false;
        };
        payload.get("journal_path").and_then(Value::as_str) == Some(name.as_str())
            && payload.get("journal_sha256").and_then(Value::as_str) == Some(sha256)
    })
}

struct Plan {
    path: PathBuf,
    sha256: String,
    slot: String,
    agent: String,
    slot_type: String,
    owner_pid: Option<i64>,
    owner_state: String,
    coordinator_pid: i64,
    coordinator_state: String,
    age_seconds: u64,
    bound_seconds: u64,
    recover_refusal: String,
    artifacts: Vec<Value>,
    already_sequence: Option<i64>,
    already_path: Option<String>,
}

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn assert_caller(identity: &Identity, label: &str) -> Result<()> {
    let mut current: Option<i64> = Some(std::process::id() as i64);
    let mut seen: Vec<i64> = Vec::new();
    for _ in 0..256 {
        let Some(pid) = current else { break };
        if seen.contains(&pid) {
            break;
        }
        seen.push(pid);
        if let Ok(candidate) = read_process_identity(pid) {
            if candidate == *identity {
                return Ok(());
            }
        }
        current = parent_pid(pid);
    }
    Err(format!(
        "{label} PID {} is not in the invoking process ancestry",
        identity.pid
    ))
}

fn field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn stored_path(cfg: &Cfg, raw: &str, label: &str) -> Result<PathBuf> {
    let parts: Vec<&str> = raw.split('/').collect();
    if raw.starts_with('/') || raw.is_empty() || parts.contains(&"..") {
        return Err(format!(
            "{label} must be a relative path inside {}: {raw:?}",
            cfg.root.display()
        ));
    }
    Ok(cfg.root.join(raw))
}

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

#[allow(clippy::too_many_arguments)]
fn artifacts(
    cfg: &Cfg,
    raw: &Map<String, Value>,
    slot: &str,
    slot_type: &str,
    states: &States,
    archives: &Archives,
) -> Result<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let slot_path = slot_directory(cfg, slot, slot_type)?;
    out.push(json!({
        "kind": "slot-directory",
        "path": slot_path.to_string_lossy(),
        "exists": slot_path.exists(),
    }));
    let planned = as_list(
        raw.get("planned").unwrap_or(&Value::Null),
        "create journal.planned",
    )?;
    for (index, item) in planned.iter().enumerate() {
        let destination_raw = as_str(
            item.get("destination").unwrap_or(&Value::Null),
            &format!("create journal.planned[{index}].destination"),
        )?;
        let destination = stored_path(cfg, destination_raw, "checkout destination")?;
        let repository_raw = field(item, "repository");
        let (_, repository, _) =
            crate::active_state::stored_repository_parts(cfg, &repository_raw)?;
        let branch = field(item, "branch");
        let branch_exists = git_run(
            &repository,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )
        .map(|_| true)
        .unwrap_or(false);
        out.push(json!({
            "kind": "worktree",
            "name": field(item, "name"),
            "path": destination.to_string_lossy(),
            "exists": destination.exists(),
            "branch": branch,
            "branch_exists": branch_exists,
            "repository": repository_raw,
        }));
    }
    if let Ok(Some(image)) =
        crate::image_record::load_slot_image(&cfg.control, slot_type, slot, &slot_path)
    {
        out.push(json!({
            "kind": "image",
            "path": image.image_file().to_string_lossy(),
            "exists": image.image_file().exists(),
        }));
    }
    let active = states
        .iter()
        .filter(|(machine, _)| *machine == cfg.machine)
        .flat_map(|(_, slots)| slots.iter())
        .find(|record| field(record, "slot") == slot);
    out.push(json!({
        "kind": "active-row",
        "slot": slot,
        "exists": active.is_some(),
        "generation": active.and_then(|record| record.get("generation").and_then(Value::as_i64)),
    }));
    let archived = archives
        .iter()
        .flat_map(|records| records.iter())
        .find(|record| field(record, "slot") == slot && field(record, "machine") == cfg.machine);
    out.push(json!({
        "kind": "archive-record",
        "slot": slot,
        "exists": archived.is_some(),
        "archive_id": archived.and_then(|record| record.get("archive_id")).cloned().unwrap_or(Value::Null),
    }));
    Ok(out)
}

fn load_states_archives(cfg: &Cfg) -> Result<(States, Archives)> {
    let mut states: States = Vec::new();
    let mut archives: Archives = Vec::new();
    for machine in crate::global_state::discover_machines(&cfg.control)? {
        let active_path = cfg.control.join(format!("ACTIVE.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&active_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                states.push((
                    machine.clone(),
                    doc.get("slots")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        let archive_path = cfg.control.join(format!("ARCHIVED.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&archive_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                archives.push(
                    doc.get("records")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
    }
    Ok((states, archives))
}

fn build_plan(
    root: Option<&Path>,
    machine_override: Option<&str>,
    cfg: &Cfg,
    slot: &str,
) -> Result<Plan> {
    let path = create_journal_path(cfg, slot, &cfg.machine);
    if path.symlink_metadata().is_err() {
        let standalone = crate::journal_state::journal_legacy_path(cfg, &cfg.machine);
        if standalone.exists() {
            let legacy = std::fs::read(&standalone)
                .ok()
                .and_then(|bytes| strict_obj(&bytes).ok())
                .unwrap_or_default();
            {
                let raw = &legacy;
                if raw.get("kind").and_then(Value::as_str) == Some("create")
                    && raw.get("slot").and_then(Value::as_str) == Some(slot)
                {
                    return Err(format!(
                        "slot {slot:?} has a legacy standalone create journal in {}; quarantine-journal covers scoped create journals only; run 'wrkslots recover' for this journal",
                        standalone.display()
                    ));
                }
            }
        }
        return Err(format!(
            "no scoped create journal is recorded for slot {slot:?}"
        ));
    }
    let (contents, identity) = read_identity(&path)?;
    let raw = strict_obj(&contents).map_err(|error| {
        format!(
            "quarantine cannot prove the contents of {}: {error}",
            path.display()
        )
    })?;
    let (contents_again, identity_again) = read_identity(&path)?;
    if identity_again != identity || contents_again != contents {
        return Err(format!(
            "create journal changed while it was read: {}; rerun quarantine-journal",
            path.display()
        ));
    }
    if raw.get("kind").and_then(Value::as_str) != Some("create")
        || raw.get("slot").and_then(Value::as_str) != Some(slot)
    {
        return Err(format!(
            "journal {} does not record a create for slot {slot:?}",
            path.display()
        ));
    }
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    validate_journal_shape(cfg, &filename, &raw).map_err(|error| {
        format!(
            "quarantine cannot prove the owners or artifacts of {}: {error}",
            path.display()
        )
    })?;
    let owner = crate::status_state::identity_from_value(raw.get("owner").unwrap_or(&Value::Null));
    let coordinator = crate::status_state::identity_from_value(
        raw.get("coordinator_lease").unwrap_or(&Value::Null),
    )
    .ok_or_else(|| "create journal has no coordinator lease".to_string())?;
    let (coordinator_state, coordinator_detail) = process_state(Some(&coordinator));
    let (owner_state, owner_detail) = match &owner {
        None => (
            coordinator_state.clone(),
            "no owner process generation is recorded (owner-less create; the coordinator lease decides)".to_string(),
        ),
        Some(owner) => process_state(Some(owner)),
    };
    if owner_state != "dead" || coordinator_state != "dead" {
        return Err(format!(
            "create journal for slot {slot:?} owners are not both proven dead: owner={owner_state} ({owner_detail}); coordinator={coordinator_state} ({coordinator_detail})"
        ));
    }
    let age = journal_age_seconds(&path)
        .ok_or_else(|| format!("cannot stat journal {}", path.display()))?;
    let ttl = as_int(
        raw.get("heartbeat_ttl_seconds").unwrap_or(&Value::Null),
        "create journal.heartbeat_ttl_seconds",
        0,
    )?;
    let bound = MIN_AGE_SECONDS.max(ttl.max(0) as u64);
    if age < bound {
        return Err(format!(
            "create journal for slot {slot:?} is too young to quarantine: age {age}s is below the bound {bound}s (max(24h, heartbeat_ttl_seconds={ttl}))"
        ));
    }
    validate_global(root, cfg)?;
    let (states, archives) = load_states_archives(cfg)?;
    let config = load_config(root, machine_override)?;
    let slot_type = raw
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let recover_refusal = match create_journal_state(
        cfg, &config, &path, &states, &archives, slot, &slot_type,
    ) {
        Ok(state) => {
            let state = state.unwrap_or_default();
            if state == "completed-create" {
                return Err(format!(
                    "create journal for slot {slot:?} already completed; it is not stranded; run 'wrkslots recover' to clear it"
                ));
            }
            return Err(format!(
                "create journal for slot {slot:?} classifies as {state}, which recover resolves; run 'wrkslots recover --slot {slot} --coordinator-pid <pid>' (add --abort-create to roll it back) instead of quarantining it"
            ));
        }
        Err(error) => {
            if error.starts_with("busy:") {
                return Err(format!(
                    "create journal for slot {slot:?} is busy, not stranded: {error}"
                ));
            }
            if error.contains("changed while") {
                return Err(format!(
                    "create journal for slot {slot:?} changed while it was classified; rerun quarantine-journal"
                ));
            }
            error
        }
    };
    let slot_path = slot_directory(cfg, slot, &slot_type)?;
    let mut planned_paths: Vec<PathBuf> = vec![slot_path.clone()];
    let planned = as_list(
        raw.get("planned").unwrap_or(&Value::Null),
        "create journal.planned",
    )?;
    for (index, item) in planned.iter().enumerate() {
        let destination_raw = as_str(
            item.get("destination").unwrap_or(&Value::Null),
            &format!("create journal.planned[{index}].destination"),
        )?;
        planned_paths.push(stored_path(cfg, destination_raw, "checkout destination")?);
    }
    for (machine, slots) in &states {
        for record in slots {
            let record_slot = field(record, "slot");
            if record_slot == slot && *machine == cfg.machine {
                continue;
            }
            let record_type = {
                let value = field(record, "slot_type");
                if value.is_empty() {
                    "agent".to_string()
                } else {
                    value
                }
            };
            let other = slot_directory(cfg, &record_slot, &record_type)?;
            if planned_paths
                .iter()
                .any(|candidate| within(candidate, &other) || within(&other, candidate))
            {
                return Err(format!(
                    "create journal for slot {slot:?} overlaps the live slot {record_slot:?} on {machine}; quarantine would strand a claim on a live row"
                ));
            }
        }
    }
    let agent = validate_name(&field(&Value::Object(raw.clone()), "agent"), "agent")?.to_string();
    let journaled = quarantine_event_for(cfg, &path, &identity.3);
    let (already_sequence, already_path) = match &journaled {
        Some(event) => (
            event.get("sequence").and_then(Value::as_i64),
            event
                .get("payload")
                .and_then(|payload| payload.get("quarantine_path"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        None => (None, None),
    };
    let artifact_rows = artifacts(cfg, &raw, slot, &slot_type, &states, &archives)?;
    Ok(Plan {
        path,
        sha256: identity.3.clone(),
        slot: slot.to_string(),
        agent,
        slot_type,
        owner_pid: owner.as_ref().map(|identity| identity.pid),
        owner_state,
        coordinator_pid: coordinator.pid,
        coordinator_state,
        age_seconds: age,
        bound_seconds: bound,
        recover_refusal,
        artifacts: artifact_rows,
        already_sequence,
        already_path,
    })
}

fn reason_text(raw: &str) -> Result<String> {
    let reason = raw.trim().to_string();
    if reason.is_empty() {
        return Err(
            "quarantine-journal requires --reason saying why this journal must be set aside instead of recovered; nothing was changed"
                .to_string(),
        );
    }
    if reason.chars().count() > REASON_LIMIT || reason.contains('\0') {
        return Err(format!(
            "--reason must be at most {REASON_LIMIT} characters without NUL bytes"
        ));
    }
    Ok(reason)
}

fn identity_value(identity: &Identity) -> Value {
    json!({
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    })
}

fn build_payload(
    plan: &Plan,
    action: &str,
    reason: &str,
    quarantine_path: Option<&Path>,
    event_sequence: Option<i64>,
) -> Value {
    json!({
        "action": action,
        "age_seconds": plan.age_seconds,
        "bound_seconds": plan.bound_seconds,
        "already_journaled_sequence": plan.already_sequence,
        "artifacts": plan.artifacts,
        "event_sequence": event_sequence,
        "journal": absolute_path(&plan.path).to_string_lossy(),
        "journal_sha256": plan.sha256,
        "quarantine_path": quarantine_path.map(|path| absolute_path(path).to_string_lossy().into_owned()),
        "reason": reason,
        "recover_refusal": plan.recover_refusal,
        "slot": plan.slot,
        "agent": plan.agent,
        "slot_type": plan.slot_type,
        "owner_pid": plan.owner_pid,
        "owner_state": plan.owner_state,
        "coordinator_pid": plan.coordinator_pid,
        "coordinator_state": plan.coordinator_state,
    })
}

fn print_payload(payload: &Value, dry_run: bool) -> String {
    let journal = payload
        .get("journal")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sha256 = payload
        .get("journal_sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut out = if dry_run {
        format!("DRY RUN: would quarantine {journal}\n")
    } else {
        format!(
            "quarantined {journal} -> {}\n",
            payload
                .get("quarantine_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
    };
    out.push_str(&format!(
        "  slot={} agent={} sha256={} age={}s bound={}s\n",
        payload
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        payload
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        sha256,
        payload
            .get("age_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        payload
            .get("bound_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    ));
    out.push_str(&format!(
        "  recover refuses: {}\n",
        payload
            .get("recover_refusal")
            .and_then(Value::as_str)
            .unwrap_or_default()
    ));
    if let Some(sequence) = payload
        .get("already_journaled_sequence")
        .and_then(Value::as_i64)
    {
        out.push_str(&format!(
            "  quarantine already journaled at event {sequence}; the move did not complete\n"
        ));
    }
    if let Some(items) = payload.get("artifacts").and_then(Value::as_array) {
        for artifact in items {
            let state = match artifact.get("exists").and_then(Value::as_bool) {
                Some(true) => "exists",
                Some(false) => "absent",
                None => "unknown",
            };
            let subject = artifact
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| artifact.get("slot").and_then(Value::as_str))
                .unwrap_or("-");
            out.push_str(&format!(
                "  {}: {subject}: {state}\n",
                artifact
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ));
        }
    }
    if dry_run {
        out.push_str(&format!(
            "nothing changed; rerun with --yes --journal {journal} --journal-sha256 {sha256} to quarantine\n"
        ));
    }
    out
}

fn fsync_directory(directory: &Path) -> Result<()> {
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("cannot fsync directory {}: {error}", directory.display()))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn quarantine_journal(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    coordinator_pid: i64,
    coordinator_authorized: bool,
    reason: &str,
    yes: bool,
    journal: Option<&str>,
    journal_sha256: Option<&str>,
    format_json: bool,
) -> Result<String> {
    require_fixture()?;
    let reason = reason_text(reason)?;
    if !coordinator_authorized {
        return Err(
            "journal quarantine is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                .to_string(),
        );
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
    let slot = validate_name(slot, "slot")?.to_string();
    let plan = build_plan(root, machine_override, &cfg, &slot)?;
    if !yes {
        let payload = build_payload(&plan, "dry-run", &reason, None, None);
        return Ok(if format_json {
            format!(
                "{}\n",
                crate::batch_text::py_compact_pub(&sort_keys(&payload))
            )
        } else {
            print_payload(&payload, true)
        });
    }
    let (Some(journal), Some(journal_sha256)) = (journal, journal_sha256) else {
        return Err(
            "quarantine-journal --yes requires --journal and --journal-sha256 with the exact values the dry run printed"
                .to_string(),
        );
    };
    if journal != absolute_path(&plan.path).to_string_lossy() || journal_sha256 != plan.sha256 {
        return Err(format!(
            "the journal changed since the dry run: --journal/--journal-sha256 do not match {} sha256={}; rerun the dry run",
            absolute_path(&plan.path).display(),
            plan.sha256
        ));
    }
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    assert_caller(&coordinator, "coordinator")?;
    refuse_partial_state(&cfg.control)?;
    let plan = build_plan(root, machine_override, &cfg, &slot)?;
    if plan.sha256 != journal_sha256 {
        return Err(format!(
            "the journal changed since the dry run: sha256 is now {}; rerun the dry run",
            plan.sha256
        ));
    }
    let directory = quarantine_directory(&cfg);
    let destination = match &plan.already_path {
        Some(recorded) => {
            let destination = PathBuf::from(recorded);
            let parent_ok = destination.parent() == Some(directory.as_path())
                || destination.parent() == Some(absolute_path(&directory).as_path());
            if !parent_ok {
                return Err(format!(
                    "the journaled quarantine destination is outside the quarantine directory: {}",
                    destination.display()
                ));
            }
            destination
        }
        None => {
            let timestamp = chrono::DateTime::from_timestamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs() as i64)
                    .unwrap_or(0),
                0,
            )
            .map(|stamp| stamp.format("%Y%m%dT%H%M%SZ").to_string())
            .unwrap_or_default();
            directory.join(format!(
                "{timestamp}-{}",
                plan.path.file_name().unwrap_or_default().to_string_lossy()
            ))
        }
    };
    if destination.symlink_metadata().is_ok() {
        return Err(format!(
            "quarantine destination already exists: {}",
            destination.display()
        ));
    }
    let created = !directory.exists();
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "cannot create quarantine directory {}: {error}",
            directory.display()
        )
    })?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755));
    }
    if created {
        if let Some(parent) = directory.parent() {
            fsync_directory(parent)?;
        }
    }
    let sequence = match plan.already_sequence {
        Some(sequence) => Some(sequence),
        None => {
            ensure_event_log(root, &cfg, &cfg.machine)?;
            let event = append_event(
                &cfg,
                &cfg.machine,
                "operation-quarantined",
                json!({
                    "slot": plan.slot,
                    "operation": "create",
                    "journal_path": plan.path.file_name().unwrap_or_default().to_string_lossy(),
                    "original_path": absolute_path(&plan.path).to_string_lossy(),
                    "quarantine_path": absolute_path(&destination).to_string_lossy(),
                    "journal_sha256": plan.sha256,
                    "reason": reason,
                    "actor": identity_value(&coordinator),
                    "coordinator_authorized": true,
                    "recover_refusal": plan.recover_refusal,
                    "artifacts": plan.artifacts,
                }),
            )?;
            event.get("sequence").and_then(Value::as_i64)
        }
    };
    std::fs::rename(&plan.path, &destination).map_err(|error| {
        format!(
            "cannot move journal {} to {}: {error}",
            plan.path.display(),
            destination.display()
        )
    })?;
    fsync_directory(&cfg.control)?;
    fsync_directory(&directory)?;
    let payload = build_payload(&plan, "quarantined", &reason, Some(&destination), sequence);
    Ok(if format_json {
        format!(
            "{}\n",
            crate::batch_text::py_compact_pub(&sort_keys(&payload))
        )
    } else {
        print_payload(&payload, false)
    })
}
