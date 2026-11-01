//! Per-record status diagnosis parity.
//!
//! Reproduces the existing per-record status object: the normalized
//! record plus owner-process state, heartbeat age/expiry, and hold
//! outcome. Machines whose state is derived from the append-only
//! event log read their active document and holds from the chain
//! fold, like the Python status. The owner diagnosis reads this
//! host's `/proc`, boot id, and machine id exactly as the existing
//! reader does. Registry storage inconsistencies and event-derived
//! owner releases stay with the existing `status` command, so
//! `storage_inconsistencies` is reported empty. Read-only.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::active_state::{load_cfg, parse_state_ordered};
use crate::global_state::discover_machines;
use crate::record_holds::load_record_holds;

type Result<T> = std::result::Result<T, String>;

#[derive(PartialEq)]
pub(crate) struct Identity {
    pub(crate) pid: i64,
    pub(crate) start_ticks: i64,
    pub(crate) boot_id: String,
    pub(crate) host_id: String,
    pub(crate) cgroup_path: String,
}

fn host_id() -> std::result::Result<String, String> {
    let candidates = ["/etc/machine-id", "/var/lib/dbus/machine-id"];
    let mut failures: Vec<String> = Vec::new();
    for path in candidates {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let value = text.trim().to_string();
                if !value.is_empty() {
                    return Ok(value);
                }
                failures.push(format!("{path}: empty"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => failures.push(format!("{path}: {error}")),
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

pub(crate) fn boot_id() -> std::result::Result<String, String> {
    match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(text) => {
            let value = text.trim().to_string();
            if value.is_empty() {
                Err("machine boot id is empty".to_string())
            } else {
                Ok(value)
            }
        }
        Err(error) => Err(format!("cannot read the machine boot id: {error}")),
    }
}

fn parse_proc_stat(raw: &[u8], pid: i64) -> Option<i64> {
    let text = String::from_utf8_lossy(raw);
    let opening = text.find(" (")?;
    let closing = text.rfind(") ")?;
    if opening == 0 || closing < opening + 2 {
        return None;
    }
    let process: i64 = text[..opening].parse().ok()?;
    if process != pid || process <= 0 {
        return None;
    }
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    if fields.len() <= 19 || fields[0].len() != 1 {
        return None;
    }
    if !"RSDZTWtXxKPI".contains(fields[0]) {
        return None;
    }
    let parent: i64 = fields[1].parse().ok()?;
    let flags: i64 = fields[6].parse().ok()?;
    let start_ticks: i64 = fields[19].parse().ok()?;
    if parent < 0 || flags < 0 || start_ticks <= 0 {
        return None;
    }
    Some(start_ticks)
}

pub(crate) fn process_start_ticks(pid: i64) -> std::result::Result<Option<i64>, String> {
    let path = format!("/proc/{pid}/stat");
    match std::fs::read(&path) {
        Ok(raw) => parse_proc_stat(&raw, pid)
            .map(Some)
            .ok_or_else(|| format!("process generation is invalid for PID {pid}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "process generation is indeterminate because {path} is unreadable: {error}"
        )),
    }
}

pub(crate) fn read_process_identity(pid: i64) -> std::result::Result<Identity, String> {
    if pid <= 0 {
        return Err("owner PID must be positive".to_string());
    }
    let dir = format!("/proc/{pid}");
    let raw = match std::fs::read(format!("{dir}/stat")) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("owner PID {pid} is not live"));
        }
        Err(error) => {
            return Err(format!(
                "process generation is indeterminate because {dir}/stat is unreadable: {error}"
            ));
        }
    };
    let start_ticks = parse_proc_stat(&raw, pid)
        .ok_or_else(|| format!("process generation is invalid for PID {pid}"))?;
    let cgroup_text = std::fs::read_to_string(format!("{dir}/cgroup"))
        .map_err(|error| format!("cannot read process cgroup for PID {pid}: {error}"))?;
    let paths: Vec<&str> = cgroup_text
        .lines()
        .filter_map(|line| line.split_once("::").map(|(_, rest)| rest))
        .collect();
    if paths.len() != 1 || !paths[0].starts_with('/') {
        return Err(format!("cannot establish one unified cgroup for PID {pid}"));
    }
    let cgroup_path = {
        let trimmed = paths[0].trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let machine_level: Option<(&str, &str)> = match (pid, cgroup_path.as_str()) {
        (1, "/init.scope") => Some(("the machine's init process", "/init.scope")),
        (2, "/") => Some(("the kernel thread daemon", "/")),
        _ => None,
    };
    if let Some((name, _)) = machine_level {
        return Err(format!(
            "PID {pid} is {name}, which can never own, coordinate or act on a slot; recording it would store an identity that stays live until the machine reboots, so the slot could never be reclaimed and no later check could tell it from a real owner. remedy: pass the PID of the real process -- if the caller is PID 1 inside its own PID namespace this refusal does not apply to it, because such a process is not this machine's init"
        ));
    }
    Ok(Identity {
        pid,
        start_ticks,
        boot_id: boot_id()?,
        host_id: host_id()?,
        cgroup_path,
    })
}

pub(crate) fn process_state(identity: Option<&Identity>) -> (String, String) {
    let Some(identity) = identity else {
        return (
            "indeterminate".to_string(),
            "no owner process generation is recorded".to_string(),
        );
    };
    let host = match host_id() {
        Ok(host) => host,
        Err(message) => return ("indeterminate".to_string(), message),
    };
    if host != identity.host_id {
        return (
            "indeterminate".to_string(),
            "the recorded owner belongs to a different machine identity".to_string(),
        );
    }
    let boot = match boot_id() {
        Ok(boot) => boot,
        Err(message) => return ("indeterminate".to_string(), message),
    };
    if boot != identity.boot_id {
        return (
            "dead".to_string(),
            "the recorded machine boot ended".to_string(),
        );
    }
    match read_process_identity(identity.pid) {
        Ok(current) => {
            if current == *identity {
                (
                    "live".to_string(),
                    format!("PID {} generation is live", identity.pid),
                )
            } else {
                (
                    "dead".to_string(),
                    format!(
                        "PID {} was reused; the recorded generation exited",
                        identity.pid
                    ),
                )
            }
        }
        Err(message) => {
            if message.contains("is not live") {
                ("dead".to_string(), message)
            } else {
                ("indeterminate".to_string(), message)
            }
        }
    }
}

pub(crate) fn identity_from_value(value: &Value) -> Option<Identity> {
    let raw = value.as_object()?;
    Some(Identity {
        pid: raw.get("pid")?.as_i64()?,
        start_ticks: raw.get("start_ticks")?.as_i64()?,
        boot_id: raw.get("boot_id")?.as_str()?.to_string(),
        host_id: raw.get("host_id")?.as_str()?.to_string(),
        cgroup_path: raw.get("cgroup_path")?.as_str()?.to_string(),
    })
}

/// Build one status object per active record in machine then shard
/// order. A hold load error refuses unless `tolerate_hold_error`, in
/// which case it is reported in the record like the existing doctor.
pub(crate) fn status_records(
    root: Option<&Path>,
    tolerate_hold_error: bool,
) -> Result<(Vec<String>, Vec<Value>)> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let machines = discover_machines(&cfg.control)?;
    let (_hold_machines, hold_entries) = load_record_holds(root)?;
    let mut records: Vec<Value> = Vec::new();
    for machine in &machines {
        // Event-log-derived machines read their active document from
        // the chain fold, like the Python status; snapshot machines
        // read the shard.
        let events_dir = cfg.control.join(format!("EVENTS.{machine}"));
        let (active, order) = if matches!(
            crate::global_state::event_log_status(&events_dir),
            crate::global_state::EventLogStatus::Derived
        ) {
            let document = crate::hold_write::active_document(root, &cfg, machine)?;
            let order = document
                .get("slots")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|slot| crate::active_state::RecordOrder {
                    slot: slot
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    agent: slot
                        .get("agent")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    slot_type: slot
                        .get("slot_type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    imported: true,
                })
                .collect();
            (document, order)
        } else {
            parse_state_ordered(root, Some(machine))?
        };
        let slots = active
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let by_slot: std::collections::HashMap<&str, &Map<String, Value>> = slots
            .iter()
            .filter_map(|slot| {
                let obj = slot.as_object()?;
                Some((obj.get("slot")?.as_str()?, obj))
            })
            .collect();
        for record_order in &order {
            let stored = by_slot.get(record_order.slot.as_str()).ok_or_else(|| {
                format!(
                    "active record {} is missing its stored form",
                    record_order.slot
                )
            })?;
            let mut value = Value::Object((*stored).clone());
            let owner = value.get("owner").cloned().unwrap_or(Value::Null);
            let identity = if owner.is_null() {
                None
            } else {
                Some(identity_from_value(&owner).ok_or_else(|| {
                    format!(
                        "active record {} has an invalid owner identity",
                        record_order.slot
                    )
                })?)
            };
            let (state, detail) = process_state(identity.as_ref());
            let heartbeat_at = value
                .get("heartbeat_at")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("active record {} has no heartbeat", record_order.slot))?;
            let stamp = crate::schema::parse_timestamp_instant(
                heartbeat_at,
                &format!("slot {} heartbeat", record_order.slot),
            )
            .map_err(|error| error.to_string())?;
            let now_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or(0);
            let age = ((now_millis - stamp.timestamp_millis()) as f64 / 1000.0).max(0.0);
            let age_seconds = age as i64;
            let ttl = value
                .get("heartbeat_ttl_seconds")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let hold_entry = hold_entries
                .iter()
                .find(|entry| entry.machine == *machine && entry.slot == record_order.slot);
            if !tolerate_hold_error {
                if let Some(entry) = hold_entry {
                    if let Some(message) = &entry.error {
                        return Err(message.clone());
                    }
                }
            }
            let (held, hold_reason, hold_error) = match hold_entry {
                Some(entry) => {
                    let reason = entry
                        .hold
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    (!entry.hold.is_null(), reason, entry.error.clone())
                }
                None => (false, None, None),
            };
            let object = value
                .as_object_mut()
                .ok_or_else(|| "active record is not an object".to_string())?;
            object.insert("owner_state".to_string(), json!(state));
            object.insert("owner_detail".to_string(), json!(detail));
            object.insert("heartbeat_age_seconds".to_string(), json!(age_seconds));
            object.insert("heartbeat_expired".to_string(), json!(age > ttl as f64));
            object.insert("held".to_string(), json!(held));
            object.insert("hold_reason".to_string(), json!(hold_reason));
            object.insert("hold_error".to_string(), json!(hold_error));
            object.insert(
                "storage_inconsistencies".to_string(),
                Value::Array(Vec::new()),
            );
            records.push(value);
        }
    }
    Ok((machines, records))
}

/// Diagnose every active record across all machines, printing one
/// status object per record in machine then shard order.
pub(crate) fn parse_status(root: Option<&Path>) -> Result<Value> {
    let (machines, records) = status_records(root, false)?;
    Ok(json!({
        "machines": machines,
        "records": records,
    }))
}

/// The Python `status` command's output, envelope and human form.
///
/// Records come from [`status_records`]; the envelope, journals list
/// and human rendering mirror `_cmd_status`. Storage inconsistencies
/// are the filesystem and Git censuses (`parse_storage`,
/// `parse_storage_git`) merged and sorted the way the Python status
/// sorts them, instead of the empty list `parse_status` reports.
pub(crate) fn status_cli(
    root: Option<&Path>,
    machine_override: Option<&str>,
    all_machines: bool,
    slot_filter: Option<&str>,
    human: bool,
) -> Result<String> {
    let (cfg, selected) = load_cfg(root, machine_override)?;
    // Refuse a partial atomic update, like the Python status.
    {
        let mut leftovers: Vec<std::path::PathBuf> = Vec::new();
        let patterns = [
            "ACTIVE.",
            "ARCHIVED.",
            "CREATE.",
            "FINISH.",
            "HANDOFF.",
            "VALIDATE-BATCH-SEAL.",
        ];
        if let Ok(entries) = std::fs::read_dir(&cfg.control) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains(".tmp.") && patterns.iter().any(|prefix| name.starts_with(prefix))
                {
                    leftovers.push(entry.path());
                }
                if name.starts_with("EVENTS.") && entry.path().is_dir() {
                    if let Ok(inner) = std::fs::read_dir(entry.path()) {
                        for item in inner.flatten() {
                            let inner_name = item.file_name().to_string_lossy().into_owned();
                            if inner_name.ends_with(".json.tmp")
                                || inner_name.contains(".json.tmp.")
                            {
                                leftovers.push(item.path());
                            }
                        }
                    }
                }
            }
        }
        leftovers.sort();
        if let Some(first) = leftovers.first() {
            return Err(format!(
                "partial atomic update found: {}; preserve it and use 'wrkslots recover'",
                first.display()
            ));
        }
    }
    let (all_machine_names, records_all) = status_records(root, false)?;
    let machines: Vec<String> = if all_machines {
        all_machine_names.clone()
    } else {
        all_machine_names
            .iter()
            .filter(|machine| *machine == &selected)
            .cloned()
            .collect()
    };
    // Journals: outstanding recovery journals plus seal journals, as
    // file names, in the Python command's order.
    let journals_value = crate::journals_state::parse_journals(root)?;
    let mut journals: Vec<String> = journals_value
        .get("journals")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    // Seal journals follow the ordinary journals, as in Python.
    {
        let mut seals: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&cfg.control) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("VALIDATE-BATCH-SEAL.") && name.ends_with(".journal") {
                    seals.push(name);
                }
            }
        }
        seals.sort();
        journals.extend(seals);
    }
    // Storage census: filesystem then Git findings, sorted by the
    // storage sort key, exactly as the Python status merges them.
    let mut inconsistencies: Vec<Value> = Vec::new();
    if let Some(findings) = crate::storage_state::parse_storage(root)?
        .get("findings")
        .and_then(Value::as_array)
    {
        inconsistencies.extend(findings.iter().cloned());
    }
    match crate::git_storage_state::parse_storage_git(root) {
        Ok(census) => {
            if let Some(findings) = census.get("findings").and_then(Value::as_array) {
                inconsistencies.extend(findings.iter().cloned());
            }
        }
        Err(error) if error.contains("does not exist or cannot be resolved") => {
            // The Python status tolerates an unavailable source
            // repository as a per-checkout finding instead of
            // refusing the whole command.
            for record in &records_all {
                let text = |key: &str| {
                    record
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
                    for checkout in checkouts {
                        let repository = checkout
                            .get("repository")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !error.contains(&format!("'{repository}'"))
                            && !error.contains(&format!("\"{repository}\""))
                        {
                            continue;
                        }
                        let name = checkout
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        inconsistencies.push(json!({
                            "kind": "repository-evidence-unavailable",
                            "scope": "row",
                            "slot": text("slot"),
                            "slot_type": text("slot_type"),
                            "machine": text("machine"),
                            "checkout": name,
                            "detail": format!(
                                "cannot inspect repository for checkout {name}: {error}"
                            ),
                            "remedy": "restore the recorded source repository, or record where it moved with 'wrkslots relocate-repository', then rerun 'wrkslots status'; no mutation may rely on unavailable Git evidence",
                        }));
                    }
                }
            }
        }
        Err(error) => return Err(error),
    }
    inconsistencies.sort_by_key(crate::storage_state::storage_sort_key);
    let mut records: Vec<Value> = Vec::new();
    for mut record in records_all {
        if !machines
            .iter()
            .any(|machine| Some(machine.as_str()) == record.get("machine").and_then(Value::as_str))
        {
            continue;
        }
        if let Some(wanted) = slot_filter {
            if record.get("slot").and_then(Value::as_str) != Some(wanted) {
                continue;
            }
        }
        let machine = record
            .get("machine")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let slot = record
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mine: Vec<Value> = inconsistencies
            .iter()
            .filter(|item| {
                item.get("machine").and_then(Value::as_str) == Some(machine.as_str())
                    && item.get("slot").and_then(Value::as_str) == Some(slot.as_str())
            })
            .cloned()
            .collect();
        if let Some(object) = record.as_object_mut() {
            object.insert("storage_inconsistencies".to_string(), Value::Array(mine));
        }
        records.push(record);
    }
    if !human {
        let envelope = json!({
            "schema": 2,
            "project_root": cfg.root.to_string_lossy(),
            "worktrees_dir": cfg.worktrees.to_string_lossy(),
            "control_dir": cfg.control.to_string_lossy(),
            "layout": cfg.layout,
            "machines": machines,
            "active": records,
            "journals": journals,
            "registry_storage_state": if inconsistencies.is_empty() { "consistent" } else { "inconsistent" },
            "registry_storage_inconsistencies": inconsistencies,
        });
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope)
                .map_err(|error| format!("cannot encode status: {error}"))?
        ));
    }
    let mut event_count = 0usize;
    for machine in &machines {
        let dir = cfg.control.join(format!("EVENTS.{machine}"));
        if dir.is_dir() {
            if let Ok(events) = crate::global_state::load_event_chain(&dir, machine) {
                event_count += events.len();
            }
        }
    }
    let mut out = format!(
        "project={} worktrees={} control={} layout={} active={} journals={} events={} registry_storage={} inconsistencies={}\n",
        cfg.root.display(),
        cfg.worktrees.display(),
        cfg.control.display(),
        cfg.layout,
        records.len(),
        journals.len(),
        event_count,
        if inconsistencies.is_empty() { "CONSISTENT" } else { "INCONSISTENT" },
        inconsistencies.len(),
    );
    for journal in &journals {
        out.push_str(&format!("RECOVERY REQUIRED: {journal}\n"));
    }
    for record in &records {
        let text = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let number = |key: &str| record.get(key).and_then(Value::as_i64).unwrap_or(0);
        let flag = |key: &str| record.get(key).and_then(Value::as_bool).unwrap_or(false);
        let row_inconsistencies = record
            .get("storage_inconsistencies")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        out.push_str(&format!(
            "{}/{}: type={} agent={} task={} generation={} owner={} heartbeat_age={}s expired={} held={} inconsistencies={}\n",
            text("machine"),
            text("slot"),
            text("slot_type"),
            text("agent"),
            text("task"),
            number("generation"),
            text("owner_state"),
            number("heartbeat_age_seconds"),
            if flag("heartbeat_expired") { "yes" } else { "no" },
            if flag("held") { "yes" } else { "no" },
            row_inconsistencies,
        ));
        if let Some(release) = record.get("owner_release").filter(|value| !value.is_null()) {
            out.push_str(&format!(
                "  {} (event {})\n",
                release
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                release
                    .get("event_sequence")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            ));
        }
    }
    for item in &inconsistencies {
        let text = |key: &str| item.get(key).and_then(Value::as_str);
        let identity = [
            text("machine"),
            text("slot_type"),
            text("slot"),
            text("checkout"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("/");
        out.push_str(&format!(
            "INCONSISTENT kind={} target={}: {}\n  REMEDY: {}\n",
            text("kind").unwrap_or_default(),
            if identity.is_empty() {
                "-".to_owned()
            } else {
                identity
            },
            text("detail").unwrap_or_default(),
            text("remedy").unwrap_or_default(),
        ));
    }
    Ok(out)
}
