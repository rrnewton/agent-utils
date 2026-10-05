//! Per-account pacing state: one JSON file per account, guarded by an exclusive `flock`.
//!
//! Layout under the state directory (default `~/.local/state/gh-paced/`, mode 0700):
//!
//! - `<account>.json`: buckets, hourly windows, in-flight writes, cooldown, rate-limit snapshot;
//! - `<account>.lock`: the lock file every process takes before reading or writing the JSON;
//! - `<account>.audit.jsonl`: the append-only audit log (see `audit`).
//!
//! The lock is held only for short read-modify-write steps, never across a sleep or a network
//! call. Saving writes a temporary file and renames it over the old one, so a crash leaves
//! either the old or the new state. A file that cannot be parsed is moved aside to
//! `<account>.json.corrupt-<unix-seconds>` and replaced with EMPTY buckets (every class must
//! refill from zero), which errs on the side of fewer calls.

use crate::budget::Bucket;
use crate::classify::Class;
use crate::ratelimit::Snapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// State file format version.
pub const STATE_VERSION: u32 = 1;

/// A process that holds a write slot (or a rate-limit refresh claim).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    /// Process ID.
    pub pid: u32,
    /// Process start time in clock ticks since boot (`/proc/<pid>/stat` field 22), so a reused
    /// PID is not mistaken for the original holder.
    pub start_ticks: u64,
    /// Random identifier of this gh-paced invocation.
    pub nonce: String,
    /// Request class of the held slot.
    pub class: Class,
    /// When the slot was taken (Unix seconds).
    pub since: f64,
}

/// A host-wide pause after GitHub pushed back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cooldown {
    /// No paced call may start before this time (Unix seconds).
    pub until: f64,
    /// When the pushback was seen.
    pub set_at: f64,
    /// What gh printed that triggered it (pattern names, never the full output).
    pub reason: String,
    /// The command that received the pushback.
    pub command: String,
}

/// Everything gh-paced remembers for one account on one host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct State {
    /// File format version.
    pub version: u32,
    /// Token buckets and hourly windows, keyed by class name.
    pub buckets: BTreeMap<String, Bucket>,
    /// Running WRITE invocations.
    #[serde(default)]
    pub in_flight: Vec<Holder>,
    /// Active pushback cooldown.
    #[serde(default)]
    pub cooldown: Option<Cooldown>,
    /// Last `GET /rate_limit` result.
    #[serde(default)]
    pub rate_limit: Option<Snapshot>,
    /// Paced calls admitted since the last successful rate-limit refresh.
    #[serde(default)]
    pub calls_since_refresh: u32,
    /// Last time any process started a rate-limit refresh (Unix seconds).
    #[serde(default)]
    pub last_refresh_attempt: f64,
    /// The process currently refreshing the rate-limit snapshot.
    #[serde(default)]
    pub refresh_claim: Option<Holder>,
}

impl State {
    /// A fresh state with no history (buckets are created full on first use).
    pub fn new() -> Self {
        Self {
            version: STATE_VERSION,
            ..Self::default()
        }
    }

    /// Drop in-flight entries and refresh claims whose process has exited.
    pub fn reap(&mut self, alive: &dyn Fn(&Holder) -> bool) -> usize {
        let before = self.in_flight.len();
        self.in_flight.retain(|h| alive(h));
        if let Some(claim) = &self.refresh_claim {
            if !alive(claim) {
                self.refresh_claim = None;
            }
        }
        before - self.in_flight.len()
    }
}

/// Validate an account name: GitHub login syntax (letters, digits, hyphens; at most 39).
pub fn valid_account(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 39
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// Paths of one account's files.
#[derive(Debug, Clone)]
pub struct Paths {
    /// State directory.
    pub dir: PathBuf,
    /// Account name.
    pub account: String,
}

impl Paths {
    /// Paths for `account` under `dir`.
    pub fn new(dir: PathBuf, account: &str) -> Self {
        Self {
            dir,
            account: account.to_string(),
        }
    }

    /// `<account>.json`.
    pub fn state(&self) -> PathBuf {
        self.dir.join(format!("{}.json", self.account))
    }

    /// `<account>.lock`.
    pub fn lock(&self) -> PathBuf {
        self.dir.join(format!("{}.lock", self.account))
    }

    /// `<account>.audit.jsonl`.
    pub fn audit(&self) -> PathBuf {
        self.dir.join(format!("{}.audit.jsonl", self.account))
    }
}

/// Resolve the state directory: `GH_PACED_STATE_DIR`, else `$XDG_STATE_HOME/gh-paced`, else
/// `$HOME/.local/state/gh-paced`.
pub fn state_dir(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    if let Some(dir) = env("GH_PACED_STATE_DIR").filter(|v| !v.is_empty()) {
        return absolute(dir, "GH_PACED_STATE_DIR");
    }
    if let Some(dir) = env("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Ok(absolute(dir, "XDG_STATE_HOME")?.join("gh-paced"));
    }
    let home = env("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "HOME is not set; set GH_PACED_STATE_DIR".to_string())?;
    Ok(absolute(home, "HOME")?.join(".local/state/gh-paced"))
}

fn absolute(dir: String, var: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(dir);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{var} must be an absolute path"))
    }
}

/// Create the state directory (mode 0700) if needed.
pub fn ensure_dir(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot create state directory {}: {e}", dir.display()))?;
    }
    Ok(())
}

/// An exclusive lock on one account's state. Dropping it releases the lock.
pub struct LockGuard {
    _file: File,
}

/// Take the account's exclusive lock, blocking until it is free.
pub fn lock(paths: &Paths) -> Result<LockGuard, String> {
    ensure_dir(&paths.dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(paths.lock())
        .map_err(|e| format!("cannot open lock file {}: {e}", paths.lock().display()))?;
    loop {
        // SAFETY: flock on a valid, owned file descriptor.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc == 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("cannot lock {}: {err}", paths.lock().display()));
        }
    }
    Ok(LockGuard { _file: file })
}

/// Result of loading state.
pub struct Loaded {
    /// The state to use.
    pub state: State,
    /// Set when the file was unreadable and was quarantined.
    pub warning: Option<String>,
    /// True when no state file existed.
    pub fresh: bool,
}

/// Load the account's state. Call only while holding [`lock`].
///
/// `empty_buckets` builds the replacement buckets for a corrupt file (all classes empty).
pub fn load(
    paths: &Paths,
    now: f64,
    empty_buckets: &dyn Fn(f64) -> BTreeMap<String, Bucket>,
) -> Loaded {
    let path = paths.state();
    let text = match std::fs::read(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                state: State::new(),
                warning: None,
                fresh: true,
            }
        }
        Err(e) => {
            return quarantine(paths, now, empty_buckets, &format!("cannot read: {e}"));
        }
    };
    match serde_json::from_slice::<State>(&text) {
        Ok(state) if state.version == STATE_VERSION => Loaded {
            state,
            warning: None,
            fresh: false,
        },
        Ok(state) => quarantine(
            paths,
            now,
            empty_buckets,
            &format!("unsupported version {}", state.version),
        ),
        Err(e) => quarantine(paths, now, empty_buckets, &format!("cannot parse: {e}")),
    }
}

/// Load the account's state for display only: nothing is moved, quarantined or written. A
/// missing file is a fresh state; an unusable file is an error. Call only while holding [`lock`].
pub fn load_readonly(paths: &Paths) -> Result<State, String> {
    let path = paths.state();
    let text = match std::fs::read(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    match serde_json::from_slice::<State>(&text) {
        Ok(state) if state.version == STATE_VERSION => Ok(state),
        Ok(state) => Err(format!(
            "{} has unsupported version {}; the next paced call will move it aside",
            path.display(),
            state.version
        )),
        Err(e) => Err(format!(
            "{} is unusable ({e}); the next paced call will move it aside",
            path.display()
        )),
    }
}

fn quarantine(
    paths: &Paths,
    now: f64,
    empty_buckets: &dyn Fn(f64) -> BTreeMap<String, Bucket>,
    why: &str,
) -> Loaded {
    let path = paths.state();
    let aside = paths.dir.join(format!(
        "{}.json.corrupt-{}",
        paths.account,
        now.floor() as i64
    ));
    let moved = std::fs::rename(&path, &aside).is_ok();
    let mut state = State::new();
    state.buckets = empty_buckets(now);
    let where_ = if moved {
        format!("moved to {}", aside.display())
    } else {
        "could not be moved aside".to_string()
    };
    Loaded {
        state,
        warning: Some(format!(
            "state file {} is unusable ({why}); {where_}; starting with EMPTY buckets",
            path.display()
        )),
        fresh: false,
    }
}

/// Save the account's state atomically. Call only while holding [`lock`].
pub fn save(paths: &Paths, state: &State) -> Result<(), String> {
    let path = paths.state();
    let tmp = paths.dir.join(format!(
        ".{}.json.tmp-{}",
        paths.account,
        std::process::id()
    ));
    let text = serde_json::to_vec_pretty(state).map_err(|e| format!("cannot encode state: {e}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    file.write_all(&text)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    drop(file);
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

/// Start time (field 22 of `/proc/<pid>/stat`) of a process, in clock ticks since boot.
pub fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name (field 2) may contain spaces and parentheses; fields after it are plain.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(19)?.parse().ok()
}

/// True when the process recorded in `h` is still running (same PID and start time).
pub fn holder_alive(h: &Holder) -> bool {
    process_start_ticks(h.pid) == Some(h.start_ticks)
}

/// A random-enough identifier for this invocation (no credential material involved).
pub fn new_nonce(now: f64) -> String {
    let mut seed = [0u8; 8];
    let got = File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut seed))
        .is_ok();
    let random = if got {
        u64::from_le_bytes(seed)
    } else {
        (now.to_bits()) ^ u64::from(std::process::id()).rotate_left(32)
    };
    format!("{:016x}", random)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gh-paced-state-{tag}-{}-{}",
            std::process::id(),
            new_nonce(0.0)
        ));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        dir
    }

    #[test]
    fn account_names() {
        assert!(valid_account("octocat"));
        assert!(valid_account("some-bot-2"));
        assert!(!valid_account(""));
        assert!(!valid_account("-lead"));
        assert!(!valid_account("../etc"));
        assert!(!valid_account("a.b"));
        assert!(!valid_account(&"a".repeat(40)));
    }

    #[test]
    fn save_load_round_trip_and_permissions() {
        let dir = tmpdir("rt");
        let paths = Paths::new(dir.clone(), "acct");
        let _g = lock(&paths).expect("lock");
        let loaded = load(&paths, 10.0, &|_| BTreeMap::new());
        assert!(loaded.fresh);
        let mut s = loaded.state;
        s.calls_since_refresh = 7;
        save(&paths, &s).expect("save");
        let mode = std::fs::metadata(paths.state())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let again = load(&paths, 11.0, &|_| BTreeMap::new());
        assert!(!again.fresh);
        assert_eq!(again.state.calls_since_refresh, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_state_is_quarantined_with_empty_buckets() {
        let dir = tmpdir("corrupt");
        let paths = Paths::new(dir.clone(), "acct");
        std::fs::write(paths.state(), b"{not json").expect("write");
        let loaded = load(&paths, 1234.0, &|now| {
            let mut m = BTreeMap::new();
            m.insert("write".to_string(), Bucket::empty(now));
            m
        });
        assert!(loaded
            .warning
            .as_deref()
            .is_some_and(|w| w.contains("EMPTY")));
        assert_eq!(loaded.state.buckets["write"].level, 0.0);
        assert!(dir.join("acct.json.corrupt-1234").exists());
        assert!(!paths.state().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn liveness_uses_start_time() {
        let me = std::process::id();
        let ticks = process_start_ticks(me).expect("own stat");
        let h = Holder {
            pid: me,
            start_ticks: ticks,
            nonce: "n".into(),
            class: Class::Write,
            since: 0.0,
        };
        assert!(holder_alive(&h));
        let stale = Holder {
            start_ticks: ticks + 1,
            ..h.clone()
        };
        assert!(!holder_alive(&stale));
        let mut s = State::new();
        s.in_flight = vec![h, stale];
        assert_eq!(s.reap(&holder_alive), 1);
        assert_eq!(s.in_flight.len(), 1);
    }

    #[test]
    fn state_dir_resolution() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(
            state_dir(&env).expect("dir"),
            PathBuf::from("/home/u/.local/state/gh-paced")
        );
        let env = |k: &str| match k {
            "XDG_STATE_HOME" => Some("/x".to_string()),
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(state_dir(&env).expect("dir"), PathBuf::from("/x/gh-paced"));
        let env = |k: &str| match k {
            "GH_PACED_STATE_DIR" => Some("relative".to_string()),
            _ => None,
        };
        assert!(state_dir(&env).is_err());
    }
}
