//! Writer: bind an unowned slot to its live owner.
//!
//! Reproduces the existing `adopt` mutation: caller capture,
//! generation and agent checks, the already-adopted and
//! historical-owner outcomes, the optional systemd task-scope
//! verification, and the `owner-bound` active-state event plus
//! ACTIVE shard rewrite. Fixture registries only until the edition
//! cutover (see `ensure_states::require_fixture`).

use std::path::Path;

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};

type Result<T> = std::result::Result<T, String>;

fn validate_agent_name(agent: &str) -> Result<()> {
    let valid = agent.len() <= 32
        && agent
            .chars()
            .next()
            .map(|ch| ch.is_ascii_lowercase())
            .unwrap_or(false)
        && agent
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && !agent.is_empty()
        && agent != "archive";
    if !valid {
        return Err(format!(
            "invalid agent '{agent}'; agent names must start with a lowercase letter and contain 1-32 lowercase letters, digits or hyphens, and 'archive' is reserved. state: REFUSED -- no slot or lifecycle record was changed. remedy: choose a name the session tooling can register; any other name could never be reported alive or dead by its liveness probe, so a slot recorded under it could never be proven free"
        ));
    }
    Ok(())
}

fn is_format_char(ch: char) -> bool {
    matches!(ch,
        '\u{00ad}' | '\u{0600}'..='\u{0605}' | '\u{061c}' | '\u{06dd}' | '\u{070f}'
        | '\u{0890}'..='\u{0891}' | '\u{08e2}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}'
        | '\u{feff}' | '\u{fff9}'..='\u{fffb}')
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
}

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn assert_caller(identity: &Identity) -> Result<()> {
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
        "owner PID {} is not in the invoking process ancestry",
        identity.pid
    ))
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

fn record_identity(record: &Value) -> Option<Identity> {
    let value = record.get("owner")?;
    if value.is_null() {
        return None;
    }
    Some(Identity {
        pid: value.get("pid")?.as_i64()?,
        start_ticks: value.get("start_ticks")?.as_i64()?,
        boot_id: value.get("boot_id")?.as_str()?.to_string(),
        host_id: value.get("host_id")?.as_str()?.to_string(),
        cgroup_path: value.get("cgroup_path")?.as_str()?.to_string(),
    })
}

fn verify_task_scope(owner: &Identity, unit: &str, invocation_id: &str) -> Result<Value> {
    if !unit.ends_with(".scope")
        || unit.contains('/')
        || unit
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || is_format_char(ch))
    {
        return Err("task scope.unit is not a task-scoped systemd unit".to_string());
    }
    if invocation_id.len() != 32
        || !invocation_id
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    {
        return Err(
            "task scope.invocation_id is not a canonical systemd invocation ID".to_string(),
        );
    }
    if !owner.cgroup_path.starts_with('/')
        || !owner.cgroup_path.ends_with(&format!("/{unit}"))
        || owner.cgroup_path.contains("/../")
        || !is_uuid(&owner.boot_id)
    {
        return Err("task scope has an invalid cgroup path or boot ID".to_string());
    }
    let uid = rustix::process::getuid().as_raw();
    let manager = format!("/user.slice/user-{uid}.slice/user@{uid}.service");
    if !owner.cgroup_path.starts_with(&format!("{manager}/")) {
        return Err(format!(
            "task scope cgroup {} is not under the systemd user manager {manager} whose invocation link is verified",
            owner.cgroup_path
        ));
    }
    let rest = owner.cgroup_path[manager.len() + 1..].to_string();
    let parents: Vec<&str> = rest.split('/').collect();
    if parents[..parents.len().saturating_sub(1)]
        .iter()
        .any(|parent| !parent.ends_with(".slice"))
    {
        return Err(format!(
            "task scope cgroup {} is not a scope placed by the systemd user manager {manager}: every component between them must be a slice",
            owner.cgroup_path
        ));
    }
    let path = Path::new("/run/user")
        .join(uid.to_string())
        .join("systemd/units")
        .join(format!("invocation:{unit}"));
    use std::os::unix::fs::MetadataExt as _;
    let before = std::fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "cannot verify task scope invocation through systemd runtime link {}: {error}",
            path.display()
        )
    })?;
    if !before.file_type().is_symlink() {
        return Err(format!(
            "task scope invocation is not a systemd runtime symlink: {}",
            path.display()
        ));
    }
    let target = std::fs::read_link(&path).map_err(|error| {
        format!(
            "cannot verify task scope invocation through systemd runtime link {}: {error}",
            path.display()
        )
    })?;
    let after = std::fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "cannot verify task scope invocation through systemd runtime link {}: {error}",
            path.display()
        )
    })?;
    if !after.file_type().is_symlink() || (before.dev(), before.ino()) != (after.dev(), after.ino())
    {
        return Err(format!(
            "task scope invocation does not match the stable systemd runtime link {}",
            path.display()
        ));
    }
    let target = target.to_string_lossy().into_owned();
    if target != invocation_id {
        return Err(format!(
            "task scope invocation does not match the stable systemd runtime link {}",
            path.display()
        ));
    }
    Ok(json!({
        "unit": unit,
        "invocation_id": invocation_id,
        "cgroup_path": owner.cgroup_path,
        "boot_id": owner.boot_id,
        "leader_pid": owner.pid,
        "leader_start_ticks": owner.start_ticks,
        "verification": "systemd-runtime-invocation-symlink-v1",
    }))
}

/// Bind the named unowned slot to its live owner.
#[allow(clippy::too_many_arguments)]
pub(crate) fn adopt_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
    task_scope_unit: Option<&str>,
    task_scope_invocation_id: Option<&str>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    validate_agent_name(agent)?;
    if task_scope_unit.is_some() != task_scope_invocation_id.is_some() {
        return Err(
            "--task-scope-unit and --task-scope-invocation-id must be supplied together"
                .to_string(),
        );
    }
    let supplied = read_process_identity(owner_pid)?;
    assert_caller(&supplied)?;
    let task_scope = match (task_scope_unit, task_scope_invocation_id) {
        (Some(unit), Some(invocation)) => Some(verify_task_scope(&supplied, unit, invocation)?),
        _ => None,
    };
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        if let Some(first) = names.first().and_then(Value::as_str) {
            return Err(format!(
                "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                cfg.control.join(first).display()
            ));
        }
    }
    validate_global(root, &cfg)?;
    refuse_target_storage(root, slot)?;
    let active = active_document(root, &cfg, &selected)?;
    let record = active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!(
            "stale generation for slot {slot}: expected {expected_generation}, current {generation}"
        ));
    }
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if record_agent != agent {
        return Err(format!(
            "ownership mismatch for slot {slot}: expected {record_agent}, got {agent}"
        ));
    }
    let existing = record_identity(&record);
    if let Some(owner) = &existing {
        if *owner == supplied
            && (task_scope.is_none()
                || record.get("task_scope").cloned().unwrap_or(Value::Null)
                    == task_scope.clone().unwrap_or(Value::Null))
        {
            return Ok(json!({
                "machine": selected,
                "slot": slot,
                "agent": agent,
                "generation": generation,
                "owner_pid": supplied.pid,
                "already_adopted": true,
            }));
        }
        let (state, detail) = process_state(Some(owner));
        return Err(format!(
            "slot {slot} already has a {state} historical owner: {detail}; ordinary adopt cannot replace it"
        ));
    }
    let confirmed = read_process_identity(supplied.pid)?;
    if confirmed != supplied {
        return Err("owner process generation changed during adopt".to_string());
    }
    assert_caller(&confirmed)?;
    if let (Some(unit), Some(invocation)) = (task_scope_unit, task_scope_invocation_id) {
        let reverified = verify_task_scope(&supplied, unit, invocation)?;
        if Some(&reverified) != task_scope.as_ref() {
            return Err("task scope changed during adopt".to_string());
        }
    }
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let stamp = utc_now();
    let mut updated = record.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("owner".to_string(), identity_value(&supplied));
        object.insert("heartbeat_at".to_string(), json!(stamp));
        if let Some(scope) = &task_scope {
            object.insert("task_scope".to_string(), scope.clone());
        }
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = updated.clone();
        }
    }
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "owner-bound",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": updated,
            "evidence": {"owner": identity_value(&supplied)},
        }),
    )?;
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    validate_global(root, &cfg)?;
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "agent": agent,
        "generation": generation,
        "owner_pid": supplied.pid,
        "already_adopted": false,
    }))
}
