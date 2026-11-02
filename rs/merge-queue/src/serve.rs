//! Long-running `serve` daemon and `status` reporting.
//!
//! `serve` is deterministic code that is always running so agents never drive
//! the queue loop by hand. It polls cheaply while idle and holds the queue
//! writer lock only while it is working:
//!
//! * Idle: no lock is held. The loop sleeps in short slices, refreshes the
//!   heartbeat file, and watches two triggers — the fingerprint of
//!   `items/` (new submit/sync/withdraw state) and the remote base commit
//!   (`git ls-remote`, polled on its own slower cadence).
//!   Two more triggers are time-based: a queued change whose refusal
//!   deferral (`next_retry_at`) has passed, and the retry after a failed work
//!   cycle (exponential backoff from [`FAILURE_BACKOFF_BASE`], capped at
//!   [`FAILURE_BACKOFF_CAP`]).
//! * Working: the lock is acquired for one plan+run cycle (the existing
//!   [`crate::run::Runner`] path; the logic is not forked) and the
//!   notification sweep, then released before the daemon goes back to idle
//!   polling. A helper thread keeps the heartbeat fresh while the cycle runs.
//!
//! One daemon per state directory is enforced by an exclusive `flock` on
//! `serve.lock`, held for the daemon's lifetime (the kernel drops it when the
//! process dies, so a crash never blocks the next start). `serve.pid` (the
//! daemon pid) is informational, and `serve.heartbeat.json` is rewritten
//! every cycle. `status` reads them plus the item records and the scratch
//! slots, and never claims a daemon is running when the lock is free, the pid
//! is dead or the heartbeat is stale.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::model::{now, Item, ItemStatus};
use crate::notify;
use crate::ops;
use crate::run::{RunOptions, Runner};
use crate::state::{write_json, Config, State};

/// Default idle poll interval in milliseconds (how often `serve` checks the
/// `items/` fingerprint and refreshes the heartbeat).
pub const DEFAULT_POLL_MS: u64 = 250;
/// Default interval in seconds between `git ls-remote` base polls.
pub const DEFAULT_BASE_POLL_SECS: u64 = 30;
/// Heartbeat age in seconds after which `status` calls a live pid stale
/// when the caller does not override it.
pub const DEFAULT_STALE_AFTER_SECS: u64 = 10;
/// How often a helper thread refreshes the heartbeat while a work cycle
/// runs, so `status` never mistakes a long validation for a hung daemon.
pub const HEARTBEAT_REFRESH: Duration = Duration::from_millis(500);
/// First retry delay after a failed work cycle; doubled per consecutive
/// failure.
pub const FAILURE_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Longest retry delay after failed work cycles.
pub const FAILURE_BACKOFF_CAP: Duration = Duration::from_secs(300);

/// Delay before retrying after `failures` consecutive failed work cycles
/// (1-based): 1 s, 2 s, 4 s, ... capped at [`FAILURE_BACKOFF_CAP`].
pub fn failure_backoff(failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(16);
    FAILURE_BACKOFF_BASE
        .saturating_mul(1u32 << shift)
        .min(FAILURE_BACKOFF_CAP)
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn shutdown_flag() -> &'static std::sync::Arc<AtomicBool> {
    static FLAG: std::sync::OnceLock<std::sync::Arc<AtomicBool>> = std::sync::OnceLock::new();
    FLAG.get_or_init(|| std::sync::Arc::new(AtomicBool::new(false)))
}

fn install_signal_handlers() {
    // `signal-hook` registers the handlers behind a safe API, so this
    // crate keeps `#![forbid(unsafe_code)]`. Both signals set one flag.
    let _ = signal_hook::flag::register(
        signal_hook::consts::SIGINT,
        std::sync::Arc::clone(shutdown_flag()),
    );
    let _ = signal_hook::flag::register(
        signal_hook::consts::SIGTERM,
        std::sync::Arc::clone(shutdown_flag()),
    );
}

/// Whether shutdown has been requested (also used by tests).
pub fn shutdown_requested() -> bool {
    shutdown_flag().load(Ordering::SeqCst)
}

/// Reset the shutdown flag (test support; a fresh daemon starts clear).
pub fn reset_shutdown() {
    shutdown_flag().store(false, Ordering::SeqCst);
}

/// Daemon heartbeat, rewritten every serve cycle to `serve.heartbeat.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Daemon process id.
    pub pid: u32,
    /// RFC 3339 UTC start time.
    pub started_at: String,
    /// RFC 3339 UTC time of this heartbeat.
    pub heartbeat_at: String,
    /// Unix seconds of `heartbeat_at`, for staleness arithmetic.
    pub heartbeat_unix: u64,
    /// Completed idle/work cycles since start.
    pub cycles: u64,
    /// `idle`, `working` or `stopped`.
    pub state: String,
    /// Last trigger that caused (or would cause) work: `startup`, `items`,
    /// `base`, `items+base`, `none`, `lock-busy` or `error`.
    pub last_trigger: String,
    /// Base commit seen by the most recent base poll (empty before it).
    pub last_base_sha: String,
    /// Queued changes at heartbeat time.
    pub queued: usize,
    /// Batches the current/last work cycle planned (empty while idle with
    /// nothing planned yet).
    pub active_batches: Vec<Vec<i64>>,
    /// Configured window (initial depth) the daemon runs with.
    pub window: usize,
    /// Scratch validation slots that exist on disk.
    pub slots_total: usize,
    /// Scratch slots that currently hold a checkout (non-empty).
    pub slots_in_use: usize,
    /// Run id of the most recent work cycle, when one ran.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_run: String,
    /// RFC 3339 UTC time the most recent work cycle finished.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_run_at: String,
    /// Most recent non-fatal error (base poll or work cycle), for `status`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_error: String,
    /// Whether this daemon lands on a simulated base (`serve --shadow`):
    /// validations are real, landings are recorded as shadow/simulated,
    /// item statuses stay queued, and the real base ref is never pushed.
    /// `false` for heartbeats written before shadow serve existed.
    #[serde(default)]
    pub shadow: bool,
    /// RFC 3339 UTC time the current work cycle started; empty while idle.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_since: String,
    /// Unix seconds of `working_since`; 0 while idle.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub working_since_unix: u64,
    /// When the daemon next retries on its own (RFC 3339 UTC): the earliest
    /// refusal deferral of a queued change, or the backoff retry after a
    /// failed work cycle. Empty when nothing is scheduled.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_retry_at: String,
    /// Consecutive failed work cycles (reset by a clean one).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub failed_cycles: u32,
}

impl Heartbeat {
    fn new(pid: u32, window: usize, shadow: bool) -> Self {
        let started = now();
        Self {
            pid,
            started_at: started.clone(),
            heartbeat_at: started,
            heartbeat_unix: unix_now(),
            cycles: 0,
            state: "idle".to_owned(),
            last_trigger: "startup".to_owned(),
            last_base_sha: String::new(),
            queued: 0,
            active_batches: Vec::new(),
            window,
            slots_total: 0,
            slots_in_use: 0,
            last_run: String::new(),
            last_run_at: String::new(),
            last_error: String::new(),
            shadow,
            working_since: String::new(),
            working_since_unix: 0,
            next_retry_at: String::new(),
            failed_cycles: 0,
        }
    }

    fn touch(&mut self) {
        self.heartbeat_at = now();
        self.heartbeat_unix = unix_now();
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a pid is alive, via `/proc/<pid>` (Linux; no signal sent).
pub fn pid_alive(pid: u32) -> bool {
    pid > 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// FNV-1a hash of `bytes` (deterministic, no dependency).
fn fnv1a(bytes: &[u8], mut hash: u64) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Cheap deterministic fingerprint of the `items/` directory.
///
/// Combines each record's name, length and contents. Reading a handful of
/// small JSON files every poll is cheap; the contents hash means a submit
/// is seen even when two writes land in the same mtime tick.
pub fn items_fingerprint(state: &State) -> String {
    let dir = state.items_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return "items:none".to_owned();
    };
    let mut names: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    let mut hash: u64 = 0xcbf29ce484222325;
    for path in &names {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        hash = fnv1a(name.as_bytes(), hash);
        if let Ok(bytes) = std::fs::read(path) {
            hash = fnv1a(&(bytes.len() as u64).to_le_bytes(), hash);
            hash = fnv1a(&bytes, hash);
        }
    }
    format!("items:{}:{hash:016x}", names.len())
}

/// Path of the single-daemon lock file (`flock`ed by a running daemon).
pub fn serve_lock_path(state: &State) -> PathBuf {
    state.root.join("serve.lock")
}

/// Whether a daemon holds `serve.lock`: `Some(true)` held, `Some(false)`
/// free, `None` when the file does not exist (no daemon that takes the lock
/// ever ran here). The probe takes a shared lock for an instant only.
pub fn serve_lock_held(state: &State) -> Option<bool> {
    let file = File::open(serve_lock_path(state)).ok()?;
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            Some(false)
        }
        Err(_) => Some(true),
    }
}

/// Take the single-daemon lock or explain who holds it. Retries for about a
/// second because a `status` probe may hold a shared lock for an instant.
fn acquire_serve_lock(state: &State) -> Result<File, String> {
    std::fs::create_dir_all(&state.root)
        .map_err(|e| format!("cannot create {}: {e}", state.root.display()))?;
    let path = serve_lock_path(state);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if FileExt::try_lock_exclusive(&file).is_ok() {
            return Ok(file);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let hb = read_heartbeat(state)
        .map(|h| format!(", heartbeat {}", h.heartbeat_at))
        .unwrap_or_default();
    Err(match read_pid(state) {
        Some(pid) => format!(
            "merge-queue serve is already running (pid {pid}{hb}; it holds {}); \
             remedy: stop it with `kill {pid}` or check it with `merge-queue status`",
            path.display()
        ),
        None => format!(
            "merge-queue serve is already running (another daemon holds {}{hb}); \
             remedy: check it with `merge-queue status`",
            path.display()
        ),
    })
}

/// Read the daemon pid file, when present and parseable.
pub fn read_pid(state: &State) -> Option<u32> {
    let text = std::fs::read_to_string(state.serve_pid_path()).ok()?;
    text.trim().parse::<u32>().ok().filter(|pid| *pid > 0)
}

/// Read the heartbeat file, when present and parseable.
pub fn read_heartbeat(state: &State) -> Option<Heartbeat> {
    let text = std::fs::read_to_string(state.serve_heartbeat_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Count scratch validation slots: `(existing, in use)`.
pub fn slot_occupancy(state: &State) -> (usize, usize) {
    let dir = state.scratch_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (0, 0);
    };
    let mut total = 0usize;
    let mut in_use = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_slot = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("slot-"));
        if is_slot && path.is_dir() {
            total += 1;
            let non_empty = std::fs::read_dir(&path).is_ok_and(|mut it| it.next().is_some());
            if non_empty {
                in_use += 1;
            }
        }
    }
    (total, in_use)
}

/// Poll the remote base commit cheaply (`git ls-remote`), or resolve the
/// local base for a local-only queue. `Ok(None)` means unknown this cycle.
pub fn remote_base_sha(git: &Git, config: &Config) -> Result<Option<String>, String> {
    if config.remote.is_empty() {
        let rev = if config.base.starts_with("refs/") {
            config.base.clone()
        } else {
            format!("refs/heads/{}", config.base)
        };
        return Ok(git.try_rev_parse(&rev));
    }
    let refname = if config.base.starts_with("refs/") {
        config.base.clone()
    } else {
        format!("refs/heads/{}", config.base)
    };
    let out = git.run_net(&["ls-remote", &config.remote, &refname])?;
    let sha = out
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(sha)
}

/// Options for one `serve` invocation.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// Idle poll interval (items fingerprint + heartbeat cadence).
    pub poll: Duration,
    /// Base poll interval (`git ls-remote` cadence).
    pub base_poll: Duration,
    /// Run at most one work cycle, then exit (scripts and tests).
    pub once: bool,
    /// Land on a simulated base (same shadow path as `run --shadow`):
    /// full plan/run/notify cycles execute and landings are recorded as
    /// shadow/simulated, but the real base ref is never pushed and item
    /// statuses stay queued. Daemon soak mode.
    pub shadow: bool,
    /// Override the configured depth for work cycles.
    pub depth: Option<usize>,
    /// Override the configured maximum batch size for work cycles.
    pub max_batch: Option<usize>,
    /// Override the configured agentctl command for push notifications.
    pub agentctl: Option<Vec<String>>,
    /// Deadline for one notification send; a send still running then has
    /// its process group killed and counts as a failed attempt.
    pub notify_timeout: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(DEFAULT_POLL_MS),
            base_poll: Duration::from_secs(DEFAULT_BASE_POLL_SECS),
            once: false,
            shadow: false,
            depth: None,
            max_batch: None,
            agentctl: None,
            notify_timeout: notify::DEFAULT_SEND_TIMEOUT,
        }
    }
}

fn write_pid(state: &State) -> Result<(), String> {
    std::fs::create_dir_all(&state.root)
        .map_err(|e| format!("cannot create {}: {e}", state.root.display()))?;
    std::fs::write(state.serve_pid_path(), format!("{}\n", std::process::id()))
        .map_err(|e| format!("cannot write {}: {e}", state.serve_pid_path().display()))
}

fn write_heartbeat(state: &State, heartbeat: &Heartbeat) -> Result<(), String> {
    write_json(&state.serve_heartbeat_path(), heartbeat)
}

/// Removes `serve.pid` on drop, but only while it still names this process:
/// the pid file is informational and never another daemon's to delete.
struct PidFile<'a>(&'a State);

impl Drop for PidFile<'_> {
    fn drop(&mut self) {
        if read_pid(self.0) == Some(std::process::id()) {
            let _ = std::fs::remove_file(self.0.serve_pid_path());
        }
    }
}

/// Sets a flag on drop, so a panicking work cycle still stops the
/// heartbeat helper thread (a scoped thread that never stops would hang).
struct SetOnDrop<'a>(&'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Queue facts the idle loop needs from one read of the item records.
#[derive(Debug, Default)]
struct RetrySchedule {
    /// Queued changes.
    queued: usize,
    /// Newest refusal deferral of a queued change that has already passed.
    due: Option<String>,
    /// Earliest refusal deferral of a queued change still in the future.
    next: Option<String>,
}

impl RetrySchedule {
    fn of(items: &[Item], at: &str) -> Self {
        let mut schedule = Self::default();
        for item in items.iter().filter(|i| i.status == ItemStatus::Queued) {
            schedule.queued += 1;
            let retry = item.next_retry_at.as_str();
            if retry.is_empty() {
                continue;
            }
            if retry <= at {
                if schedule.due.as_deref().is_none_or(|due| retry > due) {
                    schedule.due = Some(retry.to_owned());
                }
            } else if schedule.next.as_deref().is_none_or(|next| retry < next) {
                schedule.next = Some(retry.to_owned());
            }
        }
        schedule
    }
}

fn lock_heartbeat(shared: &Mutex<Heartbeat>) -> std::sync::MutexGuard<'_, Heartbeat> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keep the heartbeat fresh until `done`. Writes happen under the mutex, so
/// they never interleave with the work cycle's own heartbeat writes (which
/// share one temporary file name per process).
fn refresh_heartbeat(state: &State, shared: &Mutex<Heartbeat>, done: &AtomicBool) {
    let mut last = Instant::now();
    while !done.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
        if last.elapsed() >= HEARTBEAT_REFRESH && !done.load(Ordering::SeqCst) {
            last = Instant::now();
            let mut heartbeat = lock_heartbeat(shared);
            heartbeat.touch();
            let _ = write_heartbeat(state, &heartbeat);
        }
    }
}

/// One plan+run cycle (the caller holds the writer lock). Returns whether it
/// was clean; a failure is recorded in the heartbeat's `last_error`.
fn run_cycle(
    state: &State,
    config: &Config,
    options: &ServeOptions,
    shared: &Mutex<Heartbeat>,
) -> bool {
    // Show the planned window in the heartbeat; the Runner replans
    // internally, this is a status peek only.
    if let Ok(plan) = ops::plan(state, config, options.max_batch.unwrap_or(config.max_batch)) {
        let mut heartbeat = lock_heartbeat(shared);
        heartbeat.active_batches = plan.batches.iter().map(|b| b.prs.clone()).collect();
        heartbeat.touch();
        let _ = write_heartbeat(state, &heartbeat);
    }
    let run_options = RunOptions {
        shadow: options.shadow,
        depth: options.depth,
        max_batch: options.max_batch,
        quiet: true,
        ..RunOptions::default()
    };
    let outcome = Runner::new(state, run_options)
        .map_err(|error| format!("run refused: {error}"))
        .and_then(|runner| runner.run().map_err(|error| format!("run failed: {error}")));
    let mut heartbeat = lock_heartbeat(shared);
    match outcome {
        Ok(report) => {
            heartbeat.last_run = report.run.clone();
            heartbeat.last_run_at = now();
            eprintln!(
                "merge-queue serve: {} run {} {} {} batch(es), ejected {}, stopped: {}",
                if options.shadow { "shadow" } else { "live" },
                report.run,
                if options.shadow {
                    "simulated"
                } else {
                    "landed"
                },
                report.landed.len(),
                report.ejected.len(),
                if report.stopped.is_empty() {
                    "-"
                } else {
                    report.stopped.as_str()
                }
            );
            if report.stopped.is_empty() {
                heartbeat.last_error.clear();
                true
            } else {
                heartbeat.last_error = format!("run stopped: {}", report.stopped);
                false
            }
        }
        Err(error) => {
            eprintln!("merge-queue serve: {error}");
            heartbeat.last_error = error;
            false
        }
    }
}

/// Run one work cycle with the heartbeat kept fresh by a helper thread.
fn work_cycle(
    state: &State,
    config: &Config,
    options: &ServeOptions,
    heartbeat: &mut Heartbeat,
) -> bool {
    heartbeat.state = "working".to_owned();
    heartbeat.working_since = now();
    heartbeat.working_since_unix = unix_now();
    heartbeat.touch();
    let _ = write_heartbeat(state, heartbeat);
    let shared = Mutex::new(heartbeat.clone());
    let done = AtomicBool::new(false);
    let clean = std::thread::scope(|scope| {
        scope.spawn(|| refresh_heartbeat(state, &shared, &done));
        let _stop = SetOnDrop(&done);
        run_cycle(state, config, options, &shared)
    });
    *heartbeat = shared.into_inner().unwrap_or_else(PoisonError::into_inner);
    heartbeat.state = "idle".to_owned();
    heartbeat.working_since.clear();
    heartbeat.working_since_unix = 0;
    heartbeat.active_batches.clear();
    clean
}

/// Deliver owed notifications under the writer lock the caller holds.
fn sweep_notifications(
    state: &State,
    lock: &crate::state::Lock,
    config: &Config,
    notifier: &notify::Notifier,
) {
    // Best-effort by design: a failed send is logged and retried on a later
    // cycle, never fatal here.
    match notify::sweep_locked(state, lock, config, notifier) {
        Ok(sent) if sent > 0 => {
            eprintln!("merge-queue serve: sent {sent} notification(s)");
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("merge-queue serve: notification sweep failed: {error}");
        }
    }
}

fn trigger_label(items: bool, base: bool, retry: bool, backoff: bool) -> String {
    let parts: Vec<&str> = [
        (items, "items"),
        (base, "base"),
        (retry, "retry"),
        (backoff, "backoff"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| *name)
    .collect();
    parts.join("+")
}

fn items_error_message(error: &str) -> String {
    format!(
        "cannot read the queue's item records: {error}; remedy: repair or remove the named file \
         (the daemon plans nothing until every record reads)"
    )
}

/// Run the daemon until SIGINT/SIGTERM (or one cycle with `--once`).
///
/// Returns the process exit code: 0 on a graceful stop, 1 when the daemon
/// cannot start (no queue, or another live daemon already holds this state
/// directory's `serve.lock`). Work-cycle errors are recorded in the
/// heartbeat and retried with backoff; they never kill the daemon.
///
/// SIGINT/SIGTERM are honoured between cycles: a work cycle in progress
/// runs to completion first (the Runner has no cancellation hook).
pub fn serve(state: &State, options: &ServeOptions) -> Result<i32, String> {
    let config = state.config().map_err(|e| {
        format!(
            "{e}; remedy: run `merge-queue init` for this repository before `merge-queue serve`"
        )
    })?;
    // Held (and so the single-daemon guarantee kept) until this returns.
    let _serve_lock = acquire_serve_lock(state)?;
    reset_shutdown();
    install_signal_handlers();
    write_pid(state)?;
    let pid_file = PidFile(state);
    let git = ops::repo(state, &config)?;
    let window = options.depth.unwrap_or(config.depth).max(1);
    let mut heartbeat = Heartbeat::new(std::process::id(), window, options.shadow);
    match state.items() {
        Ok(items) => heartbeat.queued = RetrySchedule::of(&items, &now()).queued,
        Err(error) => heartbeat.last_error = items_error_message(&error),
    }
    let (total, in_use) = slot_occupancy(state);
    heartbeat.slots_total = total;
    heartbeat.slots_in_use = in_use;
    write_heartbeat(state, &heartbeat)?;
    eprintln!(
        "merge-queue serve: pid {} watching {} (poll {}ms, base poll {}s){}; Ctrl-C to stop",
        heartbeat.pid,
        state.root.display(),
        options.poll.as_millis(),
        options.base_poll.as_secs(),
        if options.shadow {
            " (shadow: landings simulated, real base unchanged)"
        } else {
            ""
        }
    );
    let notifier = notify::Notifier::resolve(options.agentctl.as_deref(), &config)
        .with_timeout(options.notify_timeout);

    let mut last_fingerprint = String::new();
    let mut last_base: Option<String> = None;
    let mut last_base_poll: Option<Instant> = None;
    // Newest refusal deferral a work cycle has already acted on.
    let mut retry_seen = String::new();
    // Backoff retry after failed work cycles: when, and as RFC 3339 text.
    let mut failure_retry: Option<(Instant, String)> = None;

    'outer: loop {
        if shutdown_requested() {
            break 'outer;
        }
        heartbeat.cycles += 1;
        let fingerprint = items_fingerprint(state);
        let items_changed = fingerprint != last_fingerprint;

        let base_due = last_base_poll.is_none_or(|at| at.elapsed() >= options.base_poll);
        let mut base_changed = false;
        if base_due {
            last_base_poll = Some(Instant::now());
            match remote_base_sha(&git, &config) {
                Ok(sha) => {
                    if let Some(sha) = sha {
                        if last_base.as_deref() != Some(sha.as_str()) {
                            // The first successful poll only sets the baseline;
                            // startup work is driven by the items fingerprint.
                            base_changed = last_base.is_some();
                            heartbeat.last_base_sha = sha.clone();
                            last_base = Some(sha);
                        }
                    }
                }
                Err(error) => {
                    heartbeat.last_error = format!("base poll failed: {error}");
                }
            }
        }

        let (total, in_use) = slot_occupancy(state);
        heartbeat.slots_total = total;
        heartbeat.slots_in_use = in_use;

        let mut swept = false;
        match state.items() {
            Err(error) => {
                // Never read an unreadable queue as an empty one: surface it
                // (heartbeat and `status`) and plan nothing until it reads.
                let message = items_error_message(&error);
                if heartbeat.last_error != message {
                    eprintln!("merge-queue serve: {message}");
                }
                heartbeat.last_error = message;
                heartbeat.last_trigger = "error".to_owned();
                heartbeat.state = "idle".to_owned();
                heartbeat.active_batches.clear();
            }
            Ok(items) => {
                if heartbeat
                    .last_error
                    .starts_with("cannot read the queue's item records")
                {
                    heartbeat.last_error.clear();
                }
                let schedule = RetrySchedule::of(&items, &now());
                heartbeat.queued = schedule.queued;
                let retry_due = schedule
                    .due
                    .as_deref()
                    .is_some_and(|due| due > retry_seen.as_str());
                let backoff_due = failure_retry
                    .as_ref()
                    .is_some_and(|(at, _)| Instant::now() >= *at);
                if items_changed || base_changed || retry_due || backoff_due {
                    heartbeat.last_trigger =
                        trigger_label(items_changed, base_changed, retry_due, backoff_due);
                    if heartbeat.queued == 0 {
                        // Nothing to land: remember the state and stay idle.
                        last_fingerprint = fingerprint.clone();
                        failure_retry = None;
                        heartbeat.failed_cycles = 0;
                        heartbeat.state = "idle".to_owned();
                        heartbeat.active_batches.clear();
                    } else {
                        match state.lock() {
                            Ok(lock) => {
                                if let Some(due) = schedule.due.as_ref() {
                                    if *due > retry_seen {
                                        retry_seen.clone_from(due);
                                    }
                                }
                                if work_cycle(state, &config, options, &mut heartbeat) {
                                    heartbeat.failed_cycles = 0;
                                    failure_retry = None;
                                } else {
                                    heartbeat.failed_cycles =
                                        heartbeat.failed_cycles.saturating_add(1);
                                    let delay = failure_backoff(heartbeat.failed_cycles);
                                    let at = chrono::Utc::now()
                                        + chrono::Duration::from_std(delay).unwrap_or_default();
                                    failure_retry = Some((
                                        Instant::now() + delay,
                                        at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                                    ));
                                    eprintln!(
                                        "merge-queue serve: work cycle failed ({} in a row); retrying in {}s",
                                        heartbeat.failed_cycles,
                                        delay.as_secs()
                                    );
                                }
                                // Notifications under the same lock, before
                                // the new baseline is taken.
                                sweep_notifications(state, &lock, &config, &notifier);
                                swept = true;
                                // Whatever the cycle did, the resulting items
                                // state is the new baseline: never spin on the
                                // same fingerprint.
                                last_fingerprint = items_fingerprint(state);
                            }
                            Err(error) => {
                                heartbeat.last_trigger = "lock-busy".to_owned();
                                heartbeat.last_error = error;
                                // Do not update last_fingerprint: retry next cycle.
                            }
                        }
                    }
                } else {
                    heartbeat.state = "idle".to_owned();
                }
                // The earliest time-based retry the daemon will act on.
                let mut next = schedule.next;
                if let Some((_, at)) = &failure_retry {
                    if next.as_deref().is_none_or(|n| at.as_str() < n) {
                        next = Some(at.clone());
                    }
                }
                heartbeat.next_retry_at = next.unwrap_or_default();
            }
        }

        // Push notifications for whatever a CLI command changed since the
        // last cycle. The writer lock is taken only when something is owed
        // (a lock-free check), and a busy lock skips this cycle's sweep.
        if !swept && notify::pending(state).is_ok_and(|owed| owed > 0) {
            if let Ok(lock) = state.lock() {
                let before = items_fingerprint(state);
                sweep_notifications(state, &lock, &config, &notifier);
                // The sweep's own delivery records are not new work.
                if before == last_fingerprint {
                    last_fingerprint = items_fingerprint(state);
                }
            }
        }

        heartbeat.touch();
        if let Err(error) = write_heartbeat(state, &heartbeat) {
            eprintln!("merge-queue serve: cannot write heartbeat: {error}");
        }

        if options.once {
            break 'outer;
        }
        // Cheap deterministic idle: fixed slices, no lock held, shutdown-aware.
        let mut slept = Duration::ZERO;
        while slept < options.poll {
            if shutdown_requested() {
                break 'outer;
            }
            let slice = Duration::from_millis(50).min(options.poll - slept);
            std::thread::sleep(slice);
            slept += slice;
        }
    }

    heartbeat.state = "stopped".to_owned();
    heartbeat.touch();
    let _ = write_heartbeat(state, &heartbeat);
    // A graceful stop removes our pid file so `status` says "not running"
    // instead of "stale". A crash leaves both files behind (and frees
    // `serve.lock`), which `status` reports truthfully as dead.
    drop(pid_file);
    eprintln!(
        "merge-queue serve: stopped after {} cycle(s)",
        heartbeat.cycles
    );
    Ok(0)
}

/// Daemon liveness as `status` reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Liveness {
    /// Pid alive and heartbeat fresh.
    Running,
    /// No pid file: the daemon is not running (or stopped gracefully).
    NotRunning,
    /// Pid file exists but no daemon is behind it: the process is dead, or
    /// `serve.lock` is free (the pid now belongs to an unrelated process).
    Dead,
    /// Pid alive but the heartbeat is older than the stale threshold.
    Stale,
    /// Pid/heartbeat files exist but cannot be parsed.
    Malformed,
}

impl Liveness {
    /// Stable machine-facing value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::NotRunning => "not-running",
            Self::Dead => "dead",
            Self::Stale => "stale",
            Self::Malformed => "malformed",
        }
    }

    /// Whether the daemon is healthy.
    pub const fn healthy(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Everything `status` reports, gathered without taking the writer lock.
#[derive(Clone, Debug, Serialize)]
pub struct StatusReport {
    /// State directory inspected.
    pub state_dir: PathBuf,
    /// Daemon liveness verdict.
    pub daemon: Liveness,
    /// Pid from the pid file, when parseable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Heartbeat record, when parseable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<Heartbeat>,
    /// Heartbeat age in seconds at inspection time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_age_secs: Option<u64>,
    /// Stale threshold used for the verdict.
    pub stale_after_secs: u64,
    /// Items by status.
    pub queued: usize,
    /// Landed items.
    pub landed: usize,
    /// Ejected items.
    pub ejected: usize,
    /// Withdrawn items.
    pub withdrawn: usize,
    /// Current window: batches from the heartbeat (daemon view), falling
    /// back to the newest run's plan when no daemon heartbeat exists.
    pub active_batches: Vec<Vec<i64>>,
    /// Scratch slots that exist.
    pub slots_total: usize,
    /// Scratch slots holding a checkout.
    pub slots_in_use: usize,
    /// Whether the daemon is in shadow mode (`serve --shadow`): landings
    /// are simulated on a private base and the real base is never pushed.
    /// Derived from the heartbeat; `false` when no daemon heartbeat exists.
    pub shadow: bool,
    /// Seconds the daemon has been in its current work cycle, when it is in
    /// one. Reported separately from liveness: a long cycle with a fresh
    /// heartbeat is healthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_secs: Option<u64>,
    /// Why the item records cannot be read, when they cannot. The queue is
    /// then unhealthy whatever the daemon's liveness: nothing can be planned.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub items_error: String,
    /// Human remedy when the daemon or the queue is not healthy.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remedy: String,
}

impl StatusReport {
    /// Whether the daemon is running and the queue it serves is readable.
    pub fn healthy(&self) -> bool {
        self.daemon.healthy() && self.items_error.is_empty()
    }
}

/// Decide liveness from pid + heartbeat without touching the queue lock.
pub fn liveness(
    state: &State,
    stale_after_secs: u64,
) -> (Liveness, Option<u32>, Option<Heartbeat>, Option<u64>) {
    let pid_path = state.serve_pid_path();
    let lock_held = serve_lock_held(state);
    let pid = if pid_path.is_file() {
        let Some(pid) = read_pid(state) else {
            return (Liveness::Malformed, None, read_heartbeat(state), None);
        };
        pid
    } else {
        match (lock_held, read_heartbeat(state)) {
            // A daemon holds the lock but its pid file is gone (removed by
            // hand): judge it by its heartbeat instead.
            (Some(true), Some(heartbeat)) if heartbeat.pid > 0 => heartbeat.pid,
            // A heartbeat alone (graceful stop leaves one) is still not running.
            (_, heartbeat) => return (Liveness::NotRunning, None, heartbeat, None),
        }
    };
    let heartbeat = read_heartbeat(state);
    // A free `serve.lock` means no daemon is running here, whatever process
    // holds the recorded pid now (pid reuse after a crash).
    if !pid_alive(pid) || lock_held == Some(false) {
        let age = heartbeat
            .as_ref()
            .map(|h| unix_now().saturating_sub(h.heartbeat_unix));
        return (Liveness::Dead, Some(pid), heartbeat, age);
    }
    let Some(heartbeat) = heartbeat else {
        // Live pid with no heartbeat yet: just started, or the file was
        // removed. Treat a brand-new pid file as running only briefly;
        // without any heartbeat we cannot prove freshness, so call it stale.
        return (Liveness::Stale, Some(pid), None, None);
    };
    let age = unix_now().saturating_sub(heartbeat.heartbeat_unix);
    if age > stale_after_secs {
        (Liveness::Stale, Some(pid), Some(heartbeat), Some(age))
    } else {
        (Liveness::Running, Some(pid), Some(heartbeat), Some(age))
    }
}

/// Gather the full status report.
pub fn collect_status(state: &State, stale_after_secs: u64) -> Result<StatusReport, String> {
    let (daemon, pid, heartbeat, heartbeat_age_secs) = liveness(state, stale_after_secs);
    let (items, items_error) = match state.items() {
        Ok(items) => (items, String::new()),
        Err(error) => (Vec::new(), error),
    };
    let count = |status: ItemStatus| items.iter().filter(|i| i.status == status).count();
    let (slots_total, slots_in_use) = slot_occupancy(state);
    let active_batches = match &heartbeat {
        Some(h) if !h.active_batches.is_empty() => h.active_batches.clone(),
        _ => newest_run_batches(state),
    };
    let working_secs = heartbeat
        .as_ref()
        .filter(|h| h.state == "working" && h.working_since_unix > 0)
        .map(|h| unix_now().saturating_sub(h.working_since_unix));
    let remedy = match daemon {
        Liveness::Running if !items_error.is_empty() => format!(
            "the daemon is running but cannot plan: an item record is unreadable ({items_error}); \
             remedy: repair or remove that file"
        ),
        Liveness::Running => String::new(),
        Liveness::NotRunning => {
            "start the daemon with `merge-queue serve` (or `systemctl --user start merge-queue-serve`)"
                .to_owned()
        }
        Liveness::Dead => format!(
            "remove the stale pid file {} and start the daemon with `merge-queue serve`",
            state.serve_pid_path().display()
        ),
        // Only reachable when even the in-cycle heartbeat thread has gone
        // quiet: a working daemon with a fresh heartbeat is `Running`.
        Liveness::Stale => format!(
            "the daemon (pid {}) is alive but has not heartbeat in {}s{}; \
             remedy: check it with `merge-queue status`, restart it with `kill {}` then `merge-queue serve`",
            pid.unwrap_or(0),
            heartbeat_age_secs.unwrap_or(0),
            working_secs
                .map(|secs| format!(" (in a work cycle for {secs}s)"))
                .unwrap_or_default(),
            pid.unwrap_or(0)
        ),
        Liveness::Malformed => format!(
            "the pid file {} is unreadable; remedy: remove it and start the daemon with `merge-queue serve`",
            state.serve_pid_path().display()
        ),
    };
    let shadow = heartbeat.as_ref().is_some_and(|h| h.shadow);
    Ok(StatusReport {
        state_dir: state.root.clone(),
        daemon,
        pid,
        heartbeat,
        heartbeat_age_secs,
        stale_after_secs,
        queued: count(ItemStatus::Queued),
        landed: count(ItemStatus::Landed),
        ejected: count(ItemStatus::Ejected),
        withdrawn: count(ItemStatus::Withdrawn),
        active_batches,
        slots_total,
        slots_in_use,
        shadow,
        working_secs,
        items_error,
        remedy,
    })
}

fn newest_run_batches(state: &State) -> Vec<Vec<i64>> {
    let Ok(entries) = std::fs::read_dir(state.runs_dir()) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    let Some(latest) = dirs.last() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(latest.join("report.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value["plan"]["batches"]
        .as_array()
        .map(|batches| {
            batches
                .iter()
                .filter_map(|b| {
                    b["prs"].as_array().map(|prs| {
                        prs.iter()
                            .filter_map(serde_json::Value::as_i64)
                            .collect::<Vec<_>>()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Render the status report as human text.
pub fn render_status(report: &StatusReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("state: {}\n", report.state_dir.display()));
    match report.daemon {
        Liveness::Running => {
            let hb = report.heartbeat.as_ref();
            out.push_str(&format!(
                "daemon: running (pid {}, heartbeat {}s ago, started {})\n",
                report.pid.unwrap_or(0),
                report.heartbeat_age_secs.unwrap_or(0),
                hb.map_or("-", |h| h.started_at.as_str())
            ));
            if let Some(h) = hb {
                let state = match report.working_secs {
                    Some(secs) => format!("working for {secs}s"),
                    None => h.state.clone(),
                };
                out.push_str(&format!(
                    "  state {} · cycles {} · last trigger {} · window {} · last run {}\n",
                    state,
                    h.cycles,
                    h.last_trigger,
                    h.window,
                    if h.last_run.is_empty() {
                        "-"
                    } else {
                        h.last_run.as_str()
                    }
                ));
                out.push_str(if h.shadow {
                    "  mode: shadow (landings simulated; real base unchanged)\n"
                } else {
                    "  mode: live\n"
                });
                if !h.next_retry_at.is_empty() {
                    out.push_str(&format!("  next retry: {}\n", h.next_retry_at));
                }
                if !h.last_error.is_empty() {
                    out.push_str(&format!("  last error: {}\n", h.last_error));
                }
            }
        }
        Liveness::NotRunning => {
            out.push_str("daemon: not running\n");
        }
        Liveness::Dead => {
            out.push_str(&format!(
                "daemon: dead (pid {} is not a live daemon{})\n",
                report.pid.unwrap_or(0),
                report
                    .heartbeat_age_secs
                    .map(|age| format!(", heartbeat {age}s ago"))
                    .unwrap_or_default()
            ));
        }
        Liveness::Stale => {
            out.push_str(&format!(
                "daemon: stale (pid {} alive, heartbeat {}s ago, threshold {}s)\n",
                report.pid.unwrap_or(0),
                report.heartbeat_age_secs.unwrap_or(0),
                report.stale_after_secs
            ));
        }
        Liveness::Malformed => {
            out.push_str("daemon: malformed (pid file unreadable)\n");
        }
    }
    if !report.remedy.is_empty() {
        out.push_str(&format!("  remedy: {}\n", report.remedy));
    }
    if report.items_error.is_empty() {
        out.push_str(&format!(
            "queue: {} queued, {} landed, {} ejected, {} withdrawn\n",
            report.queued, report.landed, report.ejected, report.withdrawn
        ));
    } else {
        out.push_str(&format!(
            "queue: UNREADABLE ({}); counts unknown\n",
            report.items_error
        ));
    }
    if report.active_batches.is_empty() {
        out.push_str("window: no active batches\n");
    } else {
        let batches: Vec<String> = report
            .active_batches
            .iter()
            .map(|prs| {
                prs.iter()
                    .map(|pr| format!("#{pr}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();
        out.push_str(&format!(
            "window: {} active batch(es): {}\n",
            batches.len(),
            batches.join(" | ")
        ));
    }
    out.push_str(&format!(
        "slots: {} in use / {} total\n",
        report.slots_in_use, report.slots_total
    ));
    out
}

#[cfg(test)]
mod unit {
    use super::*;

    fn temp_state(tag: &str) -> State {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "merge-queue-serve-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("items")).unwrap();
        State::at(root)
    }

    #[test]
    fn fingerprint_changes_with_items() {
        let state = temp_state("fp");
        let empty = items_fingerprint(&state);
        std::fs::write(state.items_dir().join("1.json"), "{\"pr\":1}").unwrap();
        let one = items_fingerprint(&state);
        assert_ne!(empty, one);
        std::fs::write(state.items_dir().join("1.json"), "{\"pr\":1,\"x\":2}").unwrap();
        assert_ne!(one, items_fingerprint(&state));
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn liveness_covers_dead_and_stale() {
        let state = temp_state("live");
        // No pid file: not running.
        assert_eq!(liveness(&state, 10).0, Liveness::NotRunning);
        // Dead pid: write an implausible pid with an old heartbeat. Pid
        // 4194304+ is above the usual pid_max on test hosts; if it happens
        // to be alive the assertion below still holds via the age path,
        // so use the heartbeat-age branch for stale instead.
        std::fs::write(state.serve_pid_path(), "999999999\n").unwrap();
        let mut heartbeat = Heartbeat::new(999_999_999, 2, false);
        heartbeat.heartbeat_unix = unix_now().saturating_sub(3600);
        heartbeat.touch();
        heartbeat.heartbeat_unix = unix_now().saturating_sub(3600);
        heartbeat.heartbeat_at = "2000-01-01T00:00:00Z".to_owned();
        write_heartbeat(&state, &heartbeat).unwrap();
        let verdict = liveness(&state, 10).0;
        assert!(
            matches!(verdict, Liveness::Dead | Liveness::Stale),
            "{verdict:?}"
        );
        // Live pid (this process) with a fresh heartbeat: running.
        std::fs::write(state.serve_pid_path(), format!("{}\n", std::process::id())).unwrap();
        let mut fresh = Heartbeat::new(std::process::id(), 2, false);
        fresh.touch();
        write_heartbeat(&state, &fresh).unwrap();
        assert_eq!(liveness(&state, 10).0, Liveness::Running);
        // Same live pid, old heartbeat: stale.
        fresh.heartbeat_unix = unix_now().saturating_sub(3600);
        write_heartbeat(&state, &fresh).unwrap();
        assert_eq!(liveness(&state, 10).0, Liveness::Stale);
        // Malformed pid file.
        std::fs::write(state.serve_pid_path(), "not-a-pid\n").unwrap();
        assert_eq!(liveness(&state, 10).0, Liveness::Malformed);
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn heartbeat_and_status_report_shadow_truthfully() {
        let state = temp_state("shadow");
        std::fs::write(state.serve_pid_path(), format!("{}\n", std::process::id())).unwrap();
        let mut heartbeat = Heartbeat::new(std::process::id(), 2, true);
        heartbeat.touch();
        write_heartbeat(&state, &heartbeat).unwrap();
        let report = collect_status(&state, 10).unwrap();
        assert!(report.shadow, "{report:?}");
        assert_eq!(report.heartbeat.as_ref().map(|h| h.shadow), Some(true));
        let text = render_status(&report);
        assert!(text.contains("mode: shadow"), "{text}");
        assert!(text.contains("simulated"), "{text}");
        // A live (non-shadow) heartbeat renders as live, never shadow.
        let mut live = Heartbeat::new(std::process::id(), 2, false);
        live.touch();
        write_heartbeat(&state, &live).unwrap();
        let report = collect_status(&state, 10).unwrap();
        assert!(!report.shadow);
        assert!(render_status(&report).contains("mode: live"));
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn old_heartbeat_without_shadow_field_reads_as_live() {
        // Heartbeats written before `serve --shadow` have no `shadow`
        // field; they must deserialize as live (false), never shadow.
        let json = r#"{"pid":1,"started_at":"2026-01-01T00:00:00Z","heartbeat_at":"2026-01-01T00:00:00Z","heartbeat_unix":1,"cycles":0,"state":"idle","last_trigger":"startup","last_base_sha":"","queued":0,"active_batches":[],"window":2,"slots_total":0,"slots_in_use":0}"#;
        let heartbeat: Heartbeat = serde_json::from_str(json).unwrap();
        assert!(!heartbeat.shadow);
    }
}
