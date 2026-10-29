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

fn boot_id() -> std::result::Result<String, String> {
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
