//! The decision log and the throughput metrics computed from it.
//!
//! With fewer validation slots than candidates, every run chooses which speculative validations
//! to spend them on. Each validation a run performs leaves one record here, so the choices can be
//! measured and the metrics they move can be optimised:
//!
//! ```text
//! <state>/decisions/<YYYY-MM-DD>.jsonl   one line per validation, by UTC day of its end
//! <state>/decisions/skips.jsonl          one line per change left out of its batch
//! <state>/decisions/rulings.jsonl        one line per ruling on a held batch
//! ```
//!
//! A record names the candidate (changes, heads, the commit and the base it was combined onto),
//! the batch width and its place in the speculation window, the predicted pass probability (null
//! until batching priors exist), per-change features, who validated it (here, or a donor host),
//! its cost (wall and CPU time), its outcome, the culprit a red batch's search named, and, for the
//! validation that landed changes, each change's time from ready to landed.
//!
//! Records are written when the run finishes, from its report, by the same code that
//! [`reconstruct`] runs over the reports of earlier runs. The directory is pruned after each
//! write to [`DecisionsConfig`]'s age and size bounds.
//!
//! [`stats`] summarises a window of records: the batch-width distribution, success rate by width,
//! changes landed per validate-hour and ready-to-land latency percentiles.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::Git;
use crate::model::Item;
use crate::state::{DecisionsConfig, PriorityConfig};

/// Schema identifier of one decision record.
pub const DECISION_SCHEMA: &str = "merge-queue-decision/v1";

/// One validation the queue chose to run, and what came of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// [`DECISION_SCHEMA`].
    pub schema: String,
    /// RFC 3339 UTC time the validation ended.
    pub at: String,
    /// RFC 3339 UTC time it started.
    pub started_at: String,
    /// Run ID (`runs/<id>/`).
    pub run: String,
    /// `run`, `shadow` or `widen`. Shadow landings are simulated.
    pub mode: String,
    /// Validation number within the run.
    pub validation: usize,
    /// `batch`, `probe`, `remainder` or `widen`.
    pub purpose: String,
    /// The candidate's changes, in combine order.
    pub changes: Vec<DecisionChange>,
    /// Commit validated.
    pub commit: String,
    /// Commit the changes were combined onto; `null` when not recorded.
    pub base: Option<String>,
    /// Number of changes in the candidate.
    pub width: usize,
    /// A batch's place in the speculation window; `null` for other purposes or when not recorded.
    pub speculation: Option<crate::run::Speculation>,
    /// Predicted probability that the candidate validates green; `null` until batching priors
    /// exist.
    pub predicted_pass: Option<f64>,
    /// Who ran it.
    pub validator: ValidatorUsed,
    /// What it cost.
    pub cost: Cost,
    /// Selection run: `all` or `N tests`.
    pub selection: String,
    /// `green`, `red`, `refused` (validation did not run) or `cancelled`.
    pub outcome: String,
    /// Label hygiene for a red verdict: `real-defect`, `false-red`,
    /// `composition`, `infra`, `unresolved`, or `host` (machine-set when the
    /// red came from a host-side refusal, exit 75). Absent means unclassified
    /// (today's behaviour); a later `reclassify` record overrides it
    /// append-only, never by rewriting this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub red_class: Option<String>,
    /// Failed tests of a red validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
    /// The change a red validation's culprit search ejected, when it named one.
    pub culprit: Option<i64>,
    /// The changes this validation landed, and when; `null` when it landed nothing.
    pub landed: Option<LandedBy>,
    /// Built by `stats --reconstruct` from an earlier run's report, not written by the run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reconstructed: bool,
}

/// One change in a decision's candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionChange {
    /// Change number.
    pub pr: i64,
    /// Exact head submitted; empty when not known.
    pub head: String,
    /// RFC 3339 UTC time it was (last) submitted as ready before this validation ended.
    pub ready_at: Option<String>,
    /// Seconds from ready to landed, when this validation landed it.
    pub ready_to_land_s: Option<f64>,
    /// What the change looks like.
    pub features: Features,
    /// The trained model's predicted pass probability for this change; `null`
    /// unless `Config::predict` is on and a model is trained (see `predict`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_pass: Option<f64>,
}

/// Per-change features for the decision log and for future batching priors.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Features {
    /// Lines added relative to the plan's base; `null` when the commits are not available.
    pub lines_added: Option<u64>,
    /// Lines deleted relative to the plan's base.
    pub lines_deleted: Option<u64>,
    /// Files changed relative to the plan's base.
    pub files: Option<usize>,
    /// Components touched: each changed path's first directory, or `(root)` for a file at the
    /// top. A queue-local bucketing until a shared change classifier exists.
    pub components: Vec<String>,
    /// Queued changes it textually conflicted with when planned.
    pub conflict_degree: Option<usize>,
    /// Numeric priority (lower lands first).
    pub priority: Option<i64>,
    /// A priority change (the priority label).
    pub prioritized: bool,
    /// How it entered the queue: `submit`, or the `sync` source (`label:NAME`, `file:PATH`, ...).
    pub trigger: Option<String>,
    /// Every changed path is a configured CI path (`priority.ci_paths`).
    pub ci_only: bool,
    /// Soft-green evidence: the submitter's own validation claim, if any.
    pub claim: Option<ClaimEvidence>,
    /// The shared change classifier's answer, when `classifier.policy` is configured and the
    /// policy and the change's paths were readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<ChangeClass>,
    /// The peripheral hook calls this change peripheral (only with the lane configured).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub peripheral: bool,
    /// Whether the peripheral answer came from the cache (`cache`) or a fresh hook call
    /// (`hook`); empty when the lane is off.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peripheral_source: String,
}

/// What the configured test-footprint policy says of a change (see the `change-classifier`
/// crate).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeClass {
    /// `skip`, `selective` or `full`.
    pub verdict: String,
    /// Paths that matched no rule.
    pub unknown: usize,
    /// Every path forces the full suite (build and CI configuration).
    pub only_force_full: bool,
    /// Every path is inert.
    pub only_inert: bool,
}

/// What the submitter said it ran before submitting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimEvidence {
    /// `complete` or `focused`.
    pub scope: String,
    /// Tests claimed; `null` for the complete suite.
    pub tests: Option<usize>,
    /// Selection policy that chose them.
    pub policy: String,
    /// The claim names the submitted head (or names no commit), rather than another commit.
    pub at_head: bool,
}

/// Where a validation ran.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ValidatorUsed {
    /// `local` or `donor`.
    pub kind: String,
    /// The donor (`NAME@HOST`), or `null` when it ran here (or the donor is not known).
    pub host: Option<String>,
}

/// What a validation cost.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// Wall time in seconds.
    pub wall_s: f64,
    /// CPU time (user plus system) in seconds, when it could be read.
    pub cpu_s: Option<f64>,
}

/// What a validation landed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandedBy {
    /// RFC 3339 UTC landing time, when known.
    pub at: Option<String>,
    /// Changes landed.
    pub prs: Vec<i64>,
}

/// Directory of the decision log.
pub fn dir(state_root: &Path) -> PathBuf {
    state_root.join("decisions")
}

/// The UTC day a day file of validation records is for (`YYYY-MM-DD.jsonl`); `None` for any
/// other file in the directory, such as `skips.jsonl` or `rulings.jsonl`.
fn day_of(name: &str) -> Option<&str> {
    let day = name.strip_suffix(".jsonl")?;
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .ok()
        .map(|_| day)
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn format_time(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Start time of a run from its ID (`YYYYMMDDTHHMMSSZ`, with an optional `-N` suffix).
pub fn run_start(run: &str) -> Option<DateTime<Utc>> {
    let stamp = run.split('-').next()?;
    NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|time| time.and_utc())
}

/// A changed path's component in the queue-local bucketing: its first directory.
pub fn component(path: &str) -> String {
    match path.split_once('/') {
        Some((first, _)) if !first.is_empty() => first.to_owned(),
        _ => "(root)".to_owned(),
    }
}

/// Lines added and deleted and the paths changed from the merge base of `base` and `head` to
/// `head`, or `None` when Git cannot say (the commits are gone).
fn diff_stat(git: &Git, base: &str, head: &str) -> Option<(u64, u64, Vec<String>)> {
    if base.is_empty() || head.is_empty() {
        return None;
    }
    let out = git
        .run(&[
            "diff",
            "--numstat",
            "--no-renames",
            &format!("{base}...{head}"),
        ])
        .ok()?;
    let (mut added, mut deleted, mut paths) = (0, 0, Vec::new());
    for line in out.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(plus), Some(minus), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // Binary files count as changed with no lines.
        added += plus.parse::<u64>().unwrap_or(0);
        deleted += minus.parse::<u64>().unwrap_or(0);
        paths.push(path.to_owned());
    }
    Some((added, deleted, paths))
}

/// Historical green rates from earlier decision records: the base-rate model.
/// `global` is the overall green share of decided (green/red) validations;
/// `by_component` is per first-component. `samples` counts decided validations.
#[derive(Clone, Debug, Default)]
pub struct PriorRates {
    /// Overall green share (0..1).
    pub global: f64,
    /// Green share per first component of a change.
    pub by_component: BTreeMap<String, f64>,
    /// Decided validations the rates came from.
    pub samples: usize,
}

/// Read the decision records under `state_root` into base rates. Fewer than 10
/// decided validations is not a model: callers then record no prediction.
pub fn prior_rates(state_root: &Path) -> Option<PriorRates> {
    let dir = state_root.join("decisions");
    let mut decided = 0usize;
    let mut green = 0usize;
    let mut per: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".jsonl") || name == "rulings.jsonl" || name == "skips.jsonl" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            let Ok(decision) = serde_json::from_str::<Decision>(line) else {
                continue;
            };
            let is_green = match decision.outcome.as_str() {
                "green" => true,
                "red" => false,
                _ => continue,
            };
            decided += 1;
            green += usize::from(is_green);
            for change in &decision.changes {
                if let Some(component) = change.features.components.first() {
                    let slot = per.entry(component.clone()).or_default();
                    slot.1 += 1;
                    slot.0 += usize::from(is_green);
                }
            }
        }
    }
    if decided < 10 {
        return None;
    }
    Some(PriorRates {
        global: green as f64 / decided as f64,
        by_component: per
            .into_iter()
            .map(|(k, (g, n))| (k, g as f64 / n as f64))
            .collect(),
        samples: decided,
    })
}

/// Per-change historical green rates from the decision records: for each change,
/// the share of its decided (green/red) validations that were green, plus the
/// global share. The value-ordering input (see `Config::value_ordering`).
pub fn change_rates(state_root: &Path) -> Option<(f64, BTreeMap<i64, f64>)> {
    let dir = state_root.join("decisions");
    let mut decided = 0usize;
    let mut green = 0usize;
    let mut per: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".jsonl") || name == "rulings.jsonl" || name == "skips.jsonl" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let is_green = match record.get("outcome").and_then(Value::as_str) {
                Some("green") => true,
                Some("red") => false,
                _ => continue,
            };
            decided += 1;
            green += usize::from(is_green);
            if let Some(changes) = record.get("changes").and_then(Value::as_array) {
                for change in changes {
                    if let Some(pr) = change.get("pr").and_then(Value::as_i64) {
                        let slot = per.entry(pr).or_default();
                        slot.1 += 1;
                        slot.0 += usize::from(is_green);
                    }
                }
            }
        }
    }
    if decided == 0 {
        return None;
    }
    Some((
        green as f64 / decided as f64,
        per.into_iter()
            .map(|(pr, (g, n))| (pr, g as f64 / n as f64))
            .collect(),
    ))
}

/// The red share of the most recent decided (green/red) validations in the
/// decision records: at most `limit` of them, newest first by their end time.
/// `None` when no decided validation is recorded, so a caller gating on the
/// rate treats no history as no evidence of a red head.
pub fn recent_red_rate(state_root: &Path, limit: usize) -> Option<f64> {
    if limit == 0 {
        return None;
    }
    let dir = state_root.join("decisions");
    let mut decided: Vec<(String, bool)> = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".jsonl") || name == "rulings.jsonl" || name == "skips.jsonl" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            let Ok(decision) = serde_json::from_str::<Decision>(line) else {
                continue;
            };
            let is_green = match decision.outcome.as_str() {
                "green" => true,
                "red" => false,
                _ => continue,
            };
            decided.push((decision.at, is_green));
        }
    }
    decided.sort_by(|a, b| b.0.cmp(&a.0));
    decided.truncate(limit);
    if decided.is_empty() {
        return None;
    }
    let red = decided.iter().filter(|(_, green)| !green).count();
    Some(red as f64 / decided.len() as f64)
}

/// Inputs for turning one run's report into decision records.
pub struct Sources<'a> {
    /// Item records by change number (for submission times, sources and claims).
    pub items: &'a BTreeMap<i64, Item>,
    /// The queue's repository, for change sizes; `None` skips them.
    pub git: Option<&'a Git>,
    /// For the CI-only feature.
    pub priority: &'a PriorityConfig,
    /// Test-footprint policy path in the repository (`classifier.policy`), read at the plan's
    /// base; empty for none.
    pub classifier_policy: &'a str,
    /// Base rates from earlier decision records; `None` records no prediction.
    pub prior: Option<PriorRates>,
    /// Trained success model (`Config::predict` on); wins over `prior` for
    /// `predicted_pass` when present.
    pub model: Option<crate::predict::Model>,
    /// Latest history feature vector per change, for `model` (see
    /// `predict::latest_vectors`); the batch width feature is patched per batch.
    pub model_vectors: BTreeMap<i64, Vec<f64>>,
}

fn str_field<'v>(value: &'v Value, key: &str) -> &'v str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn int_list(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .map(|list| list.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// The donor named on the first line of a donor-validated run's log, for reports written before
/// validation records named it.
fn donor_from_log(log: &str) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let rest = text
        .lines()
        .next()?
        .strip_prefix("merge-queue: validated by donor ")?;
    let (name, rest) = rest.split_once(" on ")?;
    let host = rest.split(" (").next()?;
    Some(format!("{name}@{host}"))
}

/// The latest event of `kind` in `item`'s history at or before `until` (any time when `None`).
fn event_time(item: &Item, kind: &str, until: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    item.history
        .iter()
        .filter(|event| event.kind == kind)
        .filter_map(|event| parse_time(&event.at))
        .filter(|at| until.is_none_or(|until| *at <= until))
        .max()
}

/// The first event of `kind` in `item`'s history at or after `from`.
fn event_after(item: &Item, kind: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    item.history
        .iter()
        .filter(|event| event.kind == kind)
        .filter_map(|event| parse_time(&event.at))
        .filter(|at| *at >= from)
        .min()
}

/// One decision record per validation in a run report (`report.json`, as JSON).
///
/// The same code serves a finishing run and [`reconstruct`]; `reconstructed` marks the records
/// built later. Fields a report does not hold are `null`.
pub fn from_report(report: &Value, sources: &Sources<'_>, reconstructed: bool) -> Vec<Decision> {
    let run = str_field(report, "run");
    let Some(start) = run_start(run) else {
        return Vec::new();
    };
    let mode = str_field(report, "mode");
    let plan = report.get("plan").cloned().unwrap_or(Value::Null);
    let plan_base = str_field(&plan, "base_sha").to_owned();
    let planned: BTreeMap<i64, &Value> = plan
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| Some((item.get("pr")?.as_i64()?, item)))
        .collect();
    // Which validation landed which changes, and when.
    let mut landed_by: BTreeMap<u64, (Vec<i64>, Option<String>)> = BTreeMap::new();
    for landed in report
        .get("landed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let prs = int_list(&landed["prs"]);
        let at = landed.get("at").and_then(Value::as_str).map(str::to_owned);
        if let Some(last) = landed["validations"]
            .as_array()
            .and_then(|ids| ids.iter().filter_map(Value::as_u64).max())
        {
            landed_by.insert(last, (prs, at));
        }
    }
    let ejected: Vec<&Value> = report
        .get("ejected")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let mut features: BTreeMap<i64, Features> = BTreeMap::new();
    // The shared classifier's policy as of the plan's base, when one is configured.
    let classifier = (!sources.classifier_policy.is_empty() && !plan_base.is_empty())
        .then(|| {
            let text = sources.git?.run(&[
                "show",
                &format!("{plan_base}:{}", sources.classifier_policy),
            ]);
            change_classifier::Policy::from_json(&text.ok()?).ok()
        })
        .flatten();
    let mut feature = |pr: i64, head: &str| -> Features {
        features
            .entry(pr)
            .or_insert_with(|| {
                let item = sources.items.get(&pr);
                let planned = planned.get(&pr);
                let stat = sources.git.and_then(|git| diff_stat(git, &plan_base, head));
                let classified = classifier
                    .as_ref()
                    .zip(stat.as_ref())
                    .map(|(policy, stat)| change_classifier::classify(policy, &stat.2));
                // The shared classifier's buckets when there is one; else first directories.
                let components: BTreeSet<String> = match &classified {
                    Some(classified) => classified.components.clone(),
                    None => stat
                        .iter()
                        .flat_map(|(_, _, paths)| paths.iter().map(|path| component(path)))
                        .collect(),
                };
                Features {
                    lines_added: stat.as_ref().map(|stat| stat.0),
                    lines_deleted: stat.as_ref().map(|stat| stat.1),
                    files: stat.as_ref().map(|stat| stat.2.len()).or_else(|| {
                        planned
                            .and_then(|p| p.get("files")?.as_u64())
                            .map(|n| n as usize)
                    }),
                    components: components.into_iter().collect(),
                    conflict_degree: planned
                        .and_then(|p| p.get("conflict_degree")?.as_u64())
                        .map(|n| n as usize),
                    priority: planned
                        .and_then(|p| p.get("priority")?.as_i64())
                        .or(item.map(|item| item.priority)),
                    prioritized: item.is_some_and(|item| item.prioritized),
                    trigger: item.map(|item| {
                        if item.source.is_empty() {
                            "submit".to_owned()
                        } else {
                            item.source.clone()
                        }
                    }),
                    ci_only: stat
                        .as_ref()
                        .is_some_and(|stat| sources.priority.only_ci_paths(&stat.2)),
                    claim: item
                        .and_then(|item| item.claim.as_ref())
                        .map(|claim| ClaimEvidence {
                            scope: claim.scope.as_str().to_owned(),
                            tests: claim.selection.tests().map(BTreeSet::len),
                            policy: claim.policy.clone(),
                            at_head: claim.commit.is_empty() || head.starts_with(&claim.commit),
                        }),
                    classification: classified.map(|classified| ChangeClass {
                        verdict: classified.verdict.as_str().to_owned(),
                        unknown: classified.unknown.len(),
                        only_force_full: classified.only_force_full(),
                        only_inert: classified.only_inert(),
                    }),
                    peripheral: planned
                        .and_then(|p| p.get("peripheral")?.as_bool())
                        .unwrap_or(false),
                    peripheral_source: planned
                        .and_then(|p| p.get("peripheral_source")?.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                }
            })
            .clone()
    };
    let mut out = Vec::new();
    for v in report
        .get("validations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = v.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let start_ms = v.get("start_ms").and_then(Value::as_u64).unwrap_or(0);
        let elapsed_ms = v.get("elapsed_ms").and_then(Value::as_u64).unwrap_or(0);
        let started = start + chrono::Duration::milliseconds(start_ms as i64);
        let ended = started + chrono::Duration::milliseconds(elapsed_ms as i64);
        let prs = int_list(&v["prs"]);
        let flag = |key: &str| v.get(key).and_then(Value::as_bool).unwrap_or(false);
        let outcome = if flag("cancelled") {
            "cancelled"
        } else if flag("refused") {
            "refused"
        } else if flag("green") {
            "green"
        } else {
            "red"
        };
        let failed: Vec<String> = v
            .get("failed")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();
        // Only a batch's (or a remainder's) red starts a culprit search; probes name none.
        let searched = matches!(str_field(v, "purpose"), "batch" | "remainder");
        let culprit = (outcome == "red" && searched)
            .then(|| {
                ejected
                    .iter()
                    .find(|e| e.get("found_by").and_then(Value::as_u64) == Some(id))
                    .or_else(|| {
                        // Reports from before culprits named their validation: a single
                        // failed-tests ejection among this candidate's changes.
                        let mut matching = ejected.iter().filter(|e| {
                            e.get("found_by").is_none()
                                && str_field(e, "kind") == "failed-tests"
                                && e["pr"].as_i64().is_some_and(|pr| prs.contains(&pr))
                        });
                        let first = matching.next();
                        first.filter(|_| matching.next().is_none())
                    })
                    .and_then(|e| e["pr"].as_i64())
            })
            .flatten();
        let landed = landed_by.get(&id).map(|(landed_prs, at)| {
            let at = at.clone().or_else(|| {
                landed_prs
                    .iter()
                    .filter_map(|pr| sources.items.get(pr))
                    // Event times and run IDs are whole seconds, so `ended` is only an
                    // estimate; allow for the truncation.
                    .filter_map(|item| {
                        event_after(item, "landed", ended - chrono::Duration::seconds(2))
                    })
                    .min()
                    .map(format_time)
            });
            LandedBy {
                at,
                prs: landed_prs.clone(),
            }
        });
        let landed_at = landed
            .as_ref()
            .and_then(|landed| landed.at.as_deref())
            .and_then(parse_time);
        let changes: Vec<DecisionChange> = prs
            .iter()
            .map(|pr| {
                let item = sources.items.get(pr);
                let head = planned
                    .get(pr)
                    .map(|p| str_field(p, "head").to_owned())
                    .filter(|head| !head.is_empty())
                    .or_else(|| item.map(|item| item.head.clone()))
                    .unwrap_or_default();
                let ready = item.and_then(|item| event_time(item, "submitted", Some(ended)));
                let landed_here = landed
                    .as_ref()
                    .is_some_and(|landed| landed.prs.contains(pr));
                DecisionChange {
                    pr: *pr,
                    features: feature(*pr, &head),
                    predicted_pass: None,
                    head,
                    ready_at: ready.map(format_time),
                    ready_to_land_s: match (ready, landed_at) {
                        (Some(ready), Some(landed)) if landed_here => {
                            Some((landed - ready).num_milliseconds() as f64 / 1000.0)
                        }
                        _ => None,
                    },
                }
            })
            .collect();
        // Trained model first (Config::predict): per-change probabilities from
        // each change's latest history vector, batch = product discounted by
        // known pair conflicts. A change with no history vector gets the
        // model's base rate.
        let mut changes = changes;
        let model_predicted = sources.model.as_ref().map(|model| {
            let width_feature = (changes.len().max(1) as f64).ln();
            let mut ps: Vec<(i64, f64)> = Vec::new();
            for change in &mut changes {
                let p = match sources.model_vectors.get(&change.pr) {
                    Some(vector) => {
                        let mut vector = vector.clone();
                        if let Some(last) = vector.last_mut() {
                            *last = width_feature;
                        }
                        model.predict_raw(&vector)
                    }
                    None => model.base_rate,
                };
                change.predicted_pass = Some(p);
                ps.push((change.pr, p));
            }
            model.batch_pass(&ps)
        });
        // Base-rate prediction: each change contributes its first component's
        // historical green rate, or the global rate when it has none.
        let predicted_pass = model_predicted.or_else(|| {
            sources.prior.as_ref().map(|prior| {
                if changes.is_empty() {
                    prior.global
                } else {
                    let sum: f64 = changes
                        .iter()
                        .map(|change| {
                            change
                                .features
                                .components
                                .first()
                                .and_then(|component| prior.by_component.get(component))
                                .copied()
                                .unwrap_or(prior.global)
                        })
                        .sum();
                    sum / changes.len() as f64
                }
            })
        });
        let donor = v
            .get("donor")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| donor_from_log(str_field(v, "log")));
        let selection = match v.get("selection") {
            Some(selection) if selection["kind"] == "all" => "all".to_owned(),
            Some(selection) => {
                let n = selection["tests"].as_array().map_or(0, Vec::len);
                format!("{n} test{}", if n == 1 { "" } else { "s" })
            }
            None => String::new(),
        };
        out.push(Decision {
            schema: DECISION_SCHEMA.to_owned(),
            at: format_time(ended),
            started_at: format_time(started),
            run: run.to_owned(),
            mode: mode.to_owned(),
            validation: id as usize,
            purpose: str_field(v, "purpose").to_owned(),
            width: prs.len(),
            changes,
            commit: str_field(v, "commit").to_owned(),
            base: v
                .get("parent")
                .and_then(Value::as_str)
                .filter(|parent| !parent.is_empty())
                .map(str::to_owned),
            speculation: v
                .get("speculation")
                .and_then(|speculation| serde_json::from_value(speculation.clone()).ok()),
            predicted_pass,
            validator: ValidatorUsed {
                kind: if donor.is_some() { "donor" } else { "local" }.to_owned(),
                host: donor,
            },
            cost: Cost {
                wall_s: elapsed_ms as f64 / 1000.0,
                cpu_s: v
                    .get("cpu_ms")
                    .and_then(Value::as_u64)
                    .map(|ms| ms as f64 / 1000.0),
            },
            selection,
            // Machine red class: host when the red came from a host-side
            // refusal (exit 75), real-defect when a culprit search named a
            // change, else unresolved. Adjudicated classes (false-red,
            // composition, infra) arrive only via `reclassify`.
            red_class: (outcome == "red").then(|| {
                if v.get("exit").and_then(Value::as_i64) == Some(75) {
                    "host".to_owned()
                } else if culprit.is_some() {
                    "real-defect".to_owned()
                } else {
                    "unresolved".to_owned()
                }
            }),
            outcome: outcome.to_owned(),
            failed,
            culprit,
            landed,
            reconstructed,
        });
    }
    out
}

/// Append `decisions` to the log (one file per UTC day of each record's end), then prune it.
pub fn append(
    state_root: &Path,
    decisions: &[Decision],
    bounds: &DecisionsConfig,
) -> Result<(), String> {
    if decisions.is_empty() {
        return Ok(());
    }
    let dir = dir(state_root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let mut by_day: BTreeMap<String, String> = BTreeMap::new();
    for decision in decisions {
        let day = decision.at.get(..10).unwrap_or("unknown").to_owned();
        let line = serde_json::to_string(decision)
            .map_err(|error| format!("cannot encode a decision record: {error}"))?;
        let text = by_day.entry(day).or_default();
        text.push_str(&line);
        text.push('\n');
    }
    for (day, text) in by_day {
        let path = dir.join(format!("{day}.jsonl"));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    prune(&dir, bounds, Utc::now())
}

/// One batch-construction skip: a change left out of its batch (it stays
/// queued), with the named reason. Written beside the validation records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkipRecord {
    /// Always `skip`.
    pub decision: String,
    /// Change left out of the batch.
    pub pr: i64,
    /// Named reason, e.g. `recount-pins refused: ...`.
    pub reason: String,
    /// Run that decided it (`runs/<id>/`).
    pub run: String,
    /// RFC 3339 UTC time.
    pub at: String,
}

/// Append skip records to `decisions/skips.jsonl` (one JSON object per line).
pub fn append_skips(state_root: &Path, skips: &[SkipRecord]) -> Result<(), String> {
    if skips.is_empty() {
        return Ok(());
    }
    let dir = dir(state_root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = dir.join("skips.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    for skip in skips {
        let line = serde_json::to_string(skip)
            .map_err(|error| format!("cannot encode a skip record: {error}"))?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    Ok(())
}

/// Schema identifier of one ruling record.
pub const RULING_SCHEMA: &str = "merge-queue-ruling/v1";

/// A ruling on a batch that passed only after retries (GREEN_FLAKY), and what came of it: an
/// `accept-flaky` ruling given with `merge-queue accept-flaky`, or a `known-flaky` one, where
/// every retried test was on the known-flaky list. Written beside the validation records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RulingRecord {
    /// [`RULING_SCHEMA`].
    pub schema: String,
    /// `accept-flaky` or `known-flaky`.
    pub ruling: String,
    /// The batch's changes, in order.
    pub prs: Vec<i64>,
    /// The heads they were validated at, aligned with `prs`.
    pub heads: Vec<String>,
    /// The validated candidate.
    pub commit: String,
    /// The base it was combined onto.
    pub base: String,
    /// The tests that passed only after a retry.
    pub flaky: Vec<String>,
    /// Who ruled: the person or agent, or the known-flaky entries' authors.
    pub by: String,
    /// Why: the reason given, or the issues tracking the known-flaky tests.
    pub reason: String,
    /// RFC 3339 UTC time the ruling was given.
    pub ruled_at: String,
    /// `landed` (exactly the validated commit) or `requeued` (validated again).
    pub outcome: String,
    /// What decided the outcome.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The run that validated the candidate, and the validation (`v<N>`).
    pub validated_in: String,
    /// The run that acted on the ruling.
    pub run: String,
    /// RFC 3339 UTC time it acted.
    pub at: String,
}

/// Append one ruling record to `decisions/rulings.jsonl`.
pub fn append_ruling(state_root: &Path, record: &RulingRecord) -> Result<(), String> {
    let dir = dir(state_root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = dir.join("rulings.jsonl");
    let mut line = serde_json::to_string(record)
        .map_err(|error| format!("cannot encode a ruling record: {error}"))?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Read every ruling record (none when the file does not exist).
pub fn read_rulings(state_root: &Path) -> Result<Vec<RulingRecord>, String> {
    let path = dir(state_root).join("rulings.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(number, line)| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "{}:{}: bad ruling record: {error}",
                    path.display(),
                    number + 1
                )
            })
        })
        .collect()
}

/// Adjudicated red classes a `reclassify` record may assert.
pub const RED_CLASSES: [&str; 6] = [
    "real-defect",
    "false-red",
    "composition",
    "infra",
    "unresolved",
    "host",
];

/// Schema identifier of one reclassification record.
pub const RECLASSIFY_SCHEMA: &str = "merge-queue-reclassification/v1";

/// One post-hoc reclassification of a decided red validation
/// (`merge-queue reclassify RUN/VALIDATION --red-class CLASS --reason TEXT`).
/// Append-only: the original decision record is never rewritten; readers
/// overlay these in file order, latest wins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reclassification {
    /// [`RECLASSIFY_SCHEMA`].
    pub schema: String,
    /// Run ID of the decided validation.
    pub run: String,
    /// Validation number within the run.
    pub validation: usize,
    /// The asserted red class (one of [`RED_CLASSES`]).
    pub red_class: String,
    /// Why.
    pub reason: String,
    /// Who reclassified.
    pub by: String,
    /// RFC 3339 UTC time.
    pub at: String,
}

/// Append one reclassification to `decisions/reclassifications.jsonl`.
pub fn append_reclassification(state_root: &Path, record: &Reclassification) -> Result<(), String> {
    let dir = dir(state_root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = dir.join("reclassifications.jsonl");
    let mut line = serde_json::to_string(record).map_err(|error| error.to_string())?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Every reclassification, in append order (none when the file is absent).
pub fn read_reclassifications(state_root: &Path) -> Vec<Reclassification> {
    let Ok(text) = std::fs::read_to_string(dir(state_root).join("reclassifications.jsonl")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Effective red class per decided validation, `(run, validation) -> class`:
/// the machine class on the record, overridden by reclassifications in order.
pub fn effective_red_classes(
    state_root: &Path,
    decisions: &[Decision],
) -> BTreeMap<(String, usize), String> {
    let mut out: BTreeMap<(String, usize), String> = BTreeMap::new();
    for decision in decisions {
        if let Some(class) = &decision.red_class {
            out.insert((decision.run.clone(), decision.validation), class.clone());
        }
    }
    for record in read_reclassifications(state_root) {
        out.insert((record.run, record.validation), record.red_class);
    }
    out
}

/// Overlay reclassifications onto copies of `decisions` (records on disk are
/// never rewritten; this is the reader-side effective view).
pub fn overlay_red_classes(state_root: &Path, decisions: &mut [Decision]) {
    let effective = effective_red_classes(state_root, decisions);
    for decision in decisions.iter_mut() {
        if let Some(class) = effective.get(&(decision.run.clone(), decision.validation)) {
            decision.red_class = Some(class.clone());
        }
    }
}

/// Delete day files older than the age bound, then the oldest until the total fits the size
/// bound. The newest day file is always kept; the other files (skips, rulings) are not day files
/// and are left alone.
pub fn prune(dir: &Path, bounds: &DecisionsConfig, now: DateTime<Utc>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut files: Vec<(String, PathBuf, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let day = day_of(&name)?.to_owned();
            let size = entry.metadata().ok()?.len();
            Some((day, entry.path(), size))
        })
        .collect();
    files.sort();
    let oldest_kept = (now - chrono::Duration::days(i64::from(bounds.keep_days())))
        .format("%Y-%m-%d")
        .to_string();
    let mut total: u64 = files.iter().map(|(_, _, size)| size).sum();
    let newest = files.len().saturating_sub(1);
    for (index, (day, path, size)) in files.iter().enumerate() {
        if index == newest {
            break;
        }
        if *day < oldest_kept || total > bounds.max_bytes() {
            std::fs::remove_file(path)
                .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
            total -= size;
        }
    }
    Ok(())
}

/// Read every validation record in the log (the day files) that ended in `[since, until)`.
pub fn read(
    state_root: &Path,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<Decision>, String> {
    let dir = dir(state_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(day_of)
                .is_some()
        })
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let decision: Decision = serde_json::from_str(line).map_err(|error| {
                format!(
                    "{}:{}: bad decision record: {error}",
                    path.display(),
                    number + 1
                )
            })?;
            if parse_time(&decision.at).is_some_and(|at| at >= since && at < until) {
                out.push(decision);
            }
        }
    }
    Ok(out)
}

/// What [`reconstruct`] could not recover, one line each.
pub const NOT_RECONSTRUCTABLE: [&str; 5] = [
    "predicted_pass: no batching priors existed (null in live records too)",
    "cost.cpu_s: runs before decision logging did not read CPU time",
    "speculation (depth and window per batch): not recorded before decision logging; each report keeps only its window trace (`windows`)",
    "base (the commit a candidate was combined onto): not recorded before decision logging",
    "culprit, when a red batch's report holds no single failed-tests ejection among its changes",
];

/// Decision records rebuilt from the reports of runs whose validations ended in
/// `[since, until)`, for windows before the log existed. Runs without a `report.json` (killed)
/// contribute their `validations/*/record.json` files, with no landing information.
pub fn reconstruct(
    state_root: &Path,
    sources: &Sources<'_>,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<(Vec<Decision>, Vec<String>), String> {
    let runs_dir = state_root.join("runs");
    let mut notes = Vec::new();
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return Ok((Vec::new(), notes));
    };
    let mut runs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    runs.sort();
    let mut out = Vec::new();
    let mut partial = 0;
    for run_dir in runs {
        let run = run_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(start) = run_start(&run) else {
            continue;
        };
        if start >= until {
            continue;
        }
        let report_path = run_dir.join("report.json");
        let report: Value = match std::fs::read_to_string(&report_path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|error| format!("cannot parse {}: {error}", report_path.display()))?,
            Err(_) => {
                let records = validation_records(&run_dir);
                if records.is_empty() {
                    continue;
                }
                partial += 1;
                serde_json::json!({"run": run, "mode": "unknown", "validations": records})
            }
        };
        out.extend(
            from_report(&report, sources, true)
                .into_iter()
                .filter(|decision| {
                    parse_time(&decision.at).is_some_and(|at| at >= since && at < until)
                }),
        );
    }
    if partial > 0 {
        notes.push(format!(
            "{partial} run(s) ended without a report (killed): their validations are counted, but not what they landed or ejected"
        ));
    }
    Ok((out, notes))
}

fn validation_records(run_dir: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(run_dir.join("validations")) else {
        return Vec::new();
    };
    let mut records: Vec<Value> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("record.json")).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    records.sort_by_key(|record: &Value| record["id"].as_u64());
    records
}

/// Batch-width distribution.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Widths {
    /// Batch validations counted.
    pub batches: usize,
    /// Average width.
    pub average: f64,
    /// Smallest width.
    pub min: usize,
    /// Largest width.
    pub max: usize,
    /// Batches per width.
    pub counts: BTreeMap<usize, usize>,
}

/// Verdicts of the batches of one width.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WidthOutcomes {
    /// Green batches.
    pub green: usize,
    /// Red batches.
    pub red: usize,
    /// Refused or cancelled batches (no verdict).
    pub no_verdict: usize,
    /// green / (green + red); `null` with no verdict at all.
    pub success_rate: Option<f64>,
}

/// Ready-to-land latency percentiles, in seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Latency {
    /// Landed changes with a known ready time.
    pub count: usize,
    /// Median.
    pub p50: Option<f64>,
    /// 90th percentile.
    pub p90: Option<f64>,
    /// 99th percentile.
    pub p99: Option<f64>,
    /// Longest.
    pub max: Option<f64>,
}

/// Throughput metrics for a window of decision records.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stats {
    /// Window start (inclusive), RFC 3339 UTC.
    pub since: String,
    /// Window end (exclusive), RFC 3339 UTC.
    pub until: String,
    /// The records were rebuilt from run reports (`--reconstruct`).
    pub reconstructed: bool,
    /// Validations of every purpose, by purpose.
    pub validations: BTreeMap<String, usize>,
    /// Validations by mode (`run`, `shadow`, ...).
    pub modes: BTreeMap<String, usize>,
    /// Validations by validator (`local`, or the donor).
    pub validators: BTreeMap<String, usize>,
    /// Batch-width distribution.
    pub widths: Widths,
    /// Success rate by batch width.
    pub success_by_width: BTreeMap<usize, WidthOutcomes>,
    /// Wall hours of every validation.
    pub validate_hours: f64,
    /// CPU hours, over the validations whose CPU time is known.
    pub cpu_hours: f64,
    /// Validations whose CPU time is known.
    pub cpu_known: usize,
    /// Changes landed (simulated in shadow mode).
    pub landed: usize,
    /// Changes landed per validate-hour; `null` with no validation time.
    pub landed_per_validate_hour: Option<f64>,
    /// Culprits named by red batches' searches.
    pub culprits: usize,
    /// Ready-to-land latency.
    pub ready_to_land_s: Latency,
    /// What the records could not hold (reconstruction only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_reconstructable: Vec<String>,
    /// Calibration of recorded `predicted_pass` against decided outcomes;
    /// `null` when no decided record carries a prediction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<Calibration>,
    /// Red verdicts by effective red class (machine class overlaid with
    /// reclassifications by the reader); absent when no red is classified.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub red_classes: BTreeMap<String, usize>,
}

/// How honest the recorded predictions were (batch level).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Calibration {
    /// Decided records with a prediction.
    pub n: usize,
    /// Mean Brier score of the predictions.
    pub brier: f64,
    /// Brier score of predicting the window base rate throughout.
    pub base_brier: f64,
    /// Reliability bins: (lower edge, count, mean predicted, observed rate).
    pub bins: Vec<(f64, usize, f64, f64)>,
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    // Nearest rank.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// Summarise `decisions` for the window `[since, until)`.
pub fn stats(decisions: &[Decision], since: DateTime<Utc>, until: DateTime<Utc>) -> Stats {
    let mut stats = Stats {
        since: format_time(since),
        until: format_time(until),
        reconstructed: decisions.iter().any(|d| d.reconstructed),
        ..Stats::default()
    };
    let mut widths: Vec<usize> = Vec::new();
    let mut latencies: Vec<f64> = Vec::new();
    for decision in decisions {
        *stats
            .validations
            .entry(decision.purpose.clone())
            .or_default() += 1;
        *stats.modes.entry(decision.mode.clone()).or_default() += 1;
        *stats
            .validators
            .entry(
                decision
                    .validator
                    .host
                    .clone()
                    .unwrap_or_else(|| decision.validator.kind.clone()),
            )
            .or_default() += 1;
        stats.validate_hours += decision.cost.wall_s / 3600.0;
        if let Some(cpu) = decision.cost.cpu_s {
            stats.cpu_hours += cpu / 3600.0;
            stats.cpu_known += 1;
        }
        if decision.culprit.is_some() {
            stats.culprits += 1;
        }
        if let Some(class) = &decision.red_class {
            *stats.red_classes.entry(class.clone()).or_default() += 1;
        }
        if let Some(landed) = &decision.landed {
            stats.landed += landed.prs.len();
        }
        latencies.extend(decision.changes.iter().filter_map(|c| c.ready_to_land_s));
        if decision.purpose == "batch" {
            widths.push(decision.width);
            let outcomes = stats.success_by_width.entry(decision.width).or_default();
            match decision.outcome.as_str() {
                "green" => outcomes.green += 1,
                "red" => outcomes.red += 1,
                _ => outcomes.no_verdict += 1,
            }
        }
    }
    for outcomes in stats.success_by_width.values_mut() {
        let verdicts = outcomes.green + outcomes.red;
        outcomes.success_rate = (verdicts > 0).then(|| outcomes.green as f64 / verdicts as f64);
    }
    if !widths.is_empty() {
        stats.widths = Widths {
            batches: widths.len(),
            average: widths.iter().sum::<usize>() as f64 / widths.len() as f64,
            min: widths.iter().copied().min().unwrap_or(0),
            max: widths.iter().copied().max().unwrap_or(0),
            counts: widths.iter().fold(BTreeMap::new(), |mut counts, width| {
                *counts.entry(*width).or_default() += 1;
                counts
            }),
        };
    }
    let scored: Vec<(f64, f64)> = decisions
        .iter()
        .filter_map(|decision| {
            let p = decision.predicted_pass?;
            let y = match decision.outcome.as_str() {
                "green" => 1.0,
                "red" => 0.0,
                _ => return None,
            };
            Some((p, y))
        })
        .collect();
    if !scored.is_empty() {
        let base = scored.iter().map(|(_, y)| y).sum::<f64>() / scored.len() as f64;
        let metrics = crate::predict::evaluate(&scored, base);
        stats.calibration = Some(Calibration {
            n: metrics.n,
            brier: metrics.brier,
            base_brier: metrics.base_brier,
            bins: metrics.bins,
        });
    }
    stats.landed_per_validate_hour =
        (stats.validate_hours > 0.0).then(|| stats.landed as f64 / stats.validate_hours);
    latencies.sort_by(f64::total_cmp);
    stats.ready_to_land_s = Latency {
        count: latencies.len(),
        p50: percentile(&latencies, 50.0),
        p90: percentile(&latencies, 90.0),
        p99: percentile(&latencies, 99.0),
        max: latencies.last().copied(),
    };
    stats
}

fn duration(seconds: Option<f64>) -> String {
    match seconds {
        None => "-".to_owned(),
        Some(s) if s < 120.0 => format!("{s:.0}s"),
        Some(s) if s < 7200.0 => format!("{:.1}m", s / 60.0),
        Some(s) => format!("{:.1}h", s / 3600.0),
    }
}

fn counts<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
    if map.is_empty() {
        return "none".to_owned();
    }
    map.iter()
        .map(|(key, count)| format!("{key} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render [`Stats`] for people.
pub fn render(stats: &Stats) -> String {
    let mut out = format!(
        "Merge-queue stats {} .. {}{}\n",
        stats.since,
        stats.until,
        if stats.reconstructed {
            " (reconstructed from run reports)"
        } else {
            ""
        }
    );
    out.push_str(&format!(
        "  validations: {} ({}); modes: {}; validators: {}\n",
        stats.validations.values().sum::<usize>(),
        counts(&stats.validations),
        counts(&stats.modes),
        counts(&stats.validators)
    ));
    let w = &stats.widths;
    if w.batches == 0 {
        out.push_str("  batch width: no batches\n");
    } else {
        out.push_str(&format!(
            "  batch width: {} batches, average {:.2}, min {}, max {}; by width: {}\n",
            w.batches,
            w.average,
            w.min,
            w.max,
            counts(&w.counts)
        ));
    }
    for (width, outcomes) in &stats.success_by_width {
        out.push_str(&format!(
            "  width {width}: {} green, {} red, {} without verdict; success {}\n",
            outcomes.green,
            outcomes.red,
            outcomes.no_verdict,
            outcomes
                .success_rate
                .map_or("-".to_owned(), |rate| format!("{:.0}%", rate * 100.0))
        ));
    }
    out.push_str(&format!(
        "  validate time: {:.2} h wall; CPU {:.2} h over {} of {} validations with CPU time\n",
        stats.validate_hours,
        stats.cpu_hours,
        stats.cpu_known,
        stats.validations.values().sum::<usize>()
    ));
    out.push_str(&format!(
        "  landed: {} change(s), {} per validate-hour; culprits named: {}\n",
        stats.landed,
        stats
            .landed_per_validate_hour
            .map_or("-".to_owned(), |rate| format!("{rate:.2}")),
        stats.culprits
    ));
    if !stats.red_classes.is_empty() {
        out.push_str(&format!("  red classes: {}\n", counts(&stats.red_classes)));
    }
    if let Some(c) = &stats.calibration {
        out.push_str(&format!(
            "  predictions: {} decided with predicted_pass; brier {:.3} (base-rate {:.3})\n",
            c.n, c.brier, c.base_brier
        ));
        for (lo, n, mean_p, obs) in &c.bins {
            out.push_str(&format!(
                "    {:.1}-{:.1}: n={} predicted {:.2} observed {:.2}\n",
                lo,
                lo + 0.1,
                n,
                mean_p,
                obs
            ));
        }
    }
    let l = &stats.ready_to_land_s;
    out.push_str(&format!(
        "  ready to land: {} change(s); p50 {}, p90 {}, p99 {}, max {}\n",
        l.count,
        duration(l.p50),
        duration(l.p90),
        duration(l.p99),
        duration(l.max)
    ));
    if !stats.not_reconstructable.is_empty() {
        out.push_str("Not reconstructable:\n");
        for line in &stats.not_reconstructable {
            out.push_str(&format!("  - {line}\n"));
        }
    }
    out
}

/// Parse a window bound: an RFC 3339 time, or a duration before `now` such as `24h`, `90m`,
/// `7d` or `30s`.
pub fn parse_bound(text: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    if let Some(time) = parse_time(text) {
        return Ok(time);
    }
    let (number, unit) = text.split_at(text.trim_end_matches(char::is_alphabetic).len());
    let amount: i64 = number
        .parse()
        .map_err(|_| format!("{text:?}: expected an RFC 3339 time or a duration such as 24h"))?;
    let seconds = match unit {
        "s" => amount,
        "m" => amount * 60,
        "h" => amount * 3600,
        "d" => amount * 86_400,
        _ => return Err(format!("{text:?}: duration unit must be s, m, h or d")),
    };
    Ok(now - chrono::Duration::seconds(seconds))
}

#[cfg(test)]
mod unit {
    use super::*;

    fn decision(purpose: &str, width: usize, outcome: &str, wall_s: f64) -> Decision {
        Decision {
            schema: DECISION_SCHEMA.to_owned(),
            at: "2026-11-03T10:00:00Z".to_owned(),
            started_at: "2026-11-03T09:50:00Z".to_owned(),
            run: "20261103T095000Z".to_owned(),
            mode: "run".to_owned(),
            validation: 1,
            purpose: purpose.to_owned(),
            changes: Vec::new(),
            commit: "c".to_owned(),
            base: None,
            width,
            speculation: None,
            predicted_pass: None,
            validator: ValidatorUsed {
                kind: "local".to_owned(),
                host: None,
            },
            cost: Cost {
                wall_s,
                cpu_s: None,
            },
            selection: "all".to_owned(),
            red_class: None,
            outcome: outcome.to_owned(),
            failed: Vec::new(),
            culprit: None,
            landed: None,
            reconstructed: false,
        }
    }

    #[test]
    fn recent_red_rate_takes_the_newest_decided_validations() {
        let root = std::env::temp_dir().join(format!(
            "mq-decisions-red-rate-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let dir = dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let mut records = Vec::new();
        for (at, outcome) in [
            ("2026-11-01T10:00:00Z", "green"),
            ("2026-11-02T10:00:00Z", "green"),
            ("2026-11-03T10:00:00Z", "red"),
            ("2026-11-04T10:00:00Z", "red"),
            ("2026-11-05T10:00:00Z", "refused"),
        ] {
            let mut record = decision("batch", 1, outcome, 1.0);
            record.at = at.to_owned();
            records.push(serde_json::to_string(&record).unwrap());
        }
        std::fs::write(dir.join("2026-11-05.jsonl"), records.join("\n")).unwrap();
        assert_eq!(recent_red_rate(&root, 20), Some(0.5));
        assert_eq!(recent_red_rate(&root, 2), Some(1.0));
        assert_eq!(recent_red_rate(&root, 0), None);
        let empty = root.join("empty");
        assert_eq!(recent_red_rate(&empty, 20), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reclassification_overlays_without_rewriting_and_stats_counts_it() {
        let root = std::env::temp_dir().join(format!("mq-reclass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(dir(&root)).unwrap();
        let mut red = decision("batch", 1, "red", 10.0);
        red.red_class = Some("unresolved".to_owned());
        std::fs::write(
            dir(&root).join("2026-11-03.jsonl"),
            serde_json::to_string(&red).unwrap() + "\n",
        )
        .unwrap();
        append_reclassification(
            &root,
            &Reclassification {
                schema: RECLASSIFY_SCHEMA.to_owned(),
                run: red.run.clone(),
                validation: 1,
                red_class: "infra".to_owned(),
                reason: "host refused".to_owned(),
                by: "test".to_owned(),
                at: "2026-11-03T11:00:00Z".to_owned(),
            },
        )
        .unwrap();
        // The record on disk is unchanged; the reader-side overlay wins.
        let raw: Decision = serde_json::from_str(
            std::fs::read_to_string(dir(&root).join("2026-11-03.jsonl"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(raw.red_class.as_deref(), Some("unresolved"));
        let mut overlaid = vec![red];
        overlay_red_classes(&root, &mut overlaid);
        assert_eq!(overlaid[0].red_class.as_deref(), Some("infra"));
        let now = Utc::now();
        let s = stats(&overlaid, now, now);
        assert_eq!(s.red_classes.get("infra"), Some(&1));
        assert!(render(&s).contains("red classes: infra 1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stats_count_widths_success_throughput_and_latency() {
        let mut green4 = decision("batch", 4, "green", 1800.0);
        green4.landed = Some(LandedBy {
            at: None,
            prs: vec![1, 2, 3, 4],
        });
        green4.changes = (1..=4)
            .map(|pr| DecisionChange {
                pr,
                head: String::new(),
                ready_at: None,
                ready_to_land_s: Some(pr as f64 * 100.0),
                features: Features::default(),
                predicted_pass: None,
            })
            .collect();
        let mut red2 = decision("batch", 2, "red", 900.0);
        red2.culprit = Some(7);
        let decisions = vec![
            green4,
            red2,
            decision("probe", 1, "red", 450.0),
            decision("batch", 1, "refused", 450.0),
        ];
        let now = Utc::now();
        let s = stats(&decisions, now, now);
        assert_eq!(s.widths.batches, 3);
        assert_eq!(s.widths.counts, BTreeMap::from([(1, 1), (2, 1), (4, 1)]));
        assert_eq!((s.widths.min, s.widths.max), (1, 4));
        assert!((s.widths.average - 7.0 / 3.0).abs() < 1e-9);
        assert_eq!(s.success_by_width[&4].success_rate, Some(1.0));
        assert_eq!(s.success_by_width[&2].success_rate, Some(0.0));
        assert_eq!(s.success_by_width[&1].success_rate, None);
        assert!((s.validate_hours - 1.0).abs() < 1e-9);
        assert_eq!(s.landed_per_validate_hour, Some(4.0));
        assert_eq!(s.culprits, 1);
        assert_eq!(s.ready_to_land_s.count, 4);
        assert_eq!(s.ready_to_land_s.p50, Some(200.0));
        assert_eq!(s.ready_to_land_s.max, Some(400.0));
        assert!(render(&s).contains("width 2: 0 green, 1 red"));
    }

    #[test]
    fn prune_keeps_the_log_within_its_age_and_size_bounds() {
        let dir = std::env::temp_dir().join(format!(
            "mq-decisions-prune-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for day in [
            "2026-09-01",
            "2026-10-30",
            "2026-11-01",
            "2026-11-02",
            "2026-11-03",
        ] {
            std::fs::write(dir.join(format!("{day}.jsonl")), "x".repeat(100)).unwrap();
        }
        // Not day files: never pruned, never counted, never mistaken for the newest day.
        for other in ["skips.jsonl", "rulings.jsonl"] {
            std::fs::write(dir.join(other), "y".repeat(400)).unwrap();
        }
        let now = parse_time("2026-11-03T12:00:00Z").unwrap();
        let bounds = DecisionsConfig {
            keep_days: 30,
            max_bytes: 250,
        };
        prune(&dir, &bounds, now).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        // 2026-09-01 is past the age bound; then the oldest go until 250 bytes fit.
        assert_eq!(
            left,
            [
                "2026-11-02.jsonl",
                "2026-11-03.jsonl",
                "rulings.jsonl",
                "skips.jsonl"
            ]
        );
        // The newest file survives even alone over the size bound.
        let tiny = DecisionsConfig {
            keep_days: 30,
            max_bytes: 1,
        };
        prune(&dir, &tiny, now).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
        assert!(dir.join("2026-11-03.jsonl").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_takes_validation_records_from_day_files_only() {
        let root = std::env::temp_dir().join(format!(
            "mq-decisions-read-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let dir = dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        // Other records beside the day files: a skip and a ruling, which are not decisions.
        let skip = SkipRecord {
            decision: "skip".to_owned(),
            pr: 7,
            reason: "recount-pins refused".to_owned(),
            run: "20261104T000000Z".to_owned(),
            at: "2026-11-04T00:00:00Z".to_owned(),
        };
        append_skips(&root, std::slice::from_ref(&skip)).unwrap();
        let ruling = RulingRecord {
            schema: RULING_SCHEMA.to_owned(),
            ruling: "accept-flaky".to_owned(),
            prs: vec![7],
            heads: vec!["a".repeat(40)],
            commit: "b".repeat(40),
            base: "c".repeat(40),
            flaky: vec!["t".to_owned()],
            by: "owner".to_owned(),
            reason: "known flake".to_owned(),
            ruled_at: "2026-11-04T00:00:00Z".to_owned(),
            outcome: "landed".to_owned(),
            detail: String::new(),
            validated_in: "20261104T000000Z v0".to_owned(),
            run: "20261104T000100Z".to_owned(),
            at: "2026-11-04T00:01:00Z".to_owned(),
        };
        append_ruling(&root, &ruling).unwrap();
        let since = parse_time("2026-11-01T00:00:00Z").unwrap();
        let until = parse_time("2026-11-05T00:00:00Z").unwrap();
        assert_eq!(read(&root, since, until).unwrap(), Vec::new());
        assert_eq!(read_rulings(&root).unwrap(), vec![ruling]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bounds_parse_times_and_durations() {
        let now = parse_time("2026-11-03T12:00:00Z").unwrap();
        assert_eq!(
            parse_bound("24h", now).unwrap(),
            parse_time("2026-11-02T12:00:00Z").unwrap()
        );
        assert_eq!(
            parse_bound("2026-11-01T00:00:00Z", now).unwrap(),
            parse_time("2026-11-01T00:00:00Z").unwrap()
        );
        assert!(parse_bound("yesterday", now).is_err());
        assert!(parse_bound("3w", now).is_err());
        assert_eq!(component("rs/merge-queue/src/run.rs"), "rs");
        assert_eq!(component("README"), "(root)");
        assert_eq!(
            run_start("20261103T101500Z-2"),
            parse_time("2026-11-03T10:15:00Z")
        );
    }
}
