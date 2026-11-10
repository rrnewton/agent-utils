//! Donor validators: other hosts validate queued candidates and return exact-SHA receipts.
//!
//! The queue host publishes each validation as a job in a small store beside the queue state
//! (`<state>/donor/`). A donor, on this host or another, reaches the store only by running this
//! command-line tool on the queue host through a configured command route (node-exec, ssh, or a
//! local path): `donor claim`, `renew`, `bundle`, `report` and `release`. That route is the
//! authentication; the lease token a claim returns binds every later call to that claim.
//!
//! A job is `queued`, then `leased` to one donor at a time, then `done` with an accepted receipt
//! or `cancelled` by the queue. A lease the donor stops renewing expires and the job is queued
//! again, so a job never runs twice at once and a dead donor costs one lease period. A receipt is
//! accepted only for the live lease, the job's exact commit and tree, and the tool identity the
//! queue computed when it published the job; it then stands in for a local validation.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::state::ValidatorConfig;
use crate::tests::{Selection, TestId, TestResults};

/// Schema of a job record.
pub const JOB_SCHEMA: &str = "merge-queue-donor-job/v1";
/// Schema of a receipt.
pub const RECEIPT_SCHEMA: &str = "merge-queue-receipt/v1";

/// Why a compare-mode job outlives its run: its candidate landed, and the donor holding the
/// lease may still report a receipt worth comparing (see [`Job::candidate_gone`]). A candidate
/// that did not land gets its job cancelled instead.
pub const CANDIDATE_LANDED: &str = "its candidate landed";
/// A repository URL reduced to what names the repository, so the forms of one repository
/// compare equal: the scheme (`https`, `http`, `ssh`, `git`, `git+ssh`, or scp-like
/// `user@host:path`), the user, the port, a trailing `/` and a `.git` suffix are dropped, and
/// the host is lowercased. A local path (or `file://` URL) stays a path.
pub fn repository_key(url: &str) -> String {
    let url = url.trim();
    let trim_path = |path: &str| {
        let path = path.trim_matches('/');
        path.strip_suffix(".git")
            .unwrap_or(path)
            .trim_end_matches('/')
            .to_owned()
    };
    let remote = |authority: &str, path: &str| {
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        format!("{host}/{}", trim_path(path))
    };
    let local = |path: &str| {
        let path = path.trim_end_matches('/');
        format!(
            "file:{}",
            path.strip_suffix(".git")
                .unwrap_or(path)
                .trim_end_matches('/')
        )
    };
    if let Some(path) = url.strip_prefix("file://") {
        return local(path);
    }
    if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        return remote(authority, path);
    }
    // scp-like `[user@]host:path`: a colon before any slash (a local path has none).
    match url.split_once(':') {
        Some((authority, path)) if !authority.contains('/') && !authority.is_empty() => {
            remote(authority, path)
        }
        _ => local(url),
    }
}

/// Whether two repository URLs name the same repository (see [`repository_key`]).
pub fn same_repository(a: &str, b: &str) -> bool {
    repository_key(a) == repository_key(b)
}

/// `donor info` output: what a donor checks before it claims anything.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct QueueInfo {
    /// URL of the queue repository's remote, or `None` for a purely local queue.
    #[serde(default)]
    pub remote_url: Option<String>,
    /// The queue host's merge-queue revision, when its binary recorded one.
    #[serde(default)]
    pub merge_queue: Option<crate::build_info::Revision>,
}

/// The URL of `remote` in the repository at `dir`, or `remote` itself when it is already a URL.
pub fn remote_url(dir: &Path, remote: &str) -> Option<String> {
    if remote.is_empty() {
        return None;
    }
    git_in(dir, &["remote", "get-url", remote])
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .or_else(|| (remote.contains("://") || remote.contains(':')).then(|| remote.to_owned()))
}

/// Seconds a lease lasts without a renewal.
pub const DEFAULT_LEASE_SECS: u64 = 120;

/// Consecutive probe-pass/launch-refuse cycles after which a host fault
/// escalates by default (see `Store::note_probe_pass`).
pub const DEFAULT_HOST_FAULT_ESCALATE_AFTER: usize = 5;

/// Where a job is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    /// Waiting for a donor.
    Queued,
    /// Held by one donor under a lease.
    Leased,
    /// An accepted receipt is attached.
    Done,
    /// The queue no longer wants it (its candidate was discarded, or the queue stopped).
    Cancelled,
}

/// What a donor must run to count as the queue's own validator.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolIdentity {
    /// Digest of the validator settings the job carries.
    pub validator_digest: String,
    /// Output of each `validator.tool_identity` command (for example the commit of each tool
    /// repository), by name, as the queue host answered when it published the job.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, String>,
    /// Source revision of the merge-queue binary (see `build.rs`). Builds of one revision are
    /// not byte-identical across hosts, so the revision is compared, never a binary's hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_queue: Option<crate::build_info::Revision>,
}

/// A verdict reduced to what two validations of one commit can be compared on.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Green: exit 0, every result non-blocking, not refused.
    pub green: bool,
    /// Admission refused to run (neither green nor red).
    #[serde(default)]
    pub refused: bool,
    /// Attributed failed tests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
    /// First log line naming the failure (#1198), when the receipt carried
    /// one (or its tail still holds one): the verdict detail an ejection
    /// quotes so it names the failing test or suite.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_line: String,
}

/// One donor's hold on a job.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    /// Donor name.
    pub donor: String,
    /// Host the donor runs on.
    pub host: String,
    /// Secret returned only to the claimant; every later call must present it.
    pub token: String,
    /// Unix seconds of the claim.
    pub claimed_unix: u64,
    /// Unix seconds after which the lease is gone unless renewed.
    pub expires_unix: u64,
    /// The donor declared at claim time that it reports admission
    /// (`Job::admitted`) before execution. Leases recorded before this
    /// field existed, and donors that do not declare it, keep the old
    /// rule that a lease itself counts as started.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reports_admission: bool,
}

/// A validation published for donors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    /// [`JOB_SCHEMA`].
    pub schema: String,
    /// Job identifier (also its file name).
    pub id: String,
    /// Exact candidate commit to validate.
    pub commit: String,
    /// Its tree.
    pub tree: String,
    /// A commit on the shared remote the candidate descends from; the bundle excludes it.
    pub base: String,
    /// The candidate's parent commit: the base itself for a root batch, the
    /// stacked-on candidate for a stacked one. Handed to the validator as
    /// `MQ_PARENT` (with `MQ_BASE` from `base`). Empty in job files written
    /// before the parent was recorded; the donor then falls back to `base`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent: String,
    /// Tests to run.
    pub selection: Selection,
    /// Tests to run beyond the complete suite, with an `all` selection (`MQ_EXTRA_TESTS`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub extra: BTreeSet<TestId>,
    /// Validator settings to run them with.
    pub validator: ValidatorConfig,
    /// Identity a donor's tool must match.
    pub tool: ToolIdentity,
    /// RFC 3339 UTC publication time.
    pub created_at: String,
    /// Current state.
    pub state: JobState,
    /// The live lease, while leased.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<Lease>,
    /// The current lease's donor has admitted the job: its pre-run checks
    /// passed and validation is about to execute. A lease alone is not
    /// admission; tool or admission waits can hold a leased job for
    /// minutes. Absent in job files written before admission was reported.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admitted: bool,
    /// The accepted receipt, once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
    /// Claims so far (a re-queue after an expired or released lease adds one).
    #[serde(default)]
    pub attempts: u32,
    /// What happened to the job, oldest first. Never holds a lease token.
    #[serde(default)]
    pub history: Vec<String>,
    /// Donors that declined the job (no base, bundle too big); a claim never hands it to
    /// them again unless the job is re-stamped. A tools mismatch is not a decline: the
    /// donor gives the job back and may claim it again (see `Worker::validate`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declined_by: Vec<String>,
    /// The queue's own verdict for the same commit (offload `compare`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Verdict>,
    /// `match`, or `mismatch: ...`, once both the local verdict and a receipt are in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<String>,
    /// Why the queue no longer wants the candidate, when a compare-mode job was kept for the
    /// donor holding its lease: its receipt is still compared (it cannot affect landing), but
    /// the job is cancelled instead of queued again if the lease ends without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_gone: Option<String>,
    /// Receipts that said validation could not run (admission refused, exit 75), as
    /// `donor: detail`, oldest first. None is a verdict: each re-queued the job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub could_not_run: Vec<String>,
    /// Accepted receipts whose validator printed no identity line for a
    /// full-hex stamped tool, as `donor: detail` (#1049): counted in
    /// `donor status` like `could_not_run`, so an operator can watch a
    /// consumer's unattested count reach zero before enabling
    /// `require_tool_identity_lines`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_identity_unattested: Vec<String>,
    /// A duplicate published for compare-only donors (`donors.compare_only`): only they may
    /// claim it, and its receipt is a comparison, never the verdict.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compare_only: bool,
    /// The batch was built on the base itself as a disjoint root. Under
    /// `keep_stale_roots` its validation is still wanted after the base moves
    /// (a green result requeues the batch, a red one decides it), so the
    /// claim-time stale sweep and the run's own stale pruning leave it alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rooted: bool,
    /// Any change in the batch was prioritized: claims lease it ahead of
    /// unprioritized jobs (#1008 donor-claim-plan-order). Absent in job files
    /// written before claim order was recorded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prioritized: bool,
    /// The batch's index in the queue's plan: claims follow plan order, with
    /// job age as the tiebreak. Absent in job files written before claim
    /// order was recorded, and for jobs no planned batch matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_index: Option<usize>,
    /// The plan generation [`Self::plan_index`] was recorded under (#1196
    /// front-starve): claims honour an index only from the store's current
    /// generation, so an index matched to a superseded plan is ignored.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub plan_generation: u64,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

impl Job {
    /// A comparison recorded after the queue had stopped wanting the candidate (a donor slower
    /// than the queue host).
    pub fn compared_after_landing(&self) -> bool {
        self.comparison.is_some() && self.candidate_gone.is_some()
    }

    /// Whether the current (or a past) donor has admitted this job:
    /// a receipt proves a past admission, and `admitted` proves the
    /// live lease's. A lease with neither has not started in any way
    /// that should protect it from stale cancellation.
    pub fn was_admitted(&self) -> bool {
        self.admitted || self.receipt.is_some()
    }

    /// Whether stale sweeps must keep this job's lease: it was admitted,
    /// or its lease comes from a donor that never declared admission
    /// reporting (including every lease recorded before the declaration
    /// existed), for which a lease alone has always counted as started.
    pub fn stale_protected(&self) -> bool {
        self.was_admitted()
            || self
                .lease
                .as_ref()
                .is_some_and(|lease| !lease.reports_admission)
    }

    /// Whether any donor has ever started this job: it was claimed at
    /// least once (attempts, or a lease or receipt in its history), even
    /// if it is queued again now after a could-not-run refusal or an
    /// expired lease. A job merely published and never claimed has not
    /// started, whatever its current state says.
    pub fn was_started(&self) -> bool {
        self.attempts > 0
            || self.lease.is_some()
            || self.receipt.is_some()
            || self
                .history
                .iter()
                .any(|line| line.contains(": leased to "))
    }

    /// The job's attempt history in one line — how many claims, how they
    /// ended, and which donors held it — for a cancellation reason or a
    /// decision record, so a requeued job never reads as never-started.
    pub fn attempt_summary(&self) -> String {
        let expired = self
            .history
            .iter()
            .filter(|line| line.contains("expired;"))
            .count();
        let mut donors: Vec<&str> = Vec::new();
        for line in &self.history {
            if let Some((_, rest)) = line.split_once(": leased to ") {
                let donor = rest.split(" on ").next().unwrap_or_default();
                if !donor.is_empty() && !donors.contains(&donor) {
                    donors.push(donor);
                }
            }
        }
        format!(
            "{} attempt(s): {} could-not-run, {} lease(s) expired{}",
            self.attempts,
            self.could_not_run.len(),
            expired,
            if donors.is_empty() {
                String::new()
            } else {
                format!("; leased to {}", donors.join(", "))
            }
        )
    }
}

impl Receipt {
    /// The receipt's verdict.
    pub fn verdict(&self) -> Verdict {
        Verdict {
            green: !self.refused && self.exit == Some(0) && self.results.green(),
            refused: self.refused,
            failed: self
                .results
                .attributed_failures()
                .iter()
                .map(ToString::to_string)
                .collect(),
            // The receipt's carried naming lines first; an older receipt
            // without them falls back to what its tail still holds (#1198).
            failure_line: self
                .failure_lines
                .first()
                .cloned()
                .or_else(|| crate::validate::first_failure_line(&self.log_tail, &[]))
                .unwrap_or_default(),
        }
    }
}

/// `match` or a one-line description of how two verdicts for one commit differ.
pub fn compare(local: &Verdict, donor: &Verdict) -> String {
    if local == donor {
        return "match".to_owned();
    }
    let word = |v: &Verdict| {
        if v.refused {
            "refused"
        } else if v.green {
            "green"
        } else {
            "red"
        }
    };
    let failed = |v: &Verdict| {
        if v.failed.is_empty() {
            String::new()
        } else {
            format!(" (failed: {})", v.failed.join(" "))
        }
    };
    let naming = |v: &Verdict| {
        if v.failure_line.is_empty() {
            String::new()
        } else {
            format!(" (first failure: {})", v.failure_line)
        }
    };
    format!(
        "mismatch: local {}{}, donor {}{}{}",
        word(local),
        failed(local),
        word(donor),
        failed(donor),
        naming(donor)
    )
}

/// Who produced a receipt.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DonorId {
    /// Donor name.
    pub name: String,
    /// Host.
    pub host: String,
}

/// A donor's report of one validation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    /// [`RECEIPT_SCHEMA`].
    pub schema: String,
    /// Job it answers.
    pub job: String,
    /// Producer.
    pub donor: DonorId,
    /// Commit the donor validated.
    pub commit: String,
    /// Tree the donor validated.
    pub tree: String,
    /// Tool identity the donor checked before it ran.
    pub tool: ToolIdentity,
    /// Tool trees the validator attested from inside the run (its
    /// `merge-queue-tool-identity:` lines, #1045): tool name -> every value
    /// printed. Receipts written before this field existed parse with it
    /// empty. A validation that ran owes a line per full-hex stamped tool:
    /// under `require_tool_identity_lines` the queue refuses a missing one
    /// as identity-mismatch; by default (#1049) it accepts and records the
    /// gap (see `tool_warnings`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ran_tools: BTreeMap<String, Vec<String>>,
    /// Donor-recorded tool warnings, e.g. `tool_identity_unattested: ...`
    /// for a full-hex stamped tool the validator printed no identity line
    /// for (#1049). Warnings never carry a verdict by themselves; the
    /// queue re-derives the same fact in `tool_check`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_warnings: Vec<String>,
    /// Why admission refused, as the one cause line the donor picked from
    /// the full runner log before cutting `log_tail` (#1059): a refusal's
    /// cause can sit far above a verbose teardown, outside the tail the
    /// queue would otherwise search. Only set on a refused receipt.
    /// Receipts written before this field existed parse without it, and
    /// the queue then picks the cause from `log_tail` as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_cause: Option<String>,
    /// The donor's merge-queue version.
    pub merge_queue_version: String,
    /// Selection actually run, when the runner collapsed the requested one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_selection: Option<Selection>,
    /// Runner exit status (`None` when killed by the timeout).
    pub exit: Option<i32>,
    /// Admission refused to run at all (neither green nor red).
    #[serde(default)]
    pub refused: bool,
    /// The validation stopped at an early red (a streamed deterministic
    /// failure, or the runner's own early verdict). Receipts written
    /// before this field existed parse with it off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub early_red: bool,
    /// Per-test results.
    pub results: TestResults,
    /// Wall time of the validation.
    pub elapsed_ms: u64,
    /// CPU time of the validation on the donor, when it could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<u64>,
    /// RFC 3339 UTC start and finish.
    pub started_at: String,
    /// RFC 3339 UTC finish.
    pub finished_at: String,
    /// Lines of the full runner log naming a failure (#1198), picked
    /// before the tail cut below, bounded per receipt: the line naming a
    /// failed suite can sit far above the tail. Receipts written before
    /// this field existed parse with it empty, and the queue then picks
    /// the naming line from `log_tail` as it can.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_lines: Vec<String>,
    /// The end of the runner's log.
    #[serde(default)]
    pub log_tail: String,
    /// Where the full log stays, on the donor host.
    #[serde(default)]
    pub log_path: String,
}

/// What a renewal tells the donor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "renewal", rename_all = "kebab-case")]
pub enum Renewal {
    /// Keep going; the lease now lasts until `expires_unix`.
    Continue {
        /// New expiry.
        expires_unix: u64,
    },
    /// Stop: the job was cancelled or the lease is no longer this donor's.
    Stop {
        /// Why.
        reason: String,
    },
}

/// The job store under `<state>/donor/`.
pub struct Store {
    dir: PathBuf,
    /// Lease length in seconds.
    pub lease_secs: u64,
    /// Could-not-run receipts one job may collect before it is cancelled, and one donor may
    /// return for it before it is left out (`donors.max_could_not_run`).
    pub could_not_run_limit: usize,
    /// The queue host's `validator.tool_identity` answers now, when known. [`Store::claim`]
    /// re-stamps a queued job published under older answers with these first.
    pub current_tools: Option<BTreeMap<String, String>>,
    /// Donor names that only ever compare: they claim only `compare_only` duplicates, and
    /// deciding donors never claim those.
    pub compare_only: std::collections::BTreeSet<String>,
    /// Consecutive probe-pass/launch-refuse cycles after which a host fault escalates:
    /// a probe pass alone no longer clears it, only an admitted validation does.
    pub host_fault_escalate_after: usize,
    /// Repository in which tool identities can be resolved to git objects for the
    /// descendant check (receipt acceptance), when the queue host has one. `None`
    /// keeps receipt tool checks exact.
    pub tool_repo: Option<PathBuf>,
}

/// The store's lock, held only for one short operation.
pub struct StoreLock(std::fs::File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// Write `bytes` to `path` readable by the owner only (job files hold lease tokens).
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Where the queue keeps each donor's last claimed cap (#918 donor-cap-file).
const DONOR_CAPS_FILE: &str = "donor-caps.json";

/// How often an unchanged cap is noted again, so `donor status` can say when a donor was last seen.
const DONOR_CAP_REFRESH_SECS: u64 = 60;

/// What a donor's cap file says (#918 donor-cap-file): `Ok(None)` when there is no file,
/// `Ok(Some(n))` for a whole number of jobs, `Err(text)` for anything else.
pub fn read_cap_file(path: &Path) -> Result<Option<usize>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse::<usize>()
            .map(Some)
            .map_err(|_| text.trim().to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("unreadable: {error}")),
    }
}

/// The cap a donor started with `--jobs jobs` claims under, given what its cap file says: the
/// file's number where it is lower, else `jobs`; a missing or malformed file leaves `jobs`.
pub fn effective_cap(jobs: usize, file: &Result<Option<usize>, String>) -> usize {
    match file {
        Ok(Some(cap)) => (*cap).min(jobs),
        _ => jobs,
    }
}

/// The wall clock in Unix seconds, for what the CLI notes and shows of donors.
pub fn unix_seconds() -> u64 {
    unix_now()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A fresh 256-bit secret, hex encoded.
fn token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("cannot read /dev/urandom: {error}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare two secrets without an early exit on the first differing byte.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Digest of the validator settings: `git hash-object` of their JSON, so both hosts compute it
/// with tools they already have.
pub fn validator_digest(validator: &ValidatorConfig) -> Result<String, String> {
    let json = serde_json::to_string(validator).map_err(|error| error.to_string())?;
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run git hash-object: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(json.as_bytes())
            .map_err(|error| format!("cannot feed git hash-object: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot run git hash-object: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Answers of the configured `validator.tool_identity` commands, run in `cwd`. Each names one
/// tool the validation depends on (a tool repository's commit, a toolchain version); both hosts
/// run them, and a donor whose tools answer differently refuses the job before it runs.
pub fn tool_answers(
    validator: &ValidatorConfig,
    cwd: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let mut answers = BTreeMap::new();
    for (name, command) in &validator.tool_identity {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("cannot run tool identity {name:?}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "tool identity {name:?} failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        answers.insert(
            name.clone(),
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        );
    }
    Ok(answers)
}

/// The identity the queue requires for `validator`, computed on this host. A merge-queue
/// binary with no clean source revision cannot be matched by any donor, so it is refused.
pub fn tool_identity(validator: &ValidatorConfig, cwd: &Path) -> Result<ToolIdentity, String> {
    let revision = crate::build_info::revision();
    if let Some(problem) = crate::build_info::unusable(revision.as_ref()) {
        return Err(format!("{problem}; donors compare it with their own"));
    }
    Ok(ToolIdentity {
        validator_digest: validator_digest(validator)?,
        tools: tool_answers(validator, cwd)?,
        merge_queue: revision,
    })
}

/// How the donor's merge-queue revision differs from the queue host's, or `None` when they
/// agree: the donor's is usable (recorded, and clean unless dirty builds are allowed) and
/// equal to the queue host's.
pub fn revision_mismatch(
    queue: Option<&crate::build_info::Revision>,
    donor: Option<&crate::build_info::Revision>,
) -> Option<String> {
    if let Some(problem) = crate::build_info::unusable(donor) {
        return Some(problem);
    }
    let describe = |revision: Option<&crate::build_info::Revision>| {
        revision.map_or_else(|| "no recorded revision".to_owned(), |r| r.describe())
    };
    (queue != donor).then(|| {
        format!(
            "this donor's merge-queue is built from {}, the queue host's from {}",
            describe(donor),
            describe(queue)
        )
    })
}

/// Why a claim declaring `declared` must be refused before leasing
/// (#1105), or `None` when it may claim: a mismatch (or an unusable
/// declaration) refuses with [`revision_mismatch`]'s wording, the same
/// the receipt path uses after the work is done. No declaration (an
/// older donor) is never a refusal: it claims exactly as before the
/// gate existed.
pub fn claim_refusal(
    queue: Option<&crate::build_info::Revision>,
    declared: Option<&crate::build_info::Revision>,
) -> Option<String> {
    declared.and_then(|donor| revision_mismatch(queue, Some(donor)))
}

/// Names of the tools whose answers differ from the job's.
pub fn mismatched_tools(
    wanted: &BTreeMap<String, String>,
    have: &BTreeMap<String, String>,
) -> Vec<String> {
    wanted
        .iter()
        .filter(|(name, value)| have.get(*name) != Some(value))
        .map(|(name, _)| name.clone())
        .collect()
}

/// How long one `validator.tool_refresh` command may run on a donor.
const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long one `validator.tool_refresh` command may run on the queue host at claim
/// time: long enough for a fetch, short enough that a claim is not held up by it.
pub const QUEUE_REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Least time between queue-host claim-time refreshes: concurrent claims share one
/// refresh instead of fetching in a storm.
pub const QUEUE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Bring the tools `names` up to date on this host: run each one's `validator.tool_refresh`
/// command in `cwd`, bounded by [`REFRESH_TIMEOUT`]. Fails on a tool with no such command, or a
/// command that fails or runs out of time.
pub fn refresh_tools(
    validator: &ValidatorConfig,
    cwd: &Path,
    names: &[String],
) -> Result<(), String> {
    refresh_tools_with_timeout(validator, cwd, names, REFRESH_TIMEOUT)
}

/// Like [`refresh_tools`], with a caller-chosen bound on each command.
pub fn refresh_tools_with_timeout(
    validator: &ValidatorConfig,
    cwd: &Path,
    names: &[String],
    timeout: std::time::Duration,
) -> Result<(), String> {
    for name in names {
        let command = validator
            .tool_refresh
            .get(name)
            .ok_or_else(|| format!("validator.tool_refresh has no command for {name:?}"))?;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run the refresh of {name:?}: {error}"))?;
        let stderr = child.stderr.take();
        let errors = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_end(&mut buffer);
            }
            buffer
        });
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("cannot wait for the refresh of {name:?}: {error}"))?
            {
                break status;
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the refresh of {name:?} ran out of time ({}s)",
                    timeout.as_secs()
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let errors = errors.join().unwrap_or_default();
        if !status.success() {
            return Err(format!(
                "the refresh of {name:?} failed (exit {}): {}",
                status.code().unwrap_or(-1),
                String::from_utf8_lossy(&errors).trim()
            ));
        }
    }
    Ok(())
}

/// How a donor built before mismatch releases began the reason it gave for declining a job its
/// tools or merge-queue revision did not match. The queue records such a decline as a release:
/// the donor is offered the job again after its own back-off, so it takes the job once it has
/// been upgraded or its tools have moved.
pub const LEGACY_MISMATCH_DECLINE: &str = "this donor's tools differ from the queue host's: ";

/// Most first-parent commits searched for a job's stamped tool tree when proving a
/// donor's newer tree descends from it (bounded descendant acceptance, #1005).
pub const TOOL_WALK_BOUND: usize = 1024;

/// Warnings for full-hex stamped tools the validator printed no
/// `merge-queue-tool-identity` line for (#1049): the donor records these
/// on its receipt, and the queue re-derives the same list in
/// [`Store::tool_check`] (a receipt cannot talk its way out of, or into,
/// an attestation by editing its own warnings).
fn unattested_warnings(
    stamped: &BTreeMap<String, String>,
    printed: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    stamped
        .iter()
        .filter(|(name, stamp)| {
            full_hex(stamp)
                && !printed
                    .get(*name)
                    .is_some_and(|values| values.iter().any(|value| !value.is_empty()))
        })
        .map(|(name, _)| {
            format!(
                "tool_identity_unattested: tool {name} printed no {} line",
                crate::validate::TOOL_IDENTITY_MARKER
            )
        })
        .collect()
}

/// Whether `value` is a full-length hex object name (SHA-1 or SHA-256).
fn full_hex(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The `<ref>` and path of a `git rev-parse <ref>:<path>` identity command, or
/// `None` for any other shape. `git -C <dir> rev-parse origin/main:ci-hub` and
/// `<ref>^{tree}` (the root tree, empty path) are recognised.
fn rev_parse_spec(command: &str) -> Option<(String, String)> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let at = words.iter().position(|word| *word == "rev-parse")?;
    let spec = words.get(at + 1)?;
    if let Some(reference) = spec.strip_suffix("^{tree}") {
        return Some((reference.to_owned(), String::new()));
    }
    let (reference, path) = spec.split_once(':')?;
    Some((reference.to_owned(), path.to_owned()))
}

/// Expand a leading `~/` (or a bare `~`) in `dir` to `$HOME`; `None` when the
/// directory names no home or `$HOME` is unset.
fn expand_home(dir: &str) -> Option<PathBuf> {
    if dir == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    let rest = dir.strip_prefix("~/")?;
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(rest))
}

/// The repository an identity command names, or why it cannot be named: its
/// `git -C <dir>` directory (a leading `~/` expanded to `$HOME`, an absolute
/// `<dir>` standing as itself, a relative one resolved against `base`), or
/// `base` itself when the command names none. A resolved directory that does
/// not exist is a named error rather than a silent failed proof: joining a
/// `~/` directory to `base` proved nothing that way (#1005).
pub fn tool_repo_checked(command: &str, base: &Path) -> Result<PathBuf, String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    for pair in words.windows(2) {
        if pair[0] == "-C" {
            let resolved = if let Some(expanded) = expand_home(pair[1]) {
                expanded
            } else {
                let dir = Path::new(pair[1]);
                if dir.is_absolute() {
                    dir.to_path_buf()
                } else {
                    base.join(dir)
                }
            };
            if !resolved.exists() {
                return Err(format!(
                    "tool identity command {command:?} names the -C directory {:?}, \
                     resolved to {}, which does not exist",
                    pair[1],
                    resolved.display()
                ));
            }
            return Ok(resolved);
        }
    }
    Ok(base.to_path_buf())
}

/// The repository an identity command names: its `git -C <dir>` directory
/// resolved against `base` (an absolute `<dir>` stands), or `base` itself when
/// the command names none.
pub fn tool_repo_of(command: &str, base: &Path) -> PathBuf {
    tool_repo_checked(command, base).unwrap_or_else(|_| base.to_path_buf())
}

/// The first commit on the bounded first-parent walk from `head` whose tree at
/// `path` is `tree`, if any.
fn tree_in_walk(repo: &Path, head: &str, tree: &str, path: &str) -> Option<String> {
    let bound = (TOOL_WALK_BOUND + 1).to_string();
    let walk = git_in(
        repo,
        &[
            "rev-list",
            "--first-parent",
            &format!("--max-count={bound}"),
            head,
        ],
    )
    .ok()?;
    for commit in walk.lines().take(TOOL_WALK_BOUND) {
        let at_path = if path.is_empty() {
            format!("{commit}^{{tree}}")
        } else {
            format!("{commit}:{path}")
        };
        if git_in(repo, &["rev-parse", &at_path]).ok().as_deref() == Some(tree) {
            return Some(commit.to_owned());
        }
    }
    None
}

/// Whether the donor's tool answer `have` for `name` provably descends from the
/// job's stamp in `repo` (bounded descendant acceptance, #1005). The answer
/// must also sit on this host's own history: found in the bounded first-parent
/// walk from the identity ref's current tip (which may have moved past the
/// donor's commit since it answered), or itself descending from that tip after
/// the caller's fetch. Anything unprovable — non-git answers, missing objects,
/// unrelated trees — is `false`, and the caller keeps the exact-match
/// behaviour. A repository that cannot be named at all is an error.
pub fn tool_descends_from(
    validator: &ValidatorConfig,
    name: &str,
    stamp: &str,
    have: &str,
    repo: &Path,
) -> Result<bool, String> {
    if stamp == have {
        return Ok(true);
    }
    if !full_hex(stamp) || !full_hex(have) {
        return Ok(false);
    }
    let repo = &match validator.tool_identity.get(name) {
        Some(command) => tool_repo_checked(command, repo)?,
        None => repo.to_path_buf(),
    };
    let Ok(stamp_kind) = git_in(repo, &["cat-file", "-t", stamp]) else {
        return Ok(false);
    };
    let Ok(have_kind) = git_in(repo, &["cat-file", "-t", have]) else {
        return Ok(false);
    };
    let git = crate::git::Git::new(repo);
    match (stamp_kind.as_str(), have_kind.as_str()) {
        ("commit", "commit") => {
            if !git.is_ancestor(stamp, have).unwrap_or(false) {
                return Ok(false);
            }
            // With a derivable tip, the donor's commit must also sit on the
            // queue's own history: in its bounded first-parent walk, or (the
            // donor ahead of this host, after the caller's fetch) descending
            // from its tip.
            let tip = validator
                .tool_identity
                .get(name)
                .and_then(|command| rev_parse_spec(command))
                .and_then(|(reference, _)| {
                    git_in(repo, &["rev-parse", &format!("{reference}^{{commit}}")]).ok()
                });
            match tip {
                Some(tip) => Ok(git.is_ancestor(have, &tip).unwrap_or(false)
                    || git.is_ancestor(&tip, have).unwrap_or(false)),
                None => Ok(true),
            }
        }
        ("tree", "tree") => {
            let Some(command) = validator.tool_identity.get(name) else {
                return Ok(false);
            };
            let Some((reference, path)) = rev_parse_spec(command) else {
                return Ok(false);
            };
            let Ok(tip) = git_in(repo, &["rev-parse", &format!("{reference}^{{commit}}")]) else {
                return Ok(false);
            };
            // The queue's tip may have moved past the donor's commit since the
            // donor answered: the donor's tree need only appear in the bounded
            // first-parent walk from the current tip, with the stamp below it.
            if let Some(have_commit) = tree_in_walk(repo, &tip, have, &path) {
                if tree_in_walk(repo, &have_commit, stamp, &path).is_some() {
                    return Ok(true);
                }
            }
            // Or the donor is ahead of this host (its objects fetched, its
            // commit not yet on the tip's history): a commit descending from
            // the tip whose tree is the donor's answer, with the stamp below.
            let bound = (TOOL_WALK_BOUND + 1).to_string();
            if let Ok(all) = git_in(
                repo,
                &["rev-list", &format!("--max-count={bound}"), "--all"],
            ) {
                for commit in all.lines().take(TOOL_WALK_BOUND) {
                    let at_path = if path.is_empty() {
                        format!("{commit}^{{tree}}")
                    } else {
                        format!("{commit}:{path}")
                    };
                    if git_in(repo, &["rev-parse", &at_path]).ok().as_deref() == Some(have)
                        && git.is_ancestor(&tip, commit).unwrap_or(false)
                        && tree_in_walk(repo, commit, stamp, &path).is_some()
                    {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// How the donor's tools differ from the job's, or `None` when they agree.
pub fn tool_mismatch(
    wanted: &BTreeMap<String, String>,
    have: &BTreeMap<String, String>,
) -> Option<String> {
    let differences: Vec<String> = wanted
        .iter()
        .filter(|(name, value)| have.get(*name) != Some(value))
        .map(|(name, value)| {
            format!(
                "{name}: queue {value:?}, donor {:?}",
                have.get(name).map(String::as_str).unwrap_or("<missing>")
            )
        })
        .collect();
    (!differences.is_empty()).then(|| differences.join("; "))
}

impl Store {
    /// The store for the queue state rooted at `state_root`.
    pub fn new(state_root: &Path) -> Self {
        Self {
            dir: state_root.join("donor"),
            lease_secs: DEFAULT_LEASE_SECS,
            could_not_run_limit: crate::state::DEFAULT_RETRY_BOUND as usize,
            current_tools: None,
            compare_only: std::collections::BTreeSet::new(),
            host_fault_escalate_after: DEFAULT_HOST_FAULT_ESCALATE_AFTER,
            tool_repo: None,
        }
    }

    fn jobs_dir(&self) -> PathBuf {
        self.dir.join("jobs")
    }

    fn job_path(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("invalid job id {id:?}"));
        }
        Ok(self.jobs_dir().join(format!("{id}.json")))
    }

    /// Take the store lock (waits: every holder is brief).
    pub fn lock(&self) -> Result<StoreLock, String> {
        std::fs::create_dir_all(self.jobs_dir())
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in [&self.dir, &self.jobs_dir()] {
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let path = self.dir.join("lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        file.lock_exclusive()
            .map_err(|error| format!("cannot lock {}: {error}", path.display()))?;
        Ok(StoreLock(file))
    }

    /// When the queue host last attempted a claim-time tool refresh (unix seconds),
    /// or 0 when it never has. The attempt time is recorded whether the refresh
    /// succeeded or not, so a failing refresh is retried at most once per interval.
    pub fn queue_refresh_attempted_at(&self) -> u64 {
        std::fs::read_to_string(self.dir.join("tool-refresh-at"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Record a queue-host claim-time tool-refresh attempt now.
    pub fn note_queue_refresh_attempt(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = write_private(
            &self.dir.join("tool-refresh-at"),
            unix_now().to_string().as_bytes(),
        );
    }

    /// Whether a queue-host claim-time tool refresh is due: at least
    /// [`QUEUE_REFRESH_INTERVAL`] since the last attempt.
    pub fn queue_refresh_due(&self) -> bool {
        unix_now().saturating_sub(self.queue_refresh_attempted_at())
            >= QUEUE_REFRESH_INTERVAL.as_secs()
    }

    fn read(&self, id: &str) -> Result<Job, String> {
        let path = self.job_path(id)?;
        let text =
            std::fs::read_to_string(&path).map_err(|error| format!("no job {id}: {error}"))?;
        serde_json::from_str(&text).map_err(|error| format!("unreadable job {id}: {error}"))
    }

    fn write(&self, job: &Job) -> Result<(), String> {
        let path = self.job_path(&job.id)?;
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(job).map_err(|error| error.to_string())?;
        write_private(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, &path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// Every job, oldest first.
    pub fn jobs(&self) -> Result<Vec<Job>, String> {
        let mut jobs = Vec::new();
        let entries = match std::fs::read_dir(self.jobs_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(jobs),
            Err(error) => return Err(format!("cannot list donor jobs: {error}")),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            jobs.push(
                serde_json::from_str::<Job>(&text)
                    .map_err(|error| format!("unreadable {}: {error}", path.display()))?,
            );
        }
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(jobs)
    }

    /// Return every expired lease's job to the queue.
    fn expire(&self, at: u64) -> Result<(), String> {
        for mut job in self.jobs()? {
            if job.state != JobState::Leased {
                continue;
            }
            let Some(lease) = job.lease.clone() else {
                continue;
            };
            if lease.expires_unix <= at {
                job.lease = None;
                if let Some(gone) = &job.candidate_gone {
                    job.state = JobState::Cancelled;
                    job.history.push(format!(
                        "{}: lease of {} on {} expired; cancelled: {gone}",
                        now(),
                        lease.donor,
                        lease.host
                    ));
                } else {
                    job.state = JobState::Queued;
                    job.history.push(format!(
                        "{}: lease of {} on {} expired; queued again",
                        now(),
                        lease.donor,
                        lease.host
                    ));
                }
                self.write(&job)?;
            }
        }
        Ok(())
    }

    /// Remove finished jobs older than a week, and cancel jobs no donor took within a day
    /// (compare mode keeps publishing whether or not a donor runs).
    fn prune(&self) -> Result<(), String> {
        let at = chrono::Utc::now();
        for mut job in self.jobs()? {
            let Ok(created) = chrono::DateTime::parse_from_rfc3339(&job.created_at) else {
                continue;
            };
            let age = at.signed_duration_since(created);
            match job.state {
                JobState::Done | JobState::Cancelled if age > chrono::Duration::days(7) => {
                    let _ = std::fs::remove_file(self.job_path(&job.id)?);
                }
                JobState::Queued if age > chrono::Duration::days(1) => {
                    job.state = JobState::Cancelled;
                    job.history.push(format!(
                        "{}: cancelled: no donor took it within a day",
                        now()
                    ));
                    self.write(&job)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Publish a validation.
    #[allow(clippy::too_many_arguments)]
    pub fn publish(
        &self,
        commit: &str,
        tree: &str,
        base: &str,
        parent: &str,
        selection: &Selection,
        extra: &BTreeSet<TestId>,
        validator: &ValidatorConfig,
        tool: &ToolIdentity,
        rooted: bool,
        prioritized: bool,
        plan_index: Option<usize>,
    ) -> Result<Job, String> {
        let _lock = self.lock()?;
        self.prune()?;
        let id = format!(
            "{}-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%3f"),
            &commit[..commit.len().min(12)],
            &token()?[..8]
        );
        let job = Job {
            schema: JOB_SCHEMA.to_owned(),
            id,
            commit: commit.to_owned(),
            tree: tree.to_owned(),
            base: base.to_owned(),
            parent: parent.to_owned(),
            selection: selection.clone(),
            extra: extra.clone(),
            validator: validator.clone(),
            tool: tool.clone(),
            created_at: now(),
            state: JobState::Queued,
            lease: None,
            admitted: false,
            receipt: None,
            attempts: 0,
            history: Vec::new(),
            declined_by: Vec::new(),
            local: None,
            comparison: None,
            candidate_gone: None,
            could_not_run: Vec::new(),
            tool_identity_unattested: Vec::new(),
            compare_only: false,
            rooted,
            prioritized,
            plan_index,
            plan_generation: self.current_plan_generation(),
        };
        self.write(&job)?;
        Ok(job)
    }

    /// The store's current plan generation (0 before any plan was stamped).
    pub fn current_plan_generation(&self) -> u64 {
        std::fs::read_to_string(self.dir.join("plan-generation.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| value["generation"].as_u64())
            .unwrap_or(0)
    }

    /// Advance the plan generation and restamp queued jobs with their
    /// current indices (#1196 front-starve): jobs not named keep their old
    /// generation, so claims ignore their now-stale indices.
    pub fn set_plan_generation(
        &self,
        generation: u64,
        restamps: &[(String, Option<usize>)],
    ) -> Result<(), String> {
        let _lock = self.lock()?;
        write_private(
            &self.dir.join("plan-generation.json"),
            serde_json::json!({"generation": generation})
                .to_string()
                .as_bytes(),
        )?;
        for (id, index) in restamps {
            let Ok(mut job) = self.read(id) else {
                continue;
            };
            if job.state != JobState::Queued {
                continue;
            }
            job.plan_generation = generation;
            job.plan_index = *index;
            self.write(&job)?;
        }
        Ok(())
    }

    /// Publish the compare-only duplicate of `main`: the same candidate for donors that
    /// only ever compare. Its receipt never decides the batch.
    pub fn publish_compare_duplicate(&self, main: &Job) -> Result<Job, String> {
        let _lock = self.lock()?;
        let id = format!(
            "{}-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%3f"),
            &main.commit[..main.commit.len().min(12)],
            &token()?[..8]
        );
        let mut job = main.clone();
        job.id = id;
        job.state = JobState::Queued;
        job.lease = None;
        job.receipt = None;
        job.attempts = 0;
        job.created_at = now();
        job.history = vec![format!(
            "{}: published as the compare-only duplicate of {}",
            now(),
            main.id
        )];
        job.declined_by = Vec::new();
        job.local = None;
        job.comparison = None;
        job.candidate_gone = None;
        job.could_not_run = Vec::new();
        job.compare_only = true;
        self.write(&job)?;
        Ok(job)
    }

    /// Record, on both jobs, how a compare-only duplicate's receipt compares with the
    /// deciding job's receipt. A duplicate still running records nothing yet.
    pub fn record_comparison(&self, deciding: &str, compared: &str) -> Result<bool, String> {
        let _lock = self.lock()?;
        let deciding_job = self.read(deciding)?;
        let mut compared_job = self.read(compared)?;
        let (Some(deciding_receipt), Some(compared_receipt)) =
            (&deciding_job.receipt, &compared_job.receipt)
        else {
            return Ok(false);
        };
        let comparison = compare(&deciding_receipt.verdict(), &compared_receipt.verdict());
        let mut deciding_job = deciding_job;
        deciding_job.comparison = Some(comparison.clone());
        self.write(&deciding_job)?;
        compared_job.comparison = Some(comparison);
        self.write(&compared_job)?;
        Ok(true)
    }

    /// Lease the oldest queued job to `donor`, unless it already holds `max_jobs` leases.
    /// The returned job carries the lease token; nothing else ever returns it. With
    /// [`Store::current_tools`] set, queued jobs are re-stamped first
    /// ([`Store::restamp_queued`]).
    /// Lease a job to a donor that does not declare admission reporting:
    /// its lease keeps the old started rule (see `Job::stale_protected`).
    pub fn claim(&self, donor: &str, host: &str, max_jobs: usize) -> Result<Option<Job>, String> {
        self.claim_reporting(donor, host, max_jobs, false)
    }

    /// [`Self::claim`], declaring whether this donor reports admission
    /// before execution (`--reports-admission` on the claim verb).
    pub fn claim_reporting(
        &self,
        donor: &str,
        host: &str,
        max_jobs: usize,
        reports_admission: bool,
    ) -> Result<Option<Job>, String> {
        if donor.trim().is_empty() {
            return Err("a donor needs a name".to_owned());
        }
        let _lock = self.lock()?;
        let at = unix_now();
        self.expire(at)?;
        self.restamp_queued()?;
        // A donor with a host fault is leased nothing until it reports an admitted
        // validation or claims after a passing probe (cleared in the claim handler).
        // An escalated fault (see `note_host_fault`) keeps the fault recorded on a
        // probe pass, but arms one trial claim so an admitted validation can clear it.
        if let Some(fault) = self.host_faults().get(donor) {
            if fault["trial"].as_bool() != Some(true) {
                return Ok(None);
            }
        }
        let jobs = self.jobs()?;
        let held = jobs
            .iter()
            .filter(|job| {
                job.state == JobState::Leased
                    && job.lease.as_ref().is_some_and(|lease| lease.donor == donor)
            })
            .count();
        if held >= max_jobs.max(1) {
            return Ok(None);
        }
        let compare_only_donor = self.compare_only.contains(donor);
        // Claim order (#1008): prioritized jobs first, then plan order, with
        // job age (id) as the tiebreak. Only an index recorded under the
        // current plan generation counts (#1196 front-starve): an index
        // matched to a superseded plan (a recomposed flight restamped
        // against the run's first plan) is ignored, and a job with a
        // current index claims ahead of one without.
        let generation = self.current_plan_generation();
        let effective = |job: &Job| {
            if job.plan_generation == generation {
                job.plan_index
            } else {
                None
            }
        };
        let Some(mut job) = jobs
            .into_iter()
            .filter(|job| {
                job.state == JobState::Queued
                    && job.compare_only == compare_only_donor
                    && !job.declined_by.iter().any(|name| name == donor)
            })
            .min_by(|a, b| {
                std::cmp::Reverse(a.prioritized)
                    .cmp(&std::cmp::Reverse(b.prioritized))
                    .then_with(|| match (effective(a), effective(b)) {
                        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.id.cmp(&b.id)),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.id.cmp(&b.id),
                    })
            })
        else {
            return Ok(None);
        };
        job.state = JobState::Leased;
        job.attempts += 1;
        job.admitted = false;
        job.lease = Some(Lease {
            donor: donor.to_owned(),
            host: host.to_owned(),
            token: token()?,
            claimed_unix: at,
            expires_unix: at + self.lease_secs,
            reports_admission,
        });
        job.history
            .push(format!("{}: leased to {donor} on {host}", now()));
        self.write(&job)?;
        Ok(Some(job))
    }

    /// Give every queued job published under tool answers other than
    /// [`Store::current_tools`] the current ones. A tool repository that moved after the job
    /// was published (a fix landed in the validator's own tooling) leaves it stamped with
    /// answers no up-to-date donor has: each such donor (still different after its refresh)
    /// declines it and is never offered it again, so it would sit queued until it is pruned.
    /// The queue host validates with its
    /// current tools, so the job takes them too, and the donors that declined the old stamp
    /// may claim it again. Leased and finished jobs keep the stamp they ran under. The
    /// caller holds the store lock.
    fn restamp_queued(&self) -> Result<(), String> {
        let Some(current) = &self.current_tools else {
            return Ok(());
        };
        for mut job in self.jobs()? {
            if job.state != JobState::Queued || job.tool.tools == *current {
                continue;
            }
            let describe = |tools: &BTreeMap<String, String>| {
                tools
                    .iter()
                    .map(|(name, answer)| format!("{name}={answer}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let declined = std::mem::take(&mut job.declined_by);
            job.history.push(format!(
                "{}: re-stamped with the queue host's current tools ({} -> {}){}",
                now(),
                describe(&job.tool.tools),
                describe(current),
                if declined.is_empty() {
                    String::new()
                } else {
                    format!("; {} may claim it again", declined.join(", "))
                }
            ));
            job.tool.tools = current.clone();
            self.write(&job)?;
        }
        Ok(())
    }

    /// The job, if `token` is its live lease's.
    fn leased(&self, id: &str, token: &str, at: u64) -> Result<Job, String> {
        let job = self.read(id)?;
        match (&job.state, &job.lease) {
            (JobState::Leased, Some(lease))
                if same_secret(&lease.token, token) && lease.expires_unix > at =>
            {
                Ok(job)
            }
            (JobState::Cancelled, _) => Err(format!("job {id} was cancelled")),
            (JobState::Done, _) => Err(format!("job {id} is already done")),
            _ => Err(format!(
                "job {id} is not leased with that token (the lease expired or went to another donor)"
            )),
        }
    }

    /// Extend a live lease, or tell the donor to stop.
    pub fn renew(&self, id: &str, token: &str) -> Result<Renewal, String> {
        let _lock = self.lock()?;
        let at = unix_now();
        match self.leased(id, token, at) {
            Ok(mut job) => {
                let expires_unix = at + self.lease_secs;
                if let Some(lease) = job.lease.as_mut() {
                    lease.expires_unix = expires_unix;
                }
                self.write(&job)?;
                Ok(Renewal::Continue { expires_unix })
            }
            Err(reason) => Ok(Renewal::Stop { reason }),
        }
    }

    /// Record that the lease's donor has admitted the job: its pre-run
    /// checks passed and validation is about to execute. Until this is
    /// recorded, a leased rooted job counts as unstarted for the stale
    /// sweeps.
    pub fn admit(&self, id: &str, token: &str) -> Result<Job, String> {
        let _lock = self.lock()?;
        let mut job = self.leased(id, token, unix_now())?;
        if !job.admitted {
            job.admitted = true;
            let donor = job
                .lease
                .as_ref()
                .map(|lease| lease.donor.clone())
                .unwrap_or_default();
            job.history.push(format!("{}: admitted by {donor}", now()));
            self.write(&job)?;
        }
        Ok(job)
    }

    /// The job for a bundle request, if `token` is its live lease's.
    pub fn for_bundle(&self, id: &str, token: &str) -> Result<Job, String> {
        let _lock = self.lock()?;
        self.leased(id, token, unix_now())
    }

    /// Give a job back; it is queued again. With `decline` the donor never gets it again
    /// (it cannot run it here: wrong tools, no base, a bundle too big).
    pub fn release(
        &self,
        id: &str,
        token: &str,
        reason: &str,
        decline: bool,
    ) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut job = self.leased(id, token, unix_now())?;
        let donor = job.lease.take().map(|l| l.donor).unwrap_or_default();
        job.state = if job.candidate_gone.is_some() {
            JobState::Cancelled
        } else {
            JobState::Queued
        };
        // A mismatch is never a decline: the donor may be upgraded, or its tools may move.
        let mismatch = decline && reason.starts_with(LEGACY_MISMATCH_DECLINE);
        let decline = decline && !mismatch;
        job.history.push(format!(
            "{}: {} by {donor}{}: {reason}",
            now(),
            if decline { "declined" } else { "released" },
            if mismatch {
                " (declined for a mismatch, which is recorded as a release: it is offered the \
                 job again)"
            } else {
                ""
            }
        ));
        if decline && !job.declined_by.contains(&donor) {
            job.declined_by.push(donor.clone());
        }
        self.note_give_back(&donor, reason);
        self.write(&job)
    }

    /// Record the queue's own verdict for the job's commit (offload `compare`).
    pub fn record_local(&self, id: &str, verdict: Verdict) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut job = self.read(id)?;
        if let Some(receipt) = &job.receipt {
            job.comparison = Some(compare(&verdict, &receipt.verdict()));
        }
        job.local = Some(verdict);
        self.write(&job)
    }

    /// The receipt's tool refusal, if any, with acceptance notes for tools the
    /// donor ran newer than the stamp. The settings digest and the merge-queue
    /// revision must match exactly; each tool must match the stamp or provably
    /// descend from it in [`Store::tool_repo`] (bounded descendant acceptance,
    /// #1005). Without a tool repository the tools must match exactly. The same
    /// rule then binds the trees the validator attested from inside the run
    /// (`ran_tools`, #1045): a full-hex stamp owes exactly one attested tree,
    /// except that a MISSING line refuses only under
    /// `require_tool_identity_lines`; by default (#1049) it is accepted and
    /// recorded here as a `tool_identity_unattested` note (and counted on
    /// the job by [`Store::report`]). A present line is held to the rule in
    /// both modes.
    fn tool_check(&self, job: &Job, receipt: &Receipt) -> (Option<String>, Vec<String>) {
        if receipt.tool.validator_digest != job.tool.validator_digest {
            return (
                Some(format!(
                    "the receipt's tool identity {:?} is not the job's {:?}",
                    receipt.tool, job.tool
                )),
                Vec::new(),
            );
        }
        if receipt.tool.merge_queue != job.tool.merge_queue {
            return (
                Some(format!(
                    "the receipt's tool identity {:?} is not the job's {:?}",
                    receipt.tool, job.tool
                )),
                Vec::new(),
            );
        }
        let mut notes = Vec::new();
        let recorded = &receipt.tool.tools;
        let stamped = &job.tool.tools;
        if recorded.len() != stamped.len()
            || recorded.keys().any(|name| !stamped.contains_key(name))
        {
            return (
                Some(format!(
                    "the receipt's tool identity {:?} is not the job's {:?}",
                    receipt.tool, job.tool
                )),
                Vec::new(),
            );
        }
        for (name, stamp) in stamped {
            let have = &recorded[name];
            if have == stamp {
                continue;
            }
            let descends = match &self.tool_repo {
                Some(repo) => match tool_descends_from(&job.validator, name, stamp, have, repo) {
                    Ok(descends) => descends,
                    Err(error) => return (Some(error), Vec::new()),
                },
                None => false,
            };
            if descends {
                notes.push(format!(
                    "accepted with descendant tool {name} ({} -> {})",
                    crate::plan::short(stamp),
                    crate::plan::short(have)
                ));
            } else {
                return (
                    Some(format!(
                        "the receipt's tool {name} {have:?} neither matches the job's \
                         stamp {stamp:?} nor provably descends from it"
                    )),
                    Vec::new(),
                );
            }
        }
        // The trees the validator itself attested from inside the run (#1045):
        // the pre-run answers above say what the donor identified, these say
        // what actually ran, and the two must agree (or descend, #1005) or the
        // receipt certifies a validation nobody can name. A refused receipt
        // never ran, so it owes no attestation.
        if !stamped.is_empty()
            && !receipt.refused
            && receipt.exit != Some(crate::validate::REFUSAL_EXIT)
        {
            for name in receipt.ran_tools.keys() {
                if !stamped.contains_key(name) {
                    return (
                        Some(format!(
                            "identity-mismatch: the receipt's validator attested tool \
                             {name:?}, which the job does not stamp"
                        )),
                        Vec::new(),
                    );
                }
            }
            for (name, stamp) in stamped {
                let mut printed: Vec<&String> = Vec::new();
                if let Some(values) = receipt.ran_tools.get(name) {
                    for value in values {
                        if !printed.contains(&value) {
                            printed.push(value);
                        }
                    }
                }
                if full_hex(stamp) {
                    match printed.as_slice() {
                        [] if job.validator.require_tool_identity_lines => {
                            return (
                                Some(format!(
                                    "identity-mismatch: the receipt's validator printed no \
                                     {} line for tool {name}, so the tree it ran is unattested",
                                    crate::validate::TOOL_IDENTITY_MARKER
                                )),
                                Vec::new(),
                            );
                        }
                        [] => {
                            // #1049: absent is not wrong until the consumer opts
                            // into strictness; record it and accept. `report`
                            // also counts it on the job from the same fact.
                            notes.push(format!(
                                "tool_identity_unattested: tool {name} printed no {} \
                                 line; accepted unattested (require_tool_identity_lines \
                                 is off)",
                                crate::validate::TOOL_IDENTITY_MARKER
                            ));
                        }
                        [value] if *value == stamp => {}
                        [value] if full_hex(value) => {
                            let descends = match &self.tool_repo {
                                Some(repo) => match tool_descends_from(
                                    &job.validator,
                                    name,
                                    stamp,
                                    value,
                                    repo,
                                ) {
                                    Ok(descends) => descends,
                                    Err(error) => return (Some(error), Vec::new()),
                                },
                                None => false,
                            };
                            if descends {
                                notes.push(format!(
                                    "accepted with descendant ran tool {name} ({} -> {})",
                                    crate::plan::short(stamp),
                                    crate::plan::short(value)
                                ));
                            } else {
                                return (
                                    Some(format!(
                                        "identity-mismatch: the receipt's validator ran tool \
                                         {name} at {value:?}, which neither matches the job's \
                                         stamp {stamp:?} nor provably descends from it"
                                    )),
                                    Vec::new(),
                                );
                            }
                        }
                        [value] => {
                            return (
                                Some(format!(
                                    "identity-mismatch: the receipt's validator ran tool \
                                     {name} at {value:?}, not the full hex tree the job \
                                     stamps ({stamp:?})"
                                )),
                                Vec::new(),
                            );
                        }
                        values => {
                            return (
                                Some(format!(
                                    "identity-mismatch: the receipt's validator attested \
                                     tool {name} at {} different trees {values:?}",
                                    values.len()
                                )),
                                Vec::new(),
                            );
                        }
                    }
                } else if !printed.is_empty() && printed != [stamp] {
                    return (
                        Some(format!(
                            "identity-mismatch: the receipt's validator ran tool {name} \
                             at {printed:?}, not the job's stamp {stamp:?}"
                        )),
                        Vec::new(),
                    );
                }
            }
        }
        (None, notes)
    }

    /// Accept `receipt` for job `id` under lease `token`, or refuse it with the reason (which
    /// is also recorded on the job).
    pub fn report(&self, id: &str, token: &str, receipt: Receipt) -> Result<(), String> {
        let _lock = self.lock()?;
        let at = unix_now();
        let mut job = match self.leased(id, token, at) {
            Ok(job) => job,
            Err(reason) => {
                if let Ok(mut job) = self.read(id) {
                    job.history
                        .push(format!("{}: refused a receipt: {reason}", now()));
                    self.write(&job)?;
                }
                return Err(reason);
            }
        };
        let lease = job.lease.clone().unwrap_or_else(|| Lease {
            donor: String::new(),
            host: String::new(),
            token: String::new(),
            claimed_unix: 0,
            expires_unix: 0,
            reports_admission: false,
        });
        let (tool_refusal, tool_notes) = self.tool_check(&job, &receipt);
        let refusal = if receipt.schema != RECEIPT_SCHEMA {
            Some(format!("unknown receipt schema {:?}", receipt.schema))
        } else if receipt.job != job.id {
            Some(format!("the receipt answers job {}", receipt.job))
        } else if receipt.donor.name != lease.donor {
            Some(format!(
                "the receipt is from {}, the lease is {}'s",
                receipt.donor.name, lease.donor
            ))
        } else if receipt.commit != job.commit {
            Some(format!(
                "the receipt validated {}, not the job's commit {}",
                receipt.commit, job.commit
            ))
        } else if receipt.tree != job.tree {
            Some(format!(
                "the receipt validated tree {}, not the job's tree {}",
                receipt.tree, job.tree
            ))
        } else {
            tool_refusal
        };
        if let Some(reason) = refusal {
            job.history
                .push(format!("{}: refused a receipt: {reason}", now()));
            self.write(&job)?;
            return Err(reason);
        }
        if receipt.refused || receipt.exit == Some(crate::validate::REFUSAL_EXIT) {
            // Validation never ran: no verdict in either mode, so the job goes back to the
            // queue. A donor that keeps answering this way is left out of it.
            // The recorded detail names the refusal's cause (#1059): the
            // donor's own pick from its full log when its receipt carries
            // one, else the cause picked from the log tail — never the
            // teardown line a wrapper prints after refusing.
            let cause_field = receipt
                .refusal_cause
                .as_deref()
                .map(str::trim)
                .filter(|cause| !cause.is_empty());
            let cause = cause_field
                .map(crate::validate::cut_line)
                .or_else(|| {
                    crate::validate::refusal_cause(&receipt.log_tail).map(crate::validate::cut_line)
                })
                .unwrap_or("");
            let detail = format!(
                "exit {}{}",
                receipt
                    .exit
                    .map_or_else(|| "none".to_owned(), |code| code.to_string()),
                if cause.is_empty() {
                    String::new()
                } else {
                    format!(", cause: {cause}")
                }
            );
            // A stale-base refusal (the structured job-scope marker; see
            // `validate::STALE_BASE_TOKEN`) says the job itself is stale:
            // like a job `sweep_stale` cancels, it is not a donor failure,
            // so it records no could-not-run strike and no host fault.
            // The job is queued again for the sweep to cancel (or, if the
            // base settles, to run), and the donor is neither probed nor
            // paused for it (donor side, in `Worker::validate`).
            let stale_job = crate::validate::stale_base_refusal(&receipt.log_tail).is_some()
                || cause_field
                    .is_some_and(|field| crate::validate::stale_base_refusal(field).is_some());
            if stale_job {
                job.lease = None;
                job.state = if job.candidate_gone.is_some() {
                    JobState::Cancelled
                } else {
                    JobState::Queued
                };
                job.history.push(format!(
                    "{}: {} on {} refused a stale job ({detail}); no verdict, no strike, {}",
                    now(),
                    receipt.donor.name,
                    receipt.donor.host,
                    if job.state == JobState::Queued {
                        "queued again"
                    } else {
                        "cancelled (the queue no longer wants the candidate)"
                    }
                ));
                self.write(&job)?;
                return Err(format!(
                    "the receipt says the job is stale ({detail}); no verdict, no strike, \
                     the job is queued again for the stale sweep"
                ));
            }
            // A could-not-run refusal for one candidate is not, by itself, a
            // host fault: it depends on the candidate's commits as often as
            // on the host. The job is queued again and counted below; the
            // donor probes its validator (MQ_PROBE=1) right after this
            // refusal, and only a failing probe records a host fault (see
            // `Store::record_host_fault`, called by the donor when its probe
            // fails). A busy admission refusal (validate lock or box busy,
            // slots full, queue position, lease contention) is never a host
            // fault either, and is counted the same way so a persistently
            // refused candidate is still bounded.
            // One narrow exception: a host-scope refusal on the launch right
            // after a probe pass is a flap cycle (the validator claims the
            // host cannot run, yet its probe just passed). That records the
            // fault queue-side so consecutive cycles escalate; it costs the
            // job no strike. A first host-scope refusal is probed first (see
            // above) and faults nothing here.
            // Pacing evidence overrides the host-scope label here too
            // (#1193 census route): a pacing refusal is never a flap fault.
            // It also records no strike (#1241 paced-edges): host-wide
            // contention must not cancel a candidate's job at the bound.
            // The donor's consecutive pacing count, the claim timeout,
            // and the item deferral bound still bound the loop.
            let paced_receipt = crate::validate::gh_paced_refusal(&receipt.log_tail)
                || cause_field.is_some_and(crate::validate::gh_paced_refusal);
            if paced_receipt {
                job.lease = None;
                job.state = if job.candidate_gone.is_some() {
                    JobState::Cancelled
                } else {
                    JobState::Queued
                };
                job.history.push(format!(
                    "{}: {} on {} could not run the validation ({detail}); no verdict, no strike (pacing-guard refusal: transient, counted only donor-side), {}",
                    now(),
                    receipt.donor.name,
                    receipt.donor.host,
                    if job.state == JobState::Queued {
                        "queued again"
                    } else {
                        "cancelled (the queue no longer wants the candidate)"
                    }
                ));
                self.write(&job)?;
                return Err(format!(
                    "the receipt says validation could not run ({detail}); no verdict, \
                     no strike (pacing), the job is queued again"
                ));
            }
            if !paced_receipt {
                if let Some((true, reason)) =
                    crate::validate::could_not_run_scope(&receipt.log_tail)
                {
                    let flap_passed = self
                        .host_flap()
                        .get(&receipt.donor.name)
                        .is_some_and(|entry| entry["probe_passed"].as_bool() == Some(true));
                    if flap_passed {
                        job.lease = None;
                        job.state = if job.candidate_gone.is_some() {
                            JobState::Cancelled
                        } else {
                            JobState::Queued
                        };
                        job.history.push(format!(
                            "{}: host could not run after a passing probe: {reason} ({} on {}); \
                         no verdict, no strike",
                            now(),
                            receipt.donor.name,
                            receipt.donor.host
                        ));
                        self.note_host_fault(&receipt.donor.name, &reason);
                        self.write(&job)?;
                        return Err(format!(
                            "host could not run after a passing probe ({reason}); no verdict, \
                         no strike, the donor is paused"
                        ));
                    }
                }
            }
            let busy = crate::validate::could_not_run_busy(&receipt.log_tail);
            job.could_not_run
                .push(format!("{}: {detail}", receipt.donor.name));
            let by_this_donor = job
                .could_not_run
                .iter()
                .filter(|entry| entry.starts_with(&format!("{}: ", receipt.donor.name)))
                .count();
            // Bounded: past the limit the job is cancelled, and a waiting decide-mode run
            // counts that as a refusal (which its own deferral bound escalates).
            let exhausted = job.could_not_run.len() >= self.could_not_run_limit;
            job.lease = None;
            job.state = if job.candidate_gone.is_some() || exhausted {
                JobState::Cancelled
            } else {
                JobState::Queued
            };
            let mut note = format!(
                "{}: {} on {} could not run the validation ({detail}); no verdict, {}",
                now(),
                receipt.donor.name,
                receipt.donor.host,
                if job.state == JobState::Queued {
                    "queued again".to_owned()
                } else if exhausted {
                    format!(
                        "cancelled: {} could-not-run receipts reach the bound ({})",
                        job.could_not_run.len(),
                        self.could_not_run_limit
                    )
                } else {
                    "cancelled (the queue no longer wants the candidate)".to_owned()
                }
            );
            if by_this_donor >= self.could_not_run_limit && !job.declined_by.contains(&lease.donor)
            {
                job.declined_by.push(lease.donor.clone());
                note.push_str(&format!(
                    "; {} could not run it {by_this_donor} times and will not get it again",
                    lease.donor
                ));
            }
            if busy {
                note.push_str("; busy admission refusal: never a host fault");
            }
            if crate::validate::gh_paced_refusal(&receipt.log_tail)
                || cause_field.is_some_and(crate::validate::gh_paced_refusal)
            {
                note.push_str("; pacing-guard refusal: transient, counted, not a host fault");
            }
            job.history.push(note);
            self.write(&job)?;
            return Err(format!(
                "the receipt says validation could not run ({detail}); no verdict, the job is \
                 queued again"
            ));
        }
        for note in &tool_notes {
            job.history.push(format!("{}: {note}", now()));
        }
        // Count the acceptance's unattested tools on the job itself (#1049),
        // re-derived here so a receipt's own `tool_warnings` cannot talk its
        // way out of the count. Under strictness this list is empty at this
        // point: a missing line already refused above.
        for warning in unattested_warnings(&job.tool.tools, &receipt.ran_tools) {
            job.tool_identity_unattested
                .push(format!("{}: {warning}", receipt.donor.name));
        }
        job.state = JobState::Done;
        job.lease = None;
        job.history.push(format!(
            "{}: receipt from {} on {} accepted{}",
            now(),
            receipt.donor.name,
            receipt.donor.host,
            if job.compare_only {
                " (compare-only: compared, never the verdict)"
            } else if job.candidate_gone.is_some() {
                " after the queue had stopped wanting the candidate (compared only)"
            } else {
                ""
            }
        ));
        if let Some(local) = &job.local {
            job.comparison = Some(compare(local, &receipt.verdict()));
        }
        self.note_receipt(&receipt.donor.name.clone());
        // An admitted validation clears a host fault, but never an
        // operator hold (#1104): only `clear-fault` lifts that.
        if !self.operator_held(&receipt.donor.name) {
            self.clear_host_fault(&receipt.donor.name.clone());
        }
        job.receipt = Some(receipt);
        self.write(&job)
    }

    /// Append a note to a live job's history (its decision record): used
    /// when the queue acts on a job for reasons its own history does not
    /// already tell — a stale-root cancellation naming the attempt
    /// history, say. Finished jobs are left alone.
    pub fn note_history(&self, id: &str, note: &str) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut job = self.read(id)?;
        if matches!(job.state, JobState::Done | JobState::Cancelled) {
            return Ok(());
        }
        job.history.push(format!("{}: {note}", now()));
        self.write(&job)
    }

    /// The queue no longer wants job `id`.
    pub fn cancel(&self, id: &str, reason: &str) -> Result<(), String> {
        self.cancel_live(id, reason).map(|_| ())
    }

    /// Cancel job `id` if it is still queued or leased; returns whether it was. Its donor is
    /// told to stop on its next renewal, and its receipt is refused.
    pub fn cancel_live(&self, id: &str, reason: &str) -> Result<bool, String> {
        let _lock = self.lock()?;
        let mut job = self.read(id)?;
        if matches!(job.state, JobState::Done | JobState::Cancelled) {
            return Ok(false);
        }
        // A job leased to a donor on THIS host may have a local validation
        // unit running for it (the donor's validator started one, e.g. a
        // transient systemd user service): cancelling the job must stop
        // that unit too, or it holds its validate slot with nobody able to
        // stop it once the queue and the donor move on.
        let local_lease = job
            .lease
            .as_ref()
            .filter(|lease| lease.host == host_name())
            .cloned();
        let commit = job.commit.clone();
        // A stale cancellation of a job no donor ever admitted (queued, or
        // leased but not yet at validator start) gets its own marker, so
        // `donor status` can count how often the base moving under a job
        // that never started costs a requeue (#1026).
        let stale_unadmitted = reason.starts_with("stale:") && !job.was_admitted();
        job.state = JobState::Cancelled;
        job.lease = None;
        job.history.push(format!("{}: cancelled: {reason}", now()));
        if stale_unadmitted {
            job.history.push(format!(
                "{}: stale_unadmitted_cancel: cancelled as stale before any donor \
                 admitted it; the validator never started",
                now()
            ));
        }
        self.write(&job)?;
        if local_lease.is_some() {
            let (stopped, left) = stop_local_validation_units(id, &commit);
            if !stopped.is_empty() || !left.is_empty() {
                let mut job = self.read(id)?;
                for unit in &stopped {
                    job.history.push(format!(
                        "{}: stopped local validation unit {unit} for this cancelled job",
                        now()
                    ));
                }
                for unit in &left {
                    job.history.push(format!(
                        "{}: local validation unit {unit} still running after a stop for \
                         this cancelled job",
                        now()
                    ));
                }
                self.write(&job)?;
            }
        }
        Ok(true)
    }

    /// Cancel every live job whose candidate does not contain `base` (the queue's current
    /// base): it can never land as published, so offering it again only burns a donor's
    /// setup time on a stale-base refusal. A stale job is not a donor failure: no
    /// could-not-run strike is recorded. `contains(commit)` answers whether that candidate
    /// contains `base`. With `keep_stale_roots`, a rooted job a donor already holds is
    /// exempt however stale: its run still wants the verdict (green requeues the batch,
    /// red decides it). A queued rooted job is swept like any other: no donor has paid
    /// for it, and dispatched on the old base it is refused by a validator that insists
    /// on a fresh base and could not land as built anyway (#915
    /// merge-queue-keep-running-roots-only). Returns the cancelled ids.
    pub fn sweep_stale(
        &self,
        base: &str,
        contains: &dyn Fn(&str) -> bool,
        keep_stale_roots: bool,
    ) -> Result<Vec<String>, String> {
        let reason = format!(
            "stale: the base moved to {}, which its candidate does not contain",
            crate::plan::short(base)
        );
        let stale: Vec<String> = self
            .jobs()?
            .into_iter()
            .filter(|job| matches!(job.state, JobState::Queued | JobState::Leased))
            .filter(|job| {
                !(keep_stale_roots
                    && job.rooted
                    && job.state == JobState::Leased
                    && job.stale_protected())
            })
            .filter(|job| !contains(&job.commit))
            .map(|job| job.id)
            .collect();
        let mut cancelled = Vec::new();
        for id in stale {
            if self.cancel_live(&id, &reason)? {
                cancelled.push(id);
            }
        }
        Ok(cancelled)
    }

    /// The queue no longer wants job `id`'s candidate, in compare mode: a queued job is
    /// cancelled, a leased one is kept for its donor (see [`Job::candidate_gone`]). Returns
    /// whether the job changed.
    pub fn retire(&self, id: &str, reason: &str) -> Result<bool, String> {
        let _lock = self.lock()?;
        self.expire(unix_now())?;
        let mut job = self.read(id)?;
        match job.state {
            JobState::Queued => {
                job.state = JobState::Cancelled;
                job.history.push(format!("{}: cancelled: {reason}", now()));
            }
            JobState::Leased if job.candidate_gone.is_none() => {
                job.candidate_gone = Some(reason.to_owned());
                job.history.push(format!(
                    "{}: {reason}; kept for its leased donor, whose receipt is still compared",
                    now()
                ));
            }
            _ => return Ok(false),
        }
        self.write(&job)?;
        Ok(true)
    }

    /// [`Store::cancel_all_live`] for compare mode: a leased job kept for the late
    /// comparison of a candidate that landed ([`CANDIDATE_LANDED`]) is left to its donor;
    /// every other queued or leased job is cancelled. Returns the cancelled ids.
    pub fn cancel_all_live_but_landed(&self, reason: &str) -> Result<Vec<String>, String> {
        let live: Vec<String> = self
            .jobs()?
            .into_iter()
            .filter(|job| matches!(job.state, JobState::Queued | JobState::Leased))
            .filter(|job| job.candidate_gone.as_deref() != Some(CANDIDATE_LANDED))
            .map(|job| job.id)
            .collect();
        let mut cancelled = Vec::new();
        for id in live {
            if self.cancel_live(&id, reason)? {
                cancelled.push(id);
            }
        }
        Ok(cancelled)
    }

    /// Cancel every queued or leased job with `reason`; returns their ids. A run calls this
    /// before it publishes anything: runs never overlap, so a live job then is one an earlier
    /// run left behind when it did not finish, and its candidate is gone.
    pub fn cancel_all_live(&self, reason: &str) -> Result<Vec<String>, String> {
        let live: Vec<String> = self
            .jobs()?
            .into_iter()
            .filter(|job| matches!(job.state, JobState::Queued | JobState::Leased))
            .map(|job| job.id)
            .collect();
        let mut cancelled = Vec::new();
        for id in live {
            if self.cancel_live(&id, reason)? {
                cancelled.push(id);
            }
        }
        Ok(cancelled)
    }

    /// One donor's current run of consecutive give-backs with one reason.
    fn give_back_runs(&self) -> BTreeMap<String, (String, usize)> {
        std::fs::read_to_string(self.dir.join("give-backs.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save_give_back_runs(&self, runs: &BTreeMap<String, (String, usize)>) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        let text =
            serde_json::to_string(runs).map_err(|error| format!("cannot encode: {error}"))?;
        write_private(&self.dir.join("give-backs.json"), text.as_bytes())
    }

    /// Record a give-back by `donor` for `reason`: same reason extends its run, a new
    /// reason starts one. Called with the store lock held.
    fn note_give_back(&self, donor: &str, reason: &str) {
        let mut runs = self.give_back_runs();
        let entry = runs
            .entry(donor.to_owned())
            .or_insert_with(|| (reason.to_owned(), 0));
        if entry.0 == reason {
            entry.1 += 1;
        } else {
            *entry = (reason.to_owned(), 1);
        }
        let _ = self.save_give_back_runs(&runs);
    }

    /// Record an accepted receipt from `donor`: it proved it can validate, so its
    /// give-back run resets. Called with the store lock held.
    fn note_receipt(&self, donor: &str) {
        let mut runs = self.give_back_runs();
        if runs.remove(donor).is_some() {
            let _ = self.save_give_back_runs(&runs);
        }
    }

    /// `ERROR:` alerts for donors whose current give-back run reaches `threshold`.
    pub fn give_back_alerts(&self, threshold: usize) -> Result<Vec<String>, String> {
        Ok(self
            .give_back_runs()
            .into_iter()
            .filter(|(_, (_, count))| *count >= threshold.max(1))
            .map(|(donor, (reason, count))| {
                format!(
                    "ERROR: donor {donor} gave back {count} consecutive jobs ({reason}); \
                     check that donor"
                )
            })
            .collect())
    }

    /// One donor's host-fault record (`host-faults.json`): it cannot validate at all,
    /// so its refusals cost jobs no strikes and it is leased nothing until it reports
    /// an admitted validation or claims after a passing probe.
    fn host_faults(&self) -> std::collections::BTreeMap<String, serde_json::Value> {
        std::fs::read_to_string(self.dir.join("host-faults.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save_host_faults(
        &self,
        faults: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        let text =
            serde_json::to_string(faults).map_err(|error| format!("cannot encode: {error}"))?;
        write_private(&self.dir.join("host-faults.json"), text.as_bytes())
    }

    /// Each donor's job cap as its last claim gave it, and when (#918 donor-cap-file): one
    /// map in one fixed file, `donor-caps.json`, read by `donor status`.
    fn donor_caps(&self) -> std::collections::BTreeMap<String, serde_json::Value> {
        std::fs::read_to_string(self.dir.join(DONOR_CAPS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Note that `donor` claimed with cap `jobs` (0: draining) at `at`. Written when the cap
    /// changed, or when the last note is older than [`DONOR_CAP_REFRESH_SECS`], so a donor
    /// claiming every few seconds does not rewrite the file every time.
    pub fn note_donor_cap(&self, donor: &str, jobs: usize, at: u64) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut caps = self.donor_caps();
        let unchanged = caps.get(donor).is_some_and(|seen| {
            seen["jobs"].as_u64() == Some(jobs as u64)
                && at.saturating_sub(seen["seen_unix"].as_u64().unwrap_or(0))
                    < DONOR_CAP_REFRESH_SECS
        });
        if unchanged {
            return Ok(());
        }
        caps.insert(
            donor.to_owned(),
            serde_json::json!({"jobs": jobs, "seen_unix": at}),
        );
        let text =
            serde_json::to_string(&caps).map_err(|error| format!("cannot encode: {error}"))?;
        write_private(&self.dir.join(DONOR_CAPS_FILE), text.as_bytes())
    }

    /// Every donor's last claimed cap and when it was seen (Unix seconds), by name.
    pub fn donor_cap_list(&self) -> Vec<(String, usize, u64)> {
        self.donor_caps()
            .into_iter()
            .map(|(donor, seen)| {
                (
                    donor,
                    usize::try_from(seen["jobs"].as_u64().unwrap_or(0)).unwrap_or(0),
                    seen["seen_unix"].as_u64().unwrap_or(0),
                )
            })
            .collect()
    }

    /// Host faults by donor name, for status and the serve heartbeat.
    pub fn host_fault_list(&self) -> Vec<(String, String, String)> {
        self.host_fault_details()
            .into_iter()
            .map(|(donor, reason, since, _, _, _)| (donor, reason, since))
            .collect()
    }

    /// Host faults with their last probe, by donor name:
    /// `(donor, reason, since, last_probe_at, last_probe_result, operator)`,
    /// where the result is `passed`, `failed`, or empty when never probed,
    /// and `operator` marks a hold only `clear-fault` lifts (#1104).
    /// Status and the serve heartbeat render this as "donor X faulted
    /// since T: reason; last probe T result", so a fault sitting at zero
    /// probes is visible.
    pub fn host_fault_details(&self) -> Vec<(String, String, String, String, String, bool)> {
        self.host_faults()
            .into_iter()
            .map(|(donor, fault)| {
                (
                    donor,
                    fault["reason"].as_str().unwrap_or_default().to_owned(),
                    fault["since"].as_str().unwrap_or_default().to_owned(),
                    fault["last_probe_at"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    fault["last_probe"].as_str().unwrap_or_default().to_owned(),
                    fault["operator"].as_bool() == Some(true),
                )
            })
            .collect()
    }

    /// Whether `donor`'s recorded fault is an operator hold (#1104).
    fn operator_held(&self, donor: &str) -> bool {
        self.host_faults()
            .get(donor)
            .is_some_and(|fault| fault["operator"].as_bool() == Some(true))
    }

    /// Record (or refresh) an operator hold on `donor` (#1104): a host
    /// fault no probe pass or admitted validation clears, only an
    /// explicit `clear-fault`. Not a probe failure: it counts no probe
    /// and no flap cycle. An existing entry keeps its `since` and probe
    /// history; any armed trial is dropped.
    pub fn record_operator_fault(&self, donor: &str, reason: &str) {
        let mut faults = self.host_faults();
        let entry = faults.entry(donor.to_owned()).or_insert_with(|| {
            serde_json::json!({"reason": reason, "since": now(), "probes": 0, "last_probe": "", "last_probe_at": ""})
        });
        entry["reason"] = serde_json::json!(reason);
        entry["operator"] = serde_json::json!(true);
        entry.as_object_mut().map(|object| object.remove("trial"));
        let _ = self.save_host_faults(&faults);
    }

    /// Record a host fault for `donor` because its probe (MQ_PROBE=1), run
    /// right after a could-not-run refusal, also failed. This is the only
    /// refusal-driven way a fault is recorded: a refusal alone never faults
    /// a host (see `Store::report`). Counts as one failed probe.
    pub fn record_host_fault(&self, donor: &str, reason: &str) {
        self.note_host_fault(donor, reason);
        self.record_probe_result(donor, false);
    }

    /// Note the outcome of a fault re-probe: a pass clears an ordinary fault
    /// (an escalated one only arms a trial claim, as `note_probe_pass`), a
    /// failure keeps it and counts one more probe cycle, so a fault never
    /// sits at cycles=0 forever.
    pub fn record_probe_result(&self, donor: &str, passed: bool) {
        let mut faults = self.host_faults();
        if let Some(entry) = faults.get_mut(donor) {
            entry["last_probe_at"] = serde_json::json!(now());
            entry["last_probe"] = serde_json::json!(if passed { "passed" } else { "failed" });
            entry["probes"] = serde_json::json!(entry["probes"].as_u64().unwrap_or_default() + 1);
            let _ = self.save_host_faults(&faults);
        }
        if passed {
            self.note_probe_pass(donor);
        }
    }

    /// Escalated host faults as `ERROR:` lines, for donor status and the serve
    /// heartbeat: a donor that keeps passing its probe and then refusing at
    /// launch, paused until an admitted validation clears it.
    pub fn host_fault_escalations(&self) -> Vec<String> {
        self.host_faults()
            .into_iter()
            .filter(|(_, fault)| fault["escalated"].as_bool() == Some(true))
            .map(|(donor, fault)| {
                format!(
                    "ERROR: donor {donor}: host fault escalated after {} probe-pass/launch-refuse \
                     cycle(s) ({}); it stays paused until an admitted validation, a probe pass \
                     no longer clears it",
                    fault["cycles"].as_u64().unwrap_or_default(),
                    fault["reason"].as_str().unwrap_or_default()
                )
            })
            .collect()
    }

    /// Whether every donor eligible for `job` is host-faulted, so a decide-mode
    /// run should keep waiting rather than spend its claim timeout. Eligible
    /// donors are the donors the store knows (from leases, receipts, give-backs
    /// and faults) that may claim this job: not compare-only for a deciding job
    /// (only compare-only for a duplicate), and not a donor this job declined.
    /// With no known eligible donor, or any known eligible donor healthy, this
    /// is false and the normal claim timeout applies.
    pub fn every_eligible_donor_faulted(&self, job: &Job) -> bool {
        let faults = self.host_faults();
        if faults.is_empty() {
            return false;
        }
        let mut known: std::collections::BTreeSet<String> = faults.keys().cloned().collect();
        known.extend(self.give_back_runs().into_keys());
        if let Ok(jobs) = self.jobs() {
            for other in &jobs {
                if let Some(lease) = &other.lease {
                    known.insert(lease.donor.clone());
                }
                if let Some(receipt) = &other.receipt {
                    known.insert(receipt.donor.name.clone());
                }
                known.extend(other.declined_by.iter().cloned());
                for entry in &other.could_not_run {
                    if let Some((donor, _)) = entry.split_once(": ") {
                        known.insert(donor.to_owned());
                    }
                }
            }
        }
        let eligible: Vec<&String> = known
            .iter()
            .filter(|name| {
                (if job.compare_only {
                    self.compare_only.contains(*name)
                } else {
                    !self.compare_only.contains(*name)
                }) && !job.declined_by.iter().any(|declined| declined == *name)
            })
            .collect();
        !eligible.is_empty()
            && eligible
                .iter()
                .all(|name| faults.contains_key(name.as_str()))
    }

    /// Probe-pass/launch-refuse flap counts by donor (`host-flap.json`): how
    /// many consecutive cycles of (probe passes, claim, host-scope refusal at
    /// launch) a donor has completed, and whether the latest probe pass is
    /// still awaiting its launch verdict.
    fn host_flap(&self) -> std::collections::BTreeMap<String, serde_json::Value> {
        std::fs::read_to_string(self.dir.join("host-flap.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save_host_flap(
        &self,
        flap: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))?;
        let text =
            serde_json::to_string(flap).map_err(|error| format!("cannot encode: {error}"))?;
        write_private(&self.dir.join("host-flap.json"), text.as_bytes())
    }

    /// Record that `donor` claims after a passing probe. An ordinary fault is
    /// cleared, as before, and the flap count remembers that this launch is
    /// the verdict on that probe: a host-scope refusal now completes one
    /// probe-pass/launch-refuse cycle instead of resetting the cycle count.
    /// An escalated fault is not cleared: it stays recorded (and shown), and
    /// one trial claim is armed so the donor can produce the admitted
    /// validation that alone clears it. An operator hold (#1104) is left
    /// entirely alone: no clear, no trial, no flap mark.
    pub fn note_probe_pass(&self, donor: &str) {
        if self.operator_held(donor) {
            return;
        }
        let mut flap = self.host_flap();
        flap.entry(donor.to_owned())
            .or_insert_with(|| serde_json::json!({"cycles": 0, "probe_passed": false}))
            ["probe_passed"] = serde_json::json!(true);
        let _ = self.save_host_flap(&flap);
        let mut faults = self.host_faults();
        match faults.get_mut(donor) {
            Some(entry) if entry["escalated"].as_bool() == Some(true) => {
                entry["trial"] = serde_json::json!(true);
                let _ = self.save_host_faults(&faults);
            }
            Some(_) => {
                faults.remove(donor);
                let _ = self.save_host_faults(&faults);
            }
            None => {}
        }
    }

    /// Record (or refresh) a donor's host fault. Called with the store lock held.
    /// A refusal at launch right after a probe pass completes a flap cycle;
    /// after `host_fault_escalate_after` consecutive cycles the fault
    /// escalates (see [`Store::note_probe_pass`]).
    fn note_host_fault(&self, donor: &str, reason: &str) {
        let mut flap = self.host_flap();
        let cycles = {
            let entry = flap
                .entry(donor.to_owned())
                .or_insert_with(|| serde_json::json!({"cycles": 0, "probe_passed": false}));
            if entry["probe_passed"].as_bool() == Some(true) {
                entry["cycles"] =
                    serde_json::json!(entry["cycles"].as_u64().unwrap_or_default() + 1);
            }
            entry["probe_passed"] = serde_json::json!(false);
            entry["cycles"].as_u64().unwrap_or_default()
        };
        let _ = self.save_host_flap(&flap);
        let mut faults = self.host_faults();
        let entry = faults.entry(donor.to_owned()).or_insert_with(|| {
            serde_json::json!({"reason": reason, "since": now(), "probes": 0, "last_probe": "", "last_probe_at": ""})
        });
        entry["reason"] = serde_json::json!(reason);
        entry["cycles"] = serde_json::json!(cycles);
        entry["escalated"] =
            serde_json::json!(cycles >= self.host_fault_escalate_after.max(1) as u64);
        entry.as_object_mut().map(|object| object.remove("trial"));
        let _ = self.save_host_faults(&faults);
    }

    /// Clear a donor's host fault (an admitted validation). Its flap cycle
    /// count goes with it: the next fault starts a fresh count. A claim after
    /// a passing probe clears through [`Store::note_probe_pass`] instead, so
    /// the cycle can be counted when its launch verdict arrives. Called with
    /// the store lock held where one is held.
    pub fn clear_host_fault(&self, donor: &str) {
        let mut faults = self.host_faults();
        if faults.remove(donor).is_some() {
            let _ = self.save_host_faults(&faults);
        }
        let mut flap = self.host_flap();
        if flap.remove(donor).is_some() {
            let _ = self.save_host_flap(&flap);
        }
    }

    /// The job's current record.
    pub fn get(&self, id: &str) -> Result<Job, String> {
        let _lock = self.lock()?;
        self.expire(unix_now())?;
        self.read(id)
    }

    /// Live leases per donor.
    pub fn leases_by_donor(&self) -> Result<BTreeMap<String, usize>, String> {
        let mut map = BTreeMap::new();
        for job in self.jobs()? {
            if let (JobState::Leased, Some(lease)) = (job.state, &job.lease) {
                *map.entry(lease.donor.clone()).or_default() += 1;
            }
        }
        Ok(map)
    }
}

/// Write a git bundle of `job.commit`, excluding `job.base`, to `out` (queue side). Refuses a
/// bundle over `max_bytes`: the donor then declines the job, leaving it to a donor whose clone
/// already has the commits.
pub fn write_bundle(
    repo: &Path,
    scratch: &Path,
    job: &Job,
    max_bytes: u64,
    out: &mut dyn std::io::Write,
) -> Result<u64, String> {
    let git = |args: &[&str]| -> Result<std::process::Output, String> {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("cannot run git {}: {error}", args.join(" ")))
    };
    let reference = format!("refs/merge-queue/donor/{}", job.id);
    let set = git(&["update-ref", &reference, &job.commit])?;
    if !set.status.success() {
        return Err(format!(
            "cannot name {} for the bundle: {}",
            job.commit,
            String::from_utf8_lossy(&set.stderr).trim()
        ));
    }
    std::fs::create_dir_all(scratch)
        .map_err(|error| format!("cannot create {}: {error}", scratch.display()))?;
    let file = scratch.join(format!("{}.bundle", job.id));
    let file_text = file.display().to_string();
    let exclude = format!("^{}", job.base);
    let created = git(&[
        "bundle", "create", "--quiet", &file_text, &reference, &exclude,
    ]);
    let _ = git(&["update-ref", "-d", &reference]);
    let created = created?;
    let result = (|| {
        if !created.status.success() {
            return Err(format!(
                "cannot bundle {} over {}: {}",
                job.commit,
                job.base,
                String::from_utf8_lossy(&created.stderr).trim()
            ));
        }
        let size = std::fs::metadata(&file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?
            .len();
        if size > max_bytes {
            return Err(format!(
                "the bundle for {} is {size} bytes, over donors.max_bundle_bytes ({max_bytes}); \
                 a donor whose clone already has the commits can still take the job",
                job.commit
            ));
        }
        let mut reader = std::fs::File::open(&file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        std::io::copy(&mut reader, out).map_err(|error| format!("cannot send the bundle: {error}"))
    })();
    let _ = std::fs::remove_file(&file);
    result
}

/// What a donor sends with a report.
#[derive(Serialize, Deserialize)]
pub struct Report {
    /// Lease token.
    pub token: String,
    /// The receipt.
    pub receipt: Receipt,
}

/// How a donor reaches the queue: an argv prefix that runs the queue's command-line tool on
/// the queue host (node-exec, ssh, or a local path), to which each request's arguments are
/// appended. The lease token travels on stdin, never in arguments.
pub struct Route {
    argv: Vec<String>,
}

impl Route {
    /// A route from its argv prefix.
    pub fn new(argv: Vec<String>) -> Result<Self, String> {
        if argv.is_empty() {
            return Err("--queue needs a command".to_owned());
        }
        Ok(Self { argv })
    }

    /// Parse a queue reply that should be one JSON value: tolerate leading noise lines
    /// (a route that merges the remote's stderr into stdout, an agentcloudctl note, a
    /// config warning) by taking the last line that parses. Skipped noise is logged once.
    fn parse_reply<T: serde::de::DeserializeOwned>(out: &[u8]) -> Result<T, String> {
        if let Ok(value) = serde_json::from_slice::<T>(out) {
            return Ok(value);
        }
        let text = String::from_utf8_lossy(out);
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        for (index, line) in lines.iter().enumerate().rev() {
            if let Ok(value) = serde_json::from_str::<T>(line) {
                let skipped = index;
                if skipped > 0 {
                    static LOGGED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !LOGGED.swap(true, Ordering::SeqCst) {
                        eprintln!(
                            "merge-queue donor: skipped {skipped} noise line(s) before the \
                         queue's JSON reply (further noise is not logged)"
                        );
                    }
                }
                return Ok(value);
            }
        }
        Err(format!(
            "no JSON reply in the queue's answer: {}",
            lines.last().copied().unwrap_or_default()
        ))
    }
}

impl Route {
    /// Run one request; stdout on success, the exit status and stderr tail otherwise.
    fn call(
        &self,
        args: &[&str],
        stdin: &[u8],
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>, String> {
        use std::io::Write;
        let mut child = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run the queue route {}: {error}", self.argv[0]))?;
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(stdin);
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stdout) = stdout {
                let _ = stdout.read_to_end(&mut buffer);
            }
            buffer
        });
        let errors = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_end(&mut buffer);
            }
            buffer
        });
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("cannot wait for the queue route: {error}"))?
            {
                break status;
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "queue route timed out after {}s ({})",
                    timeout.as_secs(),
                    args.first().copied().unwrap_or("")
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let out = reader.join().unwrap_or_default();
        let err = errors.join().unwrap_or_default();
        if status.success() {
            Ok(out)
        } else {
            // Some routes (agentcloudctl node exec) deliver the remote's stderr on
            // their local stdout, beneath a local banner line: show both tails, and
            // drop banner lines, so the remote's own error is what the operator sees.
            let tail = |bytes: &[u8]| -> String {
                let text = String::from_utf8_lossy(bytes);
                let lines: Vec<&str> = text
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("note:"))
                    .collect();
                lines[lines.len().saturating_sub(3)..].join(" / ")
            };
            let stderr_tail = tail(&err);
            let stdout_tail = tail(&out);
            Err(format!(
                "queue route {} exited {}: {}{}",
                args.first().copied().unwrap_or(""),
                status.code().unwrap_or(-1),
                stderr_tail,
                if stdout_tail.is_empty() {
                    String::new()
                } else {
                    format!("; stdout: {stdout_tail}")
                }
            ))
        }
    }
}

/// Settings for `merge-queue donor run`.
pub struct DonorOptions {
    /// The queue route's argv prefix.
    pub queue: Vec<String>,
    /// This host's clone of the queue's repository.
    pub clone: PathBuf,
    /// Remote to fetch the base from (empty: never fetch).
    pub remote: String,
    /// Donor name (one per daemon).
    pub name: String,
    /// Most jobs at once.
    pub jobs: usize,
    /// Idle poll interval.
    pub poll: std::time::Duration,
    /// Work directory (checkouts, logs).
    pub work: PathBuf,
    /// Seconds to stop claiming after a job whose tools or revision this donor did not match.
    pub mismatch_backoff: std::time::Duration,
    /// Exit once no job is running and none is available (scripts and tests).
    pub until_idle: bool,
    /// A file holding the most jobs to run at once, read before each claim; lower than `jobs`
    /// it caps them, 0 drains (#918 donor-cap-file). Running jobs are never touched.
    pub cap_file: Option<PathBuf>,
}

/// Whether a local validation unit name belongs to `job_id`/`commit`: the
/// unit name carries the job id, one of its dash-separated segments of at
/// least 8 characters (the hash segment a unit is named after), or the
/// commit's first 12 characters. Pure, so it can be tested without systemd.
pub fn unit_matches_job(unit: &str, job_id: &str, commit: &str) -> bool {
    if unit.contains(job_id) {
        return true;
    }
    if commit.len() >= 12 && unit.contains(&commit[..12]) {
        return true;
    }
    job_id
        .split('-')
        .filter(|segment| segment.len() >= 8)
        .any(|segment| unit.contains(segment))
}

/// Parse `systemctl --user list-units --plain --no-legend` output into
/// `(unit, active)` pairs for validation units only.
fn parse_validation_units(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let unit = fields.next()?;
            if !unit.starts_with("validate-merge-queue-") || !unit.ends_with(".service") {
                return None;
            }
            let _load = fields.next()?;
            let active = fields.next()? == "active";
            Some((unit.to_owned(), active))
        })
        .collect()
}

/// The local validation units systemd knows about, or none where systemd
/// (or a user bus) is unavailable: absence is not an error, just nothing
/// this host can stop.
fn local_validation_units() -> Vec<(String, bool)> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "validate-merge-queue-*",
            "--all",
            "--no-legend",
            "--plain",
        ])
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            parse_validation_units(&String::from_utf8_lossy(&out.stdout))
        }
        _ => Vec::new(),
    }
}

/// Stop every active local validation unit belonging to a job, by unit
/// name (never by pattern kill): returns `(stopped, still_active)`. A unit
/// that ignores the stop is reported, not hidden. Where no unit matches,
/// both lists are empty.
pub fn stop_local_validation_units(job_id: &str, commit: &str) -> (Vec<String>, Vec<String>) {
    let targets: Vec<String> = local_validation_units()
        .into_iter()
        .filter(|(unit, active)| *active && unit_matches_job(unit, job_id, commit))
        .map(|(unit, _)| unit)
        .collect();
    if targets.is_empty() {
        return (Vec::new(), Vec::new());
    }
    for unit in &targets {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", "--no-block", unit])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    // A stop is asynchronous: give the units a moment, then say honestly
    // which are still active.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let still: Vec<String> = local_validation_units()
        .into_iter()
        .filter(|(unit, active)| *active && targets.contains(unit))
        .map(|(unit, _)| unit)
        .collect();
    let stopped: Vec<String> = targets
        .iter()
        .filter(|unit| !still.contains(unit))
        .cloned()
        .collect();
    (stopped, still)
}

/// Local validation units still running for cancelled jobs, as
/// `(unit, job_id)`: what a serve start must stop before admitting new
/// work, and what a serve exit must name if it leaves any behind.
pub fn orphaned_local_units(store: &Store) -> Vec<(String, String)> {
    let Ok(jobs) = store.jobs() else {
        return Vec::new();
    };
    let cancelled: Vec<&Job> = jobs
        .iter()
        .filter(|job| job.state == JobState::Cancelled)
        .collect();
    if cancelled.is_empty() {
        return Vec::new();
    }
    local_validation_units()
        .into_iter()
        .filter(|(_, active)| *active)
        .filter_map(|(unit, _)| {
            cancelled
                .iter()
                .find(|job| unit_matches_job(&unit, &job.id, &job.commit))
                .map(|job| (unit, job.id.clone()))
        })
        .collect()
}

fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

const ROUTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const BUNDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

fn git_in(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn has_commit(dir: &Path, commit: &str) -> bool {
    git_in(dir, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok()
}

/// Why a donor did not report a job.
enum Unreported {
    /// The donor cannot run it here; it declined (with the reason) and does not get it again
    /// unless the job is re-stamped.
    Declined(String),
    /// The lease is gone or the job was cancelled; nothing to report.
    Lost(String),
    /// The job was given back at once: the donor is stopping, or it cannot tell yet whether it
    /// can run the job (its merge-queue revision or its tools differ and cannot be matched
    /// here), so it is offered the job again later.
    Released(String),
}

struct Worker<'a> {
    options: &'a DonorOptions,
    route: &'a Route,
    host: String,
    slots: Mutex<Vec<usize>>,
    /// Host-scope refusal state: while set, claiming is paused and the validator is
    /// probed (MQ_PROBE=1) with a 30s -> 10min back-off until it admits work again.
    host_pause: Mutex<Option<HostPause>>,
    /// Set when a probe just passed: the next claim carries --probe-passed.
    probe_passed: AtomicBool,
    /// Delay for the next host pause. A probe pass followed by another launch
    /// refusal keeps doubling it (30 s to 10 min) instead of resetting to
    /// 30 s; only an admitted validation resets it.
    next_host_delay: Mutex<std::time::Duration>,
    /// The last validator this donor ran, kept so a queue-side host fault
    /// with no local host-pause file can still be re-probed on schedule.
    last_validator: Mutex<Option<crate::state::ValidatorConfig>>,
    /// When the next no-pause fault re-probe is due, and its current delay.
    next_fault_probe: Mutex<std::time::Instant>,
    fault_probe_current: Mutex<std::time::Duration>,
    /// Consecutive pacing-guard refusals (#1193 ghpaced-transient): below
    /// [`GH_PACED_FAULT_AFTER`] they are transient (no probe, no fault);
    /// at the bound the ordinary probe-and-fault path runs, so a lasting
    /// outage still faults. An admitted validation resets the count.
    paced_refusals: Mutex<usize>,
}

/// Consecutive pacing-guard refusals after which the donor stops treating
/// them as transient and lets the probe decide a host fault (#1193).
pub const GH_PACED_FAULT_AFTER: usize = 3;

/// Probe output line naming a validator entry lock wait (item 5 of #1193).
pub const ENTRY_LOCK_WAIT_MARKER: &str = "entry lock wait";

/// Total probe attempts when each fails only on the entry lock wait.
pub const PROBE_LOCK_WAIT_ATTEMPTS: usize = 3;

/// Delay between in-probe lock-wait retries.
pub const PROBE_LOCK_WAIT_RETRY_SECS: u64 = 5;

/// Delay before an in-probe pacing retry: one pacer slot (1 per 10 s per
/// host) plus a margin (#1193 census route).
pub const PROBE_PACED_RETRY_SECS: u64 = 11;

/// The probe back-off after a pause whose current delay is `delay`: doubled,
/// capped at ten minutes.
fn doubled_probe_delay(delay: std::time::Duration) -> std::time::Duration {
    (delay * 2).min(std::time::Duration::from_secs(600))
}

/// First fault re-probe delay, and the cap it backs off to: a faulted donor
/// re-probes after 60 s, doubling to at most 5 min, whether or not a
/// host-pause file exists.
pub const FAULT_PROBE_FIRST_SECS: u64 = 60;
/// Cap for the fault re-probe back-off.
pub const FAULT_PROBE_MAX_SECS: u64 = 300;

fn fault_probe_delay(delay: std::time::Duration) -> std::time::Duration {
    (delay * 2).min(std::time::Duration::from_secs(FAULT_PROBE_MAX_SECS))
}

/// A donor paused after a host-scope could-not-run refusal.
struct HostPause {
    /// The refusal's reason.
    reason: String,
    /// When the pause began (RFC 3339).
    since: String,
    /// The validator to probe.
    validator: crate::state::ValidatorConfig,
    /// Next probe time.
    next: std::time::Instant,
    /// Current probe back-off.
    delay: std::time::Duration,
}

/// Persist a host pause so a restarted donor can resume probing: the
/// validator snapshot (`kind` + `run` are what `probe_host_validator`
/// needs) and the current back-off, beside the human-readable reason.
fn write_host_pause(work: &Path, pause: &HostPause) {
    let _ = std::fs::create_dir_all(work);
    let _ = std::fs::write(
        work.join("host-pause.json"),
        serde_json::json!({
            "reason": pause.reason,
            "since": pause.since,
            "validator": pause.validator,
            "delay_secs": pause.delay.as_secs(),
        })
        .to_string(),
    );
}

/// Restore a host pause written by [`write_host_pause`], due to probe at
/// once. `Ok(None)` means no pause file; `Err` names why a present file
/// cannot resume probing (malformed, or a legacy `{reason, since}` file
/// with no validator snapshot), so the caller can log it loudly instead of
/// silently deadlocking against the queue-side host fault.
fn restored_host_pause(work: &Path) -> Result<Option<HostPause>, String> {
    let path = work.join("host-pause.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is malformed: {error}", path.display()))?;
    let reason = value["reason"].as_str().unwrap_or_default().to_owned();
    let since = value["since"].as_str().unwrap_or_default().to_owned();
    let Some(validator_value) = value.get("validator").filter(|v| !v.is_null()) else {
        return Err(format!(
            "{} has no validator snapshot (it was written before pauses persisted one); \
             cannot resume probing from it",
            path.display()
        ));
    };
    let validator: ValidatorConfig =
        serde_json::from_value(validator_value.clone()).map_err(|error| {
            format!(
                "{} has a malformed validator snapshot: {error}",
                path.display()
            )
        })?;
    if validator.kind.trim().is_empty() {
        return Err(format!(
            "{} has a validator snapshot with no kind; cannot resume probing from it",
            path.display()
        ));
    }
    let delay_secs = value["delay_secs"]
        .as_u64()
        .filter(|secs| (1..=600).contains(secs))
        .unwrap_or(30);
    Ok(Some(HostPause {
        reason,
        since,
        validator,
        next: std::time::Instant::now(),
        delay: std::time::Duration::from_secs(delay_secs),
    }))
}

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

impl Worker<'_> {
    /// Probe the validator after a host-scope refusal: `MQ_PROBE=1`, admission checks
    /// only; exit 0 means ready. A non-`command` validator is probed by claiming again
    /// after the back-off (the queue re-faults it if it still refuses host-wide).
    fn probe_host_validator(&self, cfg: &crate::state::ValidatorConfig) -> bool {
        if cfg.kind != "command" || cfg.run.trim().is_empty() {
            return true;
        }
        // A probe that fails only on the validator's entry lock wait
        // (#1193 item 5) retries inside this probe, counted: the lock is
        // held by unrelated work (a cleanup drain), so waiting out the
        // fault back-off only re-records the fault. A probe that fails on
        // pacing-guard output likewise retries after the pacer slot
        // (#1193 census route): contention, not a host defect. Past the
        // bound the probe fails as any probe does.
        for attempt in 1..=PROBE_LOCK_WAIT_ATTEMPTS {
            let (passed, output) = self.probe_once(cfg);
            if passed {
                return true;
            }
            let lock_wait = output.contains(ENTRY_LOCK_WAIT_MARKER);
            let paced =
                crate::validate::gh_paced_refusal(&output) || output.contains("GH-PACED WARNING");
            if attempt == PROBE_LOCK_WAIT_ATTEMPTS || !(lock_wait || paced) {
                return false;
            }
            let (cause, delay) = if paced {
                ("pacing-guard refusal", PROBE_PACED_RETRY_SECS)
            } else {
                ("entry lock wait", PROBE_LOCK_WAIT_RETRY_SECS)
            };
            eprintln!(
                "merge-queue donor {}: probe attempt {attempt} failed on the {cause}; retrying inside the probe (bound {PROBE_LOCK_WAIT_ATTEMPTS})",
                self.options.name
            );
            std::thread::sleep(std::time::Duration::from_secs(delay));
        }
        false
    }

    /// One probe run: whether it passed, and its combined output (for the
    /// lock-wait classification; the probe's results file is separate).
    fn probe_once(&self, cfg: &crate::state::ValidatorConfig) -> (bool, String) {
        let results = self.options.work.join("probe-results.jsonl");
        let output_path = self
            .options
            .work
            .join(format!("probe-output-{}.log", std::process::id()));
        let Ok(output_file) = std::fs::File::create(&output_path) else {
            return (false, String::new());
        };
        let Ok(stderr_file) = output_file.try_clone() else {
            return (false, String::new());
        };
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(&cfg.run)
            .current_dir(&self.options.clone)
            .env(crate::validate::PROBE_ENV, "1")
            .env("MQ_RESULTS", &results)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(output_file))
            .stderr(std::process::Stdio::from(stderr_file));
        let Ok(mut child) = command.spawn() else {
            return (false, String::new());
        };
        let bound = cfg.probe_timeout();
        let started = std::time::Instant::now();
        let mut killed = false;
        let passed = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if started.elapsed() <= bound => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    killed = true;
                    break false;
                }
            }
        };
        let mut output = std::fs::read_to_string(&output_path).unwrap_or_default();
        if killed {
            // Name the timeout so a killed probe is distinguishable from
            // a not-ready verdict (#1271 probe-timeout-bound).
            output.push_str(&format!(
                "\nprobe killed at the {} s bound",
                bound.as_secs()
            ));
        }
        let _ = std::fs::remove_file(&output_path);
        (passed, output)
    }

    /// The job's configured pre-claim check (`validator.pre_claim_check`):
    /// run it in the clone with the candidate, repository and base in the
    /// environment, and say why it failed when it does. The command
    /// resolves whatever the candidate needs on this host itself; the
    /// queue has no notion of what is being checked.
    fn pre_claim_check(&self, job: &Job) -> Result<(), String> {
        let check = job.validator.pre_claim_check.trim();
        if check.is_empty() {
            return Ok(());
        }
        let repo = std::process::Command::new("git")
            .args([
                "-C",
                &self.options.clone.display().to_string(),
                "remote",
                "get-url",
                "origin",
            ])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| self.options.clone.display().to_string());
        let stderr_path = self
            .options
            .work
            .join(format!("pre-claim-{}.log", job.id.replace('/', "-")));
        if let Some(parent) = stderr_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let stderr_file = std::fs::File::create(&stderr_path)
            .map_err(|error| format!("pre_claim_check could not start: {error}"))?;
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(check)
            .current_dir(&self.options.clone)
            .env("MQ_CANDIDATE", &job.commit)
            .env("MQ_REPO", &repo)
            .env("MQ_BASE", &job.base)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(stderr_file);
        let mut child = command
            .spawn()
            .map_err(|error| format!("pre_claim_check could not start: {error}"))?;
        let timeout = job.validator.pre_claim_timeout();
        let started = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() <= timeout => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("pre_claim_check timed out".to_owned());
                }
            }
        };
        if status.success() {
            return Ok(());
        }
        let text = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        let reason = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(crate::validate::cut_line)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "pre_claim_check exited {}",
                    status
                        .code()
                        .map_or_else(|| "on a signal".to_owned(), |code| code.to_string())
                )
            });
        Err(reason)
    }

    /// Stop claiming for the mismatch back-off.
    fn back_off(&self, backoff: &Mutex<Option<std::time::Instant>>) {
        *backoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(std::time::Instant::now() + self.options.mismatch_backoff);
    }

    fn release(&self, job: &Job, token: &str, reason: &str, decline: bool) {
        let mut args = vec!["donor", "release", "--job", &job.id, "--reason", reason];
        if decline {
            args.push("--decline");
        }
        if let Err(error) = self.route.call(&args, token.as_bytes(), ROUTE_TIMEOUT) {
            eprintln!(
                "merge-queue donor {}: cannot give back {}: {error} (it returns to the queue when its lease expires)",
                self.options.name, job.id
            );
        }
    }

    /// Make the job's exact commit present in the clone.
    fn fetch(&self, job: &Job, token: &str) -> Result<(), Unreported> {
        let clone = &self.options.clone;
        if !has_commit(clone, &job.commit) && !self.options.remote.is_empty() {
            let _ = git_in(clone, &["fetch", "--quiet", &self.options.remote]);
        }
        if !has_commit(clone, &job.commit) {
            if !has_commit(clone, &job.base) {
                return Err(Unreported::Declined(format!(
                    "the base {} is not in this donor's clone even after fetching {:?}",
                    job.base, self.options.remote
                )));
            }
            let bundle = self
                .route
                .call(
                    &["donor", "bundle", "--job", &job.id],
                    token.as_bytes(),
                    BUNDLE_TIMEOUT,
                )
                .map_err(Unreported::Declined)?;
            let dir = self.options.work.join("bundles");
            std::fs::create_dir_all(&dir)
                .map_err(|error| Unreported::Declined(error.to_string()))?;
            let file = dir.join(format!("{}.bundle", job.id));
            std::fs::write(&file, &bundle)
                .map_err(|error| Unreported::Declined(error.to_string()))?;
            let reference = format!("refs/merge-queue/donor/{}", job.id);
            let fetched = git_in(
                clone,
                &[
                    "fetch",
                    "--quiet",
                    &file.display().to_string(),
                    &format!("+{reference}:{reference}"),
                ],
            );
            let _ = std::fs::remove_file(&file);
            let _ = git_in(clone, &["update-ref", "-d", &reference]);
            fetched.map_err(Unreported::Declined)?;
        }
        let tree = git_in(clone, &["rev-parse", &format!("{}^{{tree}}", job.commit)])
            .map_err(Unreported::Declined)?;
        if tree != job.tree {
            return Err(Unreported::Declined(format!(
                "commit {} has tree {tree} here, not the job's {}",
                job.commit, job.tree
            )));
        }
        Ok(())
    }

    fn checkout(&self, slot: usize, commit: &str, clean_ignored: bool) -> Result<PathBuf, String> {
        let path = self.options.work.join("slots").join(format!("slot-{slot}"));
        if path.join(".git").exists()
            && git_in(
                &path,
                &["checkout", "--quiet", "--detach", "--force", commit],
            )
            .is_ok()
            && git_in(
                &path,
                &["clean", if clean_ignored { "-ffdxq" } else { "-ffdq" }],
            )
            .is_ok()
        {
            return Ok(path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let _ = std::fs::remove_dir_all(&path);
        let _ = git_in(&self.options.clone, &["worktree", "prune"]);
        git_in(
            &self.options.clone,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                &path.display().to_string(),
                commit,
            ],
        )?;
        Ok(path)
    }

    fn run(&self, job: Job, backoff: &Mutex<Option<std::time::Instant>>) {
        let Some(token) = job.lease.as_ref().map(|lease| lease.token.clone()) else {
            return;
        };
        let lease_secs = job
            .lease
            .as_ref()
            .map(|lease| lease.expires_unix.saturating_sub(lease.claimed_unix))
            .unwrap_or(DEFAULT_LEASE_SECS)
            .max(1);
        let cancel = AtomicBool::new(false);
        let finished = AtomicBool::new(false);
        let lost = Mutex::new(String::new());
        let outcome = std::thread::scope(|scope| {
            scope.spawn(|| {
                let every = std::time::Duration::from_millis(lease_secs * 1000 / 4);
                let mut last = std::time::Instant::now();
                while !finished.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    if last.elapsed() < every {
                        continue;
                    }
                    last = std::time::Instant::now();
                    let answer = self
                        .route
                        .call(
                            &["donor", "renew", "--job", &job.id],
                            token.as_bytes(),
                            ROUTE_TIMEOUT,
                        )
                        .and_then(|out| Route::parse_reply::<Renewal>(&out));
                    if let Ok(Renewal::Stop { reason }) = answer {
                        *lost
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = reason;
                        cancel.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            });
            let outcome = self.validate(&job, &token, &cancel, backoff, &lost);
            finished.store(true, Ordering::SeqCst);
            outcome
        });
        match outcome {
            Ok(()) => eprintln!(
                "merge-queue donor {}: reported {} ({})",
                self.options.name, job.id, job.commit
            ),
            Err(Unreported::Declined(reason)) => {
                eprintln!(
                    "merge-queue donor {}: DECLINED {} ({}): {reason}",
                    self.options.name, job.id, job.commit
                );
                self.release(&job, &token, &reason, true);
            }
            Err(Unreported::Released(reason)) => {
                eprintln!(
                    "merge-queue donor {}: gave back {}: {reason}",
                    self.options.name, job.id
                );
                self.release(&job, &token, &reason, false);
            }
            Err(Unreported::Lost(reason)) => {
                // The queue cancelled this job (the serve is stopping, or
                // the candidate was discarded): the runner's process group
                // is already stopped, but a validation unit the validator
                // started on this host (a transient systemd service) is not
                // in that group, so stop it by name here too.
                let (stopped, left) = stop_local_validation_units(&job.id, &job.commit);
                for unit in &stopped {
                    eprintln!(
                        "merge-queue donor {}: stopped local validation unit {unit} for \
                         cancelled job {}",
                        self.options.name, job.id
                    );
                }
                for unit in &left {
                    eprintln!(
                        "merge-queue donor {}: local validation unit {unit} for cancelled \
                         job {} is still running after a stop",
                        self.options.name, job.id
                    );
                }
                eprintln!(
                    "merge-queue donor {}: stopped {}: {reason}",
                    self.options.name, job.id
                );
            }
        }
    }

    fn validate(
        &self,
        job: &Job,
        token: &str,
        cancel: &AtomicBool,
        backoff: &Mutex<Option<std::time::Instant>>,
        lost: &Mutex<String>,
    ) -> Result<(), Unreported> {
        let revision = crate::build_info::revision();
        if let Some(difference) =
            revision_mismatch(job.tool.merge_queue.as_ref(), revision.as_ref())
        {
            // Only an upgrade can match it, here or on the queue host: give the job back
            // rather than decline it, so this donor is offered it again after its back-off (at
            // once, if it is restarted upgraded).
            self.back_off(backoff);
            return Err(Unreported::Released(format!(
                "this donor's merge-queue revision differs from the job's: {difference}"
            )));
        }
        let mut have =
            tool_answers(&job.validator, &self.options.clone).map_err(Unreported::Declined)?;
        let differing = mismatched_tools(&job.tool.tools, &have);
        if !differing.is_empty() {
            let before = tool_mismatch(&job.tool.tools, &have).unwrap_or_default();
            // An older donor catches up and runs the job; one that still differs after
            // that (newer than the job, or unrelated) gives it back, below.
            if let Err(why) = refresh_tools(&job.validator, &self.options.clone, &differing) {
                self.back_off(backoff);
                return Err(Unreported::Released(format!(
                    "this donor's tools differ from the job's ({before}) and cannot be brought up \
                     to date here ({why}), so it cannot tell whether it is older or newer"
                )));
            }
            have =
                tool_answers(&job.validator, &self.options.clone).map_err(Unreported::Declined)?;
            if let Some(after) = tool_mismatch(&job.tool.tools, &have) {
                // Bounded descendant acceptance (#1005): when every differing tool
                // provably descends from the job's stamp, run the job under this
                // donor's newer tools; the receipt records them (`have`), and the
                // queue accepts it only after the same proof in its own repository.
                let differing_now = mismatched_tools(&job.tool.tools, &have);
                let mut descendant = true;
                for name in &differing_now {
                    match tool_descends_from(
                        &job.validator,
                        name,
                        &job.tool.tools[name],
                        &have[name],
                        &self.options.clone,
                    ) {
                        Ok(true) => {}
                        Ok(false) => {
                            descendant = false;
                            break;
                        }
                        Err(error) => {
                            self.back_off(backoff);
                            return Err(Unreported::Released(format!(
                                "this donor's tools still differ from the job's after \
                                 refreshing them ({after}), and whether they descend from \
                                 the job's stamp cannot be told: {error}"
                            )));
                        }
                    }
                }
                if descendant {
                    eprintln!(
                        "merge-queue donor {}: tools newer than {}'s stamp descend from \
                         it ({}); running it under this donor's tools",
                        self.options.name, job.id, after
                    );
                } else {
                    // Still different after a refresh, and not a provable descendant:
                    // this donor is newer in an unrelated way, or unrelated to the
                    // job. Give the job back (with the back-off above) rather than
                    // decline it: a decline would bar this donor until the queue
                    // re-stamps the job, and the re-stamped job may never return
                    // here. As a release it is offered again once the queue
                    // re-stamps it, and other donors may take it meanwhile (#1005
                    // merge-queue-donor-restamp-race).
                    self.back_off(backoff);
                    return Err(Unreported::Released(format!(
                        "this donor's tools still differ from the job's after refreshing them \
                         ({after}), so this donor is newer than the job or unrelated to it"
                    )));
                }
            }
            eprintln!(
                "merge-queue donor {}: refreshed {} for {}; its tools now match ({before} before)",
                self.options.name,
                differing.join(", "),
                job.id
            );
        }
        *self
            .last_validator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job.validator.clone());
        let digest = validator_digest(&job.validator).map_err(Unreported::Declined)?;
        if digest != job.tool.validator_digest {
            return Err(Unreported::Declined(format!(
                "validator settings digest {digest} is not the job's {}",
                job.tool.validator_digest
            )));
        }
        self.fetch(job, token)?;
        // The configured pre-claim check gates taking the job: it runs
        // after fetch, so the candidate tree exists in this clone on
        // every donor (a check may read the candidate itself, e.g.
        // `git show $MQ_CANDIDATE:<path>`). A host that cannot run this
        // candidate gives it back (never a decline, never a refused
        // receipt, never an admission), with back-off. One bundle fetch
        // per released job is the accepted cost; releases are rare.
        if let Err(reason) = self.pre_claim_check(job) {
            self.back_off(backoff);
            return Err(Unreported::Released(reason));
        }
        let slot = loop {
            if let Some(slot) = self
                .slots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop()
            {
                break slot;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let result = (|| {
            let checkout = self
                .checkout(slot, &job.commit, job.validator.clean_ignored)
                .map_err(Unreported::Declined)?;
            let out = self.options.work.join("runs").join(&job.id);
            std::fs::create_dir_all(&out)
                .map_err(|error| Unreported::Declined(error.to_string()))?;
            let validator =
                crate::validate::from_config(&job.validator).map_err(Unreported::Declined)?;
            // Admission is reported only now, when the validator is
            // about to start: every wait before this point (tool
            // materialization, identity refresh, fetch, checkout, and
            // the validator's own admission wrapper inside the run) is
            // still ahead of or at execution, so a lease that never
            // reaches here counts as unstarted for the stale sweeps.
            // A lease admitted here and then held in the validator's
            // materialization/admission wait is protected. An old queue
            // host without the verb refuses the call; that donor binary
            // and its queue must upgrade together, as with the digest.
            self.route
                .call(
                    &["donor", "admit", "--job", &job.id],
                    token.as_bytes(),
                    ROUTE_TIMEOUT,
                )
                .map_err(|error| {
                    Unreported::Released(format!("cannot report admission: {error}"))
                })?;
            let started_at = now();
            // The validator is told the job's base and the candidate's parent
            // from the job record — never whatever this clone's main names,
            // which can be arbitrarily stale (see crate::validate).
            let parent = if job.parent.is_empty() {
                &job.base
            } else {
                &job.parent
            };
            let mut env: Vec<(String, String)> = vec![
                (crate::validate::BASE_ENV.to_owned(), job.base.clone()),
                (crate::validate::PARENT_ENV.to_owned(), parent.clone()),
            ];
            if !job.extra.is_empty() {
                env.push((
                    crate::validate::EXTRA_TESTS_ENV.to_owned(),
                    job.extra
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                ));
            }
            let mut ran = validator
                .run_with(&checkout, &job.selection, &out, cancel, &env)
                .map_err(Unreported::Declined)?;
            // As at the queue: an extra test the runner never reported is missing.
            if !ran.cancelled && ran.interrupted.is_none() && !ran.refused() {
                ran.results
                    .require(&Selection::of(job.extra.iter().cloned()));
            }
            if let Some(why) = &ran.interrupted {
                return Err(Unreported::Released(format!("donor stopping: {why}")));
            }
            // A validator this host cannot execute says nothing about the candidate: no
            // receipt, and another donor (or this one, once fixed) takes the job.
            if let Some(why) = &ran.cannot_execute {
                *backoff
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(std::time::Instant::now() + self.options.mismatch_backoff);
                return Err(Unreported::Declined(format!(
                    "this donor cannot run the validator: {why}"
                )));
            }
            if ran.cancelled {
                let reason = lost
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                return Err(Unreported::Lost(reason));
            }
            let log_text = std::fs::read_to_string(&ran.log).unwrap_or_default();
            let tail_start = log_text.len().saturating_sub(16 * 1024);
            let tail_start = (tail_start..log_text.len())
                .find(|i| log_text.is_char_boundary(*i))
                .unwrap_or(log_text.len());
            let receipt = Receipt {
                schema: RECEIPT_SCHEMA.to_owned(),
                job: job.id.clone(),
                donor: DonorId {
                    name: self.options.name.clone(),
                    host: self.host.clone(),
                },
                commit: job.commit.clone(),
                tree: job.tree.clone(),
                tool: ToolIdentity {
                    validator_digest: digest.clone(),
                    tools: have.clone(),
                    merge_queue: revision.clone(),
                },
                ran_tools: ran.printed_tool_identity.clone(),
                tool_warnings: if ran.refused {
                    Vec::new()
                } else {
                    unattested_warnings(&job.tool.tools, &ran.printed_tool_identity)
                },
                // Pick the cause from the full log, before the tail cut
                // below can leave it above a verbose teardown (#1059).
                refusal_cause: if ran.refused {
                    crate::validate::refusal_cause(&log_text)
                        .map(|line| crate::validate::cut_line(line).to_owned())
                } else {
                    None
                },
                merge_queue_version: env!("CARGO_PKG_VERSION").to_owned(),
                effective_selection: ran.effective_selection.clone(),
                exit: ran.exit,
                refused: ran.refused,
                early_red: ran.early_red,
                results: ran.results.clone(),
                elapsed_ms: ran.elapsed.as_millis() as u64,
                cpu_ms: ran.cpu.map(|cpu| cpu.as_millis() as u64),
                started_at,
                finished_at: now(),
                // Pick the failure-naming lines from the full log, before
                // the tail cut above can leave them behind (#1198).
                failure_lines: crate::validate::failure_lines(
                    &log_text,
                    &job.validator.failure_patterns,
                ),
                log_tail: log_text[tail_start..].to_owned(),
                log_path: ran.log.display().to_string(),
            };
            if receipt.refused {
                // The queue answers a could-not-run receipt by queueing the job again; do not
                // claim straight back.
                *backoff
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(std::time::Instant::now() + self.options.mismatch_backoff);
                // A could-not-run refusal for one candidate is not a host
                // fault by itself: probe the validator (MQ_PROBE=1) right
                // after it. A passing probe means the host is healthy and
                // the refusal was candidate-specific: keep leasing. Only a
                // failing probe records a host fault (queue-side, via the
                // `donor host-fault` route) and pauses claiming. A busy
                // admission refusal is never probed and never a fault.
                // Neither is a stale-base refusal (#1059): the validator
                // marked the job itself stale, so probing this healthy
                // host for it would only hold the job and risk pausing a
                // good donor; the queue's stale sweep settles the job.
                let stale_job = crate::validate::stale_base_refusal(&log_text).is_some();
                let host_scope =
                    crate::validate::could_not_run_scope(&log_text).is_some_and(|(host, _)| host);
                // A pacing-guard refusal (#1193 ghpaced-transient) is
                // shared-budget contention, not a host defect, and a probe
                // cannot fix it: below the bound it is transient (no probe,
                // no fault, no pause; the receipt re-queues the job). At
                // the bound the ordinary probe path decides, so a lasting
                // outage still faults.
                let paced = crate::validate::gh_paced_refusal(&log_text);
                let paced_count = if paced {
                    let mut count = self
                        .paced_refusals
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *count += 1;
                    *count
                } else {
                    0
                };
                // Pacing evidence overrides even a host-scope label
                // (#1193 census route): an adapter that labels its census
                // refusal `host` must not pause the donor when the cause
                // was the pacer. Host-scoped pacing refusals are transient
                // at any count; unscoped ones keep the bound below.
                let paced_transient = paced && (host_scope || paced_count < GH_PACED_FAULT_AFTER);
                if paced_transient {
                    eprintln!(
                        "merge-queue donor {}: pacing-guard refusal ({paced_count} in a row, bound {GH_PACED_FAULT_AFTER}); transient, no host fault",
                        self.options.name
                    );
                }
                let probe_failed = !stale_job
                    && !crate::validate::could_not_run_busy(&log_text)
                    && !paced_transient
                    && !self.probe_host_validator(&job.validator);
                if (probe_failed || host_scope) && !paced_transient {
                    let reason = crate::validate::could_not_run_scope(&log_text)
                        .map(|(_, reason)| reason)
                        .unwrap_or_else(|| {
                            "validator refused (COULD_NOT_RUN) and the probe failed".to_owned()
                        });
                    let since = now();
                    eprintln!(
                        "merge-queue donor {}: host cannot validate: {reason}; pausing claims and probing",
                        self.options.name
                    );
                    // A failing probe records the fault; a host-scope refusal
                    // after a passing probe is recorded queue-side as a flap
                    // cycle when the receipt lands, so it is not sent here.
                    if probe_failed {
                        if let Err(error) = self.route.call(
                            &[
                                "donor",
                                "host-fault",
                                "--donor",
                                &self.options.name,
                                "--reason",
                                &reason,
                            ],
                            b"",
                            ROUTE_TIMEOUT,
                        ) {
                            eprintln!(
                                "merge-queue donor {}: cannot record the host fault: {error}",
                                self.options.name
                            );
                        }
                    }
                    // The pause delay is the running back-off, not a fresh
                    // 30 s: a launch refusal right after a probe pass keeps
                    // doubling it (see `next_host_delay`).
                    let delay = *self
                        .next_host_delay
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let pause = HostPause {
                        reason,
                        since,
                        validator: job.validator.clone(),
                        next: std::time::Instant::now() + delay,
                        delay,
                    };
                    write_host_pause(&self.options.work, &pause);
                    *self
                        .host_pause
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pause);
                }
            }
            if !receipt.refused {
                // An admitted validation: the host really can run again, so
                // the probe back-off starts over.
                *self
                    .next_host_delay
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    std::time::Duration::from_secs(FAULT_PROBE_FIRST_SECS);
                // Pacing contention is over too (#1193).
                *self
                    .paced_refusals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
            }
            let body = serde_json::to_vec(&Report {
                token: token.to_owned(),
                receipt,
            })
            .map_err(|error| Unreported::Declined(error.to_string()))?;
            self.route
                .call(&["donor", "report", "--job", &job.id], &body, ROUTE_TIMEOUT)
                .map_err(Unreported::Lost)?;
            Ok(())
        })();
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(slot);
        result
    }
}

/// Whether an `--until-idle` donor may exit this pass: when its last
/// claim did not fail, no job is running, and the donor is not draining
/// (cap 0). A failed claim is not emptiness: the donor stays and retries
/// on its poll instead of exiting on a transient error.
fn until_idle_exits(until_idle: bool, claim_failed: bool, active: usize, cap: usize) -> bool {
    until_idle && !claim_failed && active == 0 && cap > 0
}

/// Run a donor: claim jobs from the queue through its route, validate them here, report
/// receipts. Returns the exit status.
pub fn run_donor(options: &DonorOptions) -> Result<i32, String> {
    let route = Route::new(options.queue.clone())?;
    if options.name.trim().is_empty() {
        return Err("a donor needs --name".to_owned());
    }
    if !options.clone.join(".git").exists() && !options.clone.join("HEAD").exists() {
        return Err(format!(
            "--clone {} is not a Git repository; clone the queue's repository there first",
            options.clone.display()
        ));
    }
    let revision = crate::build_info::revision();
    if let Some(problem) = crate::build_info::unusable(revision.as_ref()) {
        return Err(format!(
            "{problem}; a donor's merge-queue must match the queue host's revision"
        ));
    }
    preflight_against_queue(options, &route, revision.as_ref())?;
    crate::interrupt::install();
    let jobs = options.jobs.max(1);
    // A previous process may have paused on a host fault: restore that
    // pause (due to probe at once) or the queue-side fault would refuse
    // every claim and no probe would ever clear it.
    let restored_pause = match restored_host_pause(&options.work) {
        Ok(Some(pause)) => {
            eprintln!(
                "merge-queue donor {}: restoring host pause from {} ({}; paused since {}); probing",
                options.name,
                options.work.join("host-pause.json").display(),
                pause.reason,
                pause.since
            );
            Some(pause)
        }
        Ok(None) => None,
        Err(why) => {
            eprintln!(
                "merge-queue donor {}: cannot resume probing: {why}; \
                 if the queue holds a host fault for this donor it stays until a passing \
                 probe claim (`donor claim --probe-passed`) or an admitted validation",
                options.name
            );
            None
        }
    };
    let restored_delay = restored_pause.as_ref().map_or(
        std::time::Duration::from_secs(FAULT_PROBE_FIRST_SECS),
        |pause| pause.delay,
    );
    let worker = Worker {
        options,
        route: &route,
        host: host_name(),
        slots: Mutex::new((0..jobs).rev().collect()),
        host_pause: Mutex::new(restored_pause),
        probe_passed: AtomicBool::new(false),
        next_host_delay: Mutex::new(restored_delay),
        last_validator: Mutex::new(None),
        next_fault_probe: Mutex::new(
            std::time::Instant::now() + std::time::Duration::from_secs(FAULT_PROBE_FIRST_SECS),
        ),
        fault_probe_current: Mutex::new(std::time::Duration::from_secs(FAULT_PROBE_FIRST_SECS)),
        paced_refusals: Mutex::new(0),
    };
    let backoff: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let active = AtomicUsize::new(0);
    // The cap the donor claims under now, from its cap file (#918 donor-cap-file), and the
    // malformed text last said of the file, so each change and each new bad value is logged once.
    let mut cap = jobs;
    let mut bad_cap: Option<String> = None;
    eprintln!(
        "merge-queue donor {}: on {}, up to {jobs} job(s){}, clone {}, merge-queue {}",
        options.name,
        worker.host,
        options
            .cap_file
            .as_ref()
            .map_or_else(String::new, |path| format!(
                " (capped by {})",
                path.display()
            )),
        options.clone.display(),
        revision.as_ref().map_or_else(String::new, |r| r.describe())
    );
    std::thread::scope(|scope| -> Result<(), String> {
        loop {
            if crate::interrupt::requested() {
                break;
            }
            let backing_off = backoff
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some_and(|until| std::time::Instant::now() < until);
            // Host pause: claim nothing; probe on the back-off, and resume with a
            // probe-passed claim once the validator admits work again.
            let host_paused = {
                let mut pause = worker
                    .host_pause
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match pause.as_mut() {
                    None => false,
                    Some(state) if std::time::Instant::now() < state.next => true,
                    Some(state) => {
                        if worker.probe_host_validator(&state.validator.clone()) {
                            eprintln!(
                                "merge-queue donor {}: host probe passed ({}; paused since {}); resuming",
                                options.name, state.reason, state.since
                            );
                            // If the launch right after this pass refuses
                            // host-wide again, the back-off keeps doubling
                            // from here instead of resetting to 30 s.
                            *worker
                                .next_host_delay
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                doubled_probe_delay(state.delay);
                            *pause = None;
                            let _ = std::fs::remove_file(options.work.join("host-pause.json"));
                            let _ = route.call(
                                &[
                                    "donor",
                                    "clear-fault",
                                    "--donor",
                                    &options.name,
                                    "--probe-passed",
                                ],
                                b"",
                                ROUTE_TIMEOUT,
                            );
                            worker.probe_passed.store(true, Ordering::SeqCst);
                            false
                        } else {
                            state.delay = fault_probe_delay(state.delay);
                            state.next = std::time::Instant::now() + state.delay;
                            let _ = route.call(
                                &[
                                    "donor",
                                    "host-fault",
                                    "--donor",
                                    &options.name,
                                    "--reason",
                                    &state.reason.clone(),
                                ],
                                b"",
                                ROUTE_TIMEOUT,
                            );
                            true
                        }
                    }
                }
            };
            // A queue-side host fault with no local host-pause file must not
            // deadlock: re-probe on the fault schedule (60 s backing off to
            // 5 min) using the last validator this donor ran, and clear the
            // fault through the route that never leases when a probe passes.
            if !host_paused {
                let due = {
                    let next = worker
                        .next_fault_probe
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    std::time::Instant::now() >= *next
                };
                if due {
                    let faulted = route
                        .call(
                            &["donor", "fault", "--donor", &options.name],
                            b"",
                            ROUTE_TIMEOUT,
                        )
                        .and_then(|out| Route::parse_reply::<serde_json::Value>(&out))
                        .map(|reply| !reply["fault"].is_null())
                        .unwrap_or(false);
                    let validator = worker
                        .last_validator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if faulted {
                        if let Some(cfg) = validator {
                            if worker.probe_host_validator(&cfg) {
                                eprintln!(
                                    "merge-queue donor {}: fault re-probe passed with no host pause; clearing the fault",
                                    options.name
                                );
                                let _ = route.call(
                                    &[
                                        "donor",
                                        "clear-fault",
                                        "--donor",
                                        &options.name,
                                        "--probe-passed",
                                    ],
                                    b"",
                                    ROUTE_TIMEOUT,
                                );
                                worker.probe_passed.store(true, Ordering::SeqCst);
                                *worker
                                    .fault_probe_current
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    std::time::Duration::from_secs(FAULT_PROBE_FIRST_SECS);
                            } else {
                                let _ = route.call(
                                    &[
                                        "donor",
                                        "host-fault",
                                        "--donor",
                                        &options.name,
                                        "--reason",
                                        "fault re-probe failed",
                                    ],
                                    b"",
                                    ROUTE_TIMEOUT,
                                );
                                let mut delay = worker
                                    .fault_probe_current
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                *delay = fault_probe_delay(*delay);
                            }
                        }
                    }
                    let delay = *worker
                        .fault_probe_current
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *worker
                        .next_fault_probe
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        std::time::Instant::now() + delay;
                }
            }
            if let Some(path) = options.cap_file.as_deref() {
                let file = read_cap_file(path);
                match &file {
                    Err(text) if bad_cap.as_deref() != Some(text.as_str()) => {
                        eprintln!(
                            "merge-queue donor {}: ignoring cap file {}: {text:?} is not a whole \
                             number of jobs; claiming under --jobs {jobs}",
                            options.name,
                            path.display()
                        );
                        bad_cap = Some(text.clone());
                    }
                    Err(_) => {}
                    Ok(_) => bad_cap = None,
                }
                let now = effective_cap(jobs, &file);
                if now != cap {
                    eprintln!(
                        "merge-queue donor {}: cap now {now} (was {cap}) from {}{}",
                        options.name,
                        path.display(),
                        if now == 0 {
                            "; draining: running jobs finish, nothing more is claimed"
                        } else {
                            ""
                        }
                    );
                    cap = now;
                }
            }
            let cap_text = cap.to_string();
            let mut claimed = false;
            // A claim that failed (route timeout, queue busy) says nothing
            // about emptiness, so it must not end an `--until-idle` donor;
            // the poll at the foot of the loop is its retry back-off. Any
            // other unclaimed pass (a genuinely empty queue, a back-off, a
            // host pause) keeps the documented exit.
            let mut claim_failed = false;
            // Draining: lease nothing, but say so to the queue, so its `donor status` shows the
            // cap. A queue too old to know `--drain` refuses the claim, which leases nothing too.
            if cap == 0 && !host_paused {
                let args = [
                    "donor",
                    "claim",
                    "--donor",
                    &options.name,
                    "--host",
                    &worker.host,
                    "--drain",
                ];
                if let Err(error) = route.call(&args, b"", ROUTE_TIMEOUT) {
                    eprintln!(
                        "merge-queue donor {}: drain note failed: {error}",
                        options.name
                    );
                }
            }
            if !host_paused && !backing_off && active.load(Ordering::SeqCst) < cap {
                let probe_passed = worker.probe_passed.swap(false, Ordering::SeqCst);
                let mut args: Vec<&str> = vec![
                    "donor",
                    "claim",
                    "--donor",
                    &options.name,
                    "--host",
                    &worker.host,
                    "--jobs",
                    &cap_text,
                ];
                args.push("--reports-admission");
                // Declare this binary's revision so the queue can refuse
                // the claim up front instead of the receipt (#1105). A
                // queue too old to know the flags refuses the claim,
                // which leases nothing, as with --drain above.
                let own_revision = crate::build_info::revision();
                if let Some(revision) = &own_revision {
                    args.push("--revision");
                    args.push(&revision.commit);
                    if revision.dirty {
                        args.push("--revision-dirty");
                    }
                }
                if probe_passed {
                    args.push("--probe-passed");
                }
                let answer = route.call(&args, b"", ROUTE_TIMEOUT);
                match answer.and_then(|out| Route::parse_reply::<Claimed>(&out)) {
                    Ok(Claimed { job: Some(job), .. }) => {
                        claimed = true;
                        active.fetch_add(1, Ordering::SeqCst);
                        eprintln!(
                            "merge-queue donor {}: took {} ({}), cap {cap}",
                            options.name, job.id, job.commit
                        );
                        let (worker, backoff, active) = (&worker, &backoff, &active);
                        scope.spawn(move || {
                            worker.run(job, backoff);
                            active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Ok(Claimed {
                        job: None,
                        refused: Some(reason),
                    }) => {
                        // Refused before leasing (a revision mismatch):
                        // every claim will be refused until the binaries
                        // match, so back off like a tool mismatch, and do
                        // not read it as an empty queue.
                        claim_failed = true;
                        worker.back_off(&backoff);
                        eprintln!(
                            "merge-queue donor {}: claim refused: {reason}",
                            options.name
                        );
                    }
                    Ok(Claimed { job: None, .. }) => {}
                    Err(error) => {
                        claim_failed = true;
                        eprintln!("merge-queue donor {}: claim failed: {error}", options.name)
                    }
                }
            }
            // A draining donor (cap 0) stays: it is idle by choice, not for want of work.
            if until_idle_exits(
                options.until_idle,
                claim_failed,
                active.load(Ordering::SeqCst),
                cap,
            ) {
                break;
            }
            if claimed {
                continue;
            }
            let until = std::time::Instant::now() + options.poll;
            while std::time::Instant::now() < until && !crate::interrupt::requested() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Ok(())
    })?;
    Ok(0)
}

/// Before claiming anything, check this donor against the queue: its clone's remote must be
/// the queue's repository (in any URL form of it), and a different merge-queue revision is
/// reported now rather than on every job. A queue too old to answer `donor info` is only warned
/// about.
fn preflight_against_queue(
    options: &DonorOptions,
    route: &Route,
    revision: Option<&crate::build_info::Revision>,
) -> Result<(), String> {
    let info = match route
        .call(&["donor", "info"], b"", ROUTE_TIMEOUT)
        .and_then(|out| Route::parse_reply::<QueueInfo>(&out))
    {
        Ok(info) => info,
        Err(error) => {
            eprintln!(
                "merge-queue donor {}: warning: the queue did not answer `donor info` ({error}); \
                 its repository and revision are not checked before claiming",
                options.name
            );
            return Ok(());
        }
    };
    if let (Some(queue), Some(ours)) = (
        info.remote_url.as_deref(),
        remote_url(&options.clone, &options.remote),
    ) {
        if !same_repository(queue, &ours) {
            return Err(format!(
                "--clone {} has remote {:?} at {ours}, which is not the queue's repository \
                 {queue}; clone the queue's repository (any URL form of it works)",
                options.clone.display(),
                options.remote
            ));
        }
        if queue != ours {
            eprintln!(
                "merge-queue donor {}: remote {:?} is {ours}, the queue's is {queue}: the same \
                 repository in another form",
                options.name, options.remote
            );
        }
    }
    if let Some(difference) = revision_mismatch(info.merge_queue.as_ref(), revision) {
        eprintln!(
            "merge-queue donor {}: warning: {difference}; every job will be declined until \
             they match",
            options.name
        );
    }
    Ok(())
}

/// `donor claim` output.
#[derive(Serialize, Deserialize)]
pub struct Claimed {
    /// The leased job (with its token), or `null` when there is nothing for this donor.
    pub job: Option<Job>,
    /// Why the claim itself was refused (a declared revision the queue
    /// host was not built from, #1105), when it was. Absent in replies
    /// from older queue hosts and on every ordinary claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// A job as `donor status` shows it: no lease token.
pub fn redacted(job: &Job) -> serde_json::Value {
    let mut value = serde_json::to_value(job).unwrap_or(serde_json::Value::Null);
    if let Some(lease) = value
        .get_mut("lease")
        .and_then(|lease| lease.as_object_mut())
    {
        lease.remove("token");
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("validator");
        if let Some(receipt) = object.get_mut("receipt").and_then(|r| r.as_object_mut()) {
            receipt.remove("results");
            receipt.remove("log_tail");
        }
    }
    value
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::tests::TestId;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mq-donor-{tag}-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn validator() -> ValidatorConfig {
        ValidatorConfig {
            kind: "command".to_owned(),
            run: "exit 0".to_owned(),
            ..ValidatorConfig::default()
        }
    }

    fn publish(store: &Store, commit: &str) -> Job {
        publish_rooted(store, commit, false)
    }

    fn publish_rooted(store: &Store, commit: &str, rooted: bool) -> Job {
        let validator = validator();
        let tool = ToolIdentity {
            validator_digest: validator_digest(&validator).unwrap(),
            tools: BTreeMap::new(),
            merge_queue: Some(crate::build_info::Revision {
                commit: "a".repeat(40),
                dirty: false,
            }),
        };
        store
            .publish(
                commit,
                "tree0",
                "base0",
                "base0",
                &Selection::of([TestId::step("a")]),
                &BTreeSet::new(),
                &validator,
                &tool,
                rooted,
                false,
                None,
            )
            .unwrap()
    }

    #[test]
    fn a_job_file_written_before_parent_was_recorded_reads_with_no_parent() {
        let store = Store::new(&temp("job-parent-backcompat"));
        let job = publish(&store, "c1");
        assert_eq!(job.parent, "base0");
        let path = store.dir.join("jobs").join(format!("{}.json", job.id));
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value.as_object_mut().unwrap().remove("parent").is_some());
        let old: Job = serde_json::from_value(value).unwrap();
        assert_eq!(old.parent, "");
        assert_eq!(old.base, "base0");
    }

    fn receipt(job: &Job, donor: &str) -> Receipt {
        Receipt {
            schema: RECEIPT_SCHEMA.to_owned(),
            job: job.id.clone(),
            donor: DonorId {
                name: donor.to_owned(),
                host: "h".to_owned(),
            },
            commit: job.commit.clone(),
            tree: job.tree.clone(),
            tool: job.tool.clone(),
            ran_tools: job
                .tool
                .tools
                .iter()
                .map(|(name, value)| (name.clone(), vec![value.clone()]))
                .collect(),
            merge_queue_version: env!("CARGO_PKG_VERSION").to_owned(),
            exit: Some(0),
            ..Receipt::default()
        }
    }

    #[test]
    fn a_reply_with_leading_noise_still_parses() {
        let noisy = b"merge-queue: warning: config sets unknown field(s) peripheral -- ignored\n\
                      note: --ws-url and AGENTCLOUD_ORCHESTRATOR_URL are both unset\n\
                      {\"job\":null}\n";
        let claimed: Claimed = Route::parse_reply(noisy).unwrap();
        assert!(claimed.job.is_none());
        // Noise with no JSON at all is an error naming the last line, not a parse panic.
        let error = match Route::parse_reply::<Claimed>(b"note: only noise\n") {
            Ok(_) => panic!("noise must not parse"),
            Err(error) => error,
        };
        assert!(error.contains("no JSON reply"), "{error}");
        // A clean reply parses exactly as before.
        let clean: Claimed = Route::parse_reply(b"{\"job\":null}").unwrap();
        assert!(clean.job.is_none());
    }

    #[test]
    fn a_route_error_shows_the_remote_stdout_tail_not_the_local_banner() {
        let route = Route::new(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "echo 'note: --ws-url wss://example.invalid'; echo remote-real-error; \
             echo local-stderr >&2; exit 3"
                .to_owned(),
        ])
        .unwrap();
        let error = route
            .call(&["claim"], b"", std::time::Duration::from_secs(30))
            .unwrap_err();
        assert!(error.contains("remote-real-error"), "{error}");
        assert!(error.contains("local-stderr"), "{error}");
        assert!(!error.contains("--ws-url"), "{error}");
    }

    #[test]
    fn a_receipt_without_early_red_still_parses_and_a_flagged_one_round_trips() {
        let store = Store::new(&temp("receipt-early-red"));
        let job = publish(&store, "c1");
        let plain = receipt(&job, "d1");
        assert!(!plain.early_red);
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("early_red"), "{text}");
        let back: Receipt = serde_json::from_str(&text).unwrap();
        assert_eq!(back, plain);
        // A receipt written before the field existed parses with it off.
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value.as_object_mut().unwrap().remove("early_red");
        let old: Receipt = serde_json::from_value(value).unwrap();
        assert!(!old.early_red);
        let mut flagged = plain.clone();
        flagged.early_red = true;
        let text = serde_json::to_string(&flagged).unwrap();
        let back: Receipt = serde_json::from_str(&text).unwrap();
        assert!(back.early_red);
    }

    #[test]
    fn a_refused_receipt_names_the_refused_line_not_the_log_tail() {
        let mut store = Store::new(&temp("refused-line"));
        store.could_not_run_limit = 99;
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let error = store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "REFUSED: pinned image not present\nvalidate-lock RELEASED\nbox FREE",
                ),
            )
            .unwrap_err();
        assert!(error.contains("pinned image not present"), "{error}");
        assert!(!error.contains("box FREE"), "{error}");
        let after = store.read(&claimed.id).unwrap();
        assert!(
            after.could_not_run[0].contains("pinned image not present"),
            "{after:?}"
        );
    }

    #[test]
    fn a_stale_base_refusal_costs_no_strike_and_names_its_cause() {
        // #1059: the validator refused because the job itself is stale
        // (exit 75 plus the structured stale-base scope line). Like a
        // job the stale sweep cancels, that is not a donor failure: no
        // could-not-run strike, no host fault, and the recorded detail
        // is the refusal cause, not the teardown line printed after it.
        let mut store = Store::new(&temp("stale-base-refusal"));
        store.could_not_run_limit = 99;
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let error = store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "MQ_COULD_NOT_RUN_SCOPE: job stale-base target does not contain \
                     the job base\nCOULD_NOT_RUN\nvalidate-run: RETIRE-RETAINED \
                     run may still use slot s1",
                ),
            )
            .unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert!(
            error.contains("target does not contain the job base"),
            "{error}"
        );
        assert!(!error.contains("RETIRE-RETAINED"), "{error}");
        let after = store.read(&claimed.id).unwrap();
        assert_eq!(after.state, JobState::Queued, "{after:?}");
        assert!(
            after.could_not_run.is_empty(),
            "a stale job is not a donor failure: {after:?}"
        );
        assert!(after.declined_by.is_empty(), "{after:?}");
        let history = after.history.join("\n");
        assert!(
            history.contains("target does not contain the job base"),
            "{history}"
        );
        assert!(!history.contains("RETIRE-RETAINED"), "{history}");
        assert!(store.host_fault_list().is_empty());
        // The next job is taken normally by the same donor.
        publish(&store, "c2");
        assert!(store.claim("d1", "h1", 1).unwrap().is_some());
    }

    #[test]
    fn a_pacing_guard_refusal_requeues_counted_with_no_host_fault() {
        // #1193 ghpaced-transient, #1241 paced-edges: exit 75 because the
        // credential pacing guard refused a git login. Queue side: the job
        // is queued again with no strike (pacing cannot reach the per-job
        // bound), the history names the pacing cause, and no host fault
        // is recorded; the donor takes the next job normally.
        let mut store = Store::new(&temp("ghpaced-refusal"));
        store.could_not_run_limit = 99;
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let error = store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "GH-PACED REFUSED: login budget exhausted\nCOULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        assert!(error.contains("queued again"), "{error}");
        let after = store.read(&claimed.id).unwrap();
        assert_eq!(after.state, JobState::Queued, "{after:?}");
        assert!(after.could_not_run.is_empty(), "{after:?}");
        let history = after.history.join("\n");
        assert!(history.contains("GH-PACED REFUSED"), "{history}");
        assert!(history.contains("no strike (pacing"), "{history}");
        assert!(store.host_fault_list().is_empty());
        publish(&store, "c2");
        assert!(store.claim("d1", "h1", 1).unwrap().is_some());
    }

    #[test]
    fn a_host_scoped_pacing_refusal_after_a_probe_pass_is_no_flap_fault() {
        // #1193 census route: the adapter labels its census refusal
        // `host`, and the donor's probe just passed, which would normally
        // complete a flap cycle and record the fault. Pacing evidence
        // overrides the label: counted requeue, no fault.
        let mut store = Store::new(&temp("ghpaced-host-scope"));
        store.could_not_run_limit = 99;
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store.note_probe_pass("d1");
        let error = store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "merge-queue adapter: census probe: GH-PACED REFUSED [bot] \
                     git_credential budget 1 per 10 s (burst 1) per host reached\n\
                     MQ_COULD_NOT_RUN_SCOPE: host census-published-ref-unreachable\n\
                     COULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        assert!(error.contains("queued again"), "{error}");
        let after = store.read(&claimed.id).unwrap();
        assert_eq!(after.state, JobState::Queued, "{after:?}");
        assert!(after.could_not_run.is_empty(), "{after:?}");
        assert!(store.host_fault_list().is_empty());
    }

    #[test]
    fn a_probe_that_is_paced_once_then_passes_records_no_fault() {
        // #1193 census route: the first probe attempt is refused by the
        // pacer; the in-probe retry after the pacer slot passes, so the
        // probe passes (the caller faults only on a false return).
        let work = temp("probe-paced");
        let count = work.join("count");
        let run = format!(
            "n=$(cat {} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {}; \
             if [ $n -lt 2 ]; then echo 'GH-PACED REFUSED: budget reached'; exit 75; fi; exit 0",
            count.display(),
            count.display()
        );
        let (options, route) = probe_worker(&work);
        let worker = Worker {
            options: &options,
            route: &route,
            host: "h1".to_owned(),
            slots: Mutex::new(vec![0]),
            host_pause: Mutex::new(None),
            probe_passed: AtomicBool::new(false),
            next_host_delay: Mutex::new(std::time::Duration::from_secs(60)),
            last_validator: Mutex::new(None),
            next_fault_probe: Mutex::new(std::time::Instant::now()),
            fault_probe_current: Mutex::new(std::time::Duration::from_secs(60)),
            paced_refusals: Mutex::new(0),
        };
        assert!(worker.probe_host_validator(&probe_cfg(run)));
        assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "2");
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_probe_slower_than_the_default_bound_passes_with_a_raised_one() {
        // #1271 probe-timeout-bound: the probe needs 2 s; killed at a 1 s
        // bound (named as a timeout), it passes with the bound raised.
        let work = temp("probe-timeout");
        let (options, route) = probe_worker(&work);
        let worker = Worker {
            options: &options,
            route: &route,
            host: "h1".to_owned(),
            slots: Mutex::new(vec![0]),
            host_pause: Mutex::new(None),
            probe_passed: AtomicBool::new(false),
            next_host_delay: Mutex::new(std::time::Duration::from_secs(60)),
            last_validator: Mutex::new(None),
            next_fault_probe: Mutex::new(std::time::Instant::now()),
            fault_probe_current: Mutex::new(std::time::Duration::from_secs(60)),
            paced_refusals: Mutex::new(0),
        };
        let mut cfg = probe_cfg("sleep 2; exit 0".to_owned());
        cfg.probe_timeout_secs = 1;
        let (passed, output) = worker.probe_once(&cfg);
        assert!(!passed);
        assert!(output.contains("probe killed at the 1 s bound"), "{output}");
        cfg.probe_timeout_secs = 5;
        assert!(worker.probe_host_validator(&cfg));
        let _ = std::fs::remove_dir_all(&work);
    }

    fn probe_worker(work: &Path) -> (DonorOptions, Route) {
        (
            DonorOptions {
                queue: vec!["true".to_owned()],
                clone: work.to_path_buf(),
                remote: String::new(),
                name: "d1".to_owned(),
                jobs: 1,
                poll: std::time::Duration::from_secs(1),
                work: work.to_path_buf(),
                mismatch_backoff: std::time::Duration::ZERO,
                until_idle: true,
                cap_file: None,
            },
            Route::new(vec!["true".to_owned()]).unwrap(),
        )
    }

    fn probe_cfg(run: String) -> crate::state::ValidatorConfig {
        crate::state::ValidatorConfig {
            kind: "command".to_owned(),
            run,
            ..Default::default()
        }
    }

    #[test]
    fn a_probe_that_waits_on_the_entry_lock_retries_inside_the_probe() {
        // #1193 item 5: the first probe fails only on the entry lock wait,
        // the retry passes, so the probe passes and no fault is recorded
        // (the caller faults only on a false return).
        let work = temp("probe-lock-wait");
        let count = work.join("count");
        let run = format!(
            "n=$(cat {} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {}; \
             if [ $n -lt 2 ]; then echo 'validation entry lock wait bound expired'; exit 1; fi; exit 0",
            count.display(),
            count.display()
        );
        let (options, route) = probe_worker(&work);
        let worker = Worker {
            options: &options,
            route: &route,
            host: "h1".to_owned(),
            slots: Mutex::new(vec![0]),
            host_pause: Mutex::new(None),
            probe_passed: AtomicBool::new(false),
            next_host_delay: Mutex::new(std::time::Duration::from_secs(60)),
            last_validator: Mutex::new(None),
            next_fault_probe: Mutex::new(std::time::Instant::now()),
            fault_probe_current: Mutex::new(std::time::Duration::from_secs(60)),
            paced_refusals: Mutex::new(0),
        };
        assert!(worker.probe_host_validator(&probe_cfg(run)));
        assert_eq!(
            std::fs::read_to_string(&count).unwrap().trim(),
            "2",
            "one lock-wait failure, one retry"
        );
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn repeated_entry_lock_waits_fail_the_probe_visibly() {
        // Past the bound the probe fails as any probe does (the caller
        // records the fault): exactly the bounded attempts ran.
        let work = temp("probe-lock-wait-bound");
        let count = work.join("count");
        let run = format!(
            "n=$(cat {} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {}; \
             echo 'validation entry lock wait bound expired'; exit 1",
            count.display(),
            count.display()
        );
        let (options, route) = probe_worker(&work);
        let worker = Worker {
            options: &options,
            route: &route,
            host: "h1".to_owned(),
            slots: Mutex::new(vec![0]),
            host_pause: Mutex::new(None),
            probe_passed: AtomicBool::new(false),
            next_host_delay: Mutex::new(std::time::Duration::from_secs(60)),
            last_validator: Mutex::new(None),
            next_fault_probe: Mutex::new(std::time::Instant::now()),
            fault_probe_current: Mutex::new(std::time::Duration::from_secs(60)),
            paced_refusals: Mutex::new(0),
        };
        assert!(!worker.probe_host_validator(&probe_cfg(run)));
        assert_eq!(
            std::fs::read_to_string(&count).unwrap().trim(),
            PROBE_LOCK_WAIT_ATTEMPTS.to_string(),
            "the bounded attempts ran"
        );
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_stale_cause_carried_on_the_receipt_survives_a_cut_tail() {
        // #1059: a new donor picks the cause from its full log onto the
        // receipt, so the queue still classifies and records it when
        // the scope line fell above the log tail's cut. A receipt
        // written before the field existed parses without it.
        let mut store = Store::new(&temp("stale-cause-field"));
        store.could_not_run_limit = 99;
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let mut receipt = refused_receipt(
            &claimed,
            "d1",
            "validate-run: RETIRE-RETAINED run may still use slot s1",
        );
        receipt.refusal_cause = Some(
            "MQ_COULD_NOT_RUN_SCOPE: job stale-base target does not contain the job base"
                .to_owned(),
        );
        let mut old: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&receipt).unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("refusal_cause");
        let old: Receipt = serde_json::from_value(old).unwrap();
        assert_eq!(old.refusal_cause, None);
        let error = store.report(&claimed.id, &token, receipt).unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert!(
            error.contains("target does not contain the job base"),
            "{error}"
        );
        assert!(!error.contains("RETIRE-RETAINED"), "{error}");
        let after = store.read(&claimed.id).unwrap();
        assert!(after.could_not_run.is_empty(), "{after:?}");
        assert!(store.host_fault_list().is_empty());
    }

    #[test]
    fn a_red_receipts_verdict_names_the_failure_line() {
        // #1198: the carried lines win; an older receipt falls back to tail.
        let store = Store::new(&temp("failure-line"));
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let mut red = receipt(&claimed, "d1");
        red.exit = Some(1);
        red.failure_lines = vec!["run-script-tests: FAILED suite-9 (status 3)".to_owned()];
        red.log_tail = "teardown only\n".to_owned();
        assert_eq!(
            red.verdict().failure_line,
            "run-script-tests: FAILED suite-9 (status 3)"
        );
        red.failure_lines = Vec::new();
        red.log_tail = "FAILED node-1\n".to_owned();
        assert_eq!(red.verdict().failure_line, "FAILED node-1");
        // The compare detail names the donor's failure line.
        let detail = compare(
            &Verdict {
                green: true,
                ..Verdict::default()
            },
            &red.verdict(),
        );
        assert!(detail.contains("first failure: FAILED node-1"), "{detail}");
    }

    fn refused_receipt(job: &Job, donor: &str, log_tail: &str) -> Receipt {
        let mut receipt = receipt(job, donor);
        receipt.refused = true;
        receipt.exit = Some(75);
        receipt.results = crate::tests::TestResults::default();
        receipt.log_tail = log_tail.to_owned();
        receipt
    }

    #[test]
    fn a_refusal_alone_is_not_a_host_fault_and_a_failing_probe_is() {
        let mut store = Store::new(&temp("host-scope"));
        store.could_not_run_limit = 99;
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let error = store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "MQ_COULD_NOT_RUN_SCOPE: host wrkslots wedged\nCOULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        assert!(error.contains("queued again"), "{error}");
        let after = store.read(&claimed.id).unwrap();
        assert_eq!(after.state, JobState::Queued, "{after:?}");
        assert_eq!(after.could_not_run.len(), 1, "{after:?}");
        // No fault: the donor is leased other jobs at once.
        assert!(store.host_fault_list().is_empty());
        publish(&store, "c2");
        assert!(store.claim("d1", "h1", 1).unwrap().is_some());
        // Only a failing probe right after the refusal records the fault.
        store.record_host_fault("d1", "wrkslots wedged");
        assert_eq!(store.host_fault_list().len(), 1);
        let details = store.host_fault_details();
        assert_eq!(details[0].4, "failed", "{details:?}");
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
        // A passing re-probe clears it, with no lease taken by the clearing.
        store.record_probe_result("d1", true);
        assert!(store.host_fault_list().is_empty());
        // A busy admission refusal is never a host fault.
        let claimed = store.claim("d1", "h1", 3).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "validate lock busy: slots full\nCOULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        assert!(store.host_fault_list().is_empty());
        assert!(store
            .read(&claimed.id)
            .unwrap()
            .history
            .join("\n")
            .contains("busy admission refusal: never a host fault"));
        // #992: a wrkslots live-journal busy receipt is the same retryable
        // busy, never a host fault, and the job is queued again.
        publish(&store, "c3");
        let claimed = store.claim("d1", "h1", 3).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let error = store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "BUSY: busy: live 'finish' mutation ACTIVE.testhost.journal by pid 4242; retry\nCOULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        assert!(error.contains("queued again"), "{error}");
        assert!(store.host_fault_list().is_empty());
    }

    #[test]
    fn every_eligible_donor_faulted_only_when_no_known_donor_is_healthy() {
        // Nobody known at all: not a host pause, the claim timeout applies.
        let store = Store::new(&temp("eligible-none"));
        let job = publish(&store, "c1");
        assert!(!store.every_eligible_donor_faulted(&job));

        // The only known donor is the faulted one: every eligible donor is
        // faulted, so a decide-mode run keeps waiting.
        let store = Store::new(&temp("eligible-only-faulted"));
        publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "MQ_COULD_NOT_RUN_SCOPE: host wedged\nCOULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        store.record_host_fault("d1", "wedged");
        let waiting = store.read(&claimed.id).unwrap();
        assert!(store.every_eligible_donor_faulted(&waiting));

        // A second donor the store knows from an admitted receipt is healthy
        // but idle: the timeout must fire, not the host-pause hold.
        let store = Store::new(&temp("eligible-one-healthy"));
        publish(&store, "c1");
        let claimed = store.claim("d2", "h2", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(&claimed.id, &token, receipt(&claimed, "d2"))
            .unwrap();
        publish(&store, "c2");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(
                &claimed.id,
                &token,
                refused_receipt(
                    &claimed,
                    "d1",
                    "MQ_COULD_NOT_RUN_SCOPE: host wedged\nCOULD_NOT_RUN",
                ),
            )
            .unwrap_err();
        store.record_host_fault("d1", "wedged");
        let waiting = store.read(&claimed.id).unwrap();
        assert!(!store.every_eligible_donor_faulted(&waiting));
    }

    #[test]
    fn probe_pass_launch_refuse_cycles_escalate_until_an_admitted_validation() {
        let mut store = Store::new(&temp("host-flap"));
        store.host_fault_escalate_after = 3;
        store.could_not_run_limit = 99;
        let host_refuse = |store: &Store| {
            let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
            let token = claimed.lease.as_ref().unwrap().token.clone();
            store
                .report(
                    &claimed.id,
                    &token,
                    refused_receipt(
                        &claimed,
                        "d1",
                        "MQ_COULD_NOT_RUN_SCOPE: host wedged\nCOULD_NOT_RUN",
                    ),
                )
                .unwrap_err();
            // The probe run right after the refusal failed too.
            store.record_host_fault("d1", "wedged");
        };
        publish(&store, "c1");
        host_refuse(&store);
        assert!(store.host_fault_escalations().is_empty());
        // Each probe pass clears an ordinary fault, and the launch refusal
        // right after it completes one cycle instead of resetting the count.
        for cycle in 1..=3 {
            store.note_probe_pass("d1");
            host_refuse(&store);
            if cycle < 3 {
                assert!(
                    store.host_fault_escalations().is_empty(),
                    "cycle {cycle} must not escalate yet"
                );
            }
        }
        let escalations = store.host_fault_escalations();
        assert_eq!(escalations.len(), 1, "{escalations:?}");
        assert!(
            escalations[0].contains("ERROR: donor d1"),
            "{escalations:?}"
        );
        assert!(
            escalations[0].contains("an admitted validation"),
            "{escalations:?}"
        );
        // A probe pass no longer clears an escalated fault, but it arms one
        // trial claim; only that job's admitted validation clears it.
        store.note_probe_pass("d1");
        assert_eq!(store.host_fault_list().len(), 1);
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a trial job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(&claimed.id, &token, receipt(&claimed, "d1"))
            .unwrap();
        assert!(store.host_fault_list().is_empty());
        assert!(store.host_fault_escalations().is_empty());
        // The next fault starts a fresh count, not an escalated one.
        publish(&store, "c2");
        host_refuse(&store);
        assert!(store.host_fault_escalations().is_empty());
    }

    #[test]
    fn a_persisted_host_pause_restores_its_validator_for_a_restart() {
        let work = temp("host-pause-restore");
        assert!(restored_host_pause(&work).unwrap().is_none());
        let pause = HostPause {
            reason: "wedged".to_owned(),
            since: "2026-11-05T20:40:05Z".to_owned(),
            validator: validator(),
            next: std::time::Instant::now() + std::time::Duration::from_secs(30),
            delay: std::time::Duration::from_secs(120),
        };
        write_host_pause(&work, &pause);
        let text = std::fs::read_to_string(work.join("host-pause.json")).unwrap();
        let written: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written["validator"]["kind"], "command");
        assert_eq!(written["validator"]["run"], "exit 0");
        assert_eq!(written["delay_secs"], 120);
        let restored = restored_host_pause(&work)
            .unwrap()
            .expect("a persisted pause must restore");
        assert_eq!(restored.reason, "wedged");
        assert_eq!(restored.since, "2026-11-05T20:40:05Z");
        assert_eq!(restored.validator, validator());
        assert_eq!(restored.delay, std::time::Duration::from_secs(120));
        assert!(
            restored.next <= std::time::Instant::now(),
            "a restored pause must be due to probe at once"
        );
    }

    #[test]
    fn a_legacy_or_malformed_host_pause_cannot_resume_probing() {
        let work = temp("host-pause-legacy");
        std::fs::write(
            work.join("host-pause.json"),
            r#"{"reason":"wedged","since":"2026-11-05T20:40:05Z"}"#,
        )
        .unwrap();
        let error = match restored_host_pause(&work) {
            Ok(_) => panic!("a legacy pause must not restore"),
            Err(error) => error,
        };
        assert!(error.contains("no validator snapshot"), "{error}");
        assert!(error.contains("cannot resume probing"), "{error}");

        std::fs::write(work.join("host-pause.json"), "not json").unwrap();
        let error = match restored_host_pause(&work) {
            Ok(_) => panic!("a malformed pause must not restore"),
            Err(error) => error,
        };
        assert!(error.contains("malformed"), "{error}");
    }

    #[test]
    fn a_requeued_job_counts_as_started_and_summarizes_its_attempts() {
        let store = Store::new(&temp("attempt-summary"));
        let job = publish(&store, "c1");
        let fresh = store.read(&job.id).unwrap();
        assert!(!fresh.was_started(), "{fresh:?}");
        assert!(fresh
            .attempt_summary()
            .starts_with("0 attempt(s): 0 could-not-run, 0"));
        // Claim, refuse (could-not-run), claim again: queued again, but a
        // donor has started it twice, and the summary says so.
        for _ in 0..2 {
            let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
            let token = claimed.lease.as_ref().unwrap().token.clone();
            store
                .report(
                    &claimed.id,
                    &token,
                    refused_receipt(&claimed, "d1", "some output\nCOULD_NOT_RUN"),
                )
                .unwrap_err();
        }
        let after = store.read(&job.id).unwrap();
        assert_eq!(after.state, JobState::Queued);
        assert!(after.was_started(), "{after:?}");
        assert_eq!(
            after.attempt_summary(),
            "2 attempt(s): 2 could-not-run, 0 lease(s) expired; leased to d1",
            "{after:?}"
        );
        // The queue can write the summary onto the job's own record.
        store
            .note_history(&job.id, "cancelled as a stale root: 2 attempt(s)")
            .unwrap();
        assert!(store
            .read(&job.id)
            .unwrap()
            .history
            .join("\n")
            .contains("cancelled as a stale root: 2 attempt(s)"));
    }

    #[test]
    fn a_validation_unit_matches_its_job_by_segment_or_commit() {
        let job = "20261106T080445125-d4fb770cd67a-4fa58f2c";
        let commit = "d4fb770cd67a1234567890abcdef1234567890abcd";
        assert!(unit_matches_job(
            "validate-merge-queue-d4fb770cd67a-1793953926667353745.service",
            job,
            commit
        ));
        assert!(unit_matches_job(
            "validate-merge-queue-20261106T080445125-d4fb770cd67a-4fa58f2c.service",
            job,
            commit
        ));
        assert!(!unit_matches_job(
            "validate-merge-queue-0123456789ab-1793953926667353745.service",
            job,
            commit
        ));
        assert!(!unit_matches_job("other.service", job, commit));
        let units = parse_validation_units(
            "validate-merge-queue-abc123456789-x.service loaded active running job\n\
             validate-merge-queue-def123456789-y.service loaded inactive dead job\n\
             merge-queue-serve.service loaded active running serve\n",
        );
        assert_eq!(
            units,
            vec![
                (
                    "validate-merge-queue-abc123456789-x.service".to_owned(),
                    true
                ),
                (
                    "validate-merge-queue-def123456789-y.service".to_owned(),
                    false
                ),
            ]
        );
    }

    #[test]
    fn fault_probe_schedule_starts_at_sixty_seconds_and_caps_at_five_minutes() {
        let mut delay = std::time::Duration::from_secs(FAULT_PROBE_FIRST_SECS);
        for want in [120, 240, 300, 300] {
            delay = fault_probe_delay(delay);
            assert_eq!(delay, std::time::Duration::from_secs(want));
        }
        // A fault recorded from a failed probe counts probes, so it never
        // sits at zero probes: details name the last probe and its result.
        let store = Store::new(&temp("fault-probes"));
        store.record_host_fault("d1", "wedged");
        store.record_probe_result("d1", false);
        let details = store.host_fault_details();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].1, "wedged");
        assert_eq!(details[0].4, "failed");
        assert!(!details[0].3.is_empty(), "{details:?}");
        let faults: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(store.dir.join("host-faults.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(faults["d1"]["probes"], 2, "{faults}");
    }

    #[test]
    fn an_operator_fault_survives_probe_passes_until_it_is_cleared() {
        let store = Store::new(&temp("operator-fault"));
        publish(&store, "c1");
        store.record_operator_fault("d1", "host draining for maintenance");
        let details = store.host_fault_details();
        assert_eq!(details.len(), 1, "{details:?}");
        assert_eq!(details[0].1, "host draining for maintenance");
        assert!(details[0].5, "not marked operator: {details:?}");
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
        // Neither kind of probe pass clears it or arms a trial, and the
        // last probe is still recorded on the entry.
        store.note_probe_pass("d1");
        assert_eq!(store.host_fault_list().len(), 1);
        store.record_probe_result("d1", true);
        let details = store.host_fault_details();
        assert_eq!(details.len(), 1, "{details:?}");
        assert_eq!(details[0].4, "passed", "{details:?}");
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
        // An ordinary fault on another donor still clears as today.
        store.record_host_fault("d2", "wedged");
        store.note_probe_pass("d2");
        assert_eq!(store.host_fault_list().len(), 1);
        // Only an explicit clear lifts the operator hold.
        store.clear_host_fault("d1");
        assert!(store.host_fault_list().is_empty());
        assert!(store.claim("d1", "h1", 1).unwrap().is_some());
    }

    #[test]
    fn probe_backoff_doubles_to_ten_minutes_and_admission_resets_it() {
        let mut delay = std::time::Duration::from_secs(30);
        for want in [60, 120, 240, 480, 600, 600] {
            delay = doubled_probe_delay(delay);
            assert_eq!(delay, std::time::Duration::from_secs(want));
        }
    }

    #[test]
    fn unclassified_refusals_for_two_different_jobs_are_not_a_host_fault() {
        let store = Store::new(&temp("host-backstop"));
        let first = publish(&store, "c1");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(
                &claimed.id,
                &token,
                refused_receipt(&claimed, "d1", "some output\nwedge line"),
            )
            .unwrap_err();
        assert_eq!(store.read(&first.id).unwrap().could_not_run.len(), 1);
        let second = publish(&store, "c2");
        let mut held = None;
        for _ in 0..3 {
            if let Some(job) = store.claim("d1", "h1", 3).unwrap() {
                if job.id == second.id {
                    held = Some(job);
                    break;
                }
            }
        }
        let held = held.expect("d1 holds the second job");
        let token = held.lease.as_ref().unwrap().token.clone();
        store
            .report(
                &held.id,
                &token,
                refused_receipt(&held, "d1", "other\nwedge line"),
            )
            .unwrap_err();
        // Both refusals are counted, neither faults the host, and the donor
        // may lease again: only a failing probe faults a host.
        assert_eq!(store.read(&first.id).unwrap().could_not_run.len(), 1);
        assert_eq!(store.read(&second.id).unwrap().could_not_run.len(), 1);
        assert_eq!(store.read(&second.id).unwrap().state, JobState::Queued);
        assert!(store.host_fault_list().is_empty());
        assert!(store.claim("d1", "h1", 5).unwrap().is_some());
    }

    #[test]
    fn three_consecutive_give_backs_with_one_reason_alert_and_a_receipt_resets() {
        let store = Store::new(&temp("give-back-alert"));
        assert!(store.give_back_alerts(3).unwrap().is_empty());
        for commit in ["c1", "c2", "c3"] {
            publish(&store, commit);
            let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
            let token = claimed.lease.as_ref().unwrap().token.clone();
            store
                .release(&claimed.id, &token, "the revision differs", false)
                .unwrap();
        }
        let alerts = store.give_back_alerts(3).unwrap();
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert!(
            alerts[0].starts_with("ERROR: donor d1 gave back 3 consecutive jobs"),
            "{alerts:?}"
        );
        assert!(store.give_back_alerts(4).unwrap().is_empty());
        // An accepted receipt proves the donor can validate: the run resets.
        publish(&store, "c4");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .report(&claimed.id, &token, receipt(&claimed, "d1"))
            .unwrap();
        assert!(store.give_back_alerts(3).unwrap().is_empty());
        // A different donor's single give-back is not d1's run, and a new reason
        // starts a new run.
        publish(&store, "c5");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store
            .release(&claimed.id, &token, "no base", false)
            .unwrap();
        assert!(store.give_back_alerts(3).unwrap().is_empty());
    }

    #[test]
    fn a_stale_job_is_cancelled_by_the_sweep_and_never_claimed() {
        let store = Store::new(&temp("stale-sweep"));
        let stale = publish(&store, "c1");
        let fresh = publish(&store, "c2");
        let cancelled = store
            .sweep_stale("base99", &|commit| commit == "c2", false)
            .unwrap();
        assert_eq!(cancelled, vec![stale.id.clone()]);
        let job = store.read(&stale.id).unwrap();
        assert_eq!(job.state, JobState::Cancelled);
        assert!(
            job.history
                .join("\n")
                .contains("stale: the base moved to base99"),
            "{:?}",
            job.history
        );
        assert!(job.could_not_run.is_empty(), "stale is not a donor failure");
        // The sweep is idempotent, and the fresh job is still claimable.
        assert!(store
            .sweep_stale("base99", &|commit| commit == "c2", false)
            .unwrap()
            .is_empty());
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("the fresh job");
        assert_eq!(claimed.id, fresh.id);
    }

    #[test]
    fn a_rooted_job_leased_but_not_admitted_is_swept_and_an_admitted_one_is_kept() {
        let store = Store::new(&temp("stale-sweep-admission"));
        let unadmitted = publish_rooted(&store, "c1", true);
        let claimed = store
            .claim_reporting("d1", "h1", 1, true)
            .unwrap()
            .expect("a rooted job");
        assert_eq!(claimed.id, unadmitted.id);
        assert!(!store.read(&unadmitted.id).unwrap().was_admitted());
        let cancelled = store
            .sweep_stale("base99", &|commit| commit == "fresh", true)
            .unwrap();
        assert_eq!(cancelled, vec![unadmitted.id.clone()]);
        let job = store.read(&unadmitted.id).unwrap();
        assert_eq!(job.state, JobState::Cancelled);
        assert!(job.could_not_run.is_empty(), "stale is not a donor failure");
        assert!(
            job.history
                .iter()
                .any(|line| line.contains("stale_unadmitted_cancel")),
            "an unadmitted stale cancel carries the marker: {:?}",
            job.history
        );

        let admitted = publish_rooted(&store, "c2", true);
        let claimed = store
            .claim_reporting("d1", "h1", 1, true)
            .unwrap()
            .expect("a rooted job");
        assert_eq!(claimed.id, admitted.id);
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store.admit(&claimed.id, &token).unwrap();
        assert!(store.read(&admitted.id).unwrap().was_admitted());
        assert!(store
            .sweep_stale("base99", &|commit| commit == "fresh", true)
            .unwrap()
            .is_empty());
        assert_eq!(store.read(&admitted.id).unwrap().state, JobState::Leased);
        let _ = token;

        // An admitted job that is swept anyway (not rooted, so admission
        // does not protect it) was started: no marker.
        let started = publish_rooted(&store, "c3", false);
        let claimed = store
            .claim_reporting("d2", "h1", 1, true)
            .unwrap()
            .expect("an unrooted job");
        assert_eq!(claimed.id, started.id);
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store.admit(&claimed.id, &token).unwrap();
        let cancelled = store
            .sweep_stale("base99", &|commit| commit == "fresh", true)
            .unwrap();
        assert_eq!(cancelled, vec![started.id.clone()]);
        let job = store.read(&started.id).unwrap();
        assert!(
            !job.history
                .iter()
                .any(|line| line.contains("stale_unadmitted_cancel")),
            "an admitted stale cancel carries no marker: {:?}",
            job.history
        );
    }

    #[test]
    fn an_old_style_claims_leased_rooted_job_is_kept_as_today() {
        // A donor that never declares admission reporting (including
        // every lease recorded before the declaration existed) keeps
        // the old rule: its lease alone protects a rooted job.
        let store = Store::new(&temp("stale-sweep-legacy-claim"));
        let legacy = publish_rooted(&store, "c1", true);
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a rooted job");
        assert_eq!(claimed.id, legacy.id);
        assert!(store.read(&legacy.id).unwrap().stale_protected());
        assert!(store
            .sweep_stale("base99", &|commit| commit == "fresh", true)
            .unwrap()
            .is_empty());
        assert_eq!(store.read(&legacy.id).unwrap().state, JobState::Leased);
    }

    #[test]
    fn the_sweep_keeps_only_leased_rooted_jobs_under_keep_stale_roots() {
        // #915 merge-queue-keep-running-roots-only. A rooted job a donor holds
        // survives while the flag is on: its validation is paid for and its
        // verdict still decides the batch. A queued rooted job is swept like a
        // non-root stale job: no donor has started it, and on the old base it
        // cannot land as built. With the flag off a root is swept like anything
        // else.
        let store = Store::new(&temp("stale-sweep-roots"));
        let leased_root = publish_rooted(&store, "c1", true);
        let queued_root = publish_rooted(&store, "c2", true);
        let plain = publish(&store, "c3");
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("oldest job");
        assert_eq!(claimed.id, leased_root.id);
        let token = claimed.lease.as_ref().unwrap().token.clone();
        store.admit(&claimed.id, &token).unwrap();
        let cancelled = store
            .sweep_stale("base99", &|commit| commit == "fresh", true)
            .unwrap();
        assert_eq!(cancelled, vec![queued_root.id.clone(), plain.id.clone()]);
        assert_eq!(store.read(&leased_root.id).unwrap().state, JobState::Leased);
        for swept in [&queued_root, &plain] {
            let job = store.read(&swept.id).unwrap();
            assert_eq!(job.state, JobState::Cancelled);
            assert!(job.could_not_run.is_empty(), "stale is not a donor failure");
        }
        let cancelled = store
            .sweep_stale("base99", &|commit| commit == "fresh", false)
            .unwrap();
        assert_eq!(cancelled, vec![leased_root.id.clone()]);
        assert_eq!(
            store.read(&leased_root.id).unwrap().state,
            JobState::Cancelled
        );
    }

    #[test]
    fn a_cap_file_caps_jobs_drains_at_zero_and_is_ignored_when_absent_or_malformed() {
        // #918 donor-cap-file.
        let dir = temp("cap-file");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cap");
        assert_eq!(read_cap_file(&path), Ok(None), "no file");
        assert_eq!(effective_cap(4, &read_cap_file(&path)), 4);
        for (text, file, cap) in [
            ("2\n", Ok(Some(2)), 2),
            (" 0 ", Ok(Some(0)), 0),
            ("9", Ok(Some(9)), 4),
            ("two", Err("two".to_owned()), 4),
            ("-1", Err("-1".to_owned()), 4),
            ("", Err(String::new()), 4),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(read_cap_file(&path), file, "{text:?}");
            assert_eq!(effective_cap(4, &read_cap_file(&path)), cap, "{text:?}");
        }
    }

    #[test]
    fn the_queue_keeps_each_donors_last_cap_in_one_file() {
        // #918 donor-cap-file: one map in one fixed file, rewritten when a cap changes or
        // its last note is a minute old, never once per claim.
        let store = Store::new(&temp("donor-caps"));
        store.note_donor_cap("d1", 2, 1000).unwrap();
        store.note_donor_cap("d2", 0, 1000).unwrap();
        assert_eq!(
            store.donor_cap_list(),
            vec![("d1".to_owned(), 2, 1000), ("d2".to_owned(), 0, 1000)]
        );
        store.note_donor_cap("d1", 2, 1030).unwrap();
        assert_eq!(
            store.donor_cap_list()[0],
            ("d1".to_owned(), 2, 1000),
            "rewritten for nothing"
        );
        store
            .note_donor_cap("d1", 2, 1000 + DONOR_CAP_REFRESH_SECS)
            .unwrap();
        assert_eq!(
            store.donor_cap_list()[0].2,
            1000 + DONOR_CAP_REFRESH_SECS,
            "never seen again"
        );
        store.note_donor_cap("d1", 1, 1061).unwrap();
        assert_eq!(
            store.donor_cap_list()[0],
            ("d1".to_owned(), 1, 1061),
            "a change was not noted"
        );
        let files: Vec<String> = std::fs::read_dir(&store.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("cap"))
            .collect();
        assert_eq!(files, vec![DONOR_CAPS_FILE.to_owned()]);
    }

    #[test]
    fn compare_only_donors_claim_only_duplicates_and_never_decide() {
        let mut store = Store::new(&temp("compare-only"));
        store.compare_only = ["d2".to_owned()].into();
        let main = publish(&store, "c1");
        let dup = store.publish_compare_duplicate(&main).unwrap();
        assert!(dup.compare_only && !main.compare_only);
        // The compare-only donor gets only the duplicate; a deciding donor never gets it.
        let claimed_dup = store.claim("d2", "h2", 1).unwrap().expect("a duplicate");
        assert_eq!(claimed_dup.id, dup.id);
        assert!(store.claim("d2", "h2", 1).unwrap().is_none());
        let claimed_main = store
            .claim("d1", "h1", 1)
            .unwrap()
            .expect("the deciding job");
        assert_eq!(claimed_main.id, main.id);
        // The duplicate's receipt is accepted as a comparison, never the verdict.
        let dup_token = claimed_dup.lease.as_ref().unwrap().token.clone();
        store
            .report(&dup.id, &dup_token, receipt(&claimed_dup, "d2"))
            .unwrap();
        let dup_after = store.read(&dup.id).unwrap();
        assert_eq!(dup_after.state, JobState::Done);
        assert!(
            dup_after.history.join("\n").contains("compare-only"),
            "{:?}",
            dup_after.history
        );
        // The deciding receipt then records the comparison on both jobs.
        let main_token = claimed_main.lease.as_ref().unwrap().token.clone();
        store
            .report(&main.id, &main_token, receipt(&claimed_main, "d1"))
            .unwrap();
        assert!(store.record_comparison(&main.id, &dup.id).unwrap());
        assert_eq!(
            store.read(&main.id).unwrap().comparison.as_deref(),
            Some("match")
        );
        assert_eq!(
            store.read(&dup.id).unwrap().comparison.as_deref(),
            Some("match")
        );
    }

    #[test]
    fn a_job_published_before_a_tool_moved_is_re_stamped_and_offered_again() {
        let mut store = Store::new(&temp("restamp"));
        let stale = publish(&store, "c1");
        assert!(stale.tool.tools.is_empty());
        // A donor whose tools already moved (still different after its refresh) declines the
        // stale stamp, and is never offered the job again while the stamp stays.
        let first = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = first.lease.as_ref().unwrap().token.clone();
        store
            .release(&first.id, &token, "this donor's tools differ", true)
            .unwrap();
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
        // The queue host's tools moved too: the next claim re-stamps the queued job, and the
        // donor that declined it gets it.
        let moved: BTreeMap<String, String> = [("tooling".to_owned(), "v2".to_owned())].into();
        store.current_tools = Some(moved.clone());
        let again = store.claim("d1", "h1", 1).unwrap().expect("re-stamped job");
        assert_eq!(again.id, first.id);
        assert_eq!(again.tool.tools, moved);
        assert!(again.declined_by.is_empty(), "{:?}", again.declined_by);
        let text = again.history.join("\n");
        assert!(
            text.contains("re-stamped with the queue host's current tools"),
            "{text}"
        );
        assert!(text.contains("d1 may claim it again"), "{text}");
        // A leased job keeps the stamp it runs under, and matching answers change nothing.
        store.current_tools = Some([("tooling".to_owned(), "v3".to_owned())].into());
        assert!(store.claim("d2", "h2", 1).unwrap().is_none());
        let job = store.read(&first.id).unwrap();
        assert_eq!(job.tool.tools, moved, "a leased job is not re-stamped");
    }

    /// A git repo with two commits touching `ci-hub/tool` (v1 then v2) on top of
    /// an unrelated third commit; returns (repo, v1 commit, v1 tree, v2 commit,
    /// v2 tree, unrelated tree).
    fn tool_repo(tag: &str) -> (PathBuf, String, String, String, String, String) {
        let repo = temp(tag);
        let git = |args: &[&str]| git_in(&repo, args).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "dev@example.com"]);
        git(&["config", "user.name", "Dev"]);
        std::fs::create_dir_all(repo.join("ci-hub")).unwrap();
        std::fs::write(repo.join("ci-hub/tool"), "v1\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "v1"]);
        let c1 = git(&["rev-parse", "HEAD"]);
        let t1 = git(&["rev-parse", "HEAD:ci-hub"]);
        std::fs::write(repo.join("ci-hub/tool"), "v2\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "v2"]);
        let c2 = git(&["rev-parse", "HEAD"]);
        let t2 = git(&["rev-parse", "HEAD:ci-hub"]);
        std::fs::write(repo.join("ci-hub/tool"), "other\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "other"]);
        let unrelated = git(&["rev-parse", "HEAD:ci-hub"]);
        git(&["reset", "-q", "--hard", &c2]);
        (repo, c1, t1, c2, t2, unrelated)
    }

    fn git_validator() -> ValidatorConfig {
        let mut validator = validator();
        validator.tool_identity = [(
            "ci-hub".to_owned(),
            "git rev-parse origin/main:ci-hub".to_owned(),
        )]
        .into();
        validator
    }

    #[test]
    fn descendant_tools_prove_by_commit_and_by_tree() {
        let (repo, c1, t1, c2, t2, unrelated) = tool_repo("descendant");
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", &c2]).unwrap();
        let validator = git_validator();
        // Commit form: c2 descends from c1, not the other way round.
        assert!(tool_descends_from(&validator, "ci-hub", &c1, &c2, &repo).unwrap());
        assert!(!tool_descends_from(&validator, "ci-hub", &c2, &c1, &repo).unwrap());
        // Tree form: t2 descends from t1 through origin/main; unrelated proves nothing.
        assert!(tool_descends_from(&validator, "ci-hub", &t1, &t2, &repo).unwrap());
        assert!(!tool_descends_from(&validator, "ci-hub", &t1, &unrelated, &repo).unwrap());
        // Non-git answers and missing objects never prove.
        assert!(!tool_descends_from(&validator, "ci-hub", "1.90", "1.91", &repo).unwrap());
        assert!(!tool_descends_from(&validator, "ci-hub", &"f".repeat(40), &t2, &repo).unwrap());
    }

    #[test]
    fn a_tilde_tool_repo_proves_descent_and_a_relative_one_names_its_error() {
        let home = temp("tilde-home");
        let repo = home.join("work").join("tools");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "dev@example.com"]);
        git(&["config", "user.name", "Dev"]);
        std::fs::create_dir_all(repo.join("ci-hub")).unwrap();
        std::fs::write(repo.join("ci-hub/tool"), "v1\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "v1"]);
        let t1 = git(&["rev-parse", "HEAD:ci-hub"]);
        std::fs::write(repo.join("ci-hub/tool"), "v2\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "v2"]);
        let t2 = git(&["rev-parse", "HEAD:ci-hub"]);
        let c2 = git(&["rev-parse", "HEAD"]);
        git(&["update-ref", "refs/remotes/origin/main", &c2]);
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let mut validator = validator();
        validator.tool_identity = [(
            "ci-hub".to_owned(),
            "git -C ~/work/tools rev-parse origin/main:ci-hub".to_owned(),
        )]
        .into();
        // The proof runs in $HOME/work/tools, not in <base>/~/work/tools.
        let proved = tool_descends_from(&validator, "ci-hub", &t1, &t2, Path::new("/nonexistent"));
        validator.tool_identity = [(
            "ci-hub".to_owned(),
            "git -C tools rev-parse origin/main:ci-hub".to_owned(),
        )]
        .into();
        let relative =
            tool_descends_from(&validator, "ci-hub", &t1, &t2, Path::new("/nonexistent"));
        match saved {
            Some(saved) => std::env::set_var("HOME", saved),
            None => std::env::remove_var("HOME"),
        }
        assert!(proved.unwrap(), "a ~/ -C proves descent");
        let error = relative.unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("\"tools\""), "{error}");
    }

    #[test]
    fn a_receipt_is_accepted_after_the_tip_moves_past_the_donors_answer() {
        let (repo, _c1, t1, _c2, t2, unrelated) = tool_repo("receipt-moved-tip");
        // Three more commits on top of the donor's answer, none touching ci-hub:
        // the queue's current answer is the tip's tree, the donor's is behind it.
        for step in 1..=3 {
            std::fs::write(repo.join("other.txt"), format!("step {step}\n")).unwrap();
            git_in(&repo, &["add", "."]).unwrap();
            git_in(&repo, &["commit", "-q", "-m", &format!("step {step}")]).unwrap();
        }
        let tip = git_in(&repo, &["rev-parse", "HEAD"]).unwrap();
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", &tip]).unwrap();
        let validator = git_validator();
        assert!(tool_descends_from(&validator, "ci-hub", &t1, &t2, &repo).unwrap());
        assert!(!tool_descends_from(&validator, "ci-hub", &t1, &unrelated, &repo).unwrap());
        let mut store = Store::new(&temp("receipt-moved-tip-store"));
        store.tool_repo = Some(repo.clone());
        let tool = ToolIdentity {
            validator_digest: validator_digest(&validator).unwrap(),
            tools: [("ci-hub".to_owned(), t1.clone())].into(),
            merge_queue: Some(crate::build_info::Revision {
                commit: "a".repeat(40),
                dirty: false,
            }),
        };
        let job = store
            .publish(
                "cand",
                "tree0",
                "base0",
                "base0",
                &Selection::of([TestId::step("a")]),
                &BTreeSet::new(),
                &validator,
                &tool,
                false,
                false,
                None,
            )
            .unwrap();
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let mut bad = receipt(&claimed, "d1");
        bad.tool.tools.insert("ci-hub".to_owned(), unrelated);
        let error = store.report(&job.id, &token, bad).unwrap_err();
        assert!(error.contains("descends"), "{error}");
        let mut good = receipt(&claimed, "d1");
        good.tool.tools.insert("ci-hub".to_owned(), t2);
        store.report(&job.id, &token, good).unwrap();
        assert_eq!(store.read(&job.id).unwrap().state, JobState::Done);
    }

    #[test]
    fn a_receipt_with_a_descendant_tool_is_accepted_and_an_unrelated_one_refused() {
        let (repo, _c1, t1, _c2, t2, unrelated) = tool_repo("receipt-tools");
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", &_c2]).unwrap();
        let mut store = Store::new(&temp("receipt-tools-store"));
        store.tool_repo = Some(repo.clone());
        let validator = git_validator();
        let tool = ToolIdentity {
            validator_digest: validator_digest(&validator).unwrap(),
            tools: [("ci-hub".to_owned(), t1.clone())].into(),
            merge_queue: Some(crate::build_info::Revision {
                commit: "a".repeat(40),
                dirty: false,
            }),
        };
        let job = store
            .publish(
                "cand",
                "tree0",
                "base0",
                "base0",
                &Selection::of([TestId::step("a")]),
                &BTreeSet::new(),
                &validator,
                &tool,
                false,
                false,
                None,
            )
            .unwrap();
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        // Unrelated tree: refused, job stays leased.
        let mut bad = receipt(&claimed, "d1");
        bad.tool.tools.insert("ci-hub".to_owned(), unrelated);
        let error = store.report(&job.id, &token, bad).unwrap_err();
        assert!(error.contains("descends"), "{error}");
        assert_eq!(store.read(&job.id).unwrap().state, JobState::Leased);
        // Descendant tree: accepted, history names the descendant tool.
        let mut good = receipt(&claimed, "d1");
        good.tool.tools.insert("ci-hub".to_owned(), t2);
        store.report(&job.id, &token, good).unwrap();
        let done = store.read(&job.id).unwrap();
        assert_eq!(done.state, JobState::Done);
        assert!(
            done.history
                .join("\n")
                .contains("accepted with descendant tool ci-hub"),
            "{:?}",
            done.history
        );
    }

    #[test]
    fn a_receipt_whose_validator_ran_another_tree_is_refused_as_identity_mismatch() {
        // #1045: the pre-run answers are fresh, but the validator attests from
        // inside the run that it loaded something else (or nothing): the queue
        // must refuse, however fresh the pre-run answers look.
        let (repo, _c1, t1, c2, t2, unrelated) = tool_repo("receipt-ran-tools");
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", &c2]).unwrap();
        let validator = git_validator();
        let mut strict = git_validator();
        strict.require_tool_identity_lines = true;
        let publish_git = |store: &Store, validator: &ValidatorConfig, stamp: &str| {
            let tool = ToolIdentity {
                validator_digest: validator_digest(validator).unwrap(),
                tools: [("ci-hub".to_owned(), stamp.to_owned())].into(),
                merge_queue: Some(crate::build_info::Revision {
                    commit: "a".repeat(40),
                    dirty: false,
                }),
            };
            store
                .publish(
                    "cand",
                    "tree0",
                    "base0",
                    "base0",
                    &Selection::of([TestId::step("a")]),
                    &BTreeSet::new(),
                    validator,
                    &tool,
                    false,
                    false,
                    None,
                )
                .unwrap()
        };
        // Default (not strict, #1049): a missing attestation is accepted but
        // recorded, never silent — an upgrading consumer whose validator does
        // not print the lines yet must keep working.
        let mut store = Store::new(&temp("receipt-unattested-store"));
        store.tool_repo = Some(repo.clone());
        let job = publish_git(&store, &validator, &t1);
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let mut missing = receipt(&claimed, "d1");
        missing.ran_tools.clear();
        store.report(&job.id, &token, missing).unwrap();
        let done = store.read(&job.id).unwrap();
        assert_eq!(done.state, JobState::Done, "{done:?}");
        assert_eq!(done.tool_identity_unattested.len(), 1, "{done:?}");
        assert!(
            done.tool_identity_unattested[0].contains("ci-hub"),
            "{done:?}"
        );
        assert!(
            done.history.join("\n").contains("tool_identity_unattested"),
            "{:?}",
            done.history
        );
        // Stamp t1 (the older tree) under strict: missing, unrelated and extra
        // attestations refuse; a descendant attestation accepts.
        let mut store = Store::new(&temp("receipt-ran-tools-store"));
        store.tool_repo = Some(repo.clone());
        let job = publish_git(&store, &strict, &t1);
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let mut missing = receipt(&claimed, "d1");
        missing.ran_tools.clear();
        let error = store.report(&job.id, &token, missing).unwrap_err();
        assert!(error.contains("identity-mismatch"), "{error}");
        let mut wrong = receipt(&claimed, "d1");
        wrong.ran_tools = [("ci-hub".to_owned(), vec![unrelated.clone()])].into();
        let error = store.report(&job.id, &token, wrong).unwrap_err();
        assert!(error.contains("identity-mismatch"), "{error}");
        let mut extra = receipt(&claimed, "d1");
        extra
            .ran_tools
            .insert("surprise".to_owned(), vec![t1.clone()]);
        let error = store.report(&job.id, &token, extra).unwrap_err();
        assert!(error.contains("identity-mismatch"), "{error}");
        assert_eq!(store.read(&job.id).unwrap().state, JobState::Leased);
        let mut descendant = receipt(&claimed, "d1");
        descendant.ran_tools = [("ci-hub".to_owned(), vec![t2.clone()])].into();
        store.report(&job.id, &token, descendant).unwrap();
        assert_eq!(store.read(&job.id).unwrap().state, JobState::Done);
        // Stamp t2 (fresh): the devbig176 shape — validator ran the stale
        // ancestor t1. And a refused receipt (validation never ran) owes no
        // attestation at all.
        let mut store = Store::new(&temp("receipt-ran-stale-store"));
        store.tool_repo = Some(repo);
        store.could_not_run_limit = 99;
        let job = publish_git(&store, &validator, &t2);
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = claimed.lease.as_ref().unwrap().token.clone();
        let mut stale = receipt(&claimed, "d1");
        stale.ran_tools = [("ci-hub".to_owned(), vec![t1])].into();
        let error = store.report(&job.id, &token, stale).unwrap_err();
        assert!(error.contains("identity-mismatch"), "{error}");
        assert_eq!(store.read(&job.id).unwrap().state, JobState::Leased);
        let mut refused = receipt(&claimed, "d1");
        refused.refused = true;
        refused.exit = Some(75);
        refused.ran_tools.clear();
        let error = store.report(&job.id, &token, refused).unwrap_err();
        assert!(!error.contains("identity-mismatch"), "{error}");
    }

    #[test]
    fn unattested_warnings_name_only_missing_full_hex_tools() {
        // #1049: the donor-side receipt warning mirrors the queue's rule —
        // full-hex stamps with no printed line warn; version stamps and
        // attested tools do not.
        let stamped: BTreeMap<String, String> = [
            ("ci-hub".to_owned(), "a".repeat(40)),
            ("toolchain".to_owned(), "1.90".to_owned()),
            ("other".to_owned(), "b".repeat(40)),
        ]
        .into();
        let printed: BTreeMap<String, Vec<String>> =
            [("other".to_owned(), vec!["b".repeat(40)])].into();
        let warnings = unattested_warnings(&stamped, &printed);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("tool_identity_unattested"),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("ci-hub"), "{warnings:?}");
        // The opt-in key stays out of serialized configs until enabled, so an
        // existing consumer's validator digest cannot move under it.
        let text = serde_json::to_string(&git_validator()).unwrap();
        assert!(!text.contains("require_tool_identity_lines"), "{text}");
    }

    /// Publish a job with claim order and a controlled id (age), returning it.
    fn publish_ordered(
        store: &Store,
        commit: &str,
        prioritized: bool,
        plan_index: Option<usize>,
        id: &str,
    ) -> Job {
        let validator = validator();
        let tool = ToolIdentity {
            validator_digest: validator_digest(&validator).unwrap(),
            tools: BTreeMap::new(),
            merge_queue: Some(crate::build_info::Revision {
                commit: "a".repeat(40),
                dirty: false,
            }),
        };
        let mut job = store
            .publish(
                commit,
                "tree0",
                "base0",
                "base0",
                &Selection::of([TestId::step("a")]),
                &BTreeSet::new(),
                &validator,
                &tool,
                false,
                prioritized,
                plan_index,
            )
            .unwrap();
        let old_path = store.job_path(&job.id).unwrap();
        job.id = id.to_owned();
        store.write(&job).unwrap();
        std::fs::remove_file(old_path).unwrap();
        job
    }

    #[test]
    fn until_idle_exits_only_when_no_claim_failed() {
        // No claim failure, nothing running, not draining: exit.
        assert!(until_idle_exits(true, false, 0, 1));
        // A failed claim is not emptiness: stay and retry.
        assert!(!until_idle_exits(true, true, 0, 1));
        // Work still running: stay.
        assert!(!until_idle_exits(true, false, 1, 1));
        // Draining (cap 0): idle by choice, not for want of work: stay.
        assert!(!until_idle_exits(true, false, 0, 0));
        // Not an --until-idle donor at all: never exits on idleness.
        assert!(!until_idle_exits(false, false, 0, 1));
    }

    #[test]
    fn claims_follow_priority_then_plan_order_then_age() {
        let store = Store::new(&temp("claim-order"));
        // Published in age order 1, 2, 3; plan order and priority disagree.
        let first_by_age = publish_ordered(
            &store,
            "c1",
            false,
            Some(1),
            "20260101T000000000-c11111111111-00000001",
        );
        let second_by_age = publish_ordered(
            &store,
            "c2",
            false,
            Some(0),
            "20260101T000000000-c22222222222-00000002",
        );
        let prioritized = publish_ordered(
            &store,
            "c3",
            true,
            Some(2),
            "20260101T000000000-c33333333333-00000003",
        );
        // A prioritized job published last is claimed first; then plan order
        // beats age.
        let claim = |store: &Store| store.claim("d1", "h1", 10).unwrap().expect("a job").id;
        assert_eq!(claim(&store), prioritized.id);
        assert_eq!(claim(&store), second_by_age.id);
        assert_eq!(claim(&store), first_by_age.id);
    }

    #[test]
    fn claims_without_recorded_order_keep_age_order() {
        let store = Store::new(&temp("claim-order-old"));
        // Job files written before claim order was recorded deserialize with
        // no priority and no plan index.
        let made = publish_ordered(
            &store,
            "c1",
            false,
            None,
            "20260101T000000000-c11111111111-00000001",
        );
        let mut value = serde_json::to_value(&made).unwrap();
        value.as_object_mut().unwrap().remove("prioritized");
        value.as_object_mut().unwrap().remove("plan_index");
        let old: Job = serde_json::from_value(value).unwrap();
        assert!(!old.prioritized && old.plan_index.is_none());
        let newer = publish_ordered(
            &store,
            "c2",
            false,
            None,
            "20260101T000000000-c22222222222-00000002",
        );
        // Equal (absent) priority keeps oldest-first.
        let claim = |store: &Store| store.claim("d1", "h1", 10).unwrap().expect("a job").id;
        assert_eq!(claim(&store), made.id);
        assert_eq!(claim(&store), newer.id);
    }

    #[test]
    fn an_index_from_a_stale_plan_does_not_leapfrog_an_older_unindexed_job() {
        // #1196 front-starve dp2, replayed: positions 1-4 where position 4
        // (v15) carries index 3 matched to the run's first plan and
        // position 1 (v10) carries none. Once the serve order advances a
        // generation and restamps v10 at position 0, the donor claims v10
        // first; v15's stale index no longer counts.
        let store = Store::new(&temp("claim-order-stale-plan"));
        let v10 = publish_ordered(
            &store,
            "c1",
            false,
            None,
            "20260101T000000000-c11111111111-00000001",
        );
        let v15 = publish_ordered(
            &store,
            "c2",
            false,
            Some(3),
            "20260101T000000000-c22222222222-00000002",
        );
        store
            .set_plan_generation(1, &[(v10.id.clone(), Some(0))])
            .unwrap();
        let claim = |store: &Store| store.claim("d1", "h1", 10).unwrap().expect("a job").id;
        assert_eq!(claim(&store), v10.id);
        assert_eq!(claim(&store), v15.id);
        let v10_after = store.read(&v10.id).unwrap();
        assert_eq!(v10_after.plan_generation, 1);
        assert_eq!(v10_after.plan_index, Some(0));
    }

    #[test]
    fn a_queue_refresh_attempt_is_rate_limited() {
        let store = Store::new(&temp("queue-refresh-rate"));
        assert!(store.queue_refresh_due(), "never attempted: due");
        store.note_queue_refresh_attempt();
        assert!(!store.queue_refresh_due(), "just attempted: not due");
        assert!(store.queue_refresh_attempted_at() > 0);
    }

    #[test]
    fn a_mismatch_decline_from_a_donor_built_before_releases_is_recorded_as_a_release() {
        let store = Store::new(&temp("legacy-mismatch"));
        let job = publish(&store, "c1");
        let first = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = first.lease.as_ref().unwrap().token.clone();
        let reason = format!(
            "{LEGACY_MISMATCH_DECLINE}this donor's merge-queue is built from a, the queue \
             host's from b"
        );
        store.release(&first.id, &token, &reason, true).unwrap();
        // Once that donor is upgraded it takes the job: it is offered it again.
        let again = store.claim("d1", "h1", 1).unwrap().expect("offered again");
        assert_eq!(again.id, job.id);
        assert!(again.declined_by.is_empty(), "{:?}", again.declined_by);
        let text = again.history.join("\n");
        assert!(
            text.contains("released by d1 (declined for a mismatch"),
            "{text}"
        );
    }

    #[test]
    fn one_lease_per_job_and_per_donor_limit() {
        let store = Store::new(&temp("lease"));
        publish(&store, "c1");
        publish(&store, "c2");
        let first = store.claim("d1", "h1", 1).unwrap().expect("a job");
        // d1 holds its limit; d2 gets the other job, and nobody gets the first again.
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
        let second = store.claim("d2", "h2", 4).unwrap().expect("a job");
        assert_ne!(first.id, second.id);
        assert!(store.claim("d3", "h3", 4).unwrap().is_none());
    }

    #[test]
    fn an_expired_lease_is_queued_again_and_its_holder_is_refused() {
        let mut store = Store::new(&temp("expire"));
        store.lease_secs = 0;
        publish(&store, "c1");
        let first = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = first.lease.as_ref().unwrap().token.clone();
        store.lease_secs = DEFAULT_LEASE_SECS;
        let second = store.claim("d2", "h2", 1).unwrap().expect("re-queued");
        assert_eq!(first.id, second.id);
        assert_eq!(second.attempts, 2);
        assert!(matches!(
            store.renew(&first.id, &token).unwrap(),
            Renewal::Stop { .. }
        ));
        assert!(store
            .report(&first.id, &token, receipt(&first, "d1"))
            .is_err());
        let token2 = second.lease.as_ref().unwrap().token.clone();
        store
            .report(&second.id, &token2, receipt(&second, "d2"))
            .unwrap();
        assert_eq!(store.get(&first.id).unwrap().state, JobState::Done);
    }

    #[test]
    fn a_receipt_for_another_commit_tree_tool_or_donor_is_refused() {
        let store = Store::new(&temp("refuse"));
        publish(&store, "c1");
        let job = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = job.lease.as_ref().unwrap().token.clone();
        let mut wrong = receipt(&job, "d1");
        wrong.commit = "c9".to_owned();
        assert!(store
            .report(&job.id, &token, wrong)
            .unwrap_err()
            .contains("not the job's commit"));
        let mut wrong = receipt(&job, "d1");
        wrong.tree = "t9".to_owned();
        assert!(store
            .report(&job.id, &token, wrong)
            .unwrap_err()
            .contains("tree"));
        let mut wrong = receipt(&job, "d1");
        wrong
            .tool
            .tools
            .insert("toolchain".to_owned(), "other".to_owned());
        assert!(store
            .report(&job.id, &token, wrong)
            .unwrap_err()
            .contains("tool identity"));
        let wrong = receipt(&job, "d2");
        assert!(store
            .report(&job.id, &token, wrong)
            .unwrap_err()
            .contains("lease is d1's"));
        assert!(store
            .report(&job.id, "0".repeat(64).as_str(), receipt(&job, "d1"))
            .unwrap_err()
            .contains("not leased with that token"));
        // Every refusal is on the job's record, and the right receipt is still accepted.
        assert_eq!(
            store
                .get(&job.id)
                .unwrap()
                .history
                .iter()
                .filter(|line| line.contains("refused a receipt"))
                .count(),
            5
        );
        store.report(&job.id, &token, receipt(&job, "d1")).unwrap();
    }

    #[test]
    fn a_cancelled_job_stops_its_donor() {
        let store = Store::new(&temp("cancel"));
        publish(&store, "c1");
        let job = store.claim("d1", "h1", 1).unwrap().expect("a job");
        let token = job.lease.as_ref().unwrap().token.clone();
        store.cancel(&job.id, "candidate discarded").unwrap();
        assert!(matches!(
            store.renew(&job.id, &token).unwrap(),
            Renewal::Stop { reason } if reason.contains("cancelled")
        ));
        assert!(store.claim("d2", "h2", 1).unwrap().is_none());
    }

    #[test]
    fn revisions_match_only_when_equal_and_usable() {
        use crate::build_info::Revision;
        let at = |commit: char, dirty: bool| Revision {
            commit: commit.to_string().repeat(40),
            dirty,
        };
        let same = at('a', false);
        assert!(revision_mismatch(Some(&same), Some(&same.clone())).is_none());
        let other = revision_mismatch(Some(&at('a', false)), Some(&at('b', false))).unwrap();
        assert!(
            other.contains(&"b".repeat(40)) && other.contains(&"a".repeat(40)),
            "{other}"
        );
        // The queue host recorded nothing (an older queue, or a binary built outside Git).
        let none = revision_mismatch(None, Some(&same)).unwrap();
        assert!(none.contains("no recorded revision"), "{none}");
        // A donor that recorded nothing is refused whatever the queue host has.
        let missing = revision_mismatch(Some(&same), None).unwrap();
        assert!(missing.contains("no source revision"), "{missing}");
        // A dirty donor never matches a clean queue host.
        assert!(revision_mismatch(Some(&same), Some(&at('a', true))).is_some());
    }

    #[test]
    fn a_claim_is_refused_only_for_a_declared_mismatched_revision() {
        use crate::build_info::Revision;
        let at = |commit: char, dirty: bool| Revision {
            commit: commit.to_string().repeat(40),
            dirty,
        };
        let queue = at('a', false);
        // An old donor declares nothing: no refusal, whatever the queue has.
        assert!(claim_refusal(Some(&queue), None).is_none());
        assert!(claim_refusal(None, None).is_none());
        // A matching declaration claims as today.
        assert!(claim_refusal(Some(&queue), Some(&at('a', false))).is_none());
        // A mismatch refuses with the receipt path's wording.
        let refused = claim_refusal(Some(&queue), Some(&at('b', false))).unwrap();
        assert!(
            refused.contains("this donor's merge-queue is built from"),
            "{refused}"
        );
        // So does a dirty (unusable) declaration.
        let refused = claim_refusal(Some(&queue), Some(&at('a', true))).unwrap();
        assert!(refused.contains("uncommitted changes"), "{refused}");
    }

    #[test]
    fn a_retired_job_is_compared_only_by_its_leased_donor() {
        let dir = std::env::temp_dir().join(format!("mq-retire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::new(&dir);
        store.lease_secs = 1;
        // A queued job is cancelled outright.
        let queued = publish(&store, &"1".repeat(40));
        assert!(store.retire(&queued.id, "its candidate landed").unwrap());
        assert_eq!(store.get(&queued.id).unwrap().state, JobState::Cancelled);
        // A leased one stays with its donor; when the lease ends it is cancelled, not queued.
        let leased = publish(&store, &"2".repeat(40));
        let claimed = store.claim("d1", "h1", 1).unwrap().unwrap();
        assert_eq!(claimed.id, leased.id);
        assert!(store.retire(&leased.id, "its candidate landed").unwrap());
        let job = store.get(&leased.id).unwrap();
        assert_eq!(job.state, JobState::Leased);
        assert_eq!(job.candidate_gone.as_deref(), Some("its candidate landed"));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert_eq!(store.get(&leased.id).unwrap().state, JobState::Cancelled);
        assert!(store.claim("d2", "h2", 1).unwrap().is_none());
        // A retired job its donor gives back is cancelled too.
        store.lease_secs = 60;
        let released = publish(&store, &"3".repeat(40));
        let token = store
            .claim("d1", "h1", 1)
            .unwrap()
            .unwrap()
            .lease
            .unwrap()
            .token;
        store.retire(&released.id, "superseded").unwrap();
        store
            .release(&released.id, &token, "giving back", false)
            .unwrap();
        assert_eq!(store.get(&released.id).unwrap().state, JobState::Cancelled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repository_keys_ignore_url_form_but_not_the_repository() {
        for (a, b) in [
            ("https://github.com/o/r.git", "git@github.com:o/r"),
            ("ssh://git@GitHub.com:22/o/r/", "https://github.com/o/r"),
            ("git+ssh://me@host/o/r.git", "host:o/r"),
            ("/srv/r.git", "file:///srv/r/"),
        ] {
            assert!(same_repository(a, b), "{a} vs {b}");
        }
        for (a, b) in [
            ("https://github.com/o/r", "https://github.com/o/other"),
            ("https://github.com/o/r", "https://gitlab.com/o/r"),
            ("/srv/r", "/srv/other"),
        ] {
            assert!(!same_repository(a, b), "{a} vs {b}");
        }
    }
}
