//! The queue loop: build candidates, validate them concurrently, land green ones in order,
//! search red ones on their failed tests only, eject culprits, and record what every landing was
//! validated with.
//!
//! Invariants:
//! * The base only ever moves to a commit whose exact tree was validated (by the batch's own run,
//!   or by a remainder run after an ejection). Landing re-checks the base first; if it moved, the
//!   batch is rebuilt on the new base and validated again. The per-change `command` driver lands
//!   one change per batch unless `land.allow_batches` explicitly accepts unvalidated
//!   intermediate trees.
//! * A culprit search never runs the whole suite: every probe runs only the failed tests. The
//!   remainder of a batch reruns the failed tests, the tests the batch run did not see pass
//!   (skipped or unreported), and the tests the selection policy says the removed change
//!   affected, within the batch's own selection.
//! * A conclusion that blames a change or the base needs the failure seen twice; a failure that
//!   does not reproduce is recorded as flaky, never blamed.
//! * Every landing records its selection, policy and scope; a focused landing leaves an
//!   obligation that only a green complete run discharges.
//!
//! The window is rolling (a dependent pipeline): up to `window` batches validate at once, each
//! on top of the one before; when the head lands, the next batch starts at once. A red head
//! cancels everything behind it. The window grows by one per landed batch up to `max_depth` and
//! halves per red batch (AIMD), never below one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::git::{Combined, Git};
use crate::model::{now, Discharge, Item, ItemStatus, Obligation, Plan};
use crate::ops::{self, repo};
use crate::plan::short;
use crate::state::{write_json, Config, State};
use crate::tests::{Outcome, Scope, Selection, TestId, ValidationClaim};
use crate::validate::{from_config, Chosen, Ran, Validator, UNATTRIBUTED};

/// How a run behaves.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Build candidates and compute selections, then stop: no validation, no landing, no state
    /// changes.
    pub dry_run: bool,
    /// Validate and search for real, but land on a simulated base instead of the real one, and
    /// change no item or obligation.
    pub shadow: bool,
    /// Override the configured speculative depth (initial window and probe width).
    pub depth: Option<usize>,
    /// Override the configured maximum window (`max_depth`).
    pub max_depth: Option<usize>,
    /// Override the configured maximum batch size.
    pub max_batch: Option<usize>,
    /// Stop starting new batches after this many (0 = drain the plan).
    pub max_batches: usize,
    /// After the queue drains, discharge open obligations with one complete run.
    pub widen: bool,
    /// Do not echo events to stderr.
    pub quiet: bool,
    /// First drain the peer queues whose changes this queue's changes require.
    pub with_peers: bool,
}

/// What a peer queue's run did, when `--with-peers` drained it first.
#[derive(Clone, Debug, Serialize)]
pub struct PeerRun {
    /// Peer name.
    pub peer: String,
    /// The peer's run ID (its report is in the peer's state directory).
    pub run: String,
    /// Changes the peer landed.
    pub landed: Vec<i64>,
    /// Changes the peer ejected.
    pub ejected: Vec<i64>,
    /// Why the peer's run stopped early, if it did.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stopped: String,
}

/// One validated-and-landed (or simulated) batch.
#[derive(Clone, Debug, Serialize)]
pub struct Landed {
    /// Changes landed, in order.
    pub prs: Vec<i64>,
    /// Base commit after landing.
    pub tip: String,
    /// What validated this exact tree.
    pub claim: ValidationClaim,
    /// Validation IDs whose results were used.
    pub validations: Vec<usize>,
    /// Tests that failed once and passed on rerun (never hidden).
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub flaky: BTreeSet<TestId>,
    /// Set when only part of a batch landed (a per-change driver stopped midway).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// One ejected change.
#[derive(Clone, Debug, Serialize)]
pub struct Ejection {
    /// Change number.
    pub pr: i64,
    /// Stable kind: `failed-tests` or `conflict`.
    pub kind: String,
    /// Human explanation.
    pub reason: String,
    /// Failed tests, for `failed-tests`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<TestId>,
    /// Report file.
    pub report: PathBuf,
}

/// One validation the run performed.
#[derive(Clone, Debug, Serialize)]
pub struct ValidationRecord {
    /// Sequence number within the run.
    pub id: usize,
    /// `batch`, `probe`, `remainder` or `widen`.
    pub purpose: String,
    /// Changes on top of the parent, in order.
    pub prs: Vec<i64>,
    /// Commit validated.
    pub commit: String,
    /// Selection run.
    pub selection: Selection,
    /// Policy that chose it.
    pub policy: String,
    /// Whether it was green.
    pub green: bool,
    /// Whether the queue cancelled it (its candidate was discarded).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cancelled: bool,
    /// Failed tests.
    pub failed: Vec<TestId>,
    /// Start time in milliseconds after the run began.
    pub start_ms: u64,
    /// Wall time in milliseconds.
    pub elapsed_ms: u64,
    /// Directory with the log and results.
    pub dir: PathBuf,
}

/// Summary of one run.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RunReport {
    /// Run ID.
    pub run: String,
    /// `run`, `shadow`, `dry-run` or `widen`.
    pub mode: String,
    /// Base commit at start.
    pub base_start: String,
    /// Base commit at end.
    pub base_end: String,
    /// The plan the run started from.
    pub plan: Plan,
    /// Landed batches.
    pub landed: Vec<Landed>,
    /// Ejected changes.
    pub ejected: Vec<Ejection>,
    /// Changes returned to the queue for a later run (dependents of ejected changes).
    pub requeued: Vec<i64>,
    /// Generated files regenerated on candidates.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub regenerated: Vec<Regeneration>,
    /// Peer queues drained first (`--with-peers`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub peer_runs: Vec<PeerRun>,
    /// Batches that would have been validated (dry runs only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub would_validate: Vec<WouldValidate>,
    /// Every validation, in order.
    pub validations: Vec<ValidationRecord>,
    /// Obligations created by focused landings.
    pub obligations: Vec<String>,
    /// Why the run stopped early, if it did.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stopped: String,
    /// Wall time of the run in seconds.
    pub wall_s: f64,
    /// Sum of validation wall times in seconds.
    pub validation_s: f64,
    /// Most validations observed running at the same time.
    pub max_concurrent: usize,
    /// Window size each time a batch started (AIMD trace).
    pub windows: Vec<usize>,
    /// Changes landed per hour of wall time.
    pub prs_per_hour: f64,
}

/// One regeneration of generated files on a candidate.
#[derive(Clone, Debug, Serialize)]
pub struct Regeneration {
    /// The change whose combination needed it.
    pub pr: i64,
    /// Generated paths that conflicted (empty when `regenerate.when` is `always`).
    pub conflicted: Vec<String>,
    /// Generated paths the command rewrote.
    pub rewritten: Vec<String>,
    /// The regeneration commit, or the unchanged tip when nothing was rewritten.
    pub commit: String,
}

/// A batch a dry run would validate.
#[derive(Clone, Debug, Serialize)]
pub struct WouldValidate {
    /// Changes, in order.
    pub prs: Vec<i64>,
    /// Candidate commit.
    pub commit: String,
    /// Selection the policy chose.
    pub selection: Selection,
    /// Policy name.
    pub policy: String,
}

/// A candidate: `parent` plus `prs` combined in order.
#[derive(Clone, Debug)]
struct Candidate {
    prs: Vec<i64>,
    parent: String,
    tip: String,
    /// Commit after each change, aligned with `prs`.
    prefixes: Vec<String>,
}

impl Candidate {
    /// Commit of the prefix with `i` changes (0 = parent).
    fn prefix(&self, i: usize) -> &str {
        if i == 0 {
            &self.parent
        } else {
            &self.prefixes[i - 1]
        }
    }
}

/// Changes that did not combine onto a candidate: number, kind (`conflict` or
/// `regeneration`), and detail.
type Uncombined = Vec<(i64, &'static str, String)>;

/// Outcome of handling one batch.
enum BatchOutcome {
    /// Landed, or nothing left of the batch to land.
    Done,
    /// The base moved under the batch; these changes go back to the front of the queue.
    BaseMoved(Vec<i64>),
    /// Stop the queue; these changes go back to the front of the queue.
    Stop(String, Vec<i64>),
}

/// Worktree slots for concurrent validations, reused between candidates.
struct Slots {
    git: Git,
    dir: PathBuf,
    clean_ignored: bool,
    free: Mutex<Vec<usize>>,
}

impl Slots {
    fn new(git: Git, dir: PathBuf, width: usize, clean_ignored: bool) -> Self {
        Self {
            git,
            dir,
            clean_ignored,
            free: Mutex::new((0..width).rev().collect()),
        }
    }

    fn take(&self) -> usize {
        loop {
            if let Some(slot) = lock(&self.free).pop() {
                return slot;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn give(&self, slot: usize) {
        lock(&self.free).push(slot);
    }

    /// Check out `commit` in slot `slot`, reusing the worktree.
    fn checkout(&self, slot: usize, commit: &str) -> Result<PathBuf, String> {
        let path = self.dir.join(format!("slot-{slot}"));
        if path.join(".git").exists() {
            let wt = Git::new(&path);
            let clean = if self.clean_ignored {
                "-ffdxq"
            } else {
                "-ffdq"
            };
            if wt
                .run(&["checkout", "--quiet", "--detach", "--force", commit])
                .is_ok()
                && wt.run(&["clean", clean]).is_ok()
            {
                return Ok(path);
            }
        }
        self.git.add_worktree(&path, commit)?;
        Ok(path)
    }
}

/// A validation job.
struct Job {
    purpose: &'static str,
    prs: Vec<i64>,
    commit: String,
    selection: Selection,
    policy: String,
}

type JobResult = Result<(usize, Ran), String>;

/// A batch in the rolling window.
struct Flight {
    id: u64,
    candidate: Candidate,
    chosen: Chosen,
    cancel: Arc<AtomicBool>,
    result: Option<JobResult>,
}

/// Observations of one prefix during a culprit search.
#[derive(Clone, Debug, Default)]
struct Obs {
    fails: usize,
    passes: usize,
    failed: BTreeSet<TestId>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Seen {
    Unknown,
    Pass,
    Fail,
}

impl Obs {
    fn seen(&self) -> Seen {
        if self.passes > 0 {
            Seen::Pass
        } else if self.fails > 0 {
            Seen::Fail
        } else {
            Seen::Unknown
        }
    }

    fn confirmed_fail(&self) -> bool {
        self.passes == 0 && self.fails >= 2
    }
}

enum Culprit {
    /// The bare parent fails the focus, twice.
    BaseRed(BTreeSet<TestId>),
    /// The full candidate passed the focus on rerun.
    Flaky(BTreeSet<TestId>),
    /// The change at `index` makes the focus fail (seen twice) and the prefix before it passes.
    Found {
        index: usize,
        failed: BTreeSet<TestId>,
        flaky: BTreeSet<TestId>,
    },
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A queue run in progress. All state is behind locks, so validation threads and the landing
/// loop share one runner.
pub struct Runner<'a> {
    state: &'a State,
    config: Config,
    git: Git,
    validator: Box<dyn Validator>,
    options: RunOptions,
    width: usize,
    max_window: usize,
    run_dir: PathBuf,
    slots: Slots,
    items: Mutex<BTreeMap<i64, Item>>,
    deps: Mutex<BTreeMap<i64, BTreeSet<i64>>>,
    /// Changes ejected or set aside in this run; never rebuilt into a candidate again.
    excluded: Mutex<BTreeSet<i64>>,
    /// Heads of changes landed (or simulated as landed) in this run.
    landed_heads: Mutex<Vec<String>>,
    /// Simulated base for shadow and dry runs.
    simulated: Mutex<Option<String>>,
    base: Mutex<String>,
    next_validation: Mutex<usize>,
    started: Instant,
    report: Mutex<RunReport>,
    events: Mutex<std::fs::File>,
    /// Latest runner handle and validation directory per validated commit, for same-commit
    /// reruns (`MQ_RERUN_FROM`, `MQ_RERUN_OF`).
    handles: Mutex<BTreeMap<String, (String, PathBuf)>>,
    /// Budgeted hosting client, when landed pull requests are to be closed.
    hosting: Option<crate::github::Hosting>,
}

impl<'a> Runner<'a> {
    /// Prepare a run: load config, items and validator, and allocate the run directory.
    pub fn new(state: &'a State, options: RunOptions) -> Result<Self, String> {
        let config = state.config()?;
        let validator = from_config(&config.validator)?;
        let simulate = options.shadow || options.dry_run;
        if !simulate && !matches!(config.land.driver.as_str(), "push" | "command") {
            return Err(format!(
                "land.driver is {:?}; set it to `push` or `command`, or run with --shadow",
                config.land.driver
            ));
        }
        let width = options.depth.unwrap_or(config.depth).max(1);
        let configured_max = options.max_depth.unwrap_or(config.max_depth);
        let max_window = if configured_max == 0 {
            width
        } else {
            configured_max.max(width)
        };
        let git = repo(state, &config)?;
        let config_close_prs = config.land.close_prs && !simulate;
        let config_github = config.github.clone();
        let (run_id, run_dir) = state.new_run()?;
        let events = std::fs::File::create(run_dir.join("events.jsonl"))
            .map_err(|error| format!("cannot create events log: {error}"))?;
        let items = state
            .items()?
            .into_iter()
            .map(|item| (item.pr, item))
            .collect();
        // One slot per concurrent validation, plus one for selection queries.
        let slots = Slots::new(
            git.clone(),
            state.scratch_dir(),
            max_window + 1,
            config.validator.clean_ignored,
        );
        let mode = if options.dry_run {
            "dry-run"
        } else if options.shadow {
            "shadow"
        } else {
            "run"
        };
        Ok(Self {
            state,
            config,
            git,
            validator,
            width,
            max_window,
            run_dir,
            slots,
            items: Mutex::new(items),
            deps: Mutex::new(BTreeMap::new()),
            excluded: Mutex::new(BTreeSet::new()),
            landed_heads: Mutex::new(Vec::new()),
            simulated: Mutex::new(None),
            base: Mutex::new(String::new()),
            next_validation: Mutex::new(0),
            started: Instant::now(),
            report: Mutex::new(RunReport {
                run: run_id,
                mode: mode.to_owned(),
                ..RunReport::default()
            }),
            events: Mutex::new(events),
            handles: Mutex::new(BTreeMap::new()),
            hosting: if config_close_prs {
                Some(crate::github::Hosting::new(&config_github, &state.root)?)
            } else {
                None
            },
            options,
        })
    }

    fn simulating(&self) -> bool {
        self.options.shadow || self.options.dry_run
    }

    fn event(&self, kind: &str, detail: &str) {
        let line = serde_json::json!({"at": now(), "kind": kind, "detail": detail});
        {
            use std::io::Write;
            let _ = writeln!(lock(&self.events), "{line}");
        }
        if !self.options.quiet {
            eprintln!("merge-queue: {kind}: {detail}");
        }
    }

    /// The real base (fetched), or the simulated one in shadow and dry runs.
    fn current_base(&self) -> Result<String, String> {
        if self.simulating() {
            if let Some(simulated) = lock(&self.simulated).clone() {
                return Ok(simulated);
            }
        }
        let (base, _) = ops::refresh(&self.git, &self.config, &[])?;
        Ok(base)
    }

    fn requeue(&self, prs: &[i64], why: &str) {
        let prs: Vec<i64> = {
            let mut excluded = lock(&self.excluded);
            prs.iter()
                .copied()
                .filter(|pr| excluded.insert(*pr))
                .collect()
        };
        if prs.is_empty() {
            return;
        }
        lock(&self.report).requeued.extend(prs.iter().copied());
        self.event("requeue", &format!("{}: {why}", list(&prs)));
    }

    /// Set aside every change depending (transitively) on `pr` and drop it from `pending`.
    fn drop_dependents(&self, pr: i64, pending: &mut VecDeque<Vec<i64>>) {
        let deps = lock(&self.deps).clone();
        let mut dropped = BTreeSet::from([pr]);
        loop {
            let more: Vec<i64> = deps
                .iter()
                .filter(|(after, befores)| {
                    !dropped.contains(*after) && !befores.is_disjoint(&dropped)
                })
                .map(|(after, _)| *after)
                .collect();
            if more.is_empty() {
                break;
            }
            dropped.extend(more);
        }
        dropped.remove(&pr);
        for batch in pending.iter_mut() {
            batch.retain(|p| !dropped.contains(p));
        }
        pending.retain(|batch| !batch.is_empty());
        let dropped: Vec<i64> = dropped.into_iter().collect();
        self.requeue(&dropped, &format!("depend on ejected #{pr}"));
    }

    /// Combine `prs` onto `parent`. Excluded changes are skipped; changes whose dependencies
    /// were skipped are set aside; changes that do not combine are returned for ejection.
    fn build(&self, parent: &str, prs: &[i64]) -> Result<(Candidate, Uncombined), String> {
        let items = lock(&self.items).clone();
        let deps = lock(&self.deps).clone();
        let mut skipped = lock(&self.excluded).clone();
        let mut exclude_heads: Vec<String> = items
            .values()
            .filter(|item| item.status == ItemStatus::Landed)
            .map(|item| item.head.clone())
            .collect();
        exclude_heads.extend(lock(&self.landed_heads).iter().cloned());
        let mut tip = parent.to_owned();
        let mut kept: Vec<i64> = Vec::new();
        let mut prefixes = Vec::new();
        let mut conflicts = Vec::new();
        let mut held = Vec::new();
        let regen = self.config.regenerate.clone();
        for pr in prs {
            if skipped.contains(pr) {
                continue;
            }
            if deps
                .get(pr)
                .is_some_and(|befores| !befores.is_disjoint(&skipped))
            {
                skipped.insert(*pr);
                held.push(*pr);
                continue;
            }
            let item = &items[pr];
            if item.exact && !self.git.fast_forwards(&tip, &item.head)? {
                skipped.insert(*pr);
                conflicts.push((*pr, "not-fast-forward", short(&tip).to_owned()));
                continue;
            }
            let mut exclude: Vec<&str> = kept.iter().map(|p| items[p].head.as_str()).collect();
            exclude.extend(exclude_heads.iter().map(String::as_str));
            let message = format!("Merge #{pr} ({})", item.head_ref);
            let combined = self.git.combine_with(
                self.config.combine,
                &tip,
                &item.head,
                &exclude,
                &message,
                &|path| regen.covers(path),
            )?;
            let (next, conflicted) = match combined {
                Combined::Clean(next) => (next, Vec::new()),
                Combined::NeedsRegen(next, paths) => (next, paths),
                Combined::Conflict(paths) => {
                    skipped.insert(*pr);
                    conflicts.push((*pr, "conflict", paths.join(", ")));
                    continue;
                }
            };
            let next = if !conflicted.is_empty() || (regen.enabled() && regen.always()) {
                match self.regenerate(&next, *pr, &conflicted)? {
                    Ok(regenerated) => regenerated,
                    Err(why) => {
                        skipped.insert(*pr);
                        conflicts.push((*pr, "regeneration", why));
                        continue;
                    }
                }
            } else {
                next
            };
            tip = next;
            kept.push(*pr);
            prefixes.push(tip.clone());
        }
        self.requeue(&held, "a dependency did not combine");
        Ok((
            Candidate {
                prs: kept,
                parent: parent.to_owned(),
                tip,
                prefixes,
            },
            conflicts,
        ))
    }

    /// Rewrite the generated files on `tip` (which just combined `pr`) with the configured
    /// command and commit the result. `Ok(Err(why))` means this change cannot be regenerated.
    fn regenerate(
        &self,
        tip: &str,
        pr: i64,
        conflicted: &[String],
    ) -> Result<Result<String, String>, String> {
        let regen = &self.config.regenerate;
        if !regen.enabled() {
            return Ok(Err(
                "generated files conflict but no regeneration is configured".into(),
            ));
        }
        let slot = self.slots.take();
        let result = (|| -> Result<Result<String, String>, String> {
            let checkout = self.slots.checkout(slot, tip)?;
            let output = Command::new("sh")
                .arg("-c")
                .arg(&regen.command)
                .current_dir(&checkout)
                .env("MQ_PR", pr.to_string())
                .env("MQ_REGEN_PATHS", regen.paths.join(" "))
                .env("MQ_CONFLICTED", conflicted.join(" "))
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|error| format!("cannot run regenerate.command: {error}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
                return Ok(Err(format!(
                    "regenerate.command failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                )));
            }
            let mut wt = Git::new(&checkout);
            wt.identity = self.git.identity.clone();
            let mut changed: Vec<String> = wt
                .run(&["diff", "--name-only", "--no-renames", "HEAD"])?
                .lines()
                .chain(
                    wt.run(&["ls-files", "--others", "--exclude-standard"])?
                        .lines(),
                )
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            changed.sort();
            changed.dedup();
            let stray: Vec<&String> = changed.iter().filter(|path| !regen.covers(path)).collect();
            if !stray.is_empty() {
                return Ok(Err(format!(
                    "regenerate.command changed files that are not generated: {}",
                    stray
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            for path in conflicted.iter().chain(&changed) {
                let text = std::fs::read_to_string(checkout.join(path)).unwrap_or_default();
                if text
                    .lines()
                    .any(|line| line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> "))
                {
                    return Ok(Err(format!(
                        "conflict markers remain in {path} after regeneration"
                    )));
                }
            }
            let commit = if changed.is_empty() {
                tip.to_owned()
            } else {
                let mut add = vec!["add", "-A", "--"];
                add.extend(changed.iter().map(String::as_str));
                wt.run(&add)?;
                let message = format!("merge-queue: regenerate {} after #{pr}", changed.join(", "));
                let mut command = Command::new("git");
                command.arg("-C").arg(&checkout).args([
                    "commit",
                    "-q",
                    "--no-verify",
                    "-m",
                    &message,
                ]);
                if let Some((name, email)) = &self.git.identity {
                    command
                        .env("GIT_AUTHOR_NAME", name)
                        .env("GIT_AUTHOR_EMAIL", email)
                        .env("GIT_COMMITTER_NAME", name)
                        .env("GIT_COMMITTER_EMAIL", email);
                }
                let committed = command
                    .output()
                    .map_err(|error| format!("cannot commit regenerated files: {error}"))?;
                if !committed.status.success() {
                    return Err(format!(
                        "cannot commit regenerated files: {}",
                        String::from_utf8_lossy(&committed.stderr).trim()
                    ));
                }
                wt.rev_parse("HEAD")?
            };
            self.event(
                "regenerate",
                &format!(
                    "#{pr}: {} at {}{}",
                    if changed.is_empty() {
                        "generated files already current".to_owned()
                    } else {
                        format!("rewrote {}", changed.join(", "))
                    },
                    short(&commit),
                    if conflicted.is_empty() {
                        String::new()
                    } else {
                        format!(" (conflicted: {})", conflicted.join(", "))
                    }
                ),
            );
            lock(&self.report).regenerated.push(Regeneration {
                pr,
                conflicted: conflicted.to_vec(),
                rewritten: changed,
                commit: commit.clone(),
            });
            Ok(Ok(commit))
        })();
        self.slots.give(slot);
        result
    }

    /// Whether green results recorded at exactly `commit` cover `selection`: every selected test
    /// passed in the latest run at that commit that selected it, and an `all` selection needs a
    /// complete run there whose failures were all rerun green there.
    fn covered(&self, commit: &str, selection: &Selection) -> bool {
        let report = lock(&self.report);
        let runs = report
            .validations
            .iter()
            .filter(|v| v.commit == commit && !v.cancelled);
        let mut complete = false;
        let mut failing: BTreeSet<TestId> = BTreeSet::new();
        let mut passed: BTreeSet<TestId> = BTreeSet::new();
        for run in runs {
            let unattributed = run.failed.iter().any(|t| t.step == UNATTRIBUTED);
            if run.selection.is_all() {
                complete = true;
                failing = run.failed.iter().cloned().collect();
                passed.clear();
                continue;
            }
            for test in run.selection.tests().into_iter().flatten() {
                if unattributed
                    || run
                        .failed
                        .iter()
                        .any(|f| f == test || f.step == test.step && test.case.is_none())
                {
                    failing.insert(test.clone());
                    passed.remove(test);
                } else {
                    failing
                        .retain(|f| !(f == test || (test.case.is_none() && f.step == test.step)));
                    passed.insert(test.clone());
                }
            }
        }
        match selection {
            Selection::All => complete && failing.is_empty(),
            Selection::Tests(tests) => tests.iter().all(|test| {
                passed
                    .iter()
                    .any(|p| p == test || (p.case.is_none() && p.step == test.step))
                    || (complete && !failing.iter().any(|f| f.step == test.step))
            }),
        }
    }

    /// Build, then eject whatever did not combine (and set its dependents aside).
    fn build_and_eject(
        &self,
        parent: &str,
        prs: &[i64],
        pending: &mut VecDeque<Vec<i64>>,
        context: &str,
    ) -> Result<Candidate, String> {
        let (candidate, rejected) = self.build(parent, prs)?;
        for (pr, kind, detail) in rejected {
            let reason = match kind {
                "conflict" => format!(
                    "does not combine onto {}{context}: conflicts on {detail}; rebase and resubmit",
                    short(parent)
                ),
                "not-fast-forward" => format!(
                    "must land as its own commits (another repository requires its exact head), but it is not on top of {detail}{context}; rebase it and update what requires it"
                ),
                _ => format!(
                    "generated files could not be regenerated onto {}{context}: {detail}",
                    short(parent)
                ),
            };
            self.eject(pr, kind, &reason, &[])?;
            self.drop_dependents(pr, pending);
        }
        Ok(candidate)
    }

    /// Run validations concurrently and wait for all of them.
    fn validate_all(&self, jobs: Vec<Job>) -> Vec<JobResult> {
        let never = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .into_iter()
                .map(|job| {
                    let never = &never;
                    scope.spawn(move || self.validate_one(job, never))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("validation thread panicked".to_owned()))
                })
                .collect()
        })
    }

    fn validate_one(&self, job: Job, cancel: &AtomicBool) -> JobResult {
        let id = {
            let mut next = lock(&self.next_validation);
            *next += 1;
            *next
        };
        let dir = self
            .run_dir
            .join("validations")
            .join(format!("{id:04}-{}", job.purpose));
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        self.event(
            "validate",
            &format!(
                "v{id} {} {} on {}: {} ({})",
                job.purpose,
                list(&job.prs),
                short(&job.commit),
                job.selection.summary(),
                job.policy
            ),
        );
        // A rerun at a commit already validated gets that run's handle, so a runner that can
        // rerun only the failed parts of an earlier run (at the same commit) can do so.
        let extra: Vec<(String, String)> = lock(&self.handles)
            .get(&job.commit)
            .map(|(handle, previous)| {
                vec![
                    ("MQ_RERUN_FROM".to_owned(), handle.clone()),
                    ("MQ_RERUN_OF".to_owned(), previous.display().to_string()),
                ]
            })
            .unwrap_or_default();
        let slot = self.slots.take();
        let start_ms = self.started.elapsed().as_millis() as u64;
        let result = self.slots.checkout(slot, &job.commit).and_then(|checkout| {
            self.validator
                .run_with(&checkout, &job.selection, &dir, cancel, &extra)
        });
        self.slots.give(slot);
        let ran = result?;
        if let Some(handle) = &ran.handle {
            lock(&self.handles).insert(job.commit.clone(), (handle.clone(), dir.clone()));
        }
        let record = ValidationRecord {
            id,
            purpose: job.purpose.to_owned(),
            prs: job.prs,
            commit: job.commit,
            selection: job.selection,
            policy: job.policy,
            green: ran.green(),
            cancelled: ran.cancelled,
            failed: if ran.cancelled {
                Vec::new()
            } else {
                ran.failed().into_iter().collect()
            },
            start_ms,
            elapsed_ms: ran.elapsed.as_millis() as u64,
            dir: dir.clone(),
        };
        write_json(&dir.join("record.json"), &record)?;
        self.event(
            "result",
            &format!(
                "v{id} {} in {:.1}s{}",
                if record.cancelled {
                    "cancelled"
                } else if record.green {
                    "green"
                } else {
                    "red"
                },
                ran.elapsed.as_secs_f64(),
                if record.failed.is_empty() {
                    String::new()
                } else {
                    format!(", failed: {}", ids(&record.failed))
                }
            ),
        );
        let mut report = lock(&self.report);
        report.validation_s += ran.elapsed.as_secs_f64();
        report.validations.push(record);
        Ok((id, ran))
    }

    /// The policy's selection for `commit` relative to `base`.
    fn select(&self, commit: &str, base: &str) -> Result<Chosen, String> {
        let slot = self.slots.take();
        let result = self
            .slots
            .checkout(slot, commit)
            .and_then(|checkout| self.validator.select(&checkout, base, commit));
        self.slots.give(slot);
        result
    }

    /// Execute the run. Errors after the run started stop it and are recorded in the report.
    pub fn run(self) -> Result<RunReport, String> {
        if self.options.with_peers {
            if let Err(error) = self.run_peers() {
                self.event("peers", &format!("could not drain peers: {error}"));
            }
        }
        let mut stop = match self.drive() {
            Ok(stop) => stop,
            Err(error) => format!("error: {error}"),
        };
        if !stop.is_empty() {
            self.event("stopped", &stop);
        }
        if self.options.widen && stop.is_empty() {
            if self.simulating() {
                self.event(
                    "widen",
                    "skipped: obligations are real state and this run is simulated",
                );
            } else if let Err(error) = self.widen() {
                stop = format!("widen failed: {error}");
            }
        }
        self.finish(stop)
    }

    /// Drain every peer queue that holds a change some queued change here requires.
    fn run_peers(&self) -> Result<(), String> {
        let items: Vec<Item> = lock(&self.items).values().cloned().collect();
        let holds = ops::peer_holds(&self.config, &items)?;
        let needed: BTreeSet<String> = items
            .iter()
            .filter(|item| holds.contains_key(&item.pr))
            .flat_map(|item| item.requires.iter().map(|r| r.peer.clone()))
            .collect();
        for name in needed {
            let peer = ops::peer(&self.config, &name)?;
            let _lock = peer.state.lock()?;
            self.event("peers", &format!("draining {name} first"));
            let options = RunOptions {
                widen: false,
                with_peers: false,
                ..self.options.clone()
            };
            let report = Runner::new(&peer.state, options)?.run()?;
            lock(&self.report).peer_runs.push(PeerRun {
                peer: name.clone(),
                run: report.run.clone(),
                landed: report.landed.iter().flat_map(|l| l.prs.clone()).collect(),
                ejected: report.ejected.iter().map(|e| e.pr).collect(),
                stopped: report.stopped.clone(),
            });
        }
        Ok(())
    }

    fn finish(&self, stop: String) -> Result<RunReport, String> {
        let report = {
            let mut report = lock(&self.report).clone();
            report.stopped = stop;
            report.base_end = lock(&self.base).clone();
            report.wall_s = self.started.elapsed().as_secs_f64();
            report.max_concurrent = max_concurrent(&report.validations);
            let landed: usize = report.landed.iter().map(|landed| landed.prs.len()).sum();
            report.prs_per_hour = if report.wall_s > 0.0 {
                landed as f64 * 3600.0 / report.wall_s
            } else {
                0.0
            };
            report
        };
        write_json(&self.run_dir.join("report.json"), &report)?;
        std::fs::write(self.run_dir.join("report.md"), render_report(&report))
            .map_err(|error| format!("cannot write report: {error}"))?;
        Ok(report)
    }

    /// Plan, then drain. Returns the reason the run stopped early, or an empty string.
    fn drive(&self) -> Result<String, String> {
        let mut max_batch = self.options.max_batch.unwrap_or(self.config.max_batch);
        if self.config.land.driver == "command" && !self.config.land.allow_batches {
            max_batch = 1;
        }
        let plan = ops::plan(self.state, &self.config, max_batch)?;
        {
            let mut deps = lock(&self.deps);
            for (before, after) in &plan.dependencies {
                deps.entry(*after).or_default().insert(*before);
            }
        }
        *lock(&self.base) = plan.base_sha.clone();
        if self.simulating() {
            *lock(&self.simulated) = Some(plan.base_sha.clone());
        }
        write_json(&self.run_dir.join("plan.json"), &plan)?;
        self.event(
            "plan",
            &format!(
                "{} batch(es), {} held, base {}",
                plan.batches.len(),
                plan.held.len(),
                short(&plan.base_sha)
            ),
        );
        {
            let mut report = lock(&self.report);
            report.base_start = plan.base_sha.clone();
            report.plan = plan.clone();
        }
        let mut pending: VecDeque<Vec<i64>> =
            plan.batches.iter().map(|batch| batch.prs.clone()).collect();
        if self.options.dry_run {
            return self.dry_run(&mut pending);
        }
        self.pipeline(&mut pending)
    }

    fn dry_run(&self, pending: &mut VecDeque<Vec<i64>>) -> Result<String, String> {
        let mut started = 0;
        while let Some(prs) = pending.pop_front() {
            if self.options.max_batches > 0 && started >= self.options.max_batches {
                break;
            }
            let parent = self.current_base()?;
            let candidate = self.build_and_eject(&parent, &prs, pending, "")?;
            if candidate.prs.is_empty() {
                continue;
            }
            started += 1;
            let chosen = self.select(&candidate.tip, &candidate.parent)?;
            self.event(
                "dry-run",
                &format!(
                    "would validate {} at {} with {} ({}), then land",
                    list(&candidate.prs),
                    short(&candidate.tip),
                    chosen.selection.summary(),
                    chosen.policy
                ),
            );
            lock(&self.report).would_validate.push(WouldValidate {
                prs: candidate.prs.clone(),
                commit: candidate.tip.clone(),
                selection: chosen.selection,
                policy: chosen.policy,
            });
            *lock(&self.simulated) = Some(candidate.tip.clone());
            *lock(&self.base) = candidate.tip;
        }
        Ok(String::new())
    }

    /// The rolling speculative window.
    fn pipeline(&self, pending: &mut VecDeque<Vec<i64>>) -> Result<String, String> {
        let mut window = self.width.min(self.max_window);
        let mut flights: VecDeque<Flight> = VecDeque::new();
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel::<(u64, JobResult)>();
            let mut next_id = 0u64;
            let mut started = 0usize;
            let result = (|| -> Result<String, String> {
                loop {
                    // Fill the window, each new batch on top of the last one in flight.
                    while flights.len() < window
                        && !pending.is_empty()
                        && (self.options.max_batches == 0 || started < self.options.max_batches)
                    {
                        let parent = match flights.back() {
                            Some(flight) => flight.candidate.tip.clone(),
                            None => {
                                let base = self.current_base()?;
                                *lock(&self.base) = base.clone();
                                base
                            }
                        };
                        let prs = pending.pop_front().unwrap_or_default();
                        let candidate = self.build_and_eject(&parent, &prs, pending, "")?;
                        if candidate.prs.is_empty() {
                            continue;
                        }
                        let chosen = self.select(&candidate.tip, &candidate.parent)?;
                        let cancel = Arc::new(AtomicBool::new(false));
                        let job = Job {
                            purpose: "batch",
                            prs: candidate.prs.clone(),
                            commit: candidate.tip.clone(),
                            selection: chosen.selection.clone(),
                            policy: chosen.policy.clone(),
                        };
                        let id = next_id;
                        next_id += 1;
                        let tx = tx.clone();
                        let flag = Arc::clone(&cancel);
                        scope.spawn(move || {
                            let _ = tx.send((id, self.validate_one(job, &flag)));
                        });
                        flights.push_back(Flight {
                            id,
                            candidate,
                            chosen,
                            cancel,
                            result: None,
                        });
                        started += 1;
                        lock(&self.report).windows.push(window);
                    }
                    if flights.is_empty() {
                        return Ok(String::new());
                    }
                    let (id, result) = rx
                        .recv()
                        .map_err(|_| "validation channel closed".to_owned())?;
                    if let Some(flight) = flights.iter_mut().find(|flight| flight.id == id) {
                        flight.result = Some(result);
                    }
                    // Land (or search) heads that have finished, in order.
                    while flights
                        .front()
                        .is_some_and(|flight| flight.result.is_some())
                    {
                        let Some(head) = flights.pop_front() else {
                            break;
                        };
                        let (vid, ran) = head
                            .result
                            .unwrap_or_else(|| Err("missing result".to_owned()))?;
                        let outcome = if ran.green() {
                            let claim = ValidationClaim::new(
                                head.chosen.selection.clone(),
                                head.chosen.policy.clone(),
                                head.candidate.tip.clone(),
                            );
                            let outcome =
                                self.land(&head.candidate, claim, vec![vid], BTreeSet::new())?;
                            if matches!(outcome, BatchOutcome::Done) {
                                window = (window + 1).min(self.max_window);
                            }
                            outcome
                        } else {
                            // Everything behind a red batch was built on its tree.
                            discard(&mut flights, &rx, pending);
                            window = (window / 2).max(1);
                            self.handle_red(&head.candidate, &head.chosen, &ran, vid, pending)?
                        };
                        match outcome {
                            BatchOutcome::Done => {}
                            BatchOutcome::BaseMoved(prs) => {
                                self.event(
                                    "base-moved",
                                    &format!("rebuilding {} on the new base", list(&prs)),
                                );
                                discard(&mut flights, &rx, pending);
                                pending.push_front(prs);
                            }
                            BatchOutcome::Stop(reason, prs) => {
                                discard(&mut flights, &rx, pending);
                                if !prs.is_empty() {
                                    pending.push_front(prs);
                                }
                                return Ok(reason);
                            }
                        }
                    }
                }
            })();
            if result.is_err() {
                discard(&mut flights, &rx, pending);
            }
            result
        })
    }

    fn eject(&self, pr: i64, kind: &str, reason: &str, failed: &[TestId]) -> Result<(), String> {
        lock(&self.excluded).insert(pr);
        let dir = self.run_dir.join("ejections");
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let report = dir.join(format!("{pr}.md"));
        let mut text = format!("# #{pr} ejected ({kind})\n\n{reason}\n");
        if !failed.is_empty() {
            text.push_str("\nFailed tests:\n");
            for test in failed {
                text.push_str(&format!("- `{test}`\n"));
            }
        }
        text.push_str(
            "\nValidation logs are under the run's `validations/` directory; `events.jsonl` has the sequence.\n",
        );
        std::fs::write(&report, text)
            .map_err(|error| format!("cannot write {}: {error}", report.display()))?;
        self.event("eject", &format!("#{pr} ({kind}): {reason}"));
        if !self.simulating() {
            let mut items = lock(&self.items);
            if let Some(item) = items.get_mut(&pr) {
                item.status = ItemStatus::Ejected;
                item.log(
                    "ejected",
                    format!("{kind}: {reason} (report {})", report.display()),
                );
                self.state.save_item(item)?;
            }
        }
        lock(&self.report).ejected.push(Ejection {
            pr,
            kind: kind.to_owned(),
            reason: reason.to_owned(),
            failed: failed.to_vec(),
            report,
        });
        Ok(())
    }

    /// Record `prs` as landed at `tip` with `claim` (status, obligation, report).
    fn record_landed(
        &self,
        prs: &[i64],
        tip: &str,
        claim: &ValidationClaim,
        validations: Vec<usize>,
        flaky: BTreeSet<TestId>,
        note: &str,
    ) -> Result<(), String> {
        *lock(&self.base) = tip.to_owned();
        {
            let items = lock(&self.items);
            lock(&self.landed_heads).extend(
                prs.iter()
                    .filter_map(|pr| items.get(pr))
                    .map(|i| i.head.clone()),
            );
        }
        let suffix = if note.is_empty() {
            String::new()
        } else {
            format!("; {note}")
        };
        self.event(
            if self.simulating() {
                "land-simulated"
            } else {
                "landed"
            },
            &format!(
                "{} at {} (validated with {}, {}, policy {}){suffix}",
                list(prs),
                short(tip),
                claim.selection.summary(),
                claim.scope.as_str(),
                claim.policy
            ),
        );
        let run_id = lock(&self.report).run.clone();
        if !self.simulating() {
            {
                let mut items = lock(&self.items);
                for pr in prs {
                    if let Some(item) = items.get_mut(pr) {
                        item.status = ItemStatus::Landed;
                        item.landed_as = tip.to_owned();
                        if let Some(hosting) = &self.hosting {
                            match hosting.close(*pr, &crate::sync::close_comment(tip)) {
                                Ok(()) => item.log("closed", "pull request closed after landing"),
                                Err(error) => {
                                    item.close_pending = true;
                                    item.log("close-postponed", error);
                                }
                            }
                        }
                        item.log(
                            "landed",
                            format!(
                                "in {} at {tip}; validated with {} ({}, policy {}){suffix}",
                                list(prs),
                                claim.selection.summary(),
                                claim.scope.as_str(),
                                claim.policy
                            ),
                        );
                        self.state.save_item(item)?;
                    }
                }
            }
            if claim.scope == Scope::Focused {
                let index = lock(&self.report).landed.len();
                let obligation = Obligation {
                    id: format!("{run_id}-{index}"),
                    created_at: now(),
                    landed: tip.to_owned(),
                    prs: prs.to_vec(),
                    validated: claim.clone(),
                    discharged: None,
                    failed_runs: Vec::new(),
                };
                self.state.save_obligation(&obligation)?;
                lock(&self.report).obligations.push(obligation.id);
            }
        }
        lock(&self.report).landed.push(Landed {
            prs: prs.to_vec(),
            tip: tip.to_owned(),
            claim: claim.clone(),
            validations,
            flaky,
            note: note.to_owned(),
        });
        Ok(())
    }

    /// Land a validated candidate through the configured driver (or the simulated base).
    fn land(
        &self,
        candidate: &Candidate,
        claim: ValidationClaim,
        validations: Vec<usize>,
        flaky: BTreeSet<TestId>,
    ) -> Result<BatchOutcome, String> {
        let requiring: Vec<Item> = {
            let items = lock(&self.items);
            candidate
                .prs
                .iter()
                .filter_map(|pr| items.get(pr))
                .filter(|item| !item.requires.is_empty())
                .cloned()
                .collect()
        };
        if !requiring.is_empty() {
            let unmet = ops::peer_holds(&self.config, &requiring)?;
            if let Some(held) = unmet.values().next() {
                return Ok(BatchOutcome::Stop(
                    format!(
                        "#{}: {} (checked again before landing)",
                        held.pr, held.reason
                    ),
                    candidate.prs.clone(),
                ));
            }
        }
        if self.config.exact_sha && !self.covered(&candidate.tip, &claim.selection) {
            return Ok(BatchOutcome::Stop(
                format!(
                    "exact-commit rule: {} has no green coverage of {} recorded at that exact commit; refusing to land",
                    short(&candidate.tip),
                    claim.selection.summary()
                ),
                candidate.prs.clone(),
            ));
        }
        if self.simulating() {
            {
                let mut simulated = lock(&self.simulated);
                if simulated.as_deref() != Some(candidate.parent.as_str()) {
                    return Ok(BatchOutcome::BaseMoved(candidate.prs.clone()));
                }
                *simulated = Some(candidate.tip.clone());
            }
            self.record_landed(
                &candidate.prs,
                &candidate.tip,
                &claim,
                validations,
                flaky,
                "",
            )?;
            return Ok(BatchOutcome::Done);
        }
        let base_ref = format!("refs/heads/{}", self.config.base);
        match self.config.land.driver.as_str() {
            "push" => {
                let pushed = if self.config.remote.is_empty() {
                    self.git
                        .run(&["update-ref", &base_ref, &candidate.tip, &candidate.parent])
                        .map(drop)
                } else {
                    let spec = format!("{}:{base_ref}", candidate.tip);
                    self.git
                        .run_net(&["push", "--quiet", &self.config.remote, &spec])
                        .map(drop)
                };
                let now = self.current_base()?;
                if let Err(error) = pushed {
                    if now != candidate.parent {
                        return Ok(BatchOutcome::BaseMoved(candidate.prs.clone()));
                    }
                    return Ok(BatchOutcome::Stop(
                        format!("push failed: {error}"),
                        candidate.prs.clone(),
                    ));
                }
                if now != candidate.tip {
                    return Ok(BatchOutcome::Stop(
                        format!(
                            "pushed {} but the base is now {}; check the base by hand",
                            short(&candidate.tip),
                            short(&now)
                        ),
                        Vec::new(),
                    ));
                }
                self.record_landed(&candidate.prs, &now, &claim, validations, flaky, "")?;
                Ok(BatchOutcome::Done)
            }
            "command" => self.land_by_command(candidate, &claim, validations, flaky),
            other => Err(format!("unknown land.driver {other:?}")),
        }
    }

    /// The per-change driver: run `land.command` once per change and check the resulting tree.
    fn land_by_command(
        &self,
        candidate: &Candidate,
        claim: &ValidationClaim,
        validations: Vec<usize>,
        flaky: BTreeSet<TestId>,
    ) -> Result<BatchOutcome, String> {
        // A per-change driver (such as a hosted rebase merge) rewrites commit IDs, so an
        // unchanged base is recognised by its tree, not its commit.
        let now = self.current_base()?;
        if now != candidate.parent
            && self.git.tree_of(&now)? != self.git.tree_of(&candidate.parent)?
        {
            return Ok(BatchOutcome::BaseMoved(candidate.prs.clone()));
        }
        let items = lock(&self.items).clone();
        let mut tip = now;
        let partial_note = "partial batch: the driver stopped before the rest; this intermediate tree was not validated on its own";
        for (i, pr) in candidate.prs.iter().enumerate() {
            let item = &items[pr];
            let prefix = &candidate.prefixes[i];
            let expected_tree = self.git.tree_of(prefix)?;
            let status = Command::new("sh")
                .arg("-c")
                .arg(&self.config.land.command)
                .current_dir(&self.config.repo)
                .env("MQ_PR", pr.to_string())
                .env("MQ_HEAD_REF", &item.head_ref)
                .env("MQ_HEAD", &item.head)
                .env("MQ_PREFIX", prefix)
                .env("MQ_EXPECTED_TREE", &expected_tree)
                .env("MQ_PARENT", &tip)
                .env("MQ_BASE", &self.config.base)
                .env("MQ_REMOTE", &self.config.remote)
                .status()
                .map_err(|error| format!("cannot run land.command: {error}"))?;
            let landed_so_far = &candidate.prs[..i];
            if !status.success() {
                if !landed_so_far.is_empty() {
                    self.record_landed(
                        landed_so_far,
                        &tip,
                        claim,
                        validations,
                        flaky,
                        partial_note,
                    )?;
                }
                return Ok(BatchOutcome::Stop(
                    format!(
                        "land.command failed for #{pr} (exit {})",
                        status.code().unwrap_or(-1)
                    ),
                    candidate.prs[i..].to_vec(),
                ));
            }
            let after = self.current_base()?;
            let tree = self.git.tree_of(&after)?;
            if tree != expected_tree {
                if !landed_so_far.is_empty() {
                    self.record_landed(
                        landed_so_far,
                        &tip,
                        claim,
                        validations,
                        flaky,
                        partial_note,
                    )?;
                }
                return Ok(BatchOutcome::Stop(
                    format!(
                        "after landing #{pr} the base tree {} differs from the validated tree {}; check the base by hand",
                        short(&tree),
                        short(&expected_tree)
                    ),
                    Vec::new(),
                ));
            }
            tip = after;
        }
        self.record_landed(&candidate.prs, &tip, claim, validations, flaky, "")?;
        Ok(BatchOutcome::Done)
    }

    /// A red batch: find culprits with failed-test-only probes, eject them, and land the rest.
    fn handle_red(
        &self,
        candidate: &Candidate,
        chosen: &Chosen,
        ran: &Ran,
        vid: usize,
        pending: &mut VecDeque<Vec<i64>>,
    ) -> Result<BatchOutcome, String> {
        let mut seen_failed = ran.failed();
        // Without per-test attribution the only focus available is the batch's own selection.
        let mut focus = if seen_failed.contains(&TestId::step(UNATTRIBUTED)) {
            chosen.selection.clone()
        } else {
            Selection::of(seen_failed.iter().cloned())
        };
        // Tests the batch run did not see pass cannot be carried forward to a remainder.
        let mut unproven: BTreeSet<TestId> = not_passed(ran);
        let mut current = candidate.clone();
        let mut validations = vec![vid];
        let mut flaky = BTreeSet::new();
        let mut carried = false;
        loop {
            match self.find_culprit(&current, &focus, &seen_failed)? {
                Culprit::BaseRed(tests) => {
                    return Ok(BatchOutcome::Stop(
                        format!(
                            "the base {} itself fails {} (seen twice); fix the base first",
                            short(&current.parent),
                            ids(&tests.into_iter().collect::<Vec<_>>())
                        ),
                        current.prs.clone(),
                    ));
                }
                Culprit::Flaky(tests) => {
                    self.event(
                        "flaky",
                        &format!(
                            "{} failed, then passed on the same tree",
                            ids(&tests.iter().cloned().collect::<Vec<_>>())
                        ),
                    );
                    flaky.extend(tests);
                    let mut claim = ValidationClaim::new(
                        chosen.selection.clone(),
                        chosen.policy.clone(),
                        current.tip.clone(),
                    );
                    if carried {
                        claim.policy = format!("carried-forward+delta({})", chosen.policy);
                        claim.scope = Scope::Focused;
                    }
                    return self.land(&current, claim, validations, flaky);
                }
                Culprit::Found {
                    index,
                    failed,
                    flaky: flaked,
                } => {
                    flaky.extend(flaked);
                    let culprit = current.prs[index];
                    let reason = format!(
                        "{} fail with #{culprit} on top of {} (seen twice) and pass without it",
                        ids(&failed.iter().cloned().collect::<Vec<_>>()),
                        if index == 0 {
                            format!("the base {}", short(&current.parent))
                        } else {
                            list(&current.prs[..index])
                        }
                    );
                    // Before removing it, learn what else the culprit's change affects.
                    let affected = self
                        .select(current.prefix(index + 1), current.prefix(index))
                        .map(|chosen| chosen.selection)
                        .unwrap_or(Selection::All);
                    self.eject(
                        culprit,
                        "failed-tests",
                        &reason,
                        &failed.iter().cloned().collect::<Vec<_>>(),
                    )?;
                    self.drop_dependents(culprit, pending);
                    let rebuilt = self.build_and_eject(
                        &current.parent,
                        &current.prs,
                        pending,
                        &format!(" once #{culprit} is removed"),
                    )?;
                    if rebuilt.prs.is_empty() {
                        return Ok(BatchOutcome::Done);
                    }
                    // Delta revalidation: the failed tests, the tests not yet seen passing, and
                    // whatever the removed change touched, within what the batch selected. Under
                    // the exact-commit rule nothing carries over: the remainder reruns it all.
                    let exact = self.config.exact_sha;
                    let delta = if exact {
                        chosen.selection.clone()
                    } else {
                        focus
                            .union(&affected)
                            .intersect(&chosen.selection)
                            .union(&focus)
                            .union(&Selection::of(unproven.iter().cloned()))
                    };
                    let (rid, rran) = self
                        .validate_all(vec![Job {
                            purpose: "remainder",
                            prs: rebuilt.prs.clone(),
                            commit: rebuilt.tip.clone(),
                            selection: delta.clone(),
                            policy: if exact {
                                chosen.policy.clone()
                            } else {
                                format!("delta-after-eject({})", chosen.policy)
                            },
                        }])
                        .pop()
                        .ok_or("no remainder result")??;
                    validations.push(rid);
                    if rran.green() {
                        let claim = if exact {
                            ValidationClaim::new(
                                chosen.selection.clone(),
                                chosen.policy.clone(),
                                rebuilt.tip.clone(),
                            )
                        } else {
                            // The tree differs from the batch's validated tree only by the
                            // removed change; tests outside the delta are carried forward.
                            ValidationClaim {
                                selection: chosen.selection.clone(),
                                policy: format!("carried-forward+delta({})", chosen.policy),
                                scope: Scope::Focused,
                                commit: rebuilt.tip.clone(),
                            }
                        };
                        return self.land(&rebuilt, claim, validations, flaky);
                    }
                    let passed_now: BTreeSet<TestId> = rran
                        .results
                        .results
                        .iter()
                        .filter(|(_, outcome)| **outcome == Outcome::Pass)
                        .map(|(id, _)| id.clone())
                        .collect();
                    unproven = unproven
                        .difference(&passed_now)
                        .cloned()
                        .chain(not_passed(&rran))
                        .collect();
                    seen_failed = rran.failed();
                    focus = if seen_failed.contains(&TestId::step(UNATTRIBUTED)) {
                        delta
                    } else {
                        Selection::of(seen_failed.iter().cloned())
                    };
                    current = rebuilt;
                    carried = !self.config.exact_sha;
                }
            }
        }
    }

    /// Search the candidate's prefixes for the first change that makes `focus` fail, running only
    /// `focus`, with up to `width` probes at once. The bare parent is probed until known, so a
    /// broken base never ejects an innocent change; spare probe slots re-run the suspected
    /// failing prefix, and every blaming conclusion needs its failure seen twice.
    fn find_culprit(
        &self,
        candidate: &Candidate,
        focus: &Selection,
        first_failed: &BTreeSet<TestId>,
    ) -> Result<Culprit, String> {
        let n = candidate.prs.len();
        let mut obs: BTreeMap<usize, Obs> = BTreeMap::new();
        obs.insert(
            n,
            Obs {
                fails: 1,
                passes: 0,
                failed: first_failed.clone(),
            },
        );
        if self.config.trust_base && n > 0 {
            obs.insert(
                0,
                Obs {
                    passes: 1,
                    ..Obs::default()
                },
            );
        }
        let seen =
            |obs: &BTreeMap<usize, Obs>, i: usize| obs.get(&i).map_or(Seen::Unknown, Obs::seen);
        let confirmed =
            |obs: &BTreeMap<usize, Obs>, i: usize| obs.get(&i).is_some_and(Obs::confirmed_fail);
        loop {
            let flaky: BTreeSet<TestId> = obs
                .values()
                .filter(|o| o.passes > 0 && o.fails > 0)
                .flat_map(|o| o.failed.iter().cloned())
                .collect();
            if seen(&obs, n) == Seen::Pass {
                return Ok(Culprit::Flaky(flaky));
            }
            if confirmed(&obs, 0) {
                return Ok(Culprit::BaseRed(obs[&0].failed.clone()));
            }
            let hi = (0..=n).find(|i| seen(&obs, *i) == Seen::Fail).unwrap_or(n);
            let lo = (0..hi).rev().find(|i| seen(&obs, *i) == Seen::Pass);
            if hi > 0 && lo == Some(hi - 1) && confirmed(&obs, hi) {
                return Ok(Culprit::Found {
                    index: hi - 1,
                    failed: obs[&hi].failed.clone(),
                    flaky,
                });
            }
            let mut probes: Vec<usize> = Vec::new();
            if hi == 0 {
                probes.push(0);
            } else {
                if lo.is_none() {
                    probes.push(0);
                }
                let start = lo.unwrap_or(0);
                let room = self.width.saturating_sub(probes.len());
                for k in 1..=room {
                    let point = start + (hi - start) * k / (room + 1);
                    if point > start && point < hi && !probes.contains(&point) {
                        probes.push(point);
                    }
                }
                if probes.len() < self.width && !confirmed(&obs, hi) {
                    probes.push(hi);
                }
                if probes.is_empty() {
                    probes.push(hi);
                }
            }
            let jobs = probes
                .iter()
                .map(|i| Job {
                    purpose: "probe",
                    prs: candidate.prs[..*i].to_vec(),
                    commit: candidate.prefix(*i).to_owned(),
                    selection: focus.clone(),
                    policy: "failed-tests".to_owned(),
                })
                .collect();
            for (i, result) in probes.iter().zip(self.validate_all(jobs)) {
                let (_, ran) = result?;
                let entry = obs.entry(*i).or_default();
                if ran.reproduces() {
                    entry.fails += 1;
                    entry.failed.extend(ran.reproduced());
                } else {
                    entry.passes += 1;
                }
            }
        }
    }

    /// Run the complete suite on the current base. A green run discharges every open obligation
    /// it covers; a red one is recorded on them and leaves them open.
    pub fn widen(&self) -> Result<(), String> {
        let open: Vec<Obligation> = self
            .state
            .obligations()?
            .into_iter()
            .filter(|obligation| obligation.discharged.is_none())
            .collect();
        if open.is_empty() {
            self.event("widen", "no open obligations");
            return Ok(());
        }
        let base = self.current_base()?;
        *lock(&self.base) = base.clone();
        let (_, ran) = self
            .validate_all(vec![Job {
                purpose: "widen",
                prs: open.iter().flat_map(|o| o.prs.clone()).collect(),
                commit: base.clone(),
                selection: Selection::All,
                policy: "all".to_owned(),
            }])
            .pop()
            .ok_or("no widen result")??;
        let failed = ran.failed();
        let mut covered = 0;
        for mut obligation in open {
            if !self.git.is_ancestor(&obligation.landed, &base)? {
                continue;
            }
            covered += 1;
            let record = Discharge {
                at: now(),
                commit: base.clone(),
                green: ran.green(),
                failed: Selection::of(failed.iter().cloned()),
            };
            if ran.green() {
                obligation.discharged = Some(record);
            } else {
                obligation.failed_runs.push(record);
            }
            self.state.save_obligation(&obligation)?;
        }
        self.event(
            "widen",
            &if ran.green() {
                format!(
                    "complete run green on {}; {covered} obligation(s) discharged",
                    short(&base)
                )
            } else {
                format!(
                    "complete run RED on {}: {}; {covered} obligation(s) stay open (focused landings since the last green complete run are the suspects)",
                    short(&base),
                    ids(&failed.into_iter().collect::<Vec<_>>())
                )
            },
        );
        Ok(())
    }
}

/// Cancel every flight, wait for each to report, and return their batches to the queue front in
/// their original order.
fn discard(
    flights: &mut VecDeque<Flight>,
    rx: &mpsc::Receiver<(u64, JobResult)>,
    pending: &mut VecDeque<Vec<i64>>,
) {
    for flight in flights.iter() {
        flight.cancel.store(true, Ordering::Relaxed);
    }
    let mut waiting: BTreeSet<u64> = flights
        .iter()
        .filter(|flight| flight.result.is_none())
        .map(|flight| flight.id)
        .collect();
    while !waiting.is_empty() {
        match rx.recv() {
            Ok((id, _)) => {
                waiting.remove(&id);
            }
            Err(_) => break,
        }
    }
    while let Some(flight) = flights.pop_back() {
        pending.push_front(flight.candidate.prs);
    }
}

/// Tests a run did not see pass (skipped, errored, failed).
fn not_passed(ran: &Ran) -> BTreeSet<TestId> {
    ran.results
        .results
        .iter()
        .filter(|(id, outcome)| **outcome != Outcome::Pass && id.step != UNATTRIBUTED)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Most validations whose intervals overlap at one instant.
fn max_concurrent(validations: &[ValidationRecord]) -> usize {
    let mut edges: Vec<(u64, i32)> = Vec::new();
    for v in validations {
        edges.push((v.start_ms, 1));
        edges.push((v.start_ms + v.elapsed_ms, -1));
    }
    edges.sort_by_key(|(at, delta)| (*at, *delta));
    let mut current = 0i32;
    let mut best = 0i32;
    for (_, delta) in edges {
        current += delta;
        best = best.max(current);
    }
    best.max(0) as usize
}

fn list(prs: &[i64]) -> String {
    if prs.is_empty() {
        return "(nothing)".to_owned();
    }
    prs.iter()
        .map(|pr| format!("#{pr}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ids(tests: &[TestId]) -> String {
    tests
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a run report for people.
pub fn render_report(report: &RunReport) -> String {
    let mut out = format!(
        "# merge-queue {} {}\n\nBase {} -> {}. Wall {:.0}s, validation time {:.0}s, {} validation(s), at most {} at once, {:.1} changes/hour.\n",
        report.mode,
        report.run,
        short(&report.base_start),
        short(&report.base_end),
        report.wall_s,
        report.validation_s,
        report.validations.len(),
        report.max_concurrent,
        report.prs_per_hour
    );
    if !report.stopped.is_empty() {
        out.push_str(&format!("\n**Stopped:** {}\n", report.stopped));
    }
    if !report.would_validate.is_empty() {
        out.push_str("\n## Would validate\n\n");
        for would in &report.would_validate {
            out.push_str(&format!(
                "- {} at `{}`: {} ({})\n",
                list(&would.prs),
                short(&would.commit),
                would.selection.summary(),
                would.policy
            ));
        }
    }
    out.push_str("\n## Landed\n\n");
    if report.landed.is_empty() {
        out.push_str("Nothing.\n");
    }
    for landed in &report.landed {
        out.push_str(&format!(
            "- {} at `{}`: {} ({}, policy {})",
            list(&landed.prs),
            short(&landed.tip),
            landed.claim.selection.summary(),
            landed.claim.scope.as_str(),
            landed.claim.policy
        ));
        if !landed.flaky.is_empty() {
            out.push_str(&format!(
                "; flaky: {}",
                ids(&landed.flaky.iter().cloned().collect::<Vec<_>>())
            ));
        }
        if !landed.note.is_empty() {
            out.push_str(&format!("; {}", landed.note));
        }
        out.push('\n');
    }
    if !report.regenerated.is_empty() {
        out.push_str("\n## Regenerated\n\n");
        for regen in &report.regenerated {
            out.push_str(&format!(
                "- #{}: {} at `{}`{}\n",
                regen.pr,
                if regen.rewritten.is_empty() {
                    "already current".to_owned()
                } else {
                    regen.rewritten.join(", ")
                },
                short(&regen.commit),
                if regen.conflicted.is_empty() {
                    String::new()
                } else {
                    format!(" (conflicted: {})", regen.conflicted.join(", "))
                }
            ));
        }
    }
    if !report.ejected.is_empty() {
        out.push_str("\n## Ejected\n\n");
        for ejection in &report.ejected {
            out.push_str(&format!(
                "- #{} ({}): {}\n",
                ejection.pr, ejection.kind, ejection.reason
            ));
        }
    }
    if !report.requeued.is_empty() {
        out.push_str(&format!(
            "\nReturned to the queue: {}\n",
            list(&report.requeued)
        ));
    }
    if !report.obligations.is_empty() {
        out.push_str(&format!(
            "\n{} focused landing(s) owe a complete run (`merge-queue widen`): {}\n",
            report.obligations.len(),
            report.obligations.join(", ")
        ));
    }
    out.push_str("\n## Validations\n\n| # | purpose | changes | commit | selection | result | start s | seconds |\n|---|---|---|---|---|---|---|---|\n");
    for v in &report.validations {
        let result = if v.cancelled {
            "cancelled".to_owned()
        } else if v.green {
            "green".to_owned()
        } else {
            format!("red: {}", ids(&v.failed))
        };
        out.push_str(&format!(
            "| {} | {} | {} | `{}` | {} ({}) | {} | {:.1} | {:.1} |\n",
            v.id,
            v.purpose,
            list(&v.prs),
            short(&v.commit),
            v.selection.summary(),
            v.policy,
            result,
            v.start_ms as f64 / 1000.0,
            v.elapsed_ms as f64 / 1000.0
        ));
    }
    out
}

/// The `widen` command: one complete run on the base, outside a queue run. It needs a
/// validator but no land driver.
pub fn widen(state: &State) -> Result<RunReport, String> {
    let mut runner = Runner::new(
        state,
        RunOptions {
            dry_run: true,
            depth: Some(1),
            ..RunOptions::default()
        },
    )?;
    // Constructed as a dry run only to skip the land-driver check; widening is real.
    runner.options.dry_run = false;
    lock(&runner.report).mode = "widen".to_owned();
    let result = runner.widen();
    let report = runner.finish(match &result {
        Ok(()) => String::new(),
        Err(error) => format!("error: {error}"),
    })?;
    result.map(|()| report)
}
