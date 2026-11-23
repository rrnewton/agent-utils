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

/// How long lock acquisition may wait, from `WRKSLOTS_WAIT_LOCK`
/// (seconds). The soak launcher sets this from the Python CLI's
/// `--wait-lock`. Unset means the bounded default of 30 seconds
/// (#1808-C, matching the Python edition); an explicit 0 (or an
/// invalid value) keeps fail-fast.
pub(crate) fn lock_wait() -> std::time::Duration {
    match std::env::var("WRKSLOTS_WAIT_LOCK") {
        Ok(text) => text
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .map(std::time::Duration::from_secs_f64)
            .unwrap_or_default(),
        Err(_) => std::time::Duration::from_secs(30),
    }
}

/// Block in the kernel for a flock until `deadline`, then report busy
/// (#1808-C). A waiter thread performs the blocking flock on a duplicated
/// descriptor (it shares the caller's open file description, so the lock
/// stays held by the caller's file). A wait cancelled by its deadline
/// unlocks immediately when it eventually acquires: a late acquisition
/// never acts and never leaks the lock.
fn flock_blocking(
    file: &std::fs::File,
    deadline: std::time::Instant,
    shared: bool,
) -> std::result::Result<(), rustix::io::Errno> {
    let nonblocking = if shared {
        rustix::fs::FlockOperation::NonBlockingLockShared
    } else {
        rustix::fs::FlockOperation::NonBlockingLockExclusive
    };
    let busy = match rustix::fs::flock(file, nonblocking) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if std::time::Instant::now() >= deadline {
        return Err(busy);
    }
    let Ok(waiter_file) = file.try_clone() else {
        return Err(busy);
    };
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_cancelled = cancelled.clone();
    let (sender, receiver) =
        std::sync::mpsc::channel::<std::result::Result<(), rustix::io::Errno>>();
    std::thread::spawn(move || {
        let blocking = if shared {
            rustix::fs::FlockOperation::LockShared
        } else {
            rustix::fs::FlockOperation::LockExclusive
        };
        match rustix::fs::flock(&waiter_file, blocking) {
            Ok(()) => {
                if thread_cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = rustix::fs::flock(&waiter_file, rustix::fs::FlockOperation::Unlock);
                    return;
                }
                let _ = sender.send(Ok(()));
            }
            // A failed flock (EINTR, ENOLCK, ...) is an error, never an
            // acquisition: the caller must not write without the lock.
            Err(error) => {
                let _ = sender.send(Err(error));
            }
        }
    });
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    match receiver.recv_timeout(remaining) {
        Ok(result) => result,
        Err(_) => {
            cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
            Err(busy)
        }
    }
}

/// Block for an exclusive flock until `deadline`, then report busy.
pub(crate) fn flock_wait(
    file: &std::fs::File,
    deadline: std::time::Instant,
) -> std::result::Result<(), rustix::io::Errno> {
    flock_blocking(file, deadline, false)
}

/// As `flock_wait`, for a shared lock (#1200: the readers' last,
/// locked measurement), which no exclusive writer holds at the same time.
pub(crate) fn flock_wait_shared(
    file: &std::fs::File,
    deadline: std::time::Instant,
) -> std::result::Result<(), rustix::io::Errno> {
    flock_blocking(file, deadline, true)
}

pub(crate) struct Flock {
    _file: std::fs::File,
    path: PathBuf,
}

impl Drop for Flock {
    fn drop(&mut self) {
        lock_order_release_path(&self.path);
        held_lock_release(&self.path);
    }
}

/// The global lock order (#2000), mirroring the Python edition:
/// project-root, config.lock, ACTIVE.global.intent, ACTIVE.global,
/// ACTIVE.<machine>. Recorded only when WRKSLOTS_LOCK_ORDER_CHECK=1.
pub(crate) fn lock_order_rank(name: &str) -> Option<u8> {
    match name {
        "project-root" => Some(0),
        "config.lock" => Some(1),
        "ACTIVE.global.intent" => Some(2),
        "ACTIVE.global" => Some(3),
        "ACTIVE.machine" => Some(4),
        _ => None,
    }
}

pub(crate) fn lock_order_class(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name == "ACTIVE.global.lock" {
        Some("ACTIVE.global")
    } else if name == "ACTIVE.global.intent.lock" {
        Some("ACTIVE.global.intent")
    } else if name.starts_with("ACTIVE.") && name.ends_with(".json.lock") {
        Some("ACTIVE.machine")
    } else {
        None
    }
}

thread_local! {
    static LOCK_ORDER_STACK: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// The enable decision is read once per process: `env::var_os` scans
/// the environment and allocates on every call, which made the
/// recorder itself the dominant cost of the release test step in the
/// suites that spawn this binary per case (#2000 measurement:
/// ~270 CPU-seconds across one wrkslotsd test run).
fn lock_order_check_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("WRKSLOTS_LOCK_ORDER_CHECK").is_some_and(|value| value == "1")
    })
}

pub(crate) fn lock_order_acquire(name: &'static str) -> Result<()> {
    if !lock_order_check_enabled() {
        return Ok(());
    }
    let rank = lock_order_rank(name).unwrap_or(0);
    LOCK_ORDER_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(above) = stack
            .iter()
            .rev()
            .find(|held| lock_order_rank(held).unwrap_or(0) > rank)
        {
            return Err(format!(
                "lock order violation: acquiring {name} while holding {above}"
            ));
        }
        stack.push(name);
        Ok(())
    })
}

pub(crate) fn lock_order_release(name: &str) {
    if !lock_order_check_enabled() {
        return;
    }
    LOCK_ORDER_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(index) = stack.iter().rposition(|held| *held == name) {
            stack.remove(index);
        }
    });
}

pub(crate) fn lock_order_release_path(path: &Path) {
    if let Some(name) = lock_order_class(path) {
        lock_order_release(name);
    }
}

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Registry locks this thread currently holds, mirroring
    /// Python `_HELD_LOCKS`: a nested acquisition of a lock the
    /// thread already holds skips the turnstile, so a writer
    /// waiting for this very thread cannot deadlock it.
    static HELD_LOCKS: RefCell<HashMap<PathBuf, usize>> = RefCell::new(HashMap::new());
}

fn held_lock_acquire(path: &Path) {
    HELD_LOCKS.with(|held| {
        *held.borrow_mut().entry(path.to_path_buf()).or_insert(0) += 1;
    });
}

fn held_lock_release(path: &Path) {
    HELD_LOCKS.with(|held| {
        let mut held = held.borrow_mut();
        if let Some(count) = held.get_mut(path) {
            *count -= 1;
            if *count == 0 {
                held.remove(path);
            }
        }
    });
}

fn thread_holds(path: &Path) -> bool {
    HELD_LOCKS.with(|held| held.borrow().contains_key(path))
}

/// Open one lock file with the shared safety checks.
fn open_lock_file(path: &Path, subject: &Path) -> Result<std::fs::File> {
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
            subject.display()
        ));
    }
    Ok(file)
}

/// How long a reader defers to a waiting writer, mirroring
/// Python `_REGISTRY_INTENT_DEFER_SECONDS`.
const INTENT_DEFER: std::time::Duration = std::time::Duration::from_secs(10);

/// The writer-intent turnstile (#1172/#1199), mirroring Python
/// `_enter_registry_turnstile`. Only the registry lock itself
/// (subject `ACTIVE`) has one; its path sits beside it. A writer
/// holds the intent lock exclusively while it polls, and returns
/// the file to keep until the registry lock is acquired. A reader
/// passes through shared and releases at once, deferring at most
/// INTENT_DEFER; a reader that cannot pass in that bound proceeds
/// anyway. Only the flock counts, never the file's existence, so
/// a stale intent file blocks nobody. Callers that do not know
/// the file (older releases, the other edition before its own
/// turnstile) weaken fairness but can never deadlock.
fn enter_turnstile(
    path: &Path,
    subject: &Path,
    exclusive: bool,
    deadline: std::time::Instant,
) -> Result<Option<std::fs::File>> {
    if subject.file_name().is_none_or(|name| name != "ACTIVE") || thread_holds(path) {
        return Ok(None);
    }
    let Some(parent) = subject.parent() else {
        return Ok(None);
    };
    let intent_path = parent.join("ACTIVE.global.intent.lock");
    let file = open_lock_file(&intent_path, &intent_path)?;
    if exclusive {
        match flock_wait(&file, deadline) {
            Ok(()) => {
                lock_order_acquire("ACTIVE.global.intent")?;
                Ok(Some(file))
            }
            Err(_) => Err(format!(
                "state lock is busy for {}; another writer is waiting for it; retry after the current command exits",
                subject.display()
            )),
        }
    } else {
        let defer_until = std::cmp::min(deadline, std::time::Instant::now() + INTENT_DEFER);
        if flock_wait_shared(&file, defer_until).is_ok() {
            lock_order_acquire("ACTIVE.global.intent")?;
            lock_order_release("ACTIVE.global.intent");
            let _ = rustix::fs::flock(&file, rustix::fs::FlockOperation::Unlock);
        }
        Ok(None)
    }
}

pub(crate) fn acquire_lock(path: &Path, subject: &Path) -> Result<Flock> {
    acquire_lock_with_deadline(path, subject, std::time::Instant::now() + lock_wait())
}

pub(crate) fn acquire_lock_with_deadline(
    path: &Path,
    subject: &Path,
    deadline: std::time::Instant,
) -> Result<Flock> {
    let file = open_lock_file(path, subject)?;
    // Hold the writer-intent turnstile while polling, then close
    // it once the registry lock is acquired (#1199).
    let _intent = enter_turnstile(path, subject, true, deadline)?;
    match flock_wait(&file, deadline) {
        Ok(()) => {
            if let Some(name) = lock_order_class(path) {
                // A violation refusal must not leak the intent record:
                // the stack is thread-local, so a leaked entry would
                // poison the next acquisition on this thread (the
                // order-check flake seen in the full test run).
                if let Err(error) = lock_order_acquire(name) {
                    drop(_intent);
                    lock_order_release("ACTIVE.global.intent");
                    return Err(error);
                }
            }
            drop(_intent);
            lock_order_release("ACTIVE.global.intent");
            held_lock_acquire(path);
            Ok(Flock {
                _file: file,
                path: path.to_path_buf(),
            })
        }
        Err(_) => {
            lock_order_release("ACTIVE.global.intent");
            Err(format!(
                "state lock is busy for {}; retry after the current command exits",
                subject.display()
            ))
        }
    }
}

/// As `acquire_lock`, for a shared lock on `path` (#1200): readers may
/// hold it together, and no writer holds it while they do. It waits as
/// long as `acquire_lock` (`WRKSLOTS_WAIT_LOCK`).
pub(crate) fn acquire_shared_lock(path: &Path, subject: &Path) -> Result<Flock> {
    let file = open_lock_file(path, subject)?;
    let deadline = std::time::Instant::now() + lock_wait();
    // Pass the writer-intent turnstile first (#1199/#1200): a
    // reader arriving after a waiting writer queues behind it.
    let _passed = enter_turnstile(path, subject, false, deadline)?;
    match flock_wait_shared(&file, deadline) {
        Ok(()) => {
            if let Some(name) = lock_order_class(path) {
                lock_order_acquire(name)?;
            }
            held_lock_acquire(path);
            Ok(Flock {
                _file: file,
                path: path.to_path_buf(),
            })
        }
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

pub(crate) fn atomic_write_bytes(path: &Path, contents: &[u8]) -> Result<()> {
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
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
        return result;
    }
    // No retry (fsyncgate): a retry returning Ok would not prove durability.
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            format!(
                "state file {} was renamed but its durability is unknown \
                 (directory fsync failed: {error}); the state may be \
                 committed; run 'wrkslots recover'",
                path.display()
            )
        })
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
    // Every append resolves through here, so no caller can write another
    // machine's history by passing a row's recorded machine (#876).  The
    // refusal names both machines so the stale copied shard is visible.
    if machine != cfg.machine {
        return Err(format!(
            "refusing to append to a foreign machine's event log: this host is {:?}, the record names {:?}; the foreign shard EVENTS.{machine} is read-only here",
            cfg.machine, machine
        ));
    }
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
    if kind == "active-state-recorded" {
        // Defence in depth (#1807): the payload must continue the revision
        // the log currently ends at, never a stale state a lock-free
        // preparation captured before another slot's event landed. This
        // scan walks every event on each active append; that is fine for
        // now (the chain was just loaded and verified above), and a
        // revision-only tip index would be the optimisation if it bites.
        let mut derived_revision: Option<i64> = None;
        for event in &events {
            match event.get("kind").and_then(Value::as_str) {
                Some("state-imported") => {
                    derived_revision = event
                        .pointer("/payload/active/revision")
                        .and_then(Value::as_i64);
                }
                Some("active-state-recorded") => {
                    derived_revision = event
                        .pointer("/payload/revision")
                        .and_then(Value::as_i64)
                        .or(derived_revision);
                }
                _ => {}
            }
        }
        if let Some(derived) = derived_revision {
            let claimed = payload.get("previous_revision").and_then(Value::as_i64);
            if claimed != Some(derived) {
                return Err(format!(
                    "refusing to append an active-state event from stale previous revision {}: the event log for {machine} currently ends at revision {derived}; nothing was appended",
                    claimed
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "missing".to_string())
                ));
            }
        }
        // Never append what the loader would reject: one such event refuses
        // every later command (#1373).
        crate::active_state::assert_appendable_active_payload(cfg, machine, &payload, &events)
            .map_err(|error| {
                format!("refusing to append an event the registry loader would reject: {error}")
            })?;
    }
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
    note_event_in_index(cfg, machine, &events, &event);
    Ok(event)
}

/// Fold one event into the scoped-status index state (#2039).
fn index_observe(index: &mut Map<String, Value>, event: &Value) {
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
    let sequence = event.get("sequence").cloned().unwrap_or(Value::Null);
    let kind = event
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let machine = index
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(tip) = index.get_mut("tip").and_then(Value::as_object_mut) {
        tip.insert("sequence".to_string(), sequence.clone());
        tip.insert(
            "sha256".to_string(),
            event.get("sha256").cloned().unwrap_or(Value::Null),
        );
    }
    match kind.as_str() {
        "active-state-recorded" => {
            index.insert(
                "active_revision".to_string(),
                payload.get("revision").cloned().unwrap_or(Value::Null),
            );
            if let (Some(slot), Some(slots)) = (
                payload.get("slot").and_then(Value::as_str),
                index
                    .entry("slots".to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut(),
            ) {
                slots
                    .entry(slot)
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("slot entry is an object")
                    .insert("active_state".to_string(), sequence.clone());
            }
        }
        "state-imported" => {
            index.insert(
                "active_revision".to_string(),
                payload
                    .pointer("/active/revision")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            if let Some(holds) = payload.get("holds").and_then(Value::as_array) {
                let imported: Vec<String> = holds
                    .iter()
                    .filter_map(|item| item.get("slot").and_then(Value::as_str).map(str::to_string))
                    .collect();
                if let Some(slots) = index
                    .entry("slots".to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                {
                    for slot in imported {
                        slots
                            .entry(slot)
                            .or_insert_with(|| Value::Object(Map::new()))
                            .as_object_mut()
                            .expect("slot entry is an object")
                            .insert("imported".to_string(), sequence.clone());
                    }
                }
            }
        }
        _ => {}
    }
    let key = match kind.as_str() {
        "slot-held" => Some("held"),
        "slot-hold-released" => Some("hold_released"),
        "owner-released" => Some("released"),
        _ => None,
    };
    if let (Some(key), Some(slot)) = (key, payload.get("slot").and_then(Value::as_str)) {
        let slot = slot.to_string();
        if let Some(slots) = index
            .entry("slots".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
        {
            slots
                .entry(slot)
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("slot entry is an object")
                .insert(key.to_string(), sequence.clone());
        }
    }
    if kind != "operation-progress-recorded"
        && kind != "operation-completed"
        && kind != "operation-quarantined"
    {
        return;
    }
    let name = payload
        .get("journal_path")
        .and_then(Value::as_str)
        .filter(|name| {
            std::path::Path::new(name)
                .file_name()
                .is_some_and(|file| file == *name)
        })
        .map(str::to_string)
        .unwrap_or_else(|| format!("ACTIVE.{machine}.journal"));
    let mut pending_conflict = false;
    {
        let pending = index
            .entry("pending".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(pending_obj) = pending.as_object_mut() else {
            return;
        };
        if kind == "operation-progress-recorded" {
            let journal = payload.get("journal");
            let identity = (
                journal
                    .and_then(|j| j.get("kind"))
                    .cloned()
                    .unwrap_or(Value::Null),
                journal
                    .and_then(|j| j.get("slot"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            let conflict = pending_obj.iter().any(|(existing_name, existing)| {
                *existing_name != name
                    && (
                        existing.get("kind").cloned().unwrap_or(Value::Null),
                        existing.get("slot").cloned().unwrap_or(Value::Null),
                    ) == identity
            });
            if conflict {
                pending_conflict = true;
            }
            pending_obj.insert(name, json!({"kind": identity.0, "slot": identity.1}));
        } else {
            let matched = pending_obj.get(&name).is_some_and(|current| {
                payload.get("slot").cloned().unwrap_or(Value::Null)
                    == current.get("slot").cloned().unwrap_or(Value::Null)
                    && payload.get("operation").cloned().unwrap_or(Value::Null)
                        == current.get("kind").cloned().unwrap_or(Value::Null)
            });
            if matched {
                pending_obj.remove(&name);
            } else if pending_obj.contains_key(&name) {
                pending_conflict = true;
            }
        }
    }
    if pending_conflict {
        index.insert("pending_conflict".to_string(), Value::Bool(true));
    }
}

/// Maintain the scoped-status index after one append (#2039).
///
/// Applies incrementally only when the index tip is exactly the chain
/// tip this append extended; otherwise rebuilds from the chain the
/// caller just loaded. Advisory only: a stale index is rejected by
/// readers, so failures here never fail the append.
fn note_event_in_index(cfg: &Cfg, machine: &str, events_before: &[Value], event: &Value) {
    let path = cfg.control.join(format!("EVENT-INDEX.{machine}.json"));
    let mut events: Vec<Value> = events_before.to_vec();
    events.push(event.clone());
    let rebuilt = |events: &[Value]| -> Value {
        let mut index = Map::new();
        index.insert("schema".to_string(), json!(1));
        index.insert("machine".to_string(), json!(machine));
        index.insert(
            "tip".to_string(),
            json!({"sequence": 0, "sha256": "0".repeat(64)}),
        );
        index.insert("active_revision".to_string(), Value::Null);
        index.insert("slots".to_string(), Value::Object(Map::new()));
        index.insert("pending".to_string(), Value::Object(Map::new()));
        for item in events {
            index_observe(&mut index, item);
        }
        Value::Object(index)
    };
    let _ = {
        let loaded: Option<Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        let previous_sha = events_before
            .last()
            .and_then(|item| item.get("sha256"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "0".repeat(64));
        let continuous = loaded.as_ref().is_some_and(|index| {
            index.get("schema").and_then(Value::as_i64) == Some(1)
                && index.get("machine").and_then(Value::as_str) == Some(machine)
                && index.pointer("/tip/sequence").and_then(Value::as_i64)
                    == Some(events_before.len() as i64)
                && index.pointer("/tip/sha256").and_then(Value::as_str)
                    == Some(previous_sha.as_str())
        });
        let index = if continuous {
            let mut index = loaded
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            index_observe(&mut index, event);
            Value::Object(index)
        } else {
            rebuilt(&events)
        };
        atomic_write_json(&path, &index)
    };
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
    let mut archive = parse_archive(root, Some(machine))?;
    if let Some(document) = archive.as_object_mut() {
        document.remove("quarantined_records");
    }
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

/// Whether an argv runs a wrkslots removal command, mirroring
/// `_is_wrkslots_command`.
fn is_wrkslots_command(argv: &[String]) -> bool {
    const OPERATORS: [&str; 4] = [
        "remove",
        "remove-validate-batch",
        "remove-agent-batch",
        "retire-pending",
    ];
    let mut runs = false;
    for (index, argument) in argv.iter().enumerate() {
        let module_form = argument == "wrkslots" && index > 0 && argv[index - 1] == "-m";
        let script_form = argument.ends_with("/wrkslots/__main__.py");
        let binary_form = index <= 1
            && std::path::Path::new(argument)
                .file_name()
                .and_then(|name| name.to_str())
                == Some("wrkslots");
        if module_form || script_form || binary_form {
            runs = true;
        }
    }
    runs && argv
        .iter()
        .any(|argument| OPERATORS.contains(&argument.as_str()))
}

fn path_within(candidate: &Path, root: &Path) -> bool {
    candidate.starts_with(root) || root.starts_with(candidate)
}

/// PIDs of live wrkslots removal processes for this project acting on
/// `targets`, mirroring `_other_live_wrkslots_processes`.
pub(crate) fn other_live_wrkslots_processes(root: &Path, targets: &[String]) -> Vec<i64> {
    other_live_wrkslots_processes_in(root, targets, Path::new("/proc"))
}

/// The scan over a caller-chosen process table (#1778): the live
/// path passes /proc; tests pass a fake table so the rule is judged
/// hermetically, never against whatever the host happens to run.
pub(crate) fn other_live_wrkslots_processes_in(
    root: &Path,
    targets: &[String],
    proc_root: &Path,
) -> Vec<i64> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let own = i64::from(std::process::id());
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return vec![-1];
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(pid) = name.parse::<i64>() else {
            continue;
        };
        if pid == own {
            continue;
        }
        let raw = match std::fs::read(entry.path().join("cmdline")) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                found.push(pid);
                continue;
            }
        };
        let argv: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        if !is_wrkslots_command(&argv) {
            continue;
        }
        if !targets.is_empty()
            && !targets.iter().any(|target| {
                argv.iter().any(|part| {
                    part == target
                        || part.split('=').next() == Some(target.as_str())
                        || part.contains(&format!("/{target}/"))
                        || part.ends_with(&format!("/{target}"))
                })
            })
        {
            continue;
        }
        // The FIRST --project-root wins, as in `_wrkslots_project_of`:
        // a wrapper argv embedding a whole wrkslots invocation as data
        // must not have its last mention judge the process (#1778).
        let mut project: Option<PathBuf> = None;
        for (index, argument) in argv.iter().enumerate() {
            if project.is_some() {
                break;
            }
            if argument == "--project-root" {
                project = argv.get(index + 1).map(PathBuf::from);
            } else if let Some(value) = argument.strip_prefix("--project-root=") {
                project = Some(PathBuf::from(value));
            }
        }
        let project = match project {
            Some(project) => Some(project),
            None => std::fs::read_link(entry.path().join("cwd")).ok(),
        };
        let same_project = match &project {
            None => true,
            Some(candidate) => {
                let resolved =
                    std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone());
                path_within(&resolved, &root)
            }
        };
        if same_project {
            found.push(pid);
        }
    }
    found
}

/// Whether the batch that owns one seal is still running (#1736),
/// mirroring `_validate_batch_seal_owner_liveness`: `(state, pid,
/// created_at, detail)`. The recorded actor is the authorizing
/// coordinator, so "live" also requires a wrkslots process acting on
/// the sealed targets; its pid is reported then.
pub(crate) fn seal_owner_liveness(
    path: &Path,
    root: &Path,
) -> (String, Option<i64>, Option<String>, String) {
    let unknown = |detail: String| ("unknown".to_string(), None, None, detail);
    let Ok(text) = std::fs::read_to_string(path) else {
        return unknown("the seal cannot be read".to_string());
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return unknown("the seal cannot be judged: invalid JSON".to_string());
    };
    // Minimal shape validation mirroring `_validate_batch_seal_journal`
    // (#1736 review): an invalid seal is unknown, never judged.
    if value.get("kind").and_then(Value::as_str) != Some("validate-batch-seal")
        || value.get("schema").and_then(Value::as_i64) != Some(1)
    {
        return unknown("the seal cannot be judged: wrong kind or schema".to_string());
    }
    let Some(actor) = value.get("actor") else {
        return unknown("the seal records no actor".to_string());
    };
    let Some(identity) = crate::status_state::identity_from_value(actor) else {
        return unknown("the seal records no actor".to_string());
    };
    let since = value
        .get("created_at")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // The recorded batch process is the proven identity (#1736):
    // judge it alone when the seal carries one. This process itself
    // never blocks itself, as the scan below excludes its own pid.
    let operator_value = value
        .get("x")
        .and_then(|extension| extension.get("operator"));
    if let Some(operator_value) = operator_value {
        if !operator_value.is_null()
            && operator_value.get("pid").and_then(Value::as_i64)
                != Some(i64::from(std::process::id()))
        {
            let Some(operator) = crate::status_state::identity_from_value(operator_value) else {
                return unknown("the seal records no operator".to_string());
            };
            // "dead" takes the orphan proof (#1869), judged
            // relative to this process's own PID namespace: only a
            // recording from this namespace (or an unstamped legacy
            // seal under the verified host view) lets a missing or
            // reused PID count.
            let own_stamp = {
                use std::os::unix::fs::MetadataExt as _;
                std::fs::metadata("/proc/self/ns/pid")
                    .map(|metadata| format!("pid:[{}]", metadata.ino()))
                    .unwrap_or_default()
            };
            if (operator.pidns_inode.as_deref() == Some(own_stamp.as_str())
                && crate::audit_state::assert_proc_consistent().is_err())
                || (operator.pidns_inode.as_deref() != Some(own_stamp.as_str())
                    && crate::audit_state::assert_host_process_view().is_err())
            {
                return (
                    "unknown".to_string(),
                    Some(operator.pid),
                    since,
                    "the host process view is unverifiable".to_string(),
                );
            }
            if crate::classify_journals::seal_operator_proven_dead(&operator) {
                return (
                    "dead".to_string(),
                    Some(operator.pid),
                    since,
                    "the seal's operator is proven dead".to_string(),
                );
            }
            let (ostate, odetail) = crate::status_state::process_state(Some(&operator));
            return match ostate.as_str() {
                "live" => ("live".to_string(), Some(operator.pid), since, odetail),
                _ => (
                    "unknown".to_string(),
                    Some(operator.pid),
                    since,
                    format!("the seal's operator is not proven dead: {odetail}"),
                ),
            };
        }
    }
    let (state, detail) = crate::status_state::process_state(Some(&identity));
    match state.as_str() {
        "dead" => ("dead".to_string(), Some(identity.pid), since, detail),
        "live" => {
            let mut targets: Vec<String> = Vec::new();
            if let Some(items) = value.get("targets").and_then(Value::as_array) {
                for item in items {
                    if let Some(slot) = item.get("slot").and_then(Value::as_str) {
                        if !targets.iter().any(|existing| existing == slot) {
                            targets.push(slot.to_string());
                        }
                    }
                    if let Some(stored) = item.get("path").and_then(Value::as_str) {
                        if !targets.iter().any(|existing| existing == stored) {
                            targets.push(stored.to_string());
                        }
                    }
                }
            }
            let operators = other_live_wrkslots_processes(root, &targets);
            if operators == [-1] {
                return (
                    "unknown".to_string(),
                    None,
                    since,
                    "the process list cannot be read, so no batch process can be proven"
                        .to_string(),
                );
            }
            if let Some(pid) = operators.first() {
                ("live".to_string(), Some(*pid), since, detail)
            } else {
                (
                    "dead".to_string(),
                    Some(identity.pid),
                    since,
                    "no wrkslots process is acting on the sealed targets".to_string(),
                )
            }
        }
        _ => (
            "unknown".to_string(),
            Some(identity.pid),
            since,
            format!("the seal's actor is {state}: {detail}"),
        ),
    }
}

/// Refusal text for blocking seals (#1736), mirroring the seal branch
/// of `_refuse_partial_state`: live owners are in progress and never
/// pointed at recovery; only proven-dead owners are "interrupted".
pub(crate) fn seal_blocking_message(paths: &[PathBuf], root: &Path) -> String {
    let first = paths
        .first()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let statuses: Vec<(String, Option<i64>, Option<String>, String)> = paths
        .iter()
        .map(|path| seal_owner_liveness(path, root))
        .collect();
    for (state, pid, since, _detail) in &statuses {
        if state == "live" {
            if let (Some(pid), Some(since)) = (pid, since) {
                return format!(
                    "validation-batch seal in progress (pid {pid}, since {since}) recorded in {first}; the remove-validate-batch that owns it is still running; wait for it to finish and retry; list blocking journals with 'wrkslots classify-create-journals --all-kinds'"
                );
            }
        }
    }
    if !statuses.is_empty() && statuses.iter().all(|(state, ..)| state == "dead") {
        return format!(
            "interrupted validation-batch seal recorded in {first}; run 'wrkslots recover' first; list blocking journals with 'wrkslots classify-create-journals --all-kinds'"
        );
    }
    let detail = statuses
        .first()
        .map(|(_, _, _, detail)| detail.clone())
        .unwrap_or_default();
    format!(
        "validation-batch seal recorded in {first}; its owner cannot be proven dead here ({detail}), so it still binds; list blocking journals with 'wrkslots classify-create-journals --all-kinds'"
    )
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
    if !seals.is_empty() {
        let root = control
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_default();
        return Err(seal_blocking_message(&seals, &root));
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

#[cfg(test)]
mod lock_wait_tests {
    use super::*;

    #[test]
    fn flock_wait_waits_for_the_holder() {
        let dir = std::env::temp_dir().join(format!("wrkslots-lock-wait-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lock");
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        rustix::fs::flock(&holder, rustix::fs::FlockOperation::LockExclusive).unwrap();
        let started = std::time::Instant::now();
        let waiter_path = path.clone();
        let waiter = std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(waiter_path)
                .unwrap();
            flock_wait(
                &file,
                std::time::Instant::now() + std::time::Duration::from_secs(5),
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(holder);
        let result = waiter.join().unwrap();
        assert!(result.is_ok());
        assert!(started.elapsed() >= std::time::Duration::from_millis(250));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flock_wait_past_deadline_releases_its_late_acquisition() {
        // #1808-C: a waiter whose deadline passes while the holder still
        // holds must never keep the lock when it eventually acquires.
        let dir = std::env::temp_dir().join(format!("wrkslots-lock-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lock");
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        rustix::fs::flock(&holder, rustix::fs::FlockOperation::LockExclusive).unwrap();
        let waiter = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let result = flock_wait(
            &waiter,
            std::time::Instant::now() + std::time::Duration::from_millis(150),
        );
        assert!(result.is_err());
        // The caller's file stays open, as it does in real callers: only
        // the waiter's cancel-and-unlock can free the lock now. Without
        // it, the late acquisition holds through the shared description
        // and this probe never succeeds.
        drop(holder);
        // Condition, not a clock: the lock becomes free and stays free.
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut free = false;
        for _ in 0..100 {
            if rustix::fs::flock(&probe, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .is_ok()
            {
                free = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(free, "a late acquisition leaked the lock");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_deadline_bounds_every_lock_in_a_mutation() {
        // #1808 review: heartbeat's locks share one deadline, so three
        // held locks cost one wait, not one wait each.
        let dir =
            std::env::temp_dir().join(format!("wrkslots-shared-deadline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let subject_a = dir.join("ACTIVE");
        let subject_b = dir.join("ACTIVE.node.json");
        let lock_a = dir.join("a.lock");
        let lock_b = dir.join("b.lock");
        let holders: Vec<std::fs::File> = [&lock_a, &lock_b]
            .iter()
            .map(|path| {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .unwrap();
                rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).unwrap();
                file
            })
            .collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        let started = std::time::Instant::now();
        assert!(acquire_lock_with_deadline(&lock_a, &subject_a, deadline).is_err());
        assert!(acquire_lock_with_deadline(&lock_b, &subject_b, deadline).is_err());
        // The second acquisition had no fresh wait left to spend.
        assert!(started.elapsed() < std::time::Duration::from_millis(550));
        drop(holders);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flock_wait_propagates_a_flock_error_instead_of_reporting_acquired() {
        // A descriptor flock cannot act on (an O_PATH fd) fails both the
        // non-blocking attempt and the blocking waiter; that error must
        // reach the caller, never an Ok that writes without the lock.
        let target =
            std::env::temp_dir().join(format!("wrkslots-opath-target-{}", std::process::id()));
        std::fs::write(&target, b"").unwrap();
        let path_fd = rustix::fs::openat(
            rustix::fs::CWD,
            target.as_path(),
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        let file = std::fs::File::from(path_fd);
        let result = flock_wait(
            &file,
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        );
        assert!(
            result.is_err(),
            "flock on an O_PATH descriptor must not report acquired"
        );
    }

    fn turnstile_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "wrkslots-turnstile-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let subject = dir.join("ACTIVE");
        std::fs::write(&subject, "").unwrap();
        (subject.clone(), dir.join("ACTIVE.global.lock"), dir)
    }

    #[test]
    fn waiting_writer_holds_the_intent_lock() {
        std::env::set_var("WRKSLOTS_WAIT_LOCK", "10");
        let (subject, lock_path, dir) = turnstile_fixture("holds");
        let reader = acquire_shared_lock(&lock_path, &subject).unwrap();
        let (writer_subject, writer_lock) = (subject.clone(), lock_path.clone());
        let writer = std::thread::spawn(move || acquire_lock(&writer_lock, &writer_subject));
        std::thread::sleep(std::time::Duration::from_millis(300));
        // The waiting writer holds the intent lock exclusively.
        let intent = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("ACTIVE.global.intent.lock"))
            .unwrap();
        assert!(rustix::fs::flock(
            &intent,
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        )
        .is_err());
        drop(intent);
        drop(reader);
        let acquired = writer.join().unwrap();
        assert!(acquired.is_ok());
        std::env::remove_var("WRKSLOTS_WAIT_LOCK");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_intent_file_blocks_nobody() {
        let (subject, lock_path, dir) = turnstile_fixture("stale");
        std::fs::write(dir.join("ACTIVE.global.intent.lock"), "").unwrap();
        let reader = acquire_shared_lock(&lock_path, &subject).unwrap();
        drop(reader);
        let _writer = acquire_lock(&lock_path, &subject).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_writer_reports_another_writer_waiting() {
        let (subject, lock_path, dir) = turnstile_fixture("busy");
        let intent = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("ACTIVE.global.intent.lock"))
            .unwrap();
        rustix::fs::flock(&intent, rustix::fs::FlockOperation::LockExclusive).unwrap();
        let error = match acquire_lock(&lock_path, &subject) {
            Ok(_) => panic!("second writer must refuse"),
            Err(error) => error,
        };
        assert!(error.contains("another writer is waiting"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod fallback_scan_tests {
    use super::other_live_wrkslots_processes_in;
    use std::path::{Path, PathBuf};

    fn table() -> (PathBuf, PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("wrkslots-fallback-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("project");
        let other = base.join("other");
        let proc_root = base.join("proc");
        for dir in [
            root.clone(),
            other.clone(),
            proc_root.clone(),
            root.join("sub"),
            other.join("sub"),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let process = |pid: i64, argv: &[String], cwd: Option<&Path>| {
            let entry = proc_root.join(pid.to_string());
            std::fs::create_dir_all(&entry).unwrap();
            let mut bytes: Vec<u8> = Vec::new();
            for part in argv {
                bytes.extend_from_slice(part.as_bytes());
                bytes.push(0);
            }
            std::fs::write(entry.join("cmdline"), bytes).unwrap();
            if let Some(cwd) = cwd {
                std::os::unix::fs::symlink(cwd, entry.join("cwd")).unwrap();
            }
        };
        let r = root.display().to_string();
        let o = other.display().to_string();
        let argv =
            |parts: &[&str]| -> Vec<String> { parts.iter().map(|part| part.to_string()).collect() };
        process(
            4101,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                "--project-root",
                &r,
                "remove",
                "target",
            ]),
            Some(&base),
        );
        process(
            4102,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                "--project-root",
                &o,
                "remove",
                "target",
            ]),
            Some(&base),
        );
        process(
            4103,
            &argv(&["python3", "-m", "wrkslots", "remove", "target"]),
            Some(&root.join("sub")),
        );
        process(
            4104,
            &argv(&["python3", "-m", "wrkslots", "remove", "target"]),
            Some(&other.join("sub")),
        );
        process(
            4105,
            &argv(&["python3", "-m", "wrkslots", "remove", "target"]),
            None,
        );
        let unreadable = proc_root.join("4106");
        std::fs::create_dir_all(unreadable.join("cmdline")).unwrap();
        process(
            4107,
            &argv(&["python3", "-m", "wrkslots", "--project-root", &r, "recover"]),
            Some(&root),
        );
        process(
            4108,
            &argv(&["/usr/bin/wrkslots", "remove", "target"]),
            Some(&root),
        );
        process(
            4109,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                "--project-root",
                &r,
                "remove",
                "other",
            ]),
            Some(&base),
        );
        process(
            4110,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                &format!("--project-root={r}"),
                "remove-validate-batch",
                "--slot",
                "target=1",
            ]),
            Some(&base),
        );
        process(
            4111,
            &argv(&[
                "python3",
                "-m",
                "wrkslots.tests.idle_validation_host",
                "--project-root",
                &r,
                "remove",
                "target",
            ]),
            Some(&root),
        );
        process(
            4112,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                "--project-root",
                &r,
                "remove",
                "target",
            ]),
            Some(&other),
        );
        process(
            4113,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                "--project-root",
                &o,
                "remove",
                "target",
                "--project-root",
                &r,
            ]),
            Some(&base),
        );
        process(
            4114,
            &argv(&[
                "python3",
                "-m",
                "wrkslots",
                "--project-root",
                &r,
                "remove",
                "target",
                "--project-root",
                &o,
            ]),
            Some(&base),
        );
        (base, root, proc_root)
    }

    #[test]
    fn fallback_scan_table_matches_python() {
        let (base, root, proc_root) = table();
        let targets = vec!["target".to_string()];
        let mut with_targets = other_live_wrkslots_processes_in(&root, &targets, &proc_root);
        with_targets.sort_unstable();
        assert_eq!(
            with_targets,
            vec![4101, 4103, 4105, 4106, 4108, 4110, 4112, 4114]
        );
        let mut without_targets = other_live_wrkslots_processes_in(&root, &[], &proc_root);
        without_targets.sort_unstable();
        assert_eq!(
            without_targets,
            vec![4101, 4103, 4105, 4106, 4108, 4109, 4110, 4112, 4114]
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
