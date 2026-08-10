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
// `memory.max` cap, and teardown writes the step's `cgroup.kill` FIRST (a setsid-proof atomic
// SIGKILL of the whole subtree) then killpg as a belt-and-suspenders. Without a manager the
// step runs unboxed and teardown is a plain negative-pid `kill(1)` process-group SIGKILL (no
// `unsafe`/`libc`). Per-step measurement rows are collected for the perf-log sink either way.

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
    default_log_dir, mint_step_nonce, Culprit, RunEvidence, StepStream, STEP_NONCE_ENV,
};
use crate::cgroup::CgroupManager;
use crate::model::{
    command_with_inner_jobs, effective_cpu_count, effective_cpu_timeout, preferred_inner_jobs,
    step_classification, DagConfig, RunResult, Step, StepOutcome,
};
use crate::profile_enrich::{resolve_effective_inner_jobs, step_enrichment_columns};

/// A per-step measurement row (column -> value), matching the perflog step-profile schema.
type ProfileRow = BTreeMap<String, String>;

/// Optional per-step cgroup manager shared (behind an `Arc`) across the run's supervisor threads.
pub type BoxedCgroups = Option<Arc<dyn CgroupManager>>;

/// Per-step monitor poll interval (seconds) for descendant-thread-peak sampling.
const MONITOR_INTERVAL: Duration = Duration::from_secs(1);

// Scheduler idle interval between ready-set sweeps (matches Python's `time.sleep(0.05)`).
const LOOP_SLEEP: Duration = Duration::from_millis(50);
/// Poll interval while a step's supervisor waits for the child (std has no wait-with-timeout).
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How many times [`kill_descendants`] re-walks `/proc` before giving up. The walk races a tree
/// that may still be forking, so one pass can miss a child born mid-sweep; a small fixed bound
/// converts a pathological forker into a reported failure instead of an unbounded loop.
const DESCENDANT_KILL_SWEEPS: usize = 4;
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
}

/// SIGKILL a whole process group by pid (the child is its own group leader via
/// `process_group(0)`), using a negative-pid `kill(1)` so no `unsafe`/`libc` is needed.
fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        // A negative pid names a process group, but without the option terminator GNU kill may
        // parse it as another option. That silently left the group leader alive while the later
        // descendant sweep killed only its blocking child, allowing the shell to continue.
        .arg("--")
        .arg(format!("-{pid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if fresh == 0 {
            break;
        }
    }
    killed.len()
}

/// SIGKILL every process carrying this step's ownership nonce in its environment.
///
/// THIS IS THE ESCAPEE CLOSER, and it is what makes termination GENERAL rather than boxed-only.
/// The three earlier mechanisms each have a hole. `cgroup.kill` is exact but exists only when
/// containment is available. A process-group kill misses anything that called `setsid`, because an
/// escapee changes session and pgid. The `/proc` parentage sweep misses a DOUBLE-FORK escapee,
/// whose intermediate parent exits so the survivor reparents away from the step entirely —
/// measured on a planted escapee, which showed `PPid=1`. Subreaper adoption pulls such orphans
/// back to the RUNNER, but that makes them siblings of every other step's tree, not descendants of
/// the step that spawned them, so a parentage walk rooted at the step still cannot see them and a
/// walk rooted at the runner would reach other steps' live work.
///
/// A nonce closes exactly that hole. Each step is launched with `SAFE_CI_DAG_RUNNER_STEP=<nonce>`
/// in its environment; `fork` copies the environment and `execve` carries it, so EVERY descendant
/// inherits it however far it runs from its origin — changing session, changing process group, and
/// being reparented all leave it intact.
///
/// SAFETY, AND WHY THIS IS NOT A PATTERN KILL. The match is an exact NUL-delimited environment
/// entry against a token this runner minted from its own pid, a per-process sequence number, and
/// the wall clock. It is never a process name, never a command line, and never a substring of
/// either. No process outside this step's own fork tree can carry it, so unlike a `pkill`-style
/// match it cannot reach a sibling agent's work on a shared machine. Processes owned by other
/// users are unreadable and are skipped rather than signalled. The runner's own pid is excluded.
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
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
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

/// Become the reaper for orphaned descendants, so a `setsid`/double-fork escapee stays reachable.
///
/// WITHOUT THIS THE DESCENDANT SWEEP HAS NOTHING TO FIND. An escapee's intermediate parent exits
/// immediately, so the grandchild reparents to init and is no longer a descendant of anything the
/// runner knows about — measured directly on a planted escapee, which showed `PPid=1`. That is the
/// same reason `killpg` misses it: it has left the family, not merely changed process group. With
/// `PR_SET_CHILD_SUBREAPER` the kernel reparents such orphans to THIS process instead of init, so
/// they remain enumerable by parentage and [`kill_descendants`] can reach them.
///
/// The attribute is per-process and is not inherited by children, so setting it once on the runner
/// is both necessary and sufficient. A failure is reported once and is not fatal: the run then
/// degrades to exactly today's behaviour rather than refusing to start.
fn become_subreaper() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: prctl with PR_SET_CHILD_SUBREAPER takes an integer flag and touches no memory
        // owned by this process; the remaining arguments are ignored for this option.
        let rc = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if rc != 0 {
            eprintln!(
                "[scheduler] WARNING: could not become child subreaper; setsid/double-fork \
                 escapees will reparent to init and survive step teardown."
            );
        }
    });
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
        Runner {
            steps: Arc::new(steps),
            order,
            jobs: jobs.max(1),
            keep_going,
            verbosity,
            default_jobs_flag: cfg.default_jobs_flag.clone(),
            cgroups,
            mem_cap_factor: cfg.mem_cap_factor,
            default_step_mem_cap_bytes: cfg.default_step_mem_cap_bytes,
            default_step_cpu_count: cfg.default_step_cpu_count,
            default_step_cpu_timeout: cfg.default_step_cpu_timeout,
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
                if sk.contains(tag) || sh.done.contains_key(tag) || sh.running.contains(tag) {
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
                        for (tag, pid, nonce) in cut {
                            sh.aborted.insert(tag.clone());
                            reap(&self.cgroups, &tag, pid, nonce.as_deref());
                        }
                    }
                }
                let skipped = self.skipped(&sh);
                if sh.running.is_empty()
                    && (sh.stop || sh.done.len() + skipped.len() >= self.steps.len())
                {
                    break;
                }
                if !sh.stop {
                    for tag in &self.order {
                        if sh.done.contains_key(tag)
                            || sh.running.contains(tag)
                            || skipped.contains(tag)
                        {
                            continue;
                        }
                        let step = self.steps[tag].clone();
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
            step_profile_rows: sh.step_profile_rows.clone(),
            run_timed_out: sh.run_timed_out,
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
fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    buf: Arc<Mutex<Vec<u8>>>,
    tag: String,
    stream: bool,
    sink: Arc<StepStream>,
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
                    if stream {
                        // Console streaming stays line-at-a-time: hold back the unterminated tail
                        // so a partially-received line is not printed twice.
                        pending.extend_from_slice(bytes);
                        while let Some(idx) = pending.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = pending.drain(..=idx).collect();
                            let text = String::from_utf8_lossy(&line);
                            emit(&format!("[{tag}] {}", text.trim_end_matches('\n')));
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if stream && !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending);
            emit(&format!("[{tag}] {}", text.trim_end_matches('\n')));
        }
    })
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
}

/// Tear down one step's whole process tree: `cgroup.kill` first (setsid-proof), then killpg.
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
fn reap(cgroups: &BoxedCgroups, tag: &str, pid: u32, nonce: Option<&str>) {
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
    kill_group(pid);
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
    // only mechanism that reaches a double-fork escapee at all.
    if let Some(n) = nonce {
        let swept = kill_by_nonce(n);
        if swept > 0 {
            eprintln!(
                "[scheduler] step {tag}: killed {swept} process(es) by ownership nonce that \
                 neither the process-group kill nor the parentage sweep could reach \
                 (setsid/double-fork escapees)."
            );
        }
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
    let cpu_budget = effective_cpu_timeout(&step, default_step_cpu_timeout);
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
    let stream = verbosity >= 2;

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
                false,
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
            stream,
            Arc::clone(&sink),
        ));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(
            err,
            Arc::clone(&err_buf),
            tag.clone(),
            stream,
            Arc::clone(&sink),
        ));
    }

    // Poll the step's cgroup once per MONITOR_INTERVAL for two purposes: (1) a per-step peak
    // descendant-thread count (metrics only), and (2) CPU-time budget enforcement. Only when
    // boxing is enabled (both readings are meaningless otherwise), so the un-boxed path adds no
    // extra thread. The poll is interruptible (checks the stop flag every 50ms), so joining it at
    // step end returns promptly.
    let monitor_stop = Arc::new(AtomicBool::new(false));
    let thread_peak = Arc::new(Mutex::new(None::<i64>));
    let cpu_exceeded = Arc::new(AtomicBool::new(false));
    let monitor: Option<thread::JoinHandle<()>> = if boxed {
        let stop = Arc::clone(&monitor_stop);
        let peak = Arc::clone(&thread_peak);
        let cpu_flag = Arc::clone(&cpu_exceeded);
        let cg = cgroups.clone();
        let t = tag.clone();
        let cpu_timeout = cpu_budget;
        let mpid = pid;
        let mnonce = nonce.clone();
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
                    // Journal the teardown BEFORE attempting it. If the kill escalates to the CI
                    // provider cancelling the whole job, this record is already on disk; a record
                    // written after a successful kill would be missing in exactly the case that
                    // most needs explaining.
                    if let Some(e) = &evidence {
                        let c = sink.culprit();
                        e.record(
                            "step_timeout",
                            &[
                                ("step", tag.clone()),
                                ("elapsed_s", format!("{:.3}", start.elapsed().as_secs_f64())),
                                ("timeout_s", step.timeout.to_string()),
                                ("culprit_test", c.test.clone().unwrap_or_default()),
                                ("culprit_basis", c.how.to_string()),
                                ("tests_completed", c.completed.to_string()),
                            ],
                        );
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

    // Reap the whole tree (cgroup.kill + killpg) so orphan grandchildren die now and the readers
    // see EOF; then stop the monitor and join the reader threads.
    reap(&cgroups, &tag, pid, Some(&nonce));
    monitor_stop.store(true, Ordering::Relaxed);
    if let Some(m) = monitor {
        join_bounded(m, &tag, "monitor", JOIN_WAIT);
    }
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
        Some(sink.culprit())
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
    let mut combined: Vec<u8> = out_buf.lock().unwrap().clone();
    combined.extend_from_slice(&err_buf.lock().unwrap());
    let summary = last_line(&combined);

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
            StepOutcome::aborted_outcome(tag.clone(), elapsed, summary.clone(), returncode)
        } else if ok {
            StepOutcome::passed(tag.clone(), elapsed, summary.clone(), returncode)
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
                false,
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
                for (other, other_pid, other_nonce) in others {
                    sh.aborted.insert(other.clone());
                    reap(&cgroups, &other, other_pid, other_nonce.as_deref());
                }
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
        let text = String::from_utf8_lossy(&combined);
        for line in text.lines() {
            emit(&format!("[{tag}] {line}"));
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
/// * `verbosity`: 0 quiet (+failures), 1 default (+summaries), `>=2` stream child stdout.
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
                step_profile_rows: Vec::new(),
                run_timed_out: false,
            };
        }
    }
    become_subreaper();
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
    use crate::model::ResourceHint;
    use std::collections::BTreeMap;

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
