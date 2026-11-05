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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
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
/// Default seconds between label syncs (`serve --sync-every`).
pub const DEFAULT_SYNC_EVERY_SECS: u64 = 60;
/// Pull requests one label sync lists at most (`serve --sync-limit`): one page, so a sync is
/// charged one request of the API budget (three with approval at head), not three pages' worth.
/// A listing that reaches the limit withdraws nothing it did not list.
pub const DEFAULT_SYNC_LIMIT: u32 = 100;
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

/// Whether shutdown has been requested (also used by tests).
pub fn shutdown_requested() -> bool {
    crate::interrupt::requested()
}

/// Reset the shutdown flag (test support; a fresh daemon starts clear).
pub fn reset_shutdown() {
    crate::interrupt::reset();
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
    /// Label this daemon syncs (`serve --sync-label`); empty when it does not.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sync_label: String,
    /// RFC 3339 UTC time of the last successful label sync.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_sync_at: String,
    /// What the last successful label sync did, in one line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_sync: String,
    /// Why the most recent label sync failed; cleared by a successful one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sync_error: String,
    /// RFC 3339 UTC time the label sync in flight started (it runs beside the loop, and its API
    /// call can wait on the request budget); empty when none is running.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sync_started_at: String,
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
            sync_label: String::new(),
            last_sync_at: String::new(),
            last_sync: String::new(),
            sync_error: String::new(),
            sync_started_at: String::new(),
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
    lock_held(&serve_lock_path(state))
}

/// Whether some process holds a lock on the file at `path`: `None` when there is no such
/// file. The probe takes a shared lock for an instant only, so lock takers retry briefly.
fn lock_held(path: &Path) -> Option<bool> {
    let file = File::open(path).ok()?;
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            Some(false)
        }
        Err(_) => Some(true),
    }
}

/// The file a run holds locked while a validation uses the scratch slot checkout at `slot`
/// (`scratch/slot-<n>` → `scratch/slot-<n>.lock`).
pub fn slot_lock_path(slot: &Path) -> PathBuf {
    let mut name = slot.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    slot.with_file_name(name)
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

/// Count scratch validation slots: `(existing, in use)`. A slot keeps its checkout between
/// validations, so it is in use only while a run holds its lock file ([`slot_lock_path`]).
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
            if lock_held(&slot_lock_path(&path)) == Some(true) {
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

/// The shipped systemd user-unit template. `install-serve-unit` renders it
/// by replacing its three placeholders; it is never copied verbatim.
pub const SERVE_UNIT_TEMPLATE: &str = include_str!("../merge-queue-serve.service");

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Resolve `name` against `path_env` (a `PATH` value). A name containing
/// `/` is checked as a path, relative to `base` when not absolute.
pub fn find_executable(
    name: &str,
    path_env: Option<&std::ffi::OsStr>,
    base: &Path,
) -> Option<PathBuf> {
    if name.contains('/') {
        let candidate = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            base.join(name)
        };
        return is_executable_file(&candidate).then_some(candidate);
    }
    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// First program a `sh -c` template runs: skips `VAR=value` prefixes and a
/// leading `exec`. `None` for an empty template.
pub fn leading_executable(command: &str) -> Option<String> {
    let words = shell_words::split(command).ok()?;
    for word in words {
        if word == "exec" {
            continue;
        }
        let is_assignment = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        });
        if !is_assignment {
            return Some(word);
        }
    }
    None
}

fn configured_executables(
    config: &Config,
    agentctl_override: Option<&[String]>,
) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    fn shell(out: &mut Vec<(&'static str, String)>, label: &'static str, command: &str) {
        if let Some(tool) = leading_executable(command) {
            out.push((label, tool));
        }
    }
    shell(
        &mut out,
        "regenerate.build_command",
        &config.regenerate.build_command,
    );
    shell(&mut out, "regenerate.command", &config.regenerate.command);
    shell(&mut out, "validator.list", &config.validator.list);
    shell(&mut out, "validator.select", &config.validator.select);
    if config.validator.kind == "dagrun" {
        let dagrun = if config.validator.dagrun.is_empty() {
            "dagrun"
        } else {
            &config.validator.dagrun
        };
        out.push(("validator.dagrun", dagrun.to_owned()));
    } else {
        shell(&mut out, "validator.run", &config.validator.run);
    }
    if config.land.driver == "command" {
        shell(&mut out, "land.command", &config.land.command);
    }
    if let Some(tool) = config.github.gh.first() {
        out.push(("github.gh", tool.clone()));
    }
    if let Some(tool) = config.net_wrapper.first() {
        out.push(("net_wrapper", tool.clone()));
    }
    // Deliberately not checked: `notify.agentctl`. A missing sender is a
    // failed send, never a daemon failure (see cli_notify tests).
    let _ = agentctl_override;
    for tool in config.validator.admission.iter().take(1) {
        out.push(("validator.admission", tool.clone()));
    }
    out.push(("git", "git".to_owned()));
    out.push(("sh", "sh".to_owned()));
    out
}

/// Startup preflight: every executable the daemon's configured
/// regenerate/validator/land commands name must resolve on the current
/// `PATH` (relative paths resolve against the queue's repository). Returns
/// an error naming each missing tool, e.g. `cargo not found`.
pub fn preflight(config: &Config, agentctl_override: Option<&[String]>) -> Result<(), String> {
    preflight_with_path(
        config,
        agentctl_override,
        std::env::var_os("PATH").as_deref(),
    )
}

/// [`preflight`] against an explicit `PATH` value (tests).
pub fn preflight_with_path(
    config: &Config,
    agentctl_override: Option<&[String]>,
    path_env: Option<&std::ffi::OsStr>,
) -> Result<(), String> {
    // Shell builtins (`exit`, `cd`, ...) need no file on PATH.
    const BUILTINS: &[&str] = &[
        ":", ".", "[", "alias", "bg", "break", "builtin", "cd", "command", "continue", "echo",
        "eval", "exec", "exit", "export", "false", "fg", "hash", "jobs", "kill", "local", "printf",
        "pwd", "read", "readonly", "return", "set", "shift", "test", "times", "trap", "true",
        "type", "umask", "unalias", "unset", "wait",
        // Shell control keywords: a template starting with one is composite;
        // its leading word names no file (inner tools are covered by PATH).
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "{", "}", "!", "time", "function", "select", "[[", "]]",
    ];
    let missing: Vec<String> = configured_executables(config, agentctl_override)
        .into_iter()
        .filter(|(_, tool)| !BUILTINS.contains(&tool.as_str()))
        // A relative path (`common/bin/dagrun`, `./scripts/run.sh`) names a file inside the
        // candidate checkout the command runs in, which exists only at run time; the queue's
        // own repository is often bare, so resolving it there would refuse a working setup.
        .filter(|(_, tool)| !(tool.contains('/') && !Path::new(tool).is_absolute()))
        .filter(|(_, tool)| find_executable(tool, path_env, &config.repo).is_none())
        .map(|(label, tool)| format!("{tool} not found (needed by {label})"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "serve preflight failed: {}; PATH={}; remedy: install the named tool(s) or add \
             their directory to the service PATH (see `merge-queue install-serve-unit`)",
            missing.join("; "),
            path_env
                .map(|path| path.to_string_lossy())
                .unwrap_or_default()
        ))
    }
}

fn require_absolute(label: &str, path: &Path) -> Result<(), String> {
    if path.is_absolute() && !path.to_string_lossy().contains('\n') {
        Ok(())
    } else {
        Err(format!(
            "{label} must be an absolute path without a newline, got {}",
            path.display()
        ))
    }
}

/// Render the serve unit for `binary` running in `repo`, with
/// `toolchain_bin` (the directory containing `cargo`, resolved at install
/// time) first on the service `PATH`. All three paths must be absolute:
/// a systemd user service inherits no login-shell `PATH`.
pub fn render_serve_unit(
    binary: &Path,
    repo: &Path,
    toolchain_bin: &Path,
) -> Result<String, String> {
    require_absolute("merge-queue binary", binary)?;
    require_absolute("repository", repo)?;
    require_absolute("toolchain directory", toolchain_bin)?;
    let rendered = SERVE_UNIT_TEMPLATE
        .replace("__TOOLCHAIN_BIN__", &toolchain_bin.to_string_lossy())
        .replace("__MERGE_QUEUE_BIN__", &binary.to_string_lossy())
        .replace("__REPO__", &repo.to_string_lossy());
    if rendered.contains("__TOOLCHAIN_BIN__")
        || rendered.contains("__MERGE_QUEUE_BIN__")
        || rendered.contains("__REPO__")
    {
        return Err("serve unit template has an unreplaced placeholder".to_owned());
    }
    Ok(rendered)
}

/// Resolve the directory containing `cargo` without relying on a login
/// shell: `rustup which cargo` first, then `cargo` on `PATH`, then
/// `$CARGO_HOME/bin` or `~/.cargo/bin`. Errors name `cargo`.
pub fn resolve_toolchain_bin() -> Result<PathBuf, String> {
    resolve_toolchain_bin_with(
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("CARGO_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn resolve_toolchain_bin_with(
    path_env: Option<&std::ffi::OsStr>,
    cargo_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    if let Some(rustup) = find_executable("rustup", path_env, Path::new(".")) {
        if let Ok(output) = std::process::Command::new(&rustup)
            .args(["which", "cargo"])
            .stdin(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let cargo = PathBuf::from(text.trim());
                if is_executable_file(&cargo) {
                    if let Some(parent) = cargo.parent() {
                        return Ok(parent.to_path_buf());
                    }
                }
            }
        }
    }
    if let Some(cargo) = find_executable("cargo", path_env, Path::new(".")) {
        if let Some(parent) = cargo.parent() {
            if !parent.as_os_str().is_empty() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(cargo_home) = cargo_home.filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(cargo_home).join("bin"));
    }
    if let Some(home) = home.filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    for dir in candidates {
        if is_executable_file(&dir.join("cargo")) {
            return Ok(dir);
        }
    }
    Err(
        "cargo not found: no rustup, no cargo on PATH, and no cargo in \
         $CARGO_HOME/bin or ~/.cargo/bin; remedy: install Rust with rustup, \
         then re-run `merge-queue install-serve-unit`"
            .to_owned(),
    )
}

/// Default systemd user-unit directory (`$XDG_CONFIG_HOME/systemd/user`,
/// else `~/.config/systemd/user`).
pub fn default_unit_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join("systemd").join("user"));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            "HOME is not set; remedy: pass --output with the unit file path".to_owned()
        })?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user"))
}

/// Render and write `merge-queue-serve.service` to `output`, resolving the
/// toolchain directory explicitly (or using `toolchain_bin` when given,
/// which must contain an executable `cargo`). Returns the written path.
/// The toolchain directory to write into the unit: `given` when passed
/// (it must contain an executable `cargo`), else [`resolve_toolchain_bin`].
pub fn checked_toolchain_bin(given: Option<&Path>) -> Result<PathBuf, String> {
    match given {
        Some(dir) => {
            require_absolute("toolchain directory", dir)?;
            if !is_executable_file(&dir.join("cargo")) {
                return Err(format!(
                    "cargo not found in {} (passed as the toolchain directory); \
                     remedy: pass the directory containing cargo, e.g. from `rustup which cargo`",
                    dir.display()
                ));
            }
            Ok(dir.to_path_buf())
        }
        None => resolve_toolchain_bin(),
    }
}

/// Render and write `merge-queue-serve.service` to `output`, resolving the
/// toolchain directory explicitly. Returns the written path.
pub fn install_serve_unit(
    binary: &Path,
    repo: &Path,
    toolchain_bin: Option<&Path>,
    output: &Path,
) -> Result<PathBuf, String> {
    let toolchain_bin = checked_toolchain_bin(toolchain_bin)?;
    let rendered = render_serve_unit(binary, repo, &toolchain_bin)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(output, rendered)
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    Ok(output.to_path_buf())
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
    /// Override the configured agentctl registry for push notifications.
    pub agentctl_registry: Option<std::path::PathBuf>,
    /// Deadline for one notification send; a send still running then has
    /// its process group killed and counts as a failed attempt.
    pub notify_timeout: Duration,
    /// Also ingest pull requests carrying this label, on a thread beside the
    /// loop (`serve --sync-label`).
    pub sync_label: Option<String>,
    /// Interval between label syncs.
    pub sync_every: Duration,
    /// Pull requests one label sync lists at most.
    pub sync_limit: u32,
    /// Override `validator.offload` for work cycles.
    pub offload: Option<String>,
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
            agentctl_registry: None,
            notify_timeout: notify::DEFAULT_SEND_TIMEOUT,
            sync_label: None,
            sync_every: Duration::from_secs(DEFAULT_SYNC_EVERY_SECS),
            sync_limit: DEFAULT_SYNC_LIMIT,
            offload: None,
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
fn refresh_heartbeat(
    state: &State,
    shared: &Mutex<Heartbeat>,
    done: &AtomicBool,
    window: &Mutex<Vec<Vec<i64>>>,
) {
    let mut last = Instant::now();
    while !done.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
        if last.elapsed() >= HEARTBEAT_REFRESH && !done.load(Ordering::SeqCst) {
            last = Instant::now();
            let (total, in_use) = slot_occupancy(state);
            let batches = window
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let mut heartbeat = lock_heartbeat(shared);
            heartbeat.slots_total = total;
            heartbeat.slots_in_use = in_use;
            heartbeat.active_batches = batches;
            heartbeat.touch();
            let _ = write_heartbeat(state, &heartbeat);
        }
    }
}

/// One plan+run cycle (the caller holds the writer lock). Returns whether it
/// was clean; a failure is recorded in the heartbeat's `last_error`.
fn run_cycle(
    state: &State,
    options: &ServeOptions,
    shared: &Mutex<Heartbeat>,
    window: &Arc<Mutex<Vec<Vec<i64>>>>,
    writer: &crate::state::Lock,
) -> bool {
    // The heartbeat's window is what the run publishes as it builds and decides batches (see
    // `refresh_heartbeat`), never the plan: a change the run skips at construction is not in it.
    let run_options = RunOptions {
        shadow: options.shadow,
        depth: options.depth,
        max_batch: options.max_batch,
        quiet: true,
        offload: options.offload.clone(),
        drain_pid: Some(std::process::id()),
        window: Some(Arc::clone(window)),
        // A run in progress looks for recorded requests as often as the daemon polls.
        intake_poll: Some(options.poll),
        ..RunOptions::default()
    };
    let outcome = Runner::new(state, run_options)
        .map_err(|error| format!("run refused: {error}"))
        .and_then(|runner| {
            runner
                .holding(writer)
                .run()
                .map_err(|error| format!("run failed: {error}"))
        });
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
            for deferred in &report.deferred {
                let prs = deferred
                    .prs
                    .iter()
                    .map(|pr| format!("#{pr}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!(
                    "merge-queue serve: deferred {prs} (still queued, retry after {}): {}",
                    deferred.retry_after, deferred.reason
                );
            }
            // A validator that cannot execute is the host's problem, not the changes': no
            // verdict, and the operator is told what to fix.
            let alarm = report
                .deferred
                .iter()
                .find(|deferred| deferred.reason.starts_with(crate::validate::CANNOT_EXECUTE));
            if let Some(deferred) = alarm {
                eprintln!(
                    "merge-queue serve: ALARM: {}; fix the validator or the service PATH",
                    deferred.reason
                );
            }
            // Bounded retry: changes past `validator.max_deferrals` are held for their owner.
            let escalated: Vec<String> = report
                .deferred
                .iter()
                .flat_map(|deferred| deferred.escalated.iter())
                .map(|pr| format!("#{pr}"))
                .collect();
            let alarm_text = if escalated.is_empty() {
                alarm.map(|deferred| format!("ALARM: {}", deferred.reason))
            } else {
                let text = format!(
                    "ALARM: {} escalated to the owner (validation did not run the bounded number \
                     of times); held until `merge-queue retry`",
                    escalated.join(" ")
                );
                eprintln!("merge-queue serve: {text}");
                Some(text)
            };
            // A donor that keeps giving jobs back with the same reason is an ERROR the
            // health tick must see, even though no single give-back is a queue failure.
            let give_back = state.config().ok().and_then(|config| {
                crate::donor::Store::new(&state.root)
                    .give_back_alerts(config.donors.give_back_alert())
                    .ok()
                    .filter(|alerts| !alerts.is_empty())
                    .map(|alerts| alerts.join("; "))
            });
            if let Some(alert) = &give_back {
                eprintln!("merge-queue serve: {alert}");
            }
            if report.stopped.is_empty() {
                heartbeat.last_error = match (alarm_text, give_back) {
                    (Some(alarm), Some(alert)) => format!("{alarm}; {alert}"),
                    (Some(alarm), None) => alarm,
                    (None, Some(alert)) => alert,
                    (None, None) => String::new(),
                };
                true
            } else {
                heartbeat.last_error =
                    alarm_text.unwrap_or_else(|| format!("run stopped: {}", report.stopped));
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

/// Clear the busy-lock error from the heartbeat once it no longer applies: the daemon got the
/// lock, or has nothing to do with it. Any other error the heartbeat shows stays.
fn clear_busy_error(heartbeat: &mut Heartbeat, busy_error: &mut String) {
    if !busy_error.is_empty() && heartbeat.last_error == *busy_error {
        heartbeat.last_error.clear();
    }
    busy_error.clear();
}

/// Run one work cycle with the heartbeat kept fresh by a helper thread, under `writer`, the
/// writer lock the caller holds.
fn work_cycle(
    state: &State,
    options: &ServeOptions,
    heartbeat: &mut Heartbeat,
    writer: &crate::state::Lock,
) -> bool {
    heartbeat.state = "working".to_owned();
    heartbeat.working_since = now();
    heartbeat.working_since_unix = unix_now();
    heartbeat.touch();
    let _ = write_heartbeat(state, heartbeat);
    let shared = Mutex::new(heartbeat.clone());
    let done = AtomicBool::new(false);
    let window = Arc::new(Mutex::new(Vec::new()));
    let clean = std::thread::scope(|scope| {
        scope.spawn(|| refresh_heartbeat(state, &shared, &done, &window));
        let _stop = SetOnDrop(&done);
        run_cycle(state, options, &shared, &window, writer)
    });
    *heartbeat = shared.into_inner().unwrap_or_else(PoisonError::into_inner);
    heartbeat.state = "idle".to_owned();
    heartbeat.working_since.clear();
    heartbeat.working_since_unix = 0;
    heartbeat.active_batches.clear();
    clean
}

/// Apply the requests recorded while another writer held the queue, under the writer lock the
/// caller holds, and log what each did.
fn apply_intake(state: &State, config: &Config, lock: &crate::state::Lock) {
    if crate::intake::has_pending(state) {
        crate::intake::report(
            "merge-queue serve",
            crate::intake::apply(state, config, lock),
        );
    }
}

/// Record the closes of landed pull requests that a label sync made while another writer held
/// the queue, under the writer lock the caller holds.
fn record_closed(state: &State, lock: &crate::state::Lock) {
    match crate::sync::record_closed(state, lock) {
        Ok(closed) if !closed.is_empty() => {
            eprintln!(
                "merge-queue serve: recorded {} pull request close(s) made while the queue was busy",
                closed.len()
            );
        }
        Ok(_) => {}
        Err(error) => eprintln!("merge-queue serve: cannot record closes: {error}"),
    }
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

/// A label sync running on its own thread. Its API call can wait minutes on the request budget,
/// so the loop never waits for it: planning, intake and the heartbeat go on, and the loop folds
/// the outcome in once the thread is done.
struct LabelSync {
    handle: std::thread::JoinHandle<Result<crate::sync::SyncReport, String>>,
    started: Instant,
}

impl LabelSync {
    /// Start one sync of `label`: list it (no lock held), then reconcile under the writer lock
    /// when it is free, or record the submissions for its holder when it is not (a run takes
    /// them while it runs). Postponed closes of landed pull requests are made without the lock
    /// and recorded by the next holder ([`crate::sync::record_closed`]).
    fn start(
        state: &State,
        config: &Config,
        label: &str,
        limit: u32,
        notifier: &notify::Notifier,
    ) -> Self {
        let (state, config, notifier) = (state.clone(), config.clone(), notifier.clone());
        let source = crate::sync::ReadySource::Label(label.to_owned());
        let handle = std::thread::spawn(move || {
            let listing = crate::sync::list(&state, &config, &source, limit)?;
            let mut report = match state.lock() {
                Ok(lock) => crate::sync::reconcile(
                    &state,
                    &config,
                    &source,
                    listing,
                    crate::sync::Apply::Locked(&lock),
                    Some(&notifier),
                )?,
                Err(busy) => crate::sync::reconcile(
                    &state,
                    &config,
                    &source,
                    listing,
                    crate::sync::Apply::Record(&busy),
                    Some(&notifier),
                )?,
            };
            if config.land.close_prs {
                crate::sync::close_postponed(&state, &config)?;
                if let Ok(lock) = state.lock() {
                    report.closed = crate::sync::record_closed(&state, &lock)?;
                }
            }
            report.api_remaining =
                crate::github::Hosting::new(&config.github, &state.root, config.net_timeout())
                    .ok()
                    .map(|hosting| hosting.budget.remaining());
            Ok(report)
        });
        Self {
            handle,
            started: Instant::now(),
        }
    }

    /// Wait for the sync and fold its outcome into `heartbeat`, logging what it did.
    fn finish(self, label: &str, limit: u32, heartbeat: &mut Heartbeat) {
        let outcome = self
            .handle
            .join()
            .unwrap_or_else(|_| Err("the label sync thread panicked".to_owned()));
        heartbeat.sync_started_at.clear();
        match outcome {
            Ok(report) => {
                let list = |prs: &[i64]| {
                    prs.iter()
                        .map(|pr| format!("#{pr}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let mut summary = format!(
                    "{} listed, submitted [{}], withdrawn [{}]",
                    report.listed,
                    list(&report.submitted),
                    list(&report.withdrawn)
                );
                if !report.recorded.is_empty() {
                    summary.push_str(&format!(
                        ", recorded [{}] for the run holding the queue",
                        list(&report.recorded)
                    ));
                }
                if !report.refused.is_empty() {
                    let refused: Vec<i64> = report.refused.iter().map(|(pr, _)| *pr).collect();
                    summary.push_str(&format!(", refused [{}]", list(&refused)));
                }
                if report.truncated {
                    summary.push_str(&format!(
                        ", listed the limit ({limit}) so withdrew nothing unlisted: raise --sync-limit"
                    ));
                }
                if !report.submitted.is_empty()
                    || !report.withdrawn.is_empty()
                    || !report.recorded.is_empty()
                    || !report.refused.is_empty()
                {
                    eprintln!("merge-queue serve: sync --label {label}: {summary}");
                }
                for (pr, why) in &report.refused {
                    eprintln!("merge-queue serve: sync --label {label}: refused #{pr}: {why}");
                }
                heartbeat.last_sync_at = now();
                heartbeat.last_sync = summary;
                heartbeat.sync_error.clear();
            }
            Err(error) => {
                if heartbeat.sync_error != error {
                    eprintln!("merge-queue serve: sync --label {label} failed: {error}");
                }
                heartbeat.sync_error = error;
            }
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
/// SIGINT/SIGTERM stop the daemon mid-cycle too: the run cancels its
/// validations, lands and ejects nothing more, and leaves every change it
/// had not decided queued (see [`crate::interrupt`]). A second signal exits
/// at once.
pub fn serve(state: &State, options: &ServeOptions) -> Result<i32, String> {
    let config = state.config().map_err(|e| {
        format!(
            "{e}; remedy: run `merge-queue init` for this repository before `merge-queue serve`"
        )
    })?;
    // Fail loudly before the first cycle when a configured tool is missing
    // (e.g. `cargo` under a systemd unit with no toolchain on PATH).
    preflight(&config, options.agentctl.as_deref())?;
    if options.sync_label.is_some() && config.github.repo.is_empty() {
        return Err(
            "--sync-label needs a hosting repository; remedy: run `merge-queue init --github-repo OWNER/NAME` (or set github.repo in config.json)"
                .to_owned(),
        );
    }
    // Held (and so the single-daemon guarantee kept) until this returns.
    let _serve_lock = acquire_serve_lock(state)?;
    reset_shutdown();
    crate::interrupt::install();
    write_pid(state)?;
    let pid_file = PidFile(state);
    // A drain request on file was for an earlier daemon (this one holds `serve.lock`, so that
    // daemon is gone): it must not drain this one.
    if let Some(request) = crate::drain::read(state) {
        if crate::drain::clear(state).unwrap_or(false) {
            eprintln!(
                "merge-queue serve: removed a drain request for pid {} ({}): it was for an earlier daemon",
                request.pid,
                request.describe()
            );
        }
    }
    let git = ops::repo(state, &config)?;
    let window = options.depth.unwrap_or(config.depth).max(1);
    let mut heartbeat = Heartbeat::new(std::process::id(), window, options.shadow);
    heartbeat.sync_label = options.sync_label.clone().unwrap_or_default();
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
    let notifier = notify::Notifier::resolve(
        options.agentctl.as_deref(),
        options.agentctl_registry.as_deref(),
        &config,
    )
    .with_timeout(options.notify_timeout);

    let mut last_fingerprint = String::new();
    let mut last_base: Option<String> = None;
    let mut last_base_poll: Option<Instant> = None;
    // Newest refusal deferral a work cycle has already acted on.
    let mut retry_seen = String::new();
    // Backoff retry after failed work cycles: when, and as RFC 3339 text.
    let mut failure_retry: Option<(Instant, String)> = None;
    // When the last label sync started (`--sync-label`); `None` means due now.
    let mut last_sync: Option<Instant> = None;
    // The label sync in flight, if any.
    let mut label_sync: Option<LabelSync> = None;
    // The busy-lock error the heartbeat shows, until the daemon gets the lock or has no work.
    let mut busy_error = String::new();

    'outer: loop {
        if shutdown_requested() {
            break 'outer;
        }
        // Drained: no run is in progress here (a run ends before the loop comes back), so the
        // daemon exits and its replacement starts with nothing thrown away.
        if let Some(request) = crate::drain::requested_for(state, heartbeat.pid) {
            // Requests recorded during the last run go in first, so the queue lists them.
            if crate::intake::has_pending(state) {
                if let Ok(lock) = state.lock() {
                    apply_intake(state, &config, &lock);
                }
            }
            let _ = crate::drain::clear(state);
            eprintln!(
                "merge-queue serve: {} ({}); no run in progress, exiting",
                crate::drain::DRAINED,
                request.describe()
            );
            break 'outer;
        }
        heartbeat.cycles += 1;
        // Requests recorded while a run held the writer lock go in first, in order, so this
        // iteration's items fingerprint sees them.
        if crate::intake::has_pending(state) {
            if let Ok(lock) = state.lock() {
                apply_intake(state, &config, &lock);
            }
        }
        // The label sync runs beside the loop, never in its way: a sync that finished is folded
        // in (what it submitted changes this iteration's items fingerprint), and one is started
        // when it is due. With `--once` the single cycle includes its sync, so it waits for it.
        if let Some(label) = &options.sync_label {
            if label_sync
                .as_ref()
                .is_some_and(|sync| sync.handle.is_finished())
            {
                if let Some(done) = label_sync.take() {
                    last_sync = Some(done.started);
                    done.finish(label, options.sync_limit, &mut heartbeat);
                }
            }
            if label_sync.is_none() && last_sync.is_none_or(|at| at.elapsed() >= options.sync_every)
            {
                let started =
                    LabelSync::start(state, &config, label, options.sync_limit, &notifier);
                heartbeat.sync_started_at = now();
                if options.once {
                    last_sync = Some(started.started);
                    started.finish(label, options.sync_limit, &mut heartbeat);
                } else {
                    label_sync = Some(started);
                }
            }
        }
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
                        clear_busy_error(&mut heartbeat, &mut busy_error);
                        last_fingerprint = fingerprint.clone();
                        failure_retry = None;
                        heartbeat.failed_cycles = 0;
                        heartbeat.state = "idle".to_owned();
                        heartbeat.active_batches.clear();
                    } else {
                        match state.lock() {
                            Ok(lock) => {
                                clear_busy_error(&mut heartbeat, &mut busy_error);
                                apply_intake(state, &config, &lock);
                                record_closed(state, &lock);
                                if let Some(due) = schedule.due.as_ref() {
                                    if *due > retry_seen {
                                        retry_seen.clone_from(due);
                                    }
                                }
                                if work_cycle(state, options, &mut heartbeat, &lock) {
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
                                // the new baseline is taken. A stop skips
                                // them: they stay owed and go out on the
                                // next start, not inside a stop timeout.
                                if !shutdown_requested() {
                                    sweep_notifications(state, &lock, &config, &notifier);
                                }
                                swept = true;
                                // Whatever the cycle did, the resulting items
                                // state is the new baseline: never spin on the
                                // same fingerprint.
                                last_fingerprint = items_fingerprint(state);
                            }
                            Err(error) => {
                                heartbeat.last_trigger = "lock-busy".to_owned();
                                busy_error.clone_from(&error);
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
            if crate::drain::requested_for(state, heartbeat.pid).is_some() {
                continue 'outer;
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
    /// Current window: the batches a live daemon's heartbeat names. Empty while
    /// the daemon idles, and when no daemon is running.
    pub active_batches: Vec<Vec<i64>>,
    /// Scratch slots that exist.
    pub slots_total: usize,
    /// Scratch slots a validation is using now.
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
    /// Requests recorded while another writer held the queue: waiting for the
    /// lock, and refused when applied (see `crate::intake`).
    pub intake: crate::intake::Summary,
    /// A drain request on file (`merge-queue drain-for-upgrade`), whichever daemon it names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain: Option<crate::drain::Request>,
    /// Queued changes held because a change they must land after (`--after`) is ejected,
    /// withdrawn or not in the queue (see [`crate::model::Waiting`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<crate::model::WaitingChange>,
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
    // A live daemon's heartbeat is the authority: no batches there means it is idle, whatever
    // the newest run planned. A run's report is written when the run ends, so a run directory
    // can only ever show batches that are no longer active.
    let active_batches = match (&daemon, &heartbeat) {
        (Liveness::Running | Liveness::Stale, Some(h)) => h.active_batches.clone(),
        _ => Vec::new(),
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
        intake: crate::intake::summary(state),
        drain: crate::drain::read(state),
        waiting: crate::model::waiting_changes(&items),
        items_error,
        remedy,
    })
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
                if let Some(request) = &report.drain {
                    out.push_str(&if Some(request.pid) == report.pid {
                        format!(
                            "  drain: {}; finishing the validations in flight, starting none, then exiting\n",
                            request.describe()
                        )
                    } else {
                        format!(
                            "  drain: a request for pid {} ({}) is not for this daemon and is ignored\n",
                            request.pid,
                            request.describe()
                        )
                    });
                }
                if !h.next_retry_at.is_empty() {
                    out.push_str(&format!("  next retry: {}\n", h.next_retry_at));
                }
                if !h.last_error.is_empty() {
                    out.push_str(&format!("  last error: {}\n", h.last_error));
                }
                if !h.sync_label.is_empty() {
                    out.push_str(&format!(
                        "  sync: label {} · last {}{}\n",
                        h.sync_label,
                        if h.last_sync_at.is_empty() {
                            "-"
                        } else {
                            h.last_sync_at.as_str()
                        },
                        if h.last_sync.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", h.last_sync)
                        }
                    ));
                }
                if !h.sync_started_at.is_empty() {
                    out.push_str(&format!("  sync in flight since {}\n", h.sync_started_at));
                }
                if !h.sync_error.is_empty() {
                    out.push_str(&format!("  sync error: {}\n", h.sync_error));
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
        for change in &report.waiting {
            for waiting in &change.waiting_on {
                out.push_str(&format!("  #{} held: {}\n", change.pr, waiting.describe()));
            }
        }
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
    let intake = &report.intake;
    if intake.pending > 0 {
        out.push_str(&format!(
            "intake: {} request(s) recorded while a run held the queue, applied at its next free \
             slot, or at its end for a change in flight (oldest {})\n",
            intake.pending, intake.oldest_pending
        ));
        for request in &intake.requests {
            out.push_str(&format!("  {}\n", request.line()));
        }
    }
    if intake.refused > 0 {
        out.push_str(&format!(
            "intake refused: {} in the last week\n",
            intake.refused
        ));
        for entry in &intake.recent_refusals {
            // One line each: a refusal that quotes output (a precheck's) keeps it in its record.
            let mut lines = entry.error.lines();
            let first = lines.next().unwrap_or_default();
            let more = lines.count();
            out.push_str(&format!(
                "  {} recorded at {}, refused at {}: {first}{}\n",
                entry.request.describe(),
                entry.recorded_at,
                entry.refused_at,
                if more > 0 {
                    format!(
                        " (and {more} more line(s) in intake/refused/{}.json)",
                        entry.id
                    )
                } else {
                    String::new()
                }
            ));
        }
    }
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
    fn status_shows_the_label_sync_and_its_last_error() {
        let state = temp_state("sync-status");
        std::fs::write(state.serve_pid_path(), format!("{}\n", std::process::id())).unwrap();
        let mut heartbeat = Heartbeat::new(std::process::id(), 2, false);
        heartbeat.sync_label = "mq-ready".to_owned();
        heartbeat.last_sync_at = "2026-11-02T23:00:00Z".to_owned();
        heartbeat.last_sync = "3 listed, submitted [#7], withdrawn []".to_owned();
        heartbeat.sync_error = "HTTP 502: bad gateway".to_owned();
        heartbeat.touch();
        write_heartbeat(&state, &heartbeat).unwrap();
        let text = render_status(&collect_status(&state, 10).unwrap());
        assert!(
            text.contains(
                "sync: label mq-ready · last 2026-11-02T23:00:00Z (3 listed, submitted [#7], withdrawn [])"
            ),
            "{text}"
        );
        assert!(text.contains("sync error: HTTP 502: bad gateway"), "{text}");
        // A daemon that does not sync says nothing about syncing.
        let mut quiet = Heartbeat::new(std::process::id(), 2, false);
        quiet.touch();
        write_heartbeat(&state, &quiet).unwrap();
        let text = render_status(&collect_status(&state, 10).unwrap());
        assert!(
            !text.contains("sync:") && !text.contains("sync error"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn rendered_unit_names_an_explicit_toolchain_path() {
        let rendered = render_serve_unit(
            Path::new("/opt/bin/merge-queue"),
            Path::new("/srv/clone"),
            Path::new("/home/dev/.rustup/toolchains/stable/bin"),
        )
        .unwrap();
        // Explicit absolute toolchain dir on the service PATH, never a bare
        // reliance on whatever PATH systemd happens to give the unit.
        assert!(
            rendered.contains(
                "Environment=PATH=/home/dev/.rustup/toolchains/stable/bin:/usr/local/bin:/usr/bin:/bin"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("WorkingDirectory=/srv/clone"),
            "{rendered}"
        );
        assert!(
            rendered.contains("ExecStart=/opt/bin/merge-queue --repo /srv/clone serve"),
            "{rendered}"
        );
        assert!(!rendered.contains("__TOOLCHAIN_BIN__"), "{rendered}");
        // The shipped template itself must not be a bare-PATH unit.
        assert!(
            SERVE_UNIT_TEMPLATE.contains("Environment=PATH=__TOOLCHAIN_BIN__:"),
            "{SERVE_UNIT_TEMPLATE}"
        );
        // A stop reaches the daemon first, so it can cancel its validations itself.
        assert!(rendered.contains("\nKillMode=mixed\n"), "{rendered}");
        // Relative paths are refused: systemd would resolve them arbitrarily.
        assert!(render_serve_unit(
            Path::new("merge-queue"),
            Path::new("/srv/clone"),
            Path::new("/opt/bin"),
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preflight_names_a_missing_configured_tool() {
        let state = temp_state("preflight");
        let mut config = Config {
            repo: state.root.clone(),
            regenerate: crate::state::RegenerateConfig {
                paths: vec!["generated/".to_owned()],
                command: "cargo run --release -- gen".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Fake PATH with the always-needed tools but no cargo.
        let fake_bin = state.root.join("fake-bin");
        std::fs::create_dir_all(&fake_bin).unwrap();
        for tool in ["git", "sh"] {
            let real = find_executable(tool, std::env::var_os("PATH").as_deref(), Path::new("."))
                .unwrap_or_else(|| panic!("{tool} on test PATH"));
            std::os::unix::fs::symlink(&real, fake_bin.join(tool)).unwrap();
        }
        let fake_path = std::ffi::OsString::from(&fake_bin);
        let error = preflight_with_path(&config, None, Some(&fake_path)).unwrap_err();
        assert!(error.contains("cargo not found"), "{error}");
        assert!(error.contains("regenerate.command"), "{error}");
        // With an empty command there is nothing to refuse.
        config.regenerate.command = String::new();
        // git/sh are still named when PATH has nothing at all.
        let empty = std::ffi::OsString::new();
        let error = preflight_with_path(&config, None, Some(&empty)).unwrap_err();
        assert!(error.contains("git not found"), "{error}");
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[cfg(unix)]
    #[test]
    fn preflight_does_not_resolve_tools_inside_the_candidate_checkout() {
        // The documented dagrun setup names the repository's own launcher relative to the
        // candidate checkout; the queue's repository (here a bare-like empty dir) lacks it.
        let state = temp_state("preflight-relative");
        let config = Config {
            repo: state.root.clone(),
            validator: crate::state::ValidatorConfig {
                kind: "dagrun".to_owned(),
                dagrun: "common/bin/dagrun".to_owned(),
                dag: "validation.dag.yaml".to_owned(),
                run: String::new(),
                select: "./scripts/select.sh --base \"$MQ_BASE\"".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            preflight_with_path(&config, None, std::env::var_os("PATH").as_deref()).is_ok(),
            "relative in-checkout tools must not fail preflight"
        );
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn leading_executable_skips_assignments_and_exec() {
        assert_eq!(
            leading_executable("FOO=1 exec cargo test").as_deref(),
            Some("cargo")
        );
        assert_eq!(
            leading_executable("  ./scripts/gen.sh x").as_deref(),
            Some("./scripts/gen.sh")
        );
        assert_eq!(leading_executable(""), None);
    }

    #[test]
    fn old_heartbeat_without_shadow_field_reads_as_live() {
        // Heartbeats written before `serve --shadow` have no `shadow`
        // field; they must deserialize as live (false), never shadow.
        let json = r#"{"pid":1,"started_at":"2026-01-01T00:00:00Z","heartbeat_at":"2026-01-01T00:00:00Z","heartbeat_unix":1,"cycles":0,"state":"idle","last_trigger":"startup","last_base_sha":"","queued":0,"active_batches":[],"window":2,"slots_total":0,"slots_in_use":0}"#;
        let heartbeat: Heartbeat = serde_json::from_str(json).unwrap();
        assert!(!heartbeat.shadow);
    }

    #[test]
    fn a_slot_is_in_use_only_while_its_lock_is_held() {
        let state = temp_state("slots");
        for slot in ["slot-0", "slot-1"] {
            // A slot keeps its checkout between validations.
            std::fs::create_dir_all(state.scratch_dir().join(slot).join("src")).unwrap();
        }
        assert_eq!(slot_occupancy(&state), (2, 0));
        let held = File::create(slot_lock_path(&state.scratch_dir().join("slot-1"))).unwrap();
        FileExt::lock_exclusive(&held).unwrap();
        assert_eq!(slot_occupancy(&state), (2, 1));
        drop(held);
        // A child another test forks at this instant shares the lock until it execs.
        let deadline = Instant::now() + Duration::from_secs(5);
        while slot_occupancy(&state) != (2, 0) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(slot_occupancy(&state), (2, 0));
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn only_a_live_daemon_s_heartbeat_names_active_batches() {
        let state = temp_state("window");
        // The newest run finished with a planned batch.
        let run = state.runs_dir().join("20260101T000000Z");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("report.json"),
            r#"{"plan":{"batches":[{"prs":[7,8]}]}}"#,
        )
        .unwrap();
        // A live daemon (this process) that is idle has no active batches.
        std::fs::write(state.serve_pid_path(), format!("{}\n", std::process::id())).unwrap();
        let mut heartbeat = Heartbeat::new(std::process::id(), 2, false);
        heartbeat.touch();
        write_heartbeat(&state, &heartbeat).unwrap();
        let report = collect_status(&state, 60).unwrap();
        assert_eq!(report.daemon, Liveness::Running);
        assert!(
            report.active_batches.is_empty(),
            "{:?}",
            report.active_batches
        );
        assert!(render_status(&report).contains("window: no active batches"));
        // A working daemon's batches are the window.
        heartbeat.active_batches = vec![vec![9]];
        heartbeat.touch();
        write_heartbeat(&state, &heartbeat).unwrap();
        assert_eq!(
            collect_status(&state, 60).unwrap().active_batches,
            vec![vec![9]]
        );
        // A dead daemon validates nothing, whatever its last heartbeat said.
        std::fs::write(state.serve_pid_path(), "999999999\n").unwrap();
        let report = collect_status(&state, 60).unwrap();
        assert_eq!(report.daemon, Liveness::Dead);
        assert!(
            report.active_batches.is_empty(),
            "{:?}",
            report.active_batches
        );
        let _ = std::fs::remove_dir_all(&state.root);
    }

    #[test]
    fn a_busy_lock_error_clears_and_other_errors_stay() {
        let busy_text = "another merge-queue writer holds q/lock (a run in progress?)";
        let mut heartbeat = Heartbeat::new(1, 2, false);
        let mut busy = busy_text.to_owned();
        heartbeat.last_error = busy.clone();
        clear_busy_error(&mut heartbeat, &mut busy);
        assert!(heartbeat.last_error.is_empty());
        assert!(busy.is_empty());
        // An error of another kind since the busy lock is not the busy lock's to clear.
        let mut busy = busy_text.to_owned();
        heartbeat.last_error = "ALARM: the validator cannot execute".to_owned();
        clear_busy_error(&mut heartbeat, &mut busy);
        assert_eq!(heartbeat.last_error, "ALARM: the validator cannot execute");
    }
}
