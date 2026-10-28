//! State bootstrap writer parity (first writer slice).
//!
//! Reproduces `_ensure_state_shard` for the selected machine: a lone
//! empty shard gains its empty counterpart, both missing shards are
//! created empty, present shards are validated, and the event log is
//! initialized with a `state-imported` event when it has no complete
//! events. Writes go through the same held registry locks, atomic
//! replace (leftover `.tmp.` refusal, fsynced file and directory), and
//! hash-chained event append as the existing writer. A log that
//! derives state folds here only when every event is a
//! `state-imported`; other derived logs stay with `rebuild`/Python.
//! Fixture registries only: this command is differential-tested on
//! temporary directories and is not a live-registry cutover.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::active_state::{load_cfg, parse_state_ordered, Cfg};
use crate::archive_state::parse_archive;
use crate::global_state::{
    event_log_status, load_event_chain, partial_write_refusal, EventLogStatus,
};
use crate::hold_state::parse_hold;

type Result<T> = std::result::Result<T, String>;

pub(crate) struct Flock {
    _file: std::fs::File,
}

pub(crate) fn acquire_lock(path: &Path, subject: &Path) -> Result<Flock> {
    if let Some(parent) = path.parent() {
        if parent
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
            .unwrap_or(true)
        {
            return Err(format!("unsafe lock parent: {}", parent.display()));
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("cannot open state lock {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect state lock {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "state lock is not a regular file: {}",
            path.display()
        ));
    }
    match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Flock { _file: file }),
        Err(_) => Err(format!(
            "state lock is busy for {}; retry after the current command exits",
            subject.display()
        )),
    }
}

fn fsync_directory(directory: &Path) -> Result<()> {
    let dir = std::fs::File::open(directory)
        .map_err(|error| format!("cannot fsync directory {}: {error}", directory.display()))?;
    dir.sync_all()
        .map_err(|error| format!("cannot fsync directory {}: {error}", directory.display()))
}

fn refuse_symlink(path: &Path, label: &str) -> Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("{label} must not be a symlink: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect {label} {}: {error}",
            path.display()
        )),
    }
}

pub(crate) fn escape_ascii_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if (ch as u32) < 0x7f {
            out.push(ch);
        } else {
            let mut buffer = [0u16; 2];
            for unit in ch.encode_utf16(&mut buffer) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

pub(crate) fn atomic_write_json(path: &Path, payload: &Value) -> Result<()> {
    let pretty = serde_json::to_string_pretty(payload).map_err(|error| error.to_string())?;
    let mut text = escape_ascii_json(&pretty);
    text.push('\n');
    atomic_write_bytes(path, text.as_bytes())
}

fn atomic_write_bytes(path: &Path, contents: &[u8]) -> Result<()> {
    refuse_symlink(path, "state file")?;
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot prepare state directory {}: {error}",
            parent.display()
        )
    })?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!("{name}.tmp.");
    let entries = std::fs::read_dir(parent).map_err(|error| {
        format!(
            "cannot prepare state directory {}: {error}",
            parent.display()
        )
    })?;
    let mut leftovers: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot prepare state directory {}: {error}",
                parent.display()
            )
        })?;
        leftovers.push(entry.path());
    }
    let mut leftovers: Vec<PathBuf> = leftovers
        .into_iter()
        .filter(|candidate| {
            candidate
                .file_name()
                .map(|file| file.to_string_lossy().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    leftovers.sort();
    if let Some(first) = leftovers.first() {
        return Err(format!(
            "partial state update exists for {name}: {}; inspect it and use 'wrkslots recover'",
            first.display()
        ));
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let temp = parent.join(format!("{prefix}{}.{stamp:x}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_CLOEXEC)
        .open(&temp)
        .map_err(|error| {
            format!(
                "cannot create temporary state file {}: {error}",
                temp.display()
            )
        })?;
    let result = (|| -> Result<()> {
        file.write_all(contents).map_err(|error| {
            format!(
                "cannot durably replace state file {}: {error}",
                path.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "cannot durably replace state file {}: {error}",
                path.display()
            )
        })?;
        drop(file);
        std::fs::rename(&temp, path).map_err(|error| {
            format!(
                "cannot durably replace state file {}: {error}",
                path.display()
            )
        })?;
        fsync_directory(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub(crate) fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|stamp| stamp.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
        .unwrap_or_default()
}

pub(crate) fn append_event(cfg: &Cfg, machine: &str, kind: &str, payload: Value) -> Result<Value> {
    let directory = cfg.control.join(format!("EVENTS.{machine}"));
    if directory.exists()
        && directory
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
            .unwrap_or(false)
    {
        return Err(format!(
            "append-only event log is not a real directory: {}",
            directory.display()
        ));
    }
    let created = !directory.exists();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create event log {}: {error}", directory.display()))?;
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
    let events = load_event_chain(&directory, machine)?;
    let sequence = events.len() + 1;
    let previous = events
        .last()
        .and_then(|event| event.get("sha256"))
        .and_then(Value::as_str)
        .unwrap_or(&"0".repeat(64))
        .to_string();
    let mut core = Map::new();
    core.insert("schema".to_string(), json!(1));
    core.insert("machine".to_string(), json!(machine));
    core.insert("sequence".to_string(), json!(sequence));
    core.insert("previous_sha256".to_string(), json!(previous));
    core.insert("recorded_at".to_string(), json!(utc_now()));
    core.insert("kind".to_string(), json!(kind));
    core.insert("payload".to_string(), payload);
    let digest = crate::canonical::canonical_sha256(&Value::Object(core.clone()))
        .map_err(|error| error.to_string())?;
    let mut event = core;
    event.insert("sha256".to_string(), json!(digest));
    let event = Value::Object(event);
    let path = directory.join(format!("{sequence:020}.json"));
    if path.symlink_metadata().is_ok() {
        return Err(format!(
            "append-only event already exists and will not be replaced: {}",
            path.display()
        ));
    }
    atomic_write_json(&path, &event)?;
    Ok(event)
}

fn empty_active(machine: &str) -> Value {
    json!({"schema": 2, "machine": machine, "revision": 0, "slots": []})
}

fn empty_archive(machine: &str) -> Value {
    json!({"schema": 2, "machine": machine, "revision": 0, "records": []})
}

pub(crate) fn ensure_event_log(root: Option<&Path>, cfg: &Cfg, machine: &str) -> Result<()> {
    let directory = cfg.control.join(format!("EVENTS.{machine}"));
    if let EventLogStatus::Invalid(message) = event_log_status(&directory) {
        return Err(message);
    }
    if let EventLogStatus::Derived = event_log_status(&directory) {
        return Ok(());
    }
    let (active, order) = parse_state_ordered(root, Some(machine))?;
    let archive = parse_archive(root, Some(machine))?;
    let slots = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for record in &slots {
        if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
            for checkout in checkouts {
                if let Some(repository) = checkout.get("repository").and_then(Value::as_str) {
                    crate::active_state::stored_repository_parts(cfg, repository)?;
                }
            }
        }
    }
    let _ = &order;
    let mut holds: Vec<Value> = Vec::new();
    for record in &order {
        let hold = parse_hold(root, Some(machine), &record.slot)?;
        if !hold.is_null() {
            holds.push(hold);
        }
    }
    if let Some(message) = partial_write_refusal(&directory) {
        return Err(message);
    }
    append_event(
        cfg,
        machine,
        "state-imported",
        json!({"active": active, "archive": archive, "holds": holds}),
    )?;
    Ok(())
}

/// Bootstrap the selected machine's shard pair and event log the way
/// `_ensure_state_shard` does, printing the resulting pair.
pub(crate) fn require_fixture() -> Result<()> {
    if std::env::var("WRKSLOTSD_FIXTURE_WRITER").ok().as_deref() != Some("1") {
        return Err("this writer is fixture-only: set WRKSLOTSD_FIXTURE_WRITER=1 only for a temporary registry (the live registry stays on the Python edition until an approved cutover)".to_string());
    }
    Ok(())
}

/// Refuse when a partial atomic update is present in the control
/// directory, matching the existing `_refuse_partial_state` scan.
pub(crate) fn refuse_partial_state(control: &Path) -> Result<()> {
    refuse_partial_state_inner(control, false)
}

pub(crate) fn refuse_partial_state_allow_seals(control: &Path) -> Result<()> {
    refuse_partial_state_inner(control, true)
}

fn refuse_partial_state_inner(control: &Path, allow_validate_batch_seals: bool) -> Result<()> {
    let patterns: [(&str, &str); 7] = [
        ("ACTIVE.", ".json.tmp."),
        ("ARCHIVED.", ".json.tmp."),
        ("ACTIVE.", ".journal.tmp."),
        ("CREATE.", ".journal.tmp."),
        ("FINISH.", ".journal.tmp."),
        ("HANDOFF.", ".json.tmp."),
        ("VALIDATE-BATCH-SEAL.", ".journal.tmp."),
    ];
    let mut leftovers: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            control.display()
        )
    })?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot list control directory {}: {error}",
                control.display()
            )
        })?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    for (prefix, marker) in patterns {
        let mut group: Vec<PathBuf> = names
            .iter()
            .filter(|name| {
                name.strip_prefix(prefix).is_some_and(|rest| {
                    rest.find(marker)
                        .is_some_and(|at| at > 0 && at + marker.len() < rest.len())
                })
            })
            .map(|name| control.join(name))
            .collect();
        group.sort();
        leftovers.append(&mut group);
    }
    let mut event_names: Vec<String> = names
        .iter()
        .filter(|name| name.starts_with("EVENTS."))
        .cloned()
        .collect();
    event_names.sort();
    for name in event_names {
        let directory = control.join(&name);
        let is_real_dir = directory
            .symlink_metadata()
            .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
            .unwrap_or(false);
        if !is_real_dir {
            continue;
        }
        let mut group: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                if file.contains(".json.tmp.") {
                    group.push(directory.join(file));
                }
            }
        }
        group.sort();
        leftovers.append(&mut group);
    }
    if let Some(first) = leftovers.first() {
        return Err(format!(
            "partial atomic update found: {}; preserve it and use 'wrkslots recover'",
            first.display()
        ));
    }
    if allow_validate_batch_seals {
        return Ok(());
    }
    let mut seals: Vec<PathBuf> = names
        .iter()
        .filter(|name| name.starts_with("VALIDATE-BATCH-SEAL.") && name.ends_with(".journal"))
        .map(|name| control.join(name))
        .collect();
    seals.sort();
    if let Some(first) = seals.first() {
        return Err(format!(
            "interrupted validation-batch seal recorded in {}; run 'wrkslots recover' first",
            first.display()
        ));
    }
    Ok(())
}

pub(crate) fn ensure_states(root: Option<&Path>, machine_override: Option<&str>) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    crate::config_validate::validate_name(&selected, "machine")?;
    let _global = acquire_lock(
        &cfg.control.join("ACTIVE.global.lock"),
        &cfg.control.join("ACTIVE"),
    )?;
    let _machine = acquire_lock(
        &cfg.control.join(format!("ACTIVE.{selected}.json.lock")),
        &cfg.control.join(format!("ACTIVE.{selected}.json")),
    )?;
    ensure_states_locked(root, &cfg, &selected)
}

/// Bootstrap the shard pair and event log assuming the registry
/// locks are already held by the caller.
pub(crate) fn ensure_states_locked(
    root: Option<&Path>,
    cfg: &Cfg,
    selected: &str,
) -> Result<Value> {
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    let archive_path = cfg.control.join(format!("ARCHIVED.{selected}.json"));
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    if let EventLogStatus::Invalid(message) = event_log_status(&directory) {
        return Err(message);
    }
    if active_path.symlink_metadata().is_ok() && archive_path.symlink_metadata().is_ok() {
        let (active, _) = parse_state_ordered(root, Some(selected))?;
        let archive = parse_archive(root, Some(selected))?;
        ensure_event_log(root, cfg, selected)?;
        return Ok(json!({"machine": selected, "active": active, "archive": archive}));
    }
    if let EventLogStatus::Derived = event_log_status(&directory) {
        let events = load_event_chain(&directory, selected)?;
        let imports: Vec<&Value> = events
            .iter()
            .filter(|event| event.get("kind").and_then(Value::as_str) == Some("state-imported"))
            .collect();
        if imports.len() > 1 {
            return Err("append-only event log imports state more than once".to_string());
        }
        if imports.len() != events.len() {
            return Err(format!(
                "machine state for {selected} is derived from the append-only event log; use rebuild instead of snapshot pairing"
            ));
        }
        let last = events.last().cloned().unwrap_or(Value::Null);
        let payload = last.get("payload").cloned().unwrap_or(Value::Null);
        let raw_active = payload
            .get("active")
            .cloned()
            .ok_or_else(|| "state-imported event has no active state".to_string())?;
        let raw_archive = payload
            .get("archive")
            .cloned()
            .ok_or_else(|| "state-imported event has no archive state".to_string())?;
        let active = crate::active_state::normalize_active_value(
            cfg,
            &raw_active,
            selected,
            "state-imported event active",
        )?;
        let archive = crate::archive_state::normalize_archive_value(
            cfg,
            &raw_archive,
            selected,
            "state-imported event archive",
        )?;
        atomic_write_json(&active_path, &active)?;
        atomic_write_json(&archive_path, &archive)?;
        return Ok(json!({"machine": selected, "active": active, "archive": archive}));
    }
    let active_exists = active_path.symlink_metadata().is_ok();
    let archive_exists = archive_path.symlink_metadata().is_ok();
    match (active_exists, archive_exists) {
        (true, false) => {
            let (active, _) = parse_state_ordered(root, Some(selected))?;
            let revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
            let slots = active
                .get("slots")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            if revision != 0 || slots != 0 {
                return Err(format!(
                    "partial machine state for {selected}: non-empty ACTIVE has no ARCHIVED"
                ));
            }
            atomic_write_json(&archive_path, &empty_archive(selected))?;
        }
        (false, true) => {
            let archive = parse_archive(root, Some(selected))?;
            let revision = archive.get("revision").and_then(Value::as_i64).unwrap_or(0);
            let records = archive
                .get("records")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            if revision != 0 || records != 0 {
                return Err(format!(
                    "partial machine state for {selected}: non-empty ARCHIVED has no ACTIVE"
                ));
            }
            atomic_write_json(&active_path, &empty_active(selected))?;
        }
        (true, true) => {
            let _ = parse_state_ordered(root, Some(selected))?;
            let _ = parse_archive(root, Some(selected))?;
        }
        (false, false) => {
            atomic_write_json(&active_path, &empty_active(selected))?;
            atomic_write_json(&archive_path, &empty_archive(selected))?;
        }
    }
    ensure_event_log(root, cfg, selected)?;
    let (active, _) = parse_state_ordered(root, Some(selected))?;
    let archive = parse_archive(root, Some(selected))?;
    Ok(json!({"machine": selected, "active": active, "archive": archive}))
}
