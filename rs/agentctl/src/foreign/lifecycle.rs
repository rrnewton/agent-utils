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

/// Whether the recorded tmux window exists (missing binary means no window).
fn tmux_target_exists(target: &str) -> bool {
    let Some((session, name)) = target.split_once(':') else {
        return false;
    };
    if session.is_empty() || name.is_empty() {
        return false;
    }
    let output = std::process::Command::new("tmux")
        .args([
            "list-windows",
            "-t",
            &format!("={session}"),
            "-F",
            "#{window_name}",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == name),
        _ => false,
    }
}

/// Whether the recorded presentation exists (headless backends only here).
pub fn window_exists(record: &AgentRecord) -> bool {
    if record.mode == "tui" {
        return false;
    }
    if record.backend == "tmux" {
        return tmux_target_exists(&record.tmux_target);
    }
    if record.tmux_target.is_empty() {
        return false;
    }
    // Herdr presentation probing arrives with its presentation slice;
    // a recorded target with no probe is not positive evidence.
    false
}

/// True if the agent was created so recently its runner may not have recorded
/// its pid yet — do not reap it as dead in that window.
pub fn within_startup_grace(record: &AgentRecord) -> bool {
    if record.runner_pid.is_some() {
        return false;
    }
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(&record.created_at) else {
        return false;
    };
    let age = chrono::Local::now().signed_duration_since(created.with_timezone(&chrono::Local));
    age.num_seconds() < 20
}

/// Require a registered agent whose runner still names the same process.
pub fn require_live_agent_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
) -> Result<AgentRecord, ForeignOwnershipError> {
    let name = super::registry::require_valid_name(name)?;
    let registry = super::registry::read_registry_at(base)?;
    let Some(record) = registry.get(name).cloned() else {
        return Err(ForeignOwnershipError {
            code: "unknown_agent",
            message: format!("unknown agent '{name}'; run subagent_status to list live agents"),
        });
    };
    if record.mode == "tui" {
        return Err(ForeignOwnershipError {
            code: "tui_not_yet_in_rust",
            message: format!(
                "agent '{name}' runs in TUI mode, which this edition does not operate yet"
            ),
        });
    }
    if runner_identity_alive_at(proc_root, &record)? {
        return Ok(record);
    }
    if record.runner_pid.is_none() && within_startup_grace(&record) && window_exists(&record) {
        return Ok(record);
    }
    if record.runner_pid.is_none() {
        return Err(ForeignOwnershipError {
            code: "dead_runner",
            message: format!("agent '{name}' has no runner pid after startup grace"),
        });
    }
    Err(ForeignOwnershipError {
        code: "dead_runner",
        message: format!(
            "agent '{name}' runner pid {} is not alive or was reused",
            record.runner_pid.unwrap_or_default()
        ),
    })
}

/// Queued turn identity and transcript location returned by a send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendResult {
    /// Agent name.
    pub name: String,
    /// Allocated sequence number.
    pub seq: i64,
    /// Per-turn model override.
    pub model: Option<String>,
    /// Transcript location (display string).
    pub transcript: String,
    /// Worker mode.
    pub mode: String,
    /// Herdr pane owning an interactive TUI.
    pub presentation_pane: Option<String>,
    /// Earlier sequences quarantined during TUI delivery.
    pub quarantined: Vec<i64>,
}

/// Queue a prompt for a live headless worker (its next turn).
pub fn send_message_to_agent_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    text: &str,
    model: Option<&str>,
) -> Result<SendResult, ForeignOwnershipError> {
    if text.is_empty() {
        return Err(ForeignOwnershipError {
            code: "empty_message",
            message: "message must not be empty".to_string(),
        });
    }
    let record = require_live_agent_at(base, proc_root, name)?;
    let seq = enqueue_message_at(base, name, text, model)?;
    Ok(SendResult {
        name: record.name.clone(),
        seq,
        model: model.map(str::to_owned),
        transcript: transcript_path_at(base, name)?.display().to_string(),
        mode: record.mode.clone(),
        presentation_pane: record.presentation_pane.clone(),
        quarantined: Vec::new(),
    })
}

/// Result of clearing a live agent's harness conversation context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetResult {
    /// Agent name.
    pub name: String,
    /// Session id before the reset.
    pub previous_session_id: Option<String>,
    /// Transcript location (display string).
    pub transcript: String,
}

fn new_uuid4() -> String {
    let mut bytes = [0_u8; 16];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .is_ok();
    if !filled {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = ((ns >> ((index % 8) * 8)) & 0xff) as u8;
        }
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Clear one idle headless agent's harness session without disturbing its warm slot.
pub fn reset_agent_context_at(
    base: &Path,
    name: &str,
) -> Result<ResetResult, ForeignOwnershipError> {
    let name = super::registry::require_valid_name(name)?.to_string();
    let transcript_file = transcript_path_at(base, &name)?;
    let previous = with_registry_lock_at(base, |agents| {
        let Some(record) = agents.get_mut(&name) else {
            return Err(ForeignOwnershipError {
                code: "unknown_agent",
                message: format!("unknown agent '{name}'"),
            });
        };
        if record.mode == "tui" {
            return Err(ForeignOwnershipError {
                code: "tui_reset_unsupported",
                message: "interactive TUI context is owned by its visible Codex process; use /new or /resume in that tab".to_string(),
            });
        }
        if record.status == "busy" || pending_count_at(base, &name)? > 0 {
            return Err(ForeignOwnershipError {
                code: "agent_busy",
                message: format!(
                    "agent '{name}' has an active or queued turn; reset it at an idle task boundary"
                ),
            });
        }
        let previous = record.session_id.clone();
        record.session_id = if record.harness == "muse" {
            Some(new_uuid4())
        } else {
            None
        };
        if let Some(parent) = transcript_file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| io_error("create transcript dir", parent, &error))?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&transcript_file)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"=== CONTEXT RESET ===\n"))
            .map_err(|error| io_error("append reset marker", &transcript_file, &error))?;
        Ok(previous)
    })?;
    Ok(ResetResult {
        name,
        previous_session_id: previous,
        transcript: transcript_file.display().to_string(),
    })
}

/// Worker output with its requested read mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResult {
    /// Agent name.
    pub name: String,
    /// Requested read mode.
    pub mode: String,
    /// Output text.
    pub text: String,
}

/// Read durable headless turn output, rejecting unsupported boundaries.
/// TUI scrollback arrives with its presentation slice and refuses by name.
pub fn read_agent_output_at(
    base: &Path,
    name: &str,
    mode: &str,
    since_turn: Option<i64>,
    tail: Option<i64>,
) -> Result<ReadResult, ForeignOwnershipError> {
    let bad = |message: &str| ForeignOwnershipError {
        code: "bad_read_mode",
        message: message.to_string(),
    };
    if !matches!(mode, "all" | "last" | "since_turn" | "tail") {
        return Err(bad("mode must be one of: all, last, since_turn, tail"));
    }
    if mode == "since_turn" && since_turn.is_none() {
        return Err(bad("since_turn mode requires since_turn"));
    }
    if mode == "tail" && tail.is_none() {
        return Err(bad("tail mode requires tail"));
    }
    let name = super::registry::require_valid_name(name)?;
    let registry = super::registry::read_registry_at(base)?;
    if let Some(record) = registry.get(name) {
        if record.mode == "tui" {
            if record.presentation_pane.is_none() {
                return Err(ForeignOwnershipError {
                    code: "tui_pane_missing",
                    message: format!("TUI agent '{name}' has no recorded Herdr pane"),
                });
            }
            if since_turn.is_some() {
                return Err(ForeignOwnershipError {
                    code: "tui_since_turn_unsupported",
                    message: format!(
                        "TUI agent '{name}' has no durable turn-boundary capture; since_turn cannot be honored for an interactive TUI backend. Use mode=\"tail\" (optionally with an explicit `tail` line count) instead."
                    ),
                });
            }
            if mode == "last" {
                return Err(ForeignOwnershipError {
                    code: "tui_last_unsupported",
                    message: format!(
                        "TUI agent '{name}' cannot identify \"the last answer\" distinctly from scrollback; mode=\"last\" is not supported for an interactive TUI backend. Use mode=\"tail\" (optionally with an explicit `tail` line count) instead."
                    ),
                });
            }
            return Err(ForeignOwnershipError {
                code: "tui_read_not_ported",
                message: format!(
                    "reading TUI agent '{name}' scrollback is not yet in this edition"
                ),
            });
        }
    }
    let transcript_file = transcript_path_at(base, name)?;
    if !registry.contains_key(name) && !transcript_file.exists() {
        return Err(ForeignOwnershipError {
            code: "unknown_agent",
            message: format!("unknown agent '{name}' and no archived transcript"),
        });
    }
    let no_transcript = |code: &'static str| ForeignOwnershipError {
        code,
        message: format!("no transcript yet for '{name}'"),
    };
    let text = match mode {
        "last" => {
            let path = last_message_path_at(base, name)?;
            if !path.exists() {
                return Err(ForeignOwnershipError {
                    code: "no_completed_turn",
                    message: format!("no completed turn yet for '{name}' (no last-message.txt)"),
                });
            }
            std::fs::read_to_string(&path).map_err(|e| io_error("read last message", &path, &e))?
        }
        "since_turn" => {
            let since = since_turn.unwrap_or_default();
            if !transcript_file.exists() {
                return Err(no_transcript("no_transcript"));
            }
            let content = std::fs::read_to_string(&transcript_file)
                .map_err(|e| io_error("read transcript", &transcript_file, &e))?;
            let marker = format!("===TURN {since} START");
            let lines: Vec<&str> = content.lines().collect();
            match lines.iter().position(|line| line.starts_with(&marker)) {
                Some(index) => format!("{}\n", lines[index..].join("\n")),
                None => {
                    return Err(ForeignOwnershipError {
                        code: "turn_not_found",
                        message: format!("turn {since} not found in '{name}' transcript"),
                    })
                }
            }
        }
        "tail" => {
            let tail = tail.unwrap_or_default();
            if tail < 0 {
                return Err(bad("tail must be non-negative"));
            }
            if !transcript_file.exists() {
                return Err(no_transcript("no_transcript"));
            }
            let content = std::fs::read_to_string(&transcript_file)
                .map_err(|e| io_error("read transcript", &transcript_file, &e))?;
            let lines: Vec<&str> = content.lines().collect();
            let selected: &[&str] = if tail == 0 {
                &[]
            } else {
                &lines[lines.len().saturating_sub(tail as usize)..]
            };
            format!("{}\n", selected.join("\n"))
        }
        _ => {
            if !transcript_file.exists() {
                return Err(no_transcript("no_transcript"));
            }
            std::fs::read_to_string(&transcript_file)
                .map_err(|e| io_error("read transcript", &transcript_file, &e))?
        }
    };
    Ok(ReadResult {
        name: name.to_string(),
        mode: mode.to_string(),
        text,
    })
}

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN").unwrap_or_else(|_| "herdr".to_string())
}

fn herdr_api_timeout() -> Duration {
    std::env::var("SUBAGENTS_HERDR_TIMEOUT")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .and_then(|seconds| Duration::try_from_secs_f64(seconds.max(0.0)).ok())
        .unwrap_or(Duration::from_secs(2))
}

fn run_bounded(argv: &[String], timeout: Duration) -> Option<std::process::Output> {
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// One bounded Herdr request, returning its `result` object (or wait-event envelope).
pub fn herdr_call(
    args: &[&str],
    timeout: Option<Duration>,
) -> Result<Value, ForeignOwnershipError> {
    let timeout = timeout.unwrap_or_else(herdr_api_timeout);
    let mut argv = vec![herdr_bin()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    let joined = args.join(" ");
    let output = run_bounded(&argv, timeout).ok_or_else(|| ForeignOwnershipError {
        code: "herdr_unavailable",
        message: format!(
            "Herdr probe {joined} failed within {}s",
            timeout.as_secs_f64()
        ),
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("exit {}", output.status.code().unwrap_or_default())
        };
        return Err(ForeignOwnershipError {
            code: "herdr_api_failed",
            message: format!("herdr {joined} failed: {detail}"),
        });
    }
    let raw: Value = serde_json::from_str(&stdout).map_err(|_| ForeignOwnershipError {
        code: "herdr_api_invalid",
        message: format!(
            "herdr {joined} returned invalid API JSON: {}",
            stdout.trim()
        ),
    })?;
    let Some(obj) = raw.as_object() else {
        return Err(ForeignOwnershipError {
            code: "herdr_api_invalid",
            message: format!(
                "herdr {joined} returned invalid API JSON: {}",
                stdout.trim()
            ),
        });
    };
    if let Some(payload) = obj.get("result") {
        if payload.is_object() {
            return Ok(payload.clone());
        }
        return Err(ForeignOwnershipError {
            code: "herdr_api_invalid",
            message: format!("herdr {joined} returned no object"),
        });
    }
    if obj.get("event").is_some_and(Value::is_string)
        && obj.get("data").is_some_and(Value::is_object)
    {
        return Ok(raw);
    }
    Err(ForeignOwnershipError {
        code: "herdr_api_invalid",
        message: format!(
            "herdr {joined} returned invalid API JSON: {}",
            stdout.trim()
        ),
    })
}

fn herdr_not_found(error: &ForeignOwnershipError) -> bool {
    let message = error.message.to_lowercase();
    message.contains("not_found") || message.contains("not found")
}

/// Bounded health/state snapshot for one interactive Herdr-hosted TUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuiProbe {
    /// Whether the pane has foreground processes.
    pub pane_alive: bool,
    /// Whether the Codex process is alive in the pane.
    pub codex_alive: bool,
    /// Herdr agent status string.
    pub status: String,
    /// Codex pid when found.
    pub pid: Option<i64>,
    /// Probe error, when the probe itself failed.
    pub error: Option<String>,
}

/// Inspect a TUI pane without allowing status/gc to block indefinitely.
pub fn herdr_tui_probe_at(proc_root: &Path, pane_id: &str) -> TuiProbe {
    let timeout = Some(herdr_api_timeout().min(Duration::from_secs(1)));
    let outcome = (|| -> Result<TuiProbe, ForeignOwnershipError> {
        let info = herdr_call(&["pane", "process-info", "--pane", pane_id], timeout)?;
        let process_info = info.get("process_info").cloned().unwrap_or(Value::Null);
        let Some(process_info) = process_info.as_object() else {
            return Err(ForeignOwnershipError {
                code: "herdr_api_invalid",
                message: "Herdr did not return pane process information".to_string(),
            });
        };
        let processes: Vec<&serde_json::Map<String, Value>> = process_info
            .get("foreground_processes")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_object).collect())
            .filter(|items: &Vec<_>| {
                process_info
                    .get("foreground_processes")
                    .and_then(Value::as_array)
                    .is_some_and(|all| all.len() == items.len())
            })
            .unwrap_or_default();
        let pane_alive = !processes.is_empty();
        let codex_name = Path::new(&super::runner::codex_bin())
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "codex".to_string());
        let codex_process = processes.iter().find(|process| {
            process.get("name").and_then(Value::as_str) == Some(codex_name.as_str())
                || process
                    .get("argv")
                    .and_then(Value::as_array)
                    .and_then(|argv| argv.first())
                    .and_then(Value::as_str)
                    == Some(super::runner::codex_bin().as_str())
        });
        let pid = codex_process
            .and_then(|process| process.get("pid"))
            .and_then(Value::as_i64);
        let agent = herdr_call(&["agent", "get", pane_id], timeout)?;
        let status = agent
            .get("agent")
            .and_then(|agent| agent.get("agent_status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        Ok(TuiProbe {
            pane_alive,
            codex_alive: pid.is_some_and(|pid| pid_alive_at(proc_root, Some(pid))),
            status,
            pid,
            error: None,
        })
    })();
    outcome.unwrap_or_else(|error| TuiProbe {
        pane_alive: false,
        codex_alive: false,
        status: "probe_error".to_string(),
        pid: None,
        error: Some(error.message),
    })
}

/// Move the agent's state dir under `state/_archive/` (transcripts kept).
pub fn archive_state_at(base: &Path, name: &str) -> Result<Option<PathBuf>, ForeignOwnershipError> {
    let src = agent_dir_at(base, name)?;
    if !src.exists() {
        return Ok(None);
    }
    let archive = state_dir(base).join("_archive");
    std::fs::create_dir_all(&archive).map_err(|e| io_error("create archive", &archive, &e))?;
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S").to_string();
    let mut dest = archive.join(format!("{name}-{stamp}"));
    let mut n = 1;
    while dest.exists() {
        dest = archive.join(format!("{name}-{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(&src, &dest).map_err(|e| io_error("archive state", &src, &e))?;
    Ok(Some(dest))
}

fn recorded_identity_at(path: &Path) -> Result<ProcessIdentity, ForeignOwnershipError> {
    let unknown = |message: String| ForeignOwnershipError {
        code: "runner_identity_unknown",
        message,
    };
    let bytes = std::fs::read(path).map_err(|e| {
        unknown(format!(
            "{} is unreadable or malformed: {e}",
            path.display()
        ))
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| {
        unknown(format!(
            "{} is unreadable or malformed: {e}",
            path.display()
        ))
    })?;
    let Some(obj) = value.as_object() else {
        return Err(unknown(format!("{} is not an object", path.display())));
    };
    let pid = obj.get("pid").and_then(Value::as_i64);
    if !matches!(pid, Some(pid) if (2..=(1_i64 << 31) - 1).contains(&pid)) {
        return Err(unknown(format!("{} has no valid pid", path.display())));
    }
    let started_at = obj.get("started_at");
    if started_at.is_some_and(|value| !value.is_null() && !value.is_string()) {
        return Err(unknown(format!(
            "{} has an invalid started_at",
            path.display()
        )));
    }
    let boot_id = obj.get("boot_id");
    if boot_id.is_some_and(|value| {
        !value.is_null() && !value.as_str().is_some_and(super::ownership::valid_boot_id)
    }) {
        return Err(unknown(format!(
            "{} has an invalid boot_id",
            path.display()
        )));
    }
    Ok(ProcessIdentity::new(
        pid.unwrap_or_default(),
        started_at.and_then(Value::as_str).map(str::to_owned),
        boot_id.and_then(Value::as_str).map(str::to_owned),
    ))
}

/// Whether every process recorded for one worker is provably gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    /// Every recorded process has exited or its pid was reused.
    Dead,
    /// At least one recorded process may still run.
    Alive,
    /// Evidence cannot prove death.
    Inconclusive,
}

/// Whether every process recorded for one worker is provably gone.
///
/// Records are checked in order: the registry row's runner (bound to its
/// boot by the sidecar), a different pid in runner.pid, the recorded
/// harness leader and its tree, and staged replacement runners.
pub fn recorded_liveness_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    starting_grace: bool,
) -> (Liveness, String) {
    let inconclusive = |detail: String| (Liveness::Inconclusive, detail);
    let registry_path = super::registry::registry_path(base);
    let state = state_dir(base).join(name);
    let rows: Value = if registry_path.exists() {
        match std::fs::read(&registry_path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
        {
            Ok(rows) => rows,
            Err(error) => {
                return inconclusive(format!("the runtime records are malformed: {error}"));
            }
        }
    } else {
        Value::Array(Vec::new())
    };
    let Some(rows) = rows.as_array() else {
        return inconclusive(format!(
            "{} is not a JSON array of objects",
            registry_path.display()
        ));
    };
    if !rows.iter().all(Value::is_object) {
        return inconclusive(format!(
            "{} is not a JSON array of objects",
            registry_path.display()
        ));
    }
    let matching: Vec<&serde_json::Map<String, Value>> = rows
        .iter()
        .filter_map(Value::as_object)
        .filter(|row| row.get("name").and_then(Value::as_str) == Some(name))
        .collect();
    if matching.len() > 1 {
        return inconclusive(format!(
            "{} has more than one row for '{name}'",
            registry_path.display()
        ));
    }
    let row = matching.first().copied();
    let mut processes: Vec<(&str, ProcessIdentity)> = Vec::new();
    let mut row_pid: Option<i64> = None;
    if let Some(row) = row {
        match row.get("mode").and_then(Value::as_str) {
            Some("tui") => {
                return inconclusive(
                    "the row is an interactive TUI, whose liveness this proof does not cover"
                        .to_string(),
                );
            }
            None if super::registry::presentation_identity_path_at(base, name)
                .is_ok_and(|path| path.exists()) =>
            {
                return inconclusive(
                    "the row has no mode and a presentation sidecar marks it as a possible TUI"
                        .to_string(),
                );
            }
            Some(mode) if mode != "headless" => {
                return inconclusive(format!("the row has an unsupported mode '{mode}'"));
            }
            _ => {}
        }
        let raw_started = row.get("runner_started_at");
        if raw_started.is_some_and(|value| !value.is_null() && !value.is_string()) {
            return inconclusive("the row has an invalid runner_started_at".to_string());
        }
        if let Some(raw_pid) = row.get("runner_pid").filter(|value| !value.is_null()) {
            let Some(pid) = raw_pid
                .as_i64()
                .filter(|pid| (1..=(1_i64 << 31) - 1).contains(pid))
            else {
                return inconclusive("the row has an invalid runner_pid".to_string());
            };
            row_pid = Some(pid);
            // Join the boot sidecar directly (never the registry reader:
            // gc calls this under the registry lock, and flock is per
            // open-file-description, so re-entering it would self-deadlock).
            let boot_id = super::registry::runner_identity_path_at(base, name)
                .ok()
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .filter(|sidecar| {
                    sidecar.get("schema").and_then(Value::as_str)
                        == Some(super::ownership::RUNNER_SCHEMA)
                        && sidecar.get("name").and_then(Value::as_str) == Some(name)
                        && sidecar.get("runner_pid").and_then(Value::as_i64) == Some(pid)
                })
                .and_then(|sidecar| {
                    sidecar
                        .get("runner_boot_id")
                        .and_then(Value::as_str)
                        .filter(|boot| super::ownership::valid_boot_id(boot))
                        .map(str::to_owned)
                });
            processes.push((
                "its runner",
                ProcessIdentity::new(
                    pid,
                    raw_started.and_then(Value::as_str).map(str::to_owned),
                    boot_id,
                ),
            ));
        }
    }
    if let Ok(pid_path) = runner_pid_path_at(base, name) {
        if pid_path.exists() {
            let text = std::fs::read_to_string(&pid_path)
                .unwrap_or_default()
                .trim()
                .to_string();
            let parsed = text
                .parse::<i64>()
                .ok()
                .filter(|pid| (1..=(1_i64 << 31) - 1).contains(pid));
            let Some(pid) = parsed.filter(|_| text.bytes().all(|b| b.is_ascii_digit())) else {
                return inconclusive(format!("{} is malformed", pid_path.display()));
            };
            if Some(pid) != row_pid {
                processes.push((
                    "the runner named in runner.pid",
                    ProcessIdentity::new(pid, None, None),
                ));
            }
        }
    }
    let mut harness = None;
    let harness_path = state.join("active-harness.json");
    if harness_path.exists() {
        match recorded_identity_at(&harness_path) {
            Ok(identity) => processes.push(("its harness", identity)),
            Err(error) => return inconclusive(error.message),
        }
        match read_active_harness_at(base, name) {
            Ok(record) => harness = record,
            Err(error) => return inconclusive(error.message),
        }
    }
    if state.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&state) {
            let mut staged: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|file| file.to_str())
                        .is_some_and(|file| {
                            file.starts_with("MIGRATION_STAGED_") && file.ends_with(".json")
                        })
                })
                .collect();
            staged.sort();
            for path in staged {
                match recorded_identity_at(&path) {
                    Ok(identity) => processes.push(("a staged replacement runner", identity)),
                    Err(error) => return inconclusive(error.message),
                }
            }
        }
    }
    let mut gone: Vec<String> = Vec::new();
    for (label, identity) in &processes {
        match super::ownership::recorded_process_alive_at(proc_root, label, identity) {
            Ok(Some(alive)) => return (Liveness::Alive, alive),
            Ok(None) => {}
            Err(error) => return inconclusive(error.message),
        }
        gone.push(format!("{label} (pid {})", identity.pid));
        if *label == "its harness" {
            if let Some(harness) = &harness {
                match super::ownership::tree_alive_at(proc_root, harness) {
                    Ok(Some(member)) => return (Liveness::Alive, member),
                    Ok(None) => {}
                    Err(error) => return inconclusive(error.message),
                }
            }
        }
    }
    if starting_grace {
        if let Some(row) = row {
            if row_pid.is_none() {
                let created = row
                    .get("created_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Ok(instant) = chrono::DateTime::parse_from_rfc3339(created) else {
                    return inconclusive(format!(
                        "the row records no runner process and its created_at '{created}' cannot be read"
                    ));
                };
                let age =
                    chrono::Utc::now().signed_duration_since(instant.with_timezone(&chrono::Utc));
                if age.num_seconds() < 20 {
                    return inconclusive(
                        "the row records no runner process yet; its runner may still be starting"
                            .to_string(),
                    );
                }
            }
        }
    }
    let detail = if gone.is_empty() {
        "no recorded process".to_string()
    } else {
        format!(
            "every recorded process has exited or its PID now names a newer process: {}",
            gone.join(", ")
        )
    };
    (Liveness::Dead, detail)
}

fn stop_recorded_staged_runners_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
) -> Result<(), ForeignOwnershipError> {
    let state = state_dir(base).join(name);
    if !state.is_dir() {
        return Ok(());
    }
    let mut staged: Vec<PathBuf> = std::fs::read_dir(&state)
        .map_err(|e| io_error("list staged runners", &state, &e))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| {
                    file.starts_with("MIGRATION_STAGED_") && file.ends_with(".json")
                })
        })
        .collect();
    staged.sort();
    for path in staged {
        let identity = recorded_identity_at(&path)?;
        if super::ownership::recorded_process_alive_at(
            proc_root,
            "a staged replacement runner",
            &identity,
        )?
        .is_some()
        {
            terminate_runner_identity_at(proc_root, &identity, Duration::from_secs(2), Some(name))?;
        }
    }
    Ok(())
}

/// The exact tmux target for a recorded `session:name` window.
pub fn exact_tmux_target(target: &str) -> Result<String, ForeignOwnershipError> {
    let Some((session, name)) = target.split_once(':') else {
        return Err(ForeignOwnershipError {
            code: "invalid_tmux_target",
            message: format!("invalid recorded tmux window target '{target}'"),
        });
    };
    if session.is_empty() || name.is_empty() {
        return Err(ForeignOwnershipError {
            code: "invalid_tmux_target",
            message: format!("invalid recorded tmux window target '{target}'"),
        });
    }
    Ok(format!("={session}:={name}"))
}

/// The tmux session subagents live in (`$SUBAGENTS_TMUX_SESSION`).
pub fn tmux_session() -> String {
    std::env::var("SUBAGENTS_TMUX_SESSION").unwrap_or_else(|_| "subagents".to_string())
}

/// Whether a named tmux window exists in the subagents session.
pub fn tmux_window_exists(name: &str) -> bool {
    tmux_target_exists(&format!("{}:{name}", tmux_session()))
}

/// Close the presentation belonging to this record without closing its workspace.
pub fn kill_window(record: &AgentRecord) {
    if record.backend == "tmux" {
        if let Ok(target) = exact_tmux_target(&record.tmux_target) {
            let _ = std::process::Command::new("tmux")
                .args(["kill-window", "-t", &target])
                .output();
        }
    } else if let Some(tab) = record
        .tmux_target
        .split(':')
        .nth(1)
        .filter(|tab| !tab.is_empty())
    {
        // Herdr: close just the agent's tab.
        let _ = herdr_call(&["tab", "close", &record.tmux_target], None)
            .or_else(|_| herdr_call(&["tab", "close", tab], None));
    }
}

fn herdr_workspace_from_tab(tab_id: &str) -> Option<String> {
    let (workspace_id, separator, _tab) = match tab_id.split_once(':') {
        Some((a, b)) => (a, ":", b),
        None => return None,
    };
    let _ = separator;
    if workspace_id.is_empty() {
        None
    } else {
        Some(workspace_id.to_string())
    }
}

/// False only for a confirmed missing workspace; unknown preserves state.
fn herdr_workspace_exists(workspace_id: &str) -> Option<bool> {
    match herdr_call(
        &["workspace", "get", workspace_id],
        Some(herdr_api_timeout().min(Duration::from_secs(1))),
    ) {
        Ok(_) => Some(true),
        Err(error) if herdr_not_found(&error) => Some(false),
        Err(_) => None,
    }
}

fn write_workspace_loss_snapshot_at(
    base: &Path,
    record: &AgentRecord,
    workspace_id: &str,
) -> Result<PathBuf, ForeignOwnershipError> {
    ensure_agent_dirs_at(base, &record.name)?;
    let path = agent_dir_at(base, &record.name)?.join("WORKSPACE_LOST.json");
    let payload = serde_json::json!({
        "detected_at": now_iso(),
        "workspace_id": workspace_id,
        "agent": record.to_dict(),
        "recovery": "The Herdr workspace disappeared. Read transcript.log and this saved registry row before starting a replacement agent. A TUI's visible-only scrollback may be unavailable after a hard workspace death.",
    });
    write_durable_json(&path, &payload)
        .map_err(|e| io_error("write workspace loss snapshot", &path, &e))?;
    Ok(path)
}

fn reap_workspace_loss_at(
    base: &Path,
    proc_root: &Path,
    record: &AgentRecord,
    workspace_id: &str,
) -> Result<String, ForeignOwnershipError> {
    let snapshot = write_workspace_loss_snapshot_at(base, record, workspace_id)?;
    if record.mode == "headless" {
        terminate_active_harness_at(base, proc_root, record)?;
    }
    let dest = archive_state_at(base, &record.name)?;
    let saved_snapshot = match &dest {
        Some(dest) => dest.join("WORKSPACE_LOST.json"),
        None => snapshot,
    };
    let tail = dest
        .as_ref()
        .map(|dest| format!("; state archived to {}", dest.display()))
        .unwrap_or_default();
    write_event_at(
        base,
        "herdr_workspace_lost",
        &record.name,
        None,
        None,
        Some(&last_message_preview_at(base, &record.name, 240)?),
    )?;
    Ok(format!(
        "reaped {} (Herdr workspace {workspace_id} is gone; hard workspace death may have interrupted a turn; recovery snapshot {}{tail})",
        record.name,
        saved_snapshot.display()
    ))
}

/// Reap registry entries whose runner process is truly gone.
///
/// Returns human-readable notes (one per reaped agent) so callers can print
/// a visible record — silent reaping would hide lost work.
pub fn gc_at(base: &Path, proc_root: &Path) -> Result<Vec<String>, ForeignOwnershipError> {
    let mut notes: Vec<String> = Vec::new();
    let mut workspace_state: std::collections::HashMap<String, Option<bool>> = Default::default();
    with_registry_lock_at(base, |agents| {
        for name in agents.keys().cloned().collect::<Vec<_>>() {
            let rec = agents[&name].clone();
            if within_startup_grace(&rec) {
                continue;
            }
            if rec.backend == "herdr" {
                if let Some(workspace_id) = herdr_workspace_from_tab(&rec.tmux_target) {
                    let exists = *workspace_state
                        .entry(workspace_id.clone())
                        .or_insert_with(|| herdr_workspace_exists(&workspace_id));
                    match exists {
                        None => {
                            notes.push(format!(
                                "{name} Herdr workspace {workspace_id} probe failed; preserving state instead of assuming a workspace death"
                            ));
                            continue;
                        }
                        Some(false) => {
                            let process_alive = rec.mode == "headless"
                                && runner_identity_alive_at(proc_root, &rec)?;
                            if process_alive {
                                notes.push(format!(
                                    "{name} degraded: Herdr workspace {workspace_id} is gone but runner pid {} remains alive; recover its presentation before sending work",
                                    rec.runner_pid.unwrap_or_default()
                                ));
                                write_event_at(
                                    base,
                                    "herdr_workspace_lost",
                                    &name,
                                    None,
                                    None,
                                    Some(&last_message_preview_at(base, &name, 240)?),
                                )?;
                                continue;
                            }
                            match reap_workspace_loss_at(base, proc_root, &rec, &workspace_id) {
                                Ok(note) => notes.push(note),
                                Err(error) => {
                                    notes.push(format!(
                                        "{name} degraded: Herdr workspace {workspace_id} is gone but its harness could not be cleaned ({}); preserving state",
                                        error.message
                                    ));
                                    continue;
                                }
                            }
                            agents.remove(&name);
                            continue;
                        }
                        Some(true) => {}
                    }
                }
            }
            if rec.mode == "tui" {
                let Some(pane) = rec.presentation_pane.clone() else {
                    notes.push(format!(
                        "{name} degraded: TUI registry row has no pane id; preserving state"
                    ));
                    continue;
                };
                let probe = herdr_tui_probe_at(proc_root, &pane);
                if let Some(error) = &probe.error {
                    notes.push(format!(
                        "{name} TUI probe failed; preserving state: {error}"
                    ));
                    continue;
                }
                if probe.pane_alive {
                    if !probe.codex_alive {
                        notes.push(format!(
                            "{name} degraded: Herdr TUI pane {pane} is alive but Codex is not its foreground process; preserving it for inspection"
                        ));
                    }
                    continue;
                }
                if herdr_tab_exists(&rec.tmux_target) {
                    notes.push(format!(
                        "{name} degraded: Herdr tab {} remains but its TUI pane is gone; preserving state for inspection",
                        rec.tmux_target
                    ));
                    continue;
                }
                let dest = archive_state_at(base, &name)?;
                agents.remove(&name);
                let tail = dest
                    .as_ref()
                    .map(|dest| format!("; state archived to {}", dest.display()))
                    .unwrap_or_default();
                notes.push(format!("reaped {name} (Herdr TUI pane is gone){tail}"));
                continue;
            }
            let win = window_exists(&rec);
            let runner_alive = runner_identity_alive_at(proc_root, &rec)?;
            if runner_alive {
                if !win {
                    notes.push(format!(
                        "{name} degraded: runner pid {} is alive but {} presentation {} is missing; recreate presentation",
                        rec.runner_pid.unwrap_or_default(),
                        rec.backend,
                        if rec.tmux_target.is_empty() { &rec.name } else { &rec.tmux_target }
                    ));
                }
                continue;
            }
            let reason = match rec.runner_pid {
                None => "runner pid missing after startup grace".to_string(),
                Some(pid) => format!("runner pid {pid} not alive or reused"),
            };
            if let Err(error) = terminate_active_harness_at(base, proc_root, &rec) {
                notes.push(format!(
                    "{name} degraded: {reason}, but its harness could not be cleaned ({}); preserving state",
                    error.message
                ));
                continue;
            }
            let (verdict, detail) = recorded_liveness_at(base, proc_root, &name, true);
            if verdict != Liveness::Dead {
                let verdict_name = match verdict {
                    Liveness::Alive => "alive",
                    Liveness::Inconclusive => "inconclusive",
                    Liveness::Dead => "dead",
                };
                notes.push(format!(
                    "{name} degraded: {reason}, but a recorded process is {verdict_name} ({detail}); preserving state"
                ));
                continue;
            }
            if win {
                kill_window(&rec);
            }
            let dest = archive_state_at(base, &name)?;
            agents.remove(&name);
            let tail = dest
                .as_ref()
                .map(|dest| format!("; state archived to {}", dest.display()))
                .unwrap_or_default();
            notes.push(format!("reaped {name} ({reason}){tail}"));
        }
        Ok(())
    })?;
    Ok(notes)
}

fn herdr_tab_exists(tab_id: &str) -> bool {
    match herdr_call(&["tab", "get", tab_id], None) {
        Ok(_) => true,
        Err(error) if herdr_not_found(&error) => false,
        Err(error) => !herdr_not_found(&error),
    }
}

/// A worker snapshot combining registry identity, process health, and queue counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStatus {
    /// Agent name.
    pub name: String,
    /// Harness identifier.
    pub harness: String,
    /// Presentation backend.
    pub backend: String,
    /// Tmux target or Herdr tab id.
    pub tmux_target: String,
    /// Working directory.
    pub cwd: String,
    /// Model override.
    pub model: Option<String>,
    /// Harness session id.
    pub session_id: Option<String>,
    /// Display status.
    pub status: String,
    /// Runner pid.
    pub runner_pid: Option<i64>,
    /// Runner kernel start ticks.
    pub runner_started_at: Option<String>,
    /// Next message sequence number.
    pub next_seq: i64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last turn timestamp.
    pub last_turn_at: Option<String>,
    /// Whether the presentation window/tab is alive.
    pub window_alive: bool,
    /// Whether the runner process is alive.
    pub runner_alive: bool,
    /// Whether a live runner has lost its presentation.
    pub presentation_degraded: bool,
    /// Pending message count.
    pub pending: usize,
    /// Transcript path (display string).
    pub transcript: String,
    /// Bounded preview of the latest answer.
    pub last_message_preview: String,
    /// Worker mode.
    pub mode: String,
    /// Herdr pane owning an interactive TUI.
    pub presentation_pane: Option<String>,
    /// Whether automation is paused for this worker.
    pub automation_paused: bool,
}

/// Worker status snapshots plus garbage-collection notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusResult {
    /// Snapshots by sorted agent name.
    pub agents: Vec<AgentStatus>,
    /// Garbage-collection notes from the pre-pass.
    pub gc_notes: Vec<String>,
}

fn agent_status_at(
    base: &Path,
    proc_root: &Path,
    record: &AgentRecord,
) -> Result<AgentStatus, ForeignOwnershipError> {
    if record.mode == "tui" {
        let probe = match &record.presentation_pane {
            None => TuiProbe {
                pane_alive: false,
                codex_alive: false,
                status: "probe_error".to_string(),
                pid: None,
                error: Some("registry row has no TUI pane id".to_string()),
            },
            Some(pane) => herdr_tui_probe_at(proc_root, pane),
        };
        let status = if let Some(error) = &probe.error {
            format!("probe_error: {error}")
        } else {
            match probe.status.as_str() {
                "working" => "busy".to_string(),
                "idle" | "done" => "idle".to_string(),
                "blocked" => "blocked".to_string(),
                _ => "unknown".to_string(),
            }
        };
        return Ok(AgentStatus {
            name: record.name.clone(),
            harness: record.harness.clone(),
            backend: record.backend.clone(),
            tmux_target: record.tmux_target.clone(),
            cwd: record.cwd.clone(),
            model: record.model.clone(),
            session_id: record.session_id.clone(),
            status,
            runner_pid: probe.pid,
            runner_started_at: match probe.pid {
                Some(pid) => pid_start_time_at(proc_root, pid).unwrap_or(None),
                None => None,
            },
            next_seq: record.next_seq,
            created_at: record.created_at.clone(),
            last_turn_at: record.last_turn_at.clone(),
            window_alive: probe.pane_alive,
            runner_alive: probe.codex_alive,
            presentation_degraded: probe.pane_alive && !probe.codex_alive,
            pending: pending_count_at(base, &record.name)?,
            transcript: format!(
                "herdr pane {}",
                record.presentation_pane.as_deref().unwrap_or("<missing>")
            ),
            last_message_preview: String::new(),
            mode: record.mode.clone(),
            presentation_pane: record.presentation_pane.clone(),
            automation_paused: agent_dir_at(base, &record.name)
                .is_ok_and(|dir| dir.join("automation-paused").exists()),
        });
    }
    let runner_alive = runner_identity_alive_at(proc_root, record)?;
    let win = window_exists(record);
    Ok(AgentStatus {
        name: record.name.clone(),
        harness: record.harness.clone(),
        backend: record.backend.clone(),
        tmux_target: record.tmux_target.clone(),
        cwd: record.cwd.clone(),
        model: record.model.clone(),
        session_id: record.session_id.clone(),
        status: record.status.clone(),
        runner_pid: record.runner_pid,
        runner_started_at: record.runner_started_at.clone(),
        next_seq: record.next_seq,
        created_at: record.created_at.clone(),
        last_turn_at: record.last_turn_at.clone(),
        window_alive: win,
        runner_alive,
        presentation_degraded: runner_alive && !win,
        pending: pending_count_at(base, &record.name)?,
        transcript: transcript_path_at(base, &record.name)?
            .display()
            .to_string(),
        last_message_preview: last_message_preview_at(base, &record.name, 240)?,
        mode: record.mode.clone(),
        presentation_pane: record.presentation_pane.clone(),
        automation_paused: agent_dir_at(base, &record.name)
            .is_ok_and(|dir| dir.join("automation-paused").exists()),
    })
}

/// One or all worker statuses, optionally collecting confirmed dead workers first.
pub fn status_snapshot_at(
    base: &Path,
    proc_root: &Path,
    name: Option<&str>,
    run_gc: bool,
) -> Result<StatusResult, ForeignOwnershipError> {
    let notes = if run_gc {
        gc_at(base, proc_root)?
    } else {
        Vec::new()
    };
    let registry = super::registry::read_registry_at(base)?;
    let records: Vec<AgentRecord> = match name {
        Some(name) => {
            let name = super::registry::require_valid_name(name)?;
            let Some(record) = registry.get(name) else {
                return Err(ForeignOwnershipError {
                    code: "unknown_agent",
                    message: format!("unknown agent '{name}'"),
                });
            };
            vec![record.clone()]
        }
        None => registry.values().cloned().collect(),
    };
    let mut agents = Vec::new();
    for record in &records {
        agents.push(agent_status_at(base, proc_root, record)?);
    }
    Ok(StatusResult {
        agents,
        gc_notes: notes,
    })
}

/// Worker retirement outcome, including presentation cleanup and archive location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownResult {
    /// Agent name.
    pub name: String,
    /// Whether the presentation window/tab was killed.
    pub killed_window: bool,
    /// Archive destination (display string), when state was archived.
    pub archived_to: Option<String>,
    /// Kept state path when not archiving.
    pub state_path: Option<String>,
    /// Whether a registry row existed.
    pub was_registered: bool,
    /// Whether retirement was forced without backend probing.
    pub forced: bool,
    /// Presentation that force mode did not verify.
    pub unverified_presentation: Option<String>,
}

/// Remove a headless worker's tmux guard window, when it keeps one.
fn remove_legacy_tmux_guard(record: &AgentRecord) {
    if record.backend != "herdr" {
        return;
    }
    // The guard is a plain tmux window named after the worker in the
    // subagents session; closing it cannot touch the Herdr presentation.
    if tmux_window_exists(&record.name) {
        let target = format!("{}:{}", tmux_session(), record.name);
        if let Ok(exact) = exact_tmux_target(&target) {
            let _ = std::process::Command::new("tmux")
                .args(["kill-window", "-t", &exact])
                .output();
        }
    }
}

/// Retire an agent: stop its runner, close its presentation, archive state.
///
/// Force mode remains narrow: it can stop a local headless runner, but
/// never contacts or claims to close the presentation backend.
pub fn bring_down_agent_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    grace: Duration,
    archive: bool,
    force: bool,
) -> Result<DownResult, ForeignOwnershipError> {
    let valid_name = super::registry::require_valid_name(name)?.to_string();
    let record = super::registry::read_registry_at(base)?
        .get(&valid_name)
        .cloned();
    let was_registered = record.is_some();
    if let Some(record) = &record {
        if record.mode == "headless" {
            let stop = stop_path_at(base, &valid_name)?;
            std::fs::write(&stop, format!("{}\n", now_iso()))
                .map_err(|error| io_error("write stop marker", &stop, &error))?;
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                let current = super::registry::read_registry_at(base)?
                    .get(&valid_name)
                    .cloned();
                match current {
                    None => break,
                    Some(current) if !runner_identity_alive_at(proc_root, &current)? => break,
                    _ => std::thread::sleep(Duration::from_millis(200)),
                }
            }
            if let Some(current) = super::registry::read_registry_at(base)?
                .get(&valid_name)
                .cloned()
            {
                terminate_runner_at(base, proc_root, &current, grace)?;
            }
            stop_recorded_staged_runners_at(base, proc_root, &valid_name)?;
            let (verdict, detail) = recorded_liveness_at(base, proc_root, &valid_name, false);
            if verdict == Liveness::Alive || (verdict == Liveness::Inconclusive && !force) {
                let verdict_name = match verdict {
                    Liveness::Alive => "alive",
                    Liveness::Inconclusive => "inconclusive",
                    Liveness::Dead => "dead",
                };
                return Err(ForeignOwnershipError {
                    code: "worker_processes_remain",
                    message: format!(
                        "refusing to retire '{valid_name}': a recorded process is {verdict_name} ({detail}); its state and registry row are unchanged"
                    ),
                });
            }
        }
    }
    let mut unverified_presentation: Option<String> = None;
    let mut killed_window = false;
    if force {
        if let Some(record) = &record {
            let presentation = if record.mode == "tui" {
                record
                    .presentation_pane
                    .clone()
                    .unwrap_or_else(|| record.name.clone())
            } else {
                record.tmux_target.clone()
            };
            unverified_presentation =
                Some(format!("{} presentation {presentation}", record.backend));
        }
    } else if let Some(record) = &record {
        killed_window = if record.mode == "tui" {
            herdr_tab_exists(&record.tmux_target)
        } else {
            window_exists(record)
        };
        if killed_window {
            if record.mode == "tui" {
                // Closing the Herdr tab closes the presentation.
                kill_window(record);
            } else {
                kill_window(record);
            }
        }
    }
    if let Some(record) = &record {
        if record.backend == "herdr"
            && (record.mode == "headless"
                || agent_dir_at(base, &record.name)
                    .is_ok_and(|dir| dir.join("legacy-tmux-guard").exists()))
        {
            remove_legacy_tmux_guard(record);
        }
    }
    let preview = last_message_preview_at(base, &valid_name, 240)?;
    let (mut archived_to, mut state_path) = (None::<String>, None::<String>);
    if archive {
        archived_to = archive_state_at(base, &valid_name)?.map(|path| path.display().to_string());
    } else {
        let dir = agent_dir_at(base, &valid_name)?;
        if dir.exists() {
            state_path = Some(dir.display().to_string());
        }
    }
    with_registry_lock_at(base, |agents| {
        agents.remove(&valid_name);
        Ok(())
    })?;
    if was_registered || killed_window || archived_to.is_some() || state_path.is_some() {
        write_event_at(
            base,
            if force {
                "agent_down_forced"
            } else {
                "agent_down"
            },
            &valid_name,
            None,
            None,
            Some(&preview),
        )?;
    }
    Ok(DownResult {
        name: valid_name,
        killed_window,
        archived_to,
        state_path,
        was_registered,
        forced: force,
        unverified_presentation,
    })
}

/// Queued and quarantined message sequence numbers for one worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuiInboxResult {
    /// Agent name.
    pub name: String,
    /// Pending sequence numbers in order.
    pub pending: Vec<i64>,
    /// Quarantined sequence numbers in order.
    pub quarantined: Vec<i64>,
    /// Failed-messages directory (display string).
    pub failed_dir: String,
}

fn message_seqs(dir: &Path) -> Result<Vec<i64>, ForeignOwnershipError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("list messages", dir, &error)),
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| Message::from_path(path).map(|message| message.seq))
        .collect()
}

/// Inspect queued and quarantined message sequence numbers without delivery.
pub fn tui_inbox_snapshot_at(
    base: &Path,
    name: &str,
) -> Result<TuiInboxResult, ForeignOwnershipError> {
    let name = super::registry::require_valid_name(name)?;
    Ok(TuiInboxResult {
        name: name.to_string(),
        pending: message_seqs(&inbox_dir_at(base, name)?)?,
        quarantined: message_seqs(&failed_dir_at(base, name)?)?,
        failed_dir: failed_dir_at(base, name)?.display().to_string(),
    })
}

/// Whether tmux is installed and can report its version.
pub fn tmux_available() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Probe the selected presentation backend for startup availability.
pub fn backend_available(backend: &str) -> bool {
    if backend == "tmux" {
        tmux_available()
    } else {
        super::registry::herdr_available()
    }
}

fn configured_codex_bypass_permissions() -> Result<bool, ForeignOwnershipError> {
    match std::env::var("SUBAGENTS_CODEX_BYPASS_PERMISSIONS").as_deref() {
        Err(_) | Ok("0") => Ok(false),
        Ok("1") => Ok(true),
        Ok(_) => Err(ForeignOwnershipError {
            code: "invalid_permission_policy",
            message: "SUBAGENTS_CODEX_BYPASS_PERMISSIONS must be exactly 0 (native permissions) or 1 (bypass approvals and sandbox)".to_string(),
        }),
    }
}

fn require_antigravity_preflight(cwd: &Path) -> Result<(), ForeignOwnershipError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let settings_path = home.join(".gemini/antigravity-cli/settings.json");
    let instruction = format!(
        "Fix: edit {} so toolPermission is exactly \"always-proceed\" and trustedWorkspaces contains \"{}\". Do not use --dangerously-skip-permissions for agy subagent turns.",
        settings_path.display(),
        cwd.display()
    );
    let fail = |message: String| ForeignOwnershipError {
        code: "agy_preflight_failed",
        message,
    };
    if !settings_path.is_file() {
        return Err(fail(format!(
            "Antigravity settings file not found at {}. {instruction}",
            settings_path.display()
        )));
    }
    let raw: Value = std::fs::read(&settings_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| {
            fail(format!(
                "Antigravity settings file is not valid JSON. {instruction}"
            ))
        })?;
    let Some(raw) = raw.as_object() else {
        return Err(fail(format!(
            "Antigravity settings file must contain a JSON object. {instruction}"
        )));
    };
    if raw.get("toolPermission").and_then(Value::as_str) != Some("always-proceed") {
        return Err(fail(format!(
            "Antigravity toolPermission is not \"always-proceed\". {instruction}"
        )));
    }
    let workspaces: Vec<String> = match raw.get("trustedWorkspaces").and_then(Value::as_array) {
        Some(items) if items.iter().all(Value::is_string) => items
            .iter()
            .map(|item| {
                let item = item.as_str().unwrap_or_default();
                let expanded = if let Some(rest) = item.strip_prefix("~/") {
                    home.join(rest)
                } else {
                    PathBuf::from(item)
                };
                expanded
                    .canonicalize()
                    .unwrap_or(expanded)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect(),
        _ => {
            return Err(fail(format!(
                "Antigravity trustedWorkspaces must be a list of strings. {instruction}"
            )));
        }
    };
    if !workspaces.iter().any(|item| item == &cwd.to_string_lossy()) {
        return Err(fail(format!(
            "Antigravity cwd is not trusted. {instruction}"
        )));
    }
    Ok(())
}

/// Shell-quote one value the way POSIX shlex does.
pub fn shlex_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"@%_+=:,./-".contains(&byte))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn child_runner_command(base: &Path, name: &str) -> String {
    let mut environment: Vec<(String, String)> = vec![
        (
            "HERDR_SUBAGENTS_HOME".to_string(),
            base.display().to_string(),
        ),
        (
            "HERDR_SUBAGENTS_PROJECT_DEFAULTS".to_string(),
            super::registry::project_defaults_path(base)
                .display()
                .to_string(),
        ),
        (
            "SUBAGENTS_CODEX_BYPASS_PERMISSIONS".to_string(),
            if configured_codex_bypass_permissions().unwrap_or(false) {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
    ];
    if let Ok(policy) = std::env::var("HERDR_SUBAGENTS_POLICY") {
        if !policy.is_empty() {
            let resolved = PathBuf::from(&policy);
            let resolved = resolved.canonicalize().unwrap_or(resolved);
            environment.push((
                "HERDR_SUBAGENTS_POLICY".to_string(),
                resolved.display().to_string(),
            ));
        }
    }
    for key in [
        "CODEX_BIN",
        "AGY_BIN",
        "MUSE_BIN",
        "HERDR_BIN",
        "CODEX_HOME",
        "SUBAGENT_EFFORT",
        "SUBAGENTS_TMUX_SESSION",
        "SUBAGENTS_HERDR_WORKSPACE",
        "SUBAGENTS_TURN_TIMEOUT",
    ] {
        if let Ok(value) = std::env::var(key) {
            environment.push((key.to_string(), value));
        }
    }
    let prefix = environment
        .iter()
        .map(|(key, value)| format!("{key}={}", shlex_quote(value)))
        .collect::<Vec<_>>()
        .join(" ");
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "herdr-subagents".to_string());
    format!(
        "env {prefix} {} __runner {}",
        shlex_quote(&executable),
        shlex_quote(name)
    )
}

/// Shell command used by every backend to host a runner visibly.
pub fn runner_wrapper_at(
    base: &Path,
    name: &str,
    stage_token: Option<&str>,
) -> Result<String, ForeignOwnershipError> {
    let mut inner = child_runner_command(base, name);
    if let Some(token) = stage_token {
        inner = format!("SUBAGENTS_MIGRATION_STAGE={} {inner}", shlex_quote(token));
    }
    let stderr = shlex_quote(&runner_stderr_path_at(base, name)?.display().to_string());
    Ok(format!(
        "{inner} 2>>{stderr}; ec=$?; echo '[subagents] runner exited rc='$ec >>{stderr}; echo; \
         echo '[subagents] runner for {name} exited rc='$ec' - presentation kept for \
         inspection; run agent_down.py {name} to reap'; exec sleep infinity"
    ))
}

fn tmux_run(args: &[&str], check: bool) -> Result<std::process::Output, ForeignOwnershipError> {
    let output = std::process::Command::new("tmux").args(args).output();
    match output {
        Ok(output) => {
            if check && !output.status.success() {
                return Err(ForeignOwnershipError {
                    code: "tmux_failed",
                    message: format!(
                        "tmux {} failed: {}",
                        args.join(" "),
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                });
            }
            Ok(output)
        }
        Err(_error) if !check => Ok(std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: Vec::new(),
            stderr: b"tmux: not found".to_vec(),
        }),
        Err(error) => Err(ForeignOwnershipError {
            code: "tmux_failed",
            message: format!("cannot run tmux {}: {error}", args.join(" ")),
        }),
    }
}

fn launch_tmux_window(
    name: &str,
    cwd: &str,
    wrapper_cmd: &str,
    session: Option<&str>,
) -> Result<String, ForeignOwnershipError> {
    let session = session.map(str::to_owned).unwrap_or_else(tmux_session);
    let exists = tmux_run(&["has-session", "-t", &format!("={session}")], false)
        .is_ok_and(|output| output.status.success());
    if !exists {
        tmux_run(
            &[
                "new-session",
                "-d",
                "-s",
                &session,
                "-n",
                name,
                "-c",
                cwd,
                "bash",
                "-lc",
                wrapper_cmd,
            ],
            true,
        )?;
    } else {
        tmux_run(
            &[
                "new-window",
                "-t",
                &format!("={session}:"),
                "-n",
                name,
                "-c",
                cwd,
                "bash",
                "-lc",
                wrapper_cmd,
            ],
            true,
        )?;
    }
    Ok(format!("{session}:{name}"))
}

fn herdr_action(args: &[&str]) -> Result<(), ForeignOwnershipError> {
    let argv: Vec<String> = std::iter::once(herdr_bin())
        .chain(args.iter().map(|arg| arg.to_string()))
        .collect();
    match run_bounded(&argv, herdr_api_timeout()) {
        Some(output) if output.status.success() => Ok(()),
        Some(output) => Err(ForeignOwnershipError {
            code: "herdr_api_failed",
            message: format!(
                "herdr {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }),
        None => Err(ForeignOwnershipError {
            code: "herdr_unavailable",
            message: format!("Herdr probe {} failed", args.join(" ")),
        }),
    }
}

fn herdr_orphan_window_exists(base: &Path, name: &str) -> Result<bool, ForeignOwnershipError> {
    let Some(workspace_id) = backend_config_value(base, "herdr_workspace_id")
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    match herdr_call(&["tab", "list", "--workspace", &workspace_id], None) {
        Ok(listed) => {
            let tabs = listed
                .get("tabs")
                .and_then(Value::as_array)
                .ok_or_else(|| ForeignOwnershipError {
                    code: "herdr_api_invalid",
                    message: "herdr response lacks a tabs list".to_string(),
                })?;
            Ok(tabs
                .iter()
                .any(|tab| tab.get("label").and_then(Value::as_str) == Some(name)))
        }
        Err(error) if herdr_not_found(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn herdr_workspace_label() -> String {
    std::env::var("SUBAGENTS_HERDR_WORKSPACE").unwrap_or_else(|_| "subagents".to_string())
}

fn backend_config_value(base: &Path, key: &str) -> Option<Value> {
    let bytes = std::fs::read(super::registry::backend_config_path(base)).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get(key)
        .cloned()
}

fn remember_herdr_workspace(base: &Path, workspace_id: &str) {
    let path = super::registry::backend_config_path(base);
    let mut config = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    config.insert(
        "herdr_workspace_id".into(),
        Value::String(workspace_id.to_string()),
    );
    let _ = std::fs::create_dir_all(base);
    let _ = write_durable_json(&path, &Value::Object(config));
}

fn herdr_workspace_id(base: &Path, cwd: &str) -> Result<String, ForeignOwnershipError> {
    let label = herdr_workspace_label();
    if let Some(configured) = backend_config_value(base, "herdr_workspace_id")
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.is_empty())
    {
        match herdr_call(&["workspace", "get", &configured], None) {
            Ok(found)
                if found
                    .get("workspace")
                    .and_then(|w| w.get("label"))
                    .and_then(Value::as_str)
                    == Some(label.as_str()) =>
            {
                return Ok(configured);
            }
            Ok(_) => {}
            Err(error) if !herdr_not_found(&error) => return Err(error),
            Err(_) => {}
        }
    }
    let listed = herdr_call(&["workspace", "list"], None)?;
    let workspaces = listed
        .get("workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| ForeignOwnershipError {
            code: "herdr_api_invalid",
            message: "herdr response lacks a workspaces list".to_string(),
        })?;
    for workspace in workspaces {
        if workspace.get("label").and_then(Value::as_str) == Some(label.as_str()) {
            if let Some(id) = workspace.get("workspace_id").and_then(Value::as_str) {
                remember_herdr_workspace(base, id);
                return Ok(id.to_string());
            }
        }
    }
    let created = herdr_call(
        &[
            "workspace",
            "create",
            "--cwd",
            cwd,
            "--label",
            &label,
            "--no-focus",
        ],
        None,
    )?;
    let id = created
        .get("workspace")
        .and_then(|w| w.get("workspace_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| ForeignOwnershipError {
            code: "herdr_api_invalid",
            message: "Herdr did not return its new workspace id".to_string(),
        })?
        .to_string();
    remember_herdr_workspace(base, &id);
    Ok(id)
}

fn launch_herdr_window(
    base: &Path,
    name: &str,
    cwd: &str,
    wrapper_cmd: &str,
) -> Result<String, ForeignOwnershipError> {
    let workspace_id = herdr_workspace_id(base, cwd)?;
    let created = herdr_call(
        &[
            "tab",
            "create",
            "--workspace",
            &workspace_id,
            "--cwd",
            cwd,
            "--label",
            "subagent",
            "--no-focus",
        ],
        None,
    )?;
    let tab = created.get("tab");
    let root = created.get("root_pane");
    let (Some(tab), Some(root)) = (
        tab.and_then(Value::as_object),
        root.and_then(Value::as_object),
    ) else {
        return Err(ForeignOwnershipError {
            code: "herdr_api_invalid",
            message: "Herdr did not return a tab and root pane".to_string(),
        });
    };
    let (Some(tab_id), Some(root_pane_id)) = (
        tab.get("tab_id").and_then(Value::as_str),
        root.get("pane_id").and_then(Value::as_str),
    ) else {
        return Err(ForeignOwnershipError {
            code: "herdr_api_invalid",
            message: "Herdr did not return a tab and root pane".to_string(),
        });
    };
    let launched = herdr_action(&[
        "pane",
        "run",
        root_pane_id,
        &format!("/bin/bash -lc {}", shlex_quote(wrapper_cmd)),
    ])
    .and_then(|()| herdr_call(&["tab", "rename", tab_id, name], None).map(|_| ()));
    if let Err(error) = launched {
        if herdr_call(&["tab", "get", tab_id], None).is_ok() {
            if let Err(cleanup) = herdr_call(&["tab", "close", tab_id], None) {
                return Err(ForeignOwnershipError {
                    code: "herdr_cleanup_failed",
                    message: format!(
                        "failed to launch Herdr tab {tab_id} ({}); cleanup also failed: {}",
                        error.message, cleanup.message
                    ),
                });
            }
        }
        return Err(error);
    }
    Ok(tab_id.to_string())
}

/// New worker status, the optional first-turn sequence, and cleanup notes.
#[derive(Clone, Debug, PartialEq)]
pub struct UpResult {
    /// The new worker's status snapshot.
    pub agent: AgentStatus,
    /// First queued turn sequence, when a brief was given.
    pub queued_turn: Option<i64>,
    /// Garbage-collection notes from the pre-launch pass.
    pub gc_notes: Vec<String>,
}

/// Validate policy and launch a named headless worker, optionally enqueueing its first task.
#[allow(clippy::too_many_arguments)]
pub fn bring_up_agent_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    cwd: &str,
    brief: Option<&str>,
    model: Option<&str>,
    harness: &str,
    backend: Option<&str>,
    mode: Option<&str>,
    harness_args: &[String],
) -> Result<UpResult, ForeignOwnershipError> {
    let harness = super::registry::require_supported_harness(harness)?;
    if harness_args
        .iter()
        .any(|item| item.is_empty() || item.contains('\0'))
    {
        return Err(ForeignOwnershipError {
            code: "invalid_harness_arguments",
            message: "harness arguments must be nonempty NUL-free strings".to_string(),
        });
    }
    if harness != "muse" && !harness_args.is_empty() {
        return Err(ForeignOwnershipError {
            code: "unsupported_harness_arguments",
            message: "headless harness arguments currently apply only to Muse".to_string(),
        });
    }
    // Consumer policy modules remain a dynamic-language hook this edition
    // refuses rather than silently skipping a launch check.
    if std::env::var("HERDR_SUBAGENTS_POLICY").is_ok_and(|value| !value.is_empty()) {
        return Err(ForeignOwnershipError {
            code: "policy_invalid",
            message: "cannot load subagent policy: dynamic policy modules are not supported by this edition".to_string(),
        });
    }
    let bypass_permissions = configured_codex_bypass_permissions()?;
    let backend = super::registry::selected_backend_at(base, backend)?;
    let (mode, warning) = super::registry::selected_mode_at(base, harness, mode, Some(&backend))?;
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    if mode == "tui" && backend != "herdr" {
        return Err(ForeignOwnershipError {
            code: "tui_requires_herdr",
            message: "--mode tui requires the herdr backend".to_string(),
        });
    }
    if mode == "tui" {
        return Err(ForeignOwnershipError {
            code: "tui_not_yet_in_rust",
            message: "bringing up a TUI worker is not yet in this edition".to_string(),
        });
    }
    if !backend_available(&backend) {
        return Err(ForeignOwnershipError {
            code: if backend == "tmux" {
                "tmux_missing"
            } else {
                "herdr_missing"
            },
            message: format!(
                "{backend} is selected but is not available; check its binary and running server"
            ),
        });
    }
    let valid_name = super::registry::require_valid_name(name)?.to_string();
    let root = {
        let path = PathBuf::from(cwd);
        let expanded = if let Some(rest) = cwd.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(rest))
                .unwrap_or(path)
        } else {
            path
        };
        expanded.canonicalize().unwrap_or(expanded)
    };
    if !root.is_dir() {
        return Err(ForeignOwnershipError {
            code: "bad_cwd",
            message: format!("cwd is not a directory: {}", root.display()),
        });
    }
    if harness == "agy" {
        require_antigravity_preflight(&root)?;
    }
    let notes = gc_at(base, proc_root)?;
    with_registry_lock_at(base, |agents| {
        if agents.contains_key(&valid_name) {
            return Err(ForeignOwnershipError {
                code: "agent_exists",
                message: format!("agent '{valid_name}' already active; subagent_down it first"),
            });
        }
        let orphan = if backend == "tmux" {
            tmux_window_exists(&valid_name)
        } else {
            herdr_orphan_window_exists(base, &valid_name)?
        };
        if orphan {
            return Err(ForeignOwnershipError {
                code: "orphan_window",
                message: format!(
                    "{backend} presentation for '{valid_name}' already exists with no registry row"
                ),
            });
        }
        agents.insert(
            valid_name.clone(),
            AgentRecord {
                name: valid_name.clone(),
                harness: harness.to_string(),
                backend: backend.clone(),
                tmux_target: format!("{}:{}", tmux_session(), valid_name),
                cwd: root.to_string_lossy().into_owned(),
                model: model.map(str::to_owned),
                session_id: if harness == "muse" {
                    Some(new_uuid4())
                } else {
                    None
                },
                status: "starting".to_string(),
                runner_pid: None,
                runner_started_at: None,
                next_seq: 0,
                created_at: now_iso(),
                last_turn_at: None,
                mode: mode.clone(),
                codex_bypass_permissions: bypass_permissions,
                harness_args: harness_args.to_vec(),
                ..AgentRecord::default()
            },
        );
        Ok(())
    })?;
    ensure_agent_dirs_at(base, &valid_name)?;
    let launched = (|| -> Result<String, ForeignOwnershipError> {
        let wrapper = runner_wrapper_at(base, &valid_name, None)?;
        if backend == "tmux" {
            launch_tmux_window(&valid_name, &root.to_string_lossy(), &wrapper, None)
        } else {
            launch_herdr_window(base, &valid_name, &root.to_string_lossy(), &wrapper)
        }
    })();
    let target = match launched {
        Ok(target) => target,
        Err(error) => {
            let _ = with_registry_lock_at(base, |agents| {
                agents.remove(&valid_name);
                Ok(())
            });
            return Err(ForeignOwnershipError {
                code: if backend == "tmux" {
                    "tmux_launch_failed"
                } else {
                    "herdr_launch_failed"
                },
                message: format!(
                    "failed to launch {backend} presentation for '{valid_name}': {}",
                    error.message
                ),
            });
        }
    };
    with_registry_lock_at(base, |agents| {
        if let Some(record) = agents.get_mut(&valid_name) {
            record.tmux_target = target.clone();
        }
        Ok(())
    })?;
    let mut queued_turn: Option<i64> = None;
    if let Some(brief) = brief {
        queued_turn = Some(enqueue_message_at(base, &valid_name, brief, None)?);
    }
    write_event_at(
        base,
        "agent_up",
        &valid_name,
        None,
        None,
        Some(&last_message_preview_at(base, &valid_name, 240)?),
    )?;
    let record = super::registry::read_registry_at(base)?
        .get(&valid_name)
        .cloned()
        .ok_or_else(|| ForeignOwnershipError {
            code: "unknown_agent",
            message: format!("unknown agent '{valid_name}'"),
        })?;
    Ok(UpResult {
        agent: agent_status_at(base, proc_root, &record)?,
        queued_turn,
        gc_notes: notes,
    })
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

    #[test]
    fn gc_reaps_a_dead_headless_worker_and_status_lists_live_ones() {
        let base = fake_base("gc-status");
        with_registry_lock_at(&base, |agents| {
            for (name, pid) in [("dead-1", Some(2_147_483_646)), ("live-1", None)] {
                agents.insert(
                    name.to_string(),
                    AgentRecord {
                        name: name.to_string(),
                        harness: "codex".to_string(),
                        backend: "tmux".to_string(),
                        tmux_target: "subagents:gone".to_string(),
                        status: "idle".to_string(),
                        runner_pid: pid,
                        created_at: "2026-11-01T00:00:00Z".to_string(),
                        ..AgentRecord::default()
                    },
                );
            }
            Ok(())
        })
        .unwrap();
        ensure_agent_dirs_at(&base, "dead-1").unwrap();
        // live-1 has no runner pid but is far past startup grace; gc would
        // reap it too, so give it this process as its live runner.
        with_registry_lock_at(&base, |agents| {
            let record = agents.get_mut("live-1").unwrap();
            record.runner_pid = Some(std::process::id() as i64);
            record.runner_started_at =
                pid_start_time_at(Path::new("/proc"), std::process::id() as i64).unwrap();
            record.runner_boot_id = Some(current_boot_id_at(Path::new("/proc")).unwrap());
            Ok(())
        })
        .unwrap();
        let notes = gc_at(&base, Path::new("/proc")).unwrap();
        assert!(
            notes.iter().any(|note| note.starts_with("reaped dead-1")),
            "{notes:?}"
        );
        let registry = read_registry_at(&base).unwrap();
        assert!(!registry.contains_key("dead-1"));
        assert!(registry.contains_key("live-1"));
        let status = status_snapshot_at(&base, Path::new("/proc"), Some("live-1"), false).unwrap();
        assert_eq!(status.agents.len(), 1);
        assert!(status.agents[0].runner_alive);
        // Down on the live stand-in refuses while its runner (this test
        // process) is alive and not the recorded daemon: terminate_runner
        // would signal it, so only check the unknown-agent path here.
        let gone = bring_down_agent_at(
            &base,
            Path::new("/proc"),
            "never-registered",
            Duration::ZERO,
            true,
            false,
        )
        .unwrap();
        assert!(!gone.was_registered);
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
