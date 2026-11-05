//! Intake that never waits for a run. `submit`, `withdraw`, `retry` and `accept-flaky` change
//! item records, which needs the writer lock, and a run holds that lock for its whole length.
//! When the lock is busy they record the request under `<state>/intake/pending/` instead, one
//! file each, and the next holder of the lock applies the requests in the order they were
//! recorded before it reads the queue: the `serve` daemon at its next cycle, a `run`, a `sync`,
//! or the next intake command that finds the lock free. A request that cannot be applied then
//! (its change landed meanwhile, its head no longer resolves) is kept under `intake/refused/`
//! with the reason, and `status` shows it.
//!
//! A run whose speculative window has room does not wait for its own end: it applies the
//! requests for changes it has not started, and plans them on top of the batches in flight
//! (see `run`). Requests for a change it started wait for the run to end.
//!
//! A change whose submission is still waiting can be withdrawn too: the withdrawal is recorded
//! behind the submission and applied right after it, so a bad head can be stopped while a run
//! holds the lock.
//!
//! `list` and `status` show the waiting requests as `pending-intake`, so a submitter can see its
//! request was taken. A request identical to the last one waiting for the same change is not
//! recorded again (the command says it is already recorded), and identical requests recorded
//! one after another (by two submitters at once, say) are applied once.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::model::{now, ItemStatus};
use crate::ops::{self, Submission};
use crate::plan::short;
use crate::state::{write_json, Config, Lock, State};

/// [`Entry::schema`].
pub const INTAKE_SCHEMA: &str = "merge-queue-intake/v1";

/// Days a refused request is kept for `status`.
const REFUSED_KEEP_DAYS: i64 = 7;

/// Refused requests `status` names, newest first.
const REFUSED_SHOWN: usize = 3;

/// What was asked for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    /// `merge-queue submit`.
    Submit(Box<Submission>),
    /// `merge-queue withdraw`.
    Withdraw {
        /// Change number.
        pr: i64,
        /// Recorded in the change's history.
        reason: String,
    },
    /// `merge-queue retry` of one change.
    Retry {
        /// Change number.
        pr: i64,
    },
    /// `merge-queue accept-flaky` of one held batch.
    AcceptFlaky {
        /// The batch's change numbers.
        prs: Vec<i64>,
        /// Who gave the ruling.
        by: String,
        /// Why.
        reason: String,
    },
}

impl Request {
    /// The change the request is about (for a batch, its first change).
    pub fn pr(&self) -> i64 {
        match self {
            Self::Submit(submission) => submission.pr,
            Self::Withdraw { pr, .. } | Self::Retry { pr } => *pr,
            Self::AcceptFlaky { prs, .. } => prs.first().copied().unwrap_or_default(),
        }
    }

    /// `submit #N`, `withdraw #N`, `retry #N` or `accept-flaky #N, #M`.
    pub fn describe(&self) -> String {
        let verb = match self {
            Self::Submit(_) => "submit",
            Self::Withdraw { .. } => "withdraw",
            Self::Retry { .. } => "retry",
            Self::AcceptFlaky { prs, .. } => {
                return format!(
                    "accept-flaky {}",
                    prs.iter()
                        .map(|pr| format!("#{pr}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        };
        format!("{verb} #{}", self.pr())
    }
}

/// One recorded request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// [`INTAKE_SCHEMA`].
    pub schema: String,
    /// Identifier and file name; identifiers sort in recording order.
    pub id: String,
    /// RFC 3339 UTC time the request was recorded.
    pub recorded_at: String,
    /// Why the writer lock could not be taken then.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub busy: String,
    /// For a submission, the commit its head ref resolved to when it was recorded (shown while
    /// it waits; the submission itself still takes the ref's head when it is applied).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub seen_head: String,
    /// The request.
    pub request: Request,
    /// RFC 3339 UTC time it was refused, once it was.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refused_at: String,
    /// Why it was refused.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// What applying one request did.
#[derive(Clone, Debug)]
pub struct Applied {
    /// `submit #N` and the like (or the file name, when the record is unreadable).
    pub request: String,
    /// When it was recorded.
    pub recorded_at: String,
    /// What it did, or why it was refused.
    pub outcome: Result<String, String>,
}

impl Applied {
    /// One line for a log.
    pub fn line(&self) -> String {
        match &self.outcome {
            Ok(done) => format!(
                "applied {} recorded at {}: {done}",
                self.request, self.recorded_at
            ),
            Err(error) => format!(
                "refused {} recorded at {}: {error}",
                self.request, self.recorded_at
            ),
        }
    }
}

/// [`Pending::status`]: how `list` shows a change that has only a waiting request.
pub const PENDING_INTAKE: &str = "pending-intake";

/// A waiting request, as `list` and `status` show it. Identical requests for the same change
/// recorded one after another (a submitter repeating itself) count as one, with their number.
#[derive(Clone, Debug, Serialize)]
pub struct Pending {
    /// Change number.
    pub pr: i64,
    /// Always [`PENDING_INTAKE`], so a `list --json` reader tells these from item records.
    pub status: &'static str,
    /// `submit`, `withdraw`, `retry` or `accept-flaky`.
    pub op: String,
    /// For a submission, its head ref.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub head_ref: String,
    /// For a submission, the head it names, or else the commit its ref resolved to when it was
    /// recorded (the submission takes the ref's head at the time it is applied).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub head: String,
    /// When the first of these requests was recorded.
    pub recorded_at: String,
    /// When the last of them was recorded.
    pub last_recorded_at: String,
    /// How many identical requests were recorded in a row.
    pub requests: usize,
}

impl Pending {
    /// `#N pending-intake: submit REF at abc1234567 since T`, for a text listing.
    pub fn line(&self) -> String {
        let mut what = self.op.clone();
        if !self.head_ref.is_empty() {
            what = format!("{what} {}", self.head_ref);
        }
        if !self.head.is_empty() {
            what = format!("{what} at {}", short(&self.head));
        }
        let repeats = if self.requests > 1 {
            format!(
                " ({} identical requests, the last at {})",
                self.requests, self.last_recorded_at
            )
        } else {
            String::new()
        };
        format!(
            "#{} {PENDING_INTAKE}: {what} since {}{repeats}",
            self.pr, self.recorded_at
        )
    }
}

/// Requests waiting for the lock and recent refusals, for `status`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    /// Requests not yet applied.
    pub pending: usize,
    /// When the oldest of them was recorded.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub oldest_pending: String,
    /// The waiting requests by change, in recording order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<Pending>,
    /// Refused requests kept (the last week).
    pub refused: usize,
    /// The newest refused requests, newest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_refusals: Vec<Entry>,
}

fn pending_dir(state: &State) -> PathBuf {
    state.root.join("intake").join("pending")
}

fn refused_dir(state: &State) -> PathBuf {
    state.root.join("intake").join("refused")
}

/// The `.json` files in `dir`, sorted by name (recording order); none when it does not exist.
fn files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list {}: {error}", dir.display())),
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    Ok(files)
}

fn read(path: &Path) -> Result<Entry, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("unreadable {}: {error}", path.display()))
}

/// Whether two requests ask for the same thing (the recording time is not part of a request).
fn same(a: &Request, b: &Request) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// The waiting entries in recording order (unreadable ones left out).
fn pending_entries(state: &State) -> Vec<Entry> {
    files(&pending_dir(state))
        .unwrap_or_default()
        .iter()
        .filter_map(|path| read(path).ok())
        .collect()
}

/// Record `request` for the next holder of the writer lock; `busy` says why it could not be
/// taken now, and `seen_head` is what a submission's ref resolved to. Returns the entry and
/// whether it is new: when the last waiting request for the same change is identical, nothing
/// is recorded again and that request is returned instead.
pub fn record(
    state: &State,
    request: Request,
    busy: &str,
    seen_head: &str,
) -> Result<(Entry, bool), String> {
    if let Some(last) = pending_entries(state)
        .into_iter()
        .rfind(|entry| entry.request.pr() == request.pr())
    {
        if same(&last.request, &request) {
            return Ok((last, false));
        }
    }
    // Microseconds, then the process and a per-process count: unique, and in recording order.
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let id = format!(
        "{}-{}-{:04}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%6fZ"),
        std::process::id(),
        COUNT.fetch_add(1, Ordering::SeqCst)
    );
    let entry = Entry {
        schema: INTAKE_SCHEMA.to_owned(),
        id: id.clone(),
        recorded_at: now(),
        busy: busy.to_owned(),
        seen_head: seen_head.to_owned(),
        request,
        refused_at: String::new(),
        error: String::new(),
    };
    write_json(&pending_dir(state).join(format!("{id}.json")), &entry)?;
    Ok((entry, true))
}

/// The last submission of `pr` still waiting for the writer lock, if any. A change whose only
/// record is a waiting submission has no item record yet.
pub fn waiting_submission(state: &State, pr: i64) -> Option<Entry> {
    pending_entries(state)
        .into_iter()
        .rfind(|entry| matches!(&entry.request, Request::Submit(submission) if submission.pr == pr))
}

/// Whether any request is waiting (a directory listing; no lock).
pub fn has_pending(state: &State) -> bool {
    files(&pending_dir(state)).is_ok_and(|files| !files.is_empty())
}

/// Changes a waiting request withdraws. A run reads this before it lands anything: a change
/// withdrawn while its batch validated is not landed.
pub fn pending_withdrawals(state: &State) -> BTreeSet<i64> {
    files(&pending_dir(state))
        .unwrap_or_default()
        .iter()
        .filter_map(|path| read(path).ok())
        .filter_map(|entry| match entry.request {
            Request::Withdraw { pr, .. } => Some(pr),
            _ => None,
        })
        .collect()
}

/// Whether a request is waiting for a change not in `busy`.
pub fn has_pending_except(state: &State, busy: &BTreeSet<i64>) -> bool {
    pending_entries(state)
        .iter()
        .any(|entry| !busy.contains(&entry.request.pr()))
}

/// Apply every waiting request, oldest first, under the writer lock the caller holds. A request
/// that cannot be applied is moved to `intake/refused/` with the reason (and noted in its
/// change's history); the rest still apply. Errs only when the requests cannot be listed.
pub fn apply(state: &State, config: &Config, lock: &Lock) -> Result<Vec<Applied>, String> {
    apply_except(state, config, lock, &BTreeSet::new())
}

/// [`apply`], except that the requests for a change in `busy` (one a run in progress has
/// started) stay waiting, in order, for a later apply.
pub fn apply_except(
    state: &State,
    config: &Config,
    _lock: &Lock,
    busy: &BTreeSet<i64>,
) -> Result<Vec<Applied>, String> {
    let mut applied: Vec<Applied> = Vec::new();
    // For each change, the last request applied, its line in `applied`, and how many identical
    // requests recorded right after it were folded into it rather than applied again.
    let mut last: std::collections::BTreeMap<i64, (Request, usize, usize)> =
        std::collections::BTreeMap::new();
    for path in files(&pending_dir(state))? {
        let entry = match read(&path) {
            Ok(entry) => entry,
            Err(error) => {
                // Never let one unreadable record hold up the rest: keep it, aside.
                let name = path.file_name().unwrap_or_default().to_owned();
                let kept = refused_dir(state).join(&name);
                let _ = std::fs::create_dir_all(refused_dir(state));
                std::fs::rename(&path, &kept)
                    .map_err(|e| format!("cannot move {} aside: {e}", path.display()))?;
                applied.push(Applied {
                    request: name.to_string_lossy().into_owned(),
                    recorded_at: String::new(),
                    outcome: Err(error),
                });
                continue;
            }
        };
        let pr = entry.request.pr();
        if busy.contains(&pr) {
            continue;
        }
        if let Some((previous, line, repeats)) = last.get_mut(&pr) {
            if same(previous, &entry.request) {
                std::fs::remove_file(&path)
                    .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
                *repeats += 1;
                applied[*line].request = format!(
                    "{} (and {} identical request(s) recorded after it, the last at {})",
                    previous.describe(),
                    repeats,
                    entry.recorded_at
                );
                continue;
            }
        }
        let outcome = perform(state, config, &entry);
        if let Err(error) = &outcome {
            refuse(state, &entry, error)?;
        }
        std::fs::remove_file(&path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        last.insert(pr, (entry.request.clone(), applied.len(), 0));
        applied.push(Applied {
            request: entry.request.describe(),
            recorded_at: entry.recorded_at.clone(),
            outcome,
        });
    }
    prune_refused(state);
    Ok(applied)
}

/// Carry out one request.
fn perform(state: &State, config: &Config, entry: &Entry) -> Result<String, String> {
    let recorded = &entry.recorded_at;
    match &entry.request {
        Request::Submit(submission) => {
            let mut submission = (**submission).clone();
            submission.recorded_at.clone_from(recorded);
            let item = ops::submit(state, config, submission)?;
            Ok(format!("queued #{} at {}", item.pr, short(&item.head)))
        }
        Request::Withdraw { pr, reason } => {
            let item = state
                .item(*pr)?
                .ok_or_else(|| format!("#{pr} is not in the queue"))?;
            if item.status == ItemStatus::Landed {
                return Err(format!(
                    "#{pr} landed (as {}) before the withdrawal could be applied",
                    short(&item.landed_as)
                ));
            }
            ops::withdraw(
                state,
                *pr,
                &format!("{reason} (recorded at {recorded} while another writer held the queue)"),
            )?;
            Ok(format!("withdrew #{pr}"))
        }
        Request::Retry { pr } => {
            ops::retry_recorded(state, *pr, recorded)?;
            Ok(format!("#{pr} is back in the queue"))
        }
        Request::AcceptFlaky { prs, by, reason } => {
            let held = ops::accept_flaky(state, prs, by, reason, recorded)?;
            Ok(format!(
                "accepted the {} passes of {} at {}; a run lands that commit if the base is \
                 still {}",
                crate::validate::FLAKY_MARKER,
                entry.request.describe().trim_start_matches("accept-flaky "),
                short(&held.tip),
                short(&held.parent)
            ))
        }
    }
}

/// Keep a refused request under `intake/refused/`, and say so in its change's history.
fn refuse(state: &State, entry: &Entry, error: &str) -> Result<(), String> {
    let mut kept = entry.clone();
    kept.refused_at = now();
    kept.error = error.to_owned();
    write_json(
        &refused_dir(state).join(format!("{}.json", entry.id)),
        &kept,
    )?;
    if let Ok(Some(mut item)) = state.item(entry.request.pr()) {
        item.log(
            "intake-refused",
            format!(
                "{} recorded at {} while another writer held the queue was not applied: {error}",
                entry.request.describe(),
                entry.recorded_at
            ),
        );
        state.save_item(&item)?;
    }
    Ok(())
}

/// Forget refused requests older than [`REFUSED_KEEP_DAYS`].
fn prune_refused(state: &State) {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(REFUSED_KEEP_DAYS);
    for path in files(&refused_dir(state)).unwrap_or_default() {
        let old = read(&path).ok().and_then(|entry| {
            chrono::DateTime::parse_from_rfc3339(&entry.refused_at)
                .ok()
                .map(|at| at < cutoff)
        });
        if old == Some(true) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// The waiting requests in recording order, a request identical to the one recorded just before
/// it for the same change counted as a repeat of that one (no lock).
pub fn pending(state: &State) -> Vec<Pending> {
    let mut out: Vec<(Request, Pending)> = Vec::new();
    for entry in pending_entries(state) {
        let pr = entry.request.pr();
        if let Some((request, pending)) =
            out.iter_mut().rev().find(|(request, _)| request.pr() == pr)
        {
            if same(request, &entry.request) {
                pending.requests += 1;
                pending.last_recorded_at.clone_from(&entry.recorded_at);
                continue;
            }
        }
        let (op, head_ref, head) = match &entry.request {
            Request::Submit(submission) => (
                "submit",
                submission.head_ref.clone(),
                if submission.head.is_empty() {
                    entry.seen_head.clone()
                } else {
                    submission.head.clone()
                },
            ),
            Request::Withdraw { .. } => ("withdraw", String::new(), String::new()),
            Request::Retry { .. } => ("retry", String::new(), String::new()),
            Request::AcceptFlaky { .. } => ("accept-flaky", String::new(), String::new()),
        };
        out.push((
            entry.request.clone(),
            Pending {
                pr,
                status: PENDING_INTAKE,
                op: op.to_owned(),
                head_ref,
                head,
                recorded_at: entry.recorded_at.clone(),
                last_recorded_at: entry.recorded_at.clone(),
                requests: 1,
            },
        ));
    }
    out.into_iter().map(|(_, pending)| pending).collect()
}

/// Waiting and refused requests, for `status` (no lock).
pub fn summary(state: &State) -> Summary {
    let pending_files = files(&pending_dir(state)).unwrap_or_default();
    let refused = files(&refused_dir(state)).unwrap_or_default();
    Summary {
        pending: pending_files.len(),
        oldest_pending: pending_files
            .first()
            .and_then(|path| read(path).ok())
            .map(|entry| entry.recorded_at)
            .unwrap_or_default(),
        requests: pending(state),
        refused: refused.len(),
        recent_refusals: refused
            .iter()
            .rev()
            .filter_map(|path| read(path).ok())
            .take(REFUSED_SHOWN)
            .collect(),
    }
}

/// Print what applying the waiting requests did, on stderr (`prefix` names the command). A
/// refused request does not fail the command that applied it.
pub fn report(prefix: &str, applied: Result<Vec<Applied>, String>) {
    match applied {
        Ok(applied) => {
            for one in applied {
                eprintln!("{prefix}: {}", one.line());
            }
        }
        Err(error) => eprintln!("{prefix}: recorded requests not applied: {error}"),
    }
}
