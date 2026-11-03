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

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::state::ValidatorConfig;
use crate::tests::{Selection, TestResults};

/// Schema of a job record.
pub const JOB_SCHEMA: &str = "merge-queue-donor-job/v1";
/// Schema of a receipt.
pub const RECEIPT_SCHEMA: &str = "merge-queue-receipt/v1";
/// Seconds a lease lasts without a renewal.
pub const DEFAULT_LEASE_SECS: u64 = 120;

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
    /// Donors that declined the job (wrong tools, no base, bundle too big); a claim never
    /// hands it to them again.
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
        };
        self.write(&job)?;
        Ok(job)
    }

    /// Lease the oldest queued job to `donor`, unless it already holds `max_jobs` leases.
    /// The returned job carries the lease token; nothing else ever returns it.
    pub fn claim(&self, donor: &str, host: &str, max_jobs: usize) -> Result<Option<Job>, String> {
        if donor.trim().is_empty() {
            return Err("a donor needs a name".to_owned());
        }
        let _lock = self.lock()?;
        let at = unix_now();
        self.expire(at)?;
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
        let Some(mut job) = jobs.into_iter().find(|job| {
            job.state == JobState::Queued && !job.declined_by.iter().any(|name| name == donor)
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
        job.history.push(format!(
            "{}: {} by {donor}: {reason}",
            now(),
            if decline { "declined" } else { "released" }
        ));
        if decline && !job.declined_by.contains(&donor) {
            job.declined_by.push(donor);
        }
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
        job.state = JobState::Done;
        job.lease = None;
        job.history.push(format!(
            "{}: receipt from {} on {} accepted{}",
            now(),
            receipt.donor.name,
            receipt.donor.host,
            if job.candidate_gone.is_some() {
                " after the queue had stopped wanting the candidate (compared only)"
            } else {
                ""
            }
        ));
        if let Some(local) = &job.local {
            job.comparison = Some(compare(local, &receipt.verdict()));
        }
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

    /// Retire every queued or leased job with `reason` (compare mode); returns their ids.
    pub fn retire_all_live(&self, reason: &str) -> Result<Vec<String>, String> {
        let live: Vec<String> = self
            .jobs()?
            .into_iter()
            .filter(|job| matches!(job.state, JobState::Queued | JobState::Leased))
            .map(|job| job.id)
            .collect();
        let mut retired = Vec::new();
        for id in live {
            if self.retire(&id, reason)? {
                retired.push(id);
            }
        }
        Ok(retired)
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
            let text = String::from_utf8_lossy(&err);
            let tail: Vec<&str> = text.lines().rev().take(3).collect();
            Err(format!(
                "queue route {} exited {}: {}",
                args.first().copied().unwrap_or(""),
                status.code().unwrap_or(-1),
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
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
    /// Seconds to stop claiming after declining a job for a tool mismatch.
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
    /// The donor cannot run it here; it declined (with the reason) and never gets it again.
    Declined(String),
    /// The lease is gone or the job was cancelled; nothing to report.
    Lost(String),
    /// The donor is stopping; the job was given back at once.
    Released(String),
}

struct Worker<'a> {
    options: &'a DonorOptions,
    route: &'a Route,
    host: String,
    slots: Mutex<Vec<usize>>,
}

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

impl Worker<'_> {
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
                        .and_then(|out| {
                            serde_json::from_slice::<Renewal>(&out).map_err(|e| e.to_string())
                        });
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
        let have =
            tool_answers(&job.validator, &self.options.clone).map_err(Unreported::Declined)?;
        let difference = revision_mismatch(job.tool.merge_queue.as_ref(), revision.as_ref())
            .or_else(|| tool_mismatch(&job.tool.tools, &have));
        if let Some(difference) = difference {
            *backoff
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(std::time::Instant::now() + self.options.mismatch_backoff);
            return Err(Unreported::Declined(format!(
                "this donor's tools differ from the queue host's: {difference}"
            )));
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
            let ran = validator
                .run(&checkout, &job.selection, &out, cancel)
                .map_err(Unreported::Declined)?;
            if let Some(why) = &ran.interrupted {
                return Err(Unreported::Released(format!("donor stopping: {why}")));
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
                started_at,
                finished_at: now(),
                log_tail: log_text[tail_start..].to_owned(),
                log_path: ran.log.display().to_string(),
            };
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
    crate::interrupt::install();
    let jobs = options.jobs.max(1);
    let worker = Worker {
        options,
        route: &route,
        host: host_name(),
        slots: Mutex::new((0..jobs).rev().collect()),
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
            let mut claimed = false;
            if !backing_off && active.load(Ordering::SeqCst) < jobs {
                let answer = route.call(
                    &[
                        "donor",
                        "claim",
                        "--donor",
                        &options.name,
                        "--host",
                        &worker.host,
                        "--jobs",
                        &jobs_text,
                    ],
                    b"",
                    ROUTE_TIMEOUT,
                );
                match answer.and_then(|out| {
                    serde_json::from_slice::<Claimed>(&out).map_err(|e| e.to_string())
                }) {
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
}
