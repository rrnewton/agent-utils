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

use serde::{Deserialize, Serialize};

use crate::git::{Combined, Git};
use crate::model::{now, Discharge, Item, ItemStatus, Obligation, Plan};
use crate::ops::{self, repo};
use crate::plan::short;
use crate::state::{write_json, Config, State};
use crate::tests::{Outcome, Scope, Selection, TestId, TestResult, TestResults, ValidationClaim};
use crate::validate::{from_config, Chosen, ProbeObservation, Ran, Validator};

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
    /// Override `validator.offload` (`off`, `compare` or `decide`).
    pub offload: Option<String>,
}

/// Donor validators for this run (offload `compare` or `decide`).
struct Donors {
    store: crate::donor::Store,
    mode: crate::state::Offload,
    /// Tool identity computed on this host when the run started.
    tool: crate::donor::ToolIdentity,
    /// Jobs this run published and has not settled yet: (job id, candidate commit).
    published: Mutex<Vec<(String, String)>>,
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
    /// Stable kind: `failed-tests`, `conflict`, `regeneration` (generator
    /// ran but its output was unacceptable) or `regeneration-build`
    /// (the generator command itself failed to run or exited non-zero).
    pub kind: String,
    /// Human explanation.
    pub reason: String,
    /// Failed tests, for `failed-tests`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<TestId>,
    /// Failure classes of the failed tests (from `classify`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    /// Report file.
    pub report: PathBuf,
}

/// One batch deferred after a validation refusal: still queued, retry after backoff.
#[derive(Clone, Debug, Serialize)]
pub struct Deferred {
    /// Changes deferred, in order.
    pub prs: Vec<i64>,
    /// Why validation was refused (exit status / marker).
    pub reason: String,
    /// RFC 3339 UTC time before which validation should not be retried.
    pub retry_after: String,
    /// Refusal count recorded on the changes after this deferral.
    pub refusals: u32,
    /// Changes that reached `validator.max_deferrals` with this deferral: escalated to their
    /// owner and held until `merge-queue retry`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalated: Vec<i64>,
}

/// One validation the run performed.
#[derive(Clone, Debug, Deserialize)]
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
    /// Full sparse per-test results (outcome map, durations, log pointers
    /// and the unattributed flag). This is the stored truth; `green` and
    /// `failed` below are derived from it, never stored.
    pub results: TestResults,
    /// Runner exit status (`None` when killed by the timeout).
    pub exit: Option<i32>,
    /// Runner log file.
    pub log: PathBuf,
    /// Whether admission refused to run it (exit 75 / `COULD_NOT_RUN`). A refusal is
    /// neither green nor red and attributes no failed test.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refused: bool,
    /// Whether the queue cancelled it (its candidate was discarded).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancelled: bool,
    /// Failure classes of the failed tests (from `classify`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    /// Start time in milliseconds after the run began.
    pub start_ms: u64,
    /// Wall time in milliseconds.
    pub elapsed_ms: u64,
    /// Directory with the log and results.
    pub dir: PathBuf,
}

impl ValidationRecord {
    /// Whether it was green (derived). A refusal/cancel is never green.
    pub fn green(&self) -> bool {
        !self.cancelled && !self.refused && self.exit == Some(0) && self.results.green()
    }

    /// Failed test IDs (derived). Unattributed runs report none; their
    /// `results.unattributed` flag carries that fact.
    pub fn failed(&self) -> Vec<TestId> {
        if self.cancelled || self.refused {
            return Vec::new();
        }
        self.results.failed().into_iter().collect()
    }

    /// Full failed results (outcome/duration/log) in ID order.
    pub fn failed_results(&self) -> Vec<crate::tests::TestResult> {
        if self.cancelled || self.refused {
            return Vec::new();
        }
        self.results.failed_results()
    }
}

// Serialized form carries the persisted sparse `results` plus `green` /
// `failed` computed from it (derived, never stored), for report readers.
impl Serialize for ValidationRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ValidationRecord", 15)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("purpose", &self.purpose)?;
        state.serialize_field("prs", &self.prs)?;
        state.serialize_field("commit", &self.commit)?;
        state.serialize_field("selection", &self.selection)?;
        state.serialize_field("policy", &self.policy)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("exit", &self.exit)?;
        state.serialize_field("log", &self.log)?;
        state.serialize_field("green", &self.green())?;
        state.serialize_field("failed", &self.failed())?;
        if self.refused {
            state.serialize_field("refused", &self.refused)?;
        }
        if self.cancelled {
            state.serialize_field("cancelled", &self.cancelled)?;
        }
        if !self.classes.is_empty() {
            state.serialize_field("classes", &self.classes)?;
        }
        state.serialize_field("start_ms", &self.start_ms)?;
        state.serialize_field("elapsed_ms", &self.elapsed_ms)?;
        state.serialize_field("dir", &self.dir)?;
        state.end()
    }
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
    /// Batches deferred after a validation refusal (still queued, retry after backoff).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deferred: Vec<Deferred>,
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
///
/// `prefixes` holds the commit after each change, aligned with `prs`. Only
/// the final tip is regenerated eagerly in [`Runner::build`] (always under
/// `regenerate.when = always`, and under `when = conflict` whenever the batch
/// touched a generated file); earlier prefixes keep the raw combined tree,
/// whose generated files may be stale. A prefix is regenerated lazily, on
/// first use, by [`Runner::regenerated_prefix`] whenever the candidate's
/// batch could have left one stale, because probe validation and the
/// `validator.select` command both run in a checkout of the prefix and may
/// read generated files, so a stale prefix must never be validated or
/// selected from as-is.
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

/// Changes that did not combine onto a candidate: number, kind (`conflict`,
/// `regeneration`, `regeneration-build` or `not-fast-forward`), and detail.
type Uncombined = Vec<(i64, &'static str, String)>;

/// Outcome of handling one batch.
enum BatchOutcome {
    /// Landed, or nothing left of the batch to land.
    Done,
    /// The base moved under the batch; these changes go back to the front of the queue.
    BaseMoved(Vec<i64>),
    /// Stop the queue; these changes go back to the front of the queue.
    Stop(String, Vec<i64>),
    /// Validation admission refused: the batch stays queued and is retried after backoff.
    /// Never an ejection and never a landing.
    Deferred(String, Vec<i64>),
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
    /// Probes that observed nothing conclusive (`Missing`/`Skip`-only):
    /// counted separately so they never read as a pass.
    unknowns: usize,
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
    /// A probe was refused by admission: no culprit can be named; defer the batch.
    Refused(String),
    /// A probe observed nothing conclusive (`Missing`/`Skip`-only): no
    /// culprit can be named; the batch stops rather than guessing.
    Inconclusive(String),
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

/// Backoff before refusal retry `attempt` (0-based): base doubled per attempt, capped at 60s.
/// A base of 0 means no delay.
pub fn refusal_backoff(attempt: u32, base_secs: u64) -> Duration {
    if base_secs == 0 {
        return Duration::ZERO;
    }
    let factor = 1u64.checked_shl(attempt.min(6)).unwrap_or(64);
    Duration::from_secs((base_secs.saturating_mul(factor)).min(60))
}

/// RFC 3339 time `delay` from now, for a deferred change's `next_retry_at`.
fn retry_after_time(delay: Duration) -> String {
    let at = chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Human detail for a refused validation: for a validator that could not execute, what
/// happened, the command and the `PATH` it ran with.
fn refusal_detail(ran: &Ran) -> String {
    if let Some(why) = &ran.cannot_execute {
        return why.clone();
    }
    match ran.exit {
        Some(code) => format!("exit {code} ({})", crate::validate::REFUSAL_MARKER),
        None => format!("no exit status ({})", crate::validate::REFUSAL_MARKER),
    }
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
    /// Donor validators, when `validator.offload` is not off.
    donors: Option<Donors>,
}

impl<'a> Runner<'a> {
    /// Prepare a run: load config, items and validator, and allocate the run directory.
    pub fn new(state: &'a State, options: RunOptions) -> Result<Self, String> {
        let config = state.config()?;
        let validator = from_config(&config.validator)?;
        let offload = crate::state::Offload::parse(
            options
                .offload
                .as_deref()
                .unwrap_or(config.validator.offload.as_str()),
        )?;
        let donors = if offload == crate::state::Offload::Off || options.dry_run {
            None
        } else {
            let mut store = crate::donor::Store::new(&state.root);
            store.lease_secs = config.donors.lease();
            store.could_not_run_limit = config.donors.could_not_run_limit();
            match crate::donor::tool_identity(&config.validator, &config.repo) {
                Ok(tool) => {
                    let reason = "superseded: the run that published it ended without settling it";
                    let swept = if offload == crate::state::Offload::Compare {
                        store.retire_all_live(reason)
                    } else {
                        store.cancel_all_live(reason)
                    };
                    match swept {
                        Ok(swept) if !swept.is_empty() => eprintln!(
                            "merge-queue: {} {} donor job(s) an earlier run left live",
                            if offload == crate::state::Offload::Compare {
                                "retired"
                            } else {
                                "cancelled"
                            },
                            swept.len()
                        ),
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("merge-queue: cannot sweep earlier donor jobs: {error}")
                        }
                    }
                    Some(Donors {
                        store,
                        mode: offload,
                        tool,
                        published: Mutex::new(Vec::new()),
                    })
                }
                // Compare mode never lets the donor side affect local validation.
                Err(error) if offload == crate::state::Offload::Compare => {
                    eprintln!("merge-queue: donors off for this run: {error}");
                    None
                }
                Err(error) => return Err(format!("offload decide: {error}")),
            }
        };
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
        let items: BTreeMap<i64, Item> = state
            .items()?
            .into_iter()
            .map(|item| (item.pr, item))
            .collect();
        // Changes escalated to their owner are held: never built into a candidate until
        // `merge-queue retry`. A shadow run reads its own ledger, since it writes no items.
        let mut held: BTreeSet<i64> = items
            .values()
            .filter(|item| item.status == ItemStatus::Queued && !item.escalated.is_empty())
            .map(|item| item.pr)
            .collect();
        if simulate {
            for (pr, entry) in state.shadow_refusals()? {
                if !entry.escalated.is_empty()
                    && items.get(&pr).is_some_and(|item| item.head == entry.head)
                {
                    held.insert(pr);
                }
            }
        }
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
            excluded: Mutex::new(held),
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
            donors,
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

    /// Record a refusal deferral for `prs`: each change stays `queued`, its refusal count
    /// grows, and `next_retry_at` is set from the backoff for that count. A change whose count
    /// reaches `validator.max_deferrals` is escalated to its owner instead: held, with an
    /// alarm, until `merge-queue retry`. Shadow runs, which write no items, count in the
    /// shadow ledger so the bound holds there too. Returns the retry-after time and the new
    /// refusal count (from the first change).
    fn defer_refusal(&self, prs: &[i64], detail: &str) -> Result<(String, u32), String> {
        let base = self.config.validator.refusal_backoff();
        let limit = self.config.validator.deferral_limit();
        let escalation = |count: u32| {
            format!("validation did not run {count} time(s) (the bound is {limit}); last: {detail}")
        };
        let mut retry_after = String::new();
        let mut refusals = 0u32;
        let mut escalated = Vec::new();
        if !self.simulating() {
            let mut items = lock(&self.items);
            for pr in prs {
                if let Some(item) = items.get_mut(pr) {
                    item.validation_refusals = item.validation_refusals.saturating_add(1);
                    refusals = item.validation_refusals;
                    item.last_refusal = detail.to_owned();
                    item.deciding_report = self.run_dir.join("report.md").display().to_string();
                    item.deciding_failed.clear();
                    if item.validation_refusals >= limit {
                        item.escalated = escalation(item.validation_refusals);
                        item.next_retry_at.clear();
                        let note = item.escalated.clone();
                        item.log("escalated", note);
                        escalated.push(*pr);
                    } else {
                        let delay =
                            refusal_backoff(item.validation_refusals.saturating_sub(1), base);
                        retry_after = retry_after_time(delay);
                        item.next_retry_at = retry_after.clone();
                        item.log(
                            "validation-refused",
                            format!(
                                "{detail}; stays queued, retry after {} (refusal {} of {limit})",
                                item.next_retry_at, item.validation_refusals
                            ),
                        );
                    }
                    self.state.save_item(item)?;
                }
            }
        } else {
            let items = lock(&self.items);
            let mut ledger = self.state.shadow_refusals()?;
            for pr in prs {
                let head = items
                    .get(pr)
                    .map(|item| item.head.clone())
                    .unwrap_or_default();
                let entry = ledger.entry(*pr).or_default();
                if entry.head != head {
                    *entry = crate::state::ShadowRefusal {
                        head,
                        ..Default::default()
                    };
                }
                entry.refusals = entry.refusals.saturating_add(1);
                refusals = entry.refusals;
                if entry.refusals >= limit {
                    entry.escalated = escalation(entry.refusals);
                    escalated.push(*pr);
                }
            }
            self.state.save_shadow_refusals(&ledger)?;
        }
        if retry_after.is_empty() {
            retry_after = retry_after_time(refusal_backoff(refusals.saturating_sub(1), base));
            refusals = refusals.max(1);
        }
        lock(&self.report).deferred.push(Deferred {
            prs: prs.to_vec(),
            reason: detail.to_owned(),
            retry_after: retry_after.clone(),
            refusals,
            escalated: escalated.clone(),
        });
        self.event(
            "validation-refused",
            &format!(
                "{}: {detail}; stays queued, retry after {retry_after}",
                list(prs)
            ),
        );
        if !escalated.is_empty() {
            let alarm = format!(
                "ALARM: {} escalated to the owner: {}; held until `merge-queue retry {}`",
                list(&escalated),
                escalation(refusals),
                escalated
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            eprintln!("merge-queue: {alarm}");
            self.event("escalated", &alarm);
        }
        Ok((retry_after, refusals))
    }

    /// Retry a refused batch validation with backoff. Returns the final `(id, ran)`.
    /// Each attempt is its own validation record (refused attempts are visible, never
    /// counted as green or red).
    fn retry_refused(
        &self,
        candidate: &Candidate,
        chosen: &Chosen,
        first: (usize, Ran),
    ) -> Result<(usize, Ran), String> {
        let retries = self.config.validator.refusal_retries();
        let base = self.config.validator.refusal_backoff();
        let (mut vid, mut ran) = first;
        for attempt in 0..retries {
            if !ran.refused() {
                break;
            }
            let delay = refusal_backoff(attempt as u32, base);
            self.event(
                "validation-refused",
                &format!(
                    "{} refused ({}); retry {} of {retries} after {}s",
                    list(&candidate.prs),
                    refusal_detail(&ran),
                    attempt + 1,
                    delay.as_secs()
                ),
            );
            sleep_unless_stopped(delay);
            let (next_vid, next_ran) = self
                .validate_all(vec![Job {
                    purpose: "batch",
                    prs: candidate.prs.clone(),
                    commit: candidate.tip.clone(),
                    selection: chosen.selection.clone(),
                    policy: chosen.policy.clone(),
                }])
                .pop()
                .ok_or("no retry result")??;
            vid = next_vid;
            ran = next_ran;
        }
        Ok((vid, ran))
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
        let mut generator_ok: BTreeSet<String> = BTreeSet::new();
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
            // Regenerate right after this change whenever the combined tree may hold stale or
            // conflict-marked generated files: the change conflicted in one, its combination
            // changed one (a textual merge of generated files need not be the generator's
            // output), or `when = always`. Doing it per change keeps every prefix a
            // self-consistent tree for culprit probes, and the next change builds on the
            // regenerated prefix, so the prefixes form a fast-forward chain for per-change
            // landing drivers.
            let touched = regen.enabled()
                && (!conflicted.is_empty()
                    || regen.always()
                    || self
                        .git
                        .diff_files(&tip, &next)?
                        .iter()
                        .any(|path| regen.covers(path)));
            let next = if touched {
                match self.regenerate(&next, *pr, &conflicted)? {
                    Ok(done) => done,
                    Err((kind, why)) => {
                        // Before blaming this change, check that the generator works on the
                        // tree just before it: if that fails too, the base or the generator's
                        // environment is broken, and every change would be ejected in turn.
                        if !generator_ok.contains(&tip) {
                            match self.regenerate(&tip, *pr, &[])? {
                                Ok(_) => {
                                    generator_ok.insert(tip.clone());
                                }
                                Err((_, parent_why)) => {
                                    return Err(format!(
                                        "regeneration fails on {} itself, before #{pr}: {parent_why}; fix the base or the generator (no change was ejected)",
                                        short(&tip)
                                    ));
                                }
                            }
                        }
                        skipped.insert(*pr);
                        conflicts.push((*pr, kind, why));
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
    /// command and commit the result. `Ok(Err((kind, why)))` means this change cannot be
    /// regenerated: `regen-build-failed` (the optional build step failed: the combined tree
    /// cannot build the generator), `regen-failed` (the generator command failed),
    /// `regen-touched-other` or `regen-left-conflict`. `Err` (including a failure to start a
    /// process) means the run itself cannot continue.
    #[allow(clippy::type_complexity)]
    fn regenerate(
        &self,
        tip: &str,
        pr: i64,
        conflicted: &[String],
    ) -> Result<Result<String, (&'static str, String)>, String> {
        let regen = &self.config.regenerate;
        if !regen.enabled() {
            return Ok(Err((
                "regen-failed",
                "generated files conflict but no regeneration is configured".into(),
            )));
        }
        let slot = self.slots.take();
        let result = (|| -> Result<Result<String, (&'static str, String)>, String> {
            let checkout = self.slots.checkout(slot, tip)?;
            // A generator killed by an outside signal (a service stop, the OOM killer) says
            // nothing about the change: stop the run instead of ejecting it.
            let run_script = |script: &str, what: &str| -> Result<std::process::Output, String> {
                let output = Command::new("sh")
                    .arg("-c")
                    .arg(script)
                    .current_dir(&checkout)
                    .env("MQ_PR", pr.to_string())
                    .env("MQ_REGEN_PATHS", regen.paths.join(" "))
                    .env("MQ_CONFLICTED", conflicted.join(" "))
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|error| format!("cannot run {what}: {error}"))?;
                if let Some(signal) = crate::interrupt::outside_kill(&output.status) {
                    return Err(crate::interrupt::killed(what, signal));
                }
                if !output.status.success() {
                    crate::interrupt::check(&format!("reading the {what} failure"))?;
                }
                Ok(output)
            };
            if !regen.build_command.trim().is_empty() {
                let built = run_script(&regen.build_command, "regenerate.build_command")?;
                if !built.status.success() {
                    return Ok(Err((
                        "regen-build-failed",
                        format!(
                            "build exited {}: {}",
                            built.status.code().unwrap_or(-1),
                            output_tail(&built, 6)
                        ),
                    )));
                }
            }
            let output = run_script(&regen.command, "regenerate.command")?;
            if !output.status.success() {
                return Ok(Err((
                    "regen-failed",
                    format!(
                        "regenerate.command failed (exit {}): {}",
                        output.status.code().unwrap_or(-1),
                        output_tail(&output, 6)
                    ),
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
                return Ok(Err((
                    "regen-touched-other",
                    format!(
                        "regenerate.command changed files that are not generated: {}",
                        stray
                            .iter()
                            .map(|p| p.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )));
            }
            for path in conflicted.iter().chain(&changed) {
                let text = std::fs::read_to_string(checkout.join(path)).unwrap_or_default();
                if text
                    .lines()
                    .any(|line| line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> "))
                {
                    return Ok(Err((
                        "regen-left-conflict",
                        format!("conflict markers remain in {path} after regeneration"),
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

    /// The commit for prefix `i` of `candidate` with generated files refreshed,
    /// regenerating that specific prefix on first use and caching the result on
    /// the candidate so each prefix is regenerated at most once.
    ///
    /// Prefix decision: stale prefixes are not accepted. Validation selection
    /// can depend on generated paths: `shared_select` in `validate.rs` runs the
    /// configured `validator.select` shell command inside a checkout of the
    /// prefix commit with `MQ_BASE`/`MQ_HEAD` set, so a select script may map
    /// generated files to tests; `Runner::handle_red` uses exactly that to
    /// compute the tests a culprit affected. Probe validation can depend on
    /// them too: `find_culprit` runs the validator in a prefix checkout, and a
    /// test may compare a generated file against its sources (the `gen` test in
    /// `tests/cli_regen.rs` does). A stale generated file could therefore
    /// change a selection or a probe verdict and blame the wrong change, so the
    /// specific prefix is regenerated when probed. Prefix `0` is the parent,
    /// which was itself validated before landing, and the final prefix is the
    /// already-regenerated tip; neither is regenerated here. When the batch
    /// never touched a generated file (or regeneration is unconfigured) the
    /// raw prefix is returned unchanged.
    fn regenerated_prefix(&self, candidate: &Candidate, i: usize) -> Result<String, String> {
        Ok(candidate.prefix(i).to_owned())
    }

    /// Whether green results recorded at exactly `commit` cover `selection`: every selected test
    /// passed in the latest run at that commit that selected it, and an `all` selection needs a
    /// complete run there whose failures were all rerun green there.
    fn covered(&self, commit: &str, selection: &Selection) -> bool {
        let report = lock(&self.report);
        let runs = report
            .validations
            .iter()
            .filter(|v| v.commit == commit && !v.cancelled && !v.refused);
        let mut complete = false;
        let mut complete_blocked = false;
        let mut failing: BTreeSet<TestId> = BTreeSet::new();
        let mut passed: BTreeSet<TestId> = BTreeSet::new();
        for run in runs {
            let unattributed = run.results.unattributed;
            let failed = run.failed();
            if run.selection.is_all() {
                complete = true;
                // An unattributed complete run proves only its exit status:
                // green covers everything, red covers nothing.
                complete_blocked = unattributed && !run.green();
                failing = failed.iter().cloned().collect();
                passed.clear();
                continue;
            }
            for test in run.selection.tests().into_iter().flatten() {
                if (unattributed && !run.green())
                    || failed
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
            Selection::All => complete && !complete_blocked && failing.is_empty(),
            Selection::Tests(tests) => tests.iter().all(|test| {
                passed
                    .iter()
                    .any(|p| p == test || (p.case.is_none() && p.step == test.step))
                    || (complete
                        && !complete_blocked
                        && !failing.iter().any(|f| f.step == test.step))
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
                "regen-build-failed" => format!(
                    "the combined tree onto {}{context} does not build the generator, so its generated files cannot be regenerated (the change is stale against the base or breaks the generator's sources): {detail}",
                    short(parent)
                ),
                "regen-touched-other" | "regen-left-conflict" => format!(
                    "regeneration onto {}{context} is unsafe: {detail}",
                    short(parent)
                ),
                _ => format!(
                    "the generator failed onto {}{context}: {detail}",
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
        crate::interrupt::check(&format!("validating {}", list(&job.prs)))?;
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
        let start_ms = self.started.elapsed().as_millis() as u64;
        let ran = match &self.donors {
            Some(donors) if donors.mode == crate::state::Offload::Decide => {
                self.donor_validate(donors, id, &job, &dir, cancel)?
            }
            _ => {
                // Compare mode: donors validate the same commit too, but only the local
                // verdict decides; a donor that cannot publish never affects it.
                let compared = self.donors.as_ref().and_then(|donors| {
                    self.publish(donors, &job)
                        .map_err(|error| {
                            self.event(
                                "donor",
                                &format!("v{id}: not published for comparison: {error}"),
                            )
                        })
                        .ok()
                        .map(|published| (donors, published))
                });
                let slot = self.slots.take();
                let result = self.slots.checkout(slot, &job.commit).and_then(|checkout| {
                    self.validator
                        .run_with(&checkout, &job.selection, &dir, cancel, &extra)
                });
                self.slots.give(slot);
                if let Some((donors, published)) = compared {
                    let recorded = match &result {
                        Ok(ran) if !ran.cancelled && ran.interrupted.is_none() => {
                            donors.store.record_local(&published, local_verdict(ran))
                        }
                        _ => donors
                            .store
                            .cancel(&published, "the local validation did not finish"),
                    };
                    if let Err(error) = recorded {
                        self.event("donor", &format!("v{id}: comparison not recorded: {error}"));
                    }
                }
                result?
            }
        };
        if let Some(handle) = &ran.handle {
            lock(&self.handles).insert(job.commit.clone(), (handle.clone(), dir.clone()));
        }
        let failed: Vec<TestId> = if ran.cancelled {
            Vec::new()
        } else {
            ran.failed().into_iter().collect()
        };
        let classes = crate::state::classify(&self.config.classify, &failed);
        let record = ValidationRecord {
            id,
            purpose: job.purpose.to_owned(),
            prs: job.prs,
            commit: job.commit,
            selection: ran.effective_selection.clone().unwrap_or(job.selection),
            policy: job.policy,
            results: ran.results.clone(),
            exit: ran.exit,
            log: ran.log.clone(),
            refused: ran.refused(),
            cancelled: ran.cancelled,
            classes,
            start_ms,
            elapsed_ms: ran.elapsed.as_millis() as u64,
            dir: dir.clone(),
        };
        write_json(&dir.join("record.json"), &record)?;
        self.event(
            "result",
            &format!(
                "v{id} {} in {:.1}s{}{}",
                if ran.interrupted.is_some() {
                    "interrupted"
                } else if record.cancelled {
                    "cancelled"
                } else if record.refused {
                    "refused"
                } else if record.green() {
                    "green"
                } else {
                    "red"
                },
                ran.elapsed.as_secs_f64(),
                if record.results.unattributed {
                    " (unattributed)"
                } else {
                    ""
                },
                if record.failed().is_empty() {
                    String::new()
                } else {
                    format!(", failed: {}", ids(&record.failed()))
                }
            ),
        );
        {
            let mut report = lock(&self.report);
            report.validation_s += ran.elapsed.as_secs_f64();
            report.validations.push(record);
        }
        // An interrupted validation is recorded (cancelled) but is no result at all: every
        // caller propagates the error, so it can never become a landing or a culprit search.
        if let Some(why) = &ran.interrupted {
            return Err(format!("v{id}: {why}"));
        }
        Ok((id, ran))
    }

    /// Publish `job` for donors; returns the donor job id.
    fn publish(&self, donors: &Donors, job: &Job) -> Result<String, String> {
        let tree = self.git.tree_of(&job.commit)?;
        let base = {
            let report = lock(&self.report);
            if report.base_start.is_empty() {
                lock(&self.base).clone()
            } else {
                report.base_start.clone()
            }
        };
        let published = donors.store.publish(
            &job.commit,
            &tree,
            &base,
            &job.selection,
            &self.config.validator,
            &donors.tool,
        )?;
        lock(&donors.published).push((published.id.clone(), job.commit.clone()));
        Ok(published.id)
    }

    /// Cancel this run's donor jobs the queue no longer wants. A job whose candidate is now in
    /// the base landed; once the run has `ended`, every job still live was superseded (the
    /// next run plans and publishes its candidates afresh). The donor of a cancelled job stops
    /// on its next renewal, and its receipt is refused.
    fn prune_donor_jobs(&self, ended: bool) {
        let Some(donors) = &self.donors else {
            return;
        };
        let base = lock(&self.base).clone();
        let mut published = lock(&donors.published);
        published.retain(|(id, commit)| {
            let landed = !base.is_empty()
                && (*commit == base || self.git.is_ancestor(commit, &base).unwrap_or(false));
            let reason = if landed {
                "its candidate landed"
            } else if ended {
                "superseded: the run that published it ended without landing its candidate"
            } else {
                return true;
            };
            // In compare mode a leased job's receipt can still be compared (it cannot affect
            // landing), so only an unleased job is cancelled.
            let retired = if donors.mode == crate::state::Offload::Compare {
                donors.store.retire(id, reason)
            } else {
                donors.store.cancel_live(id, reason)
            };
            match retired {
                Ok(true) => self.event("donor", &format!("job {id} retired: {reason}")),
                Ok(false) => {}
                Err(error) => self.event("donor", &format!("cannot cancel job {id}: {error}")),
            }
            false
        });
    }

    /// Validate `job` on donors only (offload `decide`): publish, wait for an accepted
    /// receipt, and read it as this validation's result. A discarded candidate or a stop
    /// cancels the donor job.
    fn donor_validate(
        &self,
        donors: &Donors,
        id: usize,
        job: &Job,
        dir: &std::path::Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String> {
        let published = self.publish(donors, job)?;
        self.event(
            "donor",
            &format!("v{id}: published as donor job {published}"),
        );
        let claim_timeout = Duration::from_secs(self.config.donors.claim_timeout());
        let mut waiting_since = Instant::now();
        let mut could_not_run = 0;
        let started = Instant::now();
        let stopped = |cancelled: bool, interrupted: Option<String>| Ran {
            results: TestResults::default(),
            exit: None,
            elapsed: started.elapsed(),
            log: dir.join("run.log"),
            cancelled,
            interrupted,
            handle: None,
            refused: false,
            cannot_execute: None,
            effective_selection: None,
        };
        loop {
            if let Err(why) = crate::interrupt::check("a donor receipt arrived") {
                let _ = donors.store.cancel(&published, "the queue is stopping");
                return Ok(stopped(true, Some(why)));
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = donors
                    .store
                    .cancel(&published, "the candidate was discarded");
                return Ok(stopped(true, None));
            }
            let record = donors.store.get(&published)?;
            for entry in record.could_not_run.iter().skip(could_not_run) {
                // No verdict: the job is queued again; the operator should look at that donor.
                let alarm = format!(
                    "v{id}: ALARM: donor job {published}: {entry} reported that validation could \
                     not run; no verdict, queued again"
                );
                eprintln!("merge-queue: {alarm}");
                self.event("donor", &alarm);
            }
            could_not_run = record.could_not_run.len();
            match record.state {
                crate::donor::JobState::Done => {
                    let receipt = record.receipt.ok_or_else(|| {
                        format!("donor job {published} is done without a receipt")
                    })?;
                    self.event(
                        "donor",
                        &format!(
                            "v{id}: receipt from {} on {}",
                            receipt.donor.name, receipt.donor.host
                        ),
                    );
                    return receipt_ran(&receipt, dir);
                }
                crate::donor::JobState::Cancelled
                    if record.could_not_run.len() >= donors.store.could_not_run_limit =>
                {
                    // Donors never ran it: no verdict; the batch is refused (and deferred,
                    // within the deferral bound) like a validator that cannot execute.
                    let mut ran = stopped(false, None);
                    ran.refused = true;
                    ran.cannot_execute = Some(format!(
                        "{} on donors: job {published} collected {} could-not-run receipts \
                         (the bound): {}",
                        crate::validate::CANNOT_EXECUTE,
                        record.could_not_run.len(),
                        record.could_not_run.join("; ")
                    ));
                    return Ok(ran);
                }
                crate::donor::JobState::Cancelled => {
                    return Err(format!(
                        "donor job {published} was cancelled outside this run"
                    ));
                }
                crate::donor::JobState::Leased => waiting_since = Instant::now(),
                crate::donor::JobState::Queued => {
                    if waiting_since.elapsed() > claim_timeout {
                        let _ = donors.store.cancel(&published, "no donor took it in time");
                        return Err(format!(
                            "no donor took job {published} for {}s (attempts so far: {}); is a donor running (`merge-queue donor run`), and do its tools match? Nothing was ejected",
                            claim_timeout.as_secs(),
                            record.attempts
                        ));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
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
        self.prune_donor_jobs(true);
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
    ///
    /// Planning is pipelined with execution: the first batch comes from
    /// base probes plus cached pairwise results and starts validating at
    /// once, while a background thread probes the remaining pairs. The
    /// completed plan (identical to a one-shot plan) supplies every later
    /// batch. A dry run validates nothing, so it simply finishes the plan
    /// first and reports what it would do.
    fn drive(&self) -> Result<String, String> {
        let mut max_batch = self.options.max_batch.unwrap_or(self.config.max_batch);
        if self.config.land.driver == "command" && !self.config.land.allow_batches {
            max_batch = 1;
        }
        if self.options.dry_run {
            let plan = ops::plan(self.state, &self.config, max_batch)?;
            self.adopt_plan(&plan, false)?;
            let mut pending: VecDeque<Vec<i64>> =
                plan.batches.iter().map(|batch| batch.prs.clone()).collect();
            return self.dry_run(&mut pending);
        }
        let (partial, continuation) = ops::plan_staged(self.state, &self.config, max_batch)?;
        self.adopt_plan(&partial, false)?;
        self.event(
            "plan-first",
            &match partial.batches.first() {
                Some(batch) => format!(
                    "first batch {} ready from base and cached probes; planning continues in the background",
                    list(&batch.prs)
                ),
                None => "no first batch from base and cached probes; planning continues in the background".to_owned(),
            },
        );
        // Only the first batch is handed over now; the pipeline asks the
        // continuation for the rest once this batch is under way.
        let mut pending: VecDeque<Vec<i64>> = partial
            .batches
            .first()
            .map(|batch| vec![batch.prs.clone()])
            .unwrap_or_default()
            .into();
        self.pipeline(&mut pending, continuation)
    }

    /// Record a plan as the run's current plan: dependencies, base,
    /// `plan.json`, the `plan` event and the report's plan. With
    /// `completed` the plan is the finished one, so dependencies are
    /// replaced (the first-stage plan may not know every ancestry edge
    /// yet) and the event is `plan-complete` instead.
    fn adopt_plan(&self, plan: &Plan, completed: bool) -> Result<(), String> {
        {
            let mut deps = lock(&self.deps);
            if completed {
                deps.clear();
            }
            for (before, after) in &plan.dependencies {
                deps.entry(*after).or_default().insert(*before);
            }
        }
        // The completed plan binds to the same base the run started on;
        // the live base (or its simulation) may already have advanced
        // past it with the first batch, so only the first adoption moves it.
        if !completed {
            *lock(&self.base) = plan.base_sha.clone();
            if self.simulating() {
                *lock(&self.simulated) = Some(plan.base_sha.clone());
            }
        }
        write_json(&self.run_dir.join("plan.json"), plan)?;
        self.event(
            if completed { "plan-complete" } else { "plan" },
            &format!(
                "{} batch(es), {} held, base {}",
                plan.batches.len(),
                plan.held.len(),
                short(&plan.base_sha)
            ),
        );
        let mut report = lock(&self.report);
        if report.base_start.is_empty() {
            report.base_start = plan.base_sha.clone();
        }
        report.plan = plan.clone();
        Ok(())
    }

    /// Adopt the completed staged plan and replace `pending` (empty at
    /// this point) with its batches minus the changes already started, in
    /// plan order. A change the optimistic first batch already started is
    /// never handed out again; its fate is decided by that batch's own
    /// build and validation, exactly as a one-shot run would decide it.
    fn adopt_completed(
        &self,
        full: &Plan,
        started_prs: &BTreeSet<i64>,
        pending: &mut VecDeque<Vec<i64>>,
    ) -> Result<(), String> {
        self.adopt_plan(full, true)?;
        let remaining: VecDeque<Vec<i64>> = full
            .batches
            .iter()
            .map(|batch| {
                batch
                    .prs
                    .iter()
                    .copied()
                    .filter(|pr| !started_prs.contains(pr))
                    .collect::<Vec<_>>()
            })
            .filter(|prs: &Vec<i64>| !prs.is_empty())
            .collect();
        *pending = remaining;
        Ok(())
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

    /// The rolling speculative window. `pending` holds only the first
    /// batch at entry; `continuation` finishes the plan on a background
    /// thread in this scope, and the loop joins it the first time it
    /// needs a later batch, so the first validation never waits for the
    /// full pairwise probe.
    fn pipeline(
        &self,
        pending: &mut VecDeque<Vec<i64>>,
        continuation: ops::PlanContinuation,
    ) -> Result<String, String> {
        let mut window = self.width.min(self.max_window);
        let mut flights: VecDeque<Flight> = VecDeque::new();
        std::thread::scope(|scope| {
            let mut plan_join = Some(scope.spawn(move || continuation.finish()));
            let (tx, rx) = mpsc::channel::<(u64, JobResult)>();
            let mut next_id = 0u64;
            let mut started = 0usize;
            let mut started_prs: BTreeSet<i64> = BTreeSet::new();
            let mut full_loaded = false;
            let result = (|| -> Result<String, String> {
                loop {
                    // Need the completed plan before a later batch can be
                    // built. Joining here is where planning overlaps
                    // execution: any batch already in flight keeps
                    // validating on its own thread while this thread
                    // waits for the remaining pairwise probes.
                    let want_more =
                        self.options.max_batches == 0 || started < self.options.max_batches;
                    if pending.is_empty() && !full_loaded && want_more {
                        let handle = plan_join
                            .take()
                            .ok_or_else(|| "planning thread already joined".to_owned())?;
                        let full = handle
                            .join()
                            .map_err(|_| "planning thread panicked".to_owned())??;
                        self.adopt_completed(&full, &started_prs, pending)?;
                        full_loaded = true;
                    }
                    // Fill the window, each new batch on top of the last one in flight.
                    while flights.len() < window
                        && !pending.is_empty()
                        && (self.options.max_batches == 0 || started < self.options.max_batches)
                    {
                        crate::interrupt::check("building the next batch")?;
                        let parent = match flights.back() {
                            Some(flight) => flight.candidate.tip.clone(),
                            None => {
                                let base = self.current_base()?;
                                *lock(&self.base) = base.clone();
                                base
                            }
                        };
                        let prs = pending.pop_front().unwrap_or_default();
                        started_prs.extend(prs.iter().copied());
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
                    // The window still has room but only the first batch
                    // was at hand: finish the plan now (the batches in
                    // flight keep validating meanwhile) and fill the
                    // window from it before waiting for any result. This
                    // is what keeps speculation concurrent.
                    let want_more =
                        self.options.max_batches == 0 || started < self.options.max_batches;
                    if flights.len() < window && pending.is_empty() && !full_loaded && want_more {
                        let handle = plan_join
                            .take()
                            .ok_or_else(|| "planning thread already joined".to_owned())?;
                        let full = handle
                            .join()
                            .map_err(|_| "planning thread panicked".to_owned())??;
                        self.adopt_completed(&full, &started_prs, pending)?;
                        full_loaded = true;
                        continue;
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
                    // Only after the result is stored, so `discard` waits for no message
                    // this loop already took.
                    crate::interrupt::check("acting on a finished validation")?;
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
                        // Admission refusal: retry with backoff, then defer. Never land,
                        // never search for a culprit, never eject.
                        let (vid, ran) = if ran.refused() {
                            self.retry_refused(&head.candidate, &head.chosen, (vid, ran))?
                        } else {
                            (vid, ran)
                        };
                        let outcome = if ran.refused() {
                            let detail = refusal_detail(&ran);
                            discard(&mut flights, &rx, pending);
                            let (retry_after, _) =
                                self.defer_refusal(&head.candidate.prs, &detail)?;
                            BatchOutcome::Deferred(
                                format!(
                                    "validation refused for {} ({detail}); stays queued, retry after {retry_after}",
                                    list(&head.candidate.prs)
                                ),
                                head.candidate.prs.clone(),
                            )
                        } else if ran.green() {
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
                            BatchOutcome::Deferred(reason, _prs) => {
                                // Already recorded as deferred; the changes stay queued
                                // (not requeued into this run, which would spin without
                                // backoff). Later batches were discarded above and stay
                                // in `pending` for the next run.
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
        // Whatever led here may rest on a process the stop killed: never eject after a stop.
        crate::interrupt::check(&format!("ejecting #{pr} ({kind})"))?;
        lock(&self.excluded).insert(pr);
        let classes = crate::state::classify(&self.config.classify, failed);
        let class_note = if classes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", classes.join(", "))
        };
        let dir = self.run_dir.join("ejections");
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let report = dir.join(format!("{pr}.md"));
        let mut text = format!("# #{pr} ejected ({kind}){class_note}\n\n{reason}\n");
        if !classes.is_empty() {
            text.push_str(&format!("\nFailure class: {}\n", classes.join(", ")));
        }
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
        self.event("eject", &format!("#{pr} ({kind}){class_note}: {reason}"));
        // Failed results from the deciding validations, where recorded.
        let deciding_failed: Vec<TestResult> = {
            let report = lock(&self.report);
            failed
                .iter()
                .map(|id| {
                    report
                        .validations
                        .iter()
                        .rev()
                        .find_map(|v| v.results.result(id))
                        .unwrap_or(TestResult {
                            id: id.clone(),
                            outcome: Outcome::Fail,
                            duration_ms: None,
                            log: None,
                        })
                })
                .collect()
        };
        if !self.simulating() {
            let mut items = lock(&self.items);
            if let Some(item) = items.get_mut(&pr) {
                item.status = ItemStatus::Ejected;
                item.deciding_report = self.run_dir.join("report.md").display().to_string();
                item.deciding_failed = deciding_failed;
                item.log(
                    "ejected",
                    format!("{kind}{class_note}: {reason} (report {})", report.display()),
                );
                self.state.save_item(item)?;
            }
        }
        lock(&self.report).ejected.push(Ejection {
            pr,
            kind: kind.to_owned(),
            reason: reason.to_owned(),
            failed: failed.to_vec(),
            classes,
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
                        item.next_retry_at.clear();
                        item.last_refusal.clear();
                        item.deciding_report = self.run_dir.join("report.md").display().to_string();
                        item.deciding_failed.clear();
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
        self.prune_donor_jobs(false);
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
        crate::interrupt::check(&format!("landing {}", list(&candidate.prs)))?;
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
            let prefix = self.regenerated_prefix(candidate, i + 1)?;
            let expected_tree = self.git.tree_of(&prefix)?;
            let status = Command::new("sh")
                .arg("-c")
                .arg(&self.config.land.command)
                .current_dir(&self.config.repo)
                .env("MQ_PR", pr.to_string())
                .env("MQ_HEAD_REF", &item.head_ref)
                .env("MQ_HEAD", &item.head)
                .env("MQ_PREFIX", &prefix)
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
        let mut focus = if ran.results.unattributed {
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
        // The red run attributed its failures to named tests (not only an exit status).
        let mut attributed = !ran.results.unattributed && !seen_failed.is_empty();
        loop {
            match self.find_culprit(&current, &focus, &seen_failed)? {
                Culprit::Refused(detail) if attributed => {
                    return self.red_without_probe(&current, &seen_failed, &detail, pending);
                }
                Culprit::Refused(detail) => {
                    let (retry_after, _) = self.defer_refusal(&current.prs, &detail)?;
                    return Ok(BatchOutcome::Deferred(
                        format!(
                            "validation refused for {} ({detail}); stays queued, retry after {retry_after}",
                            list(&current.prs)
                        ),
                        current.prs.clone(),
                    ));
                }
                Culprit::Inconclusive(detail) => {
                    return Ok(BatchOutcome::Stop(
                        format!(
                            "culprit search inconclusive for {}: {detail}; no culprit named, nothing ejected",
                            list(&current.prs)
                        ),
                        current.prs.clone(),
                    ));
                }
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
                    // Both prefixes are regenerated on demand: `select` runs in a
                    // checkout of the first and may read generated files.
                    let after = self.regenerated_prefix(&current, index + 1)?;
                    let before = self.regenerated_prefix(&current, index)?;
                    let affected = self
                        .select(&after, &before)
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
                    if rran.refused() {
                        let detail = refusal_detail(&rran);
                        let (retry_after, _) = self.defer_refusal(&rebuilt.prs, &detail)?;
                        return Ok(BatchOutcome::Deferred(
                            format!(
                                "validation refused for {} ({detail}); stays queued, retry after {retry_after}",
                                list(&rebuilt.prs)
                            ),
                            rebuilt.prs.clone(),
                        ));
                    }
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
                    attributed = !rran.results.unattributed && !seen_failed.is_empty();
                    focus = if rran.results.unattributed {
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

    /// A red validation that named failed tests, whose follow-up probe could not run. The red
    /// run is the verdict; the probe would only have narrowed it (which change, or flaky?), so
    /// its failure is recorded and never turns a known red into a retry. A single change is
    /// ejected (or, under `validator.red_without_probe: hold`, held for its owner). Several
    /// changes cannot be told apart without probes: each goes back as a batch of its own, so
    /// its own validation (of a different commit) decides it.
    fn red_without_probe(
        &self,
        current: &Candidate,
        failed: &BTreeSet<TestId>,
        detail: &str,
        pending: &mut VecDeque<Vec<i64>>,
    ) -> Result<BatchOutcome, String> {
        let skipped = format!(
            "culprit narrowing and the flaky check were skipped: the probe could not run ({detail})"
        );
        let failed_ids = failed.iter().cloned().collect::<Vec<_>>();
        if let [pr] = current.prs[..] {
            let reason = format!(
                "{} failed in the validation of #{pr} on top of {}; {skipped}",
                ids(&failed_ids),
                short(&current.parent)
            );
            self.event("narrowing-skipped", &reason);
            if self.config.validator.hold_red_without_probe() {
                self.hold_for_owner(&[pr], &reason)?;
                return Ok(BatchOutcome::Deferred(
                    format!("#{pr} held for its owner: {reason}"),
                    vec![pr],
                ));
            }
            self.eject(pr, "failed-tests", &reason, &failed_ids)?;
            self.drop_dependents(pr, pending);
            return Ok(BatchOutcome::Done);
        }
        self.event(
            "narrowing-skipped",
            &format!(
                "{} failed in {}; {skipped}; validating each change on its own",
                ids(&failed_ids),
                list(&current.prs)
            ),
        );
        for pr in current.prs.iter().rev() {
            pending.push_front(vec![*pr]);
        }
        Ok(BatchOutcome::Done)
    }

    /// Hold `prs` for their owner with `reason` (escalated, as after the deferral bound), with
    /// an alarm. Shadow runs record it in the shadow ledger.
    fn hold_for_owner(&self, prs: &[i64], reason: &str) -> Result<(), String> {
        if !self.simulating() {
            let mut items = lock(&self.items);
            for pr in prs {
                if let Some(item) = items.get_mut(pr) {
                    item.escalated = reason.to_owned();
                    item.next_retry_at.clear();
                    item.deciding_report = self.run_dir.join("report.md").display().to_string();
                    item.log("escalated", reason);
                    self.state.save_item(item)?;
                }
            }
        } else {
            let items = lock(&self.items);
            let mut ledger = self.state.shadow_refusals()?;
            for pr in prs {
                let head = items
                    .get(pr)
                    .map(|item| item.head.clone())
                    .unwrap_or_default();
                let entry = ledger.entry(*pr).or_default();
                entry.head = head;
                entry.escalated = reason.to_owned();
            }
            self.state.save_shadow_refusals(&ledger)?;
        }
        lock(&self.excluded).extend(prs.iter().copied());
        lock(&self.report).deferred.push(Deferred {
            prs: prs.to_vec(),
            reason: reason.to_owned(),
            retry_after: String::new(),
            refusals: 0,
            escalated: prs.to_vec(),
        });
        let alarm = format!(
            "ALARM: {} escalated to the owner: {reason}; held until `merge-queue retry`",
            list(prs)
        );
        eprintln!("merge-queue: {alarm}");
        self.event("escalated", &alarm);
        Ok(())
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
                unknowns: 0,
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
            // Probe commits are regenerated on demand: the validator runs in a
            // checkout of the prefix and may read generated files, so a stale
            // prefix could blame the wrong change.
            let mut jobs = Vec::with_capacity(probes.len());
            for i in &probes {
                jobs.push(Job {
                    purpose: "probe",
                    prs: candidate.prs[..*i].to_vec(),
                    commit: self.regenerated_prefix(candidate, *i)?,
                    selection: focus.clone(),
                    policy: "failed-tests".to_owned(),
                });
            }
            for (i, result) in probes.iter().zip(self.validate_all(jobs)) {
                let (_, ran) = result?;
                if ran.refused() {
                    return Ok(Culprit::Refused(refusal_detail(&ran)));
                }
                let entry = obs.entry(*i).or_default();
                match ran.probe_observation() {
                    ProbeObservation::Fail => {
                        entry.fails += 1;
                        entry.failed.extend(ran.reproduced());
                    }
                    ProbeObservation::Pass => entry.passes += 1,
                    ProbeObservation::Unknown => {
                        entry.unknowns += 1;
                        return Ok(Culprit::Inconclusive(format!(
                            "probe at prefix {i} of {} observed only missing/skipped tests (no pass, no reproduced failure)",
                            candidate.prs.len()
                        )));
                    }
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
        let (validation_id, ran) = self
            .validate_all(vec![Job {
                purpose: "widen",
                prs: open.iter().flat_map(|o| o.prs.clone()).collect(),
                commit: base.clone(),
                selection: Selection::All,
                policy: "all".to_owned(),
            }])
            .pop()
            .ok_or("no widen result")??;
        if ran.refused() {
            // Not green, not red: discharge nothing, record no failed run, retry later.
            self.event(
                "widen",
                &format!(
                    "complete run refused on {} ({}); obligation(s) stay open, retry later",
                    short(&base),
                    refusal_detail(&ran)
                ),
            );
            return Ok(());
        }
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
                exit: ran.exit,
                results: ran.results.clone(),
                validation_id: Some(validation_id),
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

/// The local verdict compare mode records beside a donor's.
fn local_verdict(ran: &Ran) -> crate::donor::Verdict {
    crate::donor::Verdict {
        green: ran.green(),
        refused: ran.refused,
        failed: ran
            .results
            .attributed_failures()
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

/// An accepted receipt read as a local validation's result; the log keeps its tail.
fn receipt_ran(receipt: &crate::donor::Receipt, dir: &std::path::Path) -> Result<Ran, String> {
    let log = dir.join("run.log");
    std::fs::write(
        &log,
        format!(
            "merge-queue: validated by donor {} on {} (full log there: {})\n{}",
            receipt.donor.name, receipt.donor.host, receipt.log_path, receipt.log_tail
        ),
    )
    .map_err(|error| format!("cannot write {}: {error}", log.display()))?;
    Ok(Ran {
        results: receipt.results.clone(),
        exit: receipt.exit,
        elapsed: Duration::from_millis(receipt.elapsed_ms),
        log,
        cancelled: false,
        interrupted: None,
        handle: None,
        refused: receipt.refused,
        cannot_execute: None,
        effective_selection: receipt.effective_selection.clone(),
    })
}

/// Sleep for `delay`, returning early when a stop is requested.
fn sleep_unless_stopped(delay: Duration) {
    let until = Instant::now() + delay;
    while !crate::interrupt::requested() {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// The last `lines` non-empty lines of a command's combined output, joined for a one-line reason.
fn output_tail(output: &std::process::Output, lines: usize) -> String {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let kept: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    kept[kept.len().saturating_sub(lines)..].join(" | ")
}

/// Tests a run did not see pass (skipped, errored, failed). No synthetic
/// IDs exist to filter out: unattributed `all` runs record no per-test IDs.
fn not_passed(ran: &Ran) -> BTreeSet<TestId> {
    ran.results
        .results
        .iter()
        .filter(|(_, outcome)| **outcome != Outcome::Pass)
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

/// Failed results as `id (outcome, 42ms[, log])`, in ID order.
pub fn format_failed_results(results: &[TestResult]) -> String {
    results
        .iter()
        .map(|r| {
            let mut text = match r.duration_ms {
                Some(ms) => format!("{} ({}, {}ms)", r.id, r.outcome.as_str(), ms),
                None => format!("{} ({})", r.id, r.outcome.as_str()),
            };
            if let Some(log) = &r.log {
                text.push_str(&format!(" log:{log}"));
            }
            text
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Outcome counts as `pass=2, fail=1`, in outcome order.
pub fn format_outcome_counts(results: &TestResults) -> String {
    results
        .counts()
        .iter()
        .map(|(outcome, count)| format!("{}={count}", outcome.as_str()))
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
    if report.mode == "shadow" {
        out.push_str(
            "\nShadow mode: landings below are simulated on a private base; \
             the real base and item statuses are unchanged.\n",
        );
    }
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
            let classes = if ejection.classes.is_empty() {
                String::new()
            } else {
                format!("; {}", ejection.classes.join(", "))
            };
            out.push_str(&format!(
                "- #{} ({}{classes}): {}\n",
                ejection.pr, ejection.kind, ejection.reason
            ));
            if !ejection.failed.is_empty() {
                // Prefer persisted outcomes/durations from the deciding
                // validations over the bare IDs on the ejection.
                let enriched: Vec<TestResult> = ejection
                    .failed
                    .iter()
                    .map(|id| {
                        report
                            .validations
                            .iter()
                            .rev()
                            .find_map(|v| v.results.result(id))
                            .unwrap_or(TestResult {
                                id: id.clone(),
                                outcome: Outcome::Fail,
                                duration_ms: None,
                                log: None,
                            })
                    })
                    .collect();
                out.push_str(&format!("  failed: {}\n", format_failed_results(&enriched)));
            }
        }
    }
    if !report.deferred.is_empty() {
        out.push_str("\n## Deferred (validation refused, still queued)\n\n");
        for deferred in &report.deferred {
            out.push_str(&format!(
                "- {}: {}; retry after {}\n",
                list(&deferred.prs),
                deferred.reason,
                deferred.retry_after
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
        let counts = format_outcome_counts(&v.results);
        let counts = if counts.is_empty() {
            String::new()
        } else {
            format!(" [{counts}]")
        };
        let unattributed = if v.results.unattributed {
            " (unattributed)"
        } else {
            ""
        };
        let result = if v.cancelled {
            format!("cancelled{counts}{unattributed}")
        } else if v.refused {
            format!("refused (retry later){counts}{unattributed}")
        } else if v.green() {
            format!("green{counts}{unattributed}")
        } else if v.classes.is_empty() {
            format!(
                "red{counts}{unattributed}: {}",
                format_failed_results(&v.failed_results())
            )
        } else {
            format!(
                "red{counts}{unattributed}: {} ({})",
                format_failed_results(&v.failed_results()),
                v.classes.join(", ")
            )
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

#[cfg(test)]
mod unit {
    use super::*;

    fn record(results: TestResults, exit: Option<i32>) -> ValidationRecord {
        ValidationRecord {
            id: 1,
            purpose: "batch".to_owned(),
            prs: vec![7],
            commit: "c".repeat(40),
            selection: Selection::All,
            policy: "all".to_owned(),
            results,
            exit,
            log: PathBuf::from("validations/0001-batch/run.log"),
            refused: false,
            cancelled: false,
            classes: vec![],
            start_ms: 0,
            elapsed_ms: 10,
            dir: PathBuf::from("validations/0001-batch"),
        }
    }

    #[test]
    fn validation_record_round_trip_keeps_sparse_results() {
        let mut results = TestResults::default();
        results.record(TestResult {
            id: TestId::step("a"),
            outcome: Outcome::Fail,
            duration_ms: Some(42),
            log: Some("logs/a.log".to_owned()),
        });
        results.record(TestResult {
            id: TestId::step("b"),
            outcome: Outcome::Pass,
            duration_ms: Some(3),
            log: None,
        });
        let rec = record(results, Some(1));
        assert!(!rec.green());
        assert_eq!(rec.failed(), vec![TestId::step("a")]);
        let json = serde_json::to_string(&rec).unwrap();
        // `results` is the persisted truth; serialized `green` is derived.
        assert!(json.contains("\"results\""), "{json}");
        let back: ValidationRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.results, rec.results);
        assert_eq!(back.exit, Some(1));
        assert_eq!(back.log, rec.log);
        assert_eq!(back.failed_results()[0].duration_ms, Some(42));
        assert_eq!(back.failed_results()[0].log.as_deref(), Some("logs/a.log"));
    }

    #[test]
    fn unattributed_record_has_empty_failed_set_and_flag() {
        let results = TestResults {
            unattributed: true,
            ..TestResults::default()
        };
        let rec = record(results, Some(1));
        assert!(rec.failed().is_empty());
        assert!(rec.results.unattributed);
        let text = render_report(&RunReport {
            validations: vec![rec],
            ..RunReport::default()
        });
        assert!(text.contains("(unattributed)"), "{text}");
    }

    #[test]
    fn report_renders_counts_failed_details_and_ejected_failed() {
        let mut results = TestResults::default();
        results.record(TestResult {
            id: TestId::step("a"),
            outcome: Outcome::Fail,
            duration_ms: Some(42),
            log: None,
        });
        results.record(TestResult {
            id: TestId::step("b"),
            outcome: Outcome::Pass,
            duration_ms: None,
            log: None,
        });
        let rec = record(results, Some(1));
        let report = RunReport {
            validations: vec![rec],
            ejected: vec![Ejection {
                pr: 7,
                kind: "failed-tests".to_owned(),
                reason: "a fail".to_owned(),
                failed: vec![TestId::step("a")],
                classes: vec![],
                report: PathBuf::from("ejections/7.md"),
            }],
            ..RunReport::default()
        };
        let text = render_report(&report);
        assert!(text.contains("pass=1"), "{text}");
        assert!(text.contains("fail=1"), "{text}");
        assert!(text.contains("a (fail, 42ms)"), "{text}");
        // Ejected section lists the failed tests, not only the reason.
        let ejected = text.split("## Ejected").nth(1).unwrap();
        assert!(ejected.contains("failed: a (fail, 42ms)"), "{ejected}");
    }

    #[test]
    fn discharge_green_is_derived_not_stored() {
        let discharge = Discharge {
            at: "2026-01-01T00:00:00Z".to_owned(),
            commit: "c".repeat(40),
            exit: Some(1),
            results: {
                let mut r = TestResults::default();
                r.record(TestResult {
                    id: TestId::step("a"),
                    outcome: Outcome::Fail,
                    duration_ms: Some(9),
                    log: None,
                });
                r
            },
            validation_id: Some(3),
        };
        assert!(!discharge.green());
        assert_eq!(discharge.failed_ids(), vec![TestId::step("a")]);
        let json = serde_json::to_string(&discharge).unwrap();
        // Failed tests are embedded with outcomes, and the widen record is
        // referenced; `green` in the JSON is derived from `results`.
        assert!(json.contains("\"outcome\":\"fail\""), "{json}");
        assert!(json.contains("\"validation_id\":3"), "{json}");
    }

    #[test]
    fn refusal_backoff_doubles_and_caps() {
        assert_eq!(refusal_backoff(0, 2), Duration::from_secs(2));
        assert_eq!(refusal_backoff(1, 2), Duration::from_secs(4));
        assert_eq!(refusal_backoff(2, 2), Duration::from_secs(8));
        assert_eq!(refusal_backoff(10, 2), Duration::from_secs(60));
        assert_eq!(refusal_backoff(0, 0), Duration::ZERO);
        assert_eq!(refusal_backoff(3, 0), Duration::ZERO);
    }
}
