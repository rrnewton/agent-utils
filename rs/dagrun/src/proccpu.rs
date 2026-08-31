//! Boxing-independent CPU-time accounting for a step's process group, read from procfs.
//!
//! # Why this exists
//!
//! The per-step `cpu_timeout` budget is normally enforced from the step's cgroup
//! `cpu.stat` `usage_usec`, which is exact and kernel-accounted. But `cpu_stats` yields
//! `None` whenever boxing is not established, and the scheduler's guard was then simply
//! skipped — the budget was declared and enforced nothing.
//!
//! That is not an exotic corner. A caller passing `--allow-cgroup-failure` (which
//! an originating CI wrapper does unconditionally under `GITHUB_ACTIONS`/`CI`) runs
//! UNBOXED by construction, so on that lane every `cpu_timeout` was inert: measured, a
//! step with `cpu_timeout: 3` burned 60 CPU-seconds and exited green.
//!
//! The 2026-08-03 decision that chose cgroup polling over `RLIMIT_CPU` discounted
//! rlimit's "works unboxed" advantage with *"boxing is default-on, unboxed = opted out of
//! enforcement."* That premise does not hold for a lane where unboxed is the norm rather
//! than an opt-out, so this restores a bound there — WITHOUT displacing the cgroup
//! reading, which remains primary and is strictly better where available.
//!
//! # What it measures, and what it misses
//!
//! Each step is started in its own session, so the step and its descendants share one
//! process group whose pgid equals the step leader's pid. This sums, over every live
//! member of that group:
//!
//! ```text
//!   utime + stime            CPU burned by that process itself
//! + cutime + cstime          CPU of descendants it has already REAPED
//! ```
//!
//! `cutime`/`cstime` roll up recursively on `wait`, and a reaped process is by definition
//! no longer in the live set, so the two terms do not double-count. That makes this an
//! AGGREGATE measure — the property that made cgroup polling win over per-process
//! `RLIMIT_CPU`, and the one that matters for parallel command fan-out.
//!
//! Known gaps, all strictly narrower than "no enforcement at all":
//!
//! * A descendant that calls `setpgid` or `setsid` leaves the process group and stops being
//!   counted. Nonce-aware teardown may still find and kill it, so accounting is weaker than
//!   teardown on this edge.
//! * CPU burned by an exited descendant is invisible before its parent reaps it.
//! * Sampling is at the monitor interval and live snapshots are cached for up to half a
//!   second, so overshoot up to one tick plus that cache window is expected.
//!
//! The reader and its source label are kept behaviorally identical across implementations.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Stable identifier for a reading taken from the step's cgroup `cpu.stat`.
pub const CPU_SOURCE_CGROUP: &str = "cgroup";
/// Stable identifier for a reading taken from the procfs process-group fallback.
pub const CPU_SOURCE_PROCFS: &str = "procfs-subtree";

// `/proc/<pid>/stat` field indices, 0-based, AFTER the comm field has been split off.
// The raw layout is `pid (comm) state ppid pgrp ...`; comm can contain spaces and
// parentheses, so it must be removed by taking the text after the LAST ')' before any
// field splitting. Relative to that remainder, field 0 is `state`:
//   state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt
//     0     1    2      3      4      5     6     7       8       9      10
//   utime stime cutime cstime
//    11    12     13     14
const F_PGRP: usize = 2;
const F_UTIME: usize = 11;
const F_STIME: usize = 12;
const F_CUTIME: usize = 13;
const F_CSTIME: usize = 14;

/// Clock ticks per second (`sysconf(_SC_CLK_TCK)`), the unit of the `stat` CPU fields.
fn clk_tck() -> f64 {
    // SAFETY: `sysconf` is a pure query with no preconditions.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 {
        v as f64
    } else {
        100.0
    }
}

/// Whitespace-split tail of `/proc/<pid>/stat` after the comm field.
///
/// `None` for any process that disappeared mid-scan or whose stat line is malformed. A
/// racing exit is the normal case, not an error: the caller simply does not count a
/// process it cannot read.
fn stat_fields(pid_dir: &Path) -> Option<Vec<String>> {
    let raw = fs::read_to_string(pid_dir.join("stat")).ok()?;
    let close = raw.rfind(')')?;
    let fields: Vec<String> = raw[close + 1..]
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if fields.len() <= F_CSTIME {
        return None;
    }
    Some(fields)
}

/// Aggregate one procfs snapshot by process group.
fn scan_group_ticks(proc_root: &Path) -> Option<HashMap<u32, u64>> {
    let entries = fs::read_dir(proc_root).ok()?;
    let mut groups = HashMap::new();
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) || name.is_empty() {
            continue;
        }
        let Some(fields) = stat_fields(&path) else {
            continue;
        };
        let Ok(pgrp) = fields[F_PGRP].parse::<u32>() else {
            continue;
        };
        let Ok(utime) = fields[F_UTIME].parse::<u64>() else {
            continue;
        };
        let Ok(stime) = fields[F_STIME].parse::<u64>() else {
            continue;
        };
        let Ok(cutime) = fields[F_CUTIME].parse::<u64>() else {
            continue;
        };
        let Ok(cstime) = fields[F_CSTIME].parse::<u64>() else {
            continue;
        };
        let ticks = utime
            .saturating_add(stime)
            .saturating_add(cutime)
            .saturating_add(cstime);
        groups
            .entry(pgrp)
            .and_modify(|sum: &mut u64| *sum = sum.saturating_add(ticks))
            .or_insert(ticks);
    }
    Some(groups)
}

struct Snapshot {
    captured: Instant,
    groups: HashMap<u32, u64>,
}

// All active uncontained steps share one short-lived procfs snapshot. Without this cache, N
// concurrent steps each scan every process once per second: O(N * host processes) monitor work.
// Half a second is below the scheduler's one-second polling interval, so peers waking in the same
// tick share a scan without extending the documented sampling granularity.
const SNAPSHOT_TTL: Duration = Duration::from_millis(500);
static SNAPSHOT: OnceLock<Mutex<Option<Snapshot>>> = OnceLock::new();

/// Lower bound of CPU-seconds observed for process group `pgid` from live procfs.
pub fn subtree_cpu_seconds(pgid: u32) -> Option<f64> {
    if pgid <= 1 {
        return None;
    }
    let now = Instant::now();
    let mut cache = SNAPSHOT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let fresh = cache
        .as_ref()
        .is_some_and(|snapshot| now.duration_since(snapshot.captured) <= SNAPSHOT_TTL);
    if !fresh {
        *cache = scan_group_ticks(Path::new("/proc")).map(|groups| Snapshot {
            captured: Instant::now(),
            groups,
        });
    }
    let ticks = cache.as_ref()?.groups.get(&pgid)?;
    Some(*ticks as f64 / clk_tck())
}

/// [`subtree_cpu_seconds`] against an explicit procfs root, without the live snapshot cache.
pub fn subtree_cpu_seconds_in(pgid: u32, proc_root: &Path) -> Option<f64> {
    if pgid <= 1 {
        return None;
    }
    let groups = scan_group_ticks(proc_root)?;
    let ticks = groups.get(&pgid)?;
    Some(*ticks as f64 / clk_tck())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
        let got = subtree_cpu_seconds_in(100, root).unwrap();
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
        assert_eq!(subtree_cpu_seconds_in(100, tmp.path()), Some(0.0));
        assert_eq!(subtree_cpu_seconds_in(999, tmp.path()), None);
    }

    #[test]
    fn unreadable_or_malformed_procfs_is_unknown() {
        let tmp = TmpRoot::new("malformed");
        assert_eq!(
            subtree_cpu_seconds_in(100, &tmp.path().join("missing")),
            None
        );
        let dir = tmp.path().join("100");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stat"), "malformed").unwrap();
        assert_eq!(subtree_cpu_seconds_in(100, tmp.path()), None);
    }

    #[test]
    fn refuses_degenerate_pgids() {
        // pgid 0 means "my own group" and 1 is init; either would attribute unrelated CPU
        // to the step and reap it spuriously, so both must be refused, not measured.
        assert_eq!(subtree_cpu_seconds(0), None);
        assert_eq!(subtree_cpu_seconds(1), None);
    }
}
