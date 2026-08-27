//! Dependency-aware, resource-aware concurrent DAG execution.

// The DAG runner: greedy, memory-/resource-aware step scheduling.
//
// Port of the OBSERVABLE scheduling behavior of `py/dagrun/scheduler.py` for the
// no-boxing default path (Python's `cgroups=None`). Reproduced from the reference:
//
// * Greedy ready-set loop: each pass launches every ready step (deps satisfied, resources free,
//   and within the active-step limit, in longest-processing-time order) on its own supervisor
//   thread, then sleeps briefly. Per-step widths may overcommit the outer CPU bandwidth.
// * Dependency gating + dep-FAILURE skip-closure (a failed dep transitively skips dependents).
// * Named-resource capacity buckets (`hint.resources` vs `cfg.resource_caps`).
// * Longest-processing-time (LPT) dispatch order (descending `est_duration_s`, stable).
// * Per-step supervision via `bash -c` in its own process group (whole-tree teardown).
// * Fail-fast (eager-exit): by default the first genuine failure stops launching NEW steps and
//   eager-cancels in-flight steps (labelled ABORTED, not FAILED). `keep_going` instead continues
//   launching independent ready steps while dependency-failure closure skips only true
//   dependents.
// * Failure classification via [`crate::model::step_failure_reason`].
//
// Boxing: when a [`crate::cgroup::CgroupManager`] is supplied (the default `run` path), each
// step is wrapped so its bash leader self-moves into a per-step child cgroup with an inner
// `memory.max` cap. Teardown gives every process group one bounded SIGTERM diagnostic window,
// then writes the step's `cgroup.kill` (a setsid-proof atomic SIGKILL of the whole subtree) and
// follows with killpg as a belt-and-suspenders. Without a manager the step runs unboxed and uses
// process-group plus best-effort `/proc` ownership sweeps. Per-step measurement rows are collected
// for the perf-log sink either way.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::panic::AssertUnwindSafe;
use std::process::ExitStatus;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::ambient::{capture_ambient_snapshot, PsiReading};
use crate::attribution::{
    bind_process_tests, capture_max_bytes, capture_truncation_notice, default_log_dir,
    mint_step_nonce, process_snapshot, recognize, Culprit, RunEvidence, StepStream, TestEvent,
    STEP_NONCE_ENV,
};
use crate::cgroup::CgroupManager;
use crate::model::{
    canonical_cpu_timeout, command_with_inner_jobs, effective_cpu_count, effective_cpu_timeout,
    env_with_inner_jobs, graph_structure_violations, preferred_inner_jobs, resolved_wall_timeout,
    scale_cpu_timeout, step_classification, step_width_is_resizable, undeclared_resource_demands,
    validate_jobs_env_config, write_domain_violations, DagConfig, RunResult, Step, StepOutcome,
    JOBS_ENV_ENV,
};
use crate::proccpu::{subtree_cpu_seconds, CPU_SOURCE_CGROUP, CPU_SOURCE_PROCFS};
use crate::profile_enrich::{resolve_effective_inner_jobs, step_enrichment_columns};
use crate::resource_caps::{Acquire as SharedResourceAcquire, Coordinator as ResourceCoordinator};

/// A per-step measurement row (column -> value), matching the perflog step-profile schema.
type ProfileRow = BTreeMap<String, String>;

/// Final cgroup CPU counters for the durable step journal, with their units in the key names.
///
/// These are the readings already taken BEFORE `cleanup()` removes the step's cgroup, so they
/// cost nothing extra and are the last CPU figures that will ever exist for the step. They belong
/// in the journal for the same reason the terminal record exists at all: a hard kill destroys the
/// end-of-run profile flush, and then the journal is the only thing left that can say what the
/// step consumed against the budget it was given.
///
/// A missing input map, or a kernel that does not publish one of these counters, stays ABSENT. It
/// must not become a measured zero — that is the same substitution the CPU guard used to make.
fn cpu_journal_fields(cpu_stats: Option<&BTreeMap<String, i64>>) -> Vec<(&'static str, String)> {
    let Some(cpu_stats) = cpu_stats else {
        return Vec::new();
    };
    [
        ("usage_usec", "cpu_usage_usec"),
        ("nr_throttled", "cpu_nr_throttled"),
        ("throttled_usec", "cpu_throttled_usec"),
    ]
    .into_iter()
    .filter_map(|(source, journal_key)| {
        cpu_stats
            .get(source)
            .map(|value| (journal_key, value.to_string()))
    })
    .collect()
}

/// Consumed user+system CPU-seconds from a step's cgroup `cpu.stat`, or `None` when the counter
/// is ABSENT.
///
/// ABSENT IS NOT ZERO. A missing `usage_usec` means the step's CPU cannot be MEASURED, not that
/// it has consumed none. Reading it as 0 made the budget comparison permanently unsatisfiable, so
/// a declared CPU-time budget silently enforced nothing — an enforcement guard switched off by a
/// missing field, with no warning anywhere. `None` forces the caller to say so instead.
fn cpu_seconds_from_stats(stats: &BTreeMap<String, i64>) -> Option<f64> {
    stats
        .get("usage_usec")
        .map(|usec| *usec as f64 / 1_000_000.0)
}

/// Optional per-step cgroup manager shared (behind an `Arc`) across the run's supervisor threads.
pub type BoxedCgroups = Option<Arc<dyn CgroupManager>>;

/// Monotonic start epoch of the enclosing DAG step, serialized for nested consumers.
///
/// A nested timeout cannot safely start a fresh clock after its own setup: doing so makes a
/// numerically smaller timeout capable of outliving the enclosing step.  The runner owns the
/// actual step clock, so it exports that clock's epoch before spawning the child.  Consumers add
/// their own (smaller) allowance to this value and therefore keep one ordering across execs.
pub const STEP_STARTED_MONOTONIC_NS_ENV: &str = "DAGRUN_STEP_STARTED_MONOTONIC_NS";

/// Read Linux's process-independent monotonic clock in nanoseconds.
///
/// `Instant` cannot be serialized through an exec.  `CLOCK_MONOTONIC` is the same boot-scoped
/// clock class and remains stable across fork/exec without inheriting wall-clock adjustments.
pub fn monotonic_now_ns() -> Option<u64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0
        || ts.tv_sec < 0
        || ts.tv_nsec < 0
    {
        return None;
    }
    (ts.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(ts.tv_nsec as u64))
}

/// Per-step monitor poll interval (seconds) for descendant-thread-peak sampling.
const MONITOR_INTERVAL: Duration = Duration::from_secs(1);
/// How often a still-running step reports how far it has got.
///
/// 30s is chosen against the measured shape of a real run rather than by taste: on one real
/// graph 26 of 56 nodes exceed 30s and the longest is 182s, so every silent phase reports at
/// least once and the worst reports five times. Shorter would add lines to the 30 nodes that
/// finish quickly without telling anyone anything new.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);

// Scheduler idle interval between ready-set sweeps (matches Python's `time.sleep(0.05)`).
const LOOP_SLEEP: Duration = Duration::from_millis(50);
/// Poll interval while a step's supervisor waits for the child (std has no wait-with-timeout).
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How many times [`kill_descendants`] re-walks `/proc` before giving up. The walk races a tree
/// that may still be forking, so one pass can miss a child born mid-sweep; a small fixed bound
/// converts a pathological forker into a reported failure instead of an unbounded loop.
const DESCENDANT_KILL_SWEEPS: usize = 4;
/// Grace after SIGTERM so an inner runner can identify the test it was executing before SIGKILL.
/// The behavioral differential pins this duration across package implementations.
const REAP_TERM_GRACE: Duration = Duration::from_secs(5);
const REAP_TERM_POLL: Duration = Duration::from_millis(100);
/// Latches once the unboxed-teardown warning has been emitted, so it is stated once per run
/// instead of once per step.
static UNBOXED_REAP_WARNED: AtomicBool = AtomicBool::new(false);
/// How long to wait for a timed-out child to actually die after [`reap`] before giving up on it.
///
/// The wait after a kill USED TO BE UNBOUNDED. If the kill could not reach the process — the exact
/// unboxed case above — the run blocked here forever: no timeout was reported, the scheduler never
/// returned, and the end-of-run profile flush never happened, so the lane produced no evidence
/// about its own failure. Bounding it means an unkillable child degrades to a REPORTED failure
/// whatever the reason the kill failed, including reasons this code does not anticipate.
const POST_REAP_WAIT: Duration = Duration::from_secs(30);
/// How long to wait for the monitor and output-reader threads after teardown before abandoning
/// them. See [`join_bounded`].
const JOIN_WAIT: Duration = Duration::from_secs(15);

// Mutable scheduler state, guarded by one lock (mirrors the Python `Runner`'s single lock).
struct Shared {
    done: HashMap<String, StepOutcome>,
    running: HashSet<String>,
    /// tag -> the in-flight child's pid (== its process-group id), so a sibling that FAILS can
    /// eager-reap it without sharing the `Child` handle across threads.
    running_pids: HashMap<String, u32>,
    /// Tags killed by eager-exit (labelled ABORTED, not FAIL).
    aborted: HashSet<String>,
    /// Remaining capacity per named scarce resource.
    resource_avail: HashMap<String, i64>,
    /// A genuine (non-aborted) step failed.
    failed: bool,
    /// Stop scheduling new steps after a fail-fast failure or outer run timeout.
    stop: bool,
    /// Accumulated per-step measurement rows (forwarded to a metrics sink after the run).
    step_profile_rows: Vec<ProfileRow>,
    /// Per-step ownership nonce, so the eager-cancel path can terminate another step's escapees
    /// as thoroughly as that step's own supervisor would (see [`kill_by_nonce`]).
    running_nonces: HashMap<String, String>,
    /// tag -> the tags whose failure that step exists to explain, copied from the graph so the
    /// eager-cancel path can consult it without holding the step map. Empty for every ordinary
    /// step; only diagnostics appear with a non-empty list.
    explains_index: HashMap<String, Vec<String>>,
    /// tag -> its declared fail-fast family. Missing tags preserve global eager-exit.
    fail_fast_family_index: HashMap<String, String>,
    /// Families whose own failure has stopped their remaining work.
    failed_families: HashSet<String>,
    /// The WHOLE RUN exceeded its outer wall budget and cut its in-flight steps short.
    run_timed_out: bool,
    /// Child processes currently alive, and the largest count observed during this run.
    active_processes: usize,
    max_concurrent_steps: usize,
    /// Tags whose child lifetime is counted into `active_processes` and still awaiting its
    /// matching uncount. A supervisor that dies between the two would otherwise leave the count
    /// permanently inflated, and `max_concurrent_steps` is a max over it.
    counted_processes: HashSet<String>,
    /// Tags whose admission-time accounting (named resources, running/pids/nonces) has already
    /// been handed back. See [`retire`].
    retired: HashSet<String>,
}

/// Signal a process group without consulting `PATH`.
///
/// The step leader is its own group leader via `process_group(0)`, so a negative pid targets the
/// complete group.  Calling `kill(2)` directly is load-bearing: using an unqualified external
/// `kill` made teardown replaceable by a caller-controlled `PATH` entry.
fn signal_group(pid: u32, signal: i32) -> bool {
    let Ok(pgid) = i32::try_from(pid) else {
        return false;
    };
    if pgid <= 1 {
        return false;
    }
    // SAFETY: `getpgrp` takes no arguments and reads process metadata only.
    let own_group = unsafe { libc::getpgrp() };
    if pgid == own_group {
        return false;
    }
    // SAFETY: a negative pid is the documented kill(2) process-group form; no pointers are used.
    unsafe { libc::kill(-pgid, signal) == 0 }
}

/// Signal one positive pid without consulting `PATH`.
fn signal_pid(pid: u32, signal: i32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 1 || pid == i32::try_from(std::process::id()).unwrap_or(-1) {
        return false;
    }
    // SAFETY: `pid` is a checked positive process id and kill(2) dereferences no pointers.
    unsafe { libc::kill(pid, signal) == 0 }
}

/// A live process's group from one Linux `/proc/PID/stat` record.
///
/// The parenthesized command may contain spaces and parentheses, so fields are interpreted only
/// after its final `)`. Zombies deliberately return `None`: their group leader cannot be
/// wait-reaped until the supervisor regains control, and treating it as live would spend the whole
/// diagnostic grace after a cooperative SIGTERM exit.
fn live_process_group_from_stat(stat: &str) -> Option<u32> {
    let close = stat.rfind(')')?;
    // Fields after comm begin at field 3: state, ppid, pgrp.
    let mut fields = stat[close + 1..].split_whitespace();
    let (Some(state), Some(_ppid), Some(pgrp)) = (fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    if state == "Z" {
        return None;
    }
    pgrp.parse::<u32>().ok()
}

/// Requested process groups containing at least one non-zombie process.
///
/// `kill(-pgid, 0)` succeeds for an unreaped zombie group leader. The supervisor cannot reap that
/// child until teardown returns, so a signal-only probe charges the entire grace even when the
/// child honored SIGTERM immediately. One `/proc` walk handles a whole cancellation batch.
fn live_process_groups(groups: &HashSet<u32>) -> Option<HashSet<u32>> {
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut live = HashSet::new();
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some(pgrp) = live_process_group_from_stat(&stat) else {
            continue;
        };
        if groups.contains(&pgrp) {
            live.insert(pgrp);
            if live.len() == groups.len() {
                break;
            }
        }
    }
    Some(live)
}

/// Let several process groups report their in-flight work under ONE shared grace.
fn terminate_groups(pids: &[u32]) {
    let mut active: HashSet<u32> = pids
        .iter()
        .copied()
        .filter(|pid| signal_group(*pid, libc::SIGTERM))
        .collect();
    let deadline = Instant::now() + REAP_TERM_GRACE;
    while !active.is_empty() && Instant::now() < deadline {
        active = live_process_groups(&active).unwrap_or_else(|| {
            active
                .iter()
                .copied()
                .filter(|pid| signal_group(*pid, 0))
                .collect()
        });
        if !active.is_empty() {
            thread::sleep(REAP_TERM_POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

/// Every live descendant of `root`, deepest-first, from `/proc` parentage.
///
/// Reads `PPid:` out of `/proc/<pid>/status` rather than field 4 of `/proc/<pid>/stat`, because a
/// process name can contain spaces and parentheses and positional parsing of `stat` mis-attributes
/// parentage for exactly the adversarial names a test corpus is most likely to contain.
///
/// Deepest-first matters: killing a parent before its children can leave the children reparented
/// to init and out of reach on the next sweep.
fn proc_descendants(root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue; // not a pid directory
        };
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            continue; // exited between readdir and read: benign
        };
        if let Some(ppid) = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(pid);
        }
    }
    // Iterative DFS, then reverse, so parents are signalled after their children.
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut seen: HashSet<u32> = HashSet::new();
    while let Some(pid) = stack.pop() {
        for &child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child) {
                out.push(child);
                stack.push(child);
            }
        }
    }
    out.reverse();
    out
}

/// SIGKILL every descendant of `root`, sweeping until no new ones appear.
///
/// This is the fallback for the UNBOXED path, where there is no `cgroup.kill` to clear a subtree
/// atomically. A process-group kill alone misses `setsid`/double-fork escapees — an escapee changes
/// session and pgid but stays a descendant — and the strict-compat lane demonstrably has them. A
/// budget that detects without terminating is indistinguishable from no budget at all.
///
/// Sweeps repeatedly because the walk races a tree that may still be forking; it stops when a sweep
/// finds nothing new or after [`DESCENDANT_KILL_SWEEPS`], so a pathological forker degrades to a
/// bounded, reported failure rather than an unbounded loop.
///
/// SAFETY: the set is derived strictly by parentage from `root`, never from a name or command-line
/// pattern, so it cannot reach a sibling process belonging to somebody else. `root` itself is left
/// to the caller's process-group kill; this only reaches things that escaped it. Returns the number
/// of distinct pids signalled.
fn kill_descendants(root: u32) -> usize {
    if root <= 1 {
        return 0; // never walk from init, and never from a bogus pid
    }
    let own = std::process::id();
    let mut killed: HashSet<u32> = HashSet::new();
    for _ in 0..DESCENDANT_KILL_SWEEPS {
        let mut fresh = 0usize;
        for pid in proc_descendants(root) {
            if pid <= 1 || pid == own {
                continue; // a reap must never signal the runner itself
            }
            if killed.insert(pid) {
                fresh += 1;
            }
            let _ = signal_pid(pid, libc::SIGKILL);
        }
        if fresh == 0 {
            break;
        }
    }
    killed.len()
}

/// SIGKILL processes carrying this step's exact ownership nonce in their environment.
///
/// This is the BEST-EFFORT ESCAPEE CLOSER for an unboxed run. A process-group kill misses a child
/// that called `setsid`, and a parentage walk misses a double-fork survivor after it reparents.
/// Ordinary descendants inherit `DAGRUN_STEP=<nonce>` through `fork`/`execve`, so the exact
/// NUL-delimited environment entry can still associate those environment-preserving escapees
/// with their step. It is never a process-name, command-line, or substring match.
///
/// LIMIT: a hostile child can unset or replace its environment before escaping. The nonce is an
/// ownership aid, not a security boundary and not a substitute for cgroup containment. Processes
/// whose environment is unreadable are skipped, and the runner's own pid is always excluded.
///
/// Sweeps like [`kill_descendants`], for the same reason: the walk races a tree that may still be
/// forking. Returns the number of distinct pids signalled.
fn kill_by_nonce(nonce: &str) -> usize {
    if nonce.is_empty() {
        return 0;
    }
    let needle = format!("{STEP_NONCE_ENV}={nonce}");
    let own = std::process::id();
    let mut killed: HashSet<u32> = HashSet::new();
    for _ in 0..DESCENDANT_KILL_SWEEPS {
        let mut fresh = 0usize;
        let Ok(entries) = std::fs::read_dir("/proc") else {
            break;
        };
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid <= 1 || pid == own {
                continue;
            }
            // Unreadable (another user, or exited mid-walk) is the common case and is benign.
            let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
                continue;
            };
            let carries = environ
                .split(|b| *b == 0)
                .any(|entry| entry == needle.as_bytes());
            if !carries {
                continue;
            }
            if killed.insert(pid) {
                fresh += 1;
            }
            let _ = signal_pid(pid, libc::SIGKILL);
        }
        if fresh == 0 {
            break;
        }
    }
    killed.len()
}

/// Wait for an already-reaped child to exit, giving up after `limit`.
///
/// Returns the real exit status when the child dies in time, and a fabricated killed status when it
/// does not. Reporting a killed status for a process that is still alive is deliberate and is the
/// lesser evil: the alternative is the scheduler blocking indefinitely, which reports NOTHING and
/// loses the whole run's measurements along with it. The survivor is named so the leak is visible
/// rather than inferred.
fn wait_bounded(child: &mut std::process::Child, tag: &str, limit: Duration) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st,
            Ok(None) => {
                if Instant::now() >= deadline {
                    eprintln!(
                        "[scheduler] WARNING: step {tag} did not exit within {}s of being killed; \
                         abandoning the wait and reporting it as killed. Its process tree may \
                         still be running.",
                        limit.as_secs()
                    );
                    return ExitStatus::from_raw(9);
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return ExitStatus::from_raw(9),
        }
    }
}

/// Join a worker thread, abandoning it if it does not finish within `limit`.
///
/// `JoinHandle::join` cannot time out, so bounding it means polling `is_finished` and simply not
/// joining a thread that overruns. The thread stays parked until the process exits; that is a
/// deliberate, bounded leak chosen over losing the whole run's results to an indefinite block.
fn join_bounded(handle: thread::JoinHandle<()>, tag: &str, what: &str, limit: Duration) {
    let deadline = Instant::now() + limit;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    if handle.is_finished() {
        let _ = handle.join();
    } else {
        eprintln!(
            "[scheduler] WARNING: step {tag}: {what} thread still blocked {}s after teardown \
             (a surviving process is likely holding the pipe open); abandoning it so the run can \
             finish and report.",
            limit.as_secs()
        );
    }
}

fn deps_ok(sh: &Shared, step: &Step) -> bool {
    step.deps
        .iter()
        .all(|d| sh.done.get(d).map(|o| o.ok).unwrap_or(false))
}

fn deps_known(sh: &Shared, step: &Step) -> bool {
    step.deps.iter().all(|d| sh.done.contains_key(d))
}

fn res_free(sh: &Shared, step: &Step) -> bool {
    // An ABSENT cap is treated as 0, i.e. never schedulable, deliberately: silently granting
    // unlimited capacity to an undeclared resource would turn a config typo into an unbounded
    // fan-out. The two cases are identical HERE (both block) but must never be identical in the
    // diagnostics -- see `ungrantable_resources`, which renders `<absent>` distinctly so the
    // reader can tell "you forgot to declare it" from "you set it to zero on purpose".
    step.hint
        .resources
        .iter()
        .all(|(r, n)| sh.resource_avail.get(r).copied().unwrap_or(0) >= *n)
}

/// What a capacity lookup actually found. `Absent` is a DISTINCT variant, never rendered as a
/// value, because `unwrap_or(0)` FUSES "not declared" with "declared as 0" and that fusion is the
/// whole defect class here: an undeclared `resource_caps` entry reads identically to a deliberate
/// zero-capacity bucket, so a config typo and a deliberate serialization are indistinguishable in
/// the diagnostics as well as in the behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observed {
    Absent,
    Present(String),
}

impl std::fmt::Display for Observed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Observed::Absent => write!(f, "<absent>"),
            Observed::Present(v) => write!(f, "{v}"),
        }
    }
}

/// One refused condition, carrying enough to be fixed WITHOUT opening the source: where it was
/// found, what was required, what was actually observed, and the surrounding declarations that
/// turn a refusal into a spotted typo.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    site: String,
    required: String,
    observed: Observed,
    context: Vec<String>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: requires {}, but got {}",
            self.site, self.required, self.observed
        )?;
        if !self.context.is_empty() {
            write!(f, " (declared: {})", self.context.join(", "))?;
        }
        Ok(())
    }
}

/// The starved steps whose demand LIVE capacity can never grant, as refusals.
///
/// Safe to read `resource_avail` as DECLARED capacity only because the caller invokes it with
/// `running` EMPTY: every `acquire` is matched by a `release` when its step completes, so with
/// nothing running the map has returned to the configured caps. Returns empty when the starve has
/// some other cause (dangling dep, dependency cycle) -- the detector still refuses; this only
/// supplies the named cause when the cause is capacity.
///
/// Reports EVERY violation rather than the first: a first-failure abort makes the reader iterate
/// N times for N typos, and the count is itself evidence of how wide the misdeclaration is.
fn ungrantable_resources(
    resource_avail: &HashMap<String, i64>,
    steps: &HashMap<String, Step>,
    tags: &[String],
) -> Vec<Refusal> {
    let mut declared: Vec<String> = resource_avail
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    declared.sort();
    let mut out = Vec::new();
    for tag in tags {
        let Some(step) = steps.get(tag) else { continue };
        for (r, n) in &step.hint.resources {
            let cap = resource_avail.get(r).copied();
            if cap.unwrap_or(0) < *n {
                out.push(Refusal {
                    site: format!("step {tag:?}"),
                    required: format!("{r}={n}"),
                    observed: match cap {
                        None => Observed::Absent,
                        Some(c) => Observed::Present(format!("{r}={c}")),
                    },
                    context: declared.clone(),
                });
            }
        }
    }
    out
}

/// The one line an UNCONTAINED run owes its operator about per-step CPU-time budgets.
///
/// Without a cgroup, the exact `cpu.stat` counter is unavailable. The runner instead samples a
/// best-effort procfs process-group floor. It can reap an ordinary over-budget process tree, but
/// it misses processes that leave the group and CPU from exited descendants until reaped. The
/// warning must name that weaker guarantee rather than equate it with cgroup accounting.
///
/// Returns `None` when no step carries a live budget, so a graph that has genuinely disabled the
/// guard everywhere is not nagged about a bound it never asked for.
pub fn uncontained_cpu_budget_warning(cfg: &DagConfig) -> Option<String> {
    let live: Vec<i64> = cfg
        .steps
        .iter()
        .map(|step| {
            effective_cpu_timeout(
                step,
                cfg.default_step_cpu_timeout,
                cfg.cpu_timeout_multiplier,
            )
        })
        .filter(|budget| *budget > 0)
        .collect();
    let largest = live.iter().copied().max()?;
    Some(format!(
        "UNCONTAINED run: exact cgroup cpu.stat accounting is unavailable; a best-effort \
         procfs process-group CPU floor will police {} step(s) (largest {largest}s), but it can \
         miss processes that leave the group and not-yet-reaped exits. `capabilities` cannot \
         express that quality difference.",
        live.len()
    ))
}

/// Return a run configuration whose declared per-step CPU widths cannot exceed `max_cpus`.
///
/// This is intentionally visible: a caller-authored width that the run budget changes must not
/// look as though it executed unchanged. The top-level undeclared-step cpu.max default is capped
/// too, even though no jobs flag is appended for an undeclared command.
/// A declared over-budget width with neither a jobs flag nor jobs-env channel is left unchanged:
/// the runner cannot rewrite that guest's width, and [`validate_max_cpus_rewrite`] makes execution
/// fail closed instead of falsely claiming the width was lowered.
pub fn cap_config_max_cpus(cfg: &DagConfig, max_cpus: i64) -> DagConfig {
    crate::model::assert_valid_jobs_env_config(cfg);
    let max_cpus = max_cpus.max(1);
    let mut capped = cfg.clone();
    if let Some(default) = capped
        .default_step_cpu_count
        .filter(|default| *default > max_cpus)
    {
        eprintln!(
            "[scheduler] WARNING: default_step_cpu_count={default} exceeds the run total CPU-core \
             budget --max-cpus {max_cpus}; capping undeclared steps' per-step cpu.max to \
             {max_cpus}"
        );
        capped.default_step_cpu_count = Some(max_cpus);
    }
    let default_jobs_flag = capped.default_jobs_flag.clone();
    let default_jobs_env = capped.default_jobs_env.clone();
    for step in &mut capped.steps {
        if let Some(width) = step
            .hint
            .preferred_inner_jobs
            .filter(|width| *width > max_cpus)
        {
            if !step_width_is_resizable(step, &default_jobs_flag, &default_jobs_env) {
                continue;
            }
            eprintln!(
                "[scheduler] WARNING: step {} preferred_inner_jobs={width} exceeds the run total \
                 CPU-core budget --max-cpus {max_cpus}; capping its guest width and per-step \
                 cpu.max to {max_cpus}",
                step.tag()
            );
            step.hint.preferred_inner_jobs = Some(max_cpus);
        }
    }
    capped
}

/// Validate that every declared width above `max_cpus` can be rewritten in the guest command.
///
/// A step with neither width channel owns its concurrency internally. Lowering only its scheduling
/// hint and cgroup quota would leave the original worker count running inside a smaller CPU box,
/// so callers must refuse before any step starts.
pub(crate) fn validate_max_cpus_rewrite(cfg: &DagConfig, max_cpus: i64) -> Result<(), String> {
    validate_jobs_env_config(cfg)?;
    let max_cpus = max_cpus.max(1);
    let mut bad: Vec<(String, i64)> = cfg
        .steps
        .iter()
        .filter_map(|step| {
            if step.skip_reason.is_some() {
                return None;
            }
            let width = step.hint.preferred_inner_jobs?;
            (width > max_cpus
                && !step_width_is_resizable(step, &cfg.default_jobs_flag, &cfg.default_jobs_env))
            .then(|| (step.tag(), width))
        })
        .collect();
    bad.sort_by(|a, b| a.0.cmp(&b.0));
    if bad.is_empty() {
        return Ok(());
    }
    let detail = bad
        .iter()
        .map(|(tag, width)| format!("{tag} (preferred_inner_jobs={width})"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "--max-cpus {max_cpus} cannot lower guest parallelism for step(s) that offer no width \
         channel: {detail}; this machine must declare one -- set ${JOBS_ENV_ENV} to the guest's \
         worker-count ENV VAR (e.g. CARGO_BUILD_JOBS), or set the step's jobs_flag to its \
         worker-count OPTION -- or reduce preferred_inner_jobs, or raise --max-cpus"
    ))
}

/// Compatibility alias for [`cap_config_max_cpus`].
#[doc(hidden)]
#[deprecated(since = "0.13.1", note = "use cap_config_max_cpus")]
pub fn cap_config_cpu_jobs(cfg: &DagConfig, max_cpus: i64) -> DagConfig {
    cap_config_max_cpus(cfg, max_cpus)
}

fn acquire(sh: &mut Shared, step: &Step) {
    for (r, n) in &step.hint.resources {
        *sh.resource_avail.entry(r.clone()).or_insert(0) -= n;
    }
}

fn release(sh: &mut Shared, step: &Step) {
    for (r, n) in &step.hint.resources {
        *sh.resource_avail.entry(r.clone()).or_insert(0) += n;
    }
}

/// Lock the shared scheduler state, RECOVERING from poisoning rather than panicking again.
///
/// A supervisor thread that panics while holding this lock poisons it, and every other
/// `lock().unwrap()` in the run then panics too. That buries the ORIGINAL cause under a cascade of
/// secondary panics in threads that did nothing wrong, and takes the run down with no attributable
/// reason -- the unattributable, wedge-shaped failure #80 runner-supervisor-crash-loud exists to
/// eliminate. The state behind this lock is a set of maps and counters with no cross-field
/// invariant that a mid-update panic can leave unrecoverably half-applied (and the once-only
/// [`retire`] guard is precisely what keeps the accounting consistent across such a panic), so
/// continuing with the data as it stands is strictly better than a cascade.
fn lock_shared(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Stop counting `tag`'s child toward `active_processes`, AT MOST ONCE.
///
/// Idempotent for the same reason [`retire`] is: the normal path uncounts as soon as the child is
/// reaped, well before the step retires, so a supervisor that dies in between must be able to
/// uncount without double-counting a step that already did.
fn uncount_process(sh: &mut Shared, tag: &str) {
    if sh.counted_processes.remove(tag) {
        sh.active_processes = sh.active_processes.saturating_sub(1);
    }
}

/// Hand back everything `step`'s admission took, EXACTLY ONCE. Returns whether this call did it.
///
/// Every release site -- spawn failure, normal completion, and the supervisor-panic paths added
/// for #80 runner-supervisor-crash-loud -- gives back the same named-resource counts, and a panic
/// landing AFTER a normal release would otherwise release a second time. That drifts
/// `resource_avail` ABOVE its declared cap, which is worse than the leak it resembles: the cap
/// silently stops being a cap and the next run's over-admission has no visible cause.
fn retire(sh: &mut Shared, step: &Step) -> bool {
    let tag = step.tag();
    if !sh.retired.insert(tag.clone()) {
        return false;
    }
    sh.running.remove(&tag);
    sh.running_pids.remove(&tag);
    sh.running_nonces.remove(&tag);
    uncount_process(sh, &tag);
    release(sh, step);
    true
}

/// Tags whose own run FAILED, excluding steps that were merely cancelled.
///
/// An aborted step is not evidence of anything, so it must not trigger a diagnostic's exemption;
/// only a real failure does. Mirrored by the sibling edition's scheduler.
fn genuinely_failed(sh: &Shared) -> std::collections::HashSet<String> {
    sh.done
        .iter()
        .filter(|(_, outcome)| !outcome.ok && !outcome.aborted)
        .map(|(tag, _)| tag.clone())
        .collect()
}

/// Whether `tag` is a diagnostic for one of the failures in `failed`.
///
/// The ONLY thing that survives eager-exit, and deliberately conditional: a step declaring
/// `explains` is reaped like any other peer unless one of the specific nodes it names has
/// genuinely failed, so the exemption cannot be used as a blanket opt-out. Mirrored by the
/// sibling edition's scheduler.
fn is_exempt_from_eager_exit(
    sh: &Shared,
    tag: &str,
    failed: &std::collections::HashSet<String>,
) -> bool {
    sh.explains_index
        .get(tag)
        .is_some_and(|explains| explains.iter().any(|t| failed.contains(t)))
}

/// Whether global or family-scoped eager-exit blocks `tag` from starting.
fn blocked_by_fail_fast(sh: &Shared, tag: &str) -> bool {
    sh.stop
        || sh
            .fail_fast_family_index
            .get(tag)
            .is_some_and(|family| sh.failed_families.contains(family))
}

/// Record the run as failed and cut the matching fail-fast scope short.
///
/// Shared by the ordinary step-failure path, the spawn-failure path and the supervisor-panic
/// paths so all three cancel peers identically. A step without a family preserves the existing
/// global eager-exit; a step with one cancels only peers in that family. Dependency closure
/// separately excludes true dependents, while unrelated families continue.
fn trip_fail_fast(sh: &mut Shared, cgroups: &BoxedCgroups, keep_going: bool, failed_tag: &str) {
    sh.failed = true;
    if keep_going {
        return;
    }
    match sh.fail_fast_family_index.get(failed_tag).cloned() {
        Some(family) => {
            sh.failed_families.insert(family);
        }
        None => sh.stop = true,
    }
    // A node that exists to EXPLAIN this failure is spared. Reaping it destroys the only account
    // of why the run failed, which is the opposite of what eager-exit is for: the point is to
    // stop paying for work that cannot matter, and the diagnosis is the one piece of remaining
    // work that matters most. Everything else in the matching scope is cut short.
    let failed = genuinely_failed(sh);
    let candidates: HashSet<String> = sh
        .running
        .iter()
        .filter(|tag| blocked_by_fail_fast(sh, tag))
        .cloned()
        .collect();
    let spared: HashSet<String> = candidates
        .iter()
        .filter(|tag| is_exempt_from_eager_exit(sh, tag, &failed))
        .cloned()
        .collect();
    for other in &candidates {
        if !spared.contains(other) {
            sh.aborted.insert(other.clone());
        }
    }
    let others: Vec<(String, u32, Option<String>)> = sh
        .running_pids
        .iter()
        .filter(|(tag, _)| candidates.contains(*tag) && !spared.contains(*tag))
        .map(|(tag, pid)| (tag.clone(), *pid, sh.running_nonces.get(tag).cloned()))
        .collect();
    reap_many(cgroups, &others);
}

/// Give a step whose supervisor died a TERMINAL outcome, so the run can finish.
///
/// Returns `true` when this call published the outcome, `false` when the step already had one (a
/// panic in the reporting tail, after `done` was written, is a real bug but not a wedge: the run
/// can still terminate and must not be told the step failed twice).
struct SupervisorFailure {
    /// The step's `reason`, which MUST name the cause; "something went wrong" is the state this
    /// whole guard exists to eliminate.
    reason: String,
    summary: String,
    elapsed_s: f64,
}

fn publish_supervisor_failure(
    shared: &Mutex<Shared>,
    cgroups: &BoxedCgroups,
    evidence: &Option<Arc<RunEvidence>>,
    step: &Step,
    keep_going: bool,
    failure: SupervisorFailure,
) -> bool {
    let SupervisorFailure {
        reason,
        summary,
        elapsed_s,
    } = failure;
    let tag = step.tag();
    {
        let mut sh = lock_shared(shared);
        if sh.done.contains_key(&tag) {
            return false;
        }
        retire(&mut sh, step);
        sh.done.insert(
            tag.clone(),
            StepOutcome {
                tag: tag.clone(),
                ok: false,
                duration_s: elapsed_s,
                summary,
                executed_tests: None,
                filtered_tests: None,
                returncode: None,
                reason: reason.clone(),
                aborted: false,
            },
        );
        trip_fail_fast(&mut sh, cgroups, keep_going, &tag);
    }
    if let Some(e) = evidence {
        // THE REASON IS THE RECORD. The whole point of the reason string is that it NAMES the
        // cause -- which panic, or that the thread vanished with none -- and a run reconstructed
        // from the journal alone is exactly the case where nobody has the console output that
        // said so. An event that carries only the tag and a duration reports that something went
        // wrong without saying what, which is the state this guard exists to eliminate. The
        // sibling engine writes the same three fields under the same event name.
        e.record(
            "supervisor_crash",
            &[
                ("step", tag),
                ("reason", reason),
                ("elapsed_s", format!("{elapsed_s:.3}")),
            ],
        );
    }
    true
}

/// Render a `catch_unwind` payload as text. A panic whose cause is not NAMED is barely better
/// than the silence this guard replaced.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic payload of an unprintable type".to_string()
    }
}

/// Everything the panic-recovery path needs after the supervisor's own context is gone.
struct SupervisorRecovery {
    step: Step,
    shared: Arc<Mutex<Shared>>,
    cgroups: BoxedCgroups,
    evidence: Option<Arc<RunEvidence>>,
    keep_going: bool,
}

/// Run one supervisor `body`, converting ANY panic into a NAMED step failure.
///
/// LAYER ONE of the two guards behind #80 runner-supervisor-crash-loud. Before it existed,
/// exactly one failure mode inside the supervisor was handled (`spawn` returning `Err`) and any
/// other panic unwound off the worker thread: the tag stayed in `running` with nothing in `done`,
/// so the ready-set loop could never reach its break condition. The run then produced no outcome,
/// no exit and no attributable cause -- a wedge that looks exactly like work in progress, which
/// is the worst thing this tool can do.
///
/// The panic is re-reported, never swallowed: the message goes to BOTH stdout (where the step's
/// own output is) and stderr (where a CI system looks), and the step's `reason` NAMES it. The
/// default panic hook has already printed the payload and its location by the time we get here.
///
/// `AssertUnwindSafe` is a judgement, not a shrug: the only state crossing the boundary is behind
/// `Mutex<Shared>`, whose poisoning is recovered by [`lock_shared`] and whose accounting is made
/// panic-safe by the once-only [`retire`].
fn with_supervisor_guard<F: FnOnce()>(recovery: SupervisorRecovery, body: F) {
    let start = Instant::now();
    let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(body)) else {
        return;
    };
    let tag = recovery.step.tag();
    let detail = panic_detail(payload.as_ref());
    let header = format!(
        "[{tag}] \u{2717} SUPERVISOR CRASHED: the supervisor thread for this step panicked \
         ({detail}). The step's own result is UNKNOWN; it is being reported as FAILED so the run \
         cannot wedge. This is a runner bug, not a step failure."
    );
    emit(&header);
    eprintln!("{header}");
    publish_supervisor_failure(
        &recovery.shared,
        &recovery.cgroups,
        &recovery.evidence,
        &recovery.step,
        recovery.keep_going,
        SupervisorFailure {
            reason: format!("SUPERVISOR CRASHED ({detail})"),
            summary: detail,
            elapsed_s: start.elapsed().as_secs_f64(),
        },
    );
}

/// Serialize one status line to stdout (each `println!` is atomic, so lines never interleave).
fn emit(line: &str) {
    println!("{line}");
}

/// Lines [`warn`] has written, recorded only in test builds. See [`warn`].
#[cfg(test)]
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// One diagnostic line to stderr, where a CI log looks.
///
/// The stderr counterpart of [`emit`], and it exists for one reason: a warning whose ONLY
/// observable is stderr cannot otherwise be asserted on, because the test harness intercepts the
/// print macros before they reach the descriptor. An unenforceable enforcement guard is exactly
/// the thing that must not be pinned by reading the source, so test builds also record the line.
fn warn(line: &str) {
    eprintln!("{line}");
    #[cfg(test)]
    WARNINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(line.to_string());
}

/// The runner owns the immutable graph + policy and the shared mutable state.
struct Runner {
    steps: Arc<HashMap<String, Step>>,
    order: Vec<String>,
    intentional_skips: Vec<(String, crate::model::IntentionalSkipReason)>,
    /// Maximum number of DAG steps that may be active at once.
    max_steps: i64,
    keep_going: bool,
    verbosity: i64,
    /// Default inner-parallelism flag template (e.g. "-j") for steps without their own.
    default_jobs_flag: String,
    /// Default environment channel for steps that consume their width from an environment variable.
    default_jobs_env: String,
    /// Per-step cgroup boxing (memory/CPU caps + setsid-proof teardown), or `None` when unboxed.
    cgroups: BoxedCgroups,
    /// Multiplier from a step's measured RSS baseline to its inner memory cap.
    mem_cap_factor: f64,
    /// SMALL forcing-function defaults applied to a step that DECLARES NOTHING for a dimension
    /// (see `model::DEFAULT_SMALL_*`): an undeclared step is boxed into a tight
    /// 1-GiB memory.max / 1-core cpu.max / 10-s CPU-time floor so it must declare real needs.
    default_step_mem_cap_bytes: Option<i64>,
    default_step_cpu_count: Option<i64>,
    default_step_cpu_timeout: i64,
    /// Document-wide wall budget for steps that omit their own; `0` means each step derives one.
    default_step_timeout: i64,
    cpu_timeout_multiplier: f64,
    cpu_timeout_platform: String,
    /// OUTER wall budget for the WHOLE run, in seconds; `None` leaves the run unbounded.
    ///
    /// Independent of every per-step budget, and that independence is the point: no combination of
    /// individually-legal steps can run past it.
    run_timeout_s: Option<i64>,
    /// Durable, incrementally-flushed evidence for this run (per-step logs + boundary journal), or
    /// `None` when the operator opted out or the directory could not be created.
    evidence: Option<Arc<RunEvidence>>,
    /// Cross-process enforcement of this DAG's existing `resource_caps`, enabled only when the
    /// caller supplies `DAGRUN_RESOURCE_CAPS_PATH`.
    resource_coordinator: Option<ResourceCoordinator>,
    shared: Arc<Mutex<Shared>>,
    /// TEST-ONLY: run this instead of the guarded supervisor body, on the supervisor's own thread.
    ///
    /// Compiled out of every shipped build. It exists because layer TWO -- the dead-supervisor
    /// sweep -- is by definition only reachable when layer one is absent, and layer one is
    /// `with_supervisor_guard`, which is unconditional in production. Without a seam the sweep can
    /// only be tested by calling it directly on hand-built state, which is exactly the test that
    /// stays green when the call is deleted from the ready-set loop. The sibling engine gets the
    /// same coverage by replacing a method at run time; a compiled language needs the seam
    /// compiled in.
    #[cfg(test)]
    supervisor_override: Option<SupervisorBody>,
}

/// TEST-ONLY body a supervisor thread runs instead of the guarded one. See
/// [`Runner::supervisor_override`].
#[cfg(test)]
type SupervisorBody = Arc<dyn Fn(&Step) + Send + Sync>;

impl Runner {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cfg: &DagConfig,
        max_steps: i64,
        max_cpus: i64,
        keep_going: bool,
        verbosity: i64,
        cgroups: BoxedCgroups,
        order_override: Option<Vec<String>>,
        run_timeout_s: Option<i64>,
        resource_coordinator: Option<ResourceCoordinator>,
    ) -> Self {
        let max_cpus = max_cpus.max(1);
        let capped = cap_config_max_cpus(cfg, max_cpus);
        let steps: HashMap<String, Step> = capped
            .steps
            .iter()
            .map(|step| (step.tag(), step.clone()))
            .collect();
        // Dispatch order. When the caller supplies an explicit order (e.g. a critical-path
        // planner's) use it verbatim; otherwise default to LPT: sort tags by est_duration_s
        // DESCENDING (stable, so ties keep cfg/registration order, matching Python's stable
        // reverse sort).
        let order: Vec<String> = order_override.unwrap_or_else(|| {
            let mut o: Vec<String> = capped.steps.iter().map(|s| s.tag()).collect();
            o.sort_by(|a, b| {
                let ea = steps[a].hint.est_duration_s;
                let eb = steps[b].hint.est_duration_s;
                eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
            });
            o
        });
        let resource_avail: HashMap<String, i64> = capped
            .resource_caps
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let intentional_skips = capped
            .steps
            .iter()
            .filter_map(|step| step.skip_reason.map(|reason| (step.tag(), reason)))
            .collect();
        // Built before `steps` is moved into the Arc; the eager-cancel path consults this rather
        // than the step map, which it cannot reach while holding only `Shared`.
        let explains_index: HashMap<String, Vec<String>> = steps
            .iter()
            .map(|(tag, step)| (tag.clone(), step.explains.clone()))
            .collect();
        let fail_fast_family_index: HashMap<String, String> = steps
            .iter()
            .filter_map(|(tag, step)| {
                step.fail_fast_family
                    .as_ref()
                    .map(|family| (tag.clone(), family.clone()))
            })
            .collect();
        Runner {
            steps: Arc::new(steps),
            order,
            intentional_skips,
            max_steps: max_steps.max(1),
            keep_going,
            verbosity,
            default_jobs_flag: capped.default_jobs_flag.clone(),
            default_jobs_env: capped.default_jobs_env.clone(),
            cgroups,
            mem_cap_factor: capped.mem_cap_factor,
            default_step_mem_cap_bytes: capped.default_step_mem_cap_bytes,
            default_step_cpu_count: capped.default_step_cpu_count,
            default_step_cpu_timeout: capped.default_step_cpu_timeout,
            default_step_timeout: capped.default_step_timeout,
            cpu_timeout_multiplier: capped.cpu_timeout_multiplier,
            cpu_timeout_platform: capped.cpu_timeout_platform.clone(),
            run_timeout_s,
            evidence: RunEvidence::open(default_log_dir()).map(Arc::new),
            resource_coordinator,
            shared: Arc::new(Mutex::new(Shared {
                done: HashMap::new(),
                running: HashSet::new(),
                running_pids: HashMap::new(),
                aborted: HashSet::new(),
                resource_avail,
                failed: false,
                stop: false,
                step_profile_rows: Vec::new(),
                running_nonces: HashMap::new(),
                explains_index,
                fail_fast_family_index,
                failed_families: HashSet::new(),
                run_timed_out: false,
                active_processes: 0,
                max_concurrent_steps: 0,
                counted_processes: HashSet::new(),
                retired: HashSet::new(),
            })),
            #[cfg(test)]
            supervisor_override: None,
        }
    }

    // Tags whose deps FAILED (transitively) so they must never run — a fixpoint closure,
    // ported from the Python `Runner._skipped`.
    fn skipped(&self, sh: &Shared) -> HashSet<String> {
        let mut sk: HashSet<String> = HashSet::new();
        let mut changed = true;
        while changed {
            changed = false;
            for (tag, step) in self.steps.iter() {
                if sk.contains(tag)
                    || sh.done.contains_key(tag)
                    || sh.running.contains(tag)
                    || step.skip_reason.is_some()
                {
                    continue;
                }
                for d in &step.deps {
                    let dep_failed = sh.done.get(d).map(|o| !o.ok).unwrap_or(false);
                    if dep_failed || sk.contains(d) {
                        sk.insert(tag.clone());
                        changed = true;
                        break;
                    }
                }
            }
        }
        sk
    }

    /// Give an outcome to every supervisor thread that ENDED without publishing one.
    ///
    /// LAYER TWO of the two guards behind #80 runner-supervisor-crash-loud. Layer one -- the
    /// `catch_unwind` around `run_step` -- cannot cover a failure of layer one itself (a panic
    /// while reporting the first panic, or an abort-on-double-panic), and a wedge is not an
    /// acceptable second-order failure mode.
    ///
    /// THE KEY IS (launched) AND (finished) AND (no terminal outcome), and deliberately NOT "the
    /// tag is still in `running`". [`retire`] removes the tag from `running` BEFORE `done` is
    /// written, so a supervisor that dies between those two lines is in NEITHER set. A
    /// running-keyed sweep is blind to exactly that window -- which is the window a panic in the
    /// outcome-construction code lands in. That distinction was found by mutation, not by
    /// reasoning, and it is the detail most easily lost in a rewrite.
    fn sweep_dead_supervisors(&self, handles: &[(thread::JoinHandle<()>, Step)]) {
        let vanished: Vec<Step> = {
            let sh = lock_shared(&self.shared);
            handles
                .iter()
                .filter(|(h, step)| h.is_finished() && !sh.done.contains_key(&step.tag()))
                .map(|(_, step)| step.clone())
                .collect()
        };
        for step in vanished {
            let tag = step.tag();
            let published = publish_supervisor_failure(
                &self.shared,
                &self.cgroups,
                &self.evidence,
                &step,
                self.keep_going,
                SupervisorFailure {
                    reason: "SUPERVISOR VANISHED (its thread ended without publishing an outcome \
                             and without a recorded panic; the step's real result is UNKNOWN)"
                        .to_string(),
                    summary: "supervisor thread ended without publishing an outcome".to_string(),
                    elapsed_s: 0.0,
                },
            );
            if published {
                let message = format!(
                    "[scheduler] \u{2717} SUPERVISOR VANISHED for step {tag:?}: its thread is no \
                     longer alive and it never recorded a terminal outcome. Reporting the step as \
                     FAILED so the run terminates instead of waiting for a thread that is already \
                     gone. This is a runner bug; the step's own result is UNKNOWN."
                );
                emit(&message);
                eprintln!("{message}");
            }
        }
    }

    /// Drive the DAG to completion; returns `(ok, wall_seconds)`.
    fn run(&self) -> (bool, f64) {
        let mut handles: Vec<(thread::JoinHandle<()>, Step)> = Vec::new();
        // One pending completion is enough to make the ready set worth scanning again. The
        // timeout remains the polling bound for outer deadlines and cross-process resources,
        // while a finished local step no longer waits for that timeout to expire.
        let (scheduler_wake, scheduler_wake_rx) = std::sync::mpsc::sync_channel(1);
        let wall_start = Instant::now();
        for (tag, reason) in &self.intentional_skips {
            emit(&format!("[{tag}] SKIPPED reason={}", reason.value()));
            if let Some(evidence) = &self.evidence {
                evidence.record(
                    "step_skip",
                    &[
                        ("step", tag.clone()),
                        ("reason", reason.value().to_string()),
                    ],
                );
            }
        }
        let deadline = self
            .run_timeout_s
            .filter(|s| *s > 0)
            .map(|s| wall_start + Duration::from_secs(s as u64));
        loop {
            self.sweep_dead_supervisors(&handles);
            let mut launchable = Vec::new();
            {
                let mut sh = lock_shared(&self.shared);
                // OUTER BUDGET, CHECKED IN OUR OWN LOOP AND NOT BY AN EXTERNAL KILLER. Stopping
                // the run from inside is the entire reason this exists: an outside kill (a CI job
                // cancellation, a systemd RuntimeMaxSec) also destroys the evidence, so the bound
                // that fires FIRST must be one that can still write rows, flush the journal, and
                // hand a verdict back to the caller.
                if let Some(dl) = deadline {
                    if Instant::now() >= dl && !sh.run_timed_out {
                        sh.run_timed_out = true;
                        sh.failed = true;
                        sh.stop = true;
                        let cut: Vec<(String, u32, Option<String>)> = sh
                            .running_pids
                            .iter()
                            .map(|(k, v)| (k.clone(), *v, sh.running_nonces.get(k).cloned()))
                            .collect();
                        let names: Vec<String> = cut.iter().map(|(t, _, _)| t.clone()).collect();
                        eprintln!(
                            "[scheduler] RUN TIMEOUT: the whole run exceeded its outer budget of \
                             {}s ({:.1}s elapsed). Cutting {} in-flight step(s) short so the run \
                             can still report: {}",
                            self.run_timeout_s.unwrap_or(0),
                            wall_start.elapsed().as_secs_f64(),
                            cut.len(),
                            if names.is_empty() {
                                "<none running>".to_string()
                            } else {
                                names.join(", ")
                            }
                        );
                        if let Some(e) = &self.evidence {
                            e.record(
                                "run_timeout",
                                &[
                                    ("budget_s", self.run_timeout_s.unwrap_or(0).to_string()),
                                    (
                                        "elapsed_s",
                                        format!("{:.3}", wall_start.elapsed().as_secs_f64()),
                                    ),
                                    ("cut_steps", names.join(",")),
                                    ("done", sh.done.len().to_string()),
                                ],
                            );
                        }
                        let admitted: Vec<String> = sh.running.iter().cloned().collect();
                        for tag in admitted {
                            sh.aborted.insert(tag);
                        }
                        reap_many(&self.cgroups, &cut);
                    }
                }
                let skipped = self.skipped(&sh);
                // After eager-exit has tripped, ONE class of step may still start: a diagnostic
                // for a failure that has actually happened. Sparing an already-running diagnostic
                // is not enough -- the measured case had the diagnostic still QUEUED when its
                // subject failed, so it was never launched at all and the run reported the
                // symptom with no account of the cause.
                let failed_now = if sh.stop || !sh.failed_families.is_empty() {
                    genuinely_failed(&sh)
                } else {
                    std::collections::HashSet::new()
                };
                let startable_after_stop = |sh: &Shared, tag: &str| -> bool {
                    !blocked_by_fail_fast(sh, tag)
                        || is_exempt_from_eager_exit(sh, tag, &failed_now)
                };
                if let Some(coordinator) = &self.resource_coordinator {
                    let eligible: HashSet<String> = self
                        .order
                        .iter()
                        .filter(|tag| {
                            let step = &self.steps[*tag];
                            !sh.done.contains_key(*tag)
                                && !sh.running.contains(*tag)
                                && !skipped.contains(*tag)
                                && step.skip_reason.is_none()
                                && startable_after_stop(&sh, tag)
                                && deps_known(&sh, step)
                                && deps_ok(&sh, step)
                        })
                        .cloned()
                        .collect();
                    coordinator.retain_pending(&eligible);
                }
                // Do not end the run while a permitted diagnostic is still waiting to start.
                // Terminates: the set is finite, each member runs at most once, and a member
                // whose deps can never be satisfied is excluded here rather than waited on.
                let pending_diagnostics = self.order.iter().any(|tag| {
                    blocked_by_fail_fast(&sh, tag)
                        && !sh.done.contains_key(tag)
                        && !sh.running.contains(tag)
                        && !skipped.contains(tag)
                        && self.steps[tag].skip_reason.is_none()
                        && startable_after_stop(&sh, tag)
                        && deps_known(&sh, &self.steps[tag])
                        && deps_ok(&sh, &self.steps[tag])
                });
                let blocked_by_family: HashSet<String> = self
                    .order
                    .iter()
                    .filter(|tag| {
                        let tag = *tag;
                        let step = &self.steps[tag];
                        !sh.done.contains_key(tag)
                            && !sh.running.contains(tag)
                            && !skipped.contains(tag)
                            && step.skip_reason.is_none()
                            && blocked_by_fail_fast(&sh, tag)
                            && !startable_after_stop(&sh, tag)
                    })
                    .cloned()
                    .collect();
                if sh.running.is_empty()
                    && !pending_diagnostics
                    && (sh.stop
                        || sh.done.len()
                            + skipped.len()
                            + self.intentional_skips.len()
                            + blocked_by_family.len()
                            >= self.steps.len())
                {
                    if let Some(coordinator) = &self.resource_coordinator {
                        coordinator.clear_pending();
                    }
                    break;
                }
                {
                    for tag in &self.order {
                        let step = self.steps[tag].clone();
                        if sh.done.contains_key(tag)
                            || sh.running.contains(tag)
                            || skipped.contains(tag)
                            || step.skip_reason.is_some()
                        {
                            continue;
                        }
                        if !startable_after_stop(&sh, tag) {
                            continue;
                        }
                        if !deps_known(&sh, &step) {
                            continue;
                        }
                        if !deps_ok(&sh, &step) {
                            continue;
                        }
                        if sh.running.len() as i64 >= self.max_steps {
                            break;
                        }
                        if !res_free(&sh, &step) {
                            continue;
                        }
                        let resource_reservation = match &self.resource_coordinator {
                            Some(coordinator)
                                if step.hint.resources.values().any(|demand| *demand > 0) =>
                            {
                                match coordinator.try_acquire(tag, &step.hint.resources) {
                                    Ok(SharedResourceAcquire::Waiting { newly_queued }) => {
                                        if newly_queued {
                                            let resources = step
                                                .hint
                                                .resources
                                                .iter()
                                                .map(|(name, demand)| format!("{name}={demand}"))
                                                .collect::<Vec<_>>()
                                                .join(",");
                                            emit(&format!(
                                                "[{tag}] WAIT resource_caps {resources} shared with other scheduler processes"
                                            ));
                                            if let Some(evidence) = &self.evidence {
                                                evidence.record(
                                                    "resource_wait_start",
                                                    &[
                                                        ("step", tag.clone()),
                                                        ("resources", resources),
                                                    ],
                                                );
                                            }
                                        }
                                        continue;
                                    }
                                    Ok(SharedResourceAcquire::Granted {
                                        reservation,
                                        waited_seconds,
                                    }) => {
                                        if waited_seconds >= LOOP_SLEEP.as_secs_f64() {
                                            emit(&format!(
                                                "[{tag}] READY resource_caps after {waited_seconds:.3}s wait"
                                            ));
                                            if let Some(evidence) = &self.evidence {
                                                evidence.record(
                                                    "resource_wait_end",
                                                    &[
                                                        ("step", tag.clone()),
                                                        (
                                                            "waited_s",
                                                            format!("{waited_seconds:.3}"),
                                                        ),
                                                    ],
                                                );
                                            }
                                        }
                                        Ok(Some(reservation))
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            _ => Ok(None),
                        };
                        sh.running.insert(tag.clone());
                        acquire(&mut sh, &step);
                        launchable.push((step, resource_reservation));
                    }
                    // TERMINAL STARVE. Nothing is launchable, nothing is running, and work
                    // remains: no future event can change that, because every state transition in
                    // this loop is caused by a running step completing. Sleeping here is what
                    // turned three distinct defects -- an unsatisfiable resource cap, a dangling
                    // dep, and a dependency cycle -- into one indistinguishable symptom: a live
                    // process at 0% CPU with a frozen log and no exit.
                    //
                    // SOUNDNESS: the `--max-steps` cap cannot be the cause of an empty
                    // `launchable` here. `sh.running.len() >= self.max_steps` can only break the
                    // scan while `running` is NON-empty (`max_steps` is `max_steps.max(1)` in
                    // `Runner::new`, so it is >= 1). And with `running` empty no worker thread can
                    // be mutating `sh.done` or `sh.resource_avail`, so the counts read below are
                    // stable rather than merely sampled, and `resource_avail` has returned to the
                    // configured caps.
                    let accounted = sh.done.len()
                        + skipped.len()
                        + self.intentional_skips.len()
                        + blocked_by_family.len();
                    let remaining = self.steps.len().saturating_sub(accounted);
                    let waiting_on_shared_resource = self
                        .resource_coordinator
                        .as_ref()
                        .is_some_and(ResourceCoordinator::has_pending);
                    if launchable.is_empty()
                        && sh.running.is_empty()
                        && remaining > 0
                        && !waiting_on_shared_resource
                    {
                        let mut stuck: Vec<String> = self
                            .order
                            .iter()
                            .filter(|t| {
                                !sh.done.contains_key(*t)
                                    && !skipped.contains(*t)
                                    && self.steps[*t].skip_reason.is_none()
                                    && !blocked_by_family.contains(*t)
                            })
                            .cloned()
                            .collect();
                        stuck.sort();
                        eprintln!(
                            "[scheduler] REFUSED: terminal starve -- {remaining} step(s) can \
                             never be admitted; nothing is running and nothing is launchable, so \
                             no future event can unblock them."
                        );
                        for r in ungrantable_resources(&sh.resource_avail, &self.steps, &stuck) {
                            eprintln!("[scheduler]   {r}");
                        }
                        eprintln!(
                            "[scheduler]   starved step(s) ({}): {}",
                            stuck.len(),
                            stuck.join(", ")
                        );
                        if let Some(e) = &self.evidence {
                            e.record(
                                "terminal_starve",
                                &[
                                    ("starved", stuck.len().to_string()),
                                    ("steps", stuck.join(",")),
                                ],
                            );
                        }
                        sh.failed = true;
                        sh.stop = true;
                        break;
                    }
                }
            }
            for (step, resource_reservation) in launchable {
                let shared = Arc::clone(&self.shared);
                let scheduler_wake = scheduler_wake.clone();
                let keep_going = self.keep_going;
                let verbosity = self.verbosity;
                let default_jobs_flag = self.default_jobs_flag.clone();
                let default_jobs_env = self.default_jobs_env.clone();
                let cgroups = self.cgroups.clone();
                let mem_cap_factor = self.mem_cap_factor;
                let default_step_mem_cap_bytes = self.default_step_mem_cap_bytes;
                let default_step_cpu_count = self.default_step_cpu_count;
                let default_step_cpu_timeout = self.default_step_cpu_timeout;
                let default_step_timeout = self.default_step_timeout;
                let evidence = self.evidence.clone();
                let cpu_timeout_multiplier = self.cpu_timeout_multiplier;
                let cpu_timeout_platform = self.cpu_timeout_platform.clone();
                let recovery = SupervisorRecovery {
                    step: step.clone(),
                    shared: Arc::clone(&shared),
                    cgroups: cgroups.clone(),
                    evidence: evidence.clone(),
                    keep_going,
                };
                let swept_step = step.clone();
                #[cfg(test)]
                let supervisor_override = self.supervisor_override.clone();
                handles.push((
                    thread::spawn(move || {
                        // TEST-ONLY (see `Runner::supervisor_override`): stand in for the guarded
                        // body so a supervisor can end WITHOUT layer one in the picture.
                        #[cfg(test)]
                        if let Some(body) = supervisor_override {
                            body(&recovery.step);
                            let _ = scheduler_wake.try_send(());
                            return;
                        }
                        with_supervisor_guard(recovery, move || {
                            run_step(StepCtx {
                                step,
                                shared,
                                keep_going,
                                verbosity,
                                default_jobs_flag,
                                default_jobs_env,
                                cgroups,
                                mem_cap_factor,
                                default_step_mem_cap_bytes,
                                default_step_cpu_count,
                                default_step_cpu_timeout,
                                default_step_timeout,
                                evidence,
                                cpu_timeout_multiplier,
                                cpu_timeout_platform,
                                run_origin: wall_start,
                                resource_reservation,
                            });
                        });
                        let _ = scheduler_wake.try_send(());
                    }),
                    swept_step,
                ));
            }
            let _ = scheduler_wake_rx.recv_timeout(LOOP_SLEEP);
        }
        for (h, _step) in handles {
            let _ = h.join();
        }
        if let Some(coordinator) = &self.resource_coordinator {
            coordinator.clear_pending();
        }
        // NORMAL-exit backstop: reap any step cgroup that still has live procs (a setsid orphan a
        // step left behind lives there). Does NOT stop the outer scope, so a green run stays green.
        if let Some(cg) = &self.cgroups {
            if cg.enabled() {
                let leftover = cg.kill_all_remaining();
                if leftover > 0 {
                    emit(&format!(
                        "[scheduler] reaped {leftover} leftover step cgroup(s) on exit (setsid \
                         orphans a step left behind)."
                    ));
                }
            }
        }
        let failed = lock_shared(&self.shared).failed;
        (!failed, wall_start.elapsed().as_secs_f64())
    }

    fn result(&self, wall: f64) -> RunResult {
        let sh = lock_shared(&self.shared);
        let outcomes: Vec<StepOutcome> = self
            .order
            .iter()
            .filter_map(|t| sh.done.get(t).cloned())
            .collect();
        let skipped_set = self.skipped(&sh);
        let intentional_skip_tags: HashSet<&str> = self
            .intentional_skips
            .iter()
            .map(|(tag, _)| tag.as_str())
            .collect();
        let mut not_launched: Vec<String> = self
            .order
            .iter()
            .filter(|tag| {
                !sh.done.contains_key(*tag)
                    && !skipped_set.contains(*tag)
                    && !intentional_skip_tags.contains(tag.as_str())
            })
            .cloned()
            .collect();
        not_launched.sort();
        let mut skipped: Vec<String> = skipped_set.into_iter().collect();
        skipped.sort();
        RunResult {
            ok: !sh.failed && not_launched.is_empty(),
            wall_s: wall,
            outcomes,
            skipped,
            not_launched,
            intentional_skips: self.intentional_skips.clone(),
            step_profile_rows: sh.step_profile_rows.clone(),
            run_timed_out: sh.run_timed_out,
            max_concurrent_steps: sh.max_concurrent_steps,
        }
    }
}

// Read a child pipe to EOF into `buf`; when `stream`, also echo each line tagged.
/// Pump one output stream: into the in-memory buffer, through to the durable per-step log, and
/// into the test-boundary tracker.
///
/// CHUNKED, NOT LINE-ORIENTED, and that change is load-bearing for test attribution. The previous
/// `read_until(b'\n')` could not observe a test that never finished: Rust's libtest prints
/// `test some::name ... ` and only then RUNS the test, so under `--nocapture` the hung test's own
/// announcement sits in the pipe with no trailing newline and a line reader blocks on it forever —
/// the one test whose name matters is the one such a reader can never see. Reading chunks and
/// letting [`StepStream`] hold the bytes since the last newline leaves that announcement available
/// at teardown. Line-at-a-time streaming to the console is preserved by [`StepStream`]'s own
/// splitting, so `-vv` output is unchanged.
#[derive(Default)]
struct ConsoleTestIdentity {
    active: Option<String>,
}

impl ConsoleTestIdentity {
    fn decorate(&mut self, tag: &str, line: &str) -> String {
        let event = recognize(line);
        let identity = match &event {
            Some(TestEvent::Start(name)) | Some(TestEvent::End(name, _)) => name.clone(),
            None => self.active.clone().unwrap_or_else(|| tag.to_string()),
        };
        match event {
            Some(TestEvent::Start(name)) => self.active = Some(name),
            Some(TestEvent::End(_, _)) => self.active = None,
            None => {}
        }
        format!("[{tag}][test={identity}] {line}")
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    buf: Arc<Mutex<BoundedCapture>>,
    tag: String,
    verbosity: i64,
    sink: Arc<StepStream>,
    console_identity: Arc<Mutex<ConsoleTestIdentity>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut br = BufReader::new(reader);
        let mut chunk = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match br.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = &chunk[..n];
                    buf.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .feed(bytes);
                    sink.ingest(bytes);
                    if verbosity >= 2 {
                        // Console streaming stays line-at-a-time: hold back the unterminated tail
                        // so a partially-received line is not printed twice.
                        pending.extend_from_slice(bytes);
                        while let Some(idx) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=idx).collect();
                            let text = String::from_utf8_lossy(&line);
                            let text = text.trim_end_matches(['\n', '\r']);
                            if verbosity >= 5 {
                                let decorated = console_identity
                                    .lock()
                                    .map(|mut c| c.decorate(&tag, text))
                                    .unwrap_or_else(|_| format!("[{tag}][test={tag}] {text}"));
                                emit(&decorated);
                            } else {
                                emit(&format!("[{tag}] {text}"));
                            }
                        }
                        // THE SECOND UNBOUNDED BUFFER. `pending` is drained only while it
                        // CONTAINS a newline, so newline-free output grows it without limit -- on
                        // exactly the path that streams a runaway to a human. Bounding the
                        // capture alone would have left this hole open. Flush the oversized
                        // prefix as its own console line: a console line is a display artifact,
                        // and splitting one costs far less than holding an unbounded one.
                        // SAY SO WHEN IT FIRES. Without the notice below, a forced flush is a
                        // console line like any other, so the guard acting and the guard never
                        // acting produce indistinguishable output -- nobody can tell whether it
                        // has ever worked. It also cannot fire on healthy output (the longest
                        // newline-free run measured across a real corpus was ~27 KiB against a
                        // 1 MiB bound), so its one appearance carries the whole burden of
                        // explaining itself.
                        while pending.len() >= STREAM_LINE_MAX_BYTES {
                            let forced: Vec<u8> = pending.drain(..STREAM_LINE_MAX_BYTES).collect();
                            let text = String::from_utf8_lossy(&forced);
                            if verbosity >= 5 {
                                let decorated = console_identity
                                    .lock()
                                    .map(|mut c| c.decorate(&tag, &text))
                                    .unwrap_or_else(|_| format!("[{tag}][test={tag}] {text}"));
                                emit(&decorated);
                            } else {
                                emit(&format!("[{tag}] {text}"));
                            }
                            emit(&format!(
                                "[{tag}] {}",
                                stream_split_notice(STREAM_LINE_MAX_BYTES)
                            ));
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if verbosity >= 2 && !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending);
            let text = text.trim_end_matches(['\n', '\r']);
            if verbosity >= 5 {
                let decorated = console_identity
                    .lock()
                    .map(|mut c| c.decorate(&tag, text))
                    .unwrap_or_else(|_| format!("[{tag}][test={tag}] {text}"));
                emit(&decorated);
            } else {
                emit(&format!("[{tag}] {text}"));
            }
        }
    })
}

/// Largest console line the `-vv` live stream will hold back waiting for a newline.
///
/// The live-stream buffer is drained only when it CONTAINS a newline, so output with no newline
/// in it grows without limit. This is deliberately a fixed display bound rather than another
/// environment knob: it decides how a line is broken on a console, never what is retained, and
/// both the durable log and the in-memory capture are unaffected by it.
//
// The value is pinned literally, at one MiB, by
// `the_console_line_bound_is_pinned_to_one_mib_literally` below; the sibling engine pins the same
// literal, so a change to one that is not made in the other fails a test by name rather than
// drifting. This note is a plain comment, NOT rustdoc: the published crate's documentation must
// stand alone, and a source-tree path in it is a reference the reader of a packaged crate cannot
// follow.
const STREAM_LINE_MAX_BYTES: usize = 1024 * 1024;

// Leading text of the notice emitted immediately after a forced flush.
//
// A cap that acts silently is indistinguishable from a cap that never acts, so nobody can tell
// whether the guard has ever worked. This one fires only on output that has produced
// `STREAM_LINE_MAX_BYTES` with no newline -- a shape healthy output does not reach -- so the
// single line it prints is the only evidence a reader will ever get. It has to be unambiguous
// about two things: that the RUNNER did the splitting, and that splitting a console line
// discarded nothing. The sibling engine emits the same text.
const STREAM_SPLIT_MARKER: &str = "^ RUNNER SPLIT the line above";

/// The one line a reader gets when the `-vv` stream cap fires.
///
/// Names the mechanism, the threshold, the reason, and -- because "split" alone reads as data
/// loss -- states explicitly that nothing was dropped.
///
/// `limit_bytes` is a parameter rather than read from the constant directly, so a test can drive
/// the guard at a smaller bound and assert the notice names THAT bound. The sibling engine had a
/// defaulted argument here and reported the constant's original value instead of the bound in
/// force -- misreporting the very threshold the message exists to explain.
fn stream_split_notice(limit_bytes: usize) -> String {
    format!(
        "{STREAM_SPLIT_MARKER}: the step emitted {limit_bytes} bytes with no newline, \
         so the live stream broke the console line to avoid buffering it without limit. \
         NOTHING WAS DISCARDED -- the durable log and the captured output are complete; \
         only the console display was broken."
    )
}

/// The last `limit` bytes of one output stream, in a buffer that never grows past `limit`.
///
/// WHAT THIS REPLACES, and why the shape matters. The capture used to be a `Vec<u8>` extended
/// once per 8 KiB read, so a step held its ENTIRE output in the runner's RSS for the step's whole
/// lifetime, and the failure path concatenated stdout and stderr into a third copy on top. A
/// runaway step OOM-killed the RUNNER -- taking the run's verdict, its profile rows and its
/// evidence with it -- before it could fill the disk that [`DEFAULT_LOG_MAX_BYTES`] protects.
///
/// A PREALLOCATED RING, not a queue of chunks and not a `Vec` truncated after the fact: a ring
/// allocated once at exactly `limit` bytes costs exactly `limit` in steady state, independent of
/// both the step's total output and its write sizes, and the only transient above that is the
/// single ordered copy a failure dump needs.
///
/// `limit == None` means unlimited and is the explicit opt-out, not a fallback.
struct BoundedCapture {
    buf: Vec<u8>,
    limit: Option<usize>,
    pos: usize,
    wrapped: bool,
    /// EVERY byte the stream produced, including the dropped ones. Kept because "you are seeing a
    /// tail" is only actionable next to how much tail there was.
    total: u64,
}

impl BoundedCapture {
    fn new(limit: Option<usize>) -> Self {
        BoundedCapture {
            buf: match limit {
                Some(n) => vec![0u8; n],
                None => Vec::new(),
            },
            limit,
            pos: 0,
            wrapped: false,
            total: 0,
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        let Some(limit) = self.limit else {
            self.buf.extend_from_slice(chunk);
            return;
        };
        if limit == 0 {
            return;
        }
        if chunk.len() >= limit {
            // This read alone overruns the ring: only its own tail can survive, so write that and
            // reset the cursor rather than walking the ring `chunk.len()` times.
            self.buf.copy_from_slice(&chunk[chunk.len() - limit..]);
            self.pos = 0;
            self.wrapped = true;
            return;
        }
        let head = limit - self.pos;
        if chunk.len() <= head {
            self.buf[self.pos..self.pos + chunk.len()].copy_from_slice(chunk);
            self.pos += chunk.len();
            if self.pos == limit {
                self.pos = 0;
                self.wrapped = true;
            }
        } else {
            self.buf[self.pos..].copy_from_slice(&chunk[..head]);
            let rest = chunk.len() - head;
            self.buf[..rest].copy_from_slice(&chunk[head..]);
            self.pos = rest;
            self.wrapped = true;
        }
    }

    /// How many bytes the ring is currently holding.
    fn kept(&self) -> usize {
        match self.limit {
            None => self.buf.len(),
            Some(limit) => {
                if self.wrapped {
                    limit
                } else {
                    self.pos
                }
            }
        }
    }

    /// True when output was discarded, i.e. what remains is a TAIL and not the whole thing.
    fn dropped(&self) -> bool {
        (self.kept() as u64) < self.total
    }

    /// The retained bytes, oldest first. The ONE allocation proportional to the ceiling.
    fn tail(&self) -> Vec<u8> {
        match self.limit {
            None => self.buf.clone(),
            Some(_) if !self.wrapped => self.buf[..self.pos].to_vec(),
            Some(_) => {
                let mut out = Vec::with_capacity(self.kept());
                out.extend_from_slice(&self.buf[self.pos..]);
                out.extend_from_slice(&self.buf[..self.pos]);
                out
            }
        }
    }
}

fn failure_detail_lines(tag: &str, streams: &[&[u8]], verbosity: i64) -> Vec<String> {
    let mut rendered = Vec::new();
    for bytes in streams {
        // A fresh context PER SLICE: a test boundary observed in one body of output may not be
        // borrowed by the next, so nothing can assign a test name across a discontinuity. A step
        // is dumped as ONE slice (its single capture ring, both pipes in arrival order), so
        // within a step the boundary tracking is continuous, exactly as the durable log sees it.
        let mut identity = ConsoleTestIdentity::default();
        let text = String::from_utf8_lossy(bytes);
        for line in text.lines() {
            if verbosity >= 5 {
                rendered.push(identity.decorate(tag, line));
            } else {
                rendered.push(format!("[{tag}] {line}"));
            }
        }
    }
    rendered
}

/// Best-effort one-line summary: the last non-empty decoded line of captured output.
fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines().rev() {
        let t = line.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    String::new()
}

/// Test counts extracted from one step's COMPLETE captured output, before
/// verbosity decides how much of that output is presented to a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapturedTestCounts {
    executed: Option<u64>,
    filtered: Option<u64>,
}

fn count_between(line: &str, prefix: &str, suffix: &str) -> Option<u64> {
    let rest = line.get(line.find(prefix)? + prefix.len()..)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest.get(digits.len()..)?.starts_with(suffix) {
        return None;
    }
    digits.parse().ok()
}

fn captured_test_counts(bytes: &[u8]) -> CapturedTestCounts {
    let text = String::from_utf8_lossy(bytes);
    let mut running_total = 0u64;
    let mut running_seen = false;
    let mut passed_total = 0u64;
    let mut passed_seen = false;
    let mut filtered_total = 0u64;
    let mut filtered_seen = false;
    let mut overflow = false;
    for line in text.lines() {
        if let Some(value) = count_between(line, "running ", " test") {
            running_seen = true;
            running_total = running_total.checked_add(value).unwrap_or_else(|| {
                overflow = true;
                0
            });
        }
        if let Some(value) = count_between(line, "test result: ok. ", " passed") {
            passed_seen = true;
            passed_total = passed_total.checked_add(value).unwrap_or_else(|| {
                overflow = true;
                0
            });
        }
        if let Some(marker) = line.find(" filtered out") {
            if let Some(value) = line[..marker]
                .split_whitespace()
                .next_back()
                .and_then(|v| v.parse().ok())
            {
                filtered_seen = true;
                filtered_total = filtered_total.checked_add(value).unwrap_or_else(|| {
                    overflow = true;
                    0
                });
            }
        }
    }
    if overflow {
        return CapturedTestCounts {
            executed: None,
            filtered: None,
        };
    }
    CapturedTestCounts {
        // Match the canonical parser: `running N` is the primary executed
        // signal; passed-count summaries are only a truncation fallback.
        executed: if running_seen {
            Some(running_total)
        } else if passed_seen {
            Some(passed_total)
        } else {
            None
        },
        filtered: filtered_seen.then_some(filtered_total),
    }
}

/// Adapt a cgroup `cpu.pressure` `{avg10, avg60}` map to a typed [`PsiReading`] for the enrichment
/// builder; `None` (unreadable / unboxed) passes straight through.
fn psi_from(pressure: Option<BTreeMap<String, f64>>) -> Option<PsiReading> {
    let map = pressure?;
    Some(PsiReading {
        avg10: *map.get("avg10")?,
        avg60: *map.get("avg60")?,
    })
}

/// Everything a supervisor thread needs to run ONE step.
struct StepCtx {
    step: Step,
    shared: Arc<Mutex<Shared>>,
    keep_going: bool,
    verbosity: i64,
    default_jobs_flag: String,
    default_jobs_env: String,
    cgroups: BoxedCgroups,
    mem_cap_factor: f64,
    /// SMALL forcing-function defaults for an undeclared step (see `model::DEFAULT_SMALL_*`).
    default_step_mem_cap_bytes: Option<i64>,
    default_step_cpu_count: Option<i64>,
    default_step_cpu_timeout: i64,
    /// Document-wide wall budget for steps that omit their own; `0` means each step derives one.
    default_step_timeout: i64,
    /// Run-level durable evidence sink (per-step log + test-boundary journal), if enabled.
    evidence: Option<Arc<RunEvidence>>,
    cpu_timeout_multiplier: f64,
    cpu_timeout_platform: String,
    /// Monotonic origin every profiled step measures its start/finish offset from, so two rows of
    /// one run can be tested for OVERLAP. Monotonic, not wall clock: a clock step mid-run must not
    /// make one step appear to precede another.
    run_origin: Instant,
    /// Kept alive for exactly the child step's lifetime. Waiting occurred before `run_step`, so
    /// it is deliberately excluded from the step's wall and CPU budgets.
    resource_reservation: Result<Option<crate::resource_caps::Reservation>, String>,
}

/// Tear down one step's whole process tree: SIGTERM grace, then `cgroup.kill`/SIGKILL.
///
/// When per-step containment is enabled, writing the step's child `cgroup.kill` SIGKILLs the
/// ENTIRE subtree atomically, including `setsid`/double-fork escapees a process-group kill misses.
/// The killpg that follows is a belt-and-suspenders for the no-cgroup path. No Silent Failure: a
/// failed cgroup.kill while containment is enabled surfaces a warning.
/// Tear down one step's whole process tree.
///
/// `cgroup.kill` clears a subtree atomically, including `setsid`/double-fork escapees, because an
/// escapee changes session and pgid but not cgroup membership. Without containment there is no such
/// primitive, and a process-group kill alone leaves escapees running — the step never exits, the
/// run never returns, and the end-of-run profile flush never happens, so the lane cannot even
/// report what went wrong. `kill_descendants` is the fallback that closes that gap by parentage.
///
/// The degraded case is announced whenever it happens. It used to be announced ONLY when an enabled
/// cgroup's kill failed, so the unboxed path — which has exactly the same weakness — degraded in
/// silence. A step running without enforceable containment must say so once, or "unbounded" is
/// invisible in the log.
fn hard_reap(cgroups: &BoxedCgroups, tag: &str, pid: u32, nonce: Option<&str>) {
    let contained = match cgroups {
        Some(cg) if cg.enabled() => {
            if cg.kill(tag) {
                true
            } else {
                eprintln!(
                    "[scheduler] WARNING: cgroup.kill for step {tag} failed; falling back to \
                     process-group kill plus a /proc descendant sweep."
                );
                false
            }
        }
        _ => {
            // ONCE per run, not once per step: reap runs for every step (and twice for a
            // timed-out one), so a per-step warning would bury the log it is meant to inform.
            if !UNBOXED_REAP_WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[scheduler] WARNING: steps are UNBOXED (no cgroup containment). Teardown is \
                     a process-group kill plus a /proc descendant sweep, not an atomic \
                     cgroup.kill. A budget can only be enforced as far as the kill reaches."
                );
            }
            false
        }
    };
    let _ = signal_group(pid, libc::SIGKILL);
    if !contained {
        let swept = kill_descendants(pid);
        if swept > 0 {
            eprintln!(
                "[scheduler] step {tag}: killed {swept} descendant(s) the process-group kill \
                 missed (setsid/double-fork escapees)."
            );
        }
    }
    // FINAL BACKSTOP, RUN ON EVERY PATH — boxed and unboxed alike. Boxed, it should find nothing
    // and costs one `/proc` walk; that it runs anyway is the point, because "cgroup.kill returned
    // success" is a claim about a write, not evidence that the subtree is empty. Unboxed it is the
    // only best-effort mechanism that reaches an environment-preserving double-fork escapee.
    if let Some(n) = nonce {
        let swept = kill_by_nonce(n);
        if swept > 0 {
            eprintln!(
                "[scheduler] step {tag}: killed {swept} process(es) by ownership nonce that \
                     neither the process-group kill nor the parentage sweep could reach \
                     (an environment-preserving setsid/double-fork escapee)."
            );
        }
    }
}

fn reap(cgroups: &BoxedCgroups, tag: &str, pid: u32, nonce: Option<&str>) {
    // Give the original group one bounded chance to identify in-flight work, then hard-stop every
    // ownership handle. `terminate_groups` ignores zombies instead of charging them the full grace.
    terminate_groups(&[pid]);
    hard_reap(cgroups, tag, pid, nonce);
}

/// Cancel several in-flight steps under one shared grace instead of `N * REAP_TERM_GRACE`.
fn reap_many(cgroups: &BoxedCgroups, steps: &[(String, u32, Option<String>)]) {
    terminate_groups(&steps.iter().map(|(_, pid, _)| *pid).collect::<Vec<_>>());
    for (tag, pid, nonce) in steps {
        hard_reap(cgroups, tag, *pid, nonce.as_deref());
    }
}

/// Human-readable byte count (e.g. `3.5 GiB`); `"?"` when unknown.
fn fmt_bytes(n: Option<i64>) -> String {
    let mut value = match n {
        Some(v) => v as f64,
        None => return "?".to_string(),
    };
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if value < 1024.0 || unit == "TiB" {
            return if unit == "B" {
                format!("{} B", value as i64)
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{} B", n.unwrap_or(0))
}

/// Freeze the evidence that must survive a timeout BEFORE sending any signal.
///
/// Controlled runners bind tests through explicit boundaries in `StepStream`. Third-party
/// runners additionally get an owned `/proc` snapshot: nextest-style `--exact TEST` children can
/// be bound directly, while cargo-test's shared test binary remains explicitly unattributed.
/// The quantity a per-step budget bounds — and therefore the ONLY quantity a breach of it may be
/// reported in.
///
/// A wall ceiling and a CPU ceiling are different physical quantities, and for the same step they
/// are different numbers: wall keeps rising while the step is descheduled, CPU does not. Naming
/// the unit is not decoration; without it a reader compares whichever number is printed against
/// whichever limit is printed, which is how a CPU breach came to be reported as having consumed
/// more seconds than its own run's CPU rollup contained.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BudgetUnit {
    /// user+system CPU seconds, as the CPU guard measures them.
    CpuSeconds,
    /// wall-clock seconds since the step started.
    WallSeconds,
}

impl BudgetUnit {
    fn as_str(self) -> &'static str {
        match self {
            BudgetUnit::CpuSeconds => "cpu_seconds",
            BudgetUnit::WallSeconds => "wall_seconds",
        }
    }
}

/// The bound a step crossed, recorded in the unit it was actually measured in.
///
/// `measured_s` is the quantity the guard compared against `limit_s`, and ONE `unit` covers both:
/// recording a wall figure against a CPU bound is not merely discouraged, it is unrepresentable.
/// `wall_elapsed_s` is always carried alongside as context and is never the compared quantity
/// unless the limit is itself a wall limit.
struct TerminationBoundary<'a> {
    event: &'a str,
    unit: BudgetUnit,
    limit_s: i64,
    measured_s: f64,
    wall_elapsed_s: f64,
}

fn capture_termination_evidence(
    evidence: &Option<Arc<RunEvidence>>,
    sink: &StepStream,
    tag: &str,
    pid: u32,
    nonce: &str,
    boundary: TerminationBoundary<'_>,
) -> Culprit {
    let observations = process_snapshot(pid, Some(nonce));
    for row in &observations {
        emit(&format!(
            "[{tag}] ↳ process pid={} ppid={} signature={} wall={:.3}s cpu={:.3}s{} cmd={}",
            row.pid,
            row.ppid,
            row.signature,
            row.wall_elapsed_s,
            row.cpu_elapsed_s,
            row.test
                .as_ref()
                .map(|test| format!(" test={test}"))
                .unwrap_or_default(),
            row.command,
        ));
        if let Some(e) = evidence {
            e.record(
                "process_snapshot",
                &[
                    ("step", tag.to_string()),
                    ("pid", row.pid.to_string()),
                    ("ppid", row.ppid.to_string()),
                    ("signature", row.signature.to_string()),
                    ("wall_elapsed_s", format!("{:.3}", row.wall_elapsed_s)),
                    ("cpu_elapsed_s", format!("{:.3}", row.cpu_elapsed_s)),
                    ("test", row.test.clone().unwrap_or_default()),
                    ("test_basis", row.test_basis.unwrap_or("").to_string()),
                    ("command", row.command.clone()),
                ],
            );
        }
    }
    let culprit = bind_process_tests(sink.culprit(), &observations);
    if let Some(e) = evidence {
        e.record(
            boundary.event,
            &[
                ("step", tag.to_string()),
                ("measured_s", format!("{:.3}", boundary.measured_s)),
                ("limit_s", boundary.limit_s.to_string()),
                // Applies to BOTH numbers above, which is the point: they are one comparison.
                ("unit", boundary.unit.as_str().to_string()),
                ("wall_elapsed_s", format!("{:.3}", boundary.wall_elapsed_s)),
                ("culprit_test", culprit.test.clone().unwrap_or_default()),
                ("culprit_basis", culprit.how.to_string()),
                ("tests_completed", culprit.completed.to_string()),
                ("in_flight_count", culprit.in_flight.len().to_string()),
                (
                    "in_flight_tests",
                    culprit
                        .in_flight
                        .iter()
                        .map(|test| format!("{}@{:.3}s", test.name, test.elapsed_s))
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            ],
        );
    }
    culprit
}

fn checked_jobs_env_export(name: &str, value: &str) -> String {
    let failure = format!(
        "dagrun: ERROR: jobs environment variable {name} did not retain assigned width {value}; \
         refusing guest command"
    );
    format!(
        "if ! export {name}={value} || [ \"${{{name}-}}\" != {value} ]; then\n  \
         printf '%s\\n' '{failure}' >&2\n  exit 125\nfi\n"
    )
}

/// Supervise ONE step: launch (cgroup-boxed when enabled), pump output, enforce the timeout,
/// reap the whole tree, classify, and record a per-step profile row.
fn run_step(ctx: StepCtx) {
    let StepCtx {
        step,
        shared,
        keep_going,
        verbosity,
        default_jobs_flag,
        default_jobs_env,
        cgroups,
        mem_cap_factor,
        default_step_mem_cap_bytes,
        default_step_cpu_count,
        default_step_cpu_timeout,
        default_step_timeout,
        evidence,
        cpu_timeout_multiplier,
        cpu_timeout_platform,
        run_origin,
        resource_reservation,
    } = ctx;
    let tag = step.tag();
    let _resource_reservation = match resource_reservation {
        Ok(reservation) => reservation,
        Err(error) => {
            let reason = format!("resource_caps shared-state refusal: {error}");
            {
                let mut sh = lock_shared(&shared);
                retire(&mut sh, &step);
                sh.done.insert(
                    tag.clone(),
                    StepOutcome::failed(
                        tag.clone(),
                        0.0,
                        reason.clone(),
                        None,
                        false,
                        0,
                        false,
                        0,
                        false,
                        0,
                        0,
                        cpu_timeout_multiplier,
                        &cpu_timeout_platform,
                        false,
                        None,
                        None,
                    ),
                );
                trip_fail_fast(&mut sh, &cgroups, keep_going, &tag);
            }
            if let Some(evidence) = &evidence {
                evidence.record(
                    "resource_refusal",
                    &[("step", tag.clone()), ("reason", reason.clone())],
                );
            }
            emit(&format!("[{tag}] \u{2717} REFUSED {reason}"));
            eprintln!("[{tag}] REFUSED: {reason}");
            return;
        }
    };
    emit(&format!("[{tag}] \u{25b6} START  {}", step.desc));

    // Append the step's inner-parallelism (concurrency) flag when it declares one. No-op when the
    // step has no preferred_inner_jobs.
    let inner_jobs = preferred_inner_jobs(&step, None);
    let jobs_env = env_with_inner_jobs(&step, &default_jobs_env, inner_jobs);
    let mut base_cmd = command_with_inner_jobs(&step, &default_jobs_flag, inner_jobs);
    if let Some((name, value)) = &jobs_env {
        // The real cgroup wrapper exports its scope/operator CARGO_BUILD_JOBS before executing
        // `base_cmd`. Re-export the admitted per-step value at the final command boundary so a
        // configured CARGO_BUILD_JOBS channel cannot be overwritten by that outer default. The
        // name was validated before any node could spawn and the value is a decimal integer.
        base_cmd = format!("{}{base_cmd}", checked_jobs_env_export(name, value));
    }
    // cpu.max core cap. `inner_jobs` (declared width) still keys the command's `-j` flag above;
    // the cgroup core cap falls back to the small default so an undeclared step is 1-core-boxed
    // WITHOUT appending a bogus `-j 1` to a command that may not accept it.
    let cpu_count = effective_cpu_count(&step, default_step_cpu_count);
    // SMALL forcing-function defaults for an undeclared step: fall back to the DAG's tight
    // 1-GiB memory.max / 1-core cpu.max / 10-s CPU-time floor when the step declares nothing
    // for that dimension. CPU-bound caps use the same effective preferred/default width and
    // scaling rule as --max-mem planning; an explicit hard cap still wins.
    let mem_max = crate::sizing::step_mem_cap_for_inner_jobs_optional(
        &step,
        cpu_count,
        mem_cap_factor,
        default_step_mem_cap_bytes,
    );
    // CPU-time budget: declared cpu_timeout (>0) wins, else the small 10-s default.
    let cpu_canonical = canonical_cpu_timeout(&step, default_step_cpu_timeout);
    // The ENFORCED budget is the canonical one scaled for this platform; both are kept so a
    // breach can name the graph's number and the policy that changed it.
    let cpu_budget = scale_cpu_timeout(cpu_canonical, cpu_timeout_multiplier);
    // The wall ceiling this step actually runs under. A step that declared none carries the 0
    // sentinel and gets a backstop derived from its own CPU budget instead of the graph's
    // baked-in 1800 (see `resolved_wall_timeout`).
    let wall_budget = resolved_wall_timeout(&step, default_step_timeout, cpu_timeout_multiplier);
    // When boxing is enabled, prepare_command wraps the command so the bash leader self-moves into
    // the step's child cgroup BEFORE forking any grandchild (the cgroup-v2 fork-inheritance rule),
    // applying the inner memory/CPU caps. Disabled / absent -> the command is unchanged.
    let run_cmd = match &cgroups {
        Some(cg) if cg.enabled() => cg.prepare_command(&tag, &base_cmd, mem_max, cpu_count),
        _ => base_cmd,
    };

    // Parallel-speedup ENRICHMENT capture (only under real cgroup boxing, matching the Python
    // build). prepare_command has created the step's child cgroup, so cpu.pressure is readable;
    // bracket the step with two host-load snapshots so contention can be attributed later.
    let boxed = matches!(&cgroups, Some(cg) if cg.enabled());
    // WHICH LANE THIS STEP IS ON. Every guard below asks the capability registry about THIS lane
    // rather than about the engine in the abstract. The one deliberate exception is the
    // uncontained CPU-time fallback: it is attempted as a lower bound while the manifest stays
    // false because it cannot promise cgroup-equivalent enforcement.
    let lane = crate::capabilities::Lane::of_boxed(boxed);
    // Profile the width the step ACTUALLY ran under. An undeclared boxed command intentionally
    // gets no jobs flag but is constrained by the default per-step cpu.max, so it belongs in that
    // width bucket. Unboxed execution has no applied default cap and retains the ambient fallback.
    let profile_inner_jobs = inner_jobs.or(if boxed { cpu_count } else { None });
    let ambient_start = if boxed {
        Some(capture_ambient_snapshot(&[], None))
    } else {
        None
    };
    let step_pressure_start = if boxed {
        cgroups
            .as_ref()
            .and_then(|cg| psi_from(cg.cpu_pressure(&tag)))
    } else {
        None
    };

    // Per-step ownership nonce. Set BEFORE the step's own env so a DAG cannot overwrite it, and
    // inherited by every descendant through fork/exec — the only handle that still reaches a
    // double-fork escapee once it has left the step's process group AND its parentage. See
    // [`kill_by_nonce`].
    let nonce = mint_step_nonce(&tag);
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(&run_cmd);
    for (k, v) in &step.env {
        cmd.env(k, v);
    }
    // The outer step already owns its declared resource capacities. A nested runner belongs to
    // that step's process tree and must not ask for the same capacity again, which would wait on
    // itself forever. Independent top-level runners receive the path from their own launcher.
    cmd.env_remove(crate::resource_caps::PATH_ENV);
    if let Some((name, value)) = &jobs_env {
        // Runner authority wins over a DAG-supplied value on the same channel.
        cmd.env(name, value);
    }
    cmd.env(STEP_NONCE_ENV, &nonce);
    // Own process group (pgid == child pid) so teardown can reap the whole tree with a
    // negative-pid kill without ever touching the runner's own group.
    cmd.process_group(0);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let start = Instant::now();
    let started_offset_s = start.saturating_duration_since(run_origin).as_secs_f64();
    // Set this AFTER the step's declared environment so a manifest cannot forge the epoch that
    // downstream timeout ordering relies on.  The serialized clock is sampled beside the
    // supervisor's `Instant`; both precede spawn, so child setup spends from the same allowance.
    if let Some(started_ns) = monotonic_now_ns() {
        cmd.env(STEP_STARTED_MONOTONIC_NS_ENV, started_ns.to_string());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // Spawn failure: record a failed outcome so the run does not hang.
            let elapsed = start.elapsed().as_secs_f64();
            if let Some(cg) = &cgroups {
                if cg.enabled() {
                    cg.cleanup(&tag);
                }
            }
            let mut sh = lock_shared(&shared);
            retire(&mut sh, &step);
            let outcome = StepOutcome::failed(
                tag.clone(),
                elapsed,
                format!("spawn failed: {e}"),
                None,
                false,
                0,
                false,
                wall_budget,
                false,
                cpu_budget,
                cpu_canonical,
                cpu_timeout_multiplier,
                &cpu_timeout_platform,
                false,
                None,
                None,
            );
            sh.done.insert(tag.clone(), outcome);
            trip_fail_fast(&mut sh, &cgroups, keep_going, &tag);
            drop(sh);
            emit(&format!(
                "[{tag}] \u{2717} FAIL   {} (spawn failed: {e})",
                step.desc
            ));
            return;
        }
    };
    let pid = child.id();
    let abort_after_spawn = {
        let mut sh = lock_shared(&shared);
        sh.running_pids.insert(tag.clone(), pid);
        sh.running_nonces.insert(tag.clone(), nonce.clone());
        sh.active_processes += 1;
        sh.counted_processes.insert(tag.clone());
        sh.max_concurrent_steps = sh.max_concurrent_steps.max(sh.active_processes);
        sh.aborted.contains(&tag)
    };
    if abort_after_spawn {
        // A peer can fail after this tag was admitted but before its Popen registered. The
        // failing thread marks every pre-admitted tag aborted; honor that mark immediately so the
        // registration race cannot turn eager-exit into a full sibling wait.
        reap(&cgroups, &tag, pid, Some(&nonce));
    }

    // ONE stream object shared by stdout and stderr, so the tracker sees the step's output in a
    // single order and a harness that reports progress on stderr is attributed just as well.
    let sink = Arc::new(StepStream::new(&tag, evidence.clone()));
    if let Some(e) = &evidence {
        e.record(
            "step_start",
            &[
                ("step", tag.clone()),
                ("pid", pid.to_string()),
                ("timeout_s", wall_budget.to_string()),
                ("cmd", run_cmd.clone()),
            ],
        );
    }

    // ONE RING FOR THE STEP, not one per stream, and the distinction is the whole ceiling.
    //
    // The number an operator sets is what the runner may hold for a step. Two rings of that size
    // hold TWICE it, and a step that writes to both pipes then reports the drop twice -- each
    // notice naming only its own half of the total -- while the sibling engine, which reads the
    // two streams merged into one, reports it once over the whole. Same flag, same step, two
    // different answers and two different peak footprints; the durable log has always merged
    // these two pipes into one order through the shared `sink` above, so a single ring is also
    // the shape the rest of this step already uses. `compare_capture_ceiling` now floods BOTH
    // streams, so a return to per-stream accounting fails the differential rather than silently
    // doubling the bound in one edition.
    let capture_ceiling = capture_max_bytes();
    let step_capture = Arc::new(Mutex::new(BoundedCapture::new(capture_ceiling)));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(
            out,
            Arc::clone(&step_capture),
            tag.clone(),
            verbosity,
            Arc::clone(&sink),
            Arc::new(Mutex::new(ConsoleTestIdentity::default())),
        ));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(
            err,
            Arc::clone(&step_capture),
            tag.clone(),
            verbosity,
            Arc::clone(&sink),
            Arc::new(Mutex::new(ConsoleTestIdentity::default())),
        ));
    }

    // PERIODIC PROGRESS, always on -- not gated on boxing like the monitor below, because a
    // silent phase is just as unreadable un-boxed. This reports COUNTS from the attribution
    // tracker the readers already feed, so it adds no parsing and cannot disagree with the
    // step's own output. A step with no test events (a build, a manifest gate) falls back to
    // elapsed time, which still separates "moving" from "stationary" for a reader watching.
    let (progress_stop, progress_stop_rx) = std::sync::mpsc::channel();
    let progress_thread = {
        let psink = Arc::clone(&sink);
        let ptag = tag.clone();
        let pstart = start;
        thread::spawn(move || {
            let tick = Duration::from_millis(200);
            let mut since = Duration::ZERO;
            loop {
                match progress_stop_rx.recv_timeout(tick) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                since += tick;
                if since < PROGRESS_INTERVAL {
                    continue;
                }
                since = Duration::ZERO;
                let p = psink.progress();
                let secs = pstart.elapsed().as_secs();
                if p.is_silent() {
                    emit(&format!(
                        "[{ptag}] \u{2026} {secs}s elapsed, no test events yet"
                    ));
                } else {
                    let live = if p.in_flight.is_empty() {
                        String::new()
                    } else {
                        let mut names: Vec<&str> = p.in_flight.iter().map(String::as_str).collect();
                        names.sort_unstable();
                        let shown = names.len().min(2);
                        let more = names.len() - shown;
                        let mut t = format!(", running {}", names[..shown].join(", "));
                        if more > 0 {
                            t.push_str(&format!(" (+{more} more)"));
                        }
                        t
                    };
                    emit(&format!(
                        "[{ptag}] \u{2026} {} test(s) done, {} started{live} — {secs}s elapsed",
                        p.completed, p.started
                    ));
                }
            }
        })
    };

    // Poll once per MONITOR_INTERVAL for two purposes: (1) a per-step peak descendant-thread
    // count when cgroup boxing exists, and (2) CPU-time budget enforcement. The exact cgroup
    // counter remains primary. Uncontained runs get a best-effort procfs process-group floor.
    // The contractual capability remains false for uncontained runs; the warning names this
    // lower-bound attempt without promoting it to cgroup-equivalent enforcement. The poll is
    // interruptible, so joining it at step end returns promptly.
    let (monitor_stop, monitor_stop_rx) = std::sync::mpsc::channel();
    let thread_peak = Arc::new(Mutex::new(None::<i64>));
    let cpu_exceeded = Arc::new(AtomicBool::new(false));
    let termination_culprit = Arc::new(Mutex::new(None::<Culprit>));
    let monitor: Option<thread::JoinHandle<()>> = if boxed || cpu_budget > 0 {
        let peak = Arc::clone(&thread_peak);
        let cpu_flag = Arc::clone(&cpu_exceeded);
        let cg = cgroups.clone();
        let t = tag.clone();
        let cpu_timeout = cpu_budget;
        let mpid = pid;
        let mnonce = nonce.clone();
        let mevidence = evidence.clone();
        let msink = Arc::clone(&sink);
        let mculprit = Arc::clone(&termination_culprit);
        let mstart = start;
        let mlane = lane;
        Some(thread::spawn(move || {
            // THE MONITOR IS THE ONLY ENFORCER of the per-step CPU-time budget, and nothing ever
            // joins it for a result. If it dies the budget is not merely unmeasured -- it stops
            // being enforced at all, silently, while still reading as configured. Say so out loud.
            // (#80 runner-supervisor-crash-loud)
            let body_tag = t.clone();
            let body = std::panic::catch_unwind(AssertUnwindSafe(move || {
                let mut since = Duration::ZERO;
                // One-shot, so an unmeasurable budget is stated once per step, not once per tick.
                let mut unmeasurable_warned = false;
                let tick = Duration::from_millis(50);
                loop {
                    match monitor_stop_rx.recv_timeout(tick) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    since += tick;
                    if since < MONITOR_INTERVAL {
                        continue;
                    }
                    since = Duration::ZERO;
                    if let Some(c) = &cg {
                        if let Some(n) = c.thread_count(&t) {
                            let mut p = peak.lock().unwrap();
                            *p = Some(p.map_or(n, |cur| cur.max(n)));
                        }
                    }
                    if cpu_timeout <= 0
                        || cpu_flag.load(Ordering::Relaxed)
                        || (boxed && !crate::capabilities::is_enforced("cpu_timeout", mlane))
                    {
                        continue;
                    }

                    let (measured, source) = if boxed {
                        (
                            cg.as_ref()
                                .and_then(|c| c.cpu_stats(&t))
                                .and_then(|stats| cpu_seconds_from_stats(&stats)),
                            CPU_SOURCE_CGROUP,
                        )
                    } else {
                        (subtree_cpu_seconds(mpid), CPU_SOURCE_PROCFS)
                    };

                    let Some(cpu_used_s) = measured else {
                        if !unmeasurable_warned {
                            unmeasurable_warned = true;
                            let mechanism = if source == CPU_SOURCE_CGROUP {
                                "cgroup cpu.stat usage_usec"
                            } else {
                                "procfs process-group CPU accounting"
                            };
                            eprintln!(
                                "[scheduler] \u{26a0} step {t:?}: {mechanism} is unavailable, \
                                 so the {cpu_timeout}s CPU-time budget CANNOT be enforced for \
                                 this step; only the wall timeout still applies."
                            );
                        }
                        continue;
                    };
                    if cpu_used_s < cpu_timeout as f64 {
                        continue;
                    }

                    cpu_flag.store(true, Ordering::Relaxed);
                    if source == CPU_SOURCE_PROCFS {
                        eprintln!(
                            "[scheduler] \u{26a0} step {t:?} exceeded its CPU budget \
                             ({cpu_used_s:.1}s observed >= {cpu_timeout}s) measured by PROCFS \
                             SUBTREE accounting because cgroup boxing is not established; this \
                             misses processes that leave the group and not-yet-reaped exits, so \
                             it is a floor on true CPU use."
                        );
                    }
                    let culprit = capture_termination_evidence(
                        &mevidence,
                        &msink,
                        &t,
                        mpid,
                        &mnonce,
                        TerminationBoundary {
                            event: "cpu_timeout",
                            unit: BudgetUnit::CpuSeconds,
                            limit_s: cpu_timeout,
                            // The very reading the comparison above was made on.
                            measured_s: cpu_used_s,
                            wall_elapsed_s: mstart.elapsed().as_secs_f64(),
                        },
                    );
                    if let Ok(mut slot) = mculprit.lock() {
                        *slot = Some(culprit);
                    }
                    reap(&cg, &t, mpid, Some(&mnonce));
                    return;
                }
            }));
            if let Err(payload) = body {
                let detail = panic_detail(payload.as_ref());
                warn(&format!(
                    "[scheduler] \u{26a0} step {body_tag:?}: the CPU-budget monitor thread DIED \
                     with a panic ({detail}). The {cpu_timeout}s CPU-time budget is NO LONGER \
                     ENFORCED for this step; only the wall timeout still applies."
                ));
            }
        }))
    } else {
        None
    };

    // Wait for the child, enforcing the per-step timeout by polling.
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                // `wall_timeout` likewise: the deadline is not even compared when the
                // registry says this engine does not enforce one on this lane, so the
                // advertisement and the wait agree by construction rather than by two people
                // remembering. The deadline itself is the DERIVED backstop, not `step.timeout`.
                if crate::capabilities::is_enforced("wall_timeout", lane)
                    && start.elapsed().as_secs() as i64 >= wall_budget
                {
                    timed_out = true;
                    // Freeze test + process evidence BEFORE SIGTERM. The subsequent reap retains
                    // the existing gentle TERM/flush window and only then escalates to SIGKILL.
                    let c = capture_termination_evidence(
                        &evidence,
                        &sink,
                        &tag,
                        pid,
                        &nonce,
                        TerminationBoundary {
                            event: "step_timeout",
                            unit: BudgetUnit::WallSeconds,
                            limit_s: wall_budget,
                            measured_s: start.elapsed().as_secs_f64(),
                            wall_elapsed_s: start.elapsed().as_secs_f64(),
                        },
                    );
                    if let Ok(mut slot) = termination_culprit.lock() {
                        *slot = Some(c);
                    }
                    reap(&cgroups, &tag, pid, Some(&nonce));
                    break wait_bounded(&mut child, &tag, POST_REAP_WAIT);
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                break child
                    .wait()
                    .unwrap_or_else(|_| std::process::ExitStatus::from_raw(9))
            }
        }
    };

    // The child has exited. Stop counting it before teardown and reader joins, which can outlive
    // the child when a grandchild keeps an output pipe open.
    {
        let mut sh = lock_shared(&shared);
        uncount_process(&mut sh, &tag);
    }

    // Reap the whole tree (cgroup.kill + killpg) so orphan grandchildren die now and the readers
    // see EOF; then stop the monitor and join the reader threads.
    reap(&cgroups, &tag, pid, Some(&nonce));
    let _ = monitor_stop.send(());
    if let Some(m) = monitor {
        join_bounded(m, &tag, "monitor", JOIN_WAIT);
    }
    let _ = progress_stop.send(());
    join_bounded(progress_thread, &tag, "progress", JOIN_WAIT);
    // BOUNDED, and this is the join that actually hung. A surviving escapee inherited the step's
    // stdout/stderr pipe write ends, so those pipes never reach EOF and a plain `join()` here
    // blocks FOREVER — measured: with the post-reap child wait already bounded to 30s, a planted
    // escapee still forced an external kill at 300s, which locates the block here rather than at
    // `child.wait()`. Blocking here loses the entire run: the scheduler never returns, so the
    // end-of-run profile flush never happens and the lane cannot report on its own failure.
    // Abandoning a reader costs one parked thread until the process exits; keeping the run's
    // measurements is worth far more.
    for r in readers {
        join_bounded(r, &tag, "output reader", JOIN_WAIT);
    }

    // TEST-LEVEL ATTRIBUTION, derived from the step's own output stream. Computed here, after the
    // readers have finished or been abandoned, so every byte the step managed to emit has been
    // seen. The verdict is only reported for a step that did NOT finish cleanly: naming a culprit
    // for a passing step would be noise, and naming one for a step that simply exited non-zero
    // would compete with the harness's own (better) failure report.
    let culprit: Option<Culprit> = if timed_out || cpu_exceeded.load(Ordering::Relaxed) {
        termination_culprit
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .or_else(|| Some(sink.culprit()))
    } else {
        None
    };

    // Read the step's cgroup measurements BEFORE cleanup() removes the child cgroup.
    // memory_events is read once and `oom` is taken from it, so the OOM count and the recorded
    // event counters can never disagree about the same step. applied_memory_max is the cap the
    // KERNEL held, not the cap that was requested: a peak is only interpretable against the
    // ceiling that was actually in force.
    let (memory_events, applied_memory_max, peak, cpu_stats) = match &cgroups {
        Some(cg) if cg.enabled() => (
            cg.memory_events(&tag),
            cg.applied_memory_max(&tag),
            cg.peak_bytes(&tag),
            cg.cpu_stats(&tag),
        ),
        _ => (None, None, None, None),
    };
    // `oom_detection` is the ATTRIBUTION, and it is what the `capabilities` manifest advertises:
    // unenforced means the oom_kill counter is not consulted, so nothing downstream can call a
    // failure an OOM. The rest of memory.events is a recorded measurement, not a guard, and is
    // kept either way — a profile that stops recording is a different loss from a guard that
    // stops guarding.
    let oom = if crate::capabilities::is_enforced("oom_detection", lane) {
        memory_events
            .as_ref()
            .and_then(|events| events.get("oom_kill").copied())
            .unwrap_or(0)
    } else {
        0
    };
    let step_pressure_end = if boxed {
        cgroups
            .as_ref()
            .and_then(|cg| psi_from(cg.cpu_pressure(&tag)))
    } else {
        None
    };
    let ambient_end = if boxed {
        Some(capture_ambient_snapshot(&[], None))
    } else {
        None
    };
    if let Some(cg) = &cgroups {
        if cg.enabled() {
            cg.cleanup(&tag);
        }
    }
    let thread_peak = *thread_peak.lock().unwrap();
    let cpu_timed_out = cpu_exceeded.load(Ordering::Relaxed);

    let elapsed = start.elapsed().as_secs_f64();
    let dur = elapsed.round() as i64;
    let returncode: Option<i64> = match status.code() {
        Some(c) => Some(c as i64),
        None => status.signal().map(|s| -(s as i64)),
    };
    let ok = returncode == Some(0) && !timed_out && !cpu_timed_out;

    // The step's captured output -- both pipes, in arrival order -- for the summary + failure
    // detail. ONE tail, because there is one ring (see `step_capture`).
    let (combined, captured_total, captured_dropped) = {
        let cap = step_capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (cap.tail(), cap.total, cap.dropped())
    };
    let summary = last_line(&combined);
    let test_counts = captured_test_counts(&combined);

    // Build the per-step profile row (perflog step-profile schema keys + dynamic cpu.* counters).
    let mut row: ProfileRow = BTreeMap::new();
    row.insert("step".into(), tag.clone());
    row.insert(
        "classification".into(),
        step_classification(&step).value().into(),
    );
    // Resolve to a NUMBER (the effective parallelism the step ran in) — never the old "ambient"
    // string — so the speedup model can group samples by parallelism level.
    row.insert(
        "inner_jobs".into(),
        resolve_effective_inner_jobs(profile_inner_jobs).to_string(),
    );
    row.insert("elapsed_s".into(), format!("{elapsed:.3}"));
    row.insert(
        "returncode".into(),
        returncode.map(|c| c.to_string()).unwrap_or_default(),
    );
    row.insert("ok".into(), ok.to_string());
    row.insert("timed_out".into(), timed_out.to_string());
    // CPU-time budget enforcement is now at parity with the Python runner: emit the
    // real breach flag so the on-disk schema stays byte-identical across implementations.
    row.insert("cpu_timed_out".into(), cpu_timed_out.to_string());
    row.insert("oom_kills".into(), oom.to_string());
    row.insert(
        "peak_bytes".into(),
        peak.map(|p| p.to_string()).unwrap_or_default(),
    );
    row.insert(
        "thread_peak".into(),
        thread_peak.map(|t| t.to_string()).unwrap_or_default(),
    );
    // Run-overlap + applied-cap provenance. Offsets share one monotonic run origin, so two rows of
    // the same run_id overlap iff their [started, finished] intervals do.
    row.insert("started_offset_s".into(), format!("{started_offset_s:.3}"));
    row.insert(
        "finished_offset_s".into(),
        format!(
            "{:.3}",
            Instant::now()
                .saturating_duration_since(run_origin)
                .as_secs_f64()
        ),
    );
    // Blank means UNKNOWN and the literal "max" means unbounded; they are not the same answer and
    // the writer must not flatten one into the other.
    row.insert(
        "memory_max_bytes".into(),
        applied_memory_max.clone().unwrap_or_default(),
    );
    // memory.events counters, which need no subtraction to be per-step deltas (the child cgroup
    // lives exactly as long as the step). Left blank wholesale when the file could not be read, so
    // "the step had no such event" and "we never looked" stay distinct.
    for counter in ["low", "high", "max", "oom", "oom_kill"] {
        row.insert(
            format!("memory_events_{counter}"),
            memory_events.as_ref().map_or_else(String::new, |events| {
                events.get(counter).copied().unwrap_or(0).to_string()
            }),
        );
    }
    if let Some(stats) = &cpu_stats {
        for (k, v) in stats {
            row.insert(format!("cpu.{k}"), v.to_string());
        }
    }
    // Rich parallel-speedup enrichment columns (effective_cores, throttled_s, contention, PSI).
    // Only under real boxing; an un-boxed run leaves them blank (the writer fills them from the
    // STEP_PROFILE_COLUMNS schema), matching the Python build's "blank when unavailable" posture.
    if boxed {
        for (k, v) in step_enrichment_columns(
            elapsed,
            profile_inner_jobs,
            cpu_stats.as_ref(),
            ambient_start.as_ref(),
            ambient_end.as_ref(),
            step_pressure_start.as_ref(),
            step_pressure_end.as_ref(),
        ) {
            row.insert(k, v);
        }
    }

    let (was_aborted, cut_by_run_budget, reason) = {
        let mut sh = lock_shared(&shared);
        retire(&mut sh, &step);
        sh.step_profile_rows.push(row);
        let was_aborted = sh.aborted.contains(&tag);
        // Distinguish the two ways a step gets cancelled. "Another step failed" and "the whole run
        // ran out of budget" call for completely different follow-up, and reporting both with the
        // eager-exit wording sends a reader hunting for a failing peer that does not exist.
        let cut_by_run_budget = was_aborted && sh.run_timed_out;
        let outcome = if was_aborted {
            StepOutcome::aborted_outcome(
                tag.clone(),
                elapsed,
                summary.clone(),
                returncode,
                test_counts.executed,
                test_counts.filtered,
            )
        } else if ok {
            StepOutcome::passed(
                tag.clone(),
                elapsed,
                summary.clone(),
                returncode,
                test_counts.executed,
                test_counts.filtered,
            )
        } else {
            StepOutcome::failed(
                tag.clone(),
                elapsed,
                summary.clone(),
                returncode,
                oom > 0, // oomed: a step (or descendant) hit its inner memory.max
                oom,
                timed_out,
                wall_budget,
                cpu_timed_out,
                cpu_budget,
                cpu_canonical,
                cpu_timeout_multiplier,
                &cpu_timeout_platform,
                false,
                test_counts.executed,
                test_counts.filtered,
            )
        };
        let reason = outcome.reason.clone();
        sh.done.insert(tag.clone(), outcome);
        if !was_aborted && !ok {
            // A REAL failure. Eager-exit (default) stops launching NEW steps and reaps every
            // step still running so a fast failure does not wait for a slow in-flight build.
            // keep_going records the failure but leaves scheduling open: independent ready steps
            // continue, while dependency-failure closure skips only true dependents.
            trip_fail_fast(&mut sh, &cgroups, keep_going, &tag);
        }
        (was_aborted, cut_by_run_budget, reason)
    };

    // Emit the terminal status OUTSIDE the lock.
    if cut_by_run_budget {
        emit(&format!(
            "[{tag}] \u{2298} ABORT  {} ({dur}s \u{2014} cut short by the OUTER run budget, not \
             by a failure of its own or of a peer)",
            step.desc
        ));
    } else if was_aborted {
        emit(&format!(
            "[{tag}] \u{2298} ABORT  {} ({dur}s \u{2014} eager-exit after another step failed; --keep-going would continue independent work)",
            step.desc
        ));
    } else if ok {
        let extra = if !summary.is_empty() && verbosity >= 1 {
            format!("  [{summary}]")
        } else {
            String::new()
        };
        emit(&format!(
            "[{tag}] \u{2713} PASS   {} ({dur}s){extra}",
            step.desc
        ));
    } else {
        if let Some(c) = &culprit {
            // NAME THE TEST, NOT JUST THE NODE. "the strict-compat step timed out" is not
            // actionable; "test X started and never completed, 37 tests in" is.
            emit(&format!("[{tag}] \u{21b3} {}", c.describe()));
            if let Some(e) = &evidence {
                emit(&format!(
                    "[{tag}] \u{21b3} full step output preserved at {}/{}.log",
                    e.dir().display(),
                    crate::attribution::sanitize(&tag)
                ));
            }
        }
        emit(&format!(
            "[{tag}] \u{2717} FAIL   {} ({dur}s, {reason})",
            step.desc
        ));
        if oom > 0 {
            emit(&format!(
                "[{tag}] \u{25b2} MEMORY CAP HIT: OOM-killed at its inner cgroup MemoryMax \
                 (cap\u{2248}{}, peak\u{2248}{}). Confirm this is genuine growth, not an unbounded \
                 leak, before raising the step's rss_baseline_bytes / hard_mem_max_bytes hint.",
                fmt_bytes(mem_max),
                fmt_bytes(peak)
            ));
        }
        emit(&format!("[{tag}] ----- detail -----"));
        // The dump is a TAIL (see `BoundedCapture`), so when anything was dropped it says so IN
        // BAND and in numbers, rather than presenting a partial dump as the whole output. ONE
        // notice for the step, over the step's whole output: two, each counting one pipe, would
        // tell a reader neither how much the step produced nor how much survived.
        if captured_dropped {
            emit(&format!(
                "[{tag}] {}",
                capture_truncation_notice(captured_total, combined.len())
            ));
        }
        for line in failure_detail_lines(&tag, &[&combined], verbosity) {
            emit(&line);
        }
        emit(&format!("[{tag}] ----- end detail -----"));
    }

    // Terminal record. Written for EVERY step, pass or fail, so the journal alone answers "what
    // was this run doing" without needing the end-of-run profile rows that a hard kill destroys.
    if let Some(e) = &evidence {
        let counts = sink.counts();
        let mut fields = vec![
            ("step", tag.clone()),
            ("ok", ok.to_string()),
            ("aborted", was_aborted.to_string()),
            ("timed_out", timed_out.to_string()),
            ("cpu_timed_out", cpu_timed_out.to_string()),
            ("wall_elapsed_s", format!("{elapsed:.3}")),
            ("tests_started", counts.started.to_string()),
            ("tests_completed", counts.completed.to_string()),
            (
                "culprit_test",
                culprit
                    .as_ref()
                    .and_then(|c| c.test.clone())
                    .unwrap_or_default(),
            ),
        ];
        // The two ceilings this step ran under, each named for the quantity it bounds. Recorded
        // only when they are live, so a disabled budget stays absent instead of reading as 0.
        if cpu_budget > 0 {
            fields.push(("cpu_limit_s", cpu_budget.to_string()));
        }
        if wall_budget > 0 {
            fields.push(("wall_limit_s", wall_budget.to_string()));
        }
        fields.extend(cpu_journal_fields(cpu_stats.as_ref()));
        e.record("step_end", &fields);
    }
}

/// Run a whole DAG and return its [`RunResult`] (no cgroup boxing, no metrics recording).
///
/// * `combined_limit`: compatibility combined limit: both the maximum active-step count and the
///   maximum runner-controlled width of each individual step, clamped to at least 1. Concurrent
///   widths may sum above it. Use [`run_dag_limited`] when those values differ.
/// * `keep_going`: after a failure, keep launching independent ready steps and let running steps
///   finish; only true dependents are skipped.
/// * `verbosity`: 0 quiet (+failures), 1 default (+summaries), 2-4 stream child stdout,
///   and >=5 streams with the deepest recognized test identity on every line.
pub fn run_dag(
    cfg: &DagConfig,
    combined_limit: i64,
    keep_going: bool,
    verbosity: i64,
) -> RunResult {
    run_dag_limited(cfg, combined_limit, combined_limit, keep_going, verbosity)
}

/// Run a whole DAG with independent active-step and per-step CPU-width limits.
///
/// `max_cpus` caps each runner-controlled step width. Concurrent widths may sum above it. This
/// unboxed library helper does not establish the outer cgroup that enforces whole-run bandwidth.
pub fn run_dag_limited(
    cfg: &DagConfig,
    max_steps: i64,
    max_cpus: i64,
    keep_going: bool,
    verbosity: i64,
) -> RunResult {
    run_dag_boxed_limited(cfg, max_steps, max_cpus, keep_going, verbosity, None)
}

/// Run a whole DAG with an optional per-step cgroup manager (the real-work entry point).
///
/// `cgroups` supplies two-level cgroup-v2 per-step boxing + setsid-proof teardown when enabled;
/// `None` (or a disabled manager) runs unboxed with process-group teardown. Per-step measurement
/// rows are always collected into [`RunResult::step_profile_rows`] for a metrics sink to record.
pub fn run_dag_boxed(
    cfg: &DagConfig,
    combined_limit: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
) -> RunResult {
    run_dag_boxed_limited(
        cfg,
        combined_limit,
        combined_limit,
        keep_going,
        verbosity,
        cgroups,
    )
}

/// Boxed form of [`run_dag_limited`].
pub fn run_dag_boxed_limited(
    cfg: &DagConfig,
    max_steps: i64,
    max_cpus: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
) -> RunResult {
    run_dag_boxed_ordered_limited(
        cfg, max_steps, max_cpus, keep_going, verbosity, cgroups, None,
    )
}

/// Like [`run_dag_boxed`] but with an explicit dispatch `order` (e.g. a critical-path planner's)
/// and the compatibility `max_cpus` option. `max_cpus` caps each individual runner-controlled
/// step width; concurrent steps may have widths whose sum exceeds it. Aggregate bandwidth
/// containment remains the caller's responsibility. New callers that need separate
/// active-step and per-step-width limits should use [`run_dag_boxed_ordered_limited`].
pub fn run_dag_boxed_ordered(
    cfg: &DagConfig,
    max_steps: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
    max_cpus: Option<i64>,
) -> RunResult {
    run_dag_boxed_ordered_limited(
        cfg,
        max_steps,
        max_cpus.unwrap_or(max_steps),
        keep_going,
        verbosity,
        cgroups,
        order,
    )
}

/// Boxed ordered run with independent active-step and per-step CPU-width limits.
#[allow(clippy::too_many_arguments)]
pub fn run_dag_boxed_ordered_limited(
    cfg: &DagConfig,
    max_steps: i64,
    max_cpus: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
) -> RunResult {
    run_dag_boxed_deadline_limited(
        cfg, max_steps, max_cpus, keep_going, verbosity, cgroups, order, None,
    )
}

/// Steps whose own wall budget is not STRICTLY SMALLER than the run's outer budget.
///
/// INNER < OUTER IS THE WHOLE ORDERING, and it is checkable before anything runs. A step allowed
/// to run as long as (or longer than) the run itself can only ever be terminated by the outer
/// bound, which means the failure is attributed to "the run overran" instead of to the node that
/// caused it — precisely the report that made a real regression unexplainable. Returns the
/// offending `(tag, step_timeout_s)` pairs, empty when the ordering holds.
pub fn steps_violating_run_timeout(cfg: &DagConfig, run_timeout_s: i64) -> Vec<(String, i64)> {
    if run_timeout_s <= 0 {
        return Vec::new();
    }
    // The RESOLVED bound, not the declared field. A step that declares no wall budget carries
    // the 0 sentinel, which would pass a `>= run_timeout_s` test trivially while the value it
    // will actually run under — derived from its CPU budget, or 1800 — might not.
    let mut bad: Vec<(String, i64)> = cfg
        .steps
        .iter()
        .map(|s| {
            (
                s.tag(),
                resolved_wall_timeout(s, cfg.default_step_timeout, cfg.cpu_timeout_multiplier),
            )
        })
        .filter(|(_, bound)| *bound >= run_timeout_s)
        .collect();
    bad.sort();
    bad
}

/// Like [`run_dag_boxed_ordered`] plus an OUTER wall budget for the whole run.
///
/// `run_timeout_s = None` (or a non-positive value) leaves the run unbounded. When set, the
/// scheduler stops launching, terminates every in-flight step's whole tree, and RETURNS with
/// [`RunResult::run_timed_out`] set — it does not abandon the process to an outside killer,
/// because an outside kill takes the evidence with it.
///
/// FAIL CLOSED ON A MIS-ORDERED BUDGET. If any step is allowed to run as long as the whole run,
/// this REFUSES to start rather than running with an ordering that cannot attribute a failure. A
/// bound you cannot attribute is worth less than no bound, because it reads like enforcement.
#[allow(clippy::too_many_arguments)]
pub fn run_dag_boxed_deadline(
    cfg: &DagConfig,
    max_steps: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
    max_cpus: Option<i64>,
    run_timeout_s: Option<i64>,
) -> RunResult {
    run_dag_boxed_deadline_limited(
        cfg,
        max_steps,
        max_cpus.unwrap_or(max_steps),
        keep_going,
        verbosity,
        cgroups,
        order,
        run_timeout_s,
    )
}

/// Deadline-aware boxed run with independent active-step and per-step CPU-width limits.
#[allow(clippy::too_many_arguments)]
pub fn run_dag_boxed_deadline_limited(
    cfg: &DagConfig,
    max_steps: i64,
    max_cpus: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
    run_timeout_s: Option<i64>,
) -> RunResult {
    // FIRST, before anything walks the graph. The loader already refuses these, so this is the
    // door a LIBRARY caller comes through with a hand-built DagConfig -- and a cycle reaching the
    // planner is not a bad report, it is a stack-overflow abort (core dump) in this edition and a
    // RecursionError in the Python edition.
    let structural = graph_structure_violations(cfg);
    if !structural.is_empty() {
        eprintln!(
            "[scheduler] ERROR: REFUSING to run before any node starts: {} graph error(s): {}",
            structural.len(),
            structural.join("; ")
        );
        return RunResult {
            ok: false,
            wall_s: 0.0,
            outcomes: Vec::new(),
            skipped: Vec::new(),
            not_launched: Vec::new(),
            intentional_skips: Vec::new(),
            step_profile_rows: Vec::new(),
            run_timed_out: false,
            max_concurrent_steps: 0,
        };
    }
    if let Err(error) = validate_max_cpus_rewrite(cfg, max_cpus) {
        eprintln!("[scheduler] ERROR: REFUSING to run before any node starts: {error}");
        return RunResult {
            ok: false,
            wall_s: 0.0,
            outcomes: Vec::new(),
            skipped: Vec::new(),
            not_launched: Vec::new(),
            intentional_skips: Vec::new(),
            step_profile_rows: Vec::new(),
            run_timed_out: false,
            max_concurrent_steps: 0,
        };
    }
    let domain_errors = write_domain_violations(cfg);
    if !domain_errors.is_empty() {
        eprintln!(
            "[scheduler] ERROR: REFUSING to run before any node starts: write-domain policy \
             violation(s): {}",
            domain_errors.join("; ")
        );
        return RunResult {
            ok: false,
            wall_s: 0.0,
            outcomes: Vec::new(),
            skipped: Vec::new(),
            not_launched: Vec::new(),
            intentional_skips: Vec::new(),
            step_profile_rows: Vec::new(),
            run_timed_out: false,
            max_concurrent_steps: 0,
        };
    }
    // ABSENT IS NOT ZERO. Left to the ready-set loop this is an infinite 50 ms sleep at 0% CPU
    // with nothing printed — indistinguishable from a deliberate cap of 0, and from a hang. Name
    // it here, before anything can wait on it.
    let undeclared = undeclared_resource_demands(cfg);
    if !undeclared.is_empty() {
        eprintln!(
            "[scheduler] ERROR: REFUSING to run before any node starts: {} step/resource pair(s) \
             demand a named resource with NO declared cap in resource_caps, so they can never \
             become ready: {}. Declare the capacity, or set the cap to 0 explicitly to block them \
             on purpose.",
            undeclared.len(),
            undeclared.join("; ")
        );
        return RunResult {
            ok: false,
            wall_s: 0.0,
            outcomes: Vec::new(),
            skipped: Vec::new(),
            // A pre-flight refusal launched nothing, so nothing was LEFT unlaunched by a failure
            // either. Same shape as the two refusal paths above it.
            not_launched: Vec::new(),
            intentional_skips: Vec::new(),
            step_profile_rows: Vec::new(),
            run_timed_out: false,
            max_concurrent_steps: 0,
        };
    }
    if let Some(limit) = run_timeout_s.filter(|s| *s > 0) {
        let bad = steps_violating_run_timeout(cfg, limit);
        if !bad.is_empty() {
            let detail = bad
                .iter()
                .map(|(t, s)| format!("{t} ({s}s)"))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "[scheduler] ERROR: REFUSING to run: the outer run budget is {limit}s but {} \
                 step(s) declare a wall budget at least that large, so the outer bound would fire \
                 before they do and the failure could not be attributed to a node: {detail}. \
                 Lower those step timeouts or raise --run-timeout.",
                bad.len()
            );
            return RunResult {
                ok: false,
                wall_s: 0.0,
                outcomes: Vec::new(),
                skipped: Vec::new(),
                not_launched: Vec::new(),
                intentional_skips: Vec::new(),
                step_profile_rows: Vec::new(),
                run_timed_out: false,
                max_concurrent_steps: 0,
            };
        }
    }
    if let Some(cg) = &cgroups {
        if !cg.enabled() {
            // No Silent Failure: a present-but-disabled manager means containment is degraded.
            eprintln!(
                "[scheduler] WARNING: per-step cgroup manager is present but disabled; containment \
                 is DEGRADED (process-group kill only, no inner memory/CPU caps)."
            );
        }
    }
    if !matches!(&cgroups, Some(cg) if cg.enabled()) {
        // NAME THE GUARD THAT IS NOT RUNNING, not just the containment state. Covers every
        // uncontained lane at once — no manager at all, or a present-but-disabled one — because
        // both read `cpu.stat` exactly zero times.
        if let Some(notice) = uncontained_cpu_budget_warning(cfg) {
            eprintln!("[scheduler] \u{26a0} {notice}");
        }
    }
    let resource_coordinator = match ResourceCoordinator::from_env(&cfg.resource_caps) {
        Ok(coordinator) => coordinator,
        Err(error) => {
            eprintln!(
                "[scheduler] ERROR: REFUSING to run before any node starts: resource_caps shared state is unusable: {error}"
            );
            return RunResult {
                ok: false,
                wall_s: 0.0,
                outcomes: Vec::new(),
                skipped: Vec::new(),
                not_launched: Vec::new(),
                intentional_skips: Vec::new(),
                step_profile_rows: Vec::new(),
                run_timed_out: false,
                max_concurrent_steps: 0,
            };
        }
    };
    let runner = Runner::new(
        cfg,
        max_steps,
        max_cpus,
        keep_going,
        verbosity,
        cgroups,
        order,
        run_timeout_s,
        resource_coordinator,
    );
    // ANNOUNCE THE EVIDENCE PATH ONCE, AT THE START. A durable log nobody can find is not
    // evidence; and printing it only on failure is printing it only where the run still had a
    // chance to print something. Stated up front, it is on the console even for a run that is
    // later cancelled from outside.
    if let Some(e) = &runner.evidence {
        eprintln!(
            "[scheduler] per-step logs + test-boundary journal: {} (set {} to relocate, {}=1 to \
             disable)",
            e.dir().display(),
            crate::attribution::LOG_DIR_ENV,
            crate::attribution::NO_LOGS_ENV,
        );
    }
    // CONTAINMENT GOES INTO THE RECORD, NOT ONLY INTO A WARNING THAT SCROLLS PAST.
    //
    // A run used to say once, on stderr, that it was unboxed, and then no artifact carried it. Any
    // run reviewed later therefore could not be told apart from a boxed one — which is why
    // establishing whether CI boxes at all took four probes and three confident wrong answers: the
    // evidence had never existed. This writes the state into the durable journal and prints one
    // machine-readable line a banner or ledger can copy verbatim rather than re-derive.
    //
    // ABSENT MUST NOT MEAN BOXED, so the state is written on EVERY path, `unknown` included.
    let containment = crate::cgroup::run_containment(runner.cgroups.as_deref());
    eprintln!(
        "[scheduler] CONTAINMENT: {} — {}",
        containment.label(),
        containment.describe()
    );
    if let Some(e) = &runner.evidence {
        e.record(
            "containment",
            &[
                ("state", containment.label().to_string()),
                ("caps_enforced", containment.caps_enforced().to_string()),
                (
                    "subtree_killable",
                    containment.subtree_killable().to_string(),
                ),
                ("detail", containment.describe()),
            ],
        );
    }
    let (_ok, wall) = runner.run();
    runner.result(wall)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_capture_counts_tests_independently_of_console_verbosity() {
        let counts = captured_test_counts(
            b"running 2 tests\ntest result: ok. 1 passed; 0 failed; 1 ignored; 7 filtered out\n\
              running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 4 filtered out\n",
        );
        // `running` is authoritative, so ignored tests remain part of the
        // executed denominator exactly as in the canonical Python parser.
        assert_eq!(
            counts,
            CapturedTestCounts {
                executed: Some(5),
                filtered: Some(11)
            }
        );

        let fallback = captured_test_counts(
            b"test result: ok. 9 passed; 0 failed; 0 ignored; 2 filtered out\n",
        );
        assert_eq!(
            fallback,
            CapturedTestCounts {
                executed: Some(9),
                filtered: Some(2)
            }
        );
    }

    #[test]
    fn complete_capture_keeps_zero_distinct_from_unknown() {
        assert_eq!(
            captured_test_counts(
                b"running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 filtered out\n"
            ),
            CapturedTestCounts {
                executed: Some(0),
                filtered: Some(0)
            }
        );
        assert_eq!(
            captured_test_counts(b"build completed successfully\n"),
            CapturedTestCounts {
                executed: None,
                filtered: None
            }
        );
    }

    #[test]
    fn level_five_console_lines_always_name_the_deepest_test_identity() {
        let mut context = ConsoleTestIdentity::default();
        assert_eq!(
            context.decorate("step.outer", "##TEST-START suite::case"),
            "[step.outer][test=suite::case] ##TEST-START suite::case"
        );
        assert_eq!(
            context.decorate("step.outer", ":: Run1..."),
            "[step.outer][test=suite::case] :: Run1..."
        );
        assert_eq!(
            context.decorate("step.outer", "##TEST-END suite::case PASS"),
            "[step.outer][test=suite::case] ##TEST-END suite::case PASS"
        );
        assert_eq!(
            context.decorate("step.outer", "harness teardown"),
            "[step.outer][test=step.outer] harness teardown"
        );
    }

    #[test]
    fn level_five_failure_replay_preserves_identity_without_cross_stream_borrowing() {
        let stdout = b"##TEST-START stdout::case\nstdout body\n";
        let stderr = b"##TEST-START stderr::failure\nfull-error-line\n";
        let lines = failure_detail_lines("step.outer", &[stdout, stderr], 5);
        assert_eq!(
            lines,
            vec![
                "[step.outer][test=stdout::case] ##TEST-START stdout::case",
                "[step.outer][test=stdout::case] stdout body",
                "[step.outer][test=stderr::failure] ##TEST-START stderr::failure",
                "[step.outer][test=stderr::failure] full-error-line",
            ]
        );

        // A marker on stdout must never misattribute an unrelated stderr line.
        let split = failure_detail_lines(
            "step.outer",
            &[b"##TEST-START stdout::case\n", b"stderr raced first\n"],
            5,
        );
        assert_eq!(split[1], "[step.outer][test=step.outer] stderr raced first");
    }
    use crate::model::{IntentionalSkipReason, ResourceHint, StepClass};
    use std::collections::BTreeMap;

    #[test]
    fn proc_stat_parser_excludes_zombies_from_the_term_grace() {
        assert_eq!(
            live_process_group_from_stat("123 (worker ) with parens) Z 1 777 0 0"),
            None
        );
        assert_eq!(
            live_process_group_from_stat("456 (worker ) with parens) S 1 888 0 0"),
            Some(888)
        );
        assert_eq!(live_process_group_from_stat("malformed"), None);
        assert_eq!(live_process_group_from_stat("1 (x) S 0 nope"), None);
    }

    fn step(
        group: &str,
        job: &str,
        cmd: &str,
        deps: &[&str],
        est: f64,
        res: &[(&str, i64)],
    ) -> Step {
        let mut resources = BTreeMap::new();
        for (k, v) in res {
            resources.insert(k.to_string(), *v);
        }
        Step {
            group: group.into(),
            job: job.into(),
            desc: String::new(),
            description: String::new(),
            cmd: cmd.into(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            hint: ResourceHint {
                resources,
                est_duration_s: est,
                ..Default::default()
            },
            networkonly: false,
            engine_only: false,
            timeout: 1800,
            cpu_timeout: 0,
            jobs_flag: None,
            jobs_env: None,
            skip_reason: None,
            write_domains: None,
            write_domain_guarantee: None,
            explains: Vec::new(),
            fail_fast_family: None,
        }
    }

    // ------------------------------------------------- the uncontained CPU-budget notice
    //
    // A declared `cpu_timeout` uses exact cgroup accounting when boxed and a best-effort procfs
    // process-group floor when uncontained, so the run owes its operator a line naming the
    // degraded guarantee. A graph that switched the guard off everywhere must NOT be nagged.

    fn cpu_budget_step(group: &str, job: &str, cpu_timeout: i64) -> Step {
        let mut s = step(group, job, "true", &[], 0.0, &[]);
        s.cpu_timeout = cpu_timeout;
        s
    }

    #[test]
    fn the_uncontained_notice_counts_the_budgets_and_names_the_largest() {
        let cfg = DagConfig {
            steps: vec![cpu_budget_step("g", "a", 7), cpu_budget_step("g", "b", 3)],
            ..Default::default()
        };
        let notice = uncontained_cpu_budget_warning(&cfg).expect("a live budget must be named");
        assert!(notice.contains("UNCONTAINED run"), "{notice}");
        assert!(notice.contains("best-effort procfs"), "{notice}");
        assert!(notice.contains("will police 2 step(s)"), "{notice}");
        assert!(notice.contains("largest 7s"), "{notice}");
    }

    #[test]
    fn the_default_budget_counts_for_the_best_effort_floor() {
        // A step that declares nothing still gets `default_step_cpu_timeout`, and that budget is
        // policed by the same best-effort floor as a declared one.
        let cfg = DagConfig {
            steps: vec![cpu_budget_step("g", "a", 0)],
            ..Default::default()
        };
        let notice = uncontained_cpu_budget_warning(&cfg).expect("the default budget counts");
        assert!(notice.contains("will police 1 step(s)"), "{notice}");
        assert!(notice.contains("largest 10s"), "{notice}");
    }

    #[test]
    fn a_graph_with_the_guard_switched_off_everywhere_is_not_nagged() {
        let cfg = DagConfig {
            steps: vec![cpu_budget_step("g", "a", 0)],
            default_step_cpu_timeout: 0,
            ..Default::default()
        };
        assert!(uncontained_cpu_budget_warning(&cfg).is_none());
    }

    #[test]
    fn the_platform_multiplier_is_applied_before_the_notice_quotes_a_number() {
        // The notice must quote the budget that WOULD have been enforced, not the graph's
        // canonical number, or a slow platform reads a figure that never existed.
        let cfg = DagConfig {
            steps: vec![cpu_budget_step("g", "a", 4)],
            cpu_timeout_multiplier: 2.5,
            ..Default::default()
        };
        let notice = uncontained_cpu_budget_warning(&cfg).expect("a live budget must be named");
        assert!(notice.contains("largest 10s"), "{notice}");
    }

    // ------------------------------------------------------- terminal-starve brackets
    //
    // Both directions, with counts, because each leg alone is passed by a broken guard: a
    // detector that refuses EVERY run passes the negative legs, and a detector wired to nothing
    // passes the positive leg. If any negative test HANGS rather than fails, the detector has
    // regressed to the defect it exists to remove.

    fn caps(pairs: &[(&str, i64)]) -> BTreeMap<String, i64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn avail(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn indexed(steps: &[Step]) -> HashMap<String, Step> {
        steps.iter().map(|s| (s.tag(), s.clone())).collect()
    }

    #[test]
    fn positive_satisfiable_caps_yield_no_refusal_and_the_dag_still_runs() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "a", "true", &[], 0.0, &[("hg", 1)]),
                step("g", "b", "true", &[], 0.0, &[("hg", 1)]),
            ],
            resource_caps: caps(&[("hg", 1)]),
            ..Default::default()
        };
        let steps = indexed(&cfg.steps);
        assert_eq!(
            ungrantable_resources(
                &avail(&[("hg", 1)]),
                &steps,
                &["g.a".to_string(), "g.b".to_string()]
            )
            .len(),
            0,
            "positive: a satisfiable demand must produce ZERO refusals"
        );
        let res = run_dag(&cfg, 4, false, 0);
        assert!(res.ok, "positive: a satisfiable DAG must still run green");
        assert_eq!(res.outcomes.len(), 2, "positive: both steps executed");
        assert_eq!(res.skipped.len(), 0);
    }

    #[test]
    fn absent_cap_reads_differently_from_a_declared_zero() {
        // Same BEHAVIOR (both block), different DIAGNOSTIC. `unwrap_or(0)` alone makes these two
        // literally indistinguishable, which is the reason `Observed::Absent` exists.
        let steps = indexed(&[step("g", "needs_gpu", "true", &[], 0.0, &[("gpu", 1)])]);
        let tags = vec!["g.needs_gpu".to_string()];

        let missing = ungrantable_resources(&avail(&[("hg", 4)]), &steps, &tags);
        assert_eq!(missing.len(), 1, "absent: exactly 1 refusal");
        assert_eq!(missing[0].observed, Observed::Absent);
        let rendered = missing[0].to_string();
        assert!(
            rendered.contains("<absent>"),
            "absent must render distinctly: {rendered}"
        );
        assert!(
            rendered.contains("gpu=1"),
            "must name the demand: {rendered}"
        );
        assert!(
            rendered.contains("hg=4"),
            "must show what WAS declared, so a typo is spottable: {rendered}"
        );

        let zero = ungrantable_resources(&avail(&[("gpu", 0)]), &steps, &tags);
        assert_eq!(zero.len(), 1, "declared zero: exactly 1 refusal");
        assert_eq!(zero[0].observed, Observed::Present("gpu=0".into()));
        let rendered = zero[0].to_string();
        assert!(
            rendered.contains("gpu=0"),
            "a declared 0 must show its value: {rendered}"
        );
        assert!(
            !rendered.contains("<absent>"),
            "a declared 0 must NOT read as absent: {rendered}"
        );
    }

    #[test]
    fn ungrantable_cap_refuses_before_anything_runs() {
        // A DECLARED cap that can never grant the demand. This USED to be owned by the
        // terminal-starve detector, which could only see it after every unrelated step had
        // already run: "the satisfiable step still ran; only the starved one is refused" was the
        // pinned behaviour, and it meant a full build was spent before the graph was called
        // broken. `graph_structure_violations` sees it in pre-flight, from the document alone, so
        // NOTHING runs. MUST stay in step with the Python twin,
        // `test_ungrantable_cap_refuses_before_anything_runs`.
        let cfg = DagConfig {
            steps: vec![
                step("g", "needs_gpu", "true", &[], 0.0, &[("gpu", 4)]),
                step("g", "plain", "true", &[], 0.0, &[]),
            ],
            resource_caps: caps(&[("gpu", 1)]),
            ..Default::default()
        };
        let res = run_dag(&cfg, 4, false, 0);
        assert!(!res.ok, "an ungrantable demand must REFUSE, not hang");
        assert!(
            res.outcomes.is_empty(),
            "pre-flight refused, so NOTHING may have run"
        );
    }

    #[test]
    fn a_cap_declared_as_zero_stays_the_deliberate_block_the_guide_documents() {
        // The boundary of the pre-flight check, asserted from the other side so "refuse every
        // demand" cannot pass the case above. A cap of exactly 0 is documented as "blocked on
        // purpose", so it is NOT a load-time error; the terminal-starve detector still owns it,
        // which is why that detector is not dead code.
        let cfg = DagConfig {
            steps: vec![
                step("g", "blocked", "true", &[], 0.0, &[("gpu", 1)]),
                step("g", "plain", "true", &[], 0.0, &[]),
            ],
            resource_caps: caps(&[("gpu", 0)]),
            ..Default::default()
        };
        assert!(graph_structure_violations(&cfg).is_empty());
        let res = run_dag(&cfg, 4, false, 0);
        assert!(!res.ok, "the blocked step still cannot run");
        assert_eq!(
            res.outcomes.len(),
            1,
            "and the starve is still found the late way, by the detector that owns it"
        );
    }

    #[test]
    fn an_undeclared_resource_is_refused_before_anything_runs() {
        // The overlapping case, pinned to the EARLIER mechanism so the two cannot silently swap.
        // Twin of the Python `test_an_undeclared_resource_is_refused_before_anything_runs`.
        let cfg = DagConfig {
            steps: vec![
                step("g", "needs_gpu", "true", &[], 0.0, &[("gpu", 1)]),
                step("g", "plain", "true", &[], 0.0, &[]),
            ],
            resource_caps: caps(&[("hg", 4)]),
            ..Default::default()
        };
        let res = run_dag(&cfg, 4, false, 0);
        assert!(!res.ok);
        assert!(
            res.outcomes.is_empty(),
            "pre-flight refused, so NOTHING may have run"
        );
    }

    #[test]
    fn dependency_cycle_refuses_before_anything_runs_and_names_the_cycle() {
        // A cycle is not merely a starve: the planner's bottom-level walk recurses along it until
        // the stack overflows and the process ABORTS WITH A CORE DUMP. So it cannot be left to a
        // detector that runs after the acyclic steps have gone; it is refused up front, and the
        // refusal NAMES the cycle, because "there is a cycle" in a large graph is not actionable.
        let cfg = DagConfig {
            steps: vec![
                step("g", "a", "true", &["g.b"], 0.0, &[]),
                step("g", "b", "true", &["g.a"], 0.0, &[]),
                step("g", "ok", "true", &[], 0.0, &[]),
            ],
            ..Default::default()
        };
        assert_eq!(
            graph_structure_violations(&cfg),
            vec!["dependency cycle: g.a -> g.b -> g.a".to_string()]
        );
        let res = run_dag(&cfg, 4, false, 0);
        assert!(!res.ok, "a cycle must REFUSE, not hang and not abort");
        assert!(
            res.outcomes.is_empty(),
            "pre-flight refused, so NOTHING may have run"
        );
    }

    #[test]
    fn dangling_dep_refuses_instead_of_sleeping_forever() {
        let cfg = DagConfig {
            steps: vec![step("g", "a", "true", &["g.nonexistent"], 0.0, &[])],
            ..Default::default()
        };
        let res = run_dag(&cfg, 4, false, 0);
        assert!(!res.ok, "a dangling dep must REFUSE, not hang");
        assert_eq!(res.outcomes.len(), 0);
    }

    #[test]
    fn simple_dag_all_pass_respects_deps() {
        let dir = std::env::temp_dir().join(format!("dagrun_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let order_file = dir.join("order");
        let of = order_file.to_string_lossy().to_string();
        let cfg = DagConfig {
            steps: vec![
                step("g", "A", &format!("echo A >> {of}"), &[], 0.0, &[]),
                step("g", "B", &format!("echo B >> {of}"), &["g.A"], 0.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 4, false, 0);
        assert!(res.ok);
        let contents = std::fs::read_to_string(&order_file).unwrap();
        let seq: Vec<&str> = contents.split_whitespace().collect();
        assert_eq!(seq, vec!["A", "B"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_one_outcomes_retain_typed_counts_from_suppressed_output() {
        let cfg = DagConfig {
            steps: vec![
                step(
                    "g",
                    "counted",
                    "printf 'running 3 tests\\ntest result: ok. 3 passed; 0 failed; 5 filtered out\\n'",
                    &[],
                    0.0,
                    &[],
                ),
                step("g", "bannerless", "printf 'build only\\n'", &[], 0.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 2, false, 1);
        assert!(res.ok);
        let counted = res.outcomes.iter().find(|o| o.tag == "g.counted").unwrap();
        assert_eq!(
            (counted.executed_tests, counted.filtered_tests),
            (Some(3), Some(5))
        );
        let bannerless = res
            .outcomes
            .iter()
            .find(|o| o.tag == "g.bannerless")
            .unwrap();
        assert_eq!(
            (bannerless.executed_tests, bannerless.filtered_tests),
            (None, None)
        );
    }

    #[test]
    fn step_exports_unforgeable_monotonic_start_epoch() {
        let dir = std::env::temp_dir().join(format!("dagrun_epoch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let observed = dir.join("started-ns");
        let path = observed.to_string_lossy();
        let mut s = step(
            "g",
            "epoch",
            &format!("printf '%s\\n' \"${STEP_STARTED_MONOTONIC_NS_ENV}\" > {path}"),
            &[],
            0.0,
            &[],
        );
        // A manifest-provided value must not be able to forge the load-bearing start epoch.
        s.env
            .insert(STEP_STARTED_MONOTONIC_NS_ENV.to_string(), "1".to_string());
        let cfg = DagConfig {
            steps: vec![s],
            ..Default::default()
        };
        let before = monotonic_now_ns().unwrap();
        let res = run_dag(&cfg, 1, false, 0);
        let after = monotonic_now_ns().unwrap();
        assert!(res.ok);
        let exported: u64 = std::fs::read_to_string(&observed)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(exported >= before && exported <= after);
        assert_ne!(exported, 1, "step env forged the scheduler-owned epoch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dep_failure_skips_dependent() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "A", "exit 1", &[], 0.0, &[]),
                step("g", "B", "true", &["g.A"], 0.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 2, false, 0);
        assert!(!res.ok);
        let outcomes: HashMap<String, StepOutcome> = res
            .outcomes
            .iter()
            .map(|o| (o.tag.clone(), o.clone()))
            .collect();
        assert!(!outcomes["g.A"].ok);
        assert!(res.skipped.contains(&"g.B".to_string()));
        assert!(!outcomes.contains_key("g.B"));
    }

    #[test]
    fn intentional_skip_never_spawns_and_nonempty_peer_runs() {
        let dir = std::env::temp_dir().join(format!("dagrun_skip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let forbidden = dir.join("forbidden");
        let ran = dir.join("ran");
        let mut empty = step(
            "g",
            "empty",
            &format!("touch {}", forbidden.display()),
            &[],
            0.0,
            &[],
        );
        empty.skip_reason = Some(IntentionalSkipReason::EmptyManifestBucket);
        let cfg = DagConfig {
            steps: vec![
                empty,
                step(
                    "g",
                    "nonempty",
                    &format!("touch {}", ran.display()),
                    &[],
                    0.0,
                    &[],
                ),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 2, false, 0);
        assert!(res.ok);
        assert!(!forbidden.exists());
        assert!(ran.exists());
        assert_eq!(
            res.intentional_skips,
            vec![(
                "g.empty".to_string(),
                IntentionalSkipReason::EmptyManifestBucket,
            )]
        );
        assert!(res.skipped.is_empty());
        assert_eq!(
            res.outcomes
                .iter()
                .map(|o| o.tag.as_str())
                .collect::<Vec<_>>(),
            vec!["g.nonempty"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn eager_exit_aborts_inflight_step() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "fast", "sleep 0.2; exit 1", &[], 0.0, &[]),
                step("g", "slow", "sleep 5", &[], 100.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 2, false, 0);
        assert!(!res.ok);
        let outcomes: HashMap<String, StepOutcome> = res
            .outcomes
            .iter()
            .map(|o| (o.tag.clone(), o.clone()))
            .collect();
        assert!(!outcomes["g.fast"].ok);
        assert!(outcomes["g.slow"].aborted);
        assert!(!outcomes["g.slow"].ok);
    }

    /// Under the default eager-exit, later independent work lands in NO existing bucket: it is
    /// neither an outcome nor dependency-skipped. `not_launched` names it, so a caller counting
    /// only the first two cannot read the run as fully accounted for.
    #[test]
    fn fail_fast_reports_independent_step_as_not_launched() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "fail", "exit 1", &[], 100.0, &[]),
                step("g", "dependent", "true", &["g.fail"], 90.0, &[]),
                step("g", "independent", "true", &[], 80.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 1, false, 0);
        assert!(!res.ok);
        assert_eq!(
            res.outcomes
                .iter()
                .map(|o| o.tag.as_str())
                .collect::<Vec<_>>(),
            vec!["g.fail"]
        );
        assert_eq!(res.skipped, vec!["g.dependent"]);
        assert_eq!(res.not_launched, vec!["g.independent"]);
    }

    #[test]
    fn scoped_eager_exit_cancels_its_family_and_completes_an_independent_family() {
        let mut fail = step("family", "fail", "sleep 0.2; exit 1", &[], 100.0, &[]);
        let mut peer = step("family", "peer", "sleep 5", &[], 90.0, &[]);
        let mut independent = step("independent", "ok", "sleep 0.5; true", &[], 80.0, &[]);
        let mut dependent = step("family", "dependent", "true", &["family.fail"], 70.0, &[]);
        fail.fail_fast_family = Some("family-a".to_string());
        peer.fail_fast_family = Some("family-a".to_string());
        dependent.fail_fast_family = Some("family-a".to_string());
        independent.fail_fast_family = Some("family-b".to_string());
        let cfg = DagConfig {
            steps: vec![fail, peer, independent, dependent],
            ..Default::default()
        };

        let res = run_dag(&cfg, 3, false, 0);
        let outcomes: HashMap<String, StepOutcome> = res
            .outcomes
            .iter()
            .map(|outcome| (outcome.tag.clone(), outcome.clone()))
            .collect();

        assert!(!res.ok);
        assert!(!outcomes["family.fail"].ok);
        assert!(!outcomes["family.fail"].aborted);
        assert!(outcomes["family.peer"].aborted);
        assert_eq!(res.skipped, vec!["family.dependent"]);
        assert!(outcomes["independent.ok"].ok);
        assert!(!outcomes["independent.ok"].aborted);
    }

    #[test]
    fn scoped_eager_exit_does_not_launch_a_queued_family_peer() {
        let mut fail = step("family", "fail", "exit 1", &[], 100.0, &[]);
        let mut peer = step("family", "queued", "true", &[], 90.0, &[]);
        let mut independent = step("independent", "ok", "true", &[], 80.0, &[]);
        fail.fail_fast_family = Some("family-a".to_string());
        peer.fail_fast_family = Some("family-a".to_string());
        independent.fail_fast_family = Some("family-b".to_string());
        let cfg = DagConfig {
            steps: vec![fail, peer, independent],
            ..Default::default()
        };

        let res = run_dag(&cfg, 1, false, 0);
        let outcomes: HashMap<String, StepOutcome> = res
            .outcomes
            .iter()
            .map(|outcome| (outcome.tag.clone(), outcome.clone()))
            .collect();

        assert!(!res.ok);
        assert!(outcomes["independent.ok"].ok);
        assert!(!outcomes.contains_key("family.queued"));
        assert_eq!(res.not_launched, vec!["family.queued"]);
    }

    #[test]
    fn keep_going_launches_independent_step_after_failure() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "fail", "exit 1", &[], 100.0, &[]),
                step("g", "dependent", "true", &["g.fail"], 90.0, &[]),
                step("g", "independent", "true", &[], 80.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 1, true, 0);
        assert!(!res.ok); // the genuine failure still fails the run
        let outcomes: HashMap<String, StepOutcome> = res
            .outcomes
            .iter()
            .map(|o| (o.tag.clone(), o.clone()))
            .collect();
        assert!(!outcomes["g.fail"].ok);
        assert!(outcomes["g.independent"].ok); // launched AFTER the failure
        assert_eq!(res.skipped, vec!["g.dependent"]); // a true dependent is still not run
        assert!(res.not_launched.is_empty());
        assert_eq!(
            res.outcomes.len() + res.skipped.len() + res.intentional_skips.len(),
            cfg.steps.len()
        );
    }

    /// The point of the option: one run, every independent failure, not just the first.
    #[test]
    fn keep_going_collects_every_independent_failure_in_one_run() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "fail_a", "exit 1", &[], 100.0, &[]),
                step("g", "fail_b", "exit 1", &[], 90.0, &[]),
                step("g", "fail_c", "exit 1", &[], 80.0, &[]),
            ],
            ..Default::default()
        };
        let res = run_dag(&cfg, 1, true, 0);
        assert!(!res.ok);
        let mut failures: Vec<&str> = res
            .outcomes
            .iter()
            .filter(|o| !o.ok && !o.aborted)
            .map(|o| o.tag.as_str())
            .collect();
        failures.sort();
        assert_eq!(failures, vec!["g.fail_a", "g.fail_b", "g.fail_c"]);
        assert!(res.not_launched.is_empty());
    }

    #[test]
    fn spawn_failure_eager_aborts_an_inflight_sibling() {
        let slow = step("g", "slow", "sleep 10", &[], 100.0, &[]);
        let gate = step("g", "gate", "sleep 0.3", &[], 50.0, &[]);
        let mut invalid = step("g", "invalid-env", "true", &["g.gate"], 0.0, &[]);
        invalid
            .env
            .insert("BAD".to_string(), "embedded\0nul".to_string());
        let cfg = DagConfig {
            steps: vec![slow, gate, invalid],
            ..Default::default()
        };

        let started = Instant::now();
        let result = run_dag(&cfg, 2, false, 0);
        assert!(!result.ok);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "spawn failure should eager-cancel the ten-second sibling"
        );
        let outcomes: HashMap<String, StepOutcome> = result
            .outcomes
            .iter()
            .map(|outcome| (outcome.tag.clone(), outcome.clone()))
            .collect();
        assert!(outcomes["g.slow"].aborted);
        assert!(outcomes["g.gate"].ok);
        assert!(outcomes["g.invalid-env"].summary.contains("spawn failed"));
    }

    #[test]
    fn resource_cap_serializes_concurrent_steps() {
        let dir = std::env::temp_dir().join(format!("dagrun_res_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("intervals");
        let lf = log.to_string_lossy().to_string();
        let cmd = |t: &str| {
            format!(
                "echo S {t} $(date +%s.%N) >> {lf}; sleep 0.3; echo E {t} $(date +%s.%N) >> {lf}"
            )
        };
        let mut caps = BTreeMap::new();
        caps.insert("slot".to_string(), 1);
        let cfg = DagConfig {
            steps: vec![
                step("g", "one", &cmd("one"), &[], 1.0, &[("slot", 1)]),
                step("g", "two", &cmd("two"), &[], 1.0, &[("slot", 1)]),
            ],
            resource_caps: caps,
            ..Default::default()
        };
        let res = run_dag(&cfg, 4, false, 0);
        assert!(res.ok);
        let contents = std::fs::read_to_string(&log).unwrap();
        let mut starts: HashMap<String, f64> = HashMap::new();
        let mut ends: HashMap<String, f64> = HashMap::new();
        for line in contents.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 3 {
                let ts: f64 = parts[2].parse().unwrap();
                if parts[0] == "S" {
                    starts.insert(parts[1].to_string(), ts);
                } else {
                    ends.insert(parts[1].to_string(), ts);
                }
            }
        }
        // A cap of 1 must serialize the two: their run intervals cannot overlap.
        assert!(ends["one"] <= starts["two"] || ends["two"] <= starts["one"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_steps_governs_overlap_while_max_cpus_caps_each_step() {
        let make_cfg = || {
            let mut steps = Vec::new();
            for index in 0..4 {
                let mut item = step("g", &format!("step{index}"), "sleep 0.15", &[], 0.0, &[]);
                item.hint.preferred_inner_jobs = Some(1);
                item.jobs_flag = Some(String::new());
                steps.push(item);
            }
            DagConfig {
                steps,
                ..Default::default()
            }
        };

        let step_limited = run_dag_limited(&make_cfg(), 2, 4, false, 0);
        assert!(step_limited.ok);
        assert_eq!(step_limited.max_concurrent_steps, 2);

        let cpu_limited = run_dag_limited(&make_cfg(), 4, 2, false, 0);
        assert!(cpu_limited.ok);
        assert_eq!(cpu_limited.max_concurrent_steps, 4);

        let mut overcommitted_width_cfg = make_cfg();
        for step in &mut overcommitted_width_cfg.steps {
            step.hint.preferred_inner_jobs = Some(2);
        }
        let overcommitted_widths = run_dag_limited(&overcommitted_width_cfg, 4, 4, false, 0);
        assert!(overcommitted_widths.ok);
        assert_eq!(overcommitted_widths.max_concurrent_steps, 4);

        let mut default_width_cfg = make_cfg();
        for step in &mut default_width_cfg.steps {
            step.hint.preferred_inner_jobs = Some(0);
        }
        default_width_cfg.default_step_cpu_count = Some(4);
        let default_width_limited = run_dag_limited(&default_width_cfg, 2, 4, false, 0);
        assert!(default_width_limited.ok);
        assert_eq!(default_width_limited.max_concurrent_steps, 2);
    }

    #[derive(Default)]
    struct ProfileCaptureCgroups {
        cpu_counts: Mutex<Vec<Option<i64>>>,
        mem_caps: Mutex<Vec<Option<i64>>>,
        scope_build_jobs: Option<i64>,
        readonly_scope_build_jobs: bool,
    }

    impl CgroupManager for ProfileCaptureCgroups {
        fn enabled(&self) -> bool {
            true
        }

        fn prepare_command(
            &self,
            _tag: &str,
            cmd: &str,
            mem_max: Option<i64>,
            cpu_count: Option<i64>,
        ) -> String {
            self.cpu_counts.lock().unwrap().push(cpu_count);
            self.mem_caps.lock().unwrap().push(mem_max);
            if self.readonly_scope_build_jobs {
                return format!("readonly CARGO_BUILD_JOBS=8\n{cmd}");
            }
            match self.scope_build_jobs {
                Some(jobs) => format!("export CARGO_BUILD_JOBS={jobs}\n{cmd}"),
                None => cmd.to_string(),
            }
        }

        fn kill(&self, _tag: &str) -> bool {
            true
        }

        fn cleanup(&self, _tag: &str) {}

        fn oom_kills(&self, _tag: &str) -> i64 {
            0
        }

        fn peak_bytes(&self, _tag: &str) -> Option<i64> {
            None
        }

        fn cpu_stats(&self, _tag: &str) -> Option<BTreeMap<String, i64>> {
            Some(BTreeMap::from([
                ("usage_usec".to_string(), 0),
                ("user_usec".to_string(), 0),
                ("system_usec".to_string(), 0),
                ("throttled_usec".to_string(), 0),
            ]))
        }

        fn cpu_pressure(&self, _tag: &str) -> Option<BTreeMap<String, f64>> {
            None
        }

        fn thread_count(&self, _tag: &str) -> Option<i64> {
            None
        }

        fn kill_all_remaining(&self) -> i64 {
            0
        }
    }

    // ------------------------------------------------------------------ censoring provenance
    //
    // A `peak_bytes` recorded while a `memory.max` was clamping is a CENSORED observation: it
    // proves the step used everything it was allowed, not what it wanted. The row columns that
    // make that detectable — `memory_max_bytes`, the five `memory_events_*` counters, and the
    // run-relative offsets — are wired up in `run_step` above, and these tests are what hold
    // that wiring in place. Mirrors py/tests/test_censored_peak_profiles.py.

    /// A cgroup manager that reports PLANTED per-step memory measurements.
    ///
    /// `enabled()` is true so the scheduler takes exactly the boxed measurement path it takes on
    /// a real cgroup-v2 host, while the numbers come from the test rather than the kernel.
    struct PlantedCgroups {
        /// tag -> (peak_bytes, applied memory.max verbatim, memory.events counters)
        planted: BTreeMap<String, (i64, String, BTreeMap<String, i64>)>,
    }

    /// One planted step: `(tag, peak_bytes, applied memory.max verbatim, memory.events pairs)`.
    type PlantedStep<'a> = (&'a str, i64, &'a str, &'a [(&'a str, i64)]);

    impl PlantedCgroups {
        fn new(planted: &[PlantedStep<'_>]) -> Self {
            let mut map = BTreeMap::new();
            for (tag, peak, cap, events) in planted {
                let counters: BTreeMap<String, i64> =
                    events.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
                map.insert((*tag).to_string(), (*peak, (*cap).to_string(), counters));
            }
            Self { planted: map }
        }
    }

    impl CgroupManager for PlantedCgroups {
        fn enabled(&self) -> bool {
            true
        }

        fn prepare_command(
            &self,
            _tag: &str,
            cmd: &str,
            _mem_max: Option<i64>,
            _cpu_count: Option<i64>,
        ) -> String {
            cmd.to_string()
        }

        fn kill(&self, _tag: &str) -> bool {
            false
        }

        fn cleanup(&self, _tag: &str) {}

        fn oom_kills(&self, tag: &str) -> i64 {
            self.memory_events(tag)
                .and_then(|events| events.get("oom_kill").copied())
                .unwrap_or(0)
        }

        fn peak_bytes(&self, tag: &str) -> Option<i64> {
            self.planted.get(tag).map(|entry| entry.0)
        }

        fn memory_events(&self, tag: &str) -> Option<BTreeMap<String, i64>> {
            self.planted.get(tag).map(|entry| entry.2.clone())
        }

        fn applied_memory_max(&self, tag: &str) -> Option<String> {
            self.planted.get(tag).map(|entry| entry.1.clone())
        }

        fn cpu_stats(&self, _tag: &str) -> Option<BTreeMap<String, i64>> {
            None
        }

        fn cpu_pressure(&self, _tag: &str) -> Option<BTreeMap<String, f64>> {
            None
        }

        fn thread_count(&self, _tag: &str) -> Option<i64> {
            None
        }

        fn kill_all_remaining(&self) -> i64 {
            0
        }
    }

    fn profile_cell(result: &RunResult, tag: &str, column: &str) -> String {
        result
            .step_profile_rows
            .iter()
            .find(|row| row.get("step").is_some_and(|s| s == tag))
            .unwrap_or_else(|| panic!("no profile row for {tag}"))
            .get(column)
            .cloned()
            .unwrap_or_else(|| panic!("row for {tag} has no column {column}"))
    }

    const CENSORING_CAP: i64 = 8 * 1024 * 1024 * 1024;

    #[test]
    fn an_exact_cap_pass_and_an_oom_kill_stay_distinguishable_in_the_profile_row() {
        // Both steps peak exactly at their cap, so `peak_bytes` alone sees ONE population. The
        // event counters are what say that one was evicted into finishing and the other shot.
        let cgroups = Arc::new(PlantedCgroups::new(&[
            (
                "g.clamped",
                CENSORING_CAP,
                "8589934592",
                &[
                    ("low", 0),
                    ("high", 4),
                    ("max", 17),
                    ("oom", 0),
                    ("oom_kill", 0),
                ],
            ),
            (
                "g.killed",
                CENSORING_CAP,
                "8589934592",
                &[
                    ("low", 0),
                    ("high", 1),
                    ("max", 3),
                    ("oom", 2),
                    ("oom_kill", 2),
                ],
            ),
        ]));
        let cfg = DagConfig {
            steps: vec![
                step("g", "clamped", "true", &[], 0.0, &[]),
                step("g", "killed", "true", &[], 0.0, &[]),
            ],
            ..Default::default()
        };

        let result = run_dag_boxed_limited(&cfg, 2, 2, false, 0, Some(cgroups));

        assert!(result.ok);
        assert_eq!(
            profile_cell(&result, "g.clamped", "peak_bytes"),
            "8589934592"
        );
        assert_eq!(
            profile_cell(&result, "g.killed", "peak_bytes"),
            "8589934592"
        );
        assert_eq!(
            profile_cell(&result, "g.clamped", "memory_max_bytes"),
            "8589934592",
            "the cap the peak was measured under must reach the row"
        );
        assert_eq!(
            profile_cell(&result, "g.killed", "memory_max_bytes"),
            "8589934592"
        );
        // Reclaim-at-cap: held at the ceiling, never killed.
        assert_eq!(profile_cell(&result, "g.clamped", "memory_events_low"), "0");
        assert_eq!(
            profile_cell(&result, "g.clamped", "memory_events_high"),
            "4"
        );
        assert_eq!(
            profile_cell(&result, "g.clamped", "memory_events_max"),
            "17"
        );
        assert_eq!(profile_cell(&result, "g.clamped", "memory_events_oom"), "0");
        assert_eq!(
            profile_cell(&result, "g.clamped", "memory_events_oom_kill"),
            "0"
        );
        assert_eq!(profile_cell(&result, "g.clamped", "oom_kills"), "0");
        // OOM: the kernel killed it at the same ceiling.
        assert_eq!(profile_cell(&result, "g.killed", "memory_events_high"), "1");
        assert_eq!(profile_cell(&result, "g.killed", "memory_events_max"), "3");
        assert_eq!(profile_cell(&result, "g.killed", "memory_events_oom"), "2");
        assert_eq!(
            profile_cell(&result, "g.killed", "memory_events_oom_kill"),
            "2"
        );
        assert_eq!(profile_cell(&result, "g.killed", "oom_kills"), "2");
    }

    #[test]
    fn a_peak_below_its_cap_is_not_reported_as_touching_it() {
        // The uncensored case must be recognisable, or every sample looks censored.
        let cgroups = Arc::new(PlantedCgroups::new(&[(
            "g.roomy",
            2 * 1024 * 1024 * 1024,
            "8589934592",
            &[
                ("low", 0),
                ("high", 0),
                ("max", 0),
                ("oom", 0),
                ("oom_kill", 0),
            ],
        )]));
        let cfg = DagConfig {
            steps: vec![step("g", "roomy", "true", &[], 0.0, &[])],
            ..Default::default()
        };

        let result = run_dag_boxed_limited(&cfg, 1, 1, false, 0, Some(cgroups));

        assert!(result.ok);
        let peak: i64 = profile_cell(&result, "g.roomy", "peak_bytes")
            .parse()
            .unwrap();
        let cap: i64 = profile_cell(&result, "g.roomy", "memory_max_bytes")
            .parse()
            .unwrap();
        assert!(peak < cap, "peak {peak} should sit below cap {cap}");
        assert_eq!(profile_cell(&result, "g.roomy", "memory_events_max"), "0");
    }

    #[test]
    fn an_unbounded_step_says_max_and_an_unmeasured_one_says_nothing() {
        // "max" (known unbounded at this level) and blank (unknown) are DIFFERENT answers, and
        // collapsing them is what makes a store dangerous to learn from.
        let cgroups = Arc::new(PlantedCgroups::new(&[(
            "g.unbounded",
            1024,
            "max",
            &[
                ("low", 0),
                ("high", 0),
                ("max", 0),
                ("oom", 0),
                ("oom_kill", 0),
            ],
        )]));
        let cfg = DagConfig {
            steps: vec![
                step("g", "unbounded", "true", &[], 0.0, &[]),
                step("g", "unmeasured", "true", &[], 0.0, &[]),
            ],
            ..Default::default()
        };

        let result = run_dag_boxed_limited(&cfg, 2, 2, false, 0, Some(cgroups));

        assert!(result.ok);
        assert_eq!(
            profile_cell(&result, "g.unbounded", "memory_max_bytes"),
            "max"
        );
        assert_eq!(
            profile_cell(&result, "g.unbounded", "memory_events_max"),
            "0"
        );
        assert_eq!(
            profile_cell(&result, "g.unmeasured", "memory_max_bytes"),
            ""
        );
        // An unmeasured step reports no counters at all, rather than zeroes that would read as
        // "we looked and nothing happened".
        for counter in ["low", "high", "max", "oom", "oom_kill"] {
            assert_eq!(
                profile_cell(&result, "g.unmeasured", &format!("memory_events_{counter}")),
                "",
                "unmeasured memory_events_{counter} must stay blank"
            );
        }
    }

    #[test]
    fn an_unboxed_run_leaves_every_censoring_column_blank() {
        let cfg = DagConfig {
            steps: vec![step("g", "only", "true", &[], 0.0, &[])],
            ..Default::default()
        };

        let result = run_dag_limited(&cfg, 1, 1, false, 0);

        assert!(result.ok);
        assert_eq!(profile_cell(&result, "g.only", "memory_max_bytes"), "");
        for counter in ["low", "high", "max", "oom", "oom_kill"] {
            assert_eq!(
                profile_cell(&result, "g.only", &format!("memory_events_{counter}")),
                ""
            );
        }
        // The run's own timing does not need a cgroup, so the offsets are still there.
        let started: f64 = profile_cell(&result, "g.only", "started_offset_s")
            .parse()
            .unwrap();
        let finished: f64 = profile_cell(&result, "g.only", "finished_offset_s")
            .parse()
            .unwrap();
        assert!(finished >= started);
    }

    #[test]
    fn run_offsets_place_concurrent_steps_on_one_overlapping_timeline() {
        let cfg = DagConfig {
            steps: vec![
                step("g", "one", "sleep 0.5", &[], 0.0, &[]),
                step("g", "two", "sleep 0.5", &[], 0.0, &[]),
            ],
            ..Default::default()
        };

        let result = run_dag_limited(&cfg, 2, 2, false, 0);

        assert!(result.ok);
        let cell = |tag: &str, column: &str| -> f64 {
            profile_cell(&result, tag, column).parse().unwrap()
        };
        let (one_start, one_end) = (
            cell("g.one", "started_offset_s"),
            cell("g.one", "finished_offset_s"),
        );
        let (two_start, two_end) = (
            cell("g.two", "started_offset_s"),
            cell("g.two", "finished_offset_s"),
        );
        // Overlap is the interval test, computed from the row cells alone.
        assert!(one_start < two_end && two_start < one_end);
        // Both steps slept half a second, so offsets that merely happened to satisfy the
        // interval test (all zero, say) would not describe the run. Each must span its sleep.
        assert!(
            one_end - one_start >= 0.4,
            "g.one spanned {}",
            one_end - one_start
        );
        assert!(
            two_end - two_start >= 0.4,
            "g.two spanned {}",
            two_end - two_start
        );
    }

    #[test]
    fn run_offsets_place_dependent_steps_in_order_without_overlap() {
        // The same reconstruction must be able to say NO: a test that only ever sees
        // overlapping steps cannot tell a real measurement from a constant.
        let cfg = DagConfig {
            steps: vec![
                step("g", "first", "sleep 0.3", &[], 0.0, &[]),
                step("g", "second", "sleep 0.3", &["g.first"], 0.0, &[]),
            ],
            ..Default::default()
        };

        let result = run_dag_limited(&cfg, 2, 2, false, 0);

        assert!(result.ok);
        let first_end: f64 = profile_cell(&result, "g.first", "finished_offset_s")
            .parse()
            .unwrap();
        let second_start: f64 = profile_cell(&result, "g.second", "started_offset_s")
            .parse()
            .unwrap();
        assert!(
            first_end <= second_start,
            "g.first finished at {first_end} but g.second started at {second_start}"
        );
        // And the second step's offsets are measured from the RUN's origin, not its own start,
        // so they carry the ordering rather than restarting at zero.
        assert!(
            second_start >= 0.25,
            "g.second should start after g.first's 0.3s sleep, got {second_start}"
        );
    }

    #[test]
    fn undeclared_step_profiles_the_default_cpu_cap_as_inner_jobs() {
        let cfg = DagConfig {
            steps: vec![step("g", "default-width", "true", &[], 0.0, &[])],
            default_step_cpu_count: Some(1),
            ..Default::default()
        };
        let cgroups = Arc::new(ProfileCaptureCgroups::default());

        let result = run_dag_boxed_limited(&cfg, 1, 8, false, 0, Some(cgroups.clone()));

        assert!(result.ok);
        assert_eq!(
            cgroups.cpu_counts.lock().unwrap().as_slice(),
            &[Some(1)],
            "the undeclared step should be boxed at the configured default"
        );
        let row = result
            .step_profile_rows
            .iter()
            .find(|row| row.get("step").is_some_and(|tag| tag == "g.default-width"))
            .unwrap();
        assert_eq!(row.get("inner_jobs").map(String::as_str), Some("1"));
        assert_eq!(
            row.get("quota_utilization_pct").map(String::as_str),
            Some("0.00"),
            "enrichment should use the effective default width as its quota denominator"
        );
    }

    #[test]
    fn runtime_memory_caps_scale_with_preferred_and_default_widths() {
        const GIB: i64 = 1024 * 1024 * 1024;
        let mut preferred = step("g", "preferred", "true", &[], 0.0, &[]);
        preferred.hint.rss_baseline_bytes = Some(GIB);
        preferred.hint.classification = StepClass::CpuBound;
        preferred.hint.preferred_inner_jobs = Some(8);
        let mut defaulted = step("g", "defaulted", "true", &[], 0.0, &[]);
        defaulted.hint.rss_baseline_bytes = Some(GIB);
        defaulted.hint.classification = StepClass::CpuBound;
        let cfg = DagConfig {
            steps: vec![preferred, defaulted],
            mem_cap_factor: 1.0,
            default_step_cpu_count: Some(8),
            ..Default::default()
        };
        let cgroups = Arc::new(ProfileCaptureCgroups::default());

        let result = run_dag_boxed_limited(&cfg, 2, 8, false, 0, Some(cgroups.clone()));

        assert!(result.ok);
        let mut cpu_counts = cgroups.cpu_counts.lock().unwrap().clone();
        cpu_counts.sort();
        assert_eq!(cpu_counts, vec![Some(8), Some(8)]);
        let mut mem_caps = cgroups.mem_caps.lock().unwrap().clone();
        mem_caps.sort();
        assert_eq!(mem_caps, vec![Some(2 * GIB), Some(2 * GIB)]);
    }

    #[test]
    fn runtime_nonpositive_memory_hints_use_positive_default() {
        const GIB: i64 = 1024 * 1024 * 1024;
        let mut invalid = step("g", "invalid", "true", &[], 0.0, &[]);
        invalid.hint.rss_baseline_bytes = Some(0);
        invalid.hint.hard_mem_max_bytes = Some(0);
        invalid.hint.classification = StepClass::CpuBound;
        invalid.hint.preferred_inner_jobs = Some(8);
        let cfg = DagConfig {
            steps: vec![invalid],
            mem_cap_factor: 1.0,
            default_step_mem_cap_bytes: Some(GIB),
            ..Default::default()
        };
        let cgroups = Arc::new(ProfileCaptureCgroups::default());

        let result = run_dag_boxed_limited(&cfg, 1, 8, false, 0, Some(cgroups.clone()));

        assert!(result.ok);
        assert_eq!(
            cgroups.mem_caps.lock().unwrap().as_slice(),
            &[Some(2 * GIB)]
        );
    }

    #[test]
    fn write_domain_policy_refuses_before_spawning() {
        let dir = std::env::temp_dir().join(format!(
            "dagrun_write_domain_preexec_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("ran");
        let mut cfg = DagConfig {
            steps: vec![step(
                "g",
                "writer",
                &format!("touch {}", marker.display()),
                &[],
                0.0,
                &[],
            )],
            ..Default::default()
        };
        cfg.write_domain_policy.require_explicit = true;
        cfg.write_domain_policy
            .allowed_domains
            .insert("target-ci".to_string());
        let result = run_dag(&cfg, 1, false, 0);
        assert!(!result.ok);
        assert!(result.outcomes.is_empty());
        assert!(
            !marker.exists(),
            "policy refusal happened after the node wrote"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn inner_jobs_flag_is_appended_to_command() {
        // A step with preferred_inner_jobs + a "-j%d" jobs_flag has "-j4" appended; a command
        // that only succeeds when it receives exactly "-j4" therefore exits 0.
        let mut s = step(
            "g",
            "j",
            "check() { [ \"$*\" = \"-j4\" ]; }; check",
            &[],
            0.0,
            &[],
        );
        s.hint = ResourceHint {
            preferred_inner_jobs: Some(4),
            ..Default::default()
        };
        s.jobs_flag = Some("-j%d".to_string());
        let cfg = DagConfig {
            steps: vec![s],
            ..Default::default()
        };
        let res = run_dag(&cfg, 4, false, 0);
        assert!(res.ok, "expected '-j4' to be appended so the check passes");
    }

    fn jobs_env_output(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dagrun_jobs_env_{}_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test"),
            label
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn jobs_env_cfg(output: &std::path::Path, default_jobs_env: &str) -> DagConfig {
        let mut item = step(
            "g",
            "env",
            "printf '%s' \"$CARGO_BUILD_JOBS\" > \"$OBSERVED_PATH\"",
            &[],
            0.0,
            &[],
        );
        item.hint.preferred_inner_jobs = Some(4);
        item.jobs_flag = Some(String::new());
        item.env
            .insert("CARGO_BUILD_JOBS".to_string(), "99".to_string());
        item.env.insert(
            "OBSERVED_PATH".to_string(),
            output.to_string_lossy().into_owned(),
        );
        DagConfig {
            steps: vec![item],
            default_jobs_env: default_jobs_env.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn unboxed_child_observes_the_admitted_env_only_width() {
        let narrow = jobs_env_output("unboxed_narrow");
        let cfg = jobs_env_cfg(&narrow, "CARGO_BUILD_JOBS");
        assert!(run_dag_limited(&cfg, 1, 1, false, 0).ok);
        assert_eq!(std::fs::read_to_string(&narrow).unwrap(), "1");

        let normal = jobs_env_output("unboxed_normal");
        let cfg = jobs_env_cfg(&normal, "CARGO_BUILD_JOBS");
        assert!(run_dag_limited(&cfg, 1, 8, false, 0).ok);
        assert_eq!(std::fs::read_to_string(&normal).unwrap(), "4");
        let _ = std::fs::remove_file(narrow);
        let _ = std::fs::remove_file(normal);
    }

    #[test]
    fn boxed_child_keeps_the_per_step_width_after_the_scope_export() {
        let output = jobs_env_output("boxed_narrow");
        let cfg = jobs_env_cfg(&output, "CARGO_BUILD_JOBS");
        let cgroups = Arc::new(ProfileCaptureCgroups {
            scope_build_jobs: Some(8),
            ..Default::default()
        });
        assert!(run_dag_boxed_limited(&cfg, 1, 1, false, 0, Some(cgroups)).ok);
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "1");
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn boxed_scope_or_operator_width_is_unchanged_without_a_jobs_env_channel() {
        let output = jobs_env_output("boxed_operator");
        let cfg = jobs_env_cfg(&output, "");
        let cgroups = Arc::new(ProfileCaptureCgroups {
            scope_build_jobs: Some(8),
            ..Default::default()
        });
        assert!(run_dag_boxed_limited(&cfg, 1, 4, false, 0, Some(cgroups)).ok);
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "8");
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn unboxed_bash_readonly_jobs_env_refuses_before_guest_command() {
        let output = jobs_env_output("unboxed_readonly");
        let cfg = jobs_env_cfg(&output, "BASHOPTS");
        let result = run_dag_limited(&cfg, 1, 1, false, 0);
        assert!(!result.ok);
        assert_eq!(result.outcomes[0].returncode, Some(125));
        assert!(result.outcomes[0]
            .summary
            .contains("did not retain assigned width 1"));
        assert!(!output.exists(), "the guest command must not run");
    }

    #[test]
    fn boxed_readonly_jobs_env_refuses_before_guest_command() {
        let output = jobs_env_output("boxed_readonly");
        let cfg = jobs_env_cfg(&output, "CARGO_BUILD_JOBS");
        let cgroups = Arc::new(ProfileCaptureCgroups {
            readonly_scope_build_jobs: true,
            ..Default::default()
        });
        let result = run_dag_boxed_limited(&cfg, 1, 1, false, 0, Some(cgroups));
        assert!(!result.ok);
        assert_eq!(result.outcomes[0].returncode, Some(125));
        assert!(result.outcomes[0]
            .summary
            .contains("did not retain assigned width 1"));
        assert!(!output.exists(), "the guest command must not run");
    }

    #[test]
    #[should_panic(expected = "invalid jobs-env configuration")]
    fn public_cap_refuses_malformed_programmatic_jobs_env() {
        let mut cfg = jobs_env_cfg(&jobs_env_output("malformed_cap"), "bad=name");
        cfg.steps[0].hint.preferred_inner_jobs = Some(4);
        let _ = cap_config_max_cpus(&cfg, 1);
    }

    #[test]
    fn oversized_width_and_default_cpu_cap_are_clamped_to_max_cpus() {
        let mut item = step(
            "g",
            "wide",
            "check() { [ \"$*\" = \"-j2\" ]; }; check",
            &[],
            0.0,
            &[],
        );
        item.hint.preferred_inner_jobs = Some(8);
        item.jobs_flag = Some("-j%d".to_string());
        let cfg = DagConfig {
            steps: vec![item],
            default_step_cpu_count: Some(8),
            ..Default::default()
        };

        let capped = cap_config_max_cpus(&cfg, 2);
        assert_eq!(capped.steps[0].hint.preferred_inner_jobs, Some(2));
        assert_eq!(capped.default_step_cpu_count, Some(2));

        let result = run_dag_limited(&cfg, 1, 2, false, 0);
        assert!(
            result.ok,
            "expected the capped '-j2' flag to reach the command"
        );
    }

    #[test]
    fn over_budget_width_with_empty_jobs_flag_is_not_rewritten_and_refuses_before_spawn() {
        let dir = std::env::temp_dir().join(format!(
            "dagrun_unrewritable_width_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("ran");
        let mut item = step(
            "g",
            "wide",
            &format!("touch {}", marker.display()),
            &[],
            0.0,
            &[],
        );
        item.hint.preferred_inner_jobs = Some(8);
        item.jobs_flag = Some(String::new());
        let cfg = DagConfig {
            steps: vec![item],
            ..Default::default()
        };

        assert_eq!(
            validate_max_cpus_rewrite(&cfg, 2).unwrap_err(),
            "--max-cpus 2 cannot lower guest parallelism for step(s) that offer no width channel: \
             g.wide (preferred_inner_jobs=8); this machine must declare one -- set \
             $DAGRUN_JOBS_ENV to the guest's worker-count ENV VAR (e.g. \
             CARGO_BUILD_JOBS), or set the step's jobs_flag to its worker-count OPTION -- or \
             reduce preferred_inner_jobs, or raise --max-cpus"
        );
        let capped = cap_config_max_cpus(&cfg, 2);
        assert_eq!(
            capped.steps[0].hint.preferred_inner_jobs,
            Some(8),
            "the transformer must not claim it rewrote a command whose jobs_flag is empty"
        );

        let result = run_dag_limited(&cfg, 1, 2, false, 0);
        assert!(!result.ok);
        assert!(result.outcomes.is_empty());
        assert!(!marker.exists(), "refusal happened after the step spawned");

        // An intentional pre-execution skip can never spawn, so its dormant width must not reject
        // the run or erase the typed skip record.
        let mut skipped = cfg.steps[0].clone();
        skipped.job = "skipped".to_string();
        skipped.skip_reason = Some(IntentionalSkipReason::EmptyManifestBucket);
        let skipped_cfg = DagConfig {
            steps: vec![skipped],
            ..Default::default()
        };
        assert!(validate_max_cpus_rewrite(&skipped_cfg, 2).is_ok());
        let skipped_result = run_dag_limited(&skipped_cfg, 1, 2, false, 0);
        assert!(skipped_result.ok);
        assert_eq!(
            skipped_result.intentional_skips,
            vec![(
                "g.skipped".to_string(),
                IntentionalSkipReason::EmptyManifestBucket,
            )]
        );
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A termination record must say WHICH quantity crossed WHICH limit.
    ///
    /// The CPU guard compares cgroup `cpu.stat` CPU-seconds against a CPU-second budget,
    /// correctly. The RECORD of that decision used to print an unlabelled `elapsed_s` -- which
    /// was WALL -- beside a `limit_s` that was CPU-seconds. Side by side and unlabelled, the
    /// natural reading is that the two are comparable, and they are not: wall keeps rising while
    /// the step is descheduled and CPU does not, so a CPU breach could be quoted as having
    /// consumed more seconds than its own run's whole CPU rollup contained.
    #[test]
    fn a_termination_record_names_the_quantity_it_compared() {
        let dir = std::env::temp_dir().join(format!(
            "dagrun_units_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let evidence = RunEvidence::open(Some(dir.clone())).map(Arc::new);
        assert!(evidence.is_some(), "test needs a writable evidence dir");
        let sink = StepStream::new("g.step", evidence.clone());

        // A CPU breach: the compared quantity is the CPU reading, deliberately far from the wall
        // figure recorded alongside it, so a record that substituted one for the other is
        // visibly wrong rather than coincidentally right.
        capture_termination_evidence(
            &evidence,
            &sink,
            "g.step",
            std::process::id(),
            "nonce",
            TerminationBoundary {
                event: "cpu_timeout",
                unit: BudgetUnit::CpuSeconds,
                limit_s: 300,
                measured_s: 308.25,
                wall_elapsed_s: 354.587,
            },
        );
        // A wall breach was never wrong. Assert it just as hard, so "label everything
        // cpu_seconds" cannot pass.
        capture_termination_evidence(
            &evidence,
            &sink,
            "g.step",
            std::process::id(),
            "nonce",
            TerminationBoundary {
                event: "step_timeout",
                unit: BudgetUnit::WallSeconds,
                limit_s: 900,
                measured_s: 900.13,
                wall_elapsed_s: 900.13,
            },
        );
        drop(evidence);

        let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let cpu = journal
            .lines()
            .find(|l| l.contains(r#""event":"cpu_timeout""#))
            .expect("no cpu_timeout record");
        let wall = journal
            .lines()
            .find(|l| l.contains(r#""event":"step_timeout""#))
            .expect("no step_timeout record");

        for (record, expected) in [
            (
                cpu,
                [
                    r#""measured_s":"308.250""#,
                    r#""limit_s":"300""#,
                    r#""unit":"cpu_seconds""#,
                    r#""wall_elapsed_s":"354.587""#,
                ],
            ),
            (
                wall,
                [
                    r#""measured_s":"900.130""#,
                    r#""limit_s":"900""#,
                    r#""unit":"wall_seconds""#,
                    r#""wall_elapsed_s":"900.130""#,
                ],
            ),
        ] {
            for field in expected {
                assert!(
                    record.contains(field),
                    "termination record is missing {field}:\n{record}"
                );
            }
            // The ambiguous field is GONE. Retaining it would preserve the exact misreading this
            // fixes: an unlabelled seconds figure sitting next to a limit in a different unit.
            assert!(
                !record.contains(r#""elapsed_s""#),
                "the unlabelled elapsed_s must not come back:\n{record}"
            );
        }
    }

    /// The terminal step record must carry what the step actually consumed.
    ///
    /// `step_end` exists so the journal alone can answer "what was this run doing" without the
    /// end-of-run profile rows, which a hard kill destroys. A counter the kernel does not publish
    /// stays ABSENT rather than becoming a measured zero.
    #[test]
    fn cpu_journal_fields_name_their_units_and_never_invent_a_zero() {
        let full = BTreeMap::from([
            ("usage_usec".to_string(), 259_926_893i64),
            ("nr_throttled".to_string(), 994),
            ("throttled_usec".to_string(), 431_942_000),
            ("user_usec".to_string(), 1),
        ]);
        assert_eq!(
            cpu_journal_fields(Some(&full)),
            vec![
                ("cpu_usage_usec", "259926893".to_string()),
                ("cpu_nr_throttled", "994".to_string()),
                ("cpu_throttled_usec", "431942000".to_string()),
            ]
        );

        // A kernel that publishes only some counters contributes only those. Inventing the rest
        // as 0 would put a measurement in the record that was never measured.
        let partial = BTreeMap::from([("usage_usec".to_string(), 7i64)]);
        assert_eq!(
            cpu_journal_fields(Some(&partial)),
            vec![("cpu_usage_usec", "7".to_string())]
        );

        // Unboxed there is nothing to read at all.
        assert!(cpu_journal_fields(None).is_empty());
    }

    /// An ABSENT CPU counter is not a measured ZERO.
    ///
    /// The guard read `cpu.stat`'s `usage_usec` with a default of 0, so a cgroup that does not
    /// publish that counter reported "this step has burned no CPU" forever. That made `>= budget`
    /// permanently unsatisfiable: a declared `cpu_timeout` enforced nothing, quietly. The
    /// measured cases are asserted just as hard, so "always return None" cannot pass.
    #[test]
    fn an_absent_cpu_counter_is_unmeasurable_not_zero() {
        let measured = BTreeMap::from([("usage_usec".to_string(), 2_500_000i64)]);
        assert_eq!(cpu_seconds_from_stats(&measured), Some(2.5));

        // A genuine measured zero is still a measurement, and must survive as one.
        let idle = BTreeMap::from([("usage_usec".to_string(), 0i64)]);
        assert_eq!(cpu_seconds_from_stats(&idle), Some(0.0));

        // Absent means CANNOT MEASURE, and the caller must be forced to say so.
        let without = BTreeMap::from([("nr_periods".to_string(), 3i64)]);
        assert_eq!(cpu_seconds_from_stats(&without), None);
    }

    /// An UNDECLARED resource cap is not a cap of ZERO.
    ///
    /// The readiness gate reads `resource_avail.get(name).unwrap_or(0)`, which collapses two
    /// conditions whose remedies are opposites: "you forgot to declare capacity" and "this is
    /// deliberately blocked". Both produced byte-identical behaviour -- an infinite 50 ms sleep
    /// at 0% CPU with nothing printed -- so the one thing a reader needed was the one thing not
    /// reported.
    ///
    /// Bracketed BOTH ways. A one-sided test would pass if the predicate simply flagged every
    /// resource demand, so the declared-zero case is asserted just as hard as the undeclared one.
    #[test]
    fn an_undeclared_resource_demand_is_named_and_a_declared_zero_is_not() {
        let demanding = step("g", "needs", "true", &[], 0.0, &[("browser", 1)]);

        let absent = DagConfig {
            steps: vec![demanding.clone()],
            ..Default::default()
        };
        assert_eq!(
            undeclared_resource_demands(&absent),
            vec!["g.needs: browser".to_string()]
        );

        // A cap DECLARED as 0 is a real value: deliberately blocking, and still gating normally.
        let blocked = DagConfig {
            steps: vec![demanding.clone()],
            resource_caps: BTreeMap::from([("browser".to_string(), 0i64)]),
            ..Default::default()
        };
        assert!(undeclared_resource_demands(&blocked).is_empty());

        // An ordinary cap is likewise not flagged.
        let ample = DagConfig {
            steps: vec![demanding],
            resource_caps: BTreeMap::from([("browser".to_string(), 4i64)]),
            ..Default::default()
        };
        assert!(undeclared_resource_demands(&ample).is_empty());

        // A demand of 0 is satisfied by the absent-cap default of 0, so it cannot starve.
        let zero_demand = DagConfig {
            steps: vec![step("g", "idle", "true", &[], 0.0, &[("browser", 0)])],
            ..Default::default()
        };
        assert!(undeclared_resource_demands(&zero_demand).is_empty());

        // An intentionally-skipped step never launches, so its dormant demand cannot hang a run
        // and must not fail one either.
        let mut skipped = step("g", "skipped", "true", &[], 0.0, &[("browser", 1)]);
        skipped.skip_reason = Some(IntentionalSkipReason::EmptyManifestBucket);
        let skipped_cfg = DagConfig {
            steps: vec![skipped],
            ..Default::default()
        };
        assert!(undeclared_resource_demands(&skipped_cfg).is_empty());
    }

    /// #79 derived-enforcement-manifest: the `wall_timeout` flag is load-bearing. The manifest
    /// promising a per-step wall ceiling and the supervisor actually applying one are now the
    /// same decision, so flipping the registry entry must move BOTH -- assert the manifest text
    /// and the observed outcome of a step that outlives its own timeout.
    #[test]
    fn per_step_wall_ceiling_follows_the_wall_timeout_capability_flag() {
        let mut slow = step("g", "slow", "sleep 2", &[], 0.0, &[]);
        slow.timeout = 1;
        let cfg = DagConfig {
            steps: vec![slow],
            ..Default::default()
        };

        assert!(crate::capabilities::enforcement_manifest().contains("\"wall_timeout\":true"));
        let enforced = run_dag_boxed_deadline(&cfg, 1, false, 0, None, None, Some(1), Some(20));
        assert!(
            !enforced.ok,
            "a 2s step under a 1s wall ceiling must be cut, but the run succeeded"
        );

        let unenforced = crate::capabilities::with_registry_override("wall_timeout", false, || {
            // ABSENCE of the `true`: the manifest has two lanes, and `wall_timeout` is true on
            // BOTH of them unbracketed, so this is the assertion that can actually fail.
            assert!(!crate::capabilities::enforcement_manifest().contains("\"wall_timeout\":true"));
            run_dag_boxed_deadline(&cfg, 1, false, 0, None, None, Some(1), Some(20))
        });
        assert!(
            unenforced.ok,
            "with wall_timeout declared unenforced the step must be allowed to finish; instead it \
             was still cut, so the manifest and the guard can disagree"
        );
    }

    /// The refusal is what turns a hang into a bug report, so assert the RUN, not just the
    /// predicate. The outer run budget is here so this test cannot hang when the refusal is
    /// removed: without the refusal the ready-set loop sleeps until that budget expires and the
    /// failure is blamed on the run instead of on the node, which is exactly the worse report.
    #[test]
    fn a_run_with_an_undeclared_demand_is_refused_rather_than_waited_out() {
        let mut demanding = step("g", "needs", "true", &[], 0.0, &[("browser", 1)]);
        demanding.timeout = 1;
        let cfg = DagConfig {
            steps: vec![demanding],
            ..Default::default()
        };

        let started = Instant::now();
        let res = run_dag_boxed_deadline(&cfg, 1, false, 0, None, None, Some(1), Some(3));
        let elapsed = started.elapsed().as_secs_f64();

        assert!(!res.ok);
        assert!(
            !res.run_timed_out,
            "the demand can never be satisfied, so this must be refused up front and named -- not \
             waited out until the run budget expires and blamed on the run"
        );
        assert!(
            elapsed < 3.0,
            "the run should refuse before any node starts, not wait on the gate ({elapsed}s)"
        );
        assert!(res.outcomes.is_empty());

        // The refusal must not become a blanket ban on resource demands.
        let declared = DagConfig {
            steps: vec![step("g", "needs", "true", &[], 0.0, &[("browser", 1)])],
            resource_caps: BTreeMap::from([("browser".to_string(), 1i64)]),
            ..Default::default()
        };
        assert!(run_dag(&declared, 1, false, 0).ok);
    }

    // ---- #80 runner-supervisor-crash-loud -------------------------------------------------
    //
    // A wedged run is the worst outcome this tool has, because it looks like work in progress.
    // Each test below pins one of the two layers, or one of the accounting properties that make
    // a mid-flight panic survivable, and each asserts the CAUSE IS NAMED rather than merely that
    // something failed.

    fn crash_test_runner(steps: Vec<Step>, caps: &[(&str, i64)]) -> Runner {
        let cfg = DagConfig {
            steps,
            resource_caps: caps
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect::<BTreeMap<String, i64>>(),
            ..Default::default()
        };
        Runner::new(&cfg, 1, 1, false, 0, None, None, None, None)
    }

    #[test]
    fn a_panicking_supervisor_becomes_a_failed_step_that_names_the_panic() {
        let victim = step("g", "boom", "true", &[], 0.0, &[("slot", 1)]);
        let runner = crash_test_runner(vec![victim.clone()], &[("slot", 1)]);
        {
            // Admit the step exactly as the ready-set loop would, so the accounting the guard has
            // to unwind is real rather than assumed.
            let mut sh = lock_shared(&runner.shared);
            sh.running.insert(victim.tag());
            acquire(&mut sh, &victim);
            assert_eq!(sh.resource_avail.get("slot"), Some(&0));
        }
        let recovery = SupervisorRecovery {
            step: victim.clone(),
            shared: Arc::clone(&runner.shared),
            cgroups: None,
            evidence: None,
            keep_going: false,
        };
        // The default hook would print this planted panic and make the test log look like a
        // failure; silence it for the duration and restore it afterwards.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        with_supervisor_guard(recovery, || panic!("planted supervisor defect"));
        std::panic::set_hook(previous);

        let sh = lock_shared(&runner.shared);
        let outcome = sh
            .done
            .get(&victim.tag())
            .expect("a panicking supervisor must still publish a TERMINAL outcome");
        assert!(!outcome.ok);
        assert!(
            !outcome.aborted,
            "a supervisor crash is a FAILURE, not a peer-triggered abort"
        );
        assert!(
            outcome.reason.contains("SUPERVISOR CRASHED"),
            "reason was {:?}",
            outcome.reason
        );
        assert!(
            outcome.reason.contains("planted supervisor defect"),
            "the panic must be NAMED, not merely counted; reason was {:?}",
            outcome.reason
        );
        assert!(sh.failed, "the run must be marked failed");
        assert!(!sh.running.contains(&victim.tag()));
        assert_eq!(
            sh.resource_avail.get("slot"),
            Some(&1),
            "the crash path must give the admitted slot back"
        );
    }

    #[test]
    fn the_sweep_reaps_a_supervisor_that_ended_without_publishing() {
        let lost = step("g", "lost", "true", &[], 0.0, &[]);
        let runner = crash_test_runner(vec![lost.clone()], &[]);
        {
            let mut sh = lock_shared(&runner.shared);
            sh.running.insert(lost.tag());
        }
        // A thread that has already ended, with nothing in `done`: layer one is not in the
        // picture at all here, so only the sweep can end this run.
        let handle = thread::spawn(|| {});
        while !handle.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        runner.sweep_dead_supervisors(&[(handle, lost.clone())]);

        let sh = lock_shared(&runner.shared);
        let outcome = sh.done.get(&lost.tag()).expect("the sweep must publish");
        assert!(
            outcome.reason.contains("SUPERVISOR VANISHED"),
            "reason was {:?}",
            outcome.reason
        );
        assert!(outcome.reason.contains("UNKNOWN"));
        assert!(sh.failed);
    }

    #[test]
    fn the_sweep_sees_a_crash_that_lands_between_retiring_and_publishing() {
        // `retire` drops the tag from `running` and only then is `done` written. A supervisor that
        // dies in that window is in NEITHER set, so a sweep keyed on "still in `running`" is blind
        // to exactly it. This is the case that forced the (finished AND no outcome) key.
        let ghost = step("g", "ghost", "true", &[], 0.0, &[("slot", 1)]);
        let runner = crash_test_runner(vec![ghost.clone()], &[("slot", 1)]);
        {
            let mut sh = lock_shared(&runner.shared);
            sh.running.insert(ghost.tag());
            acquire(&mut sh, &ghost);
            // ... and now the supervisor retires, then dies before inserting into `done`.
            retire(&mut sh, &ghost);
            assert!(!sh.running.contains(&ghost.tag()));
            assert!(!sh.done.contains_key(&ghost.tag()));
        }
        let handle = thread::spawn(|| {});
        while !handle.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        runner.sweep_dead_supervisors(&[(handle, ghost.clone())]);

        let sh = lock_shared(&runner.shared);
        assert!(sh
            .done
            .get(&ghost.tag())
            .expect("the sweep must see a tag that is in neither `running` nor `done`")
            .reason
            .contains("SUPERVISOR VANISHED"));
    }

    #[test]
    fn the_sweep_never_contradicts_a_supervisor_that_did_publish() {
        let fine = step("g", "fine", "true", &[], 0.0, &[]);
        let runner = crash_test_runner(vec![fine.clone()], &[]);
        {
            let mut sh = lock_shared(&runner.shared);
            sh.done.insert(
                fine.tag(),
                StepOutcome::passed(fine.tag(), 0.1, String::new(), Some(0), None, None),
            );
        }
        let handle = thread::spawn(|| {});
        while !handle.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        runner.sweep_dead_supervisors(&[(handle, fine.clone())]);

        let sh = lock_shared(&runner.shared);
        assert!(
            sh.done[&fine.tag()].ok,
            "a finished thread that DID publish must be left alone"
        );
        assert!(!sh.failed);
    }

    #[test]
    fn retiring_twice_gives_the_resource_back_only_once() {
        // A panic landing after a normal release would otherwise release a SECOND time, drifting
        // `resource_avail` ABOVE its declared cap -- a cap that silently stopped being a cap.
        let one = step("g", "one", "true", &[], 0.0, &[("slot", 1)]);
        let runner = crash_test_runner(vec![one.clone()], &[("slot", 1)]);
        let mut sh = lock_shared(&runner.shared);
        sh.running.insert(one.tag());
        acquire(&mut sh, &one);
        // TWO live children, only ONE of them this step's. A second uncount would silently steal
        // the OTHER step's child from the count -- and `saturating_sub` would hide that if the
        // count were allowed to reach zero, so the peer is what makes the defect observable.
        sh.active_processes = 2;
        sh.counted_processes.insert(one.tag());
        sh.counted_processes.insert("g.peer".to_string());

        // The child-exit path uncounts as soon as `try_wait` returns, LONG before the step
        // retires. `retire` then uncounts again, which is why `uncount_process` needs its own
        // once-only guard rather than borrowing `retire`'s.
        uncount_process(&mut sh, &one.tag());
        assert_eq!(sh.active_processes, 1);

        assert!(retire(&mut sh, &one));
        assert_eq!(sh.resource_avail.get("slot"), Some(&1));
        assert_eq!(
            sh.active_processes, 1,
            "retiring after the child-exit uncount must not decrement a second time"
        );

        assert!(
            !retire(&mut sh, &one),
            "a second retire must report that it did nothing"
        );
        assert_eq!(
            sh.resource_avail.get("slot"),
            Some(&1),
            "the declared cap of 1 must not read as 2"
        );
        assert_eq!(
            sh.active_processes, 1,
            "retiring twice must not uncount a child that belongs to another step"
        );
    }

    #[test]
    fn publishing_a_crash_does_not_overwrite_an_outcome_the_step_already_recorded() {
        // A panic in the REPORTING TAIL is a runner bug, not evidence that the step failed.
        let late = step("g", "late", "true", &[], 0.0, &[]);
        let runner = crash_test_runner(vec![late.clone()], &[]);
        {
            let mut sh = lock_shared(&runner.shared);
            sh.done.insert(
                late.tag(),
                StepOutcome::passed(late.tag(), 0.1, String::new(), Some(0), None, None),
            );
        }
        let published = publish_supervisor_failure(
            &runner.shared,
            &None,
            &None,
            &late,
            false,
            SupervisorFailure {
                reason: "SUPERVISOR CRASHED (planted)".to_string(),
                summary: "planted".to_string(),
                elapsed_s: 0.0,
            },
        );
        assert!(!published);
        let sh = lock_shared(&runner.shared);
        assert!(
            sh.done[&late.tag()].ok,
            "the recorded success must stand; a crash while printing it is not a step failure"
        );
        assert!(!sh.failed);
    }

    /// Wall budget for one whole in-test run, mirroring the sibling engine's crash suite.
    ///
    /// Every step driven through `Runner::run` below is `true` or a short sleep, so a healthy run
    /// finishes in well under a second. The deadline is what turns the defect these tests pin --
    /// a WEDGE -- into a named assertion failure instead of a suite that never returns.
    const CRASH_DEADLINE: Duration = Duration::from_secs(20);

    /// Drive `runner.run()` on a side thread so a wedge FAILS this test rather than hanging it.
    fn run_with_deadline(runner: Arc<Runner>) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let (ok, _wall) = runner.run();
            let _ = tx.send(ok);
        });
        rx.recv_timeout(CRASH_DEADLINE).unwrap_or_else(|_| {
            panic!(
                "Runner::run did not return within {CRASH_DEADLINE:?}: the scheduler is WEDGED \
                 waiting for a supervisor that will never publish an outcome"
            )
        })
    }

    /// Every line [`warn`] has recorded that mentions `needle`.
    ///
    /// Reading the recorder rather than the descriptor: the test harness intercepts the print
    /// macros before they reach file descriptor 2, so a redirect of the descriptor sees nothing
    /// and the assertion would pass on an empty string for any reason at all.
    fn warnings_mentioning(needle: &str) -> Vec<String> {
        WARNINGS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|line| line.contains(needle))
            .cloned()
            .collect()
    }

    /// A manager whose `thread_count` panics, which kills the per-step monitor thread.
    #[derive(Default)]
    struct CgroupsWithABrokenThreadCount;

    impl CgroupManager for CgroupsWithABrokenThreadCount {
        fn enabled(&self) -> bool {
            true
        }

        fn prepare_command(
            &self,
            _tag: &str,
            cmd: &str,
            _mem_max: Option<i64>,
            _cpu_count: Option<i64>,
        ) -> String {
            cmd.to_string()
        }

        fn kill(&self, _tag: &str) -> bool {
            true
        }

        fn cleanup(&self, _tag: &str) {}

        fn oom_kills(&self, _tag: &str) -> i64 {
            0
        }

        fn peak_bytes(&self, _tag: &str) -> Option<i64> {
            None
        }

        fn cpu_stats(&self, _tag: &str) -> Option<BTreeMap<String, i64>> {
            None
        }

        fn cpu_pressure(&self, _tag: &str) -> Option<BTreeMap<String, f64>> {
            None
        }

        fn thread_count(&self, _tag: &str) -> Option<i64> {
            panic!("planted defect in the monitor's first cgroup read");
        }

        fn kill_all_remaining(&self) -> i64 {
            0
        }
    }

    #[test]
    fn a_dead_cpu_budget_monitor_says_the_budget_is_no_longer_enforced() {
        // The monitor is the ONLY enforcer of the per-step CPU-time budget, and nothing ever joins
        // it for a result. If it dies the budget is not merely unmeasured -- it stops being
        // enforced at all, silently, while still reading as configured. The step's own result is
        // unaffected; the point is entirely the warning.
        let slow = step("monitor-death", "slow", "sleep 1.5", &[], 0.0, &[]);
        let cfg = DagConfig {
            steps: vec![slow],
            ..Default::default()
        };
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = run_dag_boxed_limited(
            &cfg,
            1,
            1,
            false,
            0,
            Some(Arc::new(CgroupsWithABrokenThreadCount)),
        );
        std::panic::set_hook(previous);

        assert!(
            result.ok,
            "the step's own result is unaffected by the monitor dying"
        );
        // Keyed on this step's own tag, so a peer test running in parallel cannot supply the
        // evidence for it.
        let said = warnings_mentioning("monitor-death.slow");
        let died: Vec<&String> = said
            .iter()
            .filter(|line| line.contains("monitor thread DIED"))
            .collect();
        assert_eq!(
            died.len(),
            1,
            "a dead monitor must be audible exactly once; warnings were {said:?}"
        );
        assert!(
            died[0].contains("NO LONGER") && died[0].contains("ENFORCED"),
            "the warning must say the budget stopped being enforced: {:?}",
            died[0]
        );
        assert!(
            died[0].contains("planted defect in the monitor's first cgroup read"),
            "the cause must be NAMED, not merely counted: {:?}",
            died[0]
        );
    }

    #[test]
    fn a_supervisor_that_ends_without_publishing_cannot_wedge_a_whole_run() {
        // LAYER TWO, through `Runner::run` rather than by calling the sweep directly. Calling
        // `sweep_dead_supervisors` on hand-built state proves the sweep works; it says nothing
        // about the ready-set loop CALLING it, and deleting that call is what re-creates the
        // wedge this whole issue is about. Only a real run can tell the two apart.
        let lost = step("g", "lost", "true", &[], 0.0, &[]);
        let mut runner = crash_test_runner(vec![lost.clone()], &[]);
        // The supervisor thread ends immediately with nothing in `done` and no panic to catch --
        // layer one is not in the picture, so only the sweep can end this run.
        runner.supervisor_override = Some(Arc::new(|_step: &Step| {}));
        let runner = Arc::new(runner);

        assert!(
            !run_with_deadline(Arc::clone(&runner)),
            "a vanished supervisor must FAIL the run, not pass it"
        );

        let sh = lock_shared(&runner.shared);
        let outcome = sh
            .done
            .get(&lost.tag())
            .expect("the run must not finish while a launched step has no outcome");
        assert!(
            outcome.reason.contains("SUPERVISOR VANISHED"),
            "reason was {:?}",
            outcome.reason
        );
        assert!(outcome.reason.contains("UNKNOWN"));
    }

    #[test]
    fn the_crash_record_in_the_journal_names_the_cause() {
        // A run reconstructed from evidence alone is exactly the case where nobody still has the
        // console output, so an event carrying only a tag and a duration says that something went
        // wrong without saying what. The sibling engine writes step/reason/elapsed_s under this
        // same event name, and `make cross` compares journals.
        let dir = std::env::temp_dir().join(format!(
            "dagrun-crash-journal-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let evidence = Arc::new(RunEvidence::open(Some(dir.clone())).expect("an evidence dir"));
        let victim = step("g", "boom", "true", &[], 0.0, &[]);
        let runner = crash_test_runner(vec![victim.clone()], &[]);
        publish_supervisor_failure(
            &runner.shared,
            &None,
            &Some(evidence),
            &victim,
            false,
            SupervisorFailure {
                reason: "SUPERVISOR CRASHED (planted supervisor defect)".to_string(),
                summary: "planted supervisor defect".to_string(),
                elapsed_s: 0.25,
            },
        );

        let journal = std::fs::read_to_string(dir.join("journal.jsonl")).expect("a journal");
        let record = journal
            .lines()
            .find(|line| line.contains("\"event\":\"supervisor_crash\""))
            .unwrap_or_else(|| panic!("no supervisor_crash record in {journal:?}"))
            .to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            record.contains("\"reason\":\"SUPERVISOR CRASHED (planted supervisor defect)\""),
            "the record must NAME the cause; it was {record:?}"
        );
        assert!(record.contains("\"step\":\"g.boom\""));
        assert!(record.contains("\"elapsed_s\":\"0.250\""));
    }

    #[test]
    fn a_poisoned_lock_is_recovered_instead_of_cascading_into_every_other_thread() {
        // A supervisor that panics while holding the lock poisons it. With `lock().unwrap()`
        // everywhere, every OTHER thread then panics too, burying the original cause under a
        // cascade and taking the run down with no attributable reason.
        let victim = step("g", "poison", "true", &[], 0.0, &[]);
        let runner = crash_test_runner(vec![victim.clone()], &[]);
        let shared = Arc::clone(&runner.shared);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let poisoner = thread::spawn(move || {
            let mut sh = lock_shared(&shared);
            sh.failed = true;
            panic!("planted panic while holding the scheduler lock");
        });
        assert!(poisoner.join().is_err());
        std::panic::set_hook(previous);

        assert!(
            runner.shared.lock().is_err(),
            "the planted panic must really have poisoned the lock, or this proves nothing"
        );
        let sh = lock_shared(&runner.shared);
        assert!(
            sh.failed,
            "lock_shared must hand back the state as it stands rather than panicking again"
        );
    }

    // ---- #81 runner-capture-memory-bound -------------------------------------------------
    //
    // The disk ceiling stops a runaway step filling the device; it does nothing about the copy
    // the runner holds in its OWN address space, and that is the copy that kills the runner
    // first, taking the run's verdict and evidence with it.

    #[test]
    fn the_ring_never_allocates_more_than_its_ceiling_however_much_is_fed() {
        // The allocation, not merely the retained length, is the property: the previous `Vec<u8>`
        // capture grew with the step's output, so 16 MiB of output cost 16 MiB of live memory.
        let limit = 64 * 1024;
        let mut cap = BoundedCapture::new(Some(limit));
        let chunk = vec![b'x'; 8192];
        for _ in 0..2048 {
            // 16 MiB, 256 times the ceiling.
            cap.feed(&chunk);
        }
        assert_eq!(cap.total, 2048 * 8192);
        assert_eq!(cap.kept(), limit);
        assert_eq!(
            cap.buf.len(),
            limit,
            "the ring must be exactly its ceiling, however much was fed through it"
        );
        assert_eq!(
            cap.buf.capacity(),
            limit,
            "and must never have reallocated above it"
        );
    }

    #[test]
    fn the_ring_keeps_the_tail_and_reports_how_much_it_dropped() {
        let mut cap = BoundedCapture::new(Some(10));
        cap.feed(b"0123456789abcdef");
        assert_eq!(cap.tail(), b"6789abcdef", "the TAIL is what a dump needs");
        assert_eq!(cap.kept(), 10);
        assert_eq!(cap.total, 16);
        assert!(cap.dropped());
    }

    #[test]
    fn the_ring_is_byte_exact_across_a_wrap() {
        // Wrap-around is where a ring silently corrupts; pin the exact bytes, not just the length.
        let mut cap = BoundedCapture::new(Some(8));
        for chunk in [b"abc".as_slice(), b"de".as_slice(), b"fghij".as_slice()] {
            cap.feed(chunk);
        }
        assert_eq!(cap.tail(), b"cdefghij");
        assert_eq!(cap.total, 10);

        // A single read LARGER than the whole ring must also leave exactly its own tail.
        let mut big = BoundedCapture::new(Some(4));
        big.feed(b"0123456789");
        assert_eq!(big.tail(), b"6789");
        assert_eq!(big.total, 10);
    }

    #[test]
    fn an_unwrapped_ring_is_exactly_what_was_fed() {
        // Below the ceiling nothing is a tail: the capture must be lossless and say so.
        let mut cap = BoundedCapture::new(Some(1024));
        cap.feed(b"first\n");
        cap.feed(b"second\n");
        assert_eq!(cap.tail(), b"first\nsecond\n");
        assert!(!cap.dropped());
        assert_eq!(cap.kept(), 13);
        assert_eq!(last_line(&cap.tail()), "second");
    }

    #[test]
    fn the_last_line_survives_a_step_that_overran_the_ceiling() {
        // Keeping the TAIL is what makes the one-line summary work on a runaway step.
        let mut cap = BoundedCapture::new(Some(32));
        for _ in 0..4 {
            cap.feed(b"early noise that will be dropped\n");
        }
        cap.feed(b"the real verdict\n");
        assert_eq!(last_line(&cap.tail()), "the real verdict");
    }

    #[test]
    fn an_unlimited_capture_is_an_explicit_opt_out_not_a_fallback() {
        let mut cap = BoundedCapture::new(None);
        cap.feed(&vec![b'a'; 5000]);
        assert_eq!(cap.kept(), 5000);
        assert!(!cap.dropped());
    }

    #[test]
    fn the_console_line_bound_is_pinned_to_one_mib_literally() {
        // The `MUST match` note beside this constant is prose until something can fail. The
        // sibling engine pins the same literal, so a change made in one edition and not the other
        // fails a test by name instead of drifting; and the end-to-end test of this code path
        // overrides the value to stay fast, so it says nothing about the number that ships.
        assert_eq!(STREAM_LINE_MAX_BYTES, 1_048_576);
    }

    #[test]
    fn the_split_notice_is_identical_to_the_sibling_engine() {
        // Byte-for-byte with `stream_split_notice` in py/dagrun/scheduler.py. The two editions'
        // console output is compared by the differential harness, so drift here is a real
        // failure. The bound is passed in rather than read from the constant so this test can
        // name a number the constant does not have -- which is also what stops the notice
        // reporting a threshold other than the one actually in force.
        assert_eq!(
            stream_split_notice(1234),
            "^ RUNNER SPLIT the line above: the step emitted 1234 bytes with no newline, \
             so the live stream broke the console line to avoid buffering it without limit. \
             NOTHING WAS DISCARDED -- the durable log and the captured output are complete; \
             only the console display was broken."
        );
    }

    #[test]
    fn the_split_notice_says_it_was_forced_and_that_nothing_was_lost() {
        // The guard has to be legible at the one moment it acts. A forced flush that reads as
        // an ordinary console line means the cap firing and the cap never firing produce the
        // same output, so nobody can tell whether it has ever worked -- and it cannot fire on
        // healthy output, so this single line is the only evidence a reader will ever get.
        let notice = stream_split_notice(STREAM_LINE_MAX_BYTES);
        assert!(
            notice.contains("RUNNER SPLIT"),
            "the notice must attribute the split to the runner, not leave it looking like the \
             step's own formatting: {notice}"
        );
        assert!(
            notice.contains("NOTHING WAS DISCARDED"),
            "\"SPLIT\" alone reads as data loss; the notice must say the output is complete: \
             {notice}"
        );
        assert!(
            notice.contains(&STREAM_LINE_MAX_BYTES.to_string()),
            "the notice must name the threshold it enforced: {notice}"
        );
    }

    #[test]
    fn the_split_notice_names_the_bound_in_force_not_the_constant() {
        // THE DEFECT THIS PINS, found by the sibling engine's test before either shipped: the
        // notice was built from a value bound once at definition time, so it reported the
        // constant's own number even when a different bound was the one actually enforcing.
        // A message that misreports the threshold it exists to explain is worse than silence,
        // because it is confidently wrong.
        let notice = stream_split_notice(3072);
        assert!(
            notice.contains("3072"),
            "the notice must name the bound it was given: {notice}"
        );
        assert!(
            !notice.contains("1048576"),
            "the notice must NOT fall back to the shipped constant when a different bound is \
             in force: {notice}"
        );
    }

    #[test]
    fn the_capture_truncation_notice_is_identical_to_the_python_engines() {
        // Byte-for-byte with `CAPTURE_TRUNCATION_NOTICE` in py/dagrun/attribution.py. The two
        // editions' output is compared by the differential harness, so a drift here is a real
        // failure and not a cosmetic one.
        assert_eq!(
            capture_truncation_notice(1234, 300),
            "[dagrun] EARLIER OUTPUT DROPPED: this step produced 1234 bytes but only \
             the last 300 were kept in memory (raise or lift the ceiling with \
             DAGRUN_CAPTURE_MAX_BYTES; 0 = unlimited). The durable per-step log is \
             unaffected and still has the rest."
        );
    }
}
