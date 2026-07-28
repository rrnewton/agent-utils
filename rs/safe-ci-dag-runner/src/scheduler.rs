//! The DAG runner: greedy, memory-/resource-aware step scheduling.
//!
//! Port of the OBSERVABLE scheduling behavior of `py/safe_ci_dag_runner/scheduler.py` for the
//! no-boxing default path (Python's `cgroups=None`). Reproduced from the reference:
//!
//! * Greedy ready-set loop: each pass launches every ready step (deps satisfied, resources
//!   free, under the `-j` fan-out, in longest-processing-time order) on its own supervisor
//!   thread, then sleeps briefly.
//! * Dependency gating + dep-FAILURE skip-closure (a failed dep transitively skips dependents).
//! * Named-resource capacity buckets (`hint.resources` vs `cfg.resource_caps`).
//! * Longest-processing-time (LPT) dispatch order (descending `est_duration_s`, stable).
//! * Per-step supervision via `bash -c` in its own process group (whole-tree teardown).
//! * Fail-fast (eager-exit): the first genuine failure stops launching NEW steps; by default it
//!   also eager-cancels in-flight steps (labelled ABORTED, not FAILED). `keep_going` only
//!   suppresses that eager-cancel so already-running steps finish — it does NOT keep launching
//!   still-runnable steps.
//! * Failure classification via [`crate::model::step_failure_reason`].
//!
//! Boxing: when a [`crate::cgroup::CgroupManager`] is supplied (the default `run` path), each
//! step is wrapped so its bash leader self-moves into a per-step child cgroup with an inner
//! `memory.max` cap, and teardown writes the step's `cgroup.kill` FIRST (a setsid-proof atomic
//! SIGKILL of the whole subtree) then killpg as a belt-and-suspenders. Without a manager the
//! step runs unboxed and teardown is a plain negative-pid `kill(1)` process-group SIGKILL (no
//! `unsafe`/`libc`). Per-step measurement rows are collected for the perf-log sink either way.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::ambient::{capture_ambient_snapshot, PsiReading};
use crate::cgroup::CgroupManager;
use crate::model::{
    command_with_inner_jobs, preferred_inner_jobs, step_classification, DagConfig, RunResult, Step,
    StepOutcome,
};
use crate::profile_enrich::{resolve_effective_inner_jobs, step_enrichment_columns};

/// A per-step measurement row (column -> value), matching the perflog step-profile schema.
type ProfileRow = BTreeMap<String, String>;

/// Optional per-step cgroup manager shared (behind an `Arc`) across the run's supervisor threads.
pub type BoxedCgroups = Option<Arc<dyn CgroupManager>>;

/// Per-step monitor poll interval (seconds) for descendant-thread-peak sampling.
const MONITOR_INTERVAL: Duration = Duration::from_secs(1);

/// Scheduler idle interval between ready-set sweeps (matches Python's `time.sleep(0.05)`).
const LOOP_SLEEP: Duration = Duration::from_millis(50);
/// Poll interval while a step's supervisor waits for the child (std has no wait-with-timeout).
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Mutable scheduler state, guarded by one lock (mirrors the Python `Runner`'s single lock).
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
    /// Accumulated per-step measurement rows (forwarded to a metrics sink after the run).
    step_profile_rows: Vec<ProfileRow>,
}

/// SIGKILL a whole process group by pid (the child is its own group leader via
/// `process_group(0)`), using a negative-pid `kill(1)` so no `unsafe`/`libc` is needed.
fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{pid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
    shared: Arc<Mutex<Shared>>,
}

impl Runner {
    fn new(
        cfg: &DagConfig,
        jobs: i64,
        keep_going: bool,
        verbosity: i64,
        cgroups: BoxedCgroups,
        order_override: Option<Vec<String>>,
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
            shared: Arc::new(Mutex::new(Shared {
                done: HashMap::new(),
                running: HashSet::new(),
                running_pids: HashMap::new(),
                aborted: HashSet::new(),
                resource_avail,
                failed: false,
                stop: false,
                step_profile_rows: Vec::new(),
            })),
        }
    }

    /// Tags whose deps FAILED (transitively) so they must never run — a fixpoint closure,
    /// ported from the Python `Runner._skipped`.
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
        loop {
            let mut launchable: Vec<Step> = Vec::new();
            {
                let mut sh = self.shared.lock().unwrap();
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
                        sh.running.insert(tag.clone());
                        acquire(&mut sh, &step);
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
                handles.push(thread::spawn(move || {
                    run_step(StepCtx {
                        step,
                        shared,
                        keep_going,
                        verbosity,
                        default_jobs_flag,
                        cgroups,
                        mem_cap_factor,
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
        }
    }
}

/// Read a child pipe to EOF into `buf`; when `stream`, also echo each line tagged.
fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    buf: Arc<Mutex<Vec<u8>>>,
    tag: String,
    stream: bool,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut br = BufReader::new(reader);
        let mut line: Vec<u8> = Vec::new();
        loop {
            line.clear();
            match br.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    buf.lock().unwrap().extend_from_slice(&line);
                    if stream {
                        let text = String::from_utf8_lossy(&line);
                        emit(&format!("[{tag}] {}", text.trim_end_matches('\n')));
                    }
                }
                Err(_) => break,
            }
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
}

/// Tear down one step's whole process tree: `cgroup.kill` first (setsid-proof), then killpg.
///
/// When per-step containment is enabled, writing the step's child `cgroup.kill` SIGKILLs the
/// ENTIRE subtree atomically, including `setsid`/double-fork escapees a process-group kill misses.
/// The killpg that follows is a belt-and-suspenders for the no-cgroup path. No Silent Failure: a
/// failed cgroup.kill while containment is enabled surfaces a warning.
fn reap(cgroups: &BoxedCgroups, tag: &str, pid: u32) {
    if let Some(cg) = cgroups {
        if cg.enabled() && !cg.kill(tag) {
            eprintln!(
                "[scheduler] WARNING: cgroup.kill for step {tag} failed; falling back to \
                 process-group kill only — setsid/double-fork escapees may survive."
            );
        }
    }
    kill_group(pid);
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
    } = ctx;
    let tag = step.tag();
    emit(&format!("[{tag}] \u{25b6} START  {}", step.desc));

    // Append the step's inner-parallelism (concurrency) flag when it declares one. No-op when the
    // step has no preferred_inner_jobs.
    let inner_jobs = preferred_inner_jobs(&step, None);
    let base_cmd = command_with_inner_jobs(&step, &default_jobs_flag, inner_jobs);
    // Inner per-step memory cap (bytes) from the step's hint (hard cap wins; else factor*rss).
    let mem_max = crate::sizing::step_mem_cap_bytes(&step, mem_cap_factor);
    // When boxing is enabled, prepare_command wraps the command so the bash leader self-moves into
    // the step's child cgroup BEFORE forking any grandchild (the cgroup-v2 fork-inheritance rule),
    // applying the inner memory/CPU caps. Disabled / absent -> the command is unchanged.
    let run_cmd = match &cgroups {
        Some(cg) if cg.enabled() => cg.prepare_command(&tag, &base_cmd, mem_max, inner_jobs),
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

    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(&run_cmd);
    for (k, v) in &step.env {
        cmd.env(k, v);
    }
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
    }

    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, Arc::clone(&out_buf), tag.clone(), stream));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, Arc::clone(&err_buf), tag.clone(), stream));
    }

    // Poll the step's cgroup descendant-thread count for a per-step peak (metrics only). Only when
    // boxing is enabled (thread_count is meaningless otherwise), so the un-boxed path adds no
    // extra thread. The poll is interruptible (checks the stop flag every 50ms and only samples
    // once per MONITOR_INTERVAL), so joining it at step end returns promptly.
    let monitor_stop = Arc::new(AtomicBool::new(false));
    let thread_peak = Arc::new(Mutex::new(None::<i64>));
    let monitor: Option<thread::JoinHandle<()>> = if boxed {
        let stop = Arc::clone(&monitor_stop);
        let peak = Arc::clone(&thread_peak);
        let cg = cgroups.clone();
        let t = tag.clone();
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
                if let Some(cg) = &cg {
                    if let Some(n) = cg.thread_count(&t) {
                        let mut p = peak.lock().unwrap();
                        *p = Some(p.map_or(n, |cur| cur.max(n)));
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
                    reap(&cgroups, &tag, pid);
                    break child.wait().unwrap_or_else(|_| {
                        // Fabricate a killed status if wait somehow fails.
                        std::process::ExitStatus::from_raw(9)
                    });
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
    reap(&cgroups, &tag, pid);
    monitor_stop.store(true, Ordering::Relaxed);
    if let Some(m) = monitor {
        let _ = m.join();
    }
    for r in readers {
        let _ = r.join();
    }

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

    let elapsed = start.elapsed().as_secs_f64();
    let dur = elapsed.round() as i64;
    let returncode: Option<i64> = match status.code() {
        Some(c) => Some(c as i64),
        None => status.signal().map(|s| -(s as i64)),
    };
    let ok = returncode == Some(0) && !timed_out;

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

    let (was_aborted, reason) = {
        let mut sh = shared.lock().unwrap();
        sh.running.remove(&tag);
        sh.running_pids.remove(&tag);
        release(&mut sh, &step);
        sh.step_profile_rows.push(row);
        let was_aborted = sh.aborted.contains(&tag);
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
                let others: Vec<(String, u32)> = sh
                    .running_pids
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
                for (other, other_pid) in others {
                    sh.aborted.insert(other.clone());
                    reap(&cgroups, &other, other_pid);
                }
            }
        }
        (was_aborted, reason)
    };

    // Emit the terminal status OUTSIDE the lock.
    if was_aborted {
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
    run_dag_boxed_ordered(cfg, jobs, keep_going, verbosity, cgroups, None)
}

/// Like [`run_dag_boxed`] but with an explicit dispatch `order` (e.g. a critical-path planner's).
/// `None` uses the built-in longest-processing-time default.
pub fn run_dag_boxed_ordered(
    cfg: &DagConfig,
    jobs: i64,
    keep_going: bool,
    verbosity: i64,
    cgroups: BoxedCgroups,
    order: Option<Vec<String>>,
) -> RunResult {
    if let Some(cg) = &cgroups {
        if !cg.enabled() {
            // No Silent Failure: a present-but-disabled manager means containment is degraded.
            eprintln!(
                "[scheduler] WARNING: per-step cgroup manager is present but disabled; containment \
                 is DEGRADED (process-group kill only, no inner memory/CPU caps)."
            );
        }
    }
    let runner = Runner::new(cfg, jobs, keep_going, verbosity, cgroups, order);
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
