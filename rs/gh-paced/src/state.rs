//! Per-account pacing state: one JSON file per account, guarded by an exclusive `flock`.
//!
//! Layout under the state directory (default `~/.local/state/gh-paced/`, mode 0700):
//!
//! - `<account>.json`: buckets, hourly windows, in-flight writes, cooldown, rate-limit snapshot;
//! - `<account>.lock`: the lock file every process takes before reading or writing the JSON;
//! - `<account>.audit.jsonl`: the append-only audit log (see `audit`);
//! - `<account>.lease-<nonce>`: one file per running WRITE (see "Write leases" below);
//! - `snap-<nonce>/`: private copies of the body files one running invocation sends.
//!
//! The lock is held only for short read-modify-write steps, never across a sleep or a network
//! call. Saving writes a temporary file and renames it over the old one, so a crash leaves
//! either the old or the new state, and a reader without the lock (`status`) always sees one
//! whole file. A file that cannot be parsed is moved aside to
//! `<account>.json.corrupt-<unix-seconds>` and replaced by the caller's recovery state (the
//! wrapper uses every hourly window full plus a pause, see `wrapper::recovery_state`), with every
//! still-held write lease restored as an in-flight write.
//!
//! # Write leases
//!
//! A WRITE's in-flight slot must last as long as the gh process doing the write, not as long as
//! the wrapper: if the wrapper is killed with SIGKILL, gh keeps running. So an admitted WRITE
//! creates `<account>.lease-<nonce>`, takes an exclusive `flock` on it, and hands that open file
//! to gh (the descriptor is inherited across exec). The kernel keeps the lock while ANY process
//! still has that open file: the wrapper, gh, or anything gh started. A slot is live while its
//! lease is locked; another process tests that with a non-blocking shared `flock` on a fresh open.
//! A nested gh-paced (an alias or extension calling gh) proves it descends from the holder by
//! having the lease file open itself, which only a descendant can (see `inherited_file_ids`).

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
    /// Lease file name (in the state directory) whose lock tracks the slot's real lifetime.
    /// `None` for a refresh claim and for state written by an older version: those fall back to
    /// the PID and start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<String>,
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

    /// Drop in-flight entries and refresh claims whose process has exited. Returns the dropped
    /// in-flight entries, so the caller can remove their lease files.
    pub fn reap(&mut self, alive: &dyn Fn(&Holder) -> bool) -> Vec<Holder> {
        let (live, dead): (Vec<Holder>, Vec<Holder>) = std::mem::take(&mut self.in_flight)
            .into_iter()
            .partition(|h| alive(h));
        self.in_flight = live;
        if let Some(claim) = &self.refresh_claim {
            if !alive(claim) {
                self.refresh_claim = None;
            }
        }
        dead
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

    /// Lease file name for an invocation nonce.
    pub fn lease_name(&self, nonce: &str) -> String {
        format!("{}.lease-{nonce}", self.account)
    }

    /// True when `name` is a lease file name of this account (`<account>.lease-<16 hex>`), so a
    /// name read from the state file cannot point outside the state directory.
    pub fn is_lease_name(&self, name: &str) -> bool {
        name.strip_prefix(&format!("{}.lease-", self.account))
            .is_some_and(|n| n.len() == 16 && n.bytes().all(|b| b.is_ascii_hexdigit()))
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
/// `recovery` builds the state that replaces an unusable file; every lease file that is still
/// locked is added to it as an in-flight write.
pub fn load(paths: &Paths, now: f64, recovery: &dyn Fn(f64) -> State) -> Loaded {
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
            return quarantine(paths, now, recovery, &format!("cannot read: {e}"));
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
            recovery,
            &format!("unsupported version {}", state.version),
        ),
        Err(e) => quarantine(paths, now, recovery, &format!("cannot parse: {e}")),
    }
}

/// Load the account's state for display only: nothing is created, moved, quarantined or written,
/// and no lock is needed (saves replace the file atomically). A missing file is a fresh state;
/// an unusable file is an error.
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

fn quarantine(paths: &Paths, now: f64, recovery: &dyn Fn(f64) -> State, why: &str) -> Loaded {
    let path = paths.state();
    let aside = paths.dir.join(format!(
        "{}.json.corrupt-{}",
        paths.account,
        now.floor() as i64
    ));
    let moved = std::fs::rename(&path, &aside).is_ok();
    let mut state = recovery(now);
    let held = held_leases(paths, now);
    let restored = held.len();
    state.in_flight.extend(held);
    let where_ = if moved {
        format!("moved to {}", aside.display())
    } else {
        "could not be moved aside".to_string()
    };
    Loaded {
        state,
        warning: Some(format!(
            "state file {} is unusable ({why}); {where_}; every hourly budget is treated as \
             used up for the next hour and paced calls pause, and {restored} running write(s) \
             were restored from their lease files",
            path.display()
        )),
        fresh: false,
    }
}

/// Every lease file of this account that is still locked, as in-flight WRITE holders.
fn held_leases(paths: &Paths, now: f64) -> Vec<Holder> {
    let Ok(entries) = std::fs::read_dir(&paths.dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !paths.is_lease_name(&name) || !lease_held(&paths.dir, &name) {
            continue;
        }
        let nonce = name.rsplit('-').next().unwrap_or_default().to_string();
        out.push(Holder {
            pid: 0,
            start_ticks: 0,
            nonce,
            class: Class::Write,
            since: now,
            lease: Some(name),
        });
    }
    out.sort_by(|a, b| a.nonce.cmp(&b.nonce));
    out
}

/// An open, exclusively locked lease file. Dropping it closes this process's copy; the lock
/// lasts until every process that inherited the descriptor has closed it too.
#[derive(Debug)]
pub struct Lease {
    file: File,
    /// File name in the state directory.
    pub name: String,
}

impl Lease {
    /// The descriptor to keep open in the child.
    pub fn fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

/// Create and lock `<account>.lease-<nonce>`. Call while holding [`lock`].
pub fn create_lease(paths: &Paths, nonce: &str) -> Result<Lease, String> {
    let name = paths.lease_name(nonce);
    let path = paths.dir.join(&name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("cannot create lease {}: {e}", path.display()))?;
    // Blocking: the file is new with a random name, so the only other holder can be a prober's
    // momentary shared lock.
    // SAFETY: flock on a valid, owned file descriptor.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("cannot lock lease {}: {err}", path.display()));
    }
    Ok(Lease { file, name })
}

/// Remove a lease file (after its holder finished or was found dead).
pub fn remove_lease(paths: &Paths, name: &str) {
    if paths.is_lease_name(name) {
        let _ = std::fs::remove_file(paths.dir.join(name));
    }
}

/// True when some process still holds the lock on lease file `name` in `dir`. A missing file is
/// not held. Any error other than "would block" counts as held (fewer calls, not more).
pub fn lease_held(dir: &Path, name: &str) -> bool {
    let file = match File::open(dir.join(name)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    // SAFETY: flock on a valid, owned file descriptor; the probe's lock is released when the
    // file is closed at the end of this function.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if rc == 0 {
        return false;
    }
    let err = std::io::Error::last_os_error();
    err.kind() == std::io::ErrorKind::WouldBlock || err.kind() != std::io::ErrorKind::Interrupted
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

/// Liveness of a holder of `paths`' account: its lease lock when it has a valid lease name,
/// otherwise its PID and start time.
pub fn holder_alive_in(paths: &Paths, h: &Holder) -> bool {
    match &h.lease {
        Some(name) if paths.is_lease_name(name) => lease_held(&paths.dir, name),
        _ => holder_alive(h),
    }
}

/// Device and inode of a file.
pub fn file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Device and inode of every regular file this process has open. A lease file appears here only
/// when it was inherited from the wrapper that holds it, which proves descent from that wrapper.
pub fn inherited_file_ids() -> Vec<(u64, u64)> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return Vec::new();
    };
    let fds: Vec<i32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok()))
        .collect();
    let mut out = Vec::new();
    for fd in fds {
        // SAFETY: fstat writes into a local stat buffer; a closed descriptor just fails.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG
        {
            out.push((st.st_dev, st.st_ino));
        }
    }
    out
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
        let loaded = load(&paths, 10.0, &|_| State::new());
        assert!(loaded.fresh);
        let mut s = loaded.state;
        s.calls_since_refresh = 7;
        save(&paths, &s).expect("save");
        let mode = std::fs::metadata(paths.state())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let again = load(&paths, 11.0, &|_| State::new());
        assert!(!again.fresh);
        assert_eq!(again.state.calls_since_refresh, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn saturated_recovery(now: f64) -> State {
        let mut s = State::new();
        let limits = crate::config::ClassLimits {
            per_minute: 2.0,
            burst: 1.0,
            per_hour: 30,
        };
        s.buckets
            .insert("write".to_string(), Bucket::saturated(limits, now));
        s
    }

    /// A corrupt file is moved aside and replaced by the caller's recovery state, with every
    /// still-locked lease restored as an in-flight write (a dead lease is not).
    #[test]
    fn corrupt_state_is_quarantined_conservatively() {
        let dir = tmpdir("corrupt");
        let paths = Paths::new(dir.clone(), "acct");
        let live = create_lease(&paths, "00000000000000aa").expect("lease");
        drop(create_lease(&paths, "00000000000000bb").expect("lease"));
        std::fs::write(dir.join("acct.lease-not-a-nonce"), b"").expect("write");
        std::fs::write(paths.state(), b"{not json").expect("write");
        let loaded = load(&paths, 1234.0, &saturated_recovery);
        let warning = loaded.warning.expect("warning");
        assert!(warning.contains("used up for the next hour"), "{warning}");
        assert!(warning.contains("1 running write(s)"), "{warning}");
        assert_eq!(loaded.state.buckets["write"].level, 0.0);
        assert_eq!(loaded.state.buckets["write"].hour_used(), 30);
        assert_eq!(
            loaded.state.in_flight.len(),
            1,
            "{:?}",
            loaded.state.in_flight
        );
        assert_eq!(
            loaded.state.in_flight[0].lease.as_deref(),
            Some(live.name.as_str())
        );
        assert_eq!(loaded.state.in_flight[0].class, Class::Write);
        assert!(dir.join("acct.json.corrupt-1234").exists());
        assert!(!paths.state().exists());
        drop(live);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lease is held while any open file description of it is open: the creating process's,
    /// or a child's that inherited it. Another open of the same file probes it.
    #[test]
    fn lease_lifetime_follows_the_open_file() {
        let dir = tmpdir("lease");
        let paths = Paths::new(dir.clone(), "acct");
        let lease = create_lease(&paths, "0123456789abcdef").expect("lease");
        assert_eq!(lease.name, "acct.lease-0123456789abcdef");
        let h = Holder {
            pid: 0,
            start_ticks: 0,
            nonce: "0123456789abcdef".into(),
            class: Class::Write,
            since: 0.0,
            lease: Some(lease.name.clone()),
        };
        assert!(lease_held(&dir, &lease.name));
        assert!(
            holder_alive_in(&paths, &h),
            "pid 0 is ignored when a lease exists"
        );
        // A child that inherits the descriptor keeps the lease after this process lets go.
        // SAFETY: duplicating a descriptor this test owns, without close-on-exec.
        let inherited = unsafe { libc::dup(lease.fd()) };
        assert!(inherited >= 0);
        let mut child = std::process::Command::new("sleep")
            .arg("2")
            .spawn()
            .expect("spawn sleep");
        // SAFETY: closing the duplicate in this process; the child keeps its own copy.
        unsafe {
            libc::close(inherited);
        }
        let name = lease.name.clone();
        drop(lease);
        assert!(lease_held(&dir, &name), "the child still holds it");
        child.wait().expect("wait");
        assert!(
            !lease_held(&dir, &name),
            "released when the last holder exits"
        );
        assert!(!holder_alive_in(&paths, &h));
        remove_lease(&paths, &name);
        assert!(!dir.join(&name).exists());
        assert!(!lease_held(&dir, &name), "a missing lease is not held");
        // A name that is not this account's lease falls back to PID liveness.
        let forged = Holder {
            lease: Some("../../etc/passwd".into()),
            ..h
        };
        assert!(!holder_alive_in(&paths, &forged));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inherited_file_ids_include_open_files() {
        let dir = tmpdir("ids");
        let path = dir.join("f");
        std::fs::write(&path, b"x").expect("write");
        let id = file_id(&path).expect("id");
        assert!(!inherited_file_ids().contains(&id));
        let f = File::open(&path).expect("open");
        assert!(inherited_file_ids().contains(&id));
        drop(f);
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
            lease: None,
        };
        assert!(holder_alive(&h));
        let stale = Holder {
            start_ticks: ticks + 1,
            ..h.clone()
        };
        assert!(!holder_alive(&stale));
        let mut s = State::new();
        s.in_flight = vec![h.clone(), stale.clone()];
        assert_eq!(s.reap(&holder_alive), vec![stale]);
        assert_eq!(s.in_flight, vec![h]);
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
