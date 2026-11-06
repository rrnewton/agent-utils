//! Running a selection of tests against one exact commit.
//!
//! The interface is a selection in, per-test results out. Two runner kinds are built in:
//!
//! * `command`: a shell template. It reads the selection from `$MQ_TESTS` (space separated) or
//!   `$MQ_TESTS_FILE` (one per line; `$MQ_SELECTION` is `all` or `tests`) and writes one JSON object
//!   per line to `$MQ_RESULTS`: `{"id": "step::case", "outcome": "pass|fail|skip|error",
//!   "duration_ms": 12, "log": "path/to/log"}` (`duration_ms` and `log` are optional; a missing
//!   duration means duration unavailable, not zero). A command that writes no results still
//!   works, but its exit status is then the only signal: every selected test inherits it and
//!   the run is marked unattributed. So is a red run whose results name no failed test, unless
//!   dagrun-style verdict lines in its log name one.
//! * `dagrun`: a dagrun DAG. Test IDs are step names; the selection becomes `--selected`, which
//!   also runs each step's dependencies, and per-step verdicts come from the run journal.
//!
//! Both share the selection policy: a shell command printing test IDs for the checkout at
//! `$MQ_HEAD` relative to `$MQ_BASE`, or `all` when none is configured.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::state::ValidatorConfig;
use crate::tests::{Outcome, Selection, TestId, TestResult, TestResults};

/// Exit status signalling a temporary admission refusal: retry later, never a pass and never a
/// failure of the change under validation (`EX_TEMPFAIL`).
pub const REFUSAL_EXIT: i32 = 75;

/// How the reason for a validator that could not execute begins (see [`Ran::cannot_execute`]).
pub const CANNOT_EXECUTE: &str = "the validator could not execute";

/// Marker printed by an admission wrapper when it refuses to run validation at all.
pub const REFUSAL_MARKER: &str = "COULD_NOT_RUN";

/// Name for a red run that recorded no test result at all, used wherever a failed-test
/// list would otherwise be empty (never `failed-tests:` with nothing after it).
pub const NO_RECORDED_RESULT: &str = "red with no recorded test result";

/// Scope line a validator prints with a refusal: `MQ_COULD_NOT_RUN_SCOPE: host <reason>`
/// or `MQ_COULD_NOT_RUN_SCOPE: job <reason>`. A host-scope refusal is the host's fault,
/// never the change's: it costs the job no strike. No line means job scope.
pub const COULD_NOT_RUN_SCOPE_MARKER: &str = "MQ_COULD_NOT_RUN_SCOPE";

/// Parse a [`COULD_NOT_RUN_SCOPE_MARKER`] line from runner output: `(is_host, reason)`.
pub fn could_not_run_scope(log_text: &str) -> Option<(bool, String)> {
    for line in log_text.lines() {
        let Some(rest) = line.trim().strip_prefix(COULD_NOT_RUN_SCOPE_MARKER) else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        if let Some(reason) = rest.strip_prefix("host") {
            return Some((true, reason.trim().to_owned()));
        }
        if let Some(reason) = rest.strip_prefix("job") {
            return Some((false, reason.trim().to_owned()));
        }
    }
    None
}

/// Environment variable set to `1` when a donor probes its validator after a host-scope
/// refusal: admission checks only, exit 0 means ready to validate again.
pub const PROBE_ENV: &str = "MQ_PROBE";

/// Marker line a validator prints when the run is green only because some tests passed
/// after a retry inside the run (`GREEN_FLAKY: id1,id2`). The queue holds such a batch
/// for a ruling instead of landing it; see [`Ran::flaky_passes`].
pub const FLAKY_MARKER: &str = "GREEN_FLAKY";

/// Marker line a validator prints, with [`INFRA_BLOCKED_EXIT`], when the host refused every
/// measurement of some tests on a run and on its rerun of the same commit
/// (`INFRA-BLOCKED: id1,id2`): the commit has no result for them, so it is neither green nor
/// red. The queue holds such a change for a ruling, as for GREEN_FLAKY; see
/// [`Ran::infra_blocked`].
pub const INFRA_BLOCKED_MARKER: &str = "INFRA-BLOCKED";

/// Exit status that goes with [`INFRA_BLOCKED_MARKER`].
pub const INFRA_BLOCKED_EXIT: i32 = 6;

/// The verdict of a hold while a change's suspect tests run again beside the run (see
/// [`Ran::suspect_only`] and [`crate::suspect`]).
pub const SUSPECT_FLAKE_MARKER: &str = "SUSPECT-FLAKE";

/// Result-row shapes (`shape`) that make a failure suspect: a timeout, or a run that reached no
/// verdict. Neither is an assertion the change failed.
const SUSPECT_SHAPES: [&str; 2] = ["timeout", "no-verdict"];

/// Marker line a validator prints on a red run to name the failed steps that are deterministic
/// checks (`RERUN-EXEMPT: step1,step2`): their answer depends only on the tree, so a rerun of the
/// same commit cannot change it. When every failure of the run is in a named step, the queue
/// counts the red as confirmed on that one observation instead of spending a rerun on it; see
/// [`Ran::deterministic_red`].
pub const RERUN_EXEMPT_MARKER: &str = "RERUN-EXEMPT";

/// Environment variable set to `1` for a candidate validation when `validator.fail_fast` is on:
/// the runner may stop at the first red step and report the steps it skipped as `skip`.
pub const FAIL_FAST_ENV: &str = "MQ_FAIL_FAST";

/// Environment variable set to `1` for a validation (and its selection) whose batch is
/// only peripheral changes: the runner's selection may then be the focused one.
pub const PERIPHERAL_ENV: &str = "MQ_PERIPHERAL";

/// Environment variable set to `1` when the queue watches the run's streamed verdicts and
/// stops it at the first deterministic failure (see [`Ran::early_red`]): the runner opts in
/// by appending each verdict as it happens and declaring its deterministic steps early
/// with a `RERUN-EXEMPT:` line. A runner that does neither keeps the post-run behaviour.
pub const EARLY_RED_ENV: &str = "MQ_EARLY_RED";

/// The attributed failures of `results` that belong to a step named in `exempt`: the
/// failures a rerun of the same commit cannot change.
pub fn deterministic_failures(results: &TestResults, exempt: &[TestId]) -> BTreeSet<TestId> {
    results
        .attributed_failures()
        .into_iter()
        .filter(|test| exempt.iter().any(|step| step.step == test.step))
        .collect()
}

/// Tests a green run names as passed only after a retry, read from its `GREEN_FLAKY:`
/// marker line. Only a zero exit can be green-flaky: a red run's marker text (a test
/// printing it, say) means nothing.
pub fn flaky_passes(exit: Option<i32>, log_text: &str) -> Vec<TestId> {
    if exit != Some(0) {
        return Vec::new();
    }
    marker_ids(log_text, FLAKY_MARKER)
}

/// Tests a run names as INFRA-BLOCKED, read from its `INFRA-BLOCKED:` marker line. Only
/// [`INFRA_BLOCKED_EXIT`] can be INFRA-BLOCKED: the marker text in any other run means nothing.
pub fn infra_blocked(exit: Option<i32>, log_text: &str) -> Vec<TestId> {
    if exit != Some(INFRA_BLOCKED_EXIT) {
        return Vec::new();
    }
    marker_ids(log_text, INFRA_BLOCKED_MARKER)
}

/// Steps a red run names as deterministic checks, read from its `RERUN-EXEMPT:` marker line.
/// Only a red exit can carry it: in a green run, a refusal (exit 75), an INFRA-BLOCKED run
/// (exit 6) or a run killed without an exit status, the marker text means nothing.
pub fn rerun_exempt(exit: Option<i32>, log_text: &str) -> Vec<TestId> {
    if red_exit(exit) {
        marker_ids(log_text, RERUN_EXEMPT_MARKER)
    } else {
        Vec::new()
    }
}

/// Whether `exit` says the run is red: non-zero, and neither a refusal (exit 75) nor an
/// INFRA-BLOCKED run (exit 6). A run killed without an exit status is not counted.
fn red_exit(exit: Option<i32>) -> bool {
    matches!(exit, Some(code) if code != 0 && code != REFUSAL_EXIT && code != INFRA_BLOCKED_EXIT)
}

/// Whether a finished run is a red with no recorded test result: red, not cancelled or
/// refused, and its result set is empty (no result lines, no verdict lines, nothing
/// ran). For an `all` selection that is an empty map; for a test selection the exit-only
/// synthesis records only `Error`/`Missing` under `unattributed`, which is equally no
/// recorded result. A run that reported passes, skips or a named failure recorded
/// something, so it is not empty.
pub fn is_empty_red(
    exit: Option<i32>,
    cancelled: bool,
    refused: bool,
    results: &TestResults,
) -> bool {
    if cancelled || refused || !red_exit(exit) {
        return false;
    }
    results.results.is_empty()
        || (results.unattributed
            && results
                .results
                .values()
                .all(|outcome| matches!(outcome, Outcome::Error | Outcome::Missing)))
}

/// Cut a could-not-run detail to about 300 bytes, on a char boundary.
fn truncate_detail(detail: &str) -> String {
    if detail.len() <= 300 {
        return detail.to_owned();
    }
    let mut end = 300;
    while end > 0 && !detail.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &detail[..end])
}

/// A red run whose results name no failed test has lost its attribution, since a results file
/// can drop a failure. When the runner also prints dagrun-style verdict lines, a step they show
/// failed is recorded failed, unless the results reported it passed. A red run that names no
/// failure anywhere proves only its exit status, so it is marked unattributed: it then covers
/// nothing, and a culprit search reruns its whole selection.
fn name_the_failures_of_a_red_run(results: &mut TestResults, log_text: &str) {
    let logged = parse_step_lines(log_text);
    let mut named = false;
    for (id, outcome) in &logged.results {
        if *outcome == Outcome::Fail && results.results.get(id) != Some(&Outcome::Pass) {
            results.results.insert(id.clone(), Outcome::Fail);
            if let Some(ms) = logged.durations_ms.get(id) {
                results.durations_ms.insert(id.clone(), *ms);
            }
            named = true;
        }
    }
    if !named {
        results.unattributed = true;
    }
}

/// The test IDs on every `MARKER: id1,id2` line of `log_text`.
fn marker_ids(log_text: &str, marker: &str) -> Vec<TestId> {
    let mut found = Vec::new();
    for line in log_text.lines() {
        let Some(rest) = line.trim().strip_prefix(marker) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        for raw in rest.split([',', ' ']) {
            if let Ok(id) = TestId::parse(raw.trim()) {
                found.push(id);
            }
        }
    }
    found
}

/// Whether an exit status and combined runner output describe an admission refusal rather
/// than a validation result. A refusal means validation did not run: it must not be treated
/// as green, must not be treated as red, and must not attribute any test failure.
///
/// The marker counts only as the first non-empty output line of a run that exited non-zero
/// and attributed no failed test: a genuinely red run whose output merely mentions the marker
/// (a test printing it, say) is a failure, not a refusal, or the change would be deferred
/// forever instead of ejected.
pub fn is_refusal(exit: Option<i32>, log_text: &str, attributed_failures: bool) -> bool {
    if exit == Some(REFUSAL_EXIT) {
        return true;
    }
    if exit == Some(0) || attributed_failures {
        return false;
    }
    log_text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .is_some_and(|first| first.starts_with(REFUSAL_MARKER))
}

/// One finished validation.
#[derive(Clone, Debug)]
pub struct Ran {
    /// Per-test results, with every selected-but-unreported test counted as an error.
    pub results: TestResults,
    /// Runner exit status (`None` when killed by the timeout).
    pub exit: Option<i32>,
    /// Wall time.
    pub elapsed: Duration,
    /// Combined runner output.
    pub log: PathBuf,
    /// The queue cancelled the run (its candidate was discarded); its results mean nothing.
    pub cancelled: bool,
    /// Why the run says nothing about the change because the queue is stopping or an outside
    /// signal killed the runner (see [`crate::interrupt`]). Always also `cancelled`; the caller
    /// must stop with no verdict rather than read it as red.
    pub interrupted: Option<String>,
    /// Opaque handle the runner wrote to `$MQ_HANDLE_FILE`, if any; passed back as
    /// `$MQ_RERUN_FROM` to a later validation of the same commit.
    pub handle: Option<String>,
    /// Admission refused to run validation at all (exit 75 or `COULD_NOT_RUN`), or the
    /// validator could not execute: validation did not run, so this is neither green nor red.
    pub refused: bool,
    /// Tests that passed only after a retry inside this run (the validator's
    /// `GREEN_FLAKY:` marker). A green run with any of these is held for a ruling
    /// instead of landing.
    pub flaky_passes: Vec<TestId>,
    /// Tests the host refused to measure on this run and on its rerun of the same commit (the
    /// validator's `INFRA-BLOCKED:` marker, exit 6). A run with any of these is neither green
    /// nor red: its change is held for a ruling.
    pub infra_blocked: Vec<TestId>,
    /// Failed tests whose result row says the failure was a timeout or reached no verdict
    /// (`shape`), not an assertion. See [`Ran::suspect_only`].
    pub suspect: Vec<TestId>,
    /// Steps the validator names as deterministic checks on this red run (its `RERUN-EXEMPT:`
    /// marker): a rerun of the same commit cannot change their answer. See
    /// [`Ran::deterministic_red`].
    pub rerun_exempt: Vec<TestId>,
    /// Why the validator could not execute (it could not start, or exited 126 or 127 without
    /// reporting a failed test), naming the command and `PATH`. Always also `refused`.
    pub cannot_execute: Option<String>,
    /// Effective selection actually run, when the runner collapses the
    /// requested one (dagrun runs whole steps, so case IDs collapse to
    /// their steps). `None` means the requested selection ran as-is.
    pub effective_selection: Option<Selection>,
    /// CPU time (user plus system) of the runner and the descendants it waited for, when it
    /// could be read (Linux `/proc`, a runner that exited by itself); `None` otherwise.
    pub cpu: Option<Duration>,
    /// The donor that ran it (`NAME@HOST`); `None` for a validation run here.
    pub donor: Option<String>,
    /// The queue stopped this run early at its first deterministic failure, streamed by the
    /// runner before it exited (see [`EARLY_RED_ENV`]). Its results are the verdicts seen
    /// so far; unreported tests carry no outcome. Never also `cancelled`.
    pub early_red: bool,
}

/// What one culprit-search probe actually observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeObservation {
    /// At least one test passed and nothing reproduced a failure.
    Pass,
    /// A failure reproduced (attributed, or a non-zero exit).
    Fail,
    /// Nothing conclusive was observed: only `Missing`/`Skip` outcomes, no
    /// results, a refusal, or a cancellation. Never counted as a pass.
    Unknown,
}

impl Ran {
    /// Whether admission refused to run validation at all. A refusal is neither a pass nor
    /// a failure: the change stays queued and validation is retried later.
    pub fn refused(&self) -> bool {
        self.refused
    }

    /// Whether the run is green: every result non-blocking and the runner exited zero.
    /// A refusal is never green.
    pub fn green(&self) -> bool {
        !self.cancelled && !self.refused && self.exit == Some(0) && self.results.green()
    }

    /// Whether this run is a red that recorded no test result at all (see [`is_empty_red`]):
    /// nothing ran, so it can name no culprit.
    pub fn is_empty_red(&self) -> bool {
        is_empty_red(self.exit, self.cancelled, self.refused, &self.results)
    }

    /// Whether this run reproduces a failure, for a culprit search: some test actually failed
    /// or errored, or the runner exited non-zero. Tests that were merely unreported do not
    /// count (they may not exist at that commit). A refusal never reproduces a failure.
    pub fn reproduces(&self) -> bool {
        !self.cancelled
            && !self.refused
            && (!self.results.attributed_failures().is_empty() || self.exit != Some(0))
    }

    /// The failures a culprit search attributes to this run. An unattributed
    /// red run attributes nothing (its `results.unattributed` flag, not a
    /// synthetic test ID, carries that fact).
    pub fn reproduced(&self) -> BTreeSet<TestId> {
        self.results.attributed_failures()
    }

    /// Whether this red run is confirmed without a second observation: it attributed its
    /// failures to named tests, and every one of them belongs to a step the validator named in
    /// its `RERUN-EXEMPT:` marker. A red run with no attributed failure, or with any failure
    /// outside those steps, still needs its failure seen twice.
    pub fn deterministic_red(&self) -> bool {
        if !self.reproduces() || self.rerun_exempt.is_empty() {
            return false;
        }
        let failed = self.reproduced();
        !failed.is_empty() && deterministic_failures(&self.results, &self.rerun_exempt) == failed
    }

    /// The failed tests, when this red run attributed its failures and every one is suspect (a
    /// timeout or no verdict, as the validator shaped it): what a suspect-flake rerun confirms
    /// before the change is blamed. `None` for any other run.
    pub fn suspect_only(&self) -> Option<BTreeSet<TestId>> {
        if self.cancelled || self.refused || self.results.unattributed {
            return None;
        }
        let failed = self.failed();
        (!failed.is_empty() && failed.iter().all(|test| self.suspect.contains(test)))
            .then_some(failed)
    }

    /// Failed tests. A red run with no attributed failure reports none;
    /// unattributed runs are identified by `results.unattributed`.
    /// A refusal or a cancelled run reports no failed tests.
    pub fn failed(&self) -> BTreeSet<TestId> {
        if self.refused || self.cancelled {
            return BTreeSet::new();
        }
        self.results.failed()
    }

    /// Honest probe observation: `Missing`/`Skip`-only (or empty) results
    /// are `Unknown`, never a pass. An empty red (no recorded test result)
    /// observed nothing, so it is `Unknown` too: it can name no culprit.
    pub fn probe_observation(&self) -> ProbeObservation {
        if self.cancelled || self.refused || self.is_empty_red() {
            return ProbeObservation::Unknown;
        }
        if self.reproduces() {
            return ProbeObservation::Fail;
        }
        if self
            .results
            .results
            .values()
            .any(|outcome| *outcome == Outcome::Pass)
        {
            ProbeObservation::Pass
        } else {
            ProbeObservation::Unknown
        }
    }
}

/// The selection a policy chose, and the policy's name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chosen {
    /// Selected tests.
    pub selection: Selection,
    /// Policy name.
    pub policy: String,
    /// Tests the policy asks for beyond the complete suite, with [`Selection::All`]: a select
    /// command that prints `all` and then test IDs. The runner gets them in
    /// [`EXTRA_TESTS_ENV`]. Empty with any other selection.
    pub extra: BTreeSet<TestId>,
}

/// Environment variable naming the tests a validation of the complete suite runs beyond it (see
/// [`Chosen::extra`]), space separated. Set only when there are some.
pub const EXTRA_TESTS_ENV: &str = "MQ_EXTRA_TESTS";

/// A configured validator.
pub trait Validator: Sync {
    /// Ask the selection policy which tests the change from `base` to the checkout's HEAD needs.
    fn select(&self, checkout: &Path, base: &str, head: &str) -> Result<Chosen, String>;

    /// [`Validator::select`] for a batch of only peripheral changes: the select command
    /// gets `$MQ_PERIPHERAL=1` and may answer the focused selection.
    fn select_with(
        &self,
        checkout: &Path,
        base: &str,
        head: &str,
        peripheral: bool,
    ) -> Result<Chosen, String> {
        let _ = peripheral;
        self.select(checkout, base, head)
    }
    /// Run `selection` in `checkout`, writing logs under `out`. Setting `cancel` stops the run
    /// early (its process group is killed) and marks the result cancelled.
    fn run(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String>;

    /// [`Validator::run`] with extra environment variables for the runner (for example
    /// `MQ_RERUN_FROM`). Runners that take no extra context ignore them.
    fn run_with(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
        extra: &[(String, String)],
    ) -> Result<Ran, String> {
        let _ = extra;
        self.run(checkout, selection, out, cancel)
    }
}

/// File, inside a validation's directory, where a runner may write an opaque handle for its run.
pub const HANDLE_FILE: &str = "handle.txt";

fn read_handle(out: &Path) -> Option<String> {
    std::fs::read_to_string(out.join(HANDLE_FILE))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Build the validator a configuration names.
pub fn from_config(config: &ValidatorConfig) -> Result<Box<dyn Validator>, String> {
    match config.kind.as_str() {
        "command" => {
            if config.run.trim().is_empty() {
                return Err("validator.kind is `command` but validator.run is empty".to_owned());
            }
            Ok(Box::new(CommandValidator(config.clone())))
        }
        "dagrun" => {
            if config.dag.trim().is_empty() {
                return Err("validator.kind is `dagrun` but validator.dag is empty".to_owned());
            }
            Ok(Box::new(DagrunValidator(config.clone())))
        }
        "" => Err("no validator configured: set validator.kind in config.json".to_owned()),
        other => Err(format!(
            "unknown validator.kind {other:?}; use `command` or `dagrun`"
        )),
    }
}

/// A select command's output: its selection, and with `all` the IDs it names beyond the
/// complete suite.
fn parse_ids(text: &str, prefix: &str) -> Result<(Selection, BTreeSet<TestId>), String> {
    let mut tests = BTreeSet::new();
    let mut all = false;
    for line in text.lines() {
        let line = line.trim();
        let Some(id) = line.strip_prefix(prefix) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || id.starts_with('#') {
            continue;
        }
        if id == "all" {
            all = true;
            continue;
        }
        tests.insert(TestId::parse(id)?);
    }
    Ok(if all {
        (Selection::All, tests)
    } else {
        (Selection::Tests(tests), BTreeSet::new())
    })
}

fn shared_select(
    config: &ValidatorConfig,
    checkout: &Path,
    base: &str,
    head: &str,
) -> Result<Chosen, String> {
    shared_select_peripheral(config, checkout, base, head, false)
}

fn shared_select_peripheral(
    config: &ValidatorConfig,
    checkout: &Path,
    base: &str,
    head: &str,
    peripheral: bool,
) -> Result<Chosen, String> {
    if config.select.trim().is_empty() {
        return Ok(Chosen {
            selection: Selection::All,
            policy: "all".to_owned(),
            extra: BTreeSet::new(),
        });
    }
    let output = Command::new("sh")
        .arg("-c")
        .arg(&config.select)
        .current_dir(checkout)
        .env("MQ_BASE", base)
        .env("MQ_HEAD", head)
        .env(PERIPHERAL_ENV, if peripheral { "1" } else { "0" })
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run validator.select: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "validator.select failed ({}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let (selection, extra) =
        parse_ids(&String::from_utf8_lossy(&output.stdout), &config.id_prefix)?;
    let policy = if config.policy.is_empty() {
        "select-command".to_owned()
    } else {
        config.policy.clone()
    };
    Ok(Chosen {
        selection,
        policy,
        extra,
    })
}

/// Files a running validation streams its verdicts into, watched for an early deterministic
/// failure. A runner that never writes them until it exits is never stopped early.
struct EarlyRedFiles {
    /// `$MQ_RESULTS` of a `command` runner, when it has one.
    results: Option<PathBuf>,
    /// The dagrun journal of a `dagrun` runner, when it has one.
    journal: Option<PathBuf>,
    /// Combined runner output (verdict lines and the `RERUN-EXEMPT:` declaration).
    log: PathBuf,
}

impl EarlyRedFiles {
    /// The deterministic failures streamed so far, if any: a reported failure in a step the
    /// runner has already declared `RERUN-EXEMPT:`.
    fn streamed_failures(&self) -> BTreeSet<TestId> {
        let log_text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let exempt = marker_ids(&log_text, RERUN_EXEMPT_MARKER);
        if exempt.is_empty() {
            return BTreeSet::new();
        }
        let mut results = TestResults::default();
        if let Some(path) = &self.results {
            if let Ok(text) = std::fs::read_to_string(path) {
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    // A line still being written does not parse; the next poll reads it whole.
                    let Ok(parsed) = serde_json::from_str::<ResultLine>(line) else {
                        continue;
                    };
                    let (Ok(id), Some(outcome)) =
                        (TestId::parse(&parsed.id), Outcome::parse(&parsed.outcome))
                    else {
                        continue;
                    };
                    results.record(TestResult {
                        id,
                        outcome,
                        duration_ms: parsed.duration_ms,
                        log: parsed.log,
                    });
                }
            }
        }
        if let Some(path) = &self.journal {
            let journal = std::fs::read_to_string(path).unwrap_or_default();
            for (id, outcome) in parse_journal(&journal).results {
                results.results.insert(id, outcome);
            }
        }
        for (id, outcome) in parse_step_lines(&log_text).results {
            results.results.entry(id).or_insert(outcome);
        }
        deterministic_failures(&results, &exempt)
    }
}

/// How a runner process ended.
struct Finished {
    exit: Option<i32>,
    elapsed: Duration,
    /// The queue cancelled it, or it was interrupted.
    cancelled: bool,
    /// The queue stopped it early at a streamed deterministic failure.
    early_red: bool,
    /// Set when a stop request or an outside kill signal ended it (see [`Ran::interrupted`]).
    interrupted: Option<String>,
    /// The runner could not be started at all (no such file, not executable).
    spawn_error: Option<String>,
    /// CPU time of the runner and the descendants it waited for (see [`exited_cpu`]).
    cpu: Option<Duration>,
}

/// Clock ticks per second in `/proc/<pid>/stat`: `USER_HZ`, 100 on every Linux ABI.
const USER_HZ: u64 = 100;

/// CPU time (user plus system, its own and its waited-for descendants') of process `pid`, read
/// from `/proc/<pid>/stat` while it is a zombie: it has exited and not been reaped yet, so the
/// accounting is final. `None` while it still runs, or where there is no `/proc`.
fn exited_cpu(pid: u32) -> Option<Duration> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the parenthesised command name, which may itself contain spaces.
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    if fields.first() != Some(&"Z") {
        return None;
    }
    // utime, stime, cutime and cstime are fields 14 to 17 of the whole line.
    let ticks: u64 = fields
        .get(11..15)?
        .iter()
        .map(|field| field.parse::<u64>().ok())
        .sum::<Option<u64>>()?;
    Some(Duration::from_millis(ticks * 1000 / USER_HZ))
}

/// The command a validation runs: the admission prefix, then the runner's argv.
fn full_argv(config: &ValidatorConfig, argv: &[String]) -> Vec<String> {
    config
        .admission
        .iter()
        .cloned()
        .chain(argv.iter().cloned())
        .collect()
}

/// Why a validation that could not execute says nothing about the change, or `None` when it
/// ran. It could not execute when the runner failed to start, or exited 126 (a command was
/// found but could not run) or 127 (a command was not found, the shell's answer when the
/// validator or a tool it calls is missing from `PATH`) without reporting any failed test
/// itself. The reason names the command and the `PATH` it ran with, for the operator.
fn cannot_execute(
    config: &ValidatorConfig,
    argv: &[String],
    finished: &Finished,
    log_text: &str,
    reported_failures: bool,
) -> Option<String> {
    let what = match (&finished.spawn_error, finished.exit) {
        _ if finished.cancelled => return None,
        (Some(error), _) => error.clone(),
        (None, Some(127)) if !reported_failures => "exit 127: a command was not found".to_owned(),
        (None, Some(126)) if !reported_failures => {
            "exit 126: a command was found but could not be executed".to_owned()
        }
        _ => return None,
    };
    let mut detail = format!(
        "{CANNOT_EXECUTE} ({what}): command {}; PATH={}",
        shell_words::join(full_argv(config, argv)),
        std::env::var("PATH").unwrap_or_default()
    );
    if finished.spawn_error.is_none() {
        if let Some(last) = log_text
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
        {
            let cut = last
                .char_indices()
                .nth(200)
                .map_or(last, |(end, _)| &last[..end]);
            detail.push_str(&format!("; last output: {cut}"));
        }
    }
    Some(detail)
}

/// Run a shell command under the admission prefix with a timeout; output goes to `log`.
///
/// A stop request cancels the runner like a discarded candidate does; a runner that died of
/// an outside kill signal, or that finished while a stop was under way, is interrupted rather
/// than red.
/// Stop a runner's whole process group and note why in its log.
fn stop_group(child: &mut std::process::Child, log: &Path, why: &str) {
    // Quiet: `kill(1)`'s complaints would land in the queue's own log, not the run's.
    let _ = Command::new("kill")
        .args(["-TERM", "--", &format!("-{}", child.id())])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let grace = Instant::now();
    while grace.elapsed() < Duration::from_secs(2) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The group is usually gone by now; whatever is left of it is killed without a
    // "No such process" line when nothing is.
    crate::bounded::kill_group(child);
    if let Ok(mut log) = std::fs::OpenOptions::new().append(true).open(log) {
        let _ = writeln!(log, "merge-queue: validation {why}");
    }
}

fn run_logged(
    config: &ValidatorConfig,
    argv: &[String],
    checkout: &Path,
    env: &[(&str, String)],
    log: &Path,
    cancel: &AtomicBool,
    early: Option<&EarlyRedFiles>,
) -> Result<Finished, String> {
    let file = std::fs::File::create(log)
        .map_err(|error| format!("cannot create {}: {error}", log.display()))?;
    let err = file
        .try_clone()
        .map_err(|error| format!("cannot clone log handle: {error}"))?;
    let full = full_argv(config, argv);
    let mut command = Command::new(&full[0]);
    command
        .args(&full[1..])
        .current_dir(checkout)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err);
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let error = format!("cannot start {}: {error}", full[0]);
            if let Ok(mut log) = std::fs::OpenOptions::new().append(true).open(log) {
                let _ = writeln!(log, "merge-queue: {error}");
            }
            return Ok(Finished {
                exit: None,
                elapsed: started.elapsed(),
                cancelled: false,
                early_red: false,
                interrupted: None,
                spawn_error: Some(error),
                cpu: None,
            });
        }
    };
    let limit = (config.timeout_secs > 0).then(|| Duration::from_secs(config.timeout_secs));
    loop {
        // Read before reaping: once `try_wait` reaps the runner its accounting is gone.
        let cpu = exited_cpu(child.id());
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for validation: {error}"))?
        {
            let interrupted = crate::interrupt::outside_kill(&status)
                .map(|signal| crate::interrupt::killed("the validation runner", signal))
                .or_else(|| crate::interrupt::check("this validation's result was read").err());
            return Ok(Finished {
                exit: status.code(),
                elapsed: started.elapsed(),
                cancelled: interrupted.is_some(),
                early_red: false,
                interrupted,
                spawn_error: None,
                cpu,
            });
        }
        let stopping = crate::interrupt::requested();
        let cancelled = cancel.load(Ordering::Relaxed) || stopping;
        let timed_out = limit.is_some_and(|limit| started.elapsed() > limit);
        if !cancelled && !timed_out {
            if let Some(files) = early {
                let failures = files.streamed_failures();
                if !failures.is_empty() {
                    let names: Vec<String> = failures.iter().map(ToString::to_string).collect();
                    stop_group(
                        &mut child,
                        log,
                        &format!(
                            "stopped early: deterministic failure streamed for {}",
                            names.join(", ")
                        ),
                    );
                    return Ok(Finished {
                        exit: None,
                        elapsed: started.elapsed(),
                        cancelled: false,
                        early_red: true,
                        interrupted: None,
                        spawn_error: None,
                        cpu: None,
                    });
                }
            }
        }
        if cancelled || timed_out {
            let why = if stopping {
                "cancelled: the queue is stopping".to_owned()
            } else if cancelled {
                "cancelled by the queue".to_owned()
            } else {
                format!("killed after {}s", config.timeout_secs)
            };
            stop_group(&mut child, log, &why);
            return Ok(Finished {
                exit: None,
                elapsed: started.elapsed(),
                cancelled,
                early_red: false,
                interrupted: stopping
                    .then(|| crate::interrupt::check("this validation finished").unwrap_err()),
                spawn_error: None,
                cpu: None,
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A shell-template runner with a JSON-lines results contract.
pub struct CommandValidator(pub ValidatorConfig);

/// One `$MQ_RESULTS` line: `(id, outcome, duration_ms, log, shape)`.
///
/// Contract: `id` is `step` or `step::case` with no whitespace or `#` in
/// either part (see [`TestId::parse`]); `duration_ms`, `log` and `shape` are
/// optional, and a missing duration means unavailable, not zero. `shape` says how a
/// failed test failed when it was not an assertion: `timeout` or `no-verdict`.
#[derive(Deserialize)]
struct ResultLine {
    id: String,
    outcome: String,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    log: Option<String>,
    #[serde(default)]
    shape: Option<String>,
}

impl Validator for CommandValidator {
    fn select(&self, checkout: &Path, base: &str, head: &str) -> Result<Chosen, String> {
        shared_select(&self.0, checkout, base, head)
    }

    fn select_with(
        &self,
        checkout: &Path,
        base: &str,
        head: &str,
        peripheral: bool,
    ) -> Result<Chosen, String> {
        shared_select_peripheral(&self.0, checkout, base, head, peripheral)
    }

    fn run(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String> {
        self.run_with(checkout, selection, out, cancel, &[])
    }

    fn run_with(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
        extra: &[(String, String)],
    ) -> Result<Ran, String> {
        std::fs::create_dir_all(out)
            .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
        let results_path = out.join("results.jsonl");
        let tests_path = out.join("tests.txt");
        let _ = std::fs::remove_file(&results_path);
        let listed: Vec<String> = selection
            .tests()
            .map(|tests| tests.iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        std::fs::write(
            &tests_path,
            listed
                .iter()
                .map(|id| format!("{id}\n"))
                .collect::<String>(),
        )
        .map_err(|error| format!("cannot write {}: {error}", tests_path.display()))?;
        let _ = std::fs::remove_file(out.join(HANDLE_FILE));
        let mut env: Vec<(&str, String)> = vec![
            (
                "MQ_SELECTION",
                if selection.is_all() { "all" } else { "tests" }.to_owned(),
            ),
            ("MQ_TESTS", listed.join(" ")),
            ("MQ_TESTS_FILE", tests_path.display().to_string()),
            ("MQ_RESULTS", results_path.display().to_string()),
            (
                "MQ_HANDLE_FILE",
                out.join(HANDLE_FILE).display().to_string(),
            ),
        ];
        env.extend(
            extra
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone())),
        );
        if self.0.early_red_enabled() {
            env.push((EARLY_RED_ENV, "1".to_owned()));
        }
        let log = out.join("run.log");
        let argv = vec!["sh".to_owned(), "-c".to_owned(), self.0.run.clone()];
        let early_files = self.0.early_red_enabled().then(|| EarlyRedFiles {
            results: Some(results_path.clone()),
            journal: None,
            log: log.clone(),
        });
        let finished = run_logged(
            &self.0,
            &argv,
            checkout,
            &env,
            &log,
            cancel,
            early_files.as_ref(),
        )?;
        let mut results = TestResults::default();
        let mut suspect: BTreeSet<TestId> = BTreeSet::new();
        // A row the queue cannot parse is not an error that stops the run:
        // it makes this validation could-not-run (host scope), with the rows
        // that did parse kept for diagnosis and no verdict from any of them.
        let mut bad_row: Option<String> = None;
        if let Ok(text) = std::fs::read_to_string(&results_path) {
            for (index, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let line_no = index + 1;
                let parsed: ResultLine = match serde_json::from_str(line) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        bad_row.get_or_insert_with(|| {
                            truncate_detail(&format!(
                                "bad line in {} line {line_no}: {error}: {line}",
                                results_path.display()
                            ))
                        });
                        continue;
                    }
                };
                let Some(outcome) = Outcome::parse(&parsed.outcome) else {
                    bad_row.get_or_insert_with(|| {
                        truncate_detail(&format!(
                            "unknown outcome {:?} in {} line {line_no}",
                            parsed.outcome,
                            results_path.display()
                        ))
                    });
                    continue;
                };
                let id = match TestId::parse(&parsed.id) {
                    Ok(id) => id,
                    Err(error) => {
                        bad_row.get_or_insert_with(|| {
                            truncate_detail(&format!(
                                "bad line in {} line {line_no}: {error}",
                                results_path.display()
                            ))
                        });
                        continue;
                    }
                };
                if outcome == Outcome::Fail
                    && parsed
                        .shape
                        .as_deref()
                        .is_some_and(|shape| SUSPECT_SHAPES.contains(&shape))
                {
                    suspect.insert(id.clone());
                }
                results.record(TestResult {
                    id,
                    outcome,
                    duration_ms: parsed.duration_ms,
                    log: parsed.log,
                });
            }
        }
        if results.results.is_empty() && bad_row.is_none() {
            // No results file: a runner that prints dagrun-style verdict lines still attributes.
            results = parse_step_lines(&std::fs::read_to_string(&log).unwrap_or_default());
        }
        // Failures the runner reported itself, before unreported tests inherit its exit status.
        let reported_failures = !results.attributed_failures().is_empty();
        let Finished {
            exit,
            elapsed,
            cancelled,
            ..
        } = finished;
        let early_red = finished.early_red;
        let interrupted = finished.interrupted.clone();
        if early_red {
            // Stopped at the first streamed deterministic failure: the verdicts seen so far
            // are the result; unreported tests carry no outcome and are not required.
            let log_text = std::fs::read_to_string(&log).unwrap_or_default();
            return Ok(Ran {
                results,
                exit: None,
                elapsed,
                log: log.clone(),
                cancelled: false,
                interrupted: None,
                handle: read_handle(out),
                refused: false,
                flaky_passes: Vec::new(),
                infra_blocked: Vec::new(),
                rerun_exempt: marker_ids(&log_text, RERUN_EXEMPT_MARKER),
                suspect: suspect.into_iter().collect(),
                cannot_execute: None,
                effective_selection: None,
                cpu: None,
                donor: None,
                early_red: true,
            });
        }
        if results.results.is_empty() {
            results.unattributed = true;
            let outcome = if exit == Some(0) {
                Outcome::Pass
            } else {
                Outcome::Error
            };
            // No synthetic test ID for an `all` selection: the unattributed
            // flag carries the fact and the failed set stays empty.
            if let Some(tests) = selection.tests() {
                for id in tests.iter().cloned() {
                    results.record(TestResult {
                        id,
                        outcome,
                        duration_ms: None,
                        log: None,
                    });
                }
            }
        }
        results.require(selection);
        let log_text = std::fs::read_to_string(&log).unwrap_or_default();
        let mut cannot_execute =
            cannot_execute(&self.0, &argv, &finished, &log_text, reported_failures);
        let mut refused = cannot_execute.is_some()
            || (!cancelled && is_refusal(exit, &log_text, reported_failures));
        // With `validator.empty_red = could-not-run`, a red that recorded no test
        // result is a refusal on the existing could-not-run path: nothing ran, so
        // there is no verdict against the change.
        let mut empty_refused = false;
        if !refused
            && self.0.empty_red_could_not_run()
            && is_empty_red(exit, cancelled, false, &results)
        {
            refused = true;
            empty_refused = true;
        }
        // An unparseable results row is could-not-run with host scope: the
        // adapter produced output the queue cannot judge, so no parsed row
        // is a verdict. Parsed rows stay in `results` for diagnosis; the
        // scope line in the log carries the host scope to donor receipts.
        let bad_refused = bad_row.is_some() && !cancelled;
        if bad_refused {
            refused = true;
            if cannot_execute.is_none() {
                if let Some(detail) = &bad_row {
                    cannot_execute = Some(format!("could not run: {detail}"));
                }
            }
            if let Some(detail) = &bad_row {
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&log) {
                    use std::io::Write as _;
                    let _ = writeln!(file, "{COULD_NOT_RUN_SCOPE_MARKER}: host {detail}");
                }
            }
        }
        if refused && !bad_refused {
            // Validation did not run: keep no test verdict from this attempt. An
            // empty-red refusal keeps the unattributed flag so its reason can be
            // named (see `refusal_detail` in `run`). A bad-row refusal keeps
            // its parsed rows above, for diagnosis.
            results = TestResults::default();
            results.unattributed = empty_refused;
        } else if !refused
            && !cancelled
            && !reported_failures
            && !results.unattributed
            && red_exit(exit)
        {
            name_the_failures_of_a_red_run(&mut results, &log_text);
        }
        Ok(Ran {
            results,
            exit,
            elapsed,
            log,
            cancelled,
            interrupted,
            handle: read_handle(out),
            refused,
            flaky_passes: if refused {
                Vec::new()
            } else {
                flaky_passes(exit, &log_text)
            },
            infra_blocked: if refused {
                Vec::new()
            } else {
                infra_blocked(exit, &log_text)
            },
            rerun_exempt: if refused {
                Vec::new()
            } else {
                rerun_exempt(exit, &log_text)
            },
            suspect: if refused {
                Vec::new()
            } else {
                suspect.into_iter().collect()
            },
            cannot_execute,
            effective_selection: None,
            cpu: finished.cpu,
            donor: None,
            early_red: false,
        })
    }
}

/// A dagrun DAG runner: steps are tests.
pub struct DagrunValidator(pub ValidatorConfig);

#[derive(Deserialize)]
struct JournalLine {
    #[serde(default)]
    event: String,
    #[serde(default)]
    step: String,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    wall_elapsed_s: Option<String>,
    #[serde(default)]
    log: Option<String>,
}

/// Create `dir` readable only by its owner; dagrun writes its journal only into a private,
/// owned directory.
fn private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot make {} private: {error}", dir.display()))?;
    }
    Ok(())
}

/// Duration in a dagrun verdict's trailing `(...)` group, e.g. `(1s)`,
/// `(3s, exit 1)` or `(250ms)`. `None` when the verdict carries no duration
/// (duration unavailable, not zero).
fn verdict_duration_ms(verdict: &str) -> Option<u64> {
    let group = verdict.rsplit('(').next()?.split(')').next()?;
    let first = group.split(',').next()?.trim();
    if let Some(ms) = first.strip_suffix("ms") {
        ms.trim().parse::<u64>().ok()
    } else if let Some(secs) = first.strip_suffix('s') {
        secs.trim()
            .parse::<f64>()
            .ok()
            .map(|s| (s * 1000.0).round() as u64)
    } else {
        None
    }
}

/// Per-step results from dagrun's printed verdict lines: `[step] ✓ PASS ...`,
/// `[step] ✗ FAIL ...`, `[step] SKIPPED ...`.
///
/// Step names go through [`TestId::parse`]; lines whose step is not a valid
/// ID are skipped. Verdict-line durations are recorded where present. A failed
/// step's captured output, printed between `[step] ----- detail -----` and
/// `[step] ----- end detail -----` with every line tagged with the step, is
/// whatever the step printed, so none of it is read as a verdict.
pub fn parse_step_lines(text: &str) -> TestResults {
    let mut results = TestResults::default();
    let mut in_detail: BTreeSet<&str> = BTreeSet::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((step, verdict)) = rest.split_once("] ") else {
            continue;
        };
        match verdict.trim_end() {
            "----- detail -----" => {
                in_detail.insert(step);
                continue;
            }
            "----- end detail -----" => {
                in_detail.remove(step);
                continue;
            }
            _ if in_detail.contains(step) => continue,
            _ => {}
        }
        let Ok(id) = TestId::parse(step) else {
            continue;
        };
        let outcome = if verdict.starts_with("✓ PASS") {
            Outcome::Pass
        } else if verdict.starts_with("✗ FAIL") {
            Outcome::Fail
        } else if verdict.starts_with("SKIPPED") {
            Outcome::Skip
        } else {
            continue;
        };
        results.record(TestResult {
            id,
            outcome,
            duration_ms: verdict_duration_ms(verdict),
            log: None,
        });
    }
    results
}

/// Per-step results from a dagrun `journal.jsonl`.
pub fn parse_journal(text: &str) -> TestResults {
    let mut results = TestResults::default();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<JournalLine>(line) else {
            continue;
        };
        let Ok(id) = TestId::parse(&record.step) else {
            continue;
        };
        let outcome = match (record.event.as_str(), record.ok) {
            ("step_end", Some(true)) => Outcome::Pass,
            ("step_end", _) => Outcome::Fail,
            ("step_skip", _) => Outcome::Skip,
            _ => continue,
        };
        let duration_ms = record
            .wall_elapsed_s
            .and_then(|secs| secs.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0).round() as u64);
        results.record(TestResult {
            id,
            outcome,
            duration_ms,
            log: record.log,
        });
    }
    results
}

impl Validator for DagrunValidator {
    fn select(&self, checkout: &Path, base: &str, head: &str) -> Result<Chosen, String> {
        shared_select(&self.0, checkout, base, head)
    }

    fn select_with(
        &self,
        checkout: &Path,
        base: &str,
        head: &str,
        peripheral: bool,
    ) -> Result<Chosen, String> {
        shared_select_peripheral(&self.0, checkout, base, head, peripheral)
    }

    fn run(
        &self,
        checkout: &Path,
        selection: &Selection,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Ran, String> {
        let journal_dir = out.join("dagrun-log");
        let _ = std::fs::remove_dir_all(&journal_dir);
        private_dir(&journal_dir)?;
        let dagrun = if self.0.dagrun.is_empty() {
            "dagrun".to_owned()
        } else {
            self.0.dagrun.clone()
        };
        let mut argv = vec![
            dagrun,
            "run".to_owned(),
            "--dag".to_owned(),
            self.0.dag.clone(),
            "--keep-going".to_owned(),
        ];
        if let Some(tests) = selection.tests() {
            // dagrun selects whole steps; a case-level ID narrows nothing here.
            let steps: BTreeSet<&str> = tests.iter().map(|test| test.step.as_str()).collect();
            if steps.is_empty() {
                return Ok(Ran {
                    results: TestResults::default(),
                    exit: Some(0),
                    elapsed: Duration::ZERO,
                    log: out.join("run.log"),
                    cancelled: false,
                    interrupted: None,
                    handle: None,
                    refused: false,
                    flaky_passes: Vec::new(),
                    infra_blocked: Vec::new(),
                    rerun_exempt: Vec::new(),
                    suspect: Vec::new(),
                    cannot_execute: None,
                    effective_selection: Some(Selection::default()),
                    cpu: None,
                    donor: None,
                    early_red: false,
                });
            }
            argv.push("--selected".to_owned());
            argv.push(steps.into_iter().collect::<Vec<_>>().join(","));
        }
        let env = [
            ("DAGRUN_LOG_DIR", journal_dir.display().to_string()),
            (
                EARLY_RED_ENV,
                if self.0.early_red_enabled() {
                    "1".to_owned()
                } else {
                    "0".to_owned()
                },
            ),
        ];
        let log = out.join("run.log");
        let early_files = self.0.early_red_enabled().then(|| EarlyRedFiles {
            results: None,
            journal: Some(journal_dir.join("journal.jsonl")),
            log: log.clone(),
        });
        let finished = run_logged(
            &self.0,
            &argv,
            checkout,
            &env,
            &log,
            cancel,
            early_files.as_ref(),
        )?;
        let Finished {
            exit,
            elapsed,
            cancelled,
            ..
        } = finished;
        let early_red = finished.early_red;
        let interrupted = finished.interrupted.clone();
        let journal =
            std::fs::read_to_string(journal_dir.join("journal.jsonl")).unwrap_or_default();
        let mut results = parse_journal(&journal);
        if results.results.is_empty() {
            // No journal (dagrun disables it when it distrusts the directory): read the verdict
            // lines it prints instead.
            results = parse_step_lines(&std::fs::read_to_string(&log).unwrap_or_default());
        }
        // The effective selection dagrun ran (and a record must name) is
        // the requested one collapsed to whole steps.
        let whole_steps = selection
            .tests()
            .map(|tests| Selection::of(tests.iter().map(TestId::whole_step)));
        let reported_failures = !results.attributed_failures().is_empty();
        if early_red {
            let log_text = std::fs::read_to_string(&log).unwrap_or_default();
            return Ok(Ran {
                results,
                exit: None,
                elapsed,
                log: log.clone(),
                cancelled: false,
                interrupted: None,
                handle: read_handle(out),
                refused: false,
                flaky_passes: Vec::new(),
                infra_blocked: Vec::new(),
                rerun_exempt: marker_ids(&log_text, RERUN_EXEMPT_MARKER),
                suspect: Vec::new(),
                cannot_execute: None,
                effective_selection: whole_steps,
                cpu: None,
                donor: None,
                early_red: true,
            });
        }
        results.require(whole_steps.as_ref().unwrap_or(selection));
        let log_text = std::fs::read_to_string(&log).unwrap_or_default();
        let cannot_execute =
            cannot_execute(&self.0, &argv, &finished, &log_text, reported_failures);
        let mut refused = cannot_execute.is_some()
            || (!cancelled && is_refusal(exit, &log_text, reported_failures));
        let mut empty_refused = false;
        if !refused
            && self.0.empty_red_could_not_run()
            && is_empty_red(exit, cancelled, false, &results)
        {
            refused = true;
            empty_refused = true;
        }
        if refused {
            results = TestResults::default();
            results.unattributed = empty_refused;
        }
        Ok(Ran {
            results,
            exit,
            elapsed,
            log,
            cancelled,
            interrupted,
            handle: read_handle(out),
            refused,
            flaky_passes: if refused {
                Vec::new()
            } else {
                flaky_passes(exit, &log_text)
            },
            infra_blocked: if refused {
                Vec::new()
            } else {
                infra_blocked(exit, &log_text)
            },
            rerun_exempt: if refused {
                Vec::new()
            } else {
                rerun_exempt(exit, &log_text)
            },
            suspect: Vec::new(),
            cannot_execute,
            effective_selection: whole_steps,
            cpu: finished.cpu,
            donor: None,
            early_red: false,
        })
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::git::testrepo::TempDir;

    fn config(run: &str) -> ValidatorConfig {
        ValidatorConfig {
            kind: "command".into(),
            run: run.into(),
            ..ValidatorConfig::default()
        }
    }

    #[test]
    fn a_validator_that_cannot_execute_is_refused_naming_the_command_and_path() {
        let tmp = TempDir::new("validate-exec");
        let never = AtomicBool::new(false);
        let tests = Selection::of([TestId::step("a")]);
        // The shell cannot find the command (exit 127), with or without a test selection.
        for selection in [Selection::All, tests.clone()] {
            let v = from_config(&config("/nonexistent/mq-validator --run")).unwrap();
            let ran = v
                .run(&tmp.0, &selection, &tmp.0.join("missing"), &never)
                .unwrap();
            assert!(ran.refused() && !ran.green() && ran.failed().is_empty());
            let why = ran.cannot_execute.unwrap();
            assert!(why.starts_with(CANNOT_EXECUTE), "{why}");
            assert!(why.contains("exit 127") && why.contains("/nonexistent/mq-validator"));
            assert!(why.contains("PATH="), "{why}");
        }
        // Found but not executable (exit 126).
        std::fs::write(tmp.0.join("plain.sh"), "exit 0\n").unwrap();
        let v = from_config(&config("./plain.sh")).unwrap();
        let ran = v
            .run(&tmp.0, &Selection::All, &tmp.0.join("plain"), &never)
            .unwrap();
        assert!(ran.cannot_execute.unwrap().contains("exit 126"));
        // An admission prefix that does not exist never starts.
        let mut admitted = config("exit 0");
        admitted.admission = vec!["/nonexistent/gate".to_owned()];
        let ran = from_config(&admitted)
            .unwrap()
            .run(&tmp.0, &Selection::All, &tmp.0.join("gate"), &never)
            .unwrap();
        let why = ran.cannot_execute.unwrap();
        assert!(why.contains("cannot start /nonexistent/gate"), "{why}");
        // Nor does a dagrun launcher that is not there.
        let dagrun = ValidatorConfig {
            kind: "dagrun".into(),
            dagrun: "/nonexistent/dagrun".into(),
            dag: "validation.dag.yaml".into(),
            ..ValidatorConfig::default()
        };
        let ran = from_config(&dagrun)
            .unwrap()
            .run(&tmp.0, &tests, &tmp.0.join("dagrun"), &never)
            .unwrap();
        assert!(ran.refused() && ran.failed().is_empty());
        assert!(ran.cannot_execute.unwrap().contains("/nonexistent/dagrun"));
    }

    #[test]
    fn exit_127_with_a_reported_failure_is_red() {
        let tmp = TempDir::new("validate-exec-red");
        let never = AtomicBool::new(false);
        let v = from_config(&config(
            r#"echo '{"id":"a","outcome":"fail"}' >> "$MQ_RESULTS"; exit 127"#,
        ))
        .unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("a")]),
                &tmp.0.join("out"),
                &never,
            )
            .unwrap();
        assert!(ran.cannot_execute.is_none() && !ran.refused());
        assert_eq!(ran.failed(), BTreeSet::from([TestId::step("a")]));
    }

    #[test]
    fn command_results_contract_and_missing_tests() {
        let tmp = TempDir::new("validate");
        let v = from_config(&config(
            r#"for t in $MQ_TESTS; do case $t in bad*) o=fail;; *) o=pass;; esac; echo "{\"id\":\"$t\",\"outcome\":\"$o\"}" >> "$MQ_RESULTS"; done; grep -q bad "$MQ_TESTS_FILE" && exit 1; exit 0"#,
        ))
        .unwrap();
        let sel = Selection::of(["a", "bad1"].map(TestId::step));
        let ran = v
            .run(&tmp.0, &sel, &tmp.0.join("out1"), &AtomicBool::new(false))
            .unwrap();
        assert!(!ran.green());
        assert_eq!(ran.failed(), [TestId::step("bad1")].into());
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("a")]),
                &tmp.0.join("out2"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.green());
    }

    #[test]
    fn exit_status_only_runner_is_unattributed() {
        let tmp = TempDir::new("validate-exit");
        let v = from_config(&config("exit 3")).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.results.unattributed);
        assert!(
            ran.failed().is_empty(),
            "an unattributed run has an empty failed set; the flag carries it"
        );
        let ok = from_config(&config("true")).unwrap();
        assert!(ok
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("p"),
                &AtomicBool::new(false)
            )
            .unwrap()
            .green());
    }

    #[test]
    fn empty_red_is_could_not_run_only_when_configured() {
        let tmp = TempDir::new("validate-empty-red");
        // Default (`red`): an empty red is a red with no recorded result, and
        // as a probe it observes nothing.
        let ran = from_config(&config("exit 1"))
            .unwrap()
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("red"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.is_empty_red() && !ran.refused() && !ran.green());
        assert!(ran.failed().is_empty());
        assert_eq!(ran.probe_observation(), ProbeObservation::Unknown);
        // `could-not-run`: the same run is a refusal on the existing path.
        let mut cfg = config("exit 1");
        cfg.empty_red = "could-not-run".into();
        let ran = from_config(&cfg)
            .unwrap()
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("cnr"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.refused() && !ran.green() && ran.failed().is_empty());
        // A red that recorded a failure is red under both settings.
        let mut cfg = config(r#"echo '{"id":"a","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1"#);
        cfg.empty_red = "could-not-run".into();
        let ran = from_config(&cfg)
            .unwrap()
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("named"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(!ran.refused() && !ran.is_empty_red());
        assert_eq!(ran.failed(), [TestId::step("a")].into());
        assert_eq!(ran.probe_observation(), ProbeObservation::Fail);
    }

    #[test]
    fn an_unparseable_result_row_is_could_not_run_not_an_error() {
        let tmp = TempDir::new("validate-bad-row");
        // Bad JSON, unknown outcome, invalid id, and a good row beside a bad
        // one: every case is a refusal, never Err, never red or green, with
        // the parsed rows kept for diagnosis.
        for (tag, script) in [
            (
                "overflow",
                r#"echo '{"id":"x","outcome":"fail","duration_ms":-1}' >> "$MQ_RESULTS"; exit 1"#,
            ),
            (
                "outcome",
                r#"echo '{"id":"x","outcome":"bogus"}' >> "$MQ_RESULTS"; exit 1"#,
            ),
            (
                "badid",
                r#"echo '{"id":"::x","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1"#,
            ),
            (
                "mixed",
                r#"echo '{"id":"t0","outcome":"fail"}' >> "$MQ_RESULTS"; echo 'not json' >> "$MQ_RESULTS"; exit 1"#,
            ),
        ] {
            let ran = from_config(&config(script))
                .unwrap()
                .run(
                    &tmp.0,
                    &Selection::All,
                    &tmp.0.join(tag),
                    &AtomicBool::new(false),
                )
                .unwrap();
            assert!(ran.refused() && !ran.green(), "{tag}: {ran:?}");
            assert!(ran.failed().is_empty(), "{tag}");
            let why = ran.cannot_execute.as_deref().unwrap_or_default();
            assert!(why.contains("could not run"), "{tag}: {why}");
            assert!(
                why.contains("line 1") || why.contains("line 2"),
                "{tag}: {why}"
            );
            assert!(why.len() <= 340, "{tag}: {why}");
            let log = std::fs::read_to_string(tmp.0.join(tag).join("run.log")).unwrap();
            assert!(log.contains("MQ_COULD_NOT_RUN_SCOPE: host"), "{tag}: {log}");
            if tag == "mixed" {
                // The good fail row is kept for diagnosis but is no verdict.
                assert_eq!(
                    ran.results.results.get(&TestId::step("t0")),
                    Some(&Outcome::Fail),
                    "{ran:?}"
                );
            }
        }
    }

    #[test]
    fn timeout_kills_and_reports_errors() {
        let tmp = TempDir::new("validate-timeout");
        let mut cfg = config("sleep 30");
        cfg.timeout_secs = 1;
        let v = from_config(&cfg).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("x")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(ran.exit, None);
        assert!(!ran.green());
        assert!(ran.elapsed < Duration::from_secs(10));
        assert!(!ran.cancelled);
    }

    #[test]
    fn cancel_stops_a_running_validation() {
        let tmp = TempDir::new("validate-cancel");
        let v = from_config(&config("sleep 30")).unwrap();
        let cancel = AtomicBool::new(false);
        let ran = std::thread::scope(|scope| {
            let handle = scope.spawn(|| v.run(&tmp.0, &Selection::All, &tmp.0.join("o"), &cancel));
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::Relaxed);
            handle.join().unwrap().unwrap()
        });
        assert!(ran.cancelled);
        assert!(!ran.green());
        assert!(ran.elapsed < Duration::from_secs(10));
    }

    #[test]
    fn a_streamed_deterministic_failure_stops_the_run_early() {
        let tmp = TempDir::new("validate-early-red");
        let v = from_config(&config(
            r#"echo 'RERUN-EXEMPT: check.unit'; echo '{"id":"check.unit::a","outcome":"fail"}' >> "$MQ_RESULTS"; sleep 30"#,
        ))
        .unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("check.unit"), TestId::step("slow")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.early_red, "{ran:?}");
        assert!(!ran.cancelled && !ran.green() && !ran.refused());
        assert!(ran.deterministic_red(), "{ran:?}");
        assert_eq!(
            ran.failed(),
            [TestId::parse("check.unit::a").unwrap()].into()
        );
        assert!(ran.elapsed < Duration::from_secs(10), "{ran:?}");
    }

    #[test]
    fn early_red_and_fail_fast_together_stop_the_run_early() {
        let tmp = TempDir::new("validate-early-red-fail-fast");
        let v = from_config(&config(
            r#"echo "ff=$MQ_FAIL_FAST er=$MQ_EARLY_RED" > env-seen.txt; echo 'RERUN-EXEMPT: check.unit'; echo '{"id":"check.unit::a","outcome":"fail"}' >> "$MQ_RESULTS"; sleep 30"#,
        ))
        .unwrap();
        let ran = v
            .run_with(
                &tmp.0,
                &Selection::of([TestId::step("check.unit")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
                &[(FAIL_FAST_ENV.to_owned(), "1".to_owned())],
            )
            .unwrap();
        assert!(ran.early_red && ran.deterministic_red(), "{ran:?}");
        assert!(ran.elapsed < Duration::from_secs(10), "{ran:?}");
        let seen = std::fs::read_to_string(tmp.0.join("env-seen.txt")).unwrap();
        assert_eq!(seen.trim(), "ff=1 er=1", "{seen}");
    }

    #[test]
    fn a_streamed_failure_outside_the_declared_steps_never_stops_the_run() {
        let tmp = TempDir::new("validate-early-red-nonexempt");
        let v = from_config(&config(
            r#"echo 'RERUN-EXEMPT: check.unit'; echo '{"id":"flaky::a","outcome":"fail"}' >> "$MQ_RESULTS"; sleep 2; exit 1"#,
        ))
        .unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("check.unit"), TestId::step("flaky")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(!ran.early_red, "{ran:?}");
        assert!(!ran.deterministic_red(), "{ran:?}");
        assert!(ran.elapsed >= Duration::from_secs(2), "{ran:?}");
        assert!(
            ran.failed().contains(&TestId::parse("flaky::a").unwrap()),
            "{ran:?}"
        );
    }

    #[test]
    fn a_streamed_pass_never_stops_the_run() {
        let tmp = TempDir::new("validate-early-red-pass");
        let mut cfg = config(
            r#"echo 'RERUN-EXEMPT: check.unit'; echo '{"id":"check.unit::a","outcome":"pass"}' >> "$MQ_RESULTS"; sleep 30"#,
        );
        cfg.timeout_secs = 1;
        let v = from_config(&cfg).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("check.unit")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(!ran.early_red && !ran.deterministic_red(), "{ran:?}");
    }

    #[test]
    fn a_dagrun_journal_deterministic_failure_stops_the_run_early() {
        let tmp = TempDir::new("validate-early-red-dagrun");
        let script = tmp.0.join("fake-dagrun");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             echo '{\"event\":\"step_end\",\"step\":\"check.unit\",\"ok\":false}' >> \"$DAGRUN_LOG_DIR/journal.jsonl\"\n\
             echo 'RERUN-EXEMPT: check.unit'\n\
             sleep 30\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = ValidatorConfig {
            kind: "dagrun".into(),
            dagrun: script.display().to_string(),
            dag: "validation.dag.yaml".into(),
            ..ValidatorConfig::default()
        };
        let v = from_config(&cfg).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("check.unit"), TestId::step("slow")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.early_red, "{ran:?}");
        assert!(ran.deterministic_red(), "{ran:?}");
        assert_eq!(ran.failed(), [TestId::step("check.unit")].into());
        assert!(ran.elapsed < Duration::from_secs(10), "{ran:?}");
    }

    #[test]
    fn select_command_with_prefix() {
        let tmp = TempDir::new("validate-select");
        let mut cfg = config("true");
        cfg.select = "echo RUN group; echo STEP a.b; echo STEP c.d::e; echo base=$MQ_BASE".into();
        cfg.id_prefix = "STEP ".into();
        cfg.policy = "path-routing".into();
        let chosen = from_config(&cfg).unwrap().select(&tmp.0, "B", "H").unwrap();
        assert_eq!(chosen.policy, "path-routing");
        assert_eq!(
            chosen.selection,
            Selection::of([TestId::step("a.b"), TestId::parse("c.d::e").unwrap()])
        );
        assert!(chosen.extra.is_empty());
        cfg.select.clear();
        assert_eq!(
            from_config(&cfg)
                .unwrap()
                .select(&tmp.0, "B", "H")
                .unwrap()
                .selection,
            Selection::All
        );
    }

    #[test]
    fn select_all_with_ids_asks_for_those_tests_beyond_the_suite() {
        let tmp = TempDir::new("validate-select-extra");
        let mut cfg = config("true");
        cfg.select = "echo super.b; echo all; echo super.a".into();
        let chosen = from_config(&cfg).unwrap().select(&tmp.0, "B", "H").unwrap();
        assert_eq!(chosen.selection, Selection::All);
        assert_eq!(
            chosen.extra,
            [TestId::step("super.a"), TestId::step("super.b")].into()
        );
        // Without `all` the IDs are the selection itself, and nothing is beyond it.
        cfg.select = "echo super.a".into();
        let chosen = from_config(&cfg).unwrap().select(&tmp.0, "B", "H").unwrap();
        assert_eq!(chosen.selection, Selection::of([TestId::step("super.a")]));
        assert!(chosen.extra.is_empty());
    }

    #[test]
    fn command_runner_printing_verdict_lines_is_attributed_and_handles_round_trip() {
        let tmp = TempDir::new("validate-lines");
        let v = from_config(&config(
            r#"echo "[unit.a] ✓ PASS   a (1s)"; echo "[unit.b] ✗ FAIL   b (2s, exit 1)"; echo "run-42" > "$MQ_HANDLE_FILE"; echo "from=$MQ_RERUN_FROM"; exit 1"#,
        ))
        .unwrap();
        let extra = vec![("MQ_RERUN_FROM".to_owned(), "run-41".to_owned())];
        let ran = v
            .run_with(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
                &extra,
            )
            .unwrap();
        assert!(!ran.results.unattributed);
        assert_eq!(ran.failed(), [TestId::step("unit.b")].into());
        assert_eq!(ran.handle.as_deref(), Some("run-42"));
        let log = std::fs::read_to_string(tmp.0.join("o/run.log")).unwrap();
        assert!(log.contains("from=run-41"), "{log}");
    }

    #[test]
    fn verdict_line_fallback() {
        let text = "[scheduler] WARNING: evidence directory is not private\n\
[a.b] ▶ START  something\n\
[a.b] ✓ PASS   something (0s)  [ok]\n\
[c.d] ✗ FAIL   other (3s, exit 1)\n\
[e.f] SKIPPED reason=unsupported\n\
dagrun: PASS - 1 passed";
        let results = parse_step_lines(text);
        assert_eq!(results.results.len(), 3);
        assert_eq!(results.results[&TestId::step("a.b")], Outcome::Pass);
        assert_eq!(results.failed(), [TestId::step("c.d")].into());
        assert_eq!(results.results[&TestId::step("e.f")], Outcome::Skip);
        // Verdict-line durations are recorded where present.
        assert_eq!(results.durations_ms[&TestId::step("a.b")], 0);
        assert_eq!(results.durations_ms[&TestId::step("c.d")], 3000);
        assert!(!results.durations_ms.contains_key(&TestId::step("e.f")));
    }

    /// A failed step prints what it captured after its verdict, each line tagged with the step:
    /// a nested run's own lines there are not verdicts of the step.
    const SELF_TEST_FAIL: &str = "[self.test] ▶ START  driver self-test\n\
[self.test] ✗ FAIL   driver self-test (71s, exit 2)\n\
[self.test] ----- detail -----\n\
[self.test] [inner.step] ✓ PASS   nested (0s)\n\
[self.test] ✓ PASS   a line the self-test printed\n\
[self.test] SKIPPED reason=a line the self-test printed\n\
[self.test] ----- end detail -----\n\
[other.step] ✓ PASS   other (2s)\n";

    #[test]
    fn a_failed_steps_captured_output_is_never_a_verdict() {
        let results = parse_step_lines(SELF_TEST_FAIL);
        assert_eq!(results.results.len(), 2, "{:?}", results.results);
        assert_eq!(results.results[&TestId::step("self.test")], Outcome::Fail);
        assert_eq!(results.durations_ms[&TestId::step("self.test")], 71_000);
        assert_eq!(results.results[&TestId::step("other.step")], Outcome::Pass);
    }

    /// A runner that reports `self.test` skipped in its results file and exits red, the way a
    /// results writer that misread the log above did.
    fn lost_failure_runner(log: &str, results: &[(&str, &str)]) -> String {
        let mut script = format!("printf '%s' '{log}'\n");
        for (id, outcome) in results {
            script.push_str(&format!(
                "echo '{{\"id\":\"{id}\",\"outcome\":\"{outcome}\"}}' >> \"$MQ_RESULTS\"\n"
            ));
        }
        script.push_str("exit 4\n");
        script
    }

    #[test]
    fn a_red_run_whose_results_name_no_failure_reads_the_verdict_lines() {
        let tmp = TempDir::new("validate-lost-failure");
        let v = from_config(&config(&lost_failure_runner(
            SELF_TEST_FAIL,
            &[("self.test", "skip"), ("other.step", "pass")],
        )))
        .unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(!ran.results.unattributed);
        assert_eq!(ran.failed(), BTreeSet::from([TestId::step("self.test")]));
        assert_eq!(
            ran.results.durations_ms.get(&TestId::step("self.test")),
            Some(&71_000)
        );
    }

    #[test]
    fn a_red_run_that_names_no_failure_anywhere_is_unattributed() {
        let tmp = TempDir::new("validate-no-failure");
        // The results say the step passed, so a FAIL line for it is an attempt a retry
        // superseded, not the failure: the run names none.
        let v = from_config(&config(&lost_failure_runner(
            "[a.b] ✗ FAIL   a (1s, exit 1)\n[a.b] ✓ PASS   a (1s)\n",
            &[("a.b", "pass")],
        )))
        .unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.results.unattributed);
        assert!(ran.failed().is_empty());
        assert_eq!(ran.results.results[&TestId::step("a.b")], Outcome::Pass);
        // A run held as INFRA-BLOCKED, or refused, is not red: it is left as reported.
        for exit in [INFRA_BLOCKED_EXIT, REFUSAL_EXIT] {
            let held = from_config(&config(&format!(
                "echo '{{\"id\":\"a.b\",\"outcome\":\"skip\"}}' >> \"$MQ_RESULTS\"; exit {exit}"
            )))
            .unwrap()
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join(format!("h{exit}")),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(!held.results.unattributed, "exit {exit}");
        }
    }

    #[test]
    fn flaky_marker_only_counts_on_a_zero_exit() {
        let log = "noise\nGREEN_FLAKY: test.a,test.b::case\nmore\n";
        assert_eq!(
            flaky_passes(Some(0), log),
            vec![
                TestId::step("test.a"),
                TestId::parse("test.b::case").unwrap()
            ]
        );
        assert!(flaky_passes(Some(1), log).is_empty());
        assert!(flaky_passes(None, log).is_empty());
        assert!(flaky_passes(Some(0), "GREEN_FLAKY no colon\n").is_empty());
    }

    #[test]
    fn infra_blocked_marker_only_counts_with_its_exit() {
        let log = "noise\nINFRA-BLOCKED: e2e.manifest_c_programs_buck\nmore\n";
        assert_eq!(
            infra_blocked(Some(INFRA_BLOCKED_EXIT), log),
            vec![TestId::step("e2e.manifest_c_programs_buck")]
        );
        // A red or green run that prints the text is not INFRA-BLOCKED.
        assert!(infra_blocked(Some(1), log).is_empty());
        assert!(infra_blocked(Some(0), log).is_empty());
        assert!(infra_blocked(None, log).is_empty());
        // Nor is a GREEN_FLAKY line, nor the marker without its colon.
        assert!(infra_blocked(Some(INFRA_BLOCKED_EXIT), "GREEN_FLAKY: a\n").is_empty());
        assert!(infra_blocked(Some(INFRA_BLOCKED_EXIT), "INFRA-BLOCKED a\n").is_empty());
    }

    #[test]
    fn rerun_exempt_marker_only_counts_on_a_red_exit() {
        let log = "noise\nRERUN-EXEMPT: check.paths,check.names\nmore\n";
        assert_eq!(
            rerun_exempt(Some(1), log),
            vec![TestId::step("check.paths"), TestId::step("check.names")]
        );
        // A green run, a refusal, an INFRA-BLOCKED run or a killed one says nothing by it.
        assert!(rerun_exempt(Some(0), log).is_empty());
        assert!(rerun_exempt(Some(REFUSAL_EXIT), log).is_empty());
        assert!(rerun_exempt(Some(INFRA_BLOCKED_EXIT), log).is_empty());
        assert!(rerun_exempt(None, log).is_empty());
        assert!(rerun_exempt(Some(1), "RERUN-EXEMPT check.paths\n").is_empty());
    }

    /// A finished run with these outcomes, exit status and `RERUN-EXEMPT:` steps.
    fn ran_with(outcomes: &[(&str, Outcome)], exit: Option<i32>, exempt: &[&str]) -> Ran {
        let mut results = TestResults::default();
        for (id, outcome) in outcomes {
            results.record(TestResult {
                id: TestId::parse(id).unwrap(),
                outcome: *outcome,
                duration_ms: None,
                log: None,
            });
        }
        results.unattributed = outcomes.is_empty();
        Ran {
            results,
            exit,
            elapsed: Duration::ZERO,
            log: PathBuf::new(),
            cancelled: false,
            interrupted: None,
            handle: None,
            refused: false,
            flaky_passes: Vec::new(),
            infra_blocked: Vec::new(),
            rerun_exempt: exempt.iter().map(|step| TestId::step(*step)).collect(),
            suspect: Vec::new(),
            cannot_execute: None,
            effective_selection: None,
            cpu: None,
            donor: None,
            early_red: false,
        }
    }

    #[test]
    fn a_red_is_deterministic_only_when_every_failure_is_in_a_named_step() {
        let fail = Outcome::Fail;
        let pass = Outcome::Pass;
        // Every failure is a named check, whole step or a case inside it.
        assert!(ran_with(
            &[("check.paths", fail), ("unit", pass)],
            Some(1),
            &["check.paths"]
        )
        .deterministic_red());
        assert!(
            ran_with(&[("check.paths::a", fail)], Some(1), &["check.paths"]).deterministic_red()
        );
        // A failure outside the named steps needs its second sighting.
        assert!(!ran_with(
            &[("check.paths", fail), ("unit", fail)],
            Some(1),
            &["check.paths"]
        )
        .deterministic_red());
        // No marker, an unattributed red, or a green run: not deterministic.
        assert!(!ran_with(&[("check.paths", fail)], Some(1), &[]).deterministic_red());
        assert!(!ran_with(&[], Some(1), &["check.paths"]).deterministic_red());
        assert!(!ran_with(&[("check.paths", pass)], Some(0), &["check.paths"]).deterministic_red());
        // A refused or cancelled run reproduces nothing.
        let mut refused = ran_with(&[("check.paths", fail)], Some(1), &["check.paths"]);
        refused.refused = true;
        assert!(!refused.deterministic_red());
        let mut cancelled = ran_with(&[("check.paths", fail)], Some(1), &["check.paths"]);
        cancelled.cancelled = true;
        assert!(!cancelled.deterministic_red());
    }

    #[test]
    fn missing_and_skip_only_probes_are_unknown_not_pass() {
        for outcome in [Outcome::Missing, Outcome::Skip] {
            let mut results = TestResults::default();
            results.record(TestResult {
                id: TestId::step("a"),
                outcome,
                duration_ms: None,
                log: None,
            });
            let ran = Ran {
                results,
                exit: Some(0),
                elapsed: Duration::ZERO,
                log: PathBuf::new(),
                cancelled: false,
                interrupted: None,
                handle: None,
                refused: false,
                flaky_passes: Vec::new(),
                infra_blocked: Vec::new(),
                rerun_exempt: Vec::new(),
                suspect: Vec::new(),
                cannot_execute: None,
                effective_selection: None,
                cpu: None,
                donor: None,
                early_red: false,
            };
            assert_eq!(ran.probe_observation(), ProbeObservation::Unknown);
            assert_ne!(ran.probe_observation(), ProbeObservation::Pass);
        }
    }

    #[test]
    fn result_line_log_pointer_is_recorded() {
        let tmp = TempDir::new("validate-logptr");
        let v = from_config(&config(
            r#"echo '{"id":"a","outcome":"fail","duration_ms":7,"log":"logs/a.log"}' >> "$MQ_RESULTS"; exit 1"#,
        ))
        .unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("a")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(ran.results.logs[&TestId::step("a")], "logs/a.log");
        assert_eq!(ran.results.durations_ms[&TestId::step("a")], 7);
    }

    #[test]
    fn evidence_directory_is_private() {
        let tmp = TempDir::new("validate-private");
        let dir = tmp.0.join("a/dagrun-log");
        private_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn refusal_exit_75_is_neither_green_nor_red() {
        let tmp = TempDir::new("validate-refusal");
        let v = from_config(&config("echo COULD_NOT_RUN; exit 75")).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::of([TestId::step("a")]),
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.refused());
        assert!(!ran.green(), "a refusal must never be a pass");
        assert!(
            !ran.reproduces(),
            "a refusal must never reproduce a failure"
        );
        assert!(
            ran.failed().is_empty(),
            "a refusal attributes no test failure"
        );
        assert_eq!(ran.exit, Some(REFUSAL_EXIT));
    }

    #[test]
    fn refusal_marker_without_exit_75_is_still_a_refusal() {
        let tmp = TempDir::new("validate-refusal-marker");
        let v = from_config(&config("echo COULD_NOT_RUN; exit 4")).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(ran.refused());
        assert!(!ran.green());
        assert!(ran.failed().is_empty());
    }

    #[test]
    fn ordinary_failure_is_not_a_refusal() {
        let tmp = TempDir::new("validate-not-refusal");
        let v = from_config(&config("exit 1")).unwrap();
        let ran = v
            .run(
                &tmp.0,
                &Selection::All,
                &tmp.0.join("o"),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(!ran.refused());
        assert!(!ran.green());
        assert!(ran.reproduces());
    }

    #[test]
    fn dagrun_records_step_collapsed_effective_selection() {
        let tmp = TempDir::new("validate-dagrun-effective");
        let script = tmp.0.join("fake-dagrun");
        std::fs::write(
            &script,
            "#!/bin/sh\necho '[a] ✓ PASS x (1s)'\necho '[b] ✗ FAIL y (2s, exit 1)'\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = ValidatorConfig {
            kind: "dagrun".into(),
            dag: "dag.toml".into(),
            dagrun: script.display().to_string(),
            ..ValidatorConfig::default()
        };
        let v = from_config(&cfg).unwrap();
        let sel = Selection::of([
            TestId::parse("a::case1").unwrap(),
            TestId::parse("b::case2").unwrap(),
        ]);
        let ran = v
            .run(&tmp.0, &sel, &tmp.0.join("o"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            ran.effective_selection,
            Some(Selection::of([TestId::step("a"), TestId::step("b")]))
        );
        assert_eq!(ran.results.durations_ms[&TestId::step("a")], 1000);
    }

    #[test]
    fn journal_parsing() {
        let text = r#"{"event":"step_start","step":"a"}
{"event":"step_end","step":"a","ok":true,"wall_elapsed_s":"1.5"}
{"event":"step_end","step":"b","ok":false}
{"event":"step_skip","step":"c","reason":"x"}
not json"#;
        let results = parse_journal(text);
        assert_eq!(results.results[&TestId::step("a")], Outcome::Pass);
        assert_eq!(results.durations_ms[&TestId::step("a")], 1500);
        assert_eq!(results.failed(), [TestId::step("b")].into());
        assert_eq!(results.results[&TestId::step("c")], Outcome::Skip);
    }
}
