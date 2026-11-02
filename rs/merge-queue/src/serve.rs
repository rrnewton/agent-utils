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
//! * Working: the lock is acquired for one plan+run cycle (the existing
//!   [`crate::run::Runner`] path; the logic is not forked), then released
//!   before the daemon goes back to idle polling.
//!
//! Liveness is two files in the state directory: `serve.pid` (the daemon
//! pid) and `serve.heartbeat.json` (rewritten every cycle). `status` reads
//! them plus the item records and the scratch slots, and never claims a
//! daemon is running when the pid is dead or the heartbeat is stale.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::model::{now, ItemStatus};
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
}

impl Heartbeat {
    fn new(pid: u32, window: usize) -> Self {
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
    /// Override the configured depth for work cycles.
    pub depth: Option<usize>,
    /// Override the configured maximum batch size for work cycles.
    pub max_batch: Option<usize>,
    /// Override the configured agentctl command for push notifications.
    pub agentctl: Option<Vec<String>>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(DEFAULT_POLL_MS),
            base_poll: Duration::from_secs(DEFAULT_BASE_POLL_SECS),
            once: false,
            depth: None,
            max_batch: None,
            agentctl: None,
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

fn queued_count(state: &State) -> usize {
    state
        .items()
        .map(|items| {
            items
                .iter()
                .filter(|i| i.status == ItemStatus::Queued)
                .count()
        })
        .unwrap_or(0)
}

/// Run the daemon until SIGINT/SIGTERM (or one cycle with `--once`).
///
/// Returns the process exit code: 0 on a graceful stop, 1 when the daemon
/// cannot start (no queue, or another live daemon already owns this state
/// directory). Work-cycle errors are recorded in the heartbeat and retried
/// on the next trigger; they never kill the daemon.
pub fn serve(state: &State, options: &ServeOptions) -> Result<i32, String> {
    let config = state.config().map_err(|e| {
        format!(
            "{e}; remedy: run `merge-queue init` for this repository before `merge-queue serve`"
        )
    })?;
    if let Some(existing) = read_pid(state) {
        if existing != std::process::id() && pid_alive(existing) {
            let hb = read_heartbeat(state)
                .map(|h| format!(", heartbeat {}", h.heartbeat_at))
                .unwrap_or_default();
            return Err(format!(
                "merge-queue serve is already running (pid {existing}{hb}); \
                 remedy: stop it with `kill {existing}` or check it with `merge-queue status`"
            ));
        }
    }
    reset_shutdown();
    install_signal_handlers();
    write_pid(state)?;
    let git = ops::repo(state, &config)?;
    let window = options.depth.unwrap_or(config.depth).max(1);
    let mut heartbeat = Heartbeat::new(std::process::id(), window);
    heartbeat.queued = queued_count(state);
    let (total, in_use) = slot_occupancy(state);
    heartbeat.slots_total = total;
    heartbeat.slots_in_use = in_use;
    write_heartbeat(state, &heartbeat)?;
    eprintln!(
        "merge-queue serve: pid {} watching {} (poll {}ms, base poll {}s); Ctrl-C to stop",
        heartbeat.pid,
        state.root.display(),
        options.poll.as_millis(),
        options.base_poll.as_secs()
    );

    let mut last_fingerprint = String::new();
    let mut last_base: Option<String> = None;
    let mut last_base_poll: Option<Instant> = None;
    let result = Ok(0);

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

        let triggered = items_changed || base_changed;
        heartbeat.queued = queued_count(state);
        let (total, in_use) = slot_occupancy(state);
        heartbeat.slots_total = total;
        heartbeat.slots_in_use = in_use;

        if triggered {
            heartbeat.last_trigger = match (items_changed, base_changed) {
                (true, true) => "items+base".to_owned(),
                (true, false) => "items".to_owned(),
                (false, true) => "base".to_owned(),
                (false, false) => heartbeat.last_trigger.clone(),
            };
            if heartbeat.queued == 0 {
                // Nothing to land: remember the state and stay idle.
                last_fingerprint = fingerprint.clone();
                heartbeat.state = "idle".to_owned();
                heartbeat.active_batches.clear();
            } else {
                match state.lock() {
                    Ok(_lock) => {
                        heartbeat.state = "working".to_owned();
                        heartbeat.touch();
                        let _ = write_heartbeat(state, &heartbeat);
                        // Show the planned window in the heartbeat; the Runner
                        // replans internally, this is a status peek only.
                        if let Ok(plan) = ops::plan(
                            state,
                            &config,
                            options.max_batch.unwrap_or(config.max_batch),
                        ) {
                            heartbeat.active_batches =
                                plan.batches.iter().map(|b| b.prs.clone()).collect();
                            heartbeat.touch();
                            let _ = write_heartbeat(state, &heartbeat);
                        }
                        let run_options = RunOptions {
                            depth: options.depth,
                            max_batch: options.max_batch,
                            quiet: true,
                            ..RunOptions::default()
                        };
                        match Runner::new(state, run_options) {
                            Ok(runner) => match runner.run() {
                                Ok(report) => {
                                    heartbeat.last_run = report.run.clone();
                                    heartbeat.last_run_at = now();
                                    if report.stopped.is_empty() {
                                        heartbeat.last_error.clear();
                                    } else {
                                        heartbeat.last_error =
                                            format!("run stopped: {}", report.stopped);
                                    }
                                    eprintln!(
                                        "merge-queue serve: run {} landed {} batch(es), ejected {}, stopped: {}",
                                        report.run,
                                        report.landed.len(),
                                        report.ejected.len(),
                                        if report.stopped.is_empty() {
                                            "-"
                                        } else {
                                            report.stopped.as_str()
                                        }
                                    );
                                }
                                Err(error) => {
                                    heartbeat.last_error = format!("run failed: {error}");
                                    eprintln!("merge-queue serve: run failed: {error}");
                                }
                            },
                            Err(error) => {
                                heartbeat.last_error = format!("run refused: {error}");
                                eprintln!("merge-queue serve: run refused: {error}");
                            }
                        }
                        heartbeat.state = "idle".to_owned();
                        heartbeat.active_batches.clear();
                        // Whatever the cycle did, the resulting items state is
                        // the new baseline: never spin on the same fingerprint.
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

        // Push notifications for whatever this cycle (or a CLI command
        // since the last cycle) changed. Best-effort by design: a failed
        // send is logged and retried on a later cycle, never fatal here.
        let notifier = notify::Notifier::resolve(options.agentctl.as_deref(), &config);
        match notify::sweep(state, &config, &notifier) {
            Ok(sent) if sent > 0 => {
                eprintln!("merge-queue serve: sent {sent} notification(s)");
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("merge-queue serve: notification sweep failed: {error}");
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
    // A graceful stop removes the pid file so `status` says "not running"
    // instead of "stale". A crash leaves both files behind, which `status`
    // reports truthfully as dead/stale with a remedy.
    let _ = std::fs::remove_file(state.serve_pid_path());
    eprintln!(
        "merge-queue serve: stopped after {} cycle(s)",
        heartbeat.cycles
    );
    result
}

/// Daemon liveness as `status` reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Liveness {
    /// Pid alive and heartbeat fresh.
    Running,
    /// No pid file: the daemon is not running (or stopped gracefully).
    NotRunning,
    /// Pid file exists but the process is dead.
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
    /// Human remedy when the daemon is not healthy.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remedy: String,
}

/// Decide liveness from pid + heartbeat without touching the queue lock.
pub fn liveness(
    state: &State,
    stale_after_secs: u64,
) -> (Liveness, Option<u32>, Option<Heartbeat>, Option<u64>) {
    let pid_path = state.serve_pid_path();
    if !pid_path.is_file() {
        // A heartbeat alone (graceful stop leaves one) is still not running.
        return (Liveness::NotRunning, None, read_heartbeat(state), None);
    }
    let Some(pid) = read_pid(state) else {
        return (Liveness::Malformed, None, read_heartbeat(state), None);
    };
    let heartbeat = read_heartbeat(state);
    if !pid_alive(pid) {
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
    let items = state.items().unwrap_or_default();
    let count = |status: ItemStatus| items.iter().filter(|i| i.status == status).count();
    let (slots_total, slots_in_use) = slot_occupancy(state);
    let active_batches = match &heartbeat {
        Some(h) if !h.active_batches.is_empty() => h.active_batches.clone(),
        _ => newest_run_batches(state),
    };
    let remedy = match daemon {
        Liveness::Running => String::new(),
        Liveness::NotRunning => {
            "start the daemon with `merge-queue serve` (or `systemctl --user start merge-queue-serve`)"
                .to_owned()
        }
        Liveness::Dead => format!(
            "remove the stale pid file {} and start the daemon with `merge-queue serve`",
            state.serve_pid_path().display()
        ),
        Liveness::Stale => format!(
            "the daemon (pid {}) is alive but has not heartbeat in {}s; \
             remedy: check it with `merge-queue status`, restart it with `kill {}` then `merge-queue serve`",
            pid.unwrap_or(0),
            heartbeat_age_secs.unwrap_or(0),
            pid.unwrap_or(0)
        ),
        Liveness::Malformed => format!(
            "the pid file {} is unreadable; remedy: remove it and start the daemon with `merge-queue serve`",
            state.serve_pid_path().display()
        ),
    };
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
                out.push_str(&format!(
                    "  state {} · cycles {} · last trigger {} · window {} · last run {}\n",
                    h.state,
                    h.cycles,
                    h.last_trigger,
                    h.window,
                    if h.last_run.is_empty() {
                        "-"
                    } else {
                        h.last_run.as_str()
                    }
                ));
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
                "daemon: dead (pid {} is not alive{})\n",
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
    out.push_str(&format!(
        "queue: {} queued, {} landed, {} ejected, {} withdrawn\n",
        report.queued, report.landed, report.ejected, report.withdrawn
    ));
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
        let mut heartbeat = Heartbeat::new(999_999_999, 2);
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
        let mut fresh = Heartbeat::new(std::process::id(), 2);
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
}
