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
//! * Fail-fast (eager-exit) with a `keep_going` override; eager-cancelled in-flight steps are
//!   labelled ABORTED, not FAILED.
//! * Failure classification via [`crate::model::step_failure_reason`].
//!
//! Scope note (0.1): this Rust build performs NO per-step cgroup boxing and NO perf logging
//! (Python's `cgroup.py` / `perflog.py` / `teardown.py` / `ambient.py` stay Python-only).
//! Teardown here is a safe, dependency-free process-group SIGKILL (a negative-pid `kill(1)`),
//! not a cgroup kill; that matches Python's process-group fallback when cgroups are off.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{DagConfig, RunResult, Step, StepOutcome};

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
    shared: Arc<Mutex<Shared>>,
}

impl Runner {
    fn new(cfg: &DagConfig, jobs: i64, keep_going: bool, verbosity: i64) -> Self {
        let steps: HashMap<String, Step> = cfg.steps.iter().map(|s| (s.tag(), s.clone())).collect();
        // LPT dispatch order: sort tags by est_duration_s DESCENDING; the sort is stable, so
        // ties keep cfg (registration) order, matching Python's stable reverse sort.
        let mut order: Vec<String> = cfg.steps.iter().map(|s| s.tag()).collect();
        order.sort_by(|a, b| {
            let ea = steps[a].hint.est_duration_s;
            let eb = steps[b].hint.est_duration_s;
            eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
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
            shared: Arc::new(Mutex::new(Shared {
                done: HashMap::new(),
                running: HashSet::new(),
                running_pids: HashMap::new(),
                aborted: HashSet::new(),
                resource_avail,
                failed: false,
                stop: false,
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
                handles.push(thread::spawn(move || {
                    run_step(step, shared, keep_going, verbosity);
                }));
            }
            thread::sleep(LOOP_SLEEP);
        }
        for h in handles {
            let _ = h.join();
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

/// Supervise ONE step: launch, pump stdout/stderr, enforce the timeout, reap, classify.
fn run_step(step: Step, shared: Arc<Mutex<Shared>>, keep_going: bool, verbosity: i64) {
    let tag = step.tag();
    emit(&format!("[{tag}] \u{25b6} START  {}", step.desc));

    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(&step.cmd);
    // env = inherited process env + the step's overrides.
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

    // Wait for the child, enforcing the per-step timeout by polling.
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed().as_secs() as i64 >= step.timeout {
                    timed_out = true;
                    kill_group(pid);
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

    // Reap the whole process group so any orphan grandchildren are SIGKILLed now (this also
    // lets the readers finally see EOF on the pipe), then join the readers.
    kill_group(pid);
    for r in readers {
        let _ = r.join();
    }

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

    let (was_aborted, reason) = {
        let mut sh = shared.lock().unwrap();
        sh.running.remove(&tag);
        sh.running_pids.remove(&tag);
        release(&mut sh, &step);
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
                false, // oomed: no boxing in this build
                0,
                timed_out,
                step.timeout,
                false,
            )
        };
        let reason = outcome.reason.clone();
        sh.done.insert(tag.clone(), outcome);
        if !was_aborted && !ok {
            // A REAL failure: mark failed + stop scheduling. Eager-exit (default): reap every
            // step still running so a fast failure doesn't wait for a slow in-flight build.
            sh.failed = true;
            sh.stop = true;
            if !keep_going {
                let others: Vec<(String, u32)> = sh
                    .running_pids
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
                for (other, other_pid) in others {
                    sh.aborted.insert(other);
                    kill_group(other_pid);
                }
            }
        }
        (was_aborted, reason)
    };

    // Emit the terminal status OUTSIDE the lock.
    if was_aborted {
        emit(&format!(
            "[{tag}] \u{2298} ABORT  {} ({dur}s \u{2014} eager-exit after another step failed; keep_going to run all)",
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
/// * `keep_going`: when true, do not eager-exit on the first failure — run every step whose
///   deps still succeed and report all failures.
/// * `verbosity`: 0 quiet (+failures), 1 default (+summaries), `>=2` stream child stdout.
pub fn run_dag(cfg: &DagConfig, jobs: i64, keep_going: bool, verbosity: i64) -> RunResult {
    let runner = Runner::new(cfg, jobs, keep_going, verbosity);
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
}
