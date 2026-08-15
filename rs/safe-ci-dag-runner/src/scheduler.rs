//! Dependency-aware, resource-aware concurrent DAG execution.

// The DAG runner: greedy, memory-/resource-aware step scheduling.
//
// Port of the OBSERVABLE scheduling behavior of `py/safe_ci_dag_runner/scheduler.py` for the
// no-boxing default path (Python's `cgroups=None`). Reproduced from the reference:
//
// * Greedy ready-set loop: each pass launches every ready step (deps satisfied, resources
//   free, under the `-j` fan-out, in longest-processing-time order) on its own supervisor
//   thread, then sleeps briefly.
// * Dependency gating + dep-FAILURE skip-closure (a failed dep transitively skips dependents).
// * Named-resource capacity buckets (`hint.resources` vs `cfg.resource_caps`).
// * Longest-processing-time (LPT) dispatch order (descending `est_duration_s`, stable).
// * Per-step supervision via `bash -c` in its own process group (whole-tree teardown).
// * Fail-fast (eager-exit): the first genuine failure stops launching NEW steps; by default it
//   also eager-cancels in-flight steps (labelled ABORTED, not FAILED). `keep_going` only
//   suppresses that eager-cancel so already-running steps finish — it does NOT keep launching
//   still-runnable steps.
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
use std::process::ExitStatus;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::ambient::{capture_ambient_snapshot, PsiReading};
use crate::attribution::{
    bind_process_tests, default_log_dir, mint_step_nonce, process_snapshot, recognize, Culprit,
    RunEvidence, StepStream, TestEvent, STEP_NONCE_ENV,
};
use crate::cgroup::CgroupManager;
use crate::model::{
    canonical_cpu_timeout, command_with_inner_jobs, effective_cpu_count, preferred_inner_jobs,
    scale_cpu_timeout, step_classification, write_domain_violations, DagConfig, RunResult, Step,
    StepOutcome,
};
use crate::profile_enrich::{resolve_effective_inner_jobs, step_enrichment_columns};

/// A per-step measurement row (column -> value), matching the perflog step-profile schema.
type ProfileRow = BTreeMap<String, String>;

/// Optional per-step cgroup manager shared (behind an `Arc`) across the run's supervisor threads.
pub type BoxedCgroups = Option<Arc<dyn CgroupManager>>;

/// Monotonic start epoch of the enclosing DAG step, serialized for nested consumers.
///
/// A nested timeout cannot safely start a fresh clock after its own setup: doing so makes a
/// numerically smaller timeout capable of outliving the enclosing step.  The runner owns the
/// actual step clock, so it exports that clock's epoch before spawning the child.  Consumers add
/// their own (smaller) allowance to this value and therefore keep one ordering across execs.
pub const STEP_STARTED_MONOTONIC_NS_ENV: &str = "SAFE_CI_STEP_STARTED_MONOTONIC_NS";

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
    /// Stop scheduling new steps after a failure.
    stop: bool,
    /// Summed inner-jobs width of concurrently-running steps (CPA core-budget gate; see
    /// [`cores_free`]). Always tracked; only enforced when a `core_budget` is set.
    cores_used: i64,
    /// Accumulated per-step measurement rows (forwarded to a metrics sink after the run).
    step_profile_rows: Vec<ProfileRow>,
    /// Per-step ownership nonce, so the eager-cancel path can terminate another step's escapees
    /// as thoroughly as that step's own supervisor would (see [`kill_by_nonce`]).
    running_nonces: HashMap<String, String>,
    /// The WHOLE RUN exceeded its outer wall budget and cut its in-flight steps short.
    run_timed_out: bool,
    /// Child processes currently alive, and the largest count observed during this run.
    active_processes: usize,
    max_concurrent_steps: usize,
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
/// Ordinary descendants inherit `SAFE_CI_DAG_RUNNER_STEP=<nonce>` through `fork`/`execve`, so the
/// exact NUL-delimited environment entry can still associate those environment-preserving
/// escapees with their step. It is never a process-name, command-line, or substring match.
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
    step.hint
        .resources
        .iter()
        .all(|(r, n)| sh.resource_avail.get(r).copied().unwrap_or(0) >= *n)
}

/// The step's inner-jobs width for the core-budget gate (its `preferred_inner_jobs`, else 1).
/// Under `--planner cpa` this is the allocated width baked into the hint.
fn step_width(step: &Step) -> i64 {
    match preferred_inner_jobs(step, None) {
        Some(w) if w > 0 => w,
        _ => 1,
    }
}

// True when the step fits the remaining core budget, OR nothing is running (so a step wider than
// the whole budget still runs — alone — instead of deadlocking). Inactive (always true) when
// `core_budget` is `None` (the non-CPA default). Mirrors Python's `Runner._cores_free`.
fn cores_free(sh: &Shared, step: &Step, core_budget: Option<i64>) -> bool {
    match core_budget {
        None => true,
        Some(_) if sh.cores_used == 0 => true,
        Some(p) => sh.cores_used + step_width(step) <= p,
    }
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

/// Serialize one status line to stdout (each `println!` is atomic, so lines never interleave).
fn emit(line: &str) {
    println!("{line}");
}

/// The runner owns the immutable graph + policy and the shared mutable state.
struct Runner {
    steps: Arc<HashMap<String, Step>>,
    order: Vec<String>,
    intentional_skips: Vec<(String, crate::model::IntentionalSkipReason)>,
    jobs: i64,
    keep_going: bool,
    verbosity: i64,
    /// Default inner-parallelism flag template (e.g. "-j") for steps without their own.
    default_jobs_flag: String,
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
    cpu_timeout_multiplier: f64,
    cpu_timeout_platform: String,
    // CPA core-budget gate (MCPA's insight, PLANNER_DESIGN.md §5.7): when set, the ready-set loop
    // never lets the summed inner-jobs width of concurrently-running steps exceed this total core
    // budget `P`. `None` (non-CPA planners) disables the gate — behavior is unchanged.
    core_budget: Option<i64>,
    /// OUTER wall budget for the WHOLE run, in seconds; `None` leaves the run unbounded.
    ///
    /// Independent of every per-step budget, and that independence is the point: no combination of
    /// individually-legal steps can run past it.
    run_timeout_s: Option<i64>,
    /// Durable, incrementally-flushed evidence for this run (per-step logs + boundary journal), or
    /// `None` when the operator opted out or the directory could not be created.
    evidence: Option<Arc<RunEvidence>>,
    shared: Arc<Mutex<Shared>>,
}

impl Runner {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cfg: &DagConfig,
        jobs: i64,
        keep_going: bool,
        verbosity: i64,
        cgroups: BoxedCgroups,
        order_override: Option<Vec<String>>,
        core_budget: Option<i64>,
        run_timeout_s: Option<i64>,
    ) -> Self {
        let steps: HashMap<String, Step> = cfg.steps.iter().map(|s| (s.tag(), s.clone())).collect();
        // Dispatch order. When the caller supplies an explicit order (e.g. a critical-path
        // planner's) use it verbatim; otherwise default to LPT: sort tags by est_duration_s
        // DESCENDING (stable, so ties keep cfg/registration order, matching Python's stable
        // reverse sort).
        let order: Vec<String> = order_override.unwrap_or_else(|| {
            let mut o: Vec<String> = cfg.steps.iter().map(|s| s.tag()).collect();
            o.sort_by(|a, b| {
                let ea = steps[a].hint.est_duration_s;
                let eb = steps[b].hint.est_duration_s;
                eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
            });
            o
        });
        let resource_avail: HashMap<String, i64> = cfg
            .resource_caps
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let intentional_skips = cfg
            .steps
            .iter()
            .filter_map(|step| step.skip_reason.map(|reason| (step.tag(), reason)))
            .collect();
        Runner {
            steps: Arc::new(steps),
            order,
            intentional_skips,
            jobs: jobs.max(1),
            keep_going,
            verbosity,
            default_jobs_flag: cfg.default_jobs_flag.clone(),
            cgroups,
            mem_cap_factor: cfg.mem_cap_factor,
            default_step_mem_cap_bytes: cfg.default_step_mem_cap_bytes,
            default_step_cpu_count: cfg.default_step_cpu_count,
            default_step_cpu_timeout: cfg.default_step_cpu_timeout,
            cpu_timeout_multiplier: cfg.cpu_timeout_multiplier,
            cpu_timeout_platform: cfg.cpu_timeout_platform.clone(),
            core_budget,
            run_timeout_s,
            evidence: RunEvidence::open(default_log_dir()).map(Arc::new),
            shared: Arc::new(Mutex::new(Shared {
                done: HashMap::new(),
                running: HashSet::new(),
                running_pids: HashMap::new(),
                aborted: HashSet::new(),
                resource_avail,
                failed: false,
                stop: false,
                cores_used: 0,
                step_profile_rows: Vec::new(),
                running_nonces: HashMap::new(),
                run_timed_out: false,
                active_processes: 0,
                max_concurrent_steps: 0,
            })),
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

    /// Drive the DAG to completion; returns `(ok, wall_seconds)`.
    fn run(&self) -> (bool, f64) {
        let mut handles: Vec<thread::JoinHandle<()>> = Vec::new();
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
            let mut launchable: Vec<Step> = Vec::new();
            {
                let mut sh = self.shared.lock().unwrap();
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
                        for (tag, _pid, _nonce) in &cut {
                            sh.aborted.insert(tag.clone());
                        }
                        reap_many(&self.cgroups, &cut);
                    }
                }
                let skipped = self.skipped(&sh);
                if sh.running.is_empty()
                    && (sh.stop
                        || sh.done.len() + skipped.len() + self.intentional_skips.len()
                            >= self.steps.len())
                {
                    break;
                }
                if !sh.stop {
                    for tag in &self.order {
                        let step = self.steps[tag].clone();
                        if sh.done.contains_key(tag)
                            || sh.running.contains(tag)
                            || skipped.contains(tag)
                            || step.skip_reason.is_some()
                        {
                            continue;
                        }
                        if !deps_known(&sh, &step) {
                            continue;
                        }
                        if !deps_ok(&sh, &step) {
                            continue;
                        }
                        if sh.running.len() as i64 >= self.jobs {
                            break;
                        }
                        if !res_free(&sh, &step) {
                            continue;
                        }
                        if !cores_free(&sh, &step, self.core_budget) {
                            continue;
                        }
                        sh.running.insert(tag.clone());
                        acquire(&mut sh, &step);
                        sh.cores_used += step_width(&step);
                        launchable.push(step);
                    }
                }
            }
            for step in launchable {
                let shared = Arc::clone(&self.shared);
                let keep_going = self.keep_going;
                let verbosity = self.verbosity;
                let default_jobs_flag = self.default_jobs_flag.clone();
                let cgroups = self.cgroups.clone();
                let mem_cap_factor = self.mem_cap_factor;
                let default_step_mem_cap_bytes = self.default_step_mem_cap_bytes;
                let default_step_cpu_count = self.default_step_cpu_count;
                let default_step_cpu_timeout = self.default_step_cpu_timeout;
                let evidence = self.evidence.clone();
                let cpu_timeout_multiplier = self.cpu_timeout_multiplier;
                let cpu_timeout_platform = self.cpu_timeout_platform.clone();
                handles.push(thread::spawn(move || {
                    run_step(StepCtx {
                        step,
                        shared,
                        keep_going,
                        verbosity,
                        default_jobs_flag,
                        cgroups,
                        mem_cap_factor,
                        default_step_mem_cap_bytes,
                        default_step_cpu_count,
                        default_step_cpu_timeout,
                        evidence,
                        cpu_timeout_multiplier,
                        cpu_timeout_platform,
                    });
                }));
            }
            thread::sleep(LOOP_SLEEP);
        }
        for h in handles {
            let _ = h.join();
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
        let failed = self.shared.lock().unwrap().failed;
        (!failed, wall_start.elapsed().as_secs_f64())
    }

    fn result(&self, wall: f64) -> RunResult {
        let sh = self.shared.lock().unwrap();
        let outcomes: Vec<StepOutcome> = self
            .order
            .iter()
            .filter_map(|t| sh.done.get(t).cloned())
            .collect();
        let mut skipped: Vec<String> = self.skipped(&sh).into_iter().collect();
        skipped.sort();
        RunResult {
            ok: !sh.failed,
            wall_s: wall,
            outcomes,
            skipped,
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
    buf: Arc<Mutex<Vec<u8>>>,
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
                    buf.lock().unwrap().extend_from_slice(bytes);
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

fn failure_detail_lines(tag: &str, streams: &[&[u8]], verbosity: i64) -> Vec<String> {
    let mut rendered = Vec::new();
    for bytes in streams {
        // stdout and stderr are independent pipes: neither stream may borrow a
        // test boundary observed on the other. A fresh context per stream makes
        // cross-pipe scheduling incapable of assigning the wrong test name.
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
    cgroups: BoxedCgroups,
    mem_cap_factor: f64,
    /// SMALL forcing-function defaults for an undeclared step (see `model::DEFAULT_SMALL_*`).
    default_step_mem_cap_bytes: Option<i64>,
    default_step_cpu_count: Option<i64>,
    default_step_cpu_timeout: i64,
    /// Run-level durable evidence sink (per-step log + test-boundary journal), if enabled.
    evidence: Option<Arc<RunEvidence>>,
    cpu_timeout_multiplier: f64,
    cpu_timeout_platform: String,
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
struct TerminationBoundary<'a> {
    event: &'a str,
    limit_s: i64,
    elapsed_s: f64,
    cpu_stats: Option<BTreeMap<String, i64>>,
}

fn capture_termination_evidence(
    evidence: &Option<Arc<RunEvidence>>,
    sink: &StepStream,
    tag: &str,
    pid: u32,
    nonce: &str,
    boundary: TerminationBoundary<'_>,
) -> Culprit {
    let mut cpu_fields = Vec::new();
    if let Some(stats) = &boundary.cpu_stats {
        let usage_usec = stats.get("usage_usec").copied().unwrap_or(0);
        let user_usec = stats.get("user_usec").copied().unwrap_or(0);
        let system_usec = stats.get("system_usec").copied().unwrap_or(0);
        cpu_fields.extend([
            (
                "cpu_used_s",
                format!("{:.6}", usage_usec as f64 / 1_000_000.0),
            ),
            ("cpu_usage_usec", usage_usec.to_string()),
            ("cpu_user_usec", user_usec.to_string()),
            ("cpu_system_usec", system_usec.to_string()),
        ]);
        emit(&format!(
            "[{tag}] ↳ step cgroup CPU at termination: usage={:.6}s user={:.6}s system={:.6}s",
            usage_usec as f64 / 1_000_000.0,
            user_usec as f64 / 1_000_000.0,
            system_usec as f64 / 1_000_000.0,
        ));
    }
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
        let mut fields = vec![
            ("step", tag.to_string()),
            ("elapsed_s", format!("{:.3}", boundary.elapsed_s)),
            ("limit_s", boundary.limit_s.to_string()),
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
        ];
        fields.extend(cpu_fields);
        e.record(boundary.event, &fields);
    }
    culprit
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
        cgroups,
        mem_cap_factor,
        default_step_mem_cap_bytes,
        default_step_cpu_count,
        default_step_cpu_timeout,
        evidence,
        cpu_timeout_multiplier,
        cpu_timeout_platform,
    } = ctx;
    let tag = step.tag();
    emit(&format!("[{tag}] \u{25b6} START  {}", step.desc));

    // Append the step's inner-parallelism (concurrency) flag when it declares one. No-op when the
    // step has no preferred_inner_jobs.
    let inner_jobs = preferred_inner_jobs(&step, None);
    let base_cmd = command_with_inner_jobs(&step, &default_jobs_flag, inner_jobs);
    // SMALL forcing-function defaults for an undeclared step: fall back to the DAG's tight
    // 1-GiB memory.max / 1-core cpu.max / 10-s CPU-time floor when the step declares nothing
    // for that dimension. An explicit hint always wins.
    let mem_max =
        crate::sizing::step_mem_cap_bytes(&step, mem_cap_factor, default_step_mem_cap_bytes);
    // cpu.max core cap. `inner_jobs` (declared width) still keys the command's `-j` flag above;
    // the cgroup core cap falls back to the small default so an undeclared step is 1-core-boxed
    // WITHOUT appending a bogus `-j 1` to a command that may not accept it.
    let cpu_count = effective_cpu_count(&step, default_step_cpu_count);
    // CPU-time budget: declared cpu_timeout (>0) wins, else the small 10-s default.
    let cpu_canonical = canonical_cpu_timeout(&step, default_step_cpu_timeout);
    // The ENFORCED budget is the canonical one scaled for this platform; both are kept so a
    // breach can name the graph's number and the policy that changed it.
    let cpu_budget = scale_cpu_timeout(cpu_canonical, cpu_timeout_multiplier);
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
    cmd.env(STEP_NONCE_ENV, &nonce);
    // Own process group (pgid == child pid) so teardown can reap the whole tree with a
    // negative-pid kill without ever touching the runner's own group.
    cmd.process_group(0);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let start = Instant::now();
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
            let mut sh = shared.lock().unwrap();
            sh.running.remove(&tag);
            sh.running_pids.remove(&tag);
            sh.running_nonces.remove(&tag);
            release(&mut sh, &step);
            let outcome = StepOutcome::failed(
                tag.clone(),
                elapsed,
                format!("spawn failed: {e}"),
                None,
                false,
                0,
                false,
                step.timeout,
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
            sh.failed = true;
            sh.stop = true;
            drop(sh);
            emit(&format!(
                "[{tag}] \u{2717} FAIL   {} (spawn failed: {e})",
                step.desc
            ));
            return;
        }
    };
    let pid = child.id();
    {
        let mut sh = shared.lock().unwrap();
        sh.running_pids.insert(tag.clone(), pid);
        sh.running_nonces.insert(tag.clone(), nonce.clone());
        sh.active_processes += 1;
        sh.max_concurrent_steps = sh.max_concurrent_steps.max(sh.active_processes);
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
                ("timeout_s", step.timeout.to_string()),
                ("cmd", run_cmd.clone()),
            ],
        );
    }

    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(
            out,
            Arc::clone(&out_buf),
            tag.clone(),
            verbosity,
            Arc::clone(&sink),
            Arc::new(Mutex::new(ConsoleTestIdentity::default())),
        ));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(
            err,
            Arc::clone(&err_buf),
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
    let progress_stop = Arc::new(AtomicBool::new(false));
    let progress_thread = {
        let stop = Arc::clone(&progress_stop);
        let psink = Arc::clone(&sink);
        let ptag = tag.clone();
        let pstart = start;
        thread::spawn(move || {
            let tick = Duration::from_millis(200);
            let mut since = Duration::ZERO;
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(tick);
                since += tick;
                if since < PROGRESS_INTERVAL {
                    continue;
                }
                since = Duration::ZERO;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
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

    // Poll the step's cgroup once per MONITOR_INTERVAL for two purposes: (1) a per-step peak
    // descendant-thread count (metrics only), and (2) CPU-time budget enforcement. Only when
    // boxing is enabled (both readings are meaningless otherwise), so the un-boxed path adds no
    // extra thread. The poll is interruptible (checks the stop flag every 50ms), so joining it at
    // step end returns promptly.
    let monitor_stop = Arc::new(AtomicBool::new(false));
    let thread_peak = Arc::new(Mutex::new(None::<i64>));
    let cpu_exceeded = Arc::new(AtomicBool::new(false));
    let termination_culprit = Arc::new(Mutex::new(None::<Culprit>));
    let monitor: Option<thread::JoinHandle<()>> = if boxed {
        let stop = Arc::clone(&monitor_stop);
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
        Some(thread::spawn(move || {
            let mut since = Duration::ZERO;
            let tick = Duration::from_millis(50);
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(tick);
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
                    // CPU-time budget: a load-invariant per-step ceiling on consumed user+system
                    // CPU (cgroup cpu.stat usage_usec), mirroring the Python runner exactly. Reap
                    // the whole tree once when over budget, then exit the monitor.
                    if cpu_timeout > 0 && !cpu_flag.load(Ordering::Relaxed) {
                        if let Some(cs) = c.cpu_stats(&t) {
                            let cpu_used_s =
                                cs.get("usage_usec").copied().unwrap_or(0) as f64 / 1_000_000.0;
                            if cpu_used_s >= cpu_timeout as f64 {
                                cpu_flag.store(true, Ordering::Relaxed);
                                let culprit = capture_termination_evidence(
                                    &mevidence,
                                    &msink,
                                    &t,
                                    mpid,
                                    &mnonce,
                                    TerminationBoundary {
                                        event: "cpu_timeout",
                                        limit_s: cpu_timeout,
                                        elapsed_s: mstart.elapsed().as_secs_f64(),
                                        cpu_stats: Some(cs),
                                    },
                                );
                                if let Ok(mut slot) = mculprit.lock() {
                                    *slot = Some(culprit);
                                }
                                reap(&cg, &t, mpid, Some(&mnonce));
                                return;
                            }
                        }
                    }
                }
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
                if start.elapsed().as_secs() as i64 >= step.timeout {
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
                            limit_s: step.timeout,
                            elapsed_s: start.elapsed().as_secs_f64(),
                            cpu_stats: cgroups.as_ref().and_then(|c| c.cpu_stats(&tag)),
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
        let mut sh = shared.lock().unwrap();
        sh.active_processes = sh.active_processes.saturating_sub(1);
    }

    // Reap the whole tree (cgroup.kill + killpg) so orphan grandchildren die now and the readers
    // see EOF; then stop the monitor and join the reader threads.
    reap(&cgroups, &tag, pid, Some(&nonce));
    monitor_stop.store(true, Ordering::Relaxed);
    if let Some(m) = monitor {
        join_bounded(m, &tag, "monitor", JOIN_WAIT);
    }
    progress_stop.store(true, Ordering::Relaxed);
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
    let (oom, peak, cpu_stats) = match &cgroups {
        Some(cg) if cg.enabled() => (cg.oom_kills(&tag), cg.peak_bytes(&tag), cg.cpu_stats(&tag)),
        _ => (0, None, None),
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

    // Combined captured output (stdout then stderr) for the summary + failure detail.
    let stdout = out_buf.lock().unwrap().clone();
    let stderr = err_buf.lock().unwrap().clone();
    let mut combined: Vec<u8> = stdout.clone();
    combined.extend_from_slice(&stderr);
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
        resolve_effective_inner_jobs(inner_jobs).to_string(),
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
            inner_jobs,
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
        let mut sh = shared.lock().unwrap();
        sh.running.remove(&tag);
        sh.running_pids.remove(&tag);
        sh.running_nonces.remove(&tag);
        release(&mut sh, &step);
        sh.cores_used -= step_width(&step);
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
                step.timeout,
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
            // A REAL failure: mark failed + stop launching NEW steps. Eager-exit (default): reap
            // every step still running so a fast failure doesn't wait for a slow in-flight build.
            // keep_going instead lets those in-flight steps finish; it does NOT launch further steps.
            sh.failed = true;
            sh.stop = true;
            if !keep_going {
                let others: Vec<(String, u32, Option<String>)> = sh
                    .running_pids
                    .iter()
                    .map(|(k, v)| (k.clone(), *v, sh.running_nonces.get(k).cloned()))
                    .collect();
                for (other, _other_pid, _other_nonce) in &others {
                    sh.aborted.insert(other.clone());
                }
                reap_many(&cgroups, &others);
            }
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
            "[{tag}] \u{2298} ABORT  {} ({dur}s \u{2014} eager-exit after another step failed; keep_going lets in-flight steps finish)",
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
        for line in failure_detail_lines(&tag, &[&stdout, &stderr], verbosity) {
            emit(&line);
        }
        emit(&format!("[{tag}] ----- end detail -----"));
    }

    // Terminal record. Written for EVERY step, pass or fail, so the journal alone answers "what
    // was this run doing" without needing the end-of-run profile rows that a hard kill destroys.
    if let Some(e) = &evidence {
        let counts = sink.counts();
        e.record(
            "step_end",
            &[
                ("step", tag.clone()),
                ("ok", ok.to_string()),
                ("aborted", was_aborted.to_string()),
                ("timed_out", timed_out.to_string()),
                ("cpu_timed_out", cpu_timed_out.to_string()),
                ("elapsed_s", format!("{elapsed:.3}")),
                ("tests_started", counts.started.to_string()),
                ("tests_completed", counts.completed.to_string()),
                (
                    "culprit_test",
                    culprit
                        .as_ref()
                        .and_then(|c| c.test.clone())
                        .unwrap_or_default(),
                ),
            ],
        );
    }
}

/// Run a whole DAG and return its [`RunResult`] (no cgroup boxing, no metrics recording).
///
/// * `jobs`: outer scheduler fan-out (`-j`), clamped to at least 1.
/// * `keep_going`: on a failure, let already-running steps finish instead of eager-cancelling
///   them; the scheduler still stops launching new steps (it does NOT run every still-runnable
///   step), so in-flight steps report their own pass/fail rather than ABORTED.
/// * `verbosity`: 0 quiet (+failures), 1 default (+summaries), 2-4 stream child stdout,
///   and >=5 streams with the deepest recognized test identity on every line.
pub fn run_dag(cfg: &DagConfig, jobs: i64, keep_going: bool, verbosity: i64) -> RunResult {
    run_dag_boxed(cfg, jobs, keep_going, verbosity, None)
}

/// Run a whole DAG with an optional per-step cgroup manager (the real-work entry point).
///
/// `cgroups` supplies two-level cgroup-v2 per-step boxing + setsid-proof teardown when enabled;
/// `None` (or a disabled manager) runs unboxed with process-group teardown. Per-step measurement
/// rows are always collected into [`RunResult::step_profile_rows`] for a metrics sink to record.
pub fn run_dag_boxed(
    cfg: &DagConfig,
    jobs: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
) -> RunResult {
    run_dag_boxed_ordered(cfg, jobs, keep_going, verbosity, cgroups, None, None)
}

/// Like [`run_dag_boxed`] but with an explicit dispatch `order` (e.g. a critical-path planner's)
/// and an optional CPA `core_budget` (`P`): when set, the scheduler never lets the summed
/// inner-jobs width of concurrently-running steps exceed it. `order = None` uses the built-in
/// longest-processing-time default; `core_budget = None` disables the core gate.
pub fn run_dag_boxed_ordered(
    cfg: &DagConfig,
    jobs: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
    core_budget: Option<i64>,
) -> RunResult {
    run_dag_boxed_deadline(
        cfg,
        jobs,
        keep_going,
        verbosity,
        cgroups,
        order,
        core_budget,
        None,
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
    let mut bad: Vec<(String, i64)> = cfg
        .steps
        .iter()
        .filter(|s| s.timeout >= run_timeout_s)
        .map(|s| (s.tag(), s.timeout))
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
    jobs: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
    core_budget: Option<i64>,
    run_timeout_s: Option<i64>,
) -> RunResult {
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
    let runner = Runner::new(
        cfg,
        jobs,
        keep_going,
        verbosity,
        cgroups,
        order,
        core_budget,
        run_timeout_s,
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
    use crate::model::{IntentionalSkipReason, ResourceHint};
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
            skip_reason: None,
            write_domains: None,
            write_domain_guarantee: None,
        }
    }

    #[test]
    fn simple_dag_all_pass_respects_deps() {
        let dir = std::env::temp_dir().join(format!("scdr_test_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("scdr_epoch_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("scdr_skip_{}", std::process::id()));
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

    #[test]
    fn resource_cap_serializes_concurrent_steps() {
        let dir = std::env::temp_dir().join(format!("scdr_res_{}", std::process::id()));
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
    fn write_domain_policy_refuses_before_spawning() {
        let dir = std::env::temp_dir().join(format!(
            "scdr_write_domain_preexec_{}_{}",
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
        let res = run_dag(&cfg, 1, false, 0);
        assert!(res.ok, "expected '-j4' to be appended so the check passes");
    }
}
