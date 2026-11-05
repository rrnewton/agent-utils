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
    /// The accepted receipt, once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
    /// Claims so far (a re-queue after an expired or released lease adds one).
    #[serde(default)]
    pub attempts: u32,
    /// What happened to the job, oldest first. Never holds a lease token.
    #[serde(default)]
    pub history: Vec<String>,
    /// Donors that declined the job (tools still different after a refresh, no base, bundle
    /// too big); a claim never hands it to them again unless the job is re-stamped.
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
    /// A duplicate published for compare-only donors (`donors.compare_only`): only they may
    /// claim it, and its receipt is a comparison, never the verdict.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compare_only: bool,
}

impl Job {
    /// A comparison recorded after the queue had stopped wanting the candidate (a donor slower
    /// than the queue host).
    pub fn compared_after_landing(&self) -> bool {
        self.comparison.is_some() && self.candidate_gone.is_some()
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
    format!(
        "mismatch: local {}{}, donor {}{}",
        word(local),
        failed(local),
        word(donor),
        failed(donor)
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

/// Bring the tools `names` up to date on this host: run each one's `validator.tool_refresh`
/// command in `cwd`, bounded by [`REFRESH_TIMEOUT`]. Fails on a tool with no such command, or a
/// command that fails or runs out of time.
pub fn refresh_tools(
    validator: &ValidatorConfig,
    cwd: &Path,
    names: &[String],
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
            if started.elapsed() > REFRESH_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the refresh of {name:?} ran out of time ({}s)",
                    REFRESH_TIMEOUT.as_secs()
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
        selection: &Selection,
        extra: &BTreeSet<TestId>,
        validator: &ValidatorConfig,
        tool: &ToolIdentity,
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
            selection: selection.clone(),
            extra: extra.clone(),
            validator: validator.clone(),
            tool: tool.clone(),
            created_at: now(),
            state: JobState::Queued,
            lease: None,
            receipt: None,
            attempts: 0,
            history: Vec::new(),
            declined_by: Vec::new(),
            local: None,
            comparison: None,
            candidate_gone: None,
            could_not_run: Vec::new(),
            compare_only: false,
        };
        self.write(&job)?;
        Ok(job)
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
    pub fn claim(&self, donor: &str, host: &str, max_jobs: usize) -> Result<Option<Job>, String> {
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
        let Some(mut job) = jobs.into_iter().find(|job| {
            job.state == JobState::Queued
                && job.compare_only == compare_only_donor
                && !job.declined_by.iter().any(|name| name == donor)
        }) else {
            return Ok(None);
        };
        job.state = JobState::Leased;
        job.attempts += 1;
        job.lease = Some(Lease {
            donor: donor.to_owned(),
            host: host.to_owned(),
            token: token()?,
            claimed_unix: at,
            expires_unix: at + self.lease_secs,
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
        });
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
        } else if receipt.tool != job.tool {
            Some(format!(
                "the receipt's tool identity {:?} is not the job's {:?}",
                receipt.tool, job.tool
            ))
        } else {
            None
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
            let last = receipt
                .log_tail
                .lines()
                .map(str::trim)
                .rfind(|line| !line.is_empty())
                .unwrap_or("");
            let detail = format!(
                "exit {}{}",
                receipt
                    .exit
                    .map_or_else(|| "none".to_owned(), |code| code.to_string()),
                if last.is_empty() {
                    String::new()
                } else {
                    format!(", last output: {last}")
                }
            );
            // Host scope: the host cannot validate at all. No strike, no bound, no
            // verdict: the job goes back and the donor is leased nothing until it
            // reports an admitted validation or claims after a passing probe.
            if let Some((true, reason)) = crate::validate::could_not_run_scope(&receipt.log_tail) {
                job.lease = None;
                job.state = if job.candidate_gone.is_some() {
                    JobState::Cancelled
                } else {
                    JobState::Queued
                };
                job.history.push(format!(
                    "{}: host could not run: {reason} ({} on {}); no verdict, no strike",
                    now(),
                    receipt.donor.name,
                    receipt.donor.host
                ));
                self.note_host_fault(&receipt.donor.name, &reason);
                let fault_reason = reason.clone();
                self.write(&job)?;
                return Err(format!(
                    "host could not run ({fault_reason}); no verdict, no strike, \
                     the donor is paused"
                ));
            }
            // Backstop for validators that never print the scope line: could-not-run
            // for two different jobs in a row with the same last line is a host
            // fault. Both strikes come off and the donor is paused as above.
            let marked_job_scope = crate::validate::could_not_run_scope(&receipt.log_tail)
                .is_some_and(|(host, _)| !host);
            if !marked_job_scope {
                let donor_key = receipt.donor.name.clone();
                let mut pending: std::collections::BTreeMap<String, serde_json::Value> =
                    std::fs::read_to_string(self.dir.join("could-not-run-pending.json"))
                        .ok()
                        .and_then(|text| serde_json::from_str(&text).ok())
                        .unwrap_or_default();
                let (pending_job, pending_line) = pending
                    .get(&donor_key)
                    .map(|entry| {
                        (
                            entry["job"].as_str().unwrap_or_default().to_owned(),
                            entry["line"].as_str().unwrap_or_default().to_owned(),
                        )
                    })
                    .unwrap_or_default();
                if !pending_job.is_empty() && pending_job != job.id && pending_line == last {
                    if let Ok(mut previous) = self.read(&pending_job) {
                        previous
                            .could_not_run
                            .retain(|entry| !entry.starts_with(&format!("{donor_key}: ")));
                        previous.history.push(format!(
                            "{}: host could not run (backstop: same refusal as {}); \
                             strikes removed",
                            now(),
                            job.id
                        ));
                        let _ = self.write(&previous);
                    }
                    pending.remove(&donor_key);
                    if let Ok(text) = serde_json::to_string(&pending) {
                        let _ = write_private(
                            &self.dir.join("could-not-run-pending.json"),
                            text.as_bytes(),
                        );
                    }
                    job.lease = None;
                    job.state = if job.candidate_gone.is_some() {
                        JobState::Cancelled
                    } else {
                        JobState::Queued
                    };
                    job.history.push(format!(
                        "{}: host could not run (backstop: {} on {} refused two \
                         different jobs in a row: {last}); no verdict, no strike",
                        now(),
                        receipt.donor.name,
                        receipt.donor.host
                    ));
                    self.note_host_fault(
                        &receipt.donor.name,
                        &format!("unclassified host refusal: {last}"),
                    );
                    self.write(&job)?;
                    return Err(
                        "host could not run (backstop: two different jobs refused in a \
                         row with the same last line); no verdict, no strike, the donor \
                         is paused"
                            .to_owned(),
                    );
                }
                pending.insert(donor_key, serde_json::json!({"job": job.id, "line": last}));
                if let Ok(text) = serde_json::to_string(&pending) {
                    let _ = write_private(
                        &self.dir.join("could-not-run-pending.json"),
                        text.as_bytes(),
                    );
                }
            }
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
            job.history.push(note);
            self.write(&job)?;
            return Err(format!(
                "the receipt says validation could not run ({detail}); no verdict, the job is \
                 queued again"
            ));
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
        self.clear_host_fault(&receipt.donor.name.clone());
        job.receipt = Some(receipt);
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
        job.state = JobState::Cancelled;
        job.lease = None;
        job.history.push(format!("{}: cancelled: {reason}", now()));
        self.write(&job)?;
        Ok(true)
    }

    /// Cancel every live job whose candidate does not contain `base` (the queue's current
    /// base): it can never land as published, so offering it again only burns a donor's
    /// setup time on a stale-base refusal. A stale job is not a donor failure: no
    /// could-not-run strike is recorded. `contains(commit)` answers whether that candidate
    /// contains `base`. Returns the cancelled ids.
    pub fn sweep_stale(
        &self,
        base: &str,
        contains: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<String>, String> {
        let reason = format!(
            "stale: the base moved to {}, which its candidate does not contain",
            crate::plan::short(base)
        );
        let stale: Vec<String> = self
            .jobs()?
            .into_iter()
            .filter(|job| matches!(job.state, JobState::Queued | JobState::Leased))
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

    /// Host faults by donor name, for status and the serve heartbeat.
    pub fn host_fault_list(&self) -> Vec<(String, String, String)> {
        self.host_faults()
            .into_iter()
            .map(|(donor, fault)| {
                (
                    donor,
                    fault["reason"].as_str().unwrap_or_default().to_owned(),
                    fault["since"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
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
    /// validation that alone clears it.
    pub fn note_probe_pass(&self, donor: &str) {
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
        let entry = faults
            .entry(donor.to_owned())
            .or_insert_with(|| serde_json::json!({"reason": reason, "since": now()}));
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
}

/// The probe back-off after a pause whose current delay is `delay`: doubled,
/// capped at ten minutes.
fn doubled_probe_delay(delay: std::time::Duration) -> std::time::Duration {
    (delay * 2).min(std::time::Duration::from_secs(600))
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
        let results = self.options.work.join("probe-results.jsonl");
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(&cfg.run)
            .current_dir(&self.options.clone)
            .env(crate::validate::PROBE_ENV, "1")
            .env("MQ_RESULTS", &results)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let Ok(mut child) = command.spawn() else {
            return false;
        };
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if started.elapsed() <= std::time::Duration::from_secs(60) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    return false;
                }
            }
        }
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
            Err(Unreported::Lost(reason)) => eprintln!(
                "merge-queue donor {}: stopped {}: {reason}",
                self.options.name, job.id
            ),
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
            // An older donor catches up and runs the job; only one that still differs after
            // that (newer than the job, or unrelated) declines it.
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
                self.back_off(backoff);
                return Err(Unreported::Declined(format!(
                    "this donor's tools still differ from the job's after refreshing them \
                     ({after}), so this donor is newer than the job or unrelated to it"
                )));
            }
            eprintln!(
                "merge-queue donor {}: refreshed {} for {}; its tools now match ({before} before)",
                self.options.name,
                differing.join(", "),
                job.id
            );
        }
        let digest = validator_digest(&job.validator).map_err(Unreported::Declined)?;
        if digest != job.tool.validator_digest {
            return Err(Unreported::Declined(format!(
                "validator settings digest {digest} is not the job's {}",
                job.tool.validator_digest
            )));
        }
        self.fetch(job, token)?;
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
            let started_at = now();
            let env: Vec<(String, String)> = if job.extra.is_empty() {
                Vec::new()
            } else {
                vec![(
                    crate::validate::EXTRA_TESTS_ENV.to_owned(),
                    job.extra
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                )]
            };
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
                merge_queue_version: env!("CARGO_PKG_VERSION").to_owned(),
                effective_selection: ran.effective_selection.clone(),
                exit: ran.exit,
                refused: ran.refused,
                results: ran.results.clone(),
                elapsed_ms: ran.elapsed.as_millis() as u64,
                cpu_ms: ran.cpu.map(|cpu| cpu.as_millis() as u64),
                started_at,
                finished_at: now(),
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
                // A host-scope refusal pauses claiming entirely: probe the validator
                // (MQ_PROBE=1) until it admits work again.
                if let Some((true, reason)) = crate::validate::could_not_run_scope(&log_text) {
                    let since = now();
                    eprintln!(
                        "merge-queue donor {}: host cannot validate: {reason};                          pausing claims and probing",
                        self.options.name
                    );
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
                    std::time::Duration::from_secs(30);
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
    let restored_delay = restored_pause
        .as_ref()
        .map_or(std::time::Duration::from_secs(30), |pause| pause.delay);
    let worker = Worker {
        options,
        route: &route,
        host: host_name(),
        slots: Mutex::new((0..jobs).rev().collect()),
        host_pause: Mutex::new(restored_pause),
        probe_passed: AtomicBool::new(false),
        next_host_delay: Mutex::new(restored_delay),
    };
    let backoff: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let active = AtomicUsize::new(0);
    let jobs_text = jobs.to_string();
    eprintln!(
        "merge-queue donor {}: on {}, up to {jobs} job(s), clone {}, merge-queue {}",
        options.name,
        worker.host,
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
                            worker.probe_passed.store(true, Ordering::SeqCst);
                            false
                        } else {
                            state.delay = doubled_probe_delay(state.delay);
                            state.next = std::time::Instant::now() + state.delay;
                            true
                        }
                    }
                }
            };
            let mut claimed = false;
            if !host_paused && !backing_off && active.load(Ordering::SeqCst) < jobs {
                let probe_passed = worker.probe_passed.swap(false, Ordering::SeqCst);
                let mut args: Vec<&str> = vec![
                    "donor",
                    "claim",
                    "--donor",
                    &options.name,
                    "--host",
                    &worker.host,
                    "--jobs",
                    &jobs_text,
                ];
                if probe_passed {
                    args.push("--probe-passed");
                }
                let answer = route.call(&args, b"", ROUTE_TIMEOUT);
                match answer.and_then(|out| Route::parse_reply::<Claimed>(&out)) {
                    Ok(Claimed { job: Some(job) }) => {
                        claimed = true;
                        active.fetch_add(1, Ordering::SeqCst);
                        eprintln!(
                            "merge-queue donor {}: took {} ({})",
                            options.name, job.id, job.commit
                        );
                        let (worker, backoff, active) = (&worker, &backoff, &active);
                        scope.spawn(move || {
                            worker.run(job, backoff);
                            active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Ok(Claimed { job: None }) => {}
                    Err(error) => {
                        eprintln!("merge-queue donor {}: claim failed: {error}", options.name)
                    }
                }
            }
            if options.until_idle && !claimed && active.load(Ordering::SeqCst) == 0 {
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
                &Selection::of([TestId::step("a")]),
                &BTreeSet::new(),
                &validator,
                &tool,
            )
            .unwrap()
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

    fn refused_receipt(job: &Job, donor: &str, log_tail: &str) -> Receipt {
        let mut receipt = receipt(job, donor);
        receipt.refused = true;
        receipt.exit = Some(75);
        receipt.results = crate::tests::TestResults::default();
        receipt.log_tail = log_tail.to_owned();
        receipt
    }

    #[test]
    fn host_scope_refusals_cost_no_strike_and_pause_the_donor() {
        let store = Store::new(&temp("host-scope"));
        for _ in 0..3 {
            // Each round the donor talks its way back in (a probe pass, say) and
            // refuses host-wide again: never a strike, always a pause.
            store.clear_host_fault("d1");
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
            assert!(error.contains("host could not run"), "{error}");
            let after = store.read(&claimed.id).unwrap();
            assert_eq!(after.state, JobState::Queued, "{after:?}");
            assert!(after.could_not_run.is_empty(), "{after:?}");
            assert!(
                after
                    .history
                    .join("\n")
                    .contains("host could not run: wrkslots wedged"),
                "{after:?}"
            );
            // Give the lease back for the next round without a claim (the donor is
            // now paused queue-side); simulate by cancelling the lease record.
            let mut job = store.read(&claimed.id).unwrap();
            job.state = JobState::Cancelled;
            store.write(&job).unwrap();
        }
        // The faulted donor is leased nothing; a probe-passed claim clears it.
        publish(&store, "c2");
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
        store.clear_host_fault("d1");
        assert!(store.claim("d1", "h1", 1).unwrap().is_some());
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
        let waiting = store.read(&claimed.id).unwrap();
        assert!(!store.every_eligible_donor_faulted(&waiting));
    }

    #[test]
    fn probe_pass_launch_refuse_cycles_escalate_until_an_admitted_validation() {
        let mut store = Store::new(&temp("host-flap"));
        store.host_fault_escalate_after = 3;
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
    fn probe_backoff_doubles_to_ten_minutes_and_admission_resets_it() {
        let mut delay = std::time::Duration::from_secs(30);
        for want in [60, 120, 240, 480, 600, 600] {
            delay = doubled_probe_delay(delay);
            assert_eq!(delay, std::time::Duration::from_secs(want));
        }
    }

    #[test]
    fn unclassified_refusals_for_two_different_jobs_become_a_host_fault() {
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
        // The queue re-offers the first job to others; d1 takes a different one and
        // refuses with the same last line: both strikes come off.
        let second = publish(&store, "c2");
        let donor_job = store
            .jobs()
            .unwrap()
            .into_iter()
            .find(|job| job.id == second.id)
            .unwrap();
        let _ = donor_job;
        // Claim as d1 repeatedly until it holds the second job (the first may come first).
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
        let error = store
            .report(
                &held.id,
                &token,
                refused_receipt(&held, "d1", "other\nwedge line"),
            )
            .unwrap_err();
        assert!(error.contains("backstop"), "{error}");
        assert!(store.read(&first.id).unwrap().could_not_run.is_empty());
        assert!(store.read(&second.id).unwrap().could_not_run.is_empty());
        assert_eq!(store.read(&second.id).unwrap().state, JobState::Queued);
        assert_eq!(store.host_fault_list().len(), 1);
        assert!(store.claim("d1", "h1", 1).unwrap().is_none());
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
            .sweep_stale("base99", &|commit| commit == "c2")
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
            .sweep_stale("base99", &|commit| commit == "c2")
            .unwrap()
            .is_empty());
        let claimed = store.claim("d1", "h1", 1).unwrap().expect("the fresh job");
        assert_eq!(claimed.id, fresh.id);
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
