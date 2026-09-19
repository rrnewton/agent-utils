//! Generation-bound, best-effort process-group CPU observations.
//!
//! Each member contributes its original own plus waited-child ticks. All samples precede
//! all lifecycle validations: a child credited to a sampled parent must subsequently be
//! excluded. A terminal pidfd alone is insufficient because an unreaped zombie still owns
//! its CPU. Only a fresh Z state from the *same paired stat FD* preserves that saved row.
//!
//! This is not cgroup accounting: escaped groups and activity between samples can be
//! missed. Ambiguous, missing or resource-limited evidence is unavailable, never zero.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Stable source identifier for kernel cgroup CPU accounting.
pub const CPU_SOURCE_CGROUP: &str = "cgroup";
/// Stable source identifier for best-effort process-group accounting.
pub const CPU_SOURCE_PROCFS: &str = "procfs-subtree";
const MAX_RECORD: usize = 16384;
const MAX_MEMBERS: usize = 1024;
const MAX_GROUPS: usize = 256;
const MAX_ENTRIES: usize = 65536;
const FD_RESERVE: u64 = 64;
const MAX_INTERRUPTS: usize = 16;
const SCAN_TIME: Duration = Duration::from_secs(1);
const SNAPSHOT_TTL: Duration = Duration::from_millis(500);

/// No complete authenticated observation is available; this is distinct from zero CPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable(String);
impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Unavailable {}
impl From<io::Error> for Unavailable {
    fn from(value: io::Error) -> Self {
        Self(value.to_string())
    }
}
fn unavailable(reason: &str) -> Unavailable {
    Unavailable(reason.into())
}
fn check(deadline: Instant) -> Result<(), Unavailable> {
    if Instant::now() >= deadline {
        Err(unavailable("scan deadline"))
    } else {
        Ok(())
    }
}
fn clk_tck() -> f64 {
    // SAFETY: sysconf is a query with no pointer arguments.
    let value = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if value > 0 {
        value as f64
    } else {
        100.0
    }
}
fn gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH))
}
fn allow_fds(required: u64) -> Result<(), Unavailable> {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: limit is writable storage of the type getrlimit requires.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: a successful getrlimit initialized the structure.
    let soft = unsafe { limit.assume_init() }.rlim_cur;
    if soft == libc::RLIM_INFINITY {
        return Ok(());
    }
    let mut used = 0_u64;
    for entry in fs::read_dir("/proc/self/fd")? {
        entry?;
        used += 1;
        if used > MAX_ENTRIES as u64 {
            return Err(unavailable("descriptor census bound"));
        }
    }
    if used.saturating_add(required).saturating_add(FD_RESERVE) > soft {
        return Err(unavailable("descriptor reserve"));
    }
    Ok(())
}
fn read_record(file: &File, deadline: Instant) -> Result<String, Unavailable> {
    read_record_io(file, deadline).map_err(Unavailable::from)
}
fn read_record_io(file: &File, deadline: Instant) -> io::Result<String> {
    let mut data = Vec::with_capacity(1024);
    let mut interrupts = 0;
    let mut block = [0_u8; 1024];
    loop {
        check(deadline).map_err(io::Error::other)?;
        match file.read_at(&mut block, data.len() as u64) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&block[..n]);
                if data.len() > MAX_RECORD {
                    return Err(io::Error::other("oversized proc record"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                interrupts += 1;
                if interrupts > MAX_INTERRUPTS {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}
#[derive(Clone, Debug)]
struct Stat {
    pid: u32,
    group: u32,
    state: char,
    ticks: u64,
}
fn stat(text: &str) -> Result<Stat, Unavailable> {
    let bad = || unavailable("malformed stat");
    let opening = text.find(" (").ok_or_else(bad)?;
    let close = text
        .rfind(')')
        .filter(|close| *close > opening)
        .ok_or_else(bad)?;
    let pid = text[..opening].parse::<u32>().map_err(|_| bad())?;
    let fields: Vec<_> = text[close + 1..].split_whitespace().collect();
    if pid == 0 || fields.len() < 15 || fields[0].len() != 1 {
        return Err(bad());
    }
    let group = fields[2].parse::<u32>().map_err(|_| bad())?;
    let mut ticks = 0_u64;
    for index in [11, 12, 13, 14] {
        ticks = ticks
            .checked_add(fields[index].parse::<u64>().map_err(|_| bad())?)
            .ok_or_else(|| unavailable("CPU overflow"))?;
    }
    Ok(Stat {
        pid,
        group,
        state: fields[0].chars().next().ok_or_else(bad)?,
        ticks,
    })
}
fn fd_pid(text: &str) -> Result<i64, Unavailable> {
    let values: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("Pid:"))
        .collect();
    if values.len() != 1 {
        return Err(unavailable("missing or duplicate pidfd Pid"));
    }
    let value = values[0]
        .trim()
        .parse::<i64>()
        .map_err(|_| unavailable("malformed pidfd Pid"))?;
    if value < -1 || value == 0 {
        return Err(unavailable("invalid pidfd Pid"));
    }
    Ok(value)
}

struct Proc {
    root: File,
}
impl Proc {
    fn new(path: &Path) -> Result<Self, Unavailable> {
        allow_fds(2)?;
        let root = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: root is a live FD and fs is writable statfs storage.
        if unsafe { libc::fstatfs(root.as_raw_fd(), fs.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: successful fstatfs initialized fs.
        if unsafe { fs.assume_init() }.f_type != libc::PROC_SUPER_MAGIC {
            return Err(unavailable("root is not procfs"));
        }
        let proc = Self { root };
        let deadline = Instant::now() + SCAN_TIME;
        let status = read_record(&proc.open("self/status", deadline)?, deadline)?;
        let namespaces: Vec<_> = status
            .lines()
            .filter_map(|line| line.strip_prefix("NSpid:"))
            .map(str::split_whitespace)
            .map(Iterator::collect::<Vec<_>>)
            .collect();
        if namespaces != vec![vec![std::process::id().to_string().as_str()]] {
            return Err(unavailable("procfs PID namespace mismatch"));
        }
        let own = stat(&read_record(&proc.open("self/stat", deadline)?, deadline)?)?;
        if own.pid != std::process::id() {
            return Err(unavailable("procfs PID namespace mismatch"));
        }
        Ok(proc)
    }
    fn open(&self, path: &str, deadline: Instant) -> io::Result<File> {
        let path = CString::new(path).map_err(io::Error::other)?;
        for _ in 0..=MAX_INTERRUPTS {
            check(deadline).map_err(io::Error::other)?;
            // SAFETY: valid held dir FD and NUL-terminated path. Ownership of a successful
            // new descriptor is transferred exactly once to File below.
            let fd = unsafe {
                libc::openat(
                    self.root.as_raw_fd(),
                    path.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd >= 0 {
                return Ok(unsafe { File::from_raw_fd(fd) });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        Err(io::Error::other("open interrupted repeatedly"))
    }
    fn pid(&self, pidfd: &File, deadline: Instant) -> Result<i64, Unavailable> {
        let record = self.open(&format!("self/fdinfo/{}", pidfd.as_raw_fd()), deadline)?;
        fd_pid(&read_record(&record, deadline)?)
    }
    fn pair(&self, pid: u32, deadline: Instant) -> Result<Pair, PairError> {
        check(deadline)?;
        allow_fds(3)?;
        let (pidfd, file, original) = bind_pair(
            pid,
            || open_pidfd(pid, deadline),
            || {
                self.open(&format!("{pid}/stat"), deadline)
                    .map_err(PairError::Io)
            },
            |file| {
                Ok(stat(
                    &read_record_io(file, deadline).map_err(PairError::Io)?,
                )?)
            },
            |pidfd| self.pid(pidfd, deadline).map_err(PairError::from),
        )?;
        Ok(Pair {
            pidfd,
            file,
            original,
        })
    }
}
fn open_pidfd(pid: u32, deadline: Instant) -> Result<File, PairError> {
    for _ in 0..=MAX_INTERRUPTS {
        check(deadline)?;
        // SAFETY: pidfd_open takes scalar arguments and returns a new owned descriptor.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
        if fd >= 0 {
            // SAFETY: syscall returned a new live FD; this is its sole owning File.
            return Ok(unsafe { File::from_raw_fd(fd as i32) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(PairError::Io(error));
        }
    }
    Err(unavailable("pidfd_open interrupted repeatedly").into())
}
fn bind_pair<P, S>(
    pid: u32,
    open_pidfd: impl FnOnce() -> Result<P, PairError>,
    open_stat: impl FnOnce() -> Result<S, PairError>,
    read_stat: impl FnOnce(&S) -> Result<Stat, PairError>,
    fdinfo: impl FnOnce(&P) -> Result<i64, PairError>,
) -> Result<(P, S, Stat), PairError> {
    let pidfd = open_pidfd()?;
    let file = open_stat()?;
    let original = read_stat(&file)?;
    // This fresh fdinfo read follows stat open. If the original generation detached,
    // its pidfd cannot authenticate a replacement PID, even one already in Z state.
    if fdinfo(&pidfd)? != i64::from(pid) || original.pid != pid {
        return Err(unavailable("pidfd/stat generation mismatch").into());
    }
    Ok((pidfd, file, original))
}
enum PairError {
    Io(io::Error),
    Unavailable(Unavailable),
}
impl From<Unavailable> for PairError {
    fn from(value: Unavailable) -> Self {
        Self::Unavailable(value)
    }
}
impl From<PairError> for Unavailable {
    fn from(value: PairError) -> Self {
        match value {
            PairError::Io(e) => e.into(),
            PairError::Unavailable(e) => e,
        }
    }
}
struct Pair {
    pidfd: File,
    file: File,
    original: Stat,
}
trait Observation {
    fn original(&self) -> &Stat;
    fn valid(&self, deadline: Instant) -> Result<bool, Unavailable>;
}
impl Observation for Pair {
    fn original(&self) -> &Stat {
        &self.original
    }
    fn valid(&self, deadline: Instant) -> Result<bool, Unavailable> {
        let mut event = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let mut result = None;
        for _ in 0..=MAX_INTERRUPTS {
            check(deadline)?;
            // SAFETY: event is one initialized, writable pollfd and timeout is zero.
            let count = unsafe { libc::poll(&mut event, 1, 0) };
            if count >= 0 {
                result = Some(count);
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        let count = result.ok_or_else(|| unavailable("poll interrupted repeatedly"))?;
        accept(
            if count == 0 { 0 } else { event.revents },
            &self.original,
            || match read_record_io(&self.file, deadline) {
                Ok(text) => Ok(Some(stat(&text)?)),
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(None),
                Err(error) => Err(error.into()),
            },
        )
    }
}
fn accept(
    bits: i16,
    original: &Stat,
    fresh: impl FnOnce() -> Result<Option<Stat>, Unavailable>,
) -> Result<bool, Unavailable> {
    if bits == 0 {
        return Ok(true);
    }
    if bits & !(libc::POLLIN | libc::POLLHUP | libc::POLLRDNORM) != 0
        || bits & (libc::POLLIN | libc::POLLHUP) == 0
    {
        return Err(unavailable("unexpected pidfd poll result"));
    }
    let Some(current) = fresh()? else {
        return Ok(false);
    };
    if current.pid != original.pid {
        return Err(unavailable("held stat identity changed"));
    }
    match current.state {
        'Z' => Ok(true),
        'X' => Ok(false),
        _ => Err(unavailable("terminal pidfd with ambiguous task state")),
    }
}

trait Source {
    type Row: Observation;
    fn samples(
        &self,
        groups: &HashSet<u32>,
        deadline: Instant,
    ) -> Result<Vec<Self::Row>, Unavailable>;
}
struct KernelSource<'a>(&'a Proc, usize);
impl Source for KernelSource<'_> {
    type Row = Pair;
    fn samples(&self, groups: &HashSet<u32>, deadline: Instant) -> Result<Vec<Pair>, Unavailable> {
        let mut rows = Vec::new();
        let entries = fs::read_dir(format!("/proc/self/fd/{}", self.0.root.as_raw_fd()))?;
        for (index, entry) in entries.enumerate() {
            check(deadline)?;
            if index >= MAX_ENTRIES {
                return Err(unavailable("proc entry bound"));
            }
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let pid = name
                .parse::<u32>()
                .map_err(|_| unavailable("invalid proc PID"))?;
            let hint = match self
                .0
                .open(&format!("{pid}/stat"), deadline)
                .and_then(|file| read_record_io(&file, deadline))
            {
                Ok(text) => stat(&text)?,
                Err(error) if gone(&error) => continue,
                Err(error) => return Err(error.into()),
            };
            if !groups.contains(&hint.group) {
                continue;
            }
            if rows.len() >= self.1 {
                return Err(unavailable("member bound"));
            }
            let pair = match self.0.pair(pid, deadline) {
                Ok(pair) => pair,
                Err(PairError::Io(error)) if gone(&error) => continue,
                Err(error) => return Err(error.into()),
            };
            if groups.contains(&pair.original.group) {
                rows.push(pair);
            }
        }
        Ok(rows)
    }
}
fn scan<S: Source>(
    source: &S,
    groups: &HashSet<u32>,
    deadline: Instant,
) -> Result<HashMap<u32, u64>, Unavailable> {
    let rows = source.samples(groups, deadline)?;
    // Global phase barrier: never validate a chunk before later original samples.
    let mut totals = HashMap::<u32, u64>::new();
    for row in rows {
        check(deadline)?;
        if row.valid(deadline)? {
            let original = row.original();
            let value = totals.entry(original.group).or_default();
            *value = value
                .checked_add(original.ticks)
                .ok_or_else(|| unavailable("aggregate overflow"))?;
        }
    }
    Ok(totals)
}

struct Owner {
    key: u64,
    pgid: u32,
    proc: Proc,
    pair: Pair,
}
impl Owner {
    fn authenticate(&self, deadline: Instant) -> Result<(), Unavailable> {
        if self.proc.pid(&self.pair.pidfd, deadline)? != i64::from(self.pgid) {
            return Err(unavailable("owner generation gone"));
        }
        let current = stat(&read_record(&self.pair.file, deadline)?)?;
        if current.pid != self.pgid || current.group != self.pgid {
            return Err(unavailable("owner identity changed"));
        }
        if !self.pair.valid(deadline)? {
            return Err(unavailable("owner dead"));
        }
        Ok(())
    }
}
struct Snapshot {
    started: Instant,
    captured: Instant,
    keys: HashSet<u64>,
    values: Result<HashMap<u64, u64>, Unavailable>,
}
#[derive(Default)]
struct Registry {
    next: u64,
    owners: HashMap<u64, Weak<Owner>>,
    compatibility: HashMap<u32, (Arc<Owner>, Instant)>,
    snapshot: Option<Snapshot>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}
impl Registry {
    fn create(&mut self, pgid: u32, path: &Path) -> Result<Arc<Owner>, Unavailable> {
        self.create_bounded(pgid, path, MAX_GROUPS)
    }
    fn create_bounded(
        &mut self,
        pgid: u32,
        path: &Path,
        max_groups: usize,
    ) -> Result<Arc<Owner>, Unavailable> {
        if !(2..=i32::MAX as u32).contains(&pgid) {
            return Err(unavailable("invalid process group"));
        }
        // Either public API can reclaim expired compatibility registrations.
        self.compatibility
            .retain(|_, (_, used)| used.elapsed() <= SNAPSHOT_TTL);
        self.owners.retain(|_, owner| owner.strong_count() > 0);
        if self.owners.len() >= max_groups {
            return Err(unavailable("owner bound"));
        }
        let proc = Proc::new(path)?;
        let pair = proc
            .pair(pgid, Instant::now() + SCAN_TIME)
            .map_err(Unavailable::from)?;
        if pair.original.group != pgid {
            return Err(unavailable("owner is not the process-group leader"));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| unavailable("owner key exhausted"))?;
        let owner = Arc::new(Owner {
            key: self.next,
            pgid,
            proc,
            pair,
        });
        self.owners.insert(owner.key, Arc::downgrade(&owner));
        Ok(owner)
    }
    fn seconds(&mut self, owner: &Owner) -> Result<f64, Unavailable> {
        let deadline = Instant::now() + SCAN_TIME;
        owner.authenticate(deadline)?;
        let fresh = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.captured.elapsed() <= SNAPSHOT_TTL && snapshot.keys.contains(&owner.key)
        });
        if !fresh {
            let owners: Vec<_> = self.owners.values().filter_map(Weak::upgrade).collect();
            let active: Vec<_> = owners
                .iter()
                .filter(|entry| entry.authenticate(deadline).is_ok())
                .collect();
            let groups = active.iter().map(|entry| entry.pgid).collect();
            let started = Instant::now();
            let values =
                scan(&KernelSource(&owner.proc, MAX_MEMBERS), &groups, deadline).map(|totals| {
                    let mut values = HashMap::new();
                    for entry in active {
                        // Authenticate again: the root may have detached during the scan.
                        if entry.authenticate(deadline).is_ok() {
                            if let Some(ticks) = totals.get(&entry.pgid) {
                                values.insert(entry.key, *ticks);
                            }
                        }
                    }
                    values
                });
            self.snapshot = Some(Snapshot {
                started,
                captured: Instant::now(),
                keys: owners.iter().map(|entry| entry.key).collect(),
                values,
            });
        }
        owner.authenticate(deadline)?;
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| unavailable("missing snapshot"))?;
        if snapshot.captured.duration_since(snapshot.started) > SCAN_TIME {
            return Err(unavailable("scan deadline"));
        }
        let ticks = snapshot
            .values
            .as_ref()
            .map_err(Clone::clone)?
            .get(&owner.key)
            .ok_or_else(|| unavailable("no measured members"))?;
        Ok(*ticks as f64 / clk_tck())
    }
}

/// Owned invocation identity. Create at spawn and retain until its monitor stops.
/// This object never rebinds to a later occupant of the same numeric PID/PGID.
pub struct ProcessGroupCpu {
    owner: Arc<Owner>,
}
impl ProcessGroupCpu {
    /// Authenticate the process-group leader and register for shared sampling.
    pub fn new(pgid: u32) -> Result<Self, Unavailable> {
        let mut registry = registry()
            .lock()
            .map_err(|_| unavailable("poisoned CPU registry"))?;
        Ok(Self {
            owner: registry.create(pgid, Path::new("/proc"))?,
        })
    }
    /// Checked CPU seconds, or typed unavailability rather than a partial/zero sum.
    pub fn seconds(&self) -> Result<f64, Unavailable> {
        registry()
            .lock()
            .map_err(|_| unavailable("poisoned CPU registry"))?
            .seconds(&self.owner)
    }
}

/// Compatibility projection. New monitors should retain [`ProcessGroupCpu`] instead.
pub fn subtree_cpu_seconds(pgid: u32) -> Option<f64> {
    if !(2..=i32::MAX as u32).contains(&pgid) {
        return None;
    }
    let mut registry = registry().lock().ok()?;
    registry
        .compatibility
        .retain(|_, (_, used)| used.elapsed() <= SNAPSHOT_TTL);
    let cached = registry
        .compatibility
        .get(&pgid)
        .map(|(owner, _)| Arc::clone(owner));
    let owner = match cached {
        Some(owner) if owner.authenticate(Instant::now() + SCAN_TIME).is_ok() => owner,
        _ => {
            registry.compatibility.remove(&pgid);
            registry.create(pgid, Path::new("/proc")).ok()?
        }
    };
    registry
        .compatibility
        .insert(pgid, (Arc::clone(&owner), Instant::now()));
    registry.seconds(&owner).ok()
}

/// Uncached observation from an explicit authentic procfs/PID view, not synthetic stat files.
pub fn subtree_cpu_seconds_in(pgid: u32, proc_root: &Path) -> Option<f64> {
    if !(2..=i32::MAX as u32).contains(&pgid) {
        return None;
    }
    let mut local = Registry::default();
    let owner = local.create(pgid, proc_root).ok()?;
    local.seconds(&owner).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A throwaway procfs root. Deliberately NOT `tempfile`: adding a dev-dependency would
    /// need a registry fetch, and this test only needs a unique directory it removes itself.
    struct TmpRoot(PathBuf);
    impl TmpRoot {
        fn new(label: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "proccpu-{}-{}-{}",
                label,
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TmpRoot(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A synthetic procfs proves the field offsets and the aggregation rule without
    /// depending on live process timing. The layout deliberately includes a comm with a
    /// space and a ')' in it, which is the exact case a naive whitespace split gets wrong.
    fn write_stat(root: &Path, pid: u32, comm: &str, pgrp: u32, cpu: [u64; 4]) {
        let dir = root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        let mut f = vec![format!("{pid}"), format!("({comm})"), "R".into()];
        f.push("1".into()); // ppid
        f.push(pgrp.to_string()); // pgrp
        for _ in 0..6 {
            f.push("0".into()); // session..cmajflt fillers
        }
        f.push("0".into());
        f.push("0".into());
        f.push(cpu[0].to_string()); // utime
        f.push(cpu[1].to_string()); // stime
        f.push(cpu[2].to_string()); // cutime
        f.push(cpu[3].to_string()); // cstime
        fs::write(dir.join("stat"), f.join(" ")).unwrap();
    }

    #[test]
    fn sums_only_the_named_group_and_includes_reaped_children() {
        let tmp = TmpRoot::new("group");
        let root = tmp.path();
        // Two members of the target group, plus one process in a DIFFERENT group whose CPU
        // must not be attributed to the step.
        write_stat(root, 100, "leader", 100, [10, 5, 20, 5]);
        write_stat(root, 101, "child (x)", 100, [30, 0, 0, 0]);
        write_stat(root, 200, "stranger", 200, [9999, 9999, 9999, 9999]);
        let got = synthetic_seconds(100, root).unwrap();
        // (10+5+20+5) + (30) = 70 ticks
        assert!(
            (got - 70.0 / clk_tck()).abs() < 1e-9,
            "expected the two group members' own+reaped CPU and nothing from the stranger, got {got}"
        );
    }

    #[test]
    fn zero_is_a_reading_but_absence_is_unknown() {
        let tmp = TmpRoot::new("absent");
        write_stat(tmp.path(), 100, "leader", 100, [0, 0, 0, 0]);
        assert_eq!(synthetic_seconds(100, tmp.path()), Some(0.0));
        assert_eq!(synthetic_seconds(999, tmp.path()), None);
    }

    #[test]
    fn unreadable_or_malformed_procfs_is_unknown() {
        let tmp = TmpRoot::new("malformed");
        assert_eq!(synthetic_seconds(100, &tmp.path().join("missing")), None);
        let dir = tmp.path().join("100");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stat"), "malformed").unwrap();
        assert_eq!(synthetic_seconds(100, tmp.path()), None);
    }

    #[test]
    fn refuses_degenerate_pgids() {
        // pgid 0 means "my own group" and 1 is init; either would attribute unrelated CPU
        // to the step and reap it spuriously, so both must be refused, not measured.
        assert_eq!(subtree_cpu_seconds(0), None);
        assert_eq!(subtree_cpu_seconds(1), None);
    }
    struct Row {
        original: Stat,
        bits: i16,
        fresh: Result<Option<Stat>, Unavailable>,
    }
    impl Observation for Row {
        fn original(&self) -> &Stat {
            &self.original
        }
        fn valid(&self, _deadline: Instant) -> Result<bool, Unavailable> {
            accept(self.bits, &self.original, || self.fresh.clone())
        }
    }
    struct Files<'a>(&'a Path);
    impl Source for Files<'_> {
        type Row = Row;
        fn samples(
            &self,
            groups: &HashSet<u32>,
            _deadline: Instant,
        ) -> Result<Vec<Row>, Unavailable> {
            let mut rows = Vec::new();
            for entry in fs::read_dir(self.0)? {
                let original = stat(&fs::read_to_string(entry?.path().join("stat"))?)?;
                if groups.contains(&original.group) {
                    rows.push(Row {
                        original,
                        bits: 0,
                        fresh: Ok(None),
                    });
                }
            }
            Ok(rows)
        }
    }
    // Fixture adaptation only: fake lifecycle observations feed the same two-phase
    // aggregation. Live APIs never authenticate an ordinary directory as procfs.
    fn synthetic_seconds(pgid: u32, root: &Path) -> Option<f64> {
        scan(
            &Files(root),
            &HashSet::from([pgid]),
            Instant::now() + SCAN_TIME,
        )
        .ok()?
        .get(&pgid)
        .map(|ticks| *ticks as f64 / clk_tck())
    }

    #[test]
    fn shared_canonical_lifecycle_corpus() {
        struct Corpus<'a>(&'a serde_json::Value);
        impl Source for Corpus<'_> {
            type Row = Row;
            fn samples(
                &self,
                groups: &HashSet<u32>,
                _deadline: Instant,
            ) -> Result<Vec<Row>, Unavailable> {
                self.0["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|raw| groups.contains(&(raw["group"].as_u64().unwrap() as u32)))
                    .map(|raw| {
                        let original = Stat {
                            pid: raw["pid"].as_u64().unwrap() as u32,
                            group: raw["group"].as_u64().unwrap() as u32,
                            state: 'R',
                            ticks: raw["ticks"].as_u64().unwrap(),
                        };
                        let fresh = if let Some(error) = raw["error"].as_str() {
                            Err(unavailable(error))
                        } else if raw["state"] == "gone" {
                            Ok(None)
                        } else {
                            Ok(Some(Stat {
                                pid: raw["fresh_pid"].as_u64().unwrap_or(u64::from(original.pid))
                                    as u32,
                                group: original.group,
                                state: raw["state"].as_str().unwrap().chars().next().unwrap(),
                                ticks: raw["fresh_ticks"].as_u64().unwrap_or(0),
                            }))
                        };
                        Ok(Row {
                            original,
                            bits: raw["poll"].as_i64().unwrap() as i16,
                            fresh,
                        })
                    })
                    .collect()
            }
        }
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/proccpu-generation.json"))
                .unwrap();
        for case in cases.as_array().unwrap() {
            let result = scan(
                &Corpus(case),
                &HashSet::from([100]),
                Instant::now() + SCAN_TIME,
            );
            if case["error"] == true {
                assert!(result.is_err(), "{}", case["name"]);
            } else {
                assert_eq!(
                    result.unwrap().get(&100).copied(),
                    case["expected"].as_u64(),
                    "{}",
                    case["name"]
                );
            }
            println!("canonical CPU case: {}", case["name"]);
        }
    }

    #[test]
    fn all_samples_precede_every_validation() {
        struct OrderedRow {
            row: Stat,
            events: Rc<RefCell<Vec<&'static str>>>,
        }
        impl Observation for OrderedRow {
            fn original(&self) -> &Stat {
                &self.row
            }
            fn valid(&self, _deadline: Instant) -> Result<bool, Unavailable> {
                assert_eq!(&self.events.borrow()[..3], &["sample", "sample", "sample"]);
                self.events.borrow_mut().push("validate");
                Ok(true)
            }
        }
        struct OrderedSource(Rc<RefCell<Vec<&'static str>>>);
        impl Source for OrderedSource {
            type Row = OrderedRow;
            fn samples(
                &self,
                _groups: &HashSet<u32>,
                _deadline: Instant,
            ) -> Result<Vec<OrderedRow>, Unavailable> {
                Ok((0..3)
                    .map(|index| {
                        self.0.borrow_mut().push("sample");
                        OrderedRow {
                            row: Stat {
                                pid: 100 + index,
                                group: 100,
                                state: 'R',
                                ticks: 1,
                            },
                            events: Rc::clone(&self.0),
                        }
                    })
                    .collect())
            }
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        assert_eq!(
            scan(
                &OrderedSource(Rc::clone(&events)),
                &HashSet::from([100]),
                Instant::now() + SCAN_TIME
            )
            .unwrap()[&100],
            3
        );
        assert_eq!(
            *events.borrow(),
            ["sample", "sample", "sample", "validate", "validate", "validate"]
        );
    }

    #[test]
    fn stat_fdinfo_and_synthetic_root_refusals() {
        for text in ["Pid: -1\nPid: 100\n", "Pid: 0", "Pid: nope", ""] {
            assert!(fd_pid(text).is_err());
        }
        assert_eq!(fd_pid("Pid: -1\n").unwrap(), -1);
        assert_eq!(fd_pid("Pid: 100\n").unwrap(), 100);
        for counter in ["-1", "18446744073709551616"] {
            assert!(stat(&format!(
                "100 (x) R 1 100 {}{counter} 0 0 0",
                "0 ".repeat(8)
            ))
            .is_err());
        }
        let tmp = TmpRoot::new("not-procfs");
        write_stat(tmp.path(), 100, "leader", 100, [0, 0, 0, 0]);
        assert_eq!(subtree_cpu_seconds_in(100, tmp.path()), None);
    }

    struct Native {
        child: std::process::Child,
        output: std::io::BufReader<std::process::ChildStdout>,
        _root: TmpRoot,
    }
    impl Native {
        fn start(mode: &str) -> Self {
            use std::os::unix::process::CommandExt;
            use std::process::{Command, Stdio};
            let root = TmpRoot::new(mode);
            let source = root.path().join("helper.c");
            let binary = root.path().join("helper");
            fs::write(&source, include_str!("../tests/fixtures/proccpu-helper.c")).unwrap();
            assert!(Command::new("cc")
                .args(["-Wall", "-Wextra", "-Werror", "-pthread"])
                .arg(source)
                .arg("-o")
                .arg(&binary)
                .status()
                .unwrap()
                .success());
            let mut child = Command::new(binary)
                .arg(mode)
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let output = std::io::BufReader::new(child.stdout.take().unwrap());
            Self {
                child,
                output,
                _root: root,
            }
        }
        fn line(&mut self) -> String {
            use std::io::BufRead;
            let mut event = libc::pollfd {
                fd: self.output.get_ref().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: event describes one live owned pipe; finite wait only in the native test.
            assert_eq!(
                unsafe { libc::poll(&mut event, 1, 5000) },
                1,
                "native fixture output deadline"
            );
            let mut line = String::new();
            self.output.read_line(&mut line).unwrap();
            line.trim().into()
        }
        fn send(&mut self, byte: u8) {
            use std::io::Write;
            self.child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&[byte])
                .unwrap();
            self.child.stdin.as_mut().unwrap().flush().unwrap();
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            // Only the fixture process started by this object is signalled/reaped.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn native_unreaped_zombie_and_reaping_preserve_cpu() {
        let mut child = Native::start("zombie");
        let zombie = child.line().parse::<u32>().unwrap();
        let reader = ProcessGroupCpu::new(child.child.id()).unwrap();
        let pair = reader
            .owner
            .proc
            .pair(zombie, Instant::now() + SCAN_TIME)
            .map_err(Unavailable::from)
            .unwrap();
        assert_eq!(pair.original.state, 'Z');
        assert!(pair.original.ticks > 0);
        assert!(pair.valid(Instant::now() + SCAN_TIME).unwrap());
        let before = reader.seconds().unwrap();
        assert!(before >= pair.original.ticks as f64 / clk_tck());
        child.send(b'r');
        assert_eq!(child.line(), "reaped");
        assert!(!pair.valid(Instant::now() + SCAN_TIME).unwrap());
        registry().lock().unwrap().snapshot = None;
        assert!(reader.seconds().unwrap() >= before);
        child.send(b'x');
    }

    #[test]
    fn native_exited_leader_with_live_worker_remains_measurable() {
        let mut child = Native::start("leader");
        assert_eq!(child.line(), "worker");
        let reader = ProcessGroupCpu::new(child.child.id()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let current = stat(&read_record(&reader.owner.pair.file, deadline).unwrap()).unwrap();
            if current.state == 'Z' {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(reader.owner.pair.valid(deadline).unwrap());
        let before = reader.seconds().unwrap();
        child.send(b'b');
        assert_eq!(child.line(), "worked");
        registry().lock().unwrap().snapshot = None;
        assert!(reader.seconds().unwrap() > before);
        child.send(b'x');
    }

    #[test]
    fn invocation_keys_and_shared_snapshot_do_not_rebind() {
        let mut child = Native::start("zombie");
        let _ = child.line();
        let mut registry = Registry::default();
        let first = registry
            .create(child.child.id(), Path::new("/proc"))
            .unwrap();
        let second = registry
            .create(child.child.id(), Path::new("/proc"))
            .unwrap();
        assert_ne!(first.key, second.key);
        let reading = registry.seconds(&first).unwrap();
        let captured = registry.snapshot.as_ref().unwrap().captured;
        assert_eq!(registry.seconds(&second).unwrap(), reading);
        assert_eq!(registry.snapshot.as_ref().unwrap().captured, captured);
        child.send(b'r');
        assert_eq!(child.line(), "reaped");
        child.send(b'x');
        assert!(child.child.wait().unwrap().success());
        assert!(
            registry.seconds(&first).is_err(),
            "cached reading requires a still-attached owner generation"
        );
        assert!(registry.seconds(&second).is_err());
    }
    #[test]
    fn scanner_member_bound_and_cached_error_are_unavailable() {
        let mut child = Native::start("zombie");
        let _ = child.line();
        let mut registry = Registry::default();
        let owner = registry
            .create(child.child.id(), Path::new("/proc"))
            .unwrap();
        let bounded = scan(
            &KernelSource(&owner.proc, 1),
            &HashSet::from([owner.pgid]),
            Instant::now() + SCAN_TIME,
        );
        assert_eq!(bounded.unwrap_err(), unavailable("member bound"));
        // A stored refusal is reused as a refusal; it cannot become a real zero or a
        // successful scan merely because a second monitor consumes the shared snapshot.
        registry.snapshot = Some(Snapshot {
            started: Instant::now(),
            captured: Instant::now(),
            keys: HashSet::from([owner.key]),
            values: Err(unavailable("injected scanner EACCES")),
        });
        assert_eq!(
            registry.seconds(&owner).unwrap_err(),
            unavailable("injected scanner EACCES")
        );
        assert_eq!(
            registry.seconds(&owner).unwrap_err(),
            unavailable("injected scanner EACCES")
        );
        for key in 0..MAX_GROUPS as u64 {
            registry.owners.insert(key, Arc::downgrade(&owner));
        }
        assert!(
            matches!(registry.create(child.child.id(), Path::new("/proc")), Err(error) if error == unavailable("owner bound"))
        );
        child.send(b'r');
        assert_eq!(child.line(), "reaped");
        child.send(b'x');
    }

    #[test]
    fn pid_t_range_is_refused_by_owned_and_compatibility_apis() {
        for pid in [0, 1, i32::MAX as u32 + 1, u32::MAX] {
            assert!(
                matches!(ProcessGroupCpu::new(pid), Err(error) if error == unavailable("invalid process group"))
            );
            assert_eq!(subtree_cpu_seconds(pid), None);
            assert_eq!(subtree_cpu_seconds_in(pid, Path::new("/missing")), None);
        }
    }

    #[test]
    fn new_owned_registration_evicts_expired_compatibility_owner() {
        let mut child = Native::start("zombie");
        let _ = child.line();
        let mut registry = Registry::default();
        let old = registry
            .create(child.child.id(), Path::new("/proc"))
            .unwrap();
        let old_key = old.key;
        registry.seconds(&old).unwrap();
        registry
            .compatibility
            .insert(old.pgid, (old, Instant::now() - Duration::from_secs(1)));
        let new = registry
            .create_bounded(child.child.id(), Path::new("/proc"), 1)
            .unwrap();
        assert_ne!(new.key, old_key);
        assert!(registry.compatibility.is_empty());
        assert_eq!(registry.owners.len(), 1);
        assert!(registry.seconds(&new).unwrap() >= 0.0);
        child.send(b'r');
        assert_eq!(child.line(), "reaped");
        child.send(b'x');
    }

    #[test]
    fn pairing_order_rejects_replacement_zombie_and_drops_both_handles() {
        struct Handle {
            name: &'static str,
            events: Rc<RefCell<Vec<&'static str>>>,
        }
        impl Drop for Handle {
            fn drop(&mut self) {
                self.events.borrow_mut().push(self.name);
            }
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        let result = bind_pair(
            100,
            || {
                events.borrow_mut().push("pidfd-open");
                Ok(Handle {
                    name: "drop-pidfd",
                    events: Rc::clone(&events),
                })
            },
            || {
                events.borrow_mut().push("stat-open");
                Ok(Handle {
                    name: "drop-stat",
                    events: Rc::clone(&events),
                })
            },
            |_| {
                events.borrow_mut().push("stat-read");
                Ok(Stat {
                    pid: 100,
                    group: 100,
                    state: 'Z',
                    ticks: 999,
                })
            },
            |_| {
                events.borrow_mut().push("fdinfo");
                Ok(-1)
            },
        );
        assert!(
            matches!(result, Err(PairError::Unavailable(error)) if error == unavailable("pidfd/stat generation mismatch"))
        );
        assert_eq!(
            *events.borrow(),
            [
                "pidfd-open",
                "stat-open",
                "stat-read",
                "fdinfo",
                "drop-stat",
                "drop-pidfd"
            ]
        );
    }

    #[test]
    fn native_nonleader_exec_preserves_held_identity() {
        let mut child = Native::start("exec");
        assert_eq!(child.line(), "thread");
        let reader = ProcessGroupCpu::new(child.child.id()).unwrap();
        let key = reader.owner.key;
        let before = reader.seconds().unwrap();
        child.send(b'e');
        assert_eq!(child.line(), "executed");
        let deadline = Instant::now() + SCAN_TIME;
        assert_eq!(
            reader
                .owner
                .proc
                .pid(&reader.owner.pair.pidfd, deadline)
                .unwrap(),
            i64::from(child.child.id())
        );
        assert_eq!(
            stat(&read_record(&reader.owner.pair.file, deadline).unwrap())
                .unwrap()
                .pid,
            child.child.id()
        );
        assert!(reader.owner.pair.valid(deadline).unwrap());
        child.send(b'b');
        assert_eq!(child.line(), "worked");
        registry().lock().unwrap().snapshot = None;
        let after = reader.seconds().unwrap();
        assert!(after > before);
        assert_eq!(reader.owner.key, key);
        println!(
            "native nonleader exec pid={} owner={key} before_cpu={before} after_cpu={after}",
            child.child.id()
        );
        child.send(b'x');
        assert!(child.child.wait().unwrap().success());
    }

    #[test]
    fn native_ptrace_reparent_keeps_zombie_until_real_parent_reaps() {
        let mut child = Native::start("trace");
        let tracee = child.line().parse::<u32>().unwrap();
        let reader = ProcessGroupCpu::new(child.child.id()).unwrap();
        let pair = reader
            .owner
            .proc
            .pair(tracee, Instant::now() + SCAN_TIME)
            .map_err(Unavailable::from)
            .unwrap();
        let waited = || {
            let text = read_record(&reader.owner.pair.file, Instant::now() + SCAN_TIME).unwrap();
            let fields: Vec<_> = text
                .rsplit_once(')')
                .unwrap()
                .1
                .split_whitespace()
                .collect();
            fields[13].parse::<u64>().unwrap() + fields[14].parse::<u64>().unwrap()
        };
        assert_eq!(pair.original.state, 'Z');
        assert!(pair.original.ticks > 0);
        assert!(pair.valid(Instant::now() + SCAN_TIME).unwrap());
        assert!(fs::read_to_string(format!("/proc/{tracee}/status"))
            .unwrap()
            .contains(&format!("TracerPid:\t{}\n", child.child.id())));
        let before_waited = waited();
        let before = reader.seconds().unwrap();
        child.send(b't');
        assert_eq!(child.line(), "detached");
        assert!(fs::read_to_string(format!("/proc/{tracee}/status"))
            .unwrap()
            .contains("TracerPid:\t0\n"));
        assert_eq!(
            waited(),
            before_waited,
            "ptracer EXIT_TRACE must not credit child CPU"
        );
        assert!(pair.valid(Instant::now() + SCAN_TIME).unwrap());
        assert_eq!(
            stat(&read_record(&pair.file, Instant::now() + SCAN_TIME).unwrap())
                .unwrap()
                .state,
            'Z'
        );
        child.send(b'r');
        assert_eq!(child.line(), "reaped");
        assert!(!pair.valid(Instant::now() + SCAN_TIME).unwrap());
        registry().lock().unwrap().snapshot = None;
        let after = reader.seconds().unwrap();
        assert!(after >= before);
        println!("native ptrace reparent pid={} tracee={tracee} zombie_ticks={} ptracer_waited={before_waited} before_cpu={before} after_cpu={after}", child.child.id(), pair.original.ticks);
        child.send(b'x');
        assert!(child.child.wait().unwrap().success());
    }
}
