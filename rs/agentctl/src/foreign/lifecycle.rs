//! Worker lifecycle filesystem: inbox, events, migration markers, locks.
//!
//! The per-worker files a runner and its callers coordinate through —
//! queued turns, the transcript and last-answer files, stop and migration
//! markers, staged-runner identities, the central event log, and the
//! per-worker harness lock — plus runner liveness and termination through
//! exact process handles. Registry records themselves live in
//! [`super::registry`]; process evidence and cleanup in
//! [`super::ownership`].

use std::io::Write as _;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::ownership::{
    current_boot_id_at, read_stat_at, stop_process_at, ForeignOwnershipError, ProcessIdentity,
};
use super::procstat::ProcessStat;
use super::registry::{
    agent_dir_at, failed_dir_at, inbox_dir_at, inflight_dir_at, owned_by_record, processed_dir_at,
    read_active_harness_at, state_dir, terminate_active_harness_at, with_registry_lock_at,
    write_durable_json, AgentRecord, Message,
};

/// How long migration waits for a runner identity step (`$SUBAGENTS_MIGRATION_READY_TIMEOUT`).
pub fn migration_ready_timeout() -> Duration {
    std::env::var("SUBAGENTS_MIGRATION_READY_TIMEOUT")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .and_then(|seconds| Duration::try_from_secs_f64(seconds.max(0.0)).ok())
        .unwrap_or(Duration::from_secs(10))
}

/// The current local time as a second-precision ISO timestamp (registry and transcript records).
pub fn now_iso() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

fn io_error(context: &str, path: &Path, error: &std::io::Error) -> ForeignOwnershipError {
    ForeignOwnershipError {
        code: "registry_io",
        message: format!("cannot {context} {}: {error}", path.display()),
    }
}

/// The worker transcript used for headless turn reads.
pub fn transcript_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("transcript.log"))
}

/// The most recent captured headless answer.
pub fn last_message_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("last-message.txt"))
}

/// The runner process-identity file.
pub fn runner_pid_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("runner.pid"))
}

/// The runner diagnostic log.
pub fn runner_stderr_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("runner.stderr.log"))
}

/// The stop marker checked between worker turns.
pub fn stop_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("STOP"))
}

/// The marker requesting that the source runner pause intake.
pub fn migration_pause_path_at(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("MIGRATION_PAUSE"))
}

/// The source-runner acknowledgement for a migration pause.
pub fn migration_pause_ack_path_at(
    base: &Path,
    name: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join("MIGRATION_PAUSE_ACK"))
}

/// The token-specific identity file for a replacement runner.
pub fn staged_runner_path_at(
    base: &Path,
    name: &str,
    token: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join(format!("MIGRATION_STAGED_{token}.json")))
}

/// The marker permitting a staged runner to begin intake.
pub fn staged_runner_activate_path_at(
    base: &Path,
    name: &str,
    token: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join(format!("MIGRATION_ACTIVATE_{token}")))
}

/// The acknowledgement proving a staged runner took over intake.
pub fn staged_runner_activated_path_at(
    base: &Path,
    name: &str,
    token: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    Ok(agent_dir_at(base, name)?.join(format!("MIGRATION_ACTIVATED_{token}")))
}

/// The central lifecycle event log (`STATE/events.jsonl`).
pub fn event_log_path(base: &Path) -> PathBuf {
    state_dir(base).join("events.jsonl")
}

/// The event log's lock file.
pub fn event_lock_path(base: &Path) -> PathBuf {
    state_dir(base).join(".events.lock")
}

/// Create the pending, inflight, processed, and failed request directories.
pub fn ensure_agent_dirs_at(base: &Path, name: &str) -> Result<(), ForeignOwnershipError> {
    for dir in [
        inbox_dir_at(base, name)?,
        inflight_dir_at(base, name)?,
        processed_dir_at(base, name)?,
        failed_dir_at(base, name)?,
    ] {
        std::fs::create_dir_all(&dir).map_err(|error| io_error("create", &dir, &error))?;
    }
    Ok(())
}

/// A bounded, single-line preview of the latest captured answer.
pub fn last_message_preview_at(
    base: &Path,
    name: &str,
    limit: usize,
) -> Result<String, ForeignOwnershipError> {
    let path = last_message_path_at(base, name)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(io_error("read", &path, &error)),
    };
    let text: Vec<String> = String::from_utf8_lossy(&bytes)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let text = text.join(" ");
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return Ok(text);
    }
    let take = limit.saturating_sub(3);
    Ok(format!("{}...", chars[..take].iter().collect::<String>()))
}

/// Append a lifecycle event for watchers, under the event log lock.
///
/// The central log lives outside per-agent dirs so archival cannot move a
/// just-written event away from its watcher.
pub fn write_event_at(
    base: &Path,
    event_type: &str,
    name: &str,
    seq: Option<i64>,
    rc: Option<&str>,
    preview: Option<&str>,
) -> Result<(), ForeignOwnershipError> {
    std::fs::create_dir_all(state_dir(base))
        .map_err(|error| io_error("create", &state_dir(base), &error))?;
    let mut event = serde_json::Map::new();
    event.insert(
        "id".into(),
        Value::String(format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos()),
            std::process::id()
        )),
    );
    event.insert("type".into(), Value::String(event_type.to_owned()));
    event.insert("name".into(), Value::String(name.to_owned()));
    event.insert("ts".into(), Value::String(now_iso()));
    if let Some(seq) = seq {
        event.insert("seq".into(), seq.into());
    }
    if let Some(rc) = rc {
        event.insert("rc".into(), Value::String(rc.to_owned()));
    }
    if let Some(preview) = preview {
        event.insert("preview".into(), Value::String(preview.to_owned()));
    }
    let lock_path = event_lock_path(base);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o644)
        .open(&lock_path)
        .map_err(|error| io_error("open event lock", &lock_path, &error))?;
    flock(&lock, true).map_err(|error| io_error("lock event log", &lock_path, &error))?;
    let log_path = event_log_path(base);
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut file| {
            file.write_all(
                format!(
                    "{}\n",
                    serde_json::to_string(&Value::Object(event)).unwrap_or_default()
                )
                .as_bytes(),
            )
        })
        .map_err(|error| io_error("append event", &log_path, &error));
    let _ = flock(&lock, false);
    result
}

fn flock(file: &std::fs::File, exclusive: bool) -> std::io::Result<()> {
    let operation = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_UN
    };
    loop {
        // SAFETY: flock takes the descriptor of the open lock file.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// One held per-worker lock file (harness and intake locks share the shape).
pub struct HarnessLock {
    _file: std::fs::File,
}

/// The turn-intake lock file (one consumer claims turns at a time).
pub fn intake_lock_at(base: &Path, name: &str) -> Result<HarnessLock, ForeignOwnershipError> {
    let path = agent_dir_at(base, name)?.join(".headless-turn.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| io_error("open intake lock", &path, &error))?;
    flock(&file, true).map_err(|error| io_error("lock intake", &path, &error))?;
    Ok(HarnessLock { _file: file })
}

/// The per-worker harness lock file (`BASE/.harness-<name>.lock`).
pub fn harness_lock_path(base: &Path, name: &str) -> Result<PathBuf, ForeignOwnershipError> {
    let name = super::registry::require_valid_name(name)?;
    Ok(base.join(format!(".harness-{name}.lock")))
}

/// Take the per-worker harness lock; dropping the guard releases it.
pub fn harness_lock_at(base: &Path, name: &str) -> Result<HarnessLock, ForeignOwnershipError> {
    std::fs::create_dir_all(base).map_err(|error| io_error("create", base, &error))?;
    let path = harness_lock_path(base, name)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| io_error("open harness lock", &path, &error))?;
    flock(&file, true).map_err(|error| io_error("lock harness", &path, &error))?;
    Ok(HarnessLock { _file: file })
}

/// The inbox filename for one sequence number.
pub fn msg_filename(seq: i64) -> String {
    format!("{seq:012}.json")
}

/// Allocate the next sequence number under the registry lock and drop the
/// message file atomically. The sequence is allocated (and the registry
/// saved) before the file is written, exactly as the established contract.
pub fn enqueue_message_at(
    base: &Path,
    name: &str,
    text: &str,
    model: Option<&str>,
) -> Result<i64, ForeignOwnershipError> {
    let seq = with_registry_lock_at(base, |agents| {
        if !agents.contains_key(name) {
            return Err(ForeignOwnershipError {
                code: "unknown_agent",
                message: format!(
                    "unknown agent '{}'; run agent_up first or check agent_status",
                    name
                ),
            });
        }
        if migration_pause_path_at(base, name).is_ok_and(|path| path.exists()) {
            return Err(ForeignOwnershipError {
                code: "agent_migrating",
                message: format!(
                    "agent '{name}' is migrating; wait for migration to finish before sending a turn"
                ),
            });
        }
        let record = agents.get_mut(name).expect("presence checked");
        let seq = record.next_seq;
        record.next_seq += 1;
        Ok(seq)
    })?;
    ensure_agent_dirs_at(base, name)?;
    let payload = Message {
        seq,
        text: text.to_owned(),
        model: model.map(str::to_owned),
        queued_at: now_iso(),
        effort: std::env::var("SUBAGENT_EFFORT").ok(),
    }
    .to_dict();
    let dest = inbox_dir_at(base, name)?.join(msg_filename(seq));
    write_durable_json(&dest, &payload)
        .map_err(|error| io_error("write message", &dest, &error))?;
    Ok(seq)
}

/// The first sequence-ordered pending message path, or `None` when idle.
pub fn next_pending_message_at(
    base: &Path,
    name: &str,
) -> Result<Option<PathBuf>, ForeignOwnershipError> {
    let box_dir = inbox_dir_at(base, name)?;
    let entries = match std::fs::read_dir(&box_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("list inbox", &box_dir, &error)),
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    Ok(files.into_iter().next())
}

/// Count message files still awaiting consumption.
pub fn pending_count_at(base: &Path, name: &str) -> Result<usize, ForeignOwnershipError> {
    let box_dir = inbox_dir_at(base, name)?;
    let entries = match std::fs::read_dir(&box_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_error("list inbox", &box_dir, &error)),
    };
    Ok(entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .count())
}

fn write_marker_json(path: &Path, value: &Value) -> Result<(), ForeignOwnershipError> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(
        &temporary,
        format!("{}\n", serde_json::to_string(value).unwrap_or_default()),
    )
    .map_err(|error| io_error("write marker", &temporary, &error))?;
    std::fs::rename(&temporary, path).map_err(|error| io_error("publish marker", path, &error))
}

/// Record the source identity whose intake must pause before migration.
pub fn write_migration_pause_at(
    base: &Path,
    name: &str,
    old: &ProcessIdentity,
) -> Result<(), ForeignOwnershipError> {
    let path = migration_pause_path_at(base, name)?;
    write_marker_json(
        &path,
        &serde_json::json!({
            "pid": old.pid,
            "started_at": old.started_at,
            "created_at": now_iso(),
        }),
    )
}

/// Record that this runner has stopped consuming pending messages.
pub fn acknowledge_migration_pause_at(
    base: &Path,
    name: &str,
    identity: &ProcessIdentity,
) -> Result<(), ForeignOwnershipError> {
    let path = migration_pause_ack_path_at(base, name)?;
    write_marker_json(
        &path,
        &serde_json::json!({
            "pid": identity.pid,
            "started_at": identity.started_at,
            "acknowledged_at": now_iso(),
        }),
    )
}

/// Check that the pause acknowledgement belongs to the expected source process.
pub fn pause_acknowledged_at(
    base: &Path,
    name: &str,
    old: &ProcessIdentity,
) -> Result<bool, ForeignOwnershipError> {
    let path = migration_pause_ack_path_at(base, name)?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(false);
    };
    let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(false);
    };
    Ok(raw.get("pid").and_then(Value::as_i64) == Some(old.pid)
        && raw.get("started_at").and_then(Value::as_str) == old.started_at.as_deref())
}

/// Publish the identity of a replacement runner awaiting activation.
pub fn write_staged_runner_at(
    base: &Path,
    name: &str,
    token: &str,
    identity: &ProcessIdentity,
) -> Result<(), ForeignOwnershipError> {
    let path = staged_runner_path_at(base, name, token)?;
    write_marker_json(
        &path,
        &serde_json::json!({
            "pid": identity.pid,
            "started_at": identity.started_at,
            "boot_id": identity.boot_id,
            "recorded_at": now_iso(),
        }),
    )
}

/// Read a replacement-runner identity, or `None` when no valid record exists.
pub fn read_staged_runner_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    token: &str,
) -> Result<Option<ProcessIdentity>, ForeignOwnershipError> {
    let path = staged_runner_path_at(base, name, token)?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Some(pid) = raw.get("pid").and_then(Value::as_i64) else {
        return Ok(None);
    };
    let started_at = raw
        .get("started_at")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if raw
        .get("started_at")
        .is_some_and(|value| !value.is_null() && started_at.is_none())
    {
        return Ok(None);
    }
    let boot_id = raw
        .get("boot_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if raw
        .get("boot_id")
        .is_some_and(|value| !value.is_null() && boot_id.is_none())
    {
        return Ok(None);
    }
    if let Some(boot) = &boot_id {
        let current = current_boot_id_at(proc_root);
        if !super::ownership::valid_boot_id(boot) || current.as_deref() != Ok(boot.as_str()) {
            return Ok(None);
        }
    }
    let current_start = pid_start_time_at(proc_root, pid)?;
    let Some(current_start) = current_start else {
        return Ok(None);
    };
    if started_at
        .as_deref()
        .is_some_and(|started| started != current_start)
    {
        return Ok(None);
    }
    Ok(Some(ProcessIdentity::new(pid, started_at, boot_id)))
}

/// Remove pause and staged-activation markers for a finished migration.
pub fn clear_migration_markers_at(
    base: &Path,
    name: &str,
    token: &str,
) -> Result<(), ForeignOwnershipError> {
    for path in [
        migration_pause_path_at(base, name)?,
        migration_pause_ack_path_at(base, name)?,
        staged_runner_path_at(base, name, token)?,
        staged_runner_activate_path_at(base, name, token)?,
        staged_runner_activated_path_at(base, name, token)?,
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("remove marker", &path, &error)),
        }
    }
    Ok(())
}

/// Record that the staged runner accepted its activation marker.
pub fn acknowledge_staged_runner_activation_at(
    base: &Path,
    name: &str,
    token: &str,
) -> Result<(), ForeignOwnershipError> {
    let path = staged_runner_activated_path_at(base, name, token)?;
    std::fs::write(&path, format!("{}\n", now_iso()))
        .map_err(|error| io_error("write activation ack", &path, &error))
}

/// Wait within the migration bound for replacement-runner acknowledgement.
pub fn wait_for_staged_runner_activation_at(
    base: &Path,
    name: &str,
    token: &str,
) -> Result<bool, ForeignOwnershipError> {
    let path = staged_runner_activated_path_at(base, name, token)?;
    let deadline = Instant::now() + migration_ready_timeout();
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(path.exists())
}

/// Linux start ticks, or `None` only for positive disappearance.
pub fn pid_start_time_at(
    proc_root: &Path,
    pid: i64,
) -> Result<Option<String>, ForeignOwnershipError> {
    Ok(read_stat_at(proc_root, pid)?.map(|stat| stat.starttime.to_string()))
}

/// Whether a process id currently exists, from procfs alone.
/// Unreadable evidence counts as present: it is not a death certificate.
pub fn pid_alive_at(proc_root: &Path, pid: Option<i64>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    match pid_start_time_at(proc_root, pid) {
        Ok(start) => start.is_some(),
        Err(_) => true,
    }
}

/// True only when the registry's runner pid still names the same process.
pub fn runner_identity_alive_at(
    proc_root: &Path,
    record: &AgentRecord,
) -> Result<bool, ForeignOwnershipError> {
    let Some(runner_pid) = record.runner_pid else {
        return Ok(false);
    };
    if record
        .runner_boot_id
        .as_deref()
        .is_some_and(|boot| current_boot_id_at(proc_root).map_or(true, |current| current != boot))
    {
        return Ok(false);
    }
    let Some(current) = pid_start_time_at(proc_root, runner_pid)? else {
        return Ok(false);
    };
    // Rows created before runner start ticks existed keep the old answer.
    Ok(record
        .runner_started_at
        .as_deref()
        .is_none_or(|started| started == current))
}

/// Wait within the migration deadline for the source to acknowledge paused intake.
pub fn wait_for_pause_ack_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    old: &ProcessIdentity,
) -> Result<bool, ForeignOwnershipError> {
    let deadline = Instant::now() + migration_ready_timeout();
    while Instant::now() < deadline {
        if !pid_alive_at(proc_root, Some(old.pid)) {
            return Ok(false);
        }
        if pause_acknowledged_at(base, name, old)? {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(pause_acknowledged_at(base, name, old)? && pid_alive_at(proc_root, Some(old.pid)))
}

/// Wait within the migration deadline for a live replacement-runner identity.
pub fn wait_for_staged_runner_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    token: &str,
) -> Result<Option<ProcessIdentity>, ForeignOwnershipError> {
    let deadline = Instant::now() + migration_ready_timeout();
    while Instant::now() < deadline {
        if let Some(identity) = read_staged_runner_at(base, proc_root, name, token)? {
            return Ok(Some(identity));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    read_staged_runner_at(base, proc_root, name, token)
}

fn runner_cmdline_proof<'a>(
    proc_root: &'a Path,
    name: Option<&'a str>,
) -> Option<impl Fn(i64, &ProcessStat) -> bool + 'a> {
    let name = name?;
    Some(move |pid: i64, _stat: &ProcessStat| {
        let Ok(raw) = std::fs::read(proc_root.join(pid.to_string()).join("cmdline")) else {
            return false;
        };
        let argv: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|item| !item.is_empty())
            .map(|item| String::from_utf8_lossy(item).into_owned())
            .collect();
        argv.len() >= 2
            && argv.last().map(String::as_str) == Some(name)
            && argv[..argv.len() - 1].iter().any(|item| {
                let base = Path::new(item)
                    .file_name()
                    .and_then(|part| part.to_str())
                    .unwrap_or(item);
                base == "agent_runner.py" || base == "herdr-subagents"
            })
    })
}

/// Stop one recorded runner through a pidfd: SIGTERM, then SIGKILL after grace.
///
/// Returns false when it was already gone. An identity that cannot be
/// pinned raises without a signal.
pub fn terminate_runner_identity_at(
    proc_root: &Path,
    identity: &ProcessIdentity,
    grace: Duration,
    name: Option<&str>,
) -> Result<bool, ForeignOwnershipError> {
    let proof = runner_cmdline_proof(proc_root, name);
    stop_process_at(
        proc_root,
        identity,
        grace,
        proof
            .as_ref()
            .map(|proof| proof as &dyn Fn(i64, &ProcessStat) -> bool),
        None,
        &format!("runner pid {}", identity.pid),
    )
}

/// Stop a worker's harness tree and then its runner, through exact handles.
pub fn terminate_runner_at(
    base: &Path,
    proc_root: &Path,
    record: &AgentRecord,
    grace: Duration,
) -> Result<bool, ForeignOwnershipError> {
    // Spawn and identity publication use this same lock, so even a
    // zero-grace stop observes the detached child before killing its runner.
    let _lock = harness_lock_at(base, &record.name)?;
    let identity = record.runner_identity();
    let runner_alive = identity.is_some() && runner_identity_alive_at(proc_root, record)?;
    let harness = read_active_harness_at(base, &record.name)?;
    // A pre-boot-identity runner's own watchdog owns its harness group:
    // stop that runner first and only then apply the record policy.
    let defer = harness.as_ref().is_some_and(|harness| {
        harness.legacy() && runner_alive && owned_by_record(record, harness)
    });
    let harness_stopped = if defer {
        false
    } else {
        terminate_active_harness_at(base, proc_root, record)?
    };
    let Some(identity) = identity.filter(|_| runner_alive) else {
        return Ok(harness_stopped);
    };
    let stopped = terminate_runner_identity_at(proc_root, &identity, grace, Some(&record.name))?;
    if defer {
        let path = super::registry::active_harness_path_at(base, &record.name)?;
        let deadline = Instant::now() + super::ownership::default_kill_wait();
        while path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        terminate_active_harness_at(base, proc_root, record)?;
    }
    Ok(stopped || harness_stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::registry::{read_registry_at, with_registry_lock_at};

    fn fake_base(test: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agentctl-b3-7-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn seed_record(base: &Path, name: &str) {
        with_registry_lock_at(base, |agents| {
            agents.insert(
                name.to_string(),
                AgentRecord {
                    name: name.to_string(),
                    status: "idle".to_string(),
                    next_seq: 5,
                    created_at: "2026-11-07T00:00:00Z".to_string(),
                    ..AgentRecord::default()
                },
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn enqueue_allocates_and_publishes_in_order() {
        let base = fake_base("enqueue");
        seed_record(&base, "worker-1");
        std::env::remove_var("SUBAGENT_EFFORT");
        let seq = enqueue_message_at(&base, "worker-1", "hello", Some("m1")).unwrap();
        assert_eq!(seq, 5);
        let next = next_pending_message_at(&base, "worker-1").unwrap().unwrap();
        assert!(next.ends_with("000000000005.json"), "{}", next.display());
        let msg = Message::from_path(&next).unwrap();
        assert_eq!((msg.seq, msg.text.as_str()), (5, "hello"));
        assert_eq!(pending_count_at(&base, "worker-1").unwrap(), 1);
        assert_eq!(read_registry_at(&base).unwrap()["worker-1"].next_seq, 6);
        let err = enqueue_message_at(&base, "ghost", "x", None).unwrap_err();
        assert_eq!(err.code, "unknown_agent");
        // A pause marker refuses before a sequence is burned.
        std::fs::write(migration_pause_path_at(&base, "worker-1").unwrap(), "{}\n").unwrap();
        let err = enqueue_message_at(&base, "worker-1", "x", None).unwrap_err();
        assert_eq!(err.code, "agent_migrating");
        assert_eq!(read_registry_at(&base).unwrap()["worker-1"].next_seq, 6);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn events_and_migration_markers_round_trip() {
        let base = fake_base("markers");
        ensure_agent_dirs_at(&base, "worker-1").unwrap();
        write_event_at(
            &base,
            "turn_done",
            "worker-1",
            Some(5),
            Some("0"),
            Some("done"),
        )
        .unwrap();
        let log = std::fs::read_to_string(event_log_path(&base)).unwrap();
        let event: Value = serde_json::from_str(log.trim()).unwrap();
        assert_eq!(event["type"], "turn_done");
        assert_eq!(event["seq"], 5);

        let own = std::process::id() as i64;
        let identity = ProcessIdentity::new(
            own,
            pid_start_time_at(Path::new("/proc"), own).unwrap(),
            Some(current_boot_id_at(Path::new("/proc")).unwrap()),
        );
        write_migration_pause_at(&base, "worker-1", &identity).unwrap();
        acknowledge_migration_pause_at(&base, "worker-1", &identity).unwrap();
        assert!(pause_acknowledged_at(&base, "worker-1", &identity).unwrap());
        write_staged_runner_at(&base, "worker-1", "tok1", &identity).unwrap();
        assert_eq!(
            read_staged_runner_at(&base, Path::new("/proc"), "worker-1", "tok1")
                .unwrap()
                .as_ref()
                .map(|item| item.pid),
            Some(own)
        );
        acknowledge_staged_runner_activation_at(&base, "worker-1", "tok1").unwrap();
        assert!(wait_for_staged_runner_activation_at(&base, "worker-1", "tok1").unwrap());
        clear_migration_markers_at(&base, "worker-1", "tok1").unwrap();
        assert!(!migration_pause_path_at(&base, "worker-1").unwrap().exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn runner_identity_liveness_and_termination() {
        use std::os::unix::process::ExitStatusExt as _;
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("spawn runner stand-in");
        let pid = child.id() as i64;
        let started = pid_start_time_at(Path::new("/proc"), pid).unwrap().unwrap();
        let mut record = AgentRecord {
            runner_pid: Some(pid),
            runner_started_at: Some(started.clone()),
            runner_boot_id: Some(current_boot_id_at(Path::new("/proc")).unwrap()),
            ..AgentRecord::default()
        };
        assert!(runner_identity_alive_at(Path::new("/proc"), &record).unwrap());
        record.runner_started_at = Some(format!("{}9", started));
        assert!(!runner_identity_alive_at(Path::new("/proc"), &record).unwrap());
        record.runner_started_at = Some(started);
        let identity = record.runner_identity().unwrap();
        assert!(terminate_runner_identity_at(
            Path::new("/proc"),
            &identity,
            Duration::from_secs(2),
            None
        )
        .unwrap());
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGTERM));
    }
}
