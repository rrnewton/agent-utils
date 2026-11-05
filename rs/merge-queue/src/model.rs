//! Queue vocabulary: submitted changes, plans, and widening obligations.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

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

/// How built-in `--notify` delivery and event hooks divide state-change
/// reports for an item. `both` (the default) is the historical behaviour:
/// hooks and notify each fire independently. `hooks-only` makes hooks the
/// primary channel: notify is skipped only for a verdict that has a
/// configured hook (see `crate::hooks`), and still fires when no hook is
/// configured for that verdict, so a change is never silently dropped.
/// `notify-only` is the mirror: a hook with a notify counterpart is skipped
/// when the item has a notify target that will receive that verdict, and
/// still fires otherwise (including for events notify never reports).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotifyMode {
    /// Hooks and built-in notify both fire (historical behaviour).
    #[default]
    Both,
    /// Hooks primary; notify only as a fallback when no hook is configured.
    HooksOnly,
    /// Notify primary; hooks only as a fallback when notify cannot report it.
    NotifyOnly,
}

impl NotifyMode {
    /// Stable machine-facing value (`both`, `hooks-only`, `notify-only`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::HooksOnly => "hooks-only",
            Self::NotifyOnly => "notify-only",
        }
    }

    /// Parse a `notify_mode` value.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "both" => Ok(Self::Both),
            "hooks-only" => Ok(Self::HooksOnly),
            "notify-only" => Ok(Self::NotifyOnly),
            other => Err(format!(
                "notify_mode is {other:?}; expected both, hooks-only or notify-only"
            )),
        }
    }

    /// Whether this is the default (`both`).
    pub fn is_default(self) -> bool {
        self == Self::Both
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
    /// Agent name that shepherds this change (drives it to landing),
    /// carried in every event-hook payload for the item; separate from
    /// `notify` (who gets reports). Set at submit, changeable with
    /// `merge-queue shepherd`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shepherd: String,
    /// Per-item `notify_mode` override; `None` inherits `config.notify_mode`.
    /// Set with `submit --notify-mode`; a resubmission without it keeps the
    /// recorded override (like `notify` and `shepherd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_mode: Option<NotifyMode>,
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
    /// Suspect-flake reruns of this head (`validator.suspect_flake_reruns`): at most one, so a
    /// second suspect red is blamed as any red is. Left out of the record while zero.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub suspect_reruns: u32,
    /// RFC 3339 UTC time before which validation should not be retried after a refusal
    /// (empty when not deferred).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_retry_at: String,
    /// Detail of the most recent refusal (exit status and marker), for the operator.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_refusal: String,
    /// Tests that passed only after a retry in the run that validated this change
    /// (a GREEN_FLAKY run), or that the host refused to measure (an INFRA-BLOCKED run; see
    /// `hold_verdict`). Non-empty while the change is held for a ruling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flaky_nodes: Vec<String>,
    /// RFC 3339 UTC time the hold for a ruling expires (empty when not held). On
    /// expiry the change is re-queued for validation with the held nodes named, never
    /// landed unruled and never held silently forever.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub flaky_hold_until: String,
    /// The verdict the hold waits on a ruling for: empty for GREEN_FLAKY (the held tests
    /// passed only after a retry), or `INFRA-BLOCKED` (the host refused every measurement of
    /// them on a run and on its rerun of the same commit). Left out of the record while empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hold_verdict: String,
    /// What a GREEN_FLAKY hold validated: the exact candidate, the base it was built on and the
    /// validation, so that a ruling accepting the flaky passes (`merge-queue accept-flaky`) can
    /// land that very commit without validating it again. Set with the hold, cleared with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_candidate: Option<HeldCandidate>,
    /// The ruling that accepted this held change's flaky passes, until a run acts on it: the run
    /// lands the held candidate if the base has not moved, and otherwise returns the change to
    /// the queue for a new validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<Ruling>,
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

/// The candidate a GREEN_FLAKY hold validated (see [`Item::held_candidate`]).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeldCandidate {
    /// The validated commit.
    pub tip: String,
    /// The base commit it was combined onto.
    pub parent: String,
    /// The batch's changes in order, each with the head it was validated at.
    pub changes: Vec<HeldChange>,
    /// The commit after each change, aligned with `changes`.
    pub prefixes: Vec<String>,
    /// What the validation selected, and how it was chosen.
    pub claim: ValidationClaim,
    /// The run that validated it (`runs/<id>/`).
    pub run: String,
    /// The validation's number in that run (`v<N>`).
    pub validation: usize,
}

impl HeldCandidate {
    /// The batch's change numbers, in order.
    pub fn prs(&self) -> Vec<i64> {
        self.changes.iter().map(|change| change.pr).collect()
    }
}

/// A green batch validation a stopped run kept for the next run (see
/// [`crate::state::State::kept_validations`]): the next run lands it without
/// validating again while the base has not moved under it, checked by the
/// validated tree, the changes' heads, and the selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeptValidation {
    /// The validated commit.
    pub tip: String,
    /// The commit the batch was combined onto.
    pub parent: String,
    /// The batch's changes in order, each with the head it was validated at.
    pub changes: Vec<HeldChange>,
    /// What the validation selected, and how it was chosen.
    pub claim: ValidationClaim,
    /// The run that validated it (`runs/<id>/`).
    pub run: String,
    /// The validation's number in that run (`v<N>`).
    pub validation: usize,
}

impl KeptValidation {
    /// The batch's change numbers, in order.
    pub fn prs(&self) -> Vec<i64> {
        self.changes.iter().map(|change| change.pr).collect()
    }
}

/// One change of a [`HeldCandidate`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeldChange {
    /// Change number.
    pub pr: i64,
    /// The head it was validated at.
    pub head: String,
}

/// A ruling on a held change: who gave it, why, and when.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ruling {
    /// Who gave it (agent or person).
    pub by: String,
    /// Why.
    pub reason: String,
    /// RFC 3339 UTC time it was given.
    pub at: String,
}

impl Item {
    /// The effective `notify_mode` for this change: its per-item override
    /// when set, otherwise the queue's configured `notify_mode`.
    pub fn effective_notify_mode(&self, config_mode: NotifyMode) -> NotifyMode {
        self.notify_mode.unwrap_or(config_mode)
    }

    /// The verdict this change's hold waits on a ruling for: `GREEN_FLAKY` or `INFRA-BLOCKED`.
    pub fn hold_label(&self) -> &str {
        if self.hold_verdict.is_empty() {
            crate::validate::FLAKY_MARKER
        } else {
            &self.hold_verdict
        }
    }

    /// The transition and history event of this change's hold: `held-green-flaky`, or
    /// `held-infra-blocked` for an INFRA-BLOCKED hold.
    pub fn hold_kind(&self) -> &'static str {
        if self.hold_verdict == crate::validate::INFRA_BLOCKED_MARKER {
            "held-infra-blocked"
        } else if self.hold_verdict == crate::validate::SUSPECT_FLAKE_MARKER {
            "held-suspect-flake"
        } else {
            "held-green-flaky"
        }
    }

    /// Clear this change's hold for a ruling: the held tests, the expiry, the verdict, the
    /// validated candidate and any ruling on it.
    pub fn clear_hold(&mut self) {
        self.flaky_nodes.clear();
        self.flaky_hold_until.clear();
        self.hold_verdict.clear();
        self.held_candidate = None;
        self.accepted = None;
    }

    /// Whether this queued change is inside its hold for a ruling at `at`.
    pub fn is_flaky_held_at(&self, at: &str) -> bool {
        self.status == ItemStatus::Queued
            && !self.flaky_hold_until.is_empty()
            && self.flaky_hold_until.as_str() > at
    }

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
    /// The peripheral hook calls this change peripheral (absent in plans from older
    /// builds, and whenever no hook is configured: binary-touching).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub peripheral: bool,
    /// Whether the peripheral answer came from the probe cache (`cache`) or a fresh
    /// hook call (`hook`); empty when not peripheral-classified.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peripheral_source: String,
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
    /// Pairs the footprint prior marks suspect (shared affected tests). A prior only:
    /// `conflicts` remains the only verdict. Absent in plans from older builds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_conflicts: Vec<(i64, i64)>,
}

/// A change that a queued change must land after (`--after`, itself or through other queued
/// changes) and that is neither queued nor landed: ejected, withdrawn, or not in the queue. The
/// queued change is held, never validated or landed without it, until that change is queued
/// again and lands, or until the dependency is dropped by resubmitting without that `--after`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Waiting {
    /// The change waited on.
    pub pr: i64,
    /// Its status (`ejected` or `withdrawn`), or `not in the queue`.
    pub status: String,
    /// The queued changes in between, nearest first; empty when the change names it itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub through: Vec<i64>,
}

impl Waiting {
    /// `waiting on #5138 (ejected)`, or `waiting on #5139, which waits on #5138 (ejected)`.
    pub fn describe(&self) -> String {
        let mut text = "waiting on ".to_owned();
        for pr in &self.through {
            text.push_str(&format!("#{pr}, which waits on "));
        }
        text.push_str(&format!("#{} ({})", self.pr, self.status));
        text
    }

    /// The change named in `--after` that this wait comes through.
    pub fn named(&self) -> i64 {
        self.through.first().copied().unwrap_or(self.pr)
    }
}

/// What change `pr`, which must land after `after`, waits on (see [`Waiting`]), nearest first,
/// each change once.
pub fn waiting_on(pr: i64, after: &[i64], items: &BTreeMap<i64, Item>) -> Vec<Waiting> {
    let mut out: Vec<Waiting> = Vec::new();
    let mut seen = BTreeSet::from([pr]);
    let mut queue = VecDeque::from([(after.to_vec(), Vec::new())]);
    while let Some((deps, through)) = queue.pop_front() {
        for dep in deps {
            match items.get(&dep) {
                Some(item) if item.status == ItemStatus::Landed => {}
                Some(item) if item.status == ItemStatus::Queued => {
                    if seen.insert(dep) {
                        let mut next: Vec<i64> = through.clone();
                        next.push(dep);
                        queue.push_back((item.after.clone(), next));
                    }
                }
                other => {
                    if !out.iter().any(|waiting| waiting.pr == dep) {
                        out.push(Waiting {
                            pr: dep,
                            status: other
                                .map_or("not in the queue", |item| item.status.as_str())
                                .to_owned(),
                            through: through.clone(),
                        });
                    }
                }
            }
        }
    }
    out
}

/// A queued change and what it waits on (see [`Waiting`]).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WaitingChange {
    /// The queued change.
    pub pr: i64,
    /// What it waits on.
    pub waiting_on: Vec<Waiting>,
}

/// Every queued change that waits on a change it must land after, by change number.
pub fn waiting_changes(items: &[Item]) -> Vec<WaitingChange> {
    let by_pr: BTreeMap<i64, Item> = items.iter().map(|item| (item.pr, item.clone())).collect();
    by_pr
        .values()
        .filter(|item| item.status == ItemStatus::Queued)
        .filter_map(|item| {
            let waiting_on = waiting_on(item.pr, &item.after, &by_pr);
            (!waiting_on.is_empty()).then_some(WaitingChange {
                pr: item.pr,
                waiting_on,
            })
        })
        .collect()
}

/// Plan schema identifier.
pub const PLAN_SCHEMA: &str = "merge-queue-plan/v1";

/// Current time as RFC 3339 UTC with second precision.
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
