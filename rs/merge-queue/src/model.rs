//! Queue vocabulary: submitted changes, plans, and widening obligations.

use serde::{Deserialize, Serialize};

use crate::tests::{TestResult, TestResults, ValidationClaim};

/// Lifecycle of a submitted change.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    /// Waiting to be planned and landed.
    #[default]
    Queued,
    /// Landed on the base branch.
    Landed,
    /// Removed from the queue by the queue, with a report saying why.
    Ejected,
    /// Withdrawn by its submitter.
    Withdrawn,
}

impl ItemStatus {
    /// Stable machine-facing value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Landed => "landed",
            Self::Ejected => "ejected",
            Self::Withdrawn => "withdrawn",
        }
    }
}

/// Delivery record for the one notification owed for an item's current
/// terminal or deferred transition. Persisted on the item so daemon restarts
/// never resend a transition that was already delivered (or already gave up).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotifyDelivery {
    /// Transition key already attempted (`verdict:head:discriminator`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Send attempts made for `key` (bounded; see `notify::MAX_ATTEMPTS`).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub attempts: u32,
    /// Whether `key` was delivered successfully.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delivered: bool,
    /// Most recent send failure for `key`, for the operator.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_error: String,
}

impl NotifyDelivery {
    /// Whether nothing has ever been attempted.
    pub fn is_empty(&self) -> bool {
        self.key.is_empty() && self.attempts == 0 && !self.delivered && self.last_error.is_empty()
    }
}

/// One entry in an item's history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// RFC 3339 UTC time.
    pub at: String,
    /// Short kind such as `submitted`, `ejected`, `landed`.
    pub kind: String,
    /// Human detail.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// A commit in a peer repository that must be on that repository's base before the requiring
/// change may be validated or land.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    /// Peer name (a key of `peers` in the configuration).
    pub peer: String,
    /// Exact commit required on the peer's base.
    pub commit: String,
    /// The peer change that carries it, when given as `PEER#PR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<i64>,
}

/// A change submitted to the queue (one pull request or branch).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Change number (pull-request number); the item's identity.
    pub pr: i64,
    /// Head source: a branch name on the remote, or a full ref such as `refs/pull/7/head`.
    pub head_ref: String,
    /// Exact head commit submitted. The queue refuses to land any other head.
    pub head: String,
    /// Lower lands first.
    pub priority: i64,
    /// RFC 3339 UTC submission time; earlier lands first among equal priorities.
    pub submitted_at: String,
    /// Who submitted it (agent or person).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub submitted_by: String,
    /// agentctl session name to notify on a state change (landed, ejected,
    /// refusal-deferred, withdrawn); empty means no notification. Stored next
    /// to `submitted_by`; a resubmission without a name keeps the old one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notify: String,
    /// Notification delivery for the current transition (dedup + retry bound).
    #[serde(default, skip_serializing_if = "NotifyDelivery::is_empty")]
    pub notify_delivery: NotifyDelivery,
    /// One-line description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Changes that must land first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<i64>,
    /// Validate and land this change in a batch of its own (a risky change).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    /// A priority change (the configured priority label, or `submit --prioritized`): planned
    /// ahead of all other work and validated in a batch of its own, so a red result can neither
    /// hide behind nor eject unrelated changes. It still lands only on its own exact green.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prioritized: bool,
    /// Land this change as its own commits (a fast-forward), never replayed, because another
    /// repository pins one of them. Implies a batch of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
    /// Peer commits that must be on their repositories' bases first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Requirement>,
    /// The submitter's own validation claim: which tests they ran, and how they chose them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ValidationClaim>,
    /// Where the submission came from: empty for `submit`, or the `sync` source that added it
    /// (only that source withdraws it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Landed, but closing the pull request is still owed (the request budget ran out).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close_pending: bool,
    /// How many times validation admission refused to run this change (retry-later). The
    /// change stays `queued` across refusals; this count drives the backoff.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub validation_refusals: u32,
    /// RFC 3339 UTC time before which validation should not be retried after a refusal
    /// (empty when not deferred).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_retry_at: String,
    /// Detail of the most recent refusal (exit status and marker), for the operator.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_refusal: String,
    /// Why the change was escalated to its owner, when validation did not run the bounded
    /// number of times (or a red run's probe could not run under `red_without_probe: hold`).
    /// The change stays `queued` but is held until `merge-queue retry` clears this.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub escalated: String,
    /// Run report (`runs/<id>/report.md`) of the run that decided the
    /// current transition, recorded at the transition itself. Notifications
    /// link this, not whatever run happens to be newest.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deciding_report: String,
    /// Failed tests of the deciding run (with outcome/duration/log where
    /// known), recorded at the transition; empty for landed/withdrawn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deciding_failed: Vec<TestResult>,
    /// Lifecycle state.
    #[serde(default)]
    pub status: ItemStatus,
    /// Base-branch commit that contains this change, once landed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub landed_as: String,
    /// History, oldest first.
    #[serde(default)]
    pub history: Vec<Event>,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

impl Item {
    /// Whether this queued change is deferred until `at` (RFC 3339, second precision, so a
    /// lexicographic comparison is a time comparison) after a validation refusal.
    pub fn is_deferred_at(&self, at: &str) -> bool {
        self.status == ItemStatus::Queued
            && !self.next_retry_at.is_empty()
            && self.next_retry_at.as_str() > at
    }

    /// Append a history event stamped now.
    pub fn log(&mut self, kind: &str, detail: impl Into<String>) {
        self.history.push(Event {
            at: now(),
            kind: kind.to_owned(),
            detail: detail.into(),
        });
    }
}

/// A landing whose validation was focused, so the complete suite is still owed on it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Obligation {
    /// Stable identifier (the run ID and batch index).
    pub id: String,
    /// RFC 3339 UTC creation time.
    pub created_at: String,
    /// Base-branch commit the focused validation landed.
    pub landed: String,
    /// Changes in that landing.
    pub prs: Vec<i64>,
    /// What actually ran before landing.
    pub validated: ValidationClaim,
    /// Set when a green complete run covered `landed` (that commit or a descendant of it).
    pub discharged: Option<Discharge>,
    /// Complete runs that covered `landed` and were red; the obligation stays open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_runs: Vec<Discharge>,
}

/// How an obligation was discharged (or attempted): the widen run's full
/// sparse results, not a bare green flag.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct Discharge {
    /// RFC 3339 UTC time.
    pub at: String,
    /// Commit the complete run validated (`landed` or a descendant).
    pub commit: String,
    /// Runner exit status of the complete run (`None` when killed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// Full per-test results of the complete run.
    #[serde(default)]
    pub results: TestResults,
    /// Validation record ID of the widen run within its run, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_id: Option<usize>,
}

impl Discharge {
    /// Whether that complete run was green (derived, not stored).
    pub fn green(&self) -> bool {
        self.exit == Some(0) && self.results.green()
    }

    /// Failing tests with outcome/duration/log, when it was not green.
    pub fn failed_results(&self) -> Vec<TestResult> {
        self.results.failed_results()
    }

    /// IDs of the failing tests, in order.
    pub fn failed_ids(&self) -> Vec<crate::tests::TestId> {
        self.results.failed().into_iter().collect()
    }
}

// Serialized form embeds the full sparse `results` and the failed tests
// with their outcomes, plus `green` computed from them (derived, never
// stored), for obligation readers.
impl Serialize for Discharge {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Discharge", 7)?;
        state.serialize_field("at", &self.at)?;
        state.serialize_field("commit", &self.commit)?;
        state.serialize_field("exit", &self.exit)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("failed", &self.failed_results())?;
        state.serialize_field("green", &self.green())?;
        state.serialize_field("validation_id", &self.validation_id)?;
        state.end()
    }
}

/// One planned batch: changes validated and landed together, in order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlannedBatch {
    /// Zero-based position in landing order.
    pub index: usize,
    /// Changes in combine order.
    pub prs: Vec<i64>,
    /// Why this batch boundary exists (`max-batch`, `solo`, `dependency`, ...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// Disposition of a queued change the plan cannot land this round.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Held {
    /// Change number.
    pub pr: i64,
    /// Stable kind: `conflicts-with-base`, `conflicts-with-queued`, `head-moved`,
    /// `head-missing`, `waiting-on-dependency`.
    pub kind: String,
    /// Human explanation and the action that unblocks it.
    pub reason: String,
    /// Other changes involved, when any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with: Vec<i64>,
    /// Paths involved, when any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// One change as the plan saw it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlannedItem {
    /// Change number.
    pub pr: i64,
    /// Head commit at plan time.
    pub head: String,
    /// Priority.
    pub priority: i64,
    /// Submission time.
    pub submitted_at: String,
    /// Files changed relative to the base.
    pub files: usize,
    /// Number of queued changes it textually conflicts with.
    pub conflict_degree: usize,
    /// A priority change, planned ahead of all other work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prioritized: bool,
}

/// Pairwise textual conflict between two queued heads (from `git merge-tree`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    /// Lower change number.
    pub a: i64,
    /// Higher change number.
    pub b: i64,
    /// Conflicting paths.
    pub paths: Vec<String>,
}

/// A reviewable landing plan.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Schema identifier.
    pub schema: String,
    /// RFC 3339 UTC time the plan was made.
    pub created_at: String,
    /// Base branch.
    pub base: String,
    /// Exact base commit the plan is bound to.
    pub base_sha: String,
    /// Every queued change considered, by number.
    pub items: Vec<PlannedItem>,
    /// Batches in landing order.
    pub batches: Vec<PlannedBatch>,
    /// Changes not landable this round, with the reason and the unblocking action.
    pub held: Vec<Held>,
    /// Pairwise conflicts among queued heads.
    pub conflicts: Vec<Conflict>,
    /// Directed dependencies `(before, after)` the plan respected.
    pub dependencies: Vec<(i64, i64)>,
    /// Queued changes, not yet priority changes, that touch only the configured CI paths: a
    /// suggestion to add the priority label, which the queue never applies itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggest_priority: Vec<i64>,
}

/// Plan schema identifier.
pub const PLAN_SCHEMA: &str = "merge-queue-plan/v1";

/// Current time as RFC 3339 UTC with second precision.
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
