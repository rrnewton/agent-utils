//! Suspect-flake reruns (`validator.suspect_flake_reruns`).
//!
//! A single change whose red run failed only on suspect tests (timeouts, or runs that reached
//! no verdict, as the validator shapes its result rows; see
//! [`crate::validate::Ran::suspect_only`]) is held instead of ejected, and those tests run
//! again at the same commit here, beside the run: one thread, one rerun at a time, in the
//! checkout `<state>/scratch/suspect`. The rerun is a plain validation of those tests, never a
//! same-commit rerun composed from the red run. Its verdict is kept as
//! `<state>/suspect/<pr>.json`, where the run that holds the queue reads it at a batch boundary:
//! a pass re-queues the change, a failure ejects it, and a rerun that could not run re-queues it
//! without a verdict, so that its next validation decides.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::model::now;
use crate::plan::short;
use crate::state::{write_json, Config};
use crate::tests::{Selection, TestId};

/// The worker's checkout under `<state>/scratch`.
const CHECKOUT: &str = "suspect";

/// A rerun that passed: the change goes back to the queue.
pub const PASS: &str = "pass";
/// A rerun that failed again: the change is ejected.
pub const FAIL: &str = "fail";
/// A rerun that could not run: the change goes back to the queue, and its next validation
/// decides.
pub const COULD_NOT_RUN: &str = "could-not-run";

/// Set when this process stops: the rerun in progress is cancelled and no other starts.
static STOP: AtomicBool = AtomicBool::new(false);

/// The kept verdict of one suspect-flake rerun.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Verdict {
    /// The change.
    pub pr: i64,
    /// The commit rerun: the red run's candidate.
    pub commit: String,
    /// The tests rerun: the red run's failures.
    pub tests: Vec<String>,
    /// [`PASS`], [`FAIL`] or [`COULD_NOT_RUN`].
    pub outcome: String,
    /// The tests that failed again, for [`FAIL`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
    /// Tests that passed only after a retry inside the rerun.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flaky: Vec<String>,
    /// Why it could not run, or how it ended.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The rerun's log.
    pub log: String,
    /// Wall time.
    pub elapsed_ms: u64,
    /// RFC 3339 UTC time it ended.
    pub finished_at: String,
}

/// Where verdicts and rerun logs are kept.
pub fn dir(state_root: &Path) -> PathBuf {
    state_root.join("suspect")
}

/// Where `pr`'s verdict is kept.
pub fn verdict_path(state_root: &Path, pr: i64) -> PathBuf {
    dir(state_root).join(format!("{pr}.json"))
}

/// The kept verdict of `pr`'s rerun at `commit`, if it has ended.
pub fn kept(state_root: &Path, pr: i64, commit: &str) -> Option<Verdict> {
    let text = std::fs::read_to_string(verdict_path(state_root, pr)).ok()?;
    let verdict: Verdict = serde_json::from_str(&text).ok()?;
    (verdict.commit == commit).then_some(verdict)
}

/// Forget `pr`'s verdict once the run has acted on it.
pub fn forget(state_root: &Path, pr: i64) {
    let _ = std::fs::remove_file(verdict_path(state_root, pr));
}

struct Job {
    state_root: PathBuf,
    config: Config,
    pr: i64,
    commit: String,
    tests: Vec<TestId>,
}

/// Reruns handed to the worker and not ended yet, by change and commit.
static ASKED: Mutex<BTreeSet<(i64, String)>> = Mutex::new(BTreeSet::new());

/// This process's worker: one thread, one rerun at a time, in the order asked.
fn jobs() -> &'static Mutex<mpsc::Sender<Job>> {
    static JOBS: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    JOBS.get_or_init(|| {
        let (jobs, queue) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("suspect-flake".to_owned())
            .spawn(move || work(&queue));
        if let Err(error) = spawned {
            eprintln!("merge-queue: cannot start the suspect-flake worker: {error}");
        }
        Mutex::new(jobs)
    })
}

fn asked() -> std::sync::MutexGuard<'static, BTreeSet<(i64, String)>> {
    ASKED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hand the rerun of `tests` for change `pr` at `commit` to this process's worker, unless it is
/// waiting or running there already, or its verdict is kept.
pub fn ask(state_root: &Path, config: &Config, pr: i64, commit: &str, tests: &[TestId]) {
    if kept(state_root, pr, commit).is_some() || !asked().insert((pr, commit.to_owned())) {
        return;
    }
    let sent = jobs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send(Job {
            state_root: state_root.to_owned(),
            config: config.clone(),
            pr,
            commit: commit.to_owned(),
            tests: tests.to_vec(),
        });
    // No worker took it (it could not start): the hold's expiry re-queues the change.
    if sent.is_err() {
        asked().remove(&(pr, commit.to_owned()));
    }
}

/// Wait until every rerun asked of this process has ended, for at most `limit`: a one-shot run,
/// whose process ends with it.
pub fn wait(limit: Duration) {
    let started = Instant::now();
    while !asked().is_empty() && !STOP.load(Ordering::SeqCst) && started.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the worker before this process exits: the rerun in progress is cancelled, and no other
/// starts.
pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

fn work(queue: &mpsc::Receiver<Job>) {
    while let Ok(job) = queue.recv() {
        if STOP.load(Ordering::SeqCst) || crate::interrupt::requested() {
            return;
        }
        let verdict = rerun(&job);
        // A verdict that cannot be kept leaves the hold to its expiry, which re-queues the
        // change.
        if let Err(error) = write_json(&verdict_path(&job.state_root, job.pr), &verdict) {
            eprintln!(
                "merge-queue: suspect-flake verdict for #{} not kept: {error}",
                job.pr
            );
        }
        asked().remove(&(job.pr, job.commit.clone()));
    }
}

/// Run `job`'s tests at its commit and say what happened.
fn rerun(job: &Job) -> Verdict {
    let started = Instant::now();
    let out = dir(&job.state_root).join(format!("{}-{}", job.pr, short(&job.commit)));
    let mut verdict = Verdict {
        pr: job.pr,
        commit: job.commit.clone(),
        tests: job.tests.iter().map(ToString::to_string).collect(),
        outcome: COULD_NOT_RUN.to_owned(),
        log: out.join("run.log").display().to_string(),
        ..Verdict::default()
    };
    let checkout = job.state_root.join("scratch").join(CHECKOUT);
    let ended = |mut verdict: Verdict| {
        verdict.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        verdict.finished_at = now();
        verdict
    };
    let git = Git::new(&job.config.repo);
    if let Err(error) = crate::precheck::prepare(&git, &checkout, &job.commit) {
        verdict.detail = format!("cannot check out {}: {error}", short(&job.commit));
        return ended(verdict);
    }
    let mut validator = job.config.validator.clone();
    validator.timeout_secs = job.config.validator.suspect_flake_timeout().as_secs();
    let validator = match crate::validate::from_config(&validator) {
        Ok(validator) => validator,
        Err(error) => {
            verdict.detail = format!("no validator: {error}");
            return ended(verdict);
        }
    };
    let selection = Selection::of(job.tests.iter().cloned());
    match validator.run(&checkout, &selection, &out, &STOP) {
        Err(error) => verdict.detail = error,
        Ok(ran) if ran.refused() => {
            verdict.detail = format!("refused (exit {})", exit_text(ran.exit));
        }
        Ok(ran) if ran.cancelled => verdict.detail = "cancelled: the process stopped".to_owned(),
        Ok(ran) if ran.green() => {
            verdict.outcome = PASS.to_owned();
            verdict.flaky = ran.flaky_passes.iter().map(ToString::to_string).collect();
        }
        Ok(ran) => {
            verdict.outcome = FAIL.to_owned();
            let failed = ran.failed();
            verdict.failed = if failed.is_empty() {
                verdict.tests.clone()
            } else {
                failed.iter().map(ToString::to_string).collect()
            };
            verdict.detail = format!("exit {}", exit_text(ran.exit));
        }
    }
    ended(verdict)
}

fn exit_text(exit: Option<i32>) -> String {
    exit.map_or_else(
        || "none: killed at its wall limit".to_owned(),
        |code| code.to_string(),
    )
}
