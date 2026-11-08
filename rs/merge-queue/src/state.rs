//! Durable queue state under one directory.
//!
//! ```text
//! <state>/
//!   config.json            fixed: queue configuration
//!   probe-cache.json       fixed: cached planning probes (see `probe_cache`)
//!   lock                   fixed: writer lock (flock)
//!   serve.pid              fixed: `serve` daemon pid (removed on a graceful stop)
//!   serve.heartbeat.json   fixed: `serve` heartbeat, rewritten every cycle
//!   union.attributes       fixed: generated from `union_paths`, when set
//!   api-budget.json        fixed: API requests charged in the last hour (bounded)
//!   kept-validations.json  fixed: green validations a stopped run kept for the next run
//!   items/<pr>.json        one record per submitted change
//!   notifications/         notification reports sent by the daemon (`notify`)
//!   runs/<run-id>/         one directory per `run`: plan, events, validations, report
//!   decisions/<day>.jsonl  one record per validation, by UTC day (see `decisions`)
//!   obligations/<id>.json  one record per focused landing still owed a complete run
//!   scratch/<name>/        candidate worktrees, removed after use
//! ```
//!
//! Growing collections live in their own subdirectories, never beside the fixed files.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::git::CombineMode;
use crate::model::{Item, KeptValidation, NotifyMode, Obligation};

/// Current configuration schema identifier.
pub const CONFIG_SCHEMA: &str = "merge-queue-config/v1";
/// Default maximum number of changes per batch.
pub const DEFAULT_MAX_BATCH: usize = 4;
/// Default speculative depth and validation width.
pub const DEFAULT_DEPTH: usize = 2;
/// Default number of git probes planning runs concurrently; also the resolved value of a
/// configured `probe_parallelism` of 0.
pub const DEFAULT_PROBE_PARALLELISM: usize = 32;
/// Default priority for a submitted change; lower lands first.
pub const DEFAULT_PRIORITY: i64 = pr_landing_planner::priority::DEFAULT_PRIORITY;
/// Default name of the label that puts a change at the front of the queue.
pub const DEFAULT_PRIORITY_LABEL: &str = "mq-priority";

/// How validation runs.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ValidatorConfig {
    /// `command` (a shell template) or `dagrun` (a dagrun DAG, step-level results).
    pub kind: String,
    /// Shell command printing the complete test inventory, one ID per line (optional).
    pub list: String,
    /// Shell command printing the policy's selection for the candidate (`$MQ_BASE`, `$MQ_HEAD`).
    /// Empty means the policy is `all`.
    pub select: String,
    /// Only lines starting with this prefix are test IDs in `select`/`list` output (prefix removed).
    pub id_prefix: String,
    /// Name recorded for the selection policy, e.g. `path-routing`.
    pub policy: String,
    /// Shell command that runs a selection (`command` kind). See the user guide for its contract.
    pub run: String,
    /// dagrun executable for the `dagrun` kind, relative to the candidate checkout or absolute.
    pub dagrun: String,
    /// DAG file for the `dagrun` kind, relative to the candidate checkout.
    pub dag: String,
    /// Command prefix every validation runs under, such as a host admission wrapper.
    pub admission: Vec<String>,
    /// Seconds before one validation is killed and reported as errors (0 = no limit).
    pub timeout_secs: u64,
    /// Also delete ignored files (build caches) from a reused checkout before each validation.
    /// Off by default: caches make validations fast, at the cost of trusting the build to be
    /// hermetic with respect to them.
    pub clean_ignored: bool,
    /// Extra attempts after an admission refusal (exit 75 / `COULD_NOT_RUN`) within one run,
    /// each after a backoff delay.
    #[serde(default = "default_refusal_max_retries")]
    pub refusal_max_retries: usize,
    /// Base backoff in seconds between refusal retries; doubled per attempt, capped at 60.
    /// 0 means no delay between retries (used by tests).
    #[serde(default = "default_refusal_backoff_secs")]
    pub refusal_backoff_secs: u64,
    /// Seconds a GREEN_FLAKY batch (green only because some tests passed after a
    /// retry inside the run) is held for a ruling before the hold expires and the
    /// change is ejected with the flaky tests named. 0 in a stored config means
    /// the default bound.
    #[serde(default = "default_green_flaky_hold_secs")]
    pub green_flaky_hold_secs: u64,
    /// What a GREEN_FLAKY result (green only because some tests passed
    /// after a retry) does when its tests are not all on the known-flaky
    /// list: `hold` (the default: held for a ruling, below) or `land`
    /// (land at once, with the retried tests recorded). An older binary
    /// refuses a config that names this field (unknown fields are refused).
    #[serde(default = "default_green_flaky_policy")]
    pub green_flaky: String,
    /// Where validations run: empty or `off` (in this process, the default), `compare` (in
    /// this process, which decides, and also published to donors so their receipts are
    /// compared with the local verdict), or `decide` (published to donors only; an accepted
    /// receipt is the verdict). See the user guide's "Donor validators".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub offload: String,
    /// Commands naming the tools a validation depends on, by name (for example the commit of
    /// each tool repository, or a toolchain version). The queue host runs them when it
    /// publishes a donor job; a donor whose answers differ refuses the job. Contract:
    /// `tool_identity` identifies exactly what `validator.run` executes, and `tool_refresh`
    /// updates that; the validator attests the loaded tree from inside the run with
    /// `merge-queue-tool-identity: <tool> <40-hex value>` lines (#1045).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tool_identity: std::collections::BTreeMap<String, String>,
    /// Commands that bring a tool named in `tool_identity` up to date (for example a
    /// fetch of the tool repository), by the same names. A donor whose answer for a tool
    /// differs from the job's runs that tool's command once and asks again. An older donor then
    /// matches and runs the job. A donor that still differs is newer than the job, or unrelated
    /// to it, and gives the job back (it is offered the job again after a re-stamp). A tool
    /// without a command, or whose command fails, likewise gives the job back. A refresh must
    /// update what `run` loads, not only what the identity command reads: a fetch-only refresh
    /// under a working-tree-loading validator identifies one tree and runs another. The queue host
    /// also runs these commands, bounded and rate-limited, before it answers tool identity at
    /// claim time, so stamps do not go stale while every donor is busy.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tool_refresh: std::collections::BTreeMap<String, String>,
    /// Times a change may be deferred because validation did not run (an admission refusal, a
    /// validator that cannot execute, a probe that could not run, donors that could not run)
    /// before it is escalated to its owner and held until `merge-queue retry` (0 = 3).
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub max_deferrals: u32,
    /// What a red validation that names failed tests does when the follow-up probe cannot
    /// run: `eject` (empty, the default: the red run is the verdict) or `hold` (escalate the
    /// change to its owner instead).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub red_without_probe: String,
    /// What a red run that recorded no test result at all (no result lines, no verdict
    /// lines, nothing ran) does: `could-not-run` (empty-red is a refusal: no strike,
    /// requeued at the same head, counted against `max_deferrals`) or `red` (empty or
    /// `red`, the default: the red is the verdict, named as a red with no recorded test
    /// result when it ejects). An older binary refuses a config that names this field.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub empty_red: String,
    /// Ask the runner to stop a candidate validation at its first red step: `$MQ_FAIL_FAST` is
    /// `1` for every validation this host runs except the complete run on the base (`widen`).
    /// The runner reports the steps it skipped as `skip`, never `pass`, so a remainder reruns
    /// them. Off by default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fail_fast: bool,
    /// Turn off the early-red watch: by default the queue sets `$MQ_EARLY_RED=1`, watches the
    /// verdicts a runner streams, and stops the run at its first deterministic failure (a
    /// reported failure in a step already declared `RERUN-EXEMPT:`), releasing the slot and
    /// its dependents at once. A runner that streams nothing keeps the post-run behaviour.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disable_early_red: bool,
    /// Stop a candidate validation at the runner's own early verdict: a `command` runner gets
    /// `$MQ_EARLY_VERDICT`, a file in its run directory, and one that creates it holding
    /// `FAILED <test>` is stopped at once, with that test recorded as failed beside the
    /// verdicts streamed so far (an early red, not by itself a deterministic one). The runner
    /// must write the file only for a red that nothing it does later can change: not a failure
    /// its own retry may still pass, and not one it would classify as a refusal. Off by default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub early_verdict: bool,
    /// Confirm a red by validating the same commit again before blaming a change (the
    /// same-commit rerun, which gets `$MQ_RERUN_FROM`), and search a red batch for its culprit
    /// with probes of its prefixes. Off by default: retries belong in the validator, so the
    /// first red is the verdict. A red single-change batch is ejected on that one sighting,
    /// and a red batch of several changes goes back as single-change batches, each decided by
    /// its own validation.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub confirm_reruns: bool,
    /// With `confirm_reruns` on, a red batch of several changes still goes back as
    /// single-change batches, as with it off, instead of being searched with prefix probes
    /// (which hold the run for their whole length). Each single-change batch that is red is then
    /// confirmed by one same-commit rerun of its failed tests before its change is ejected.
    /// Nothing changes with `confirm_reruns` off, which always splits.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub split_red_batches: bool,
    /// With `confirm_reruns` off, a single change whose red run failed only on suspect tests
    /// (timeouts, or runs that reached no verdict, as the validator shapes its result rows) is
    /// held instead of ejected, and those tests run again at the same commit beside the run
    /// ([`crate::suspect`]): it goes back to the queue if they pass and is ejected if they fail
    /// again. Once per head. Off by default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub suspect_flake_reruns: bool,
    /// Wall limit in seconds of one suspect-flake rerun; 0 means
    /// [`DEFAULT_SUSPECT_FLAKE_TIMEOUT_SECS`].
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub suspect_flake_timeout_secs: u64,
    /// Shell command a donor runs for each job after it fetches the
    /// candidate (so the candidate tree exists in its clone, and the
    /// check may read it, e.g. `git show $MQ_CANDIDATE:<path>`), with
    /// `MQ_CANDIDATE`, `MQ_REPO` and `MQ_BASE` in the
    /// environment: the command resolves whatever the candidate needs on
    /// that host (an image, a tool, a licence) and exits 0 only when the
    /// host has it. A non-zero exit gives the job back (never a decline,
    /// never a refused receipt) with the first stderr line as the reason.
    /// Empty (the default) means no check. Setting it changes the
    /// validator digest, so the queue host and its donors must upgrade
    /// together.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pre_claim_check: String,
    /// Wall limit in seconds of `pre_claim_check`; 0 means
    /// [`DEFAULT_PRE_CLAIM_TIMEOUT_SECS`]. A timeout gives the job back
    /// like a failed check.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub pre_claim_timeout_secs: u64,
}

/// Default wall limit of a suspect-flake rerun.
pub const DEFAULT_SUSPECT_FLAKE_TIMEOUT_SECS: u64 = 1800;

fn default_roots() -> usize {
    1
}

fn is_default_roots(value: &usize) -> bool {
    *value == 1
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// Default bound on deferrals, and on could-not-run receipts per donor job.
pub const DEFAULT_RETRY_BOUND: u32 = 3;

/// Where validations run (`validator.offload`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Offload {
    /// In this process only.
    Off,
    /// In this process (deciding) and on donors (compared).
    Compare,
    /// On donors only.
    Decide,
}

impl Offload {
    /// Parse a setting value.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "" | "off" => Ok(Self::Off),
            "compare" => Ok(Self::Compare),
            "decide" => Ok(Self::Decide),
            other => Err(format!(
                "validator.offload is {other:?}; expected off, compare or decide"
            )),
        }
    }
}

/// Donor-validator settings (the queue side).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DonorsConfig {
    /// Seconds a lease lasts without a renewal (0 = 120).
    pub lease_secs: u64,
    /// Largest candidate bundle served to a donor, in bytes (0 = 256 MiB). A bigger one is
    /// refused and the donor declines the job, leaving it to a donor that has the commits.
    pub max_bundle_bytes: u64,
    /// Most leases one donor may hold at once (0 = what the donor asks for).
    pub max_jobs_per_donor: usize,
    /// Seconds a published job may wait unclaimed before the run stops with an error
    /// (offload `decide`; 0 = 900). Nothing is ejected.
    pub claim_timeout_secs: u64,
    /// Receipts saying validation could not run that one job may collect before it is
    /// cancelled, and that one donor may return for it before it is left out (0 = 3).
    pub max_could_not_run: u32,
    /// Donor names that only ever compare: in `decide` mode each published job gets a
    /// duplicate that only these donors may claim, whose receipt is recorded as a
    /// comparison and never decides the batch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compare_only: Vec<String>,
    /// Consecutive give-backs with the same reason from one donor that raise an
    /// `ERROR:` alert in `donor status` and the serve heartbeat (0 = 3).
    pub alert_after_give_backs: u32,
    /// Consecutive probe-pass/launch-refuse cycles after which a donor's host
    /// fault escalates: an `ERROR:` in `donor status` and the serve heartbeat,
    /// and the fault then stays until an admitted validation, a probe pass
    /// alone no longer clears it (0 = 5).
    pub host_fault_escalate_after: u32,
}

impl DonorsConfig {
    /// Resolved could-not-run bound.
    pub fn could_not_run_limit(&self) -> usize {
        if self.max_could_not_run == 0 {
            DEFAULT_RETRY_BOUND as usize
        } else {
            self.max_could_not_run as usize
        }
    }

    /// Resolved lease length.
    pub fn lease(&self) -> u64 {
        if self.lease_secs == 0 {
            120
        } else {
            self.lease_secs
        }
    }

    /// Resolved bundle limit.
    pub fn max_bundle(&self) -> u64 {
        if self.max_bundle_bytes == 0 {
            256 * 1024 * 1024
        } else {
            self.max_bundle_bytes
        }
    }

    /// Resolved give-back alert threshold.
    pub fn give_back_alert(&self) -> usize {
        if self.alert_after_give_backs == 0 {
            DEFAULT_RETRY_BOUND as usize
        } else {
            self.alert_after_give_backs as usize
        }
    }

    /// Whether donor `name` only ever compares (see `compare_only`).
    pub fn is_compare_only(&self, name: &str) -> bool {
        self.compare_only.iter().any(|only| only == name)
    }

    /// Resolved probe-pass/launch-refuse cycles before a host fault escalates.
    pub fn host_fault_escalate_after(&self) -> usize {
        if self.host_fault_escalate_after == 0 {
            crate::donor::DEFAULT_HOST_FAULT_ESCALATE_AFTER
        } else {
            self.host_fault_escalate_after as usize
        }
    }

    /// Resolved claim timeout.
    pub fn claim_timeout(&self) -> u64 {
        if self.claim_timeout_secs == 0 {
            900
        } else {
            self.claim_timeout_secs
        }
    }
}

/// Default seconds one peripheral-classifier call may run before the change counts as
/// binary-touching.
pub const DEFAULT_PERIPHERAL_TIMEOUT_SECS: u64 = 30;

/// Peripheral-change classification: a consumer hook that says, per change, whether it is
/// peripheral (touches no input of the product binary build and no test or fixture the
/// full selection would run differently) or binary-touching. Fail closed: any answer
/// other than a first line of exactly `peripheral`, any error, and any timeout all mean
/// binary-touching.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PeripheralConfig {
    /// Shell command run in a checkout of the change head with `$MQ_BASE`, `$MQ_HEAD` and
    /// `$MQ_PR`. Empty disables the lane: every change is binary-touching.
    pub command: String,
    /// Seconds one call may run (0 = 30).
    pub timeout_secs: u64,
    /// Largest batch of only peripheral changes (0 = twice `max_batch`).
    pub max_batch: usize,
}

impl PeripheralConfig {
    /// Whether the lane is on.
    pub fn enabled(&self) -> bool {
        !self.command.trim().is_empty()
    }

    /// Resolved per-call timeout.
    pub fn timeout(&self) -> u64 {
        if self.timeout_secs == 0 {
            DEFAULT_PERIPHERAL_TIMEOUT_SECS
        } else {
            self.timeout_secs
        }
    }

    /// Resolved batch cap for all-peripheral batches.
    pub fn batch_limit(&self, max_batch: usize) -> usize {
        if self.max_batch == 0 {
            max_batch.max(1) * 2
        } else {
            self.max_batch
        }
    }
}

/// The footprint-fed conflict prior: a test-footprints file in the repository
/// (`{test id: [paths]}`) whose overlaps mark pairs as suspect before probing.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConflictPriorConfig {
    /// Path in the repository (read at the plan's base) of the footprints JSON.
    /// Empty disables the prior.
    pub footprints: String,
}

fn is_default_conflict_prior(value: &ConflictPriorConfig) -> bool {
    value == &ConflictPriorConfig::default()
}

fn is_default_peripheral(value: &PeripheralConfig) -> bool {
    value == &PeripheralConfig::default()
}

/// The front of the queue: which changes are planned ahead of all other work.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PriorityConfig {
    /// Label that marks a pull request as a priority change when `sync` reads it from the
    /// hosting service (compared without regard to case). Empty means
    /// [`DEFAULT_PRIORITY_LABEL`]; `-` turns the label off.
    pub label: String,
    /// Path patterns of CI and validation infrastructure (exact path, directory, or `*`
    /// wildcards, as for `regenerate.paths`). `plan` suggests the label for a change touching
    /// only these paths; it never applies the label itself.
    pub ci_paths: Vec<String>,
}

impl PriorityConfig {
    /// The label `sync` honours, or `None` when it is turned off.
    pub fn label(&self) -> Option<&str> {
        match self.label.as_str() {
            "" => Some(DEFAULT_PRIORITY_LABEL),
            "-" => None,
            label => Some(label),
        }
    }

    /// Whether `labels` carry the priority label.
    pub fn labelled(&self, labels: &[String]) -> bool {
        self.label().is_some_and(|wanted| {
            labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(wanted))
        })
    }

    /// Whether every one of `files` (at least one) is CI or validation infrastructure.
    pub fn only_ci_paths<'a>(&self, files: impl IntoIterator<Item = &'a String>) -> bool {
        let mut any = false;
        for path in files {
            any = true;
            if !self
                .ci_paths
                .iter()
                .any(|pattern| pattern_covers(pattern, path))
            {
                return false;
            }
        }
        any && !self.ci_paths.is_empty()
    }
}

/// Default deadline of one precheck, in seconds.
pub const DEFAULT_PRECHECK_TIMEOUT_SECS: u64 = 300;

/// Default deadline of one background precheck, in seconds.
pub const DEFAULT_PRECHECK_BACKGROUND_TIMEOUT_SECS: u64 = 1800;

/// The precheck every submitted head must pass before it is queued (see [`crate::precheck`]).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrecheckConfig {
    /// Shell command run in a checkout of the submitted head alone; empty runs no precheck.
    pub command: String,
    /// Deadline of one run in seconds; 0 means [`DEFAULT_PRECHECK_TIMEOUT_SECS`].
    pub timeout_secs: u64,
    /// Shell command a background worker runs on every queued head, for checks too slow to
    /// wait for; empty runs none. Its failure ejects a change before it joins a batch, once
    /// the verdict is known.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub background: String,
    /// Deadline of one background run in seconds; 0 means
    /// [`DEFAULT_PRECHECK_BACKGROUND_TIMEOUT_SECS`].
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub background_timeout_secs: u64,
    /// Steps of the validation the two commands reproduce. When every failure of a red batch is
    /// one of these and some change in it failed its precheck, that change is ejected and the
    /// rest go back as one batch, not one batch each.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
}

impl PrecheckConfig {
    /// Resolved deadline.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.timeout_secs == 0 {
            DEFAULT_PRECHECK_TIMEOUT_SECS
        } else {
            self.timeout_secs
        })
    }

    /// Resolved deadline of a background run.
    pub fn background_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.background_timeout_secs == 0 {
            DEFAULT_PRECHECK_BACKGROUND_TIMEOUT_SECS
        } else {
            self.background_timeout_secs
        })
    }
}

fn is_default_hooks(hooks: &HooksConfig) -> bool {
    hooks == &HooksConfig::default()
}

fn is_default_notify_mode(mode: &NotifyMode) -> bool {
    mode.is_default()
}

fn is_default_precheck(value: &PrecheckConfig) -> bool {
    *value == PrecheckConfig::default()
}

/// Default age bound of the decision log, in days.
pub const DEFAULT_DECISIONS_KEEP_DAYS: u32 = 90;
/// Default size bound of the decision log, in bytes.
pub const DEFAULT_DECISIONS_MAX_BYTES: u64 = 64 << 20;

/// Bounds of the decision log (`decisions/`), pruned after every run.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecisionsConfig {
    /// Days of records kept; 0 means [`DEFAULT_DECISIONS_KEEP_DAYS`].
    pub keep_days: u32,
    /// Total bytes kept (the newest day is always kept); 0 means
    /// [`DEFAULT_DECISIONS_MAX_BYTES`].
    pub max_bytes: u64,
}

impl DecisionsConfig {
    /// Resolved age bound in days.
    pub fn keep_days(&self) -> u32 {
        if self.keep_days == 0 {
            DEFAULT_DECISIONS_KEEP_DAYS
        } else {
            self.keep_days
        }
    }

    /// Resolved size bound in bytes.
    pub fn max_bytes(&self) -> u64 {
        if self.max_bytes == 0 {
            DEFAULT_DECISIONS_MAX_BYTES
        } else {
            self.max_bytes
        }
    }
}

/// The shared change classifier (the `change-classifier` crate) for decision-log features.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClassifierConfig {
    /// Test-footprint policy (version 2) in the repository, read at each plan's base commit,
    /// such as `ci/test-footprints.json`. Empty: components are first directories.
    pub policy: String,
}

fn is_default_classifier(value: &ClassifierConfig) -> bool {
    *value == ClassifierConfig::default()
}

fn is_default_decisions(value: &DecisionsConfig) -> bool {
    *value == DecisionsConfig::default()
}

fn is_default_priority(value: &PriorityConfig) -> bool {
    *value == PriorityConfig::default()
}

fn is_default_donors(value: &DonorsConfig) -> bool {
    *value == DonorsConfig::default()
}

fn default_refusal_max_retries() -> usize {
    DEFAULT_REFUSAL_MAX_RETRIES
}

fn default_refusal_backoff_secs() -> u64 {
    DEFAULT_REFUSAL_BACKOFF_SECS
}

fn default_green_flaky_hold_secs() -> u64 {
    DEFAULT_GREEN_FLAKY_HOLD_SECS
}

fn default_green_flaky_policy() -> String {
    "hold".to_owned()
}

impl ValidatorConfig {
    /// Resolved refusal retries (stored verbatim; missing in an old config means default).
    pub fn refusal_retries(&self) -> usize {
        self.refusal_max_retries
    }

    /// Resolved base backoff in seconds (stored verbatim; 0 = no delay).
    pub fn refusal_backoff(&self) -> u64 {
        self.refusal_backoff_secs
    }

    /// Resolved wall limit of a suspect-flake rerun.
    pub fn suspect_flake_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.suspect_flake_timeout_secs == 0 {
            DEFAULT_SUSPECT_FLAKE_TIMEOUT_SECS
        } else {
            self.suspect_flake_timeout_secs
        })
    }

    /// Resolved deferral bound.
    pub fn deferral_limit(&self) -> u32 {
        if self.max_deferrals == 0 {
            DEFAULT_RETRY_BOUND
        } else {
            self.max_deferrals
        }
    }

    /// Whether the queue watches streamed verdicts and stops a run at its first
    /// deterministic failure (on unless `disable_early_red` is set).
    pub fn early_red_enabled(&self) -> bool {
        !self.disable_early_red
    }

    /// Whether a red run whose probe cannot run holds the change instead of ejecting it.
    pub fn hold_red_without_probe(&self) -> bool {
        self.red_without_probe == "hold"
    }

    /// Whether a red run that recorded no test result is a could-not-run refusal
    /// (`validator.empty_red` is `could-not-run`) instead of a red verdict. Empty or
    /// `red` keeps the red verdict.
    pub fn empty_red_could_not_run(&self) -> bool {
        self.empty_red == "could-not-run"
    }

    /// Whether GREEN_FLAKY results land by policy (`validator.green_flaky`
    /// is `land`) instead of being held for a ruling.
    pub fn green_flaky_lands(&self) -> bool {
        self.green_flaky == "land"
    }

    /// Resolved wall limit of the pre-claim check.
    pub fn pre_claim_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.pre_claim_timeout_secs == 0 {
            DEFAULT_PRE_CLAIM_TIMEOUT_SECS
        } else {
            self.pre_claim_timeout_secs
        })
    }

    /// Resolved GREEN_FLAKY hold bound in seconds.
    pub fn green_flaky_hold(&self) -> u64 {
        if self.green_flaky_hold_secs == 0 {
            DEFAULT_GREEN_FLAKY_HOLD_SECS
        } else {
            self.green_flaky_hold_secs
        }
    }
}

/// Default extra refusal retries within one run when the config does not set one.
pub const DEFAULT_REFUSAL_MAX_RETRIES: usize = 3;
/// Default base refusal backoff in seconds for a newly initialised queue.
pub const DEFAULT_REFUSAL_BACKOFF_SECS: u64 = 2;
/// Default GREEN_FLAKY hold bound in seconds (two hours for a ruling).
pub const DEFAULT_GREEN_FLAKY_HOLD_SECS: u64 = 7200;
/// Default wall limit in seconds of a donor's pre-claim check.
pub const DEFAULT_PRE_CLAIM_TIMEOUT_SECS: u64 = 120;

/// How a validated candidate reaches the base branch.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LandConfig {
    /// `push` (fast-forward the remote base to the candidate), `command`, or `none`.
    pub driver: String,
    /// For `command`: shell template run once per change, in order. See the user guide.
    pub command: String,
    /// For `command`: allow batches of more than one change. The base then passes through each
    /// change's intermediate tree, which was not validated on its own; off by default, which
    /// makes every batch a single change.
    pub allow_batches: bool,
    /// After landing, close each landed pull request through the hosting service (budgeted),
    /// with a comment naming the commit that contains it. Closes the budget cannot afford yet
    /// are retried by `sync`.
    pub close_prs: bool,
    /// Most closes the closer attempts per cycle (one `sync` pass, one
    /// closer pass): a cycle also spends one batched state read, so this
    /// bounds the closer's share of the host's hosting calls. 0 means
    /// [`DEFAULT_CLOSE_MAX_PER_CYCLE`]. State-done pull requests
    /// (MERGED/CLOSED) cost no close and do not count against it.
    #[serde(default)]
    pub close_max_per_cycle: usize,
    /// Extra attempts for a `push` landing after a transient remote-side rejection (one that
    /// is not a non-fast-forward: a remote fault, a 5xx, a dropped connection). 0 means
    /// [`DEFAULT_PUSH_MAX_RETRIES`]. A non-fast-forward is never retried here: it goes down
    /// the base-moved path and the batch is rebuilt.
    #[serde(default)]
    pub push_max_retries: usize,
    /// Base backoff in seconds between push retries, doubled per retry (0 means
    /// [`DEFAULT_PUSH_RETRY_BACKOFF_SECS`]): with the defaults, three tries span about two
    /// minutes before the run gives up and keeps the green validations for the next run.
    #[serde(default)]
    pub push_retry_backoff_secs: u64,
}

/// Default most closes per closer cycle: one batched state read plus one
/// close per cycle drains a backlog without holding a paced host's whole
/// hosting budget (one call per 30 s there, a cycle about a minute).
pub const DEFAULT_CLOSE_MAX_PER_CYCLE: usize = 1;

impl LandConfig {
    /// Resolved most closes the closer attempts per cycle.
    pub fn close_max_per_cycle(&self) -> usize {
        if self.close_max_per_cycle == 0 {
            DEFAULT_CLOSE_MAX_PER_CYCLE
        } else {
            self.close_max_per_cycle
        }
    }

    /// Resolved extra push attempts after a transient rejection.
    pub fn push_retries(&self) -> usize {
        if self.push_max_retries == 0 {
            DEFAULT_PUSH_MAX_RETRIES
        } else {
            self.push_max_retries
        }
    }

    /// Resolved base backoff between push retries.
    pub fn push_retry_backoff(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.push_retry_backoff_secs == 0 {
            DEFAULT_PUSH_RETRY_BACKOFF_SECS
        } else {
            self.push_retry_backoff_secs
        })
    }
}

/// Default extra push attempts after a transient rejection (three tries in all).
pub const DEFAULT_PUSH_MAX_RETRIES: usize = 2;
/// Default base push-retry backoff in seconds (delays of 40 s then 80 s).
pub const DEFAULT_PUSH_RETRY_BACKOFF_SECS: u64 = 40;

/// Generated files the queue regenerates instead of treating their conflicts as real.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegenerateConfig {
    /// Path patterns of generated files: an exact path, a directory (`dir/` or `dir`), or a
    /// pattern with `*` wildcards (matching any characters, `/` included).
    pub paths: Vec<String>,
    /// Shell command, run as `sh -c` at the root of a scratch checkout of the candidate
    /// tip, that rewrites every generated file from its sources. It must be deterministic
    /// (byte-identical output for the same sources) and touch nothing else. The queue runs it
    /// with `MQ_PR`, `MQ_REGEN_PATHS` and `MQ_CONFLICTED` set, and with `MQ_HEAD` (the change's
    /// head), `MQ_PARENT` (the commit the change was combined onto) and `MQ_CHANGE_BASE` (the
    /// commit the change's own edits start from: the head of a change it is stacked on whose
    /// copy `MQ_PARENT` already holds, or else the merge base of the two), all empty when it
    /// runs on a tree without a change of its own. A generated path that is partly hand-written
    /// is the command's to merge from those three commits: the combination settled its
    /// conflicts on the parent's side. A stdout line `merge-queue-note: TEXT` is recorded with
    /// the regeneration, in the run's events and the change's history; when the command fails,
    /// its `merge-queue-conflict: TEXT` lines make the ejection a conflict (rebase and
    /// resubmit) instead of a generator failure.
    pub command: String,
    /// Optional shell command run before `command`, such as building the generator. When it
    /// fails, the combined tree cannot even build the generator (stale against the base, or the
    /// combination broke the generator's sources), and the change is ejected as
    /// `regen-build-failed` with the error tail rather than as a generator failure.
    pub build_command: String,
    /// `conflict` (default): regenerate after a change whose combination conflicted in, or
    /// changed, a generated file (a textual merge of generated files need not be the
    /// regenerated output). `always`: regenerate after every change. Regeneration happens per
    /// change, so every prefix a culprit search probes is self-consistent.
    pub when: String,
}

impl RegenerateConfig {
    /// Whether regeneration is configured.
    pub fn enabled(&self) -> bool {
        !self.paths.is_empty() && !self.command.trim().is_empty()
    }

    /// Whether `path` is a generated file. A pattern without wildcards names either
    /// the file itself or the directory holding it (with or without a trailing
    /// slash), so `generated` covers `generated/output.txt`: a file and a
    /// directory of the same name cannot coexist in one tree, so this cannot
    /// cover an unrelated path.
    pub fn covers(&self, path: &str) -> bool {
        self.enabled()
            && self
                .paths
                .iter()
                .any(|pattern| pattern_covers(pattern, path))
    }

    /// Whether to regenerate after every change, not only after a generated-file conflict.
    pub fn always(&self) -> bool {
        self.when == "always"
    }
}

/// A commit of another repository that this one pins in several places at once: a submodule
/// gitlink, and files that repeat the gitlink's commit hash (dependency manifests, lock files,
/// build files). When a change moves the pin and the target it is combined onto pins another
/// commit, the queue takes the descendant at every site (see `crate::pins`).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PinConfig {
    /// Name, for reports.
    pub name: String,
    /// Path of the gitlink whose commit is the pin.
    pub gitlink: String,
    /// Files that repeat the pin's hash, as `regenerate.paths` names them: an exact path, a
    /// directory, or a pattern with `*` wildcards. Every full 40-hex occurrence of a pin hash
    /// in them is a pin site.
    pub files: Vec<String>,
    /// URL of the pinned repository. Its `branch` is fetched (into
    /// `refs/merge-queue/pins/<name>`) only when a commit needed to order two pins is missing.
    pub url: String,
    /// The branch a pin a change introduces must be on (default `main`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
}

impl PinConfig {
    /// Whether `path` is one of this pin's sites: the gitlink or a declared file.
    pub fn covers(&self, path: &str) -> bool {
        (!self.gitlink.is_empty() && path == self.gitlink)
            || self
                .files
                .iter()
                .any(|pattern| pattern_covers(pattern, path))
    }

    /// The branch both pins are ordered on.
    pub fn branch(&self) -> &str {
        if self.branch.is_empty() {
            "main"
        } else {
            &self.branch
        }
    }
}

/// [`path_matches`], where a pattern without wildcards also covers everything under it as a
/// directory (`dir` as well as `dir/`).
fn pattern_covers(pattern: &str, path: &str) -> bool {
    path_matches(pattern, path)
        || (!pattern.contains('*')
            && path.starts_with(&format!("{}/", pattern.trim_end_matches('/'))))
}

/// Match a path against a pattern: exact, directory prefix (`dir/`), or `*` wildcards.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(dir) = pattern.strip_suffix('/') {
        return path.starts_with(&format!("{dir}/"));
    }
    if !pattern.contains('*') {
        return pattern == path;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = path;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(after) => rest = after,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    rest.is_empty()
}

/// The hosting service (GitHub through its `gh` command), used only for ready sources and
/// closing landed pull requests, behind a request budget.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitHubConfig {
    /// `owner/name`.
    pub repo: String,
    /// Command (and leading arguments) to run instead of `gh`, e.g. a pacing wrapper.
    /// When empty, the `FLEET_REAL_GH` environment variable is honoured as a fallback
    /// (see `github::resolve_gh_command`); an explicit value always wins.
    pub gh: Vec<String>,
    /// Requests allowed per rolling hour (0 = 60).
    pub max_per_hour: u32,
    /// Budget file shared by every queue on the host; empty = `api-budget.json` in the state
    /// directory.
    pub budget_file: String,
    /// Fetch without credentials (public repositories): Git credential helpers are disabled for
    /// fetches, so fetching never spends a credential use.
    pub anonymous_fetch: bool,
}

/// A failure class: failed tests matching `pattern` are reported as `class`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClassRule {
    /// Test-ID pattern (exact, or with `*` wildcards), matched against the whole ID and against
    /// its step alone, so `test.cli` also classifies `test.cli::case`.
    pub pattern: String,
    /// Label shown in reports and ejection records, e.g. `count pin drift`.
    pub class: String,
    /// What the failure must say for the class to hold, as text with `*` wildcards that one
    /// line of the validation log tagged with the test's step (`[step] ...`, the way a
    /// dagrun-style runner prefixes each step's output) must contain, e.g.
    /// `expected * tests to execute, saw *`. Empty: the pattern alone decides. Without a
    /// readable log the rule classifies nothing, so a label always rests on the failure
    /// itself, never on which test failed. Left out of config.json while empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub evidence: String,
}

/// Whether any rule needs the validation log ([`ClassRule::evidence`]).
pub fn classify_reads_log(rules: &[ClassRule]) -> bool {
    rules.iter().any(|rule| !rule.evidence.is_empty())
}

/// Class labels for `failed` under `rules`, sorted and without duplicates. `log` is the text
/// of the validation logs the failures were seen in, for rules that need evidence.
pub fn classify(
    rules: &[ClassRule],
    failed: &[crate::tests::TestId],
    log: Option<&str>,
) -> Vec<String> {
    let mut classes: Vec<String> = failed
        .iter()
        .flat_map(|test| {
            let id = test.to_string();
            rules
                .iter()
                .filter(move |rule| {
                    (path_matches(&rule.pattern, &id) || path_matches(&rule.pattern, &test.step))
                        && (rule.evidence.is_empty()
                            || log.is_some_and(|log| shows(log, &test.step, &rule.evidence)))
                })
                .map(|rule| rule.class.clone())
        })
        .collect();
    classes.sort();
    classes.dedup();
    classes
}

/// Whether a line of `log` tagged `[step]` contains `evidence` (`*` matches any text).
fn shows(log: &str, step: &str, evidence: &str) -> bool {
    let tag = format!("[{step}]");
    log.lines()
        .filter(|line| line.contains(&tag))
        .any(|line| wildcard_contains(evidence, line))
}

/// Whether `text` contains `pattern`, where each `*` in the pattern matches any text.
fn wildcard_contains(pattern: &str, text: &str) -> bool {
    let mut rest = text;
    for part in pattern.split('*').filter(|part| !part.is_empty()) {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Push notifications on item state changes, sent by the `serve` daemon
/// through an agentctl-compatible command.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotifyConfig {
    /// Command (and leading arguments) to run instead of `agentctl`, e.g. a
    /// test fake or a wrapper. Empty means `agentctl` on `PATH`. The daemon
    /// runs `<command> send <name> --deliver-while-working --file <report>`.
    pub agentctl: Vec<String>,
    /// agentctl registry the sends address, passed as `--registry PATH`
    /// before `send`. Empty leaves agentctl's own default, `.agentctl` in the
    /// daemon's working directory (the queue repository when the daemon was
    /// started there). Omitted from config.json when empty, so a config that
    /// does not set it still loads on builds that predate it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub registry: String,
}

/// Event hooks (5891): shell commands fired, fire-and-forget, with a
/// `merge-queue-hook/v1` JSON payload on stdin when something happens.
/// An empty command turns that event off; every field absent is off, so
/// a config that does not set `hooks` still loads on older builds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksConfig {
    /// A red batch verdict.
    pub on_red: String,
    /// A change was ejected.
    pub on_eject: String,
    /// A GREEN_FLAKY hold for a ruling.
    pub on_held: String,
    /// A change landed.
    pub on_land: String,
    /// The base moved.
    pub on_base_moved: String,
    /// An intake request was refused.
    pub on_intake_refused: String,
    /// A change was escalated to its owner.
    pub on_escalated: String,
    /// Seconds one hook may run before it is killed (0 means 30).
    #[serde(default)]
    pub timeout_secs: u64,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            on_red: String::new(),
            on_eject: String::new(),
            on_held: String::new(),
            on_land: String::new(),
            on_base_moved: String::new(),
            on_intake_refused: String::new(),
            on_escalated: String::new(),
            timeout_secs: 30,
        }
    }
}

impl HooksConfig {
    /// The command for `event`, when one is configured.
    pub fn command_for(&self, event: &str) -> Option<&str> {
        let command = match event {
            "on_red" => &self.on_red,
            "on_eject" => &self.on_eject,
            "on_held" => &self.on_held,
            "on_land" => &self.on_land,
            "on_base_moved" => &self.on_base_moved,
            "on_intake_refused" => &self.on_intake_refused,
            "on_escalated" => &self.on_escalated,
            _ => return None,
        };
        if command.is_empty() {
            None
        } else {
            Some(command)
        }
    }

    /// Whether any hook is configured (the common default-off case).
    pub fn any(&self) -> bool {
        [
            &self.on_red,
            &self.on_eject,
            &self.on_held,
            &self.on_land,
            &self.on_base_moved,
            &self.on_intake_refused,
            &self.on_escalated,
        ]
        .iter()
        .any(|command| !command.is_empty())
    }
}

/// Identity for commits the queue creates.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdentityConfig {
    /// Name; empty uses Git's own configuration.
    pub name: String,
    /// Email; empty uses Git's own configuration.
    pub email: String,
}

/// Queue configuration stored in `config.json`.
///
/// Unknown top-level fields are tolerated, not rejected: a config written by
/// a newer build (for example one that sets a flag this build does not know)
/// must still load on an older binary, or a config-first rollout bricks every
/// command on the lagging host. The unknown fields are captured verbatim in
/// `unknown_fields`, preserved on rewrite, and named in a startup warning so
/// the operator knows to upgrade the binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema identifier.
    pub schema: String,
    /// Repository directory the queue operates on.
    pub repo: PathBuf,
    /// Remote holding the base branch and change heads; empty for a purely local repository.
    pub remote: String,
    /// Base branch name.
    pub base: String,
    /// Command prefix for network Git operations.
    pub net_wrapper: Vec<String>,
    /// Seconds one network command (`git fetch`, `ls-remote`, `push`, a `gh` call) may run
    /// before its process group is killed and it fails (0 = 300). A hung transport, or a
    /// credential helper waiting for budget, then costs a retry instead of holding the run,
    /// the landing or the daemon's loop.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub net_timeout_secs: u64,
    /// How changes are combined onto a candidate.
    pub combine: CombineMode,
    /// Maximum changes per batch.
    pub max_batch: usize,
    /// Speculative depth: how many batches validate concurrently, each on top of the previous.
    /// Also the number of concurrent culprit-search probes.
    pub depth: usize,
    /// Largest window the adaptive (AIMD) window may grow to: +1 per landed batch, halved per
    /// red batch, never below 1. 0 (the default) means `depth`, a fixed window.
    pub max_depth: usize,
    /// Value-ordered scheduling: when intake brings a batch whose historical green
    /// rate beats the deepest in-flight batch's, abort the deepest (it is replanned
    /// later in the run) and give its slot to the new batch. Off by default: it
    /// wants decision history worth ordering by.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub value_ordering: bool,
    /// Turn off the disjoint second root: with a window of at least 2, one slot validates
    /// a batch built on the base itself, disjoint (no textual conflict, no dependency)
    /// from every batch in flight, so one red stack cannot take the whole window down.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_disjoint_root: bool,
    /// Only start a disjoint second root when the recent red rate (the red share
    /// of the last 20 decided validations in the decision records, as a percent)
    /// is at least this threshold, so a root is only spent when a red head is
    /// likely. The default 0 turns the gate off: a root starts whenever one fits.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub disjoint_root_min_red_percent: u32,
    /// How many base-rooted batches may validate at once, each disjoint
    /// from everything in flight (1, the default: today's single disjoint
    /// root in the window's last slot). At least the window means every
    /// slot may be a root: k independent single-change validations on the
    /// current base, the first green landing and the rest going stale.
    #[serde(default = "default_roots", skip_serializing_if = "is_default_roots")]
    pub roots: usize,
    /// Keep a rooted batch whose base moved instead of cancelling it: a
    /// green one is requeued at the front to be rebuilt on the new base, a
    /// red one is decided as usual. Off (the default) cancels it at once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_stale_roots: bool,
    /// Build a batch on the stack top only when the recent pass rate (the
    /// green share of the last 20 decided validations) is at least this
    /// percent; below it, build a root on the base instead when `roots`
    /// allows. 0 (the default) always stacks.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub stack_min_pass_percent: u32,
    /// Build batches wider than one change only when the recent pass rate
    /// is at least this percent; below it, batches are one change wide
    /// whatever `max_batch` says. 0 (the default) always allows width.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub batch_min_pass_percent: u32,
    /// Success predictor (SubmitQueue §4.2): when on, decision records carry the
    /// trained model's `predicted_pass` per change and per batch (see
    /// `predict`), instead of only the base-rate prior. Off by default: today's
    /// behaviour until an operator trains (`merge-queue model train`) and
    /// enables it. Scheduling does not read it; the planner does, separately.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub predict: bool,
    /// Expected-value planner (SubmitQueue §4/§7.1, see `plan_ev`): when on,
    /// pending batches are ordered by expected landed changes per
    /// validate-hour and the lowest-value in-flight build is aborted when a
    /// strictly better build queues. Existing knobs (`max_batch`, `depth`,
    /// `roots`, the pass-rate gates) act as caps. Off by default; wants
    /// `predict` history worth ordering by.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ev_planner: bool,
    /// Concurrent git probes while planning (base and pairwise conflict checks, each a git
    /// subprocess). 0 means [`DEFAULT_PROBE_PARALLELISM`]. Probe results are cached across
    /// runs in `probe-cache.json`, so this bounds only cache misses.
    #[serde(default)]
    pub probe_parallelism: usize,
    /// Paths (Git attribute patterns) merged with Git's `union` driver when combining changes and
    /// probing conflicts. Only for line-oriented, append-only files; validation still checks the
    /// result.
    pub union_paths: Vec<String>,
    /// Generated files regenerated on the candidate instead of blocking it.
    pub regenerate: RegenerateConfig,
    /// Generated-file path patterns declared at the top level (globs, as in
    /// `regenerate.paths`): an alternative spelling for queues whose config
    /// names the derived files directly. On load they are merged into
    /// `regenerate.paths`, so every conflict check sees one set. Omitted from
    /// `config.json` while empty, so an older config still loads unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generated_paths: Vec<String>,
    /// Exact-commit rule: never carry a validation result to a different commit. A remainder
    /// after an ejection reruns the batch's whole selection, and a tree lands only with green
    /// coverage of its selection recorded at that exact commit.
    pub exact_sha: bool,
    /// Operator-asserted exception: exact merge commits may land even when
    /// the base branch's rules require linear history. Set only while an
    /// owner-granted ruleset exception is in place; off by default, when
    /// `submit --exact` of a merge commit is refused if the branch rules
    /// forbid merge commits. An older binary refuses a config that names it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact_merge_allowed: bool,
    /// Treat the base as green for culprit searches (skip probing it). Sound when only this
    /// queue lands on the base, always as exactly validated commits; a broken base would then
    /// be blamed on the first change of a red batch.
    pub trust_base: bool,
    /// Approval-at-head rule for GitHub ready sources (`sync --label` /
    /// `sync --all-open`): a pull request counts as ready only with an
    /// approving review on its current head commit, read in the same batched
    /// list call. Off by default; ready-file sources are unaffected (they
    /// make no API call and carry no review data).
    #[serde(default)]
    pub require_approval_at_head: bool,
    /// Recount the generated count pins after merging each batch instead of
    /// treating their conflicts as real: see `recount`. Off by default.
    #[serde(default)]
    pub recount_pins: bool,
    /// Commits of other repositories this one pins, resolved monotonically when changes are
    /// combined (see [`PinConfig`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<PinConfig>,
    /// Identity for commits the queue creates.
    pub identity: IdentityConfig,
    /// Failure classes: failed tests matching a rule's pattern are labelled with its class in
    /// ejection records and reports (first-class names for known failure kinds).
    pub classify: Vec<ClassRule>,
    /// The hosting service, for ready sources and closing landed pull requests.
    pub github: GitHubConfig,
    /// Peer queues by name: other repositories' merge-queue state directories. A change may
    /// require a commit on a peer's base; `run --with-peers` drains those peers first.
    pub peers: std::collections::BTreeMap<String, PathBuf>,
    /// Priority label and CI paths (the front of the queue).
    #[serde(skip_serializing_if = "is_default_priority")]
    pub priority: PriorityConfig,
    /// Decision-log bounds.
    #[serde(skip_serializing_if = "is_default_decisions")]
    pub decisions: DecisionsConfig,
    /// Shared change classifier.
    #[serde(skip_serializing_if = "is_default_classifier")]
    pub classifier: ClassifierConfig,
    /// Validation configuration.
    pub validator: ValidatorConfig,
    /// Donor-validator settings (used when `validator.offload` is not off).
    #[serde(skip_serializing_if = "is_default_donors")]
    pub donors: DonorsConfig,
    /// Peripheral-change classification (an empty command disables the lane).
    #[serde(default, skip_serializing_if = "is_default_peripheral")]
    pub peripheral: PeripheralConfig,
    /// Footprint-fed conflict prior (an empty path disables it).
    #[serde(default, skip_serializing_if = "is_default_conflict_prior")]
    pub conflict_prior: ConflictPriorConfig,
    /// Landing configuration.
    pub land: LandConfig,
    /// The precheck of submitted heads. Omitted from config.json when unset.
    #[serde(skip_serializing_if = "is_default_precheck")]
    pub precheck: PrecheckConfig,
    /// Push-notification configuration (state-change messages via agentctl).
    pub notify: NotifyConfig,
    /// How hooks and built-in notify divide reports queue-wide:
    /// `both` (default), `hooks-only` or `notify-only`. A per-item
    /// `submit --notify-mode` overrides it. Omitted from config.json
    /// while `both`, so an older config still loads unchanged.
    #[serde(default, skip_serializing_if = "is_default_notify_mode")]
    pub notify_mode: NotifyMode,
    /// Event hooks (5891). Omitted from config.json when off.
    #[serde(default, skip_serializing_if = "is_default_hooks")]
    pub hooks: HooksConfig,
    /// Top-level fields this build does not know, captured verbatim so a
    /// newer config loads (and round-trips) on an older binary. See the
    /// struct-level note.
    #[serde(flatten)]
    pub unknown_fields: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA.to_owned(),
            repo: PathBuf::new(),
            remote: String::new(),
            base: "main".to_owned(),
            net_wrapper: Vec::new(),
            net_timeout_secs: 0,
            combine: CombineMode::Rebase,
            max_batch: DEFAULT_MAX_BATCH,
            depth: DEFAULT_DEPTH,
            max_depth: 0,
            value_ordering: false,
            disable_disjoint_root: false,
            disjoint_root_min_red_percent: 0,
            roots: 1,
            keep_stale_roots: false,
            stack_min_pass_percent: 0,
            batch_min_pass_percent: 0,
            predict: false,
            ev_planner: false,
            probe_parallelism: 0,
            union_paths: Vec::new(),
            regenerate: RegenerateConfig::default(),
            generated_paths: Vec::new(),
            exact_sha: false,
            exact_merge_allowed: false,
            trust_base: false,
            require_approval_at_head: false,
            recount_pins: false,
            pins: Vec::new(),
            identity: IdentityConfig::default(),
            classify: Vec::new(),
            github: GitHubConfig::default(),
            peers: std::collections::BTreeMap::new(),
            priority: PriorityConfig::default(),
            decisions: DecisionsConfig::default(),
            classifier: ClassifierConfig::default(),
            validator: ValidatorConfig::default(),
            donors: DonorsConfig::default(),
            peripheral: PeripheralConfig::default(),
            conflict_prior: ConflictPriorConfig::default(),
            land: LandConfig::default(),
            precheck: PrecheckConfig::default(),
            notify: NotifyConfig::default(),
            notify_mode: NotifyMode::default(),
            hooks: HooksConfig::default(),
            unknown_fields: std::collections::BTreeMap::new(),
        }
    }
}

impl Config {
    /// Whether `path` is a site of a configured pin.
    pub fn pin_site(&self, path: &str) -> bool {
        self.pins.iter().any(|pin| pin.covers(path))
    }

    /// Resolved network-command timeout (a configured 0 means [`crate::bounded::NET_TIMEOUT`]).
    pub fn net_timeout(&self) -> std::time::Duration {
        if self.net_timeout_secs == 0 {
            crate::bounded::NET_TIMEOUT
        } else {
            std::time::Duration::from_secs(self.net_timeout_secs)
        }
    }

    /// Resolved planning-probe concurrency (a configured 0 means the default).
    pub fn probe_parallelism(&self) -> usize {
        if self.probe_parallelism == 0 {
            DEFAULT_PROBE_PARALLELISM
        } else {
            self.probe_parallelism
        }
    }
}

/// Config warnings collected in this process, for JSON replies (see [`State::config`]).
static CONFIG_WARNINGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Drain the config warnings collected so far in this process.
pub fn take_config_warnings() -> Vec<String> {
    CONFIG_WARNINGS
        .lock()
        .map(|mut warnings| std::mem::take(&mut *warnings))
        .unwrap_or_default()
}

/// Handle on a state directory.
#[derive(Clone, Debug)]
pub struct State {
    /// Root of the state directory.
    pub root: PathBuf,
}

/// Exclusive writer lock, released on drop.
pub struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("invalid {}: {error}", path.display()))
}

/// Write JSON atomically (temporary file plus rename).
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("cannot encode {}: {error}", path.display()))?;
    text.push('\n');
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text)
        .map_err(|error| format!("cannot write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|error| format!("cannot rename into {}: {error}", path.display()))
}

/// One change's refusal bookkeeping in shadow mode, where runs never write item records: the
/// deferral bound must hold there too.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShadowRefusal {
    /// The head the count is for; a new head starts again from zero.
    pub head: String,
    /// Deferrals of this head so far.
    pub refusals: u32,
    /// Why it was escalated, once the bound was reached (held until `merge-queue retry`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub escalated: String,
}

impl State {
    /// Path of the shadow-mode refusal ledger (one map, keyed by change number).
    pub fn shadow_refusals_path(&self) -> PathBuf {
        self.root.join("shadow-refusals.json")
    }

    /// The shadow-mode refusal ledger; empty when there is none.
    pub fn shadow_refusals(
        &self,
    ) -> Result<std::collections::BTreeMap<i64, ShadowRefusal>, String> {
        let path = self.shadow_refusals_path();
        if !path.exists() {
            return Ok(Default::default());
        }
        read_json(&path)
    }

    /// Replace the shadow-mode refusal ledger.
    pub fn save_shadow_refusals(
        &self,
        ledger: &std::collections::BTreeMap<i64, ShadowRefusal>,
    ) -> Result<(), String> {
        write_json(&self.shadow_refusals_path(), ledger)
    }

    /// Path of the kept-validations file: green batch validations a stopped run
    /// left for the next run (one fixed file, replaced as entries land or go stale).
    pub fn kept_validations_path(&self) -> PathBuf {
        self.root.join("kept-validations.json")
    }

    /// The kept green validations; empty when there are none.
    pub fn kept_validations(&self) -> Result<Vec<KeptValidation>, String> {
        let path = self.kept_validations_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        read_json(&path)
    }

    /// Replace the kept green validations.
    pub fn save_kept_validations(&self, kept: &[KeptValidation]) -> Result<(), String> {
        write_json(&self.kept_validations_path(), &kept)
    }

    /// Open (without creating) the state directory at `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Path of the configuration file.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    /// Path of the `serve` daemon pid file.
    pub fn serve_pid_path(&self) -> PathBuf {
        self.root.join("serve.pid")
    }

    /// Path of the `serve` daemon heartbeat file.
    pub fn serve_heartbeat_path(&self) -> PathBuf {
        self.root.join("serve.heartbeat.json")
    }

    /// Directory of item records.
    pub fn items_dir(&self) -> PathBuf {
        self.root.join("items")
    }

    /// Directory of run records.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// Directory of widening obligations.
    pub fn obligations_dir(&self) -> PathBuf {
        self.root.join("obligations")
    }

    /// Directory of candidate worktrees.
    pub fn scratch_dir(&self) -> PathBuf {
        self.root.join("scratch")
    }

    /// Whether a configuration exists.
    pub fn exists(&self) -> bool {
        self.config_path().is_file()
    }

    /// Create the directory layout and write the configuration.
    pub fn init(&self, config: &Config) -> Result<(), String> {
        for dir in [self.items_dir(), self.runs_dir(), self.obligations_dir()] {
            std::fs::create_dir_all(&dir)
                .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        }
        write_json(&self.config_path(), config)
    }

    /// Load the configuration.
    pub fn config(&self) -> Result<Config, String> {
        if !self.exists() {
            return Err(format!(
                "no merge queue at {} (run `merge-queue init` first)",
                self.root.display()
            ));
        }
        let config: Config = read_json(&self.config_path())?;
        if !config.unknown_fields.is_empty() {
            let names: Vec<&str> = config.unknown_fields.keys().map(String::as_str).collect();
            let revision = crate::build_info::revision()
                .map_or_else(|| "unknown".to_owned(), |r| r.describe());
            let warning = format!(
                "{} sets unknown field(s) {} — ignored by this build (revision {revision}); \
                 the config may be newer than the binary, so upgrade merge-queue if those \
                 settings should take effect",
                self.config_path().display(),
                names.join(", "),
            );
            // Once per process on stderr, and collected for JSON replies (a route that
            // merges stderr into stdout must not corrupt a JSON answer).
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                eprintln!("merge-queue: warning: {warning}");
            }
            if let Ok(mut collected) = CONFIG_WARNINGS.lock() {
                if !collected.contains(&warning) {
                    collected.push(warning);
                }
            }
        }
        if config.schema != CONFIG_SCHEMA {
            return Err(format!(
                "{} has schema {:?}; this merge-queue reads {CONFIG_SCHEMA:?}",
                self.config_path().display(),
                config.schema
            ));
        }
        // Top-level `generated_paths` is the same declaration as
        // `regenerate.paths`: fold it in so conflict checks, planning and
        // regeneration all see one set of generated files.
        let mut config = config;
        for pattern in config.generated_paths.clone() {
            if !config.regenerate.paths.contains(&pattern) {
                config.regenerate.paths.push(pattern);
            }
        }
        Ok(config)
    }

    /// Take the exclusive writer lock, failing fast if another writer holds it.
    pub fn lock(&self) -> Result<Lock, String> {
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("cannot create {}: {error}", self.root.display()))?;
        let path = self.root.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        file.try_lock_exclusive().map_err(|_| {
            format!(
                "another merge-queue writer holds {} (a run in progress?)",
                path.display()
            )
        })?;
        Ok(Lock(file))
    }

    fn item_path(&self, pr: i64) -> PathBuf {
        self.items_dir().join(format!("{pr}.json"))
    }

    /// Load one item record, if present.
    pub fn item(&self, pr: i64) -> Result<Option<Item>, String> {
        let path = self.item_path(pr);
        if path.is_file() {
            read_json(&path).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Store one item record.
    pub fn save_item(&self, item: &Item) -> Result<(), String> {
        write_json(&self.item_path(item.pr), item)
    }

    /// Load every item record, ordered by PR number.
    pub fn items(&self) -> Result<Vec<Item>, String> {
        let mut items: Vec<Item> = load_dir(&self.items_dir())?;
        items.sort_by_key(|item| item.pr);
        Ok(items)
    }

    /// Store one obligation record.
    pub fn save_obligation(&self, obligation: &Obligation) -> Result<(), String> {
        write_json(
            &self
                .obligations_dir()
                .join(format!("{}.json", obligation.id)),
            obligation,
        )
    }

    /// Load every obligation, oldest first.
    pub fn obligations(&self) -> Result<Vec<Obligation>, String> {
        let mut all: Vec<Obligation> = load_dir(&self.obligations_dir())?;
        all.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
        Ok(all)
    }

    /// Create a fresh run directory and return its ID and path.
    pub fn new_run(&self) -> Result<(String, PathBuf), String> {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        for n in 0..1000 {
            let id = if n == 0 {
                stamp.clone()
            } else {
                format!("{stamp}-{n}")
            };
            let dir = self.runs_dir().join(&id);
            if std::fs::create_dir_all(self.runs_dir()).is_ok() && std::fs::create_dir(&dir).is_ok()
            {
                return Ok((id, dir));
            }
        }
        Err("cannot allocate a run directory".to_owned())
    }
}

fn load_dir<T: DeserializeOwned>(dir: &Path) -> Result<Vec<T>, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            out.push(read_json(&path)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::tests::TestId;

    #[test]
    fn regenerate_covers_files_and_directories() {
        let config = RegenerateConfig {
            paths: vec![
                "generated/output.txt".to_owned(),
                "gen".to_owned(),
                "docs/".to_owned(),
                "assets/*.json".to_owned(),
            ],
            command: "regenerate".to_owned(),
            when: String::new(),
            build_command: String::new(),
        };
        assert!(config.covers("generated/output.txt"));
        assert!(config.covers("gen/all.txt"));
        assert!(config.covers("docs/guide/index.html"));
        assert!(config.covers("assets/deep/data.json"));
        assert!(!config.covers("genesis/all.txt"));
        assert!(!config.covers("src/gen.rs"));
        assert!(!config.covers("assets/data.xml"));
        // No command or no paths: nothing is regenerable.
        let mut disabled = config.clone();
        disabled.command = String::new();
        assert!(!disabled.covers("gen/all.txt"));
    }

    #[test]
    fn config_tolerates_and_preserves_unknown_fields() {
        // A config written by a newer build must load on an older binary:
        // rejecting it bricks every command on the lagging host.
        let text =
            r#"{"schema": "merge-queue-config/v1", "recount_pins": true, "future_flag": {"x": 1}}"#;
        let config: Config = serde_json::from_str(text).expect("unknown fields must not refuse");
        assert!(config.recount_pins);
        assert_eq!(config.unknown_fields.len(), 1);
        assert!(config.unknown_fields.contains_key("future_flag"));
        // Round-trip keeps the unknown field, so a rewrite by the older
        // binary does not silently delete the newer setting.
        let written = serde_json::to_string(&config).expect("serialize");
        assert!(written.contains("\"future_flag\""));
        let reread: Config = serde_json::from_str(&written).expect("reread");
        assert_eq!(reread, config);
        // A config with no unknown fields stays clean.
        let plain: Config =
            serde_json::from_str(r#"{"schema": "merge-queue-config/v1"}"#).expect("plain");
        assert!(plain.unknown_fields.is_empty());
    }

    #[test]
    fn an_unset_notify_registry_is_left_out_of_config_json() {
        // Older builds refuse unknown `notify` fields, so a config that does not set the
        // registry must not mention it.
        let written = serde_json::to_value(Config::default()).expect("serialize");
        assert_eq!(written["notify"], serde_json::json!({"agentctl": []}));
        let mut config = Config::default();
        config.notify.registry = "/team/.agentctl".to_owned();
        let written = serde_json::to_string(&config).expect("serialize");
        let reread: Config = serde_json::from_str(&written).expect("reread");
        assert_eq!(reread.notify.registry, "/team/.agentctl");
    }

    #[test]
    fn classify_matches_ids_and_steps() {
        let rules = vec![
            ClassRule {
                pattern: "test.crate_count".into(),
                class: "count pin drift".into(),
                ..ClassRule::default()
            },
            ClassRule {
                pattern: "*_unit".into(),
                class: "count pin drift".into(),
                ..ClassRule::default()
            },
            ClassRule {
                pattern: "lint.*".into(),
                class: "lint".into(),
                ..ClassRule::default()
            },
        ];
        let failed = [
            TestId::parse("test.crate_count::count").unwrap(),
            TestId::step("suite_unit"),
            TestId::step("build"),
        ];
        assert_eq!(
            classify(&rules, &failed, None),
            vec!["count pin drift".to_owned()]
        );
        assert_eq!(
            classify(
                &rules,
                &[TestId::step("lint.fmt"), TestId::step("suite_unit")],
                None
            ),
            vec!["count pin drift".to_owned(), "lint".to_owned()]
        );
        assert!(classify(&rules, &[TestId::step("other")], None).is_empty());
    }

    #[test]
    fn a_class_with_evidence_needs_the_failure_to_say_so() {
        let rules = vec![ClassRule {
            pattern: "test.regular_crates".into(),
            class: "count pin drift".into(),
            evidence: "expected * tests to execute, saw *".into(),
        }];
        let failed = [TestId::step("test.regular_crates")];
        let drift = "[test.regular_crates] nextest-test-results: expected 1955 tests to execute, saw 1957; refusing\n";
        // Real test failures in the same node are not pin drift.
        let real = "[test.regular_crates] test verify_run_strict_clock_determinism ... FAILED\n\
                    [test.regular_crates] test result: FAILED. 1970 passed; 5 failed\n";
        assert_eq!(
            classify(&rules, &failed, Some(drift)),
            vec!["count pin drift".to_owned()]
        );
        assert!(classify(&rules, &failed, Some(real)).is_empty());
        // No log, no evidence, no label.
        assert!(classify(&rules, &failed, None).is_empty());
        // The evidence must be on the failed step's own lines.
        let other = "[test.regular_crates_on_host] nextest-test-results: expected 1 tests to execute, saw 2\n";
        assert!(classify(&rules, &failed, Some(other)).is_empty());
        assert!(classify_reads_log(&rules));
        assert!(!classify_reads_log(&[]));
        // An unset evidence is left out of config.json, so older builds still read it.
        let written = serde_json::to_string(&ClassRule {
            pattern: "x".into(),
            class: "y".into(),
            evidence: String::new(),
        })
        .unwrap();
        assert_eq!(written, r#"{"pattern":"x","class":"y"}"#);
    }
}
