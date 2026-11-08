//! The success predictor (SubmitQueue, EuroSys '19, §4.2/§7.2): a regularised
//! logistic regression over decision-record features, trained on the queue's
//! own history. It writes a calibrated `predicted_pass` per change and, where
//! pair data allows, a pairwise conflict probability `P_conf`. Nothing here
//! schedules anything (that is the expected-value planner); this module only
//! estimates probabilities and reports how honest they are (reliability bins,
//! Brier score, a chronological holdout). Enabled only by `Config::predict`;
//! off, the decision log keeps the base-rate prior it has today.
//!
//! Training and serving share one vector construction, [`feature_vector`]:
//! the change-local features come from the change itself and the history
//! features from a [`History`] folded only from earlier decisions, so a
//! first submission is scored from its own size, components and claims
//! rather than the model base rate (#1056 predict-first-submission).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::decisions::{Decision, Features};
use crate::model::Item;

/// Persisted model schema.
pub const MODEL_SCHEMA: &str = "merge-queue-model/v1";

/// Feature names, in vector order (the intercept is separate).
pub const FEATURES: [&str; 16] = [
    "log_lines",
    "log_files",
    "n_components",
    "conflict_degree",
    "peripheral",
    "prioritized",
    "ci_only",
    "claim_complete",
    "claim_at_head",
    "log_prior_validated",
    "prior_cleared_share",
    "prior_culprit",
    "log_resubmissions",
    "author_rate",
    "component_rate",
    "log_width",
];

/// One training example: a change inside one decided validation.
#[derive(Clone, Debug)]
pub struct Example {
    /// RFC 3339 end time of the validation (chronological split key).
    pub at: String,
    /// Change number.
    pub pr: i64,
    /// Raw (unstandardised) feature vector, [`FEATURES`] order.
    pub x: Vec<f64>,
    /// 1.0 = passed, 0.0 = failed.
    pub y: f64,
    /// Sample weight (attributed examples weigh 1, shared batch labels 1/width).
    pub w: f64,
}

/// Calibration and discrimination of a scored set.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Scored examples.
    pub n: usize,
    /// Mean Brier score (lower is better; base rate scores ~p(1-p)).
    pub brier: f64,
    /// Mean log loss.
    pub log_loss: f64,
    /// Area under the ROC curve (0.5 = chance).
    pub auc: f64,
    /// Brier score of always predicting the training base rate.
    pub base_brier: f64,
    /// Reliability bins: (lower edge, count, mean predicted, observed rate).
    pub bins: Vec<(f64, usize, f64, f64)>,
}

/// A trained model, persisted as `state/model.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    /// [`MODEL_SCHEMA`].
    pub schema: String,
    /// RFC 3339 training time.
    pub trained_at: String,
    /// Decided change-instances trained on (before the holdout split).
    pub samples: usize,
    /// Change-instances excluded by red-class label hygiene (reds adjudicated
    /// `infra` / `false-red` / `host`); 0 when no reclassification exists.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub excluded_labels: usize,
    /// Overall green share of the training labels.
    pub base_rate: f64,
    /// Feature means used for standardisation.
    pub means: Vec<f64>,
    /// Feature standard deviations used for standardisation.
    pub sds: Vec<f64>,
    /// Logistic weights, [`FEATURES`] order.
    pub weights: Vec<f64>,
    /// Logistic intercept (on standardised features).
    pub intercept: f64,
    /// Pairwise conflict rates by `low:high` change-number pair: (rate, samples).
    /// Only pairs with enough co-validation history appear; absence means
    /// "no data", never "no conflict".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pair_conf: BTreeMap<String, (f64, usize)>,
    /// Holdout metrics from training (chronological last 30%).
    pub holdout: Metrics,
    /// Metrics on the training split itself.
    pub train_metrics: Metrics,
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

/// Where the persisted model lives inside a state root.
pub fn path(state_root: &Path) -> PathBuf {
    state_root.join("model.json")
}

/// Load the persisted model, if one has been trained.
pub fn load(state_root: &Path) -> Option<Model> {
    let text = std::fs::read_to_string(path(state_root)).ok()?;
    serde_json::from_str(&text).ok()
}

fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        1.0 / (1.0 + (-z).exp())
    } else {
        let e = z.exp();
        e / (1.0 + e)
    }
}

impl Model {
    /// Predicted pass probability for one raw feature vector.
    pub fn predict_raw(&self, x: &[f64]) -> f64 {
        let mut z = self.intercept;
        for (i, w) in self.weights.iter().enumerate() {
            let sd = self.sds.get(i).copied().unwrap_or(1.0).max(1e-9);
            let mean = self.means.get(i).copied().unwrap_or(0.0);
            z += w * ((x.get(i).copied().unwrap_or(0.0) - mean) / sd);
        }
        sigmoid(z).clamp(0.01, 0.99)
    }

    /// `P_conf(a, b)` where pair data allows; `None` means no data.
    pub fn p_conf(&self, a: i64, b: i64) -> Option<f64> {
        let key = pair_key(a, b);
        self.pair_conf.get(&key).map(|(rate, _)| *rate)
    }

    /// Predicted probability that a batch of changes all passes: the product
    /// of per-change probabilities, discounted per pair by `(1 - P_conf)`
    /// where pair data exists (the paper's Eq. 5 shape, §4.2.1).
    pub fn batch_pass(&self, ps: &[(i64, f64)]) -> f64 {
        let mut p: f64 = ps.iter().map(|(_, p)| *p).product();
        for i in 0..ps.len() {
            for j in (i + 1)..ps.len() {
                if let Some(conf) = self.p_conf(ps[i].0, ps[j].0) {
                    p *= 1.0 - conf;
                }
            }
        }
        p.clamp(0.01, 0.99)
    }

    /// Ranked plain-language explanation: (feature, weight, direction).
    pub fn explain(&self) -> Vec<(&'static str, f64)> {
        let mut rows: Vec<(&'static str, f64)> = FEATURES
            .iter()
            .zip(self.weights.iter())
            .map(|(name, w)| (*name, *w))
            .collect();
        rows.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows
    }
}

fn pair_key(a: i64, b: i64) -> String {
    if a < b {
        format!("{a}:{b}")
    } else {
        format!("{b}:{a}")
    }
}

/// Per-change history the features read, built only from records strictly
/// before the scored validation (no leakage from the future). Training
/// folds each decision in only after scoring it; serving folds every
/// decision recorded before the run whose records are being written.
#[derive(Clone, Debug, Default)]
pub struct History {
    validated: BTreeMap<i64, u64>,
    cleared_share: BTreeMap<i64, (u64, u64)>, // (cleared, decided)
    culprit: BTreeMap<i64, u64>,
    author_rate: BTreeMap<String, (u64, u64)>, // (green, decided) by submitter
    component_rate: BTreeMap<String, (u64, u64)>,
    decided: u64,
    green: u64,
}

impl History {
    /// Green share of the decided validations folded in (0.5 with none).
    pub fn global(&self) -> f64 {
        if self.decided == 0 {
            0.5
        } else {
            self.green as f64 / self.decided as f64
        }
    }
    fn shrunk(rate: Option<&(u64, u64)>, global: f64) -> f64 {
        // Shrink small samples toward the global rate (prior strength 4).
        match rate {
            Some((g, n)) => (*g as f64 + 4.0 * global) / (*n as f64 + 4.0),
            None => global,
        }
    }

    /// Fold one decided (green/red) decision in, after it has been scored.
    fn fold(&mut self, decision: &Decision, items: &BTreeMap<i64, Item>) {
        let batch_green = decision.outcome == "green";
        self.decided += 1;
        self.green += u64::from(batch_green);
        for change in &decision.changes {
            *self.validated.entry(change.pr).or_default() += 1;
            let slot = self.cleared_share.entry(change.pr).or_default();
            slot.1 += 1;
            slot.0 += u64::from(batch_green);
            if decision.culprit == Some(change.pr) {
                *self.culprit.entry(change.pr).or_default() += 1;
            }
            let author = items
                .get(&change.pr)
                .map(|item| item.submitted_by.clone())
                .unwrap_or_default();
            let slot = self.author_rate.entry(author).or_default();
            slot.1 += 1;
            slot.0 += u64::from(batch_green);
            if let Some(component) = change.features.components.first() {
                let slot = self.component_rate.entry(component.clone()).or_default();
                slot.1 += 1;
                slot.0 += u64::from(batch_green);
            }
        }
    }

    /// The snapshot from folding every decided decision in `decisions`,
    /// oldest first: the history a validation scored after all of them sees.
    pub fn from_decisions(decisions: &[Decision], items: &BTreeMap<i64, Item>) -> Self {
        let mut sorted: Vec<&Decision> = decisions
            .iter()
            .filter(|d| d.outcome == "green" || d.outcome == "red")
            .collect();
        sorted.sort_by(|a, b| a.at.cmp(&b.at));
        let mut history = Self::default();
        for decision in sorted {
            history.fold(decision, items);
        }
        history
    }
}

/// The one raw feature vector for a change, in [`FEATURES`] order, shared by
/// training ([`examples`]) and serving ([`ServeContext::vector`]) so the two
/// cannot skew. The first nine features come from the change itself; the
/// rest read `history`, `author` and `resubmissions`. A change with no
/// history gets the neutral values training also used for it: prior
/// validated and culprit counts 0, prior cleared share and the shrunk
/// author/component rates at the snapshot global rate, and `resubmissions`
/// as counted by [`resubmissions_before`] (0 on a first submission).
/// New features slot in here only, never in a caller.
pub fn feature_vector(
    features: &Features,
    history: &History,
    pr: i64,
    author: &str,
    resubmissions: u64,
    width: usize,
) -> Vec<f64> {
    let global = history.global();
    let (cleared, decided) = history.cleared_share.get(&pr).copied().unwrap_or((0, 0));
    vec![
        ((features.lines_added.unwrap_or(0) + features.lines_deleted.unwrap_or(0)) as f64 + 1.0)
            .ln(),
        (features.files.unwrap_or(0) as f64 + 1.0).ln(),
        features.components.len() as f64,
        features.conflict_degree.unwrap_or(0) as f64,
        f64::from(u8::from(features.peripheral)),
        f64::from(u8::from(features.prioritized)),
        f64::from(u8::from(features.ci_only)),
        f64::from(u8::from(
            features
                .claim
                .as_ref()
                .is_some_and(|c| c.scope == "complete"),
        )),
        f64::from(u8::from(features.claim.as_ref().is_some_and(|c| c.at_head))),
        (history.validated.get(&pr).copied().unwrap_or(0) as f64 + 1.0).ln(),
        if decided == 0 {
            global
        } else {
            cleared as f64 / decided as f64
        },
        history.culprit.get(&pr).copied().unwrap_or(0) as f64,
        (resubmissions as f64 + 1.0).ln(),
        History::shrunk(history.author_rate.get(author), global),
        History::shrunk(
            features
                .components
                .first()
                .and_then(|c| history.component_rate.get(c)),
            global,
        ),
        (width.max(1) as f64).ln(),
    ]
}

/// Resubmissions of one change before `before` (an RFC 3339 validation end):
/// its `head` signals stamped strictly before then, minus the first head.
/// A record with no `at` counts as before (only hand-built fixtures lack
/// one; the log always stamps it). Training and serving both count this
/// way, so a head submitted after a validation never changes its vector.
pub fn resubmissions_before(records: &[serde_json::Value], before: &str) -> u64 {
    let heads = records
        .iter()
        .filter(|r| r.get("kind").and_then(|v| v.as_str()) == Some("head"))
        .filter(|r| {
            r.get("at")
                .and_then(|v| v.as_str())
                .is_none_or(|at| at < before)
        })
        .count();
    heads.saturating_sub(1) as u64
}

/// Serving inputs for scoring changes in a new batch: the [`History`]
/// snapshot plus each change's `head`-signal times, so
/// `decisions::from_report` can build every change's vector from the
/// [`Features`] it computes for that change (see
/// `decisions::Sources::model_context`). The snapshot is the state before
/// the scored run's own decisions are appended, fixed for the whole run:
/// unlike training, which folds each decision before later ones, a served
/// prediction never reads an outcome from its own run.
#[derive(Clone, Debug, Default)]
pub struct ServeContext {
    history: History,
    // `head` signal times per change (empty string when unstamped, which
    // sorts before any validation end; see `resubmissions_before`).
    head_times: BTreeMap<i64, Vec<String>>,
}

impl ServeContext {
    /// From a folded snapshot and the signal records it was folded beside.
    pub fn new(history: History, signals: &BTreeMap<i64, Vec<serde_json::Value>>) -> Self {
        let head_times = signals
            .iter()
            .map(|(pr, records)| {
                let times = records
                    .iter()
                    .filter(|r| r.get("kind").and_then(|v| v.as_str()) == Some("head"))
                    .map(|r| {
                        r.get("at")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .collect();
                (*pr, times)
            })
            .collect();
        Self {
            history,
            head_times,
        }
    }

    /// From a state root, reading its item records from disk.
    pub fn from_state(state_root: &Path) -> Self {
        Self::from_state_with_items(state_root, &read_items(state_root))
    }

    /// From a state root's decisions and signals, folding author history
    /// through the caller's `items` (the same map serving looks authors up
    /// in, so the two cannot disagree).
    pub fn from_state_with_items(state_root: &Path, items: &BTreeMap<i64, Item>) -> Self {
        let decisions = read_decisions(state_root).unwrap_or_default();
        let mut signals = BTreeMap::new();
        for pr in crate::signal::prs(state_root) {
            if let Ok(records) = crate::signal::read(state_root, pr) {
                signals.insert(pr, records);
            }
        }
        Self::new(History::from_decisions(&decisions, items), &signals)
    }

    /// The snapshot this context scores against.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The submit-time vector for one change in a batch of `width`, as of
    /// the validation ending at `before`: [`feature_vector`] over the
    /// change's own `features`, the snapshot, and its resubmissions before
    /// then. For a change already decided at `before`, feeding the history
    /// folded from the decisions before it reproduces its training vector.
    pub fn vector(
        &self,
        features: &Features,
        pr: i64,
        author: &str,
        width: usize,
        before: &str,
    ) -> Vec<f64> {
        let heads = self
            .head_times
            .get(&pr)
            .map(|times| times.iter().filter(|at| at.as_str() < before).count())
            .unwrap_or(0);
        feature_vector(
            features,
            &self.history,
            pr,
            author,
            heads.saturating_sub(1) as u64,
            width,
        )
    }
}

/// Build training examples from decided decisions, walking chronologically so
/// every history feature sees only the past. Batch outcome is the shared
/// label (weight 1/width); signal attribution, when present for the exact
/// (run, validation), overrides it at full weight.
pub fn examples(
    decisions: &[Decision],
    signals: &BTreeMap<i64, Vec<serde_json::Value>>,
    items: &BTreeMap<i64, Item>,
    red_classes: &BTreeMap<(String, usize), String>,
) -> Vec<Example> {
    // Attribution by (pr, run, validation) from the signal log.
    let mut attribution: BTreeMap<(i64, String, usize), String> = BTreeMap::new();
    for (pr, records) in signals {
        for record in records {
            if record.get("kind").and_then(|v| v.as_str()) != Some("validated") {
                continue;
            }
            let (Some(run), Some(validation), Some(kind)) = (
                record.get("run").and_then(|v| v.as_str()),
                record.get("validation").and_then(|v| v.as_u64()),
                record.get("attribution").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            attribution.insert((*pr, run.to_owned(), validation as usize), kind.to_owned());
        }
    }
    let mut sorted: Vec<&Decision> = decisions
        .iter()
        .filter(|d| d.outcome == "green" || d.outcome == "red")
        .collect();
    sorted.sort_by(|a, b| a.at.cmp(&b.at));

    let mut history = History::default();
    let mut out = Vec::new();
    for decision in &sorted {
        // Label hygiene: reds adjudicated infra / false-red / host are not
        // evidence about the change; they never become training examples
        // (composition reds stay: they are the P_conf signal). History
        // folding below still counts the validation as taken part in.
        let effective_class = red_classes
            .get(&(decision.run.clone(), decision.validation))
            .cloned()
            .or_else(|| decision.red_class.clone());
        let excluded = decision.outcome == "red"
            && matches!(
                effective_class.as_deref(),
                Some("infra" | "false-red" | "host")
            );
        let batch_green = decision.outcome == "green";
        for change in &decision.changes {
            let author = items
                .get(&change.pr)
                .map(|item| item.submitted_by.as_str())
                .unwrap_or_default();
            let resubmissions = signals
                .get(&change.pr)
                .map(|records| resubmissions_before(records, &decision.at))
                .unwrap_or(0);
            let x = feature_vector(
                &change.features,
                &history,
                change.pr,
                author,
                resubmissions,
                decision.width,
            );
            let (y, w) = match attribution
                .get(&(change.pr, decision.run.clone(), decision.validation))
                .map(String::as_str)
            {
                Some("culprit") => (0.0, 1.0),
                Some("cleared") => (1.0, 1.0),
                _ => (
                    f64::from(u8::from(batch_green)),
                    1.0 / decision.width.max(1) as f64,
                ),
            };
            if !excluded {
                out.push(Example {
                    at: decision.at.clone(),
                    pr: change.pr,
                    x,
                    y,
                    w,
                });
            }
        }
        // Fold this decision into history only after scoring all its changes.
        history.fold(decision, items);
    }
    out
}

/// Pairwise conflict rates: a pair conflicts when a batch containing both went
/// red while each member was cleared in some other validation (alone or in a
/// green batch). Green co-validations count against. Pairs need at least 3
/// co-validations and the whole table at least 5 labelled pairs.
fn pair_rates(
    decisions: &[Decision],
    red_classes: &BTreeMap<(String, usize), String>,
) -> BTreeMap<String, (f64, usize)> {
    let mut cleared_elsewhere: BTreeMap<i64, bool> = BTreeMap::new();
    for d in decisions {
        if d.outcome == "green" {
            for c in &d.changes {
                cleared_elsewhere.insert(c.pr, true);
            }
        }
    }
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // (conflicts, total)
    for d in decisions {
        if d.outcome != "green" && d.outcome != "red" {
            continue;
        }
        // Infra/false/host reds say nothing about the pair either.
        let class = red_classes
            .get(&(d.run.clone(), d.validation))
            .map(String::as_str)
            .or(d.red_class.as_deref());
        if matches!(class, Some("infra" | "false-red" | "host")) {
            continue;
        }
        let prs: Vec<i64> = d.changes.iter().map(|c| c.pr).collect();
        for i in 0..prs.len() {
            for j in (i + 1)..prs.len() {
                let slot = counts.entry(pair_key(prs[i], prs[j])).or_default();
                slot.1 += 1;
                let conflict = d.outcome == "red"
                    && cleared_elsewhere.get(&prs[i]).copied().unwrap_or(false)
                    && cleared_elsewhere.get(&prs[j]).copied().unwrap_or(false);
                slot.0 += usize::from(conflict);
            }
        }
    }
    let labelled = counts.values().filter(|(_, n)| *n >= 3).count();
    if labelled < 5 {
        return BTreeMap::new();
    }
    counts
        .into_iter()
        .filter(|(_, (_, n))| *n >= 3)
        .map(|(k, (c, n))| (k, ((c as f64 + 1.0) / (n as f64 + 4.0), n)))
        .collect()
}

/// Score predictions against labels: calibration bins, Brier, log loss, AUC.
pub fn evaluate(scored: &[(f64, f64)], base_rate: f64) -> Metrics {
    if scored.is_empty() {
        return Metrics::default();
    }
    let n = scored.len() as f64;
    let brier = scored.iter().map(|(p, y)| (p - y) * (p - y)).sum::<f64>() / n;
    let base_brier = scored
        .iter()
        .map(|(_, y)| (base_rate - y) * (base_rate - y))
        .sum::<f64>()
        / n;
    let log_loss = scored
        .iter()
        .map(|(p, y)| {
            let p = p.clamp(1e-9, 1.0 - 1e-9);
            -(y * p.ln() + (1.0 - y) * (1.0 - p).ln())
        })
        .sum::<f64>()
        / n;
    // AUC by pairwise comparison (sets here are small).
    let (mut wins, mut pairs) = (0.0f64, 0usize);
    for (p1, y1) in scored {
        for (p2, y2) in scored {
            if *y1 > *y2 {
                pairs += 1;
                wins += if p1 > p2 {
                    1.0
                } else if p1 == p2 {
                    0.5
                } else {
                    0.0
                };
            }
        }
    }
    let auc = if pairs == 0 { 0.5 } else { wins / pairs as f64 };
    let mut bins = Vec::new();
    for b in 0..10 {
        let lo = b as f64 / 10.0;
        let hi = lo + 0.1;
        let in_bin: Vec<&(f64, f64)> = scored
            .iter()
            .filter(|(p, _)| *p >= lo && (b == 9 || *p < hi))
            .collect();
        if in_bin.is_empty() {
            continue;
        }
        let mean_p = in_bin.iter().map(|(p, _)| p).sum::<f64>() / in_bin.len() as f64;
        let obs = in_bin.iter().map(|(_, y)| y).sum::<f64>() / in_bin.len() as f64;
        bins.push((lo, in_bin.len(), mean_p, obs));
    }
    Metrics {
        n: scored.len(),
        brier,
        log_loss,
        auc,
        base_brier,
        bins,
    }
}

/// Train on examples (already chronological). The last 30% by time is the
/// holdout when at least 20 examples exist; otherwise everything trains and
/// the holdout reports the training fit, flagged by equal sample counts.
pub fn train(examples: &[Example], trained_at: &str) -> Result<Model, String> {
    if examples.len() < 10 {
        return Err(format!(
            "only {} decided change-instances; at least 10 are needed to train",
            examples.len()
        ));
    }
    let split = if examples.len() >= 20 {
        examples.len() * 7 / 10
    } else {
        examples.len()
    };
    let (train_set, holdout_set) = examples.split_at(split);
    let dims = FEATURES.len();
    let mut means = vec![0.0; dims];
    let mut sds = vec![1.0; dims];
    for (i, mean) in means.iter_mut().enumerate() {
        *mean = train_set.iter().map(|e| e.x[i] * e.w).sum::<f64>()
            / train_set.iter().map(|e| e.w).sum::<f64>().max(1e-9);
    }
    for (i, sd) in sds.iter_mut().enumerate() {
        let var = train_set
            .iter()
            .map(|e| e.w * (e.x[i] - means[i]) * (e.x[i] - means[i]))
            .sum::<f64>()
            / train_set.iter().map(|e| e.w).sum::<f64>().max(1e-9);
        *sd = var.sqrt().max(1e-9);
    }
    let base_rate = train_set.iter().map(|e| e.y * e.w).sum::<f64>()
        / train_set.iter().map(|e| e.w).sum::<f64>().max(1e-9);
    // Gradient descent on the weighted logistic loss, L2 = 1.0.
    let mut weights = vec![0.0f64; dims];
    let mut intercept =
        (base_rate.clamp(0.01, 0.99)).ln() - ((1.0 - base_rate.clamp(0.01, 0.99)).ln());
    let total_w: f64 = train_set.iter().map(|e| e.w).sum::<f64>().max(1e-9);
    for _ in 0..800 {
        let mut grad_w = vec![0.0f64; dims];
        let mut grad_b = 0.0f64;
        for e in train_set {
            let mut z = intercept;
            for i in 0..dims {
                z += weights[i] * (e.x[i] - means[i]) / sds[i];
            }
            let err = (sigmoid(z) - e.y) * e.w;
            grad_b += err;
            for i in 0..dims {
                grad_w[i] += err * (e.x[i] - means[i]) / sds[i];
            }
        }
        intercept -= 0.1 * grad_b / total_w;
        for i in 0..dims {
            weights[i] -= 0.1 * (grad_w[i] / total_w + weights[i] / total_w);
        }
    }
    let mut model = Model {
        schema: MODEL_SCHEMA.to_owned(),
        trained_at: trained_at.to_owned(),
        samples: examples.len(),
        excluded_labels: 0,
        base_rate,
        means,
        sds,
        weights,
        intercept,
        pair_conf: BTreeMap::new(),
        holdout: Metrics::default(),
        train_metrics: Metrics::default(),
    };
    let train_scored: Vec<(f64, f64)> = train_set
        .iter()
        .map(|e| (model.predict_raw(&e.x), e.y))
        .collect();
    let holdout_scored: Vec<(f64, f64)> = holdout_set
        .iter()
        .map(|e| (model.predict_raw(&e.x), e.y))
        .collect();
    model.train_metrics = evaluate(&train_scored, base_rate);
    model.holdout = if holdout_set.is_empty() {
        model.train_metrics.clone()
    } else {
        evaluate(&holdout_scored, base_rate)
    };
    Ok(model)
}

/// Train from a state root: decisions + signals + items, persist, return.
pub fn train_from_state(state_root: &Path) -> Result<Model, String> {
    let decisions = read_decisions(state_root)?;
    let mut signals = BTreeMap::new();
    for pr in crate::signal::prs(state_root) {
        signals.insert(pr, crate::signal::read(state_root, pr)?);
    }
    let items = read_items(state_root);
    let red_classes = crate::decisions::effective_red_classes(state_root, &decisions);
    let examples = examples(&decisions, &signals, &items, &red_classes);
    let excluded_labels = decisions
        .iter()
        .filter(|d| {
            d.outcome == "red"
                && matches!(
                    red_classes
                        .get(&(d.run.clone(), d.validation))
                        .map(String::as_str)
                        .or(d.red_class.as_deref()),
                    Some("infra" | "false-red" | "host")
                )
        })
        .map(|d| d.changes.len())
        .sum();
    let mut model = train(&examples, &crate::model::now())?;
    model.excluded_labels = excluded_labels;
    model.pair_conf = pair_rates(&decisions, &red_classes);
    let text = serde_json::to_string_pretty(&model).map_err(|e| e.to_string())?;
    std::fs::write(path(state_root), text)
        .map_err(|e| format!("cannot write {}: {e}", path(state_root).display()))?;
    Ok(model)
}

/// All decision records under a state root, oldest files first.
pub fn read_decisions(state_root: &Path) -> Result<Vec<Decision>, String> {
    let dir = crate::decisions::dir(state_root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.ends_with(".jsonl") && n != "rulings.jsonl" && n != "skips.jsonl"
            })
        })
        .collect();
    files.sort();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        for line in text.lines() {
            if let Ok(decision) = serde_json::from_str::<Decision>(line) {
                out.push(decision);
            }
        }
    }
    crate::decisions::overlay_red_classes(state_root, &mut out);
    Ok(out)
}

fn read_items(state_root: &Path) -> BTreeMap<i64, Item> {
    let mut out = BTreeMap::new();
    let dir = state_root.join("items");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Ok(item) = serde_json::from_str::<Item>(&text) {
                    out.insert(item.pr, item);
                }
            }
        }
    }
    out
}

/// Latest training feature vector per change known to a state root: each
/// change's vector as of its last decided validation. Only the
/// expected-value planner reads this; decision records are scored from the
/// change itself through [`ServeContext::vector`], because a first
/// submission has no decided validation to take a vector from.
pub fn latest_vectors(state_root: &Path) -> BTreeMap<i64, Vec<f64>> {
    let Ok(decisions) = read_decisions(state_root) else {
        return BTreeMap::new();
    };
    let mut signals = BTreeMap::new();
    for pr in crate::signal::prs(state_root) {
        if let Ok(records) = crate::signal::read(state_root, pr) {
            signals.insert(pr, records);
        }
    }
    let items = read_items(state_root);
    let red_classes = crate::decisions::effective_red_classes(state_root, &decisions);
    let mut out: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for example in examples(&decisions, &signals, &items, &red_classes) {
        out.insert(example.pr, example.x);
    }
    out
}

/// Human-readable model report (for `model show` / `model eval`).
pub fn render(model: &Model) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "model {} trained {} on {} decided change-instances (base rate {:.2})\n",
        model.schema, model.trained_at, model.samples, model.base_rate
    ));
    if model.excluded_labels > 0 {
        out.push_str(&format!(
            "label hygiene: {} change-instance(s) excluded as infra/false-red/host reds\n",
            model.excluded_labels
        ));
    }
    out.push_str("features by |weight| (positive raises predicted pass):\n");
    for (name, w) in model.explain() {
        out.push_str(&format!("  {name:<22} {w:+.3}\n"));
    }
    let render_metrics = |label: &str, m: &Metrics| {
        let mut s = format!(
            "{label}: n={} brier {:.3} (base-rate {:.3}) log-loss {:.3} auc {:.2}\n",
            m.n, m.brier, m.base_brier, m.log_loss, m.auc
        );
        s.push_str("  reliability bins [pred range: n, mean predicted, observed]:\n");
        for (lo, n, mean_p, obs) in &m.bins {
            s.push_str(&format!(
                "    {:.1}-{:.1}: n={} predicted {:.2} observed {:.2}\n",
                lo,
                lo + 0.1,
                n,
                mean_p,
                obs
            ));
        }
        s
    };
    out.push_str(&render_metrics("train", &model.train_metrics));
    out.push_str(&render_metrics(
        "holdout (chronological last 30%)",
        &model.holdout,
    ));
    if model.pair_conf.is_empty() {
        out.push_str("pair conflicts: no pair has enough co-validation history yet\n");
    } else {
        out.push_str(&format!(
            "pair conflicts: {} pairs with P_conf data\n",
            model.pair_conf.len()
        ));
        for (pair, (rate, n)) in &model.pair_conf {
            out.push_str(&format!("  {}: P_conf {:.2} (n={})\n", pair, rate, n));
        }
    }
    out
}

#[cfg(test)]
mod unit {
    use super::*;

    fn example(x: Vec<f64>, y: f64) -> Example {
        Example {
            at: "2026-01-01T00:00:00Z".to_owned(),
            pr: 1,
            x,
            y,
            w: 1.0,
        }
    }

    #[test]
    fn trainer_recovers_the_signal_direction() {
        // Feature 0 (log_lines) strongly negative, feature 4 (peripheral) positive.
        let mut set = Vec::new();
        for i in 0..60 {
            let big = i % 2 == 0;
            let mut x = vec![0.0; FEATURES.len()];
            x[0] = if big { 6.0 } else { 1.0 };
            x[4] = f64::from(u8::from(!big));
            set.push(example(x, if big { 0.0 } else { 1.0 }));
        }
        let model = train(&set, "2026-01-02T00:00:00Z").unwrap();
        assert!(model.weights[0] < 0.0, "lines weight {:?}", model.weights);
        assert!(
            model.weights[4] > 0.0,
            "peripheral weight {:?}",
            model.weights
        );
        assert!(model.holdout.brier < model.holdout.base_brier);
        assert!(model.predict_raw(&set[1].x) > model.predict_raw(&set[0].x));
    }

    #[test]
    fn calibration_bins_and_brier_are_honest() {
        let scored: Vec<(f64, f64)> = vec![(0.9, 1.0), (0.9, 1.0), (0.1, 0.0), (0.1, 0.0)];
        let m = evaluate(&scored, 0.5);
        assert!((m.brier - 0.01).abs() < 1e-9);
        assert_eq!(m.bins.len(), 2);
        assert!((m.auc - 1.0).abs() < 1e-9);
    }

    #[test]
    fn batch_pass_discounts_known_conflicts_only() {
        let mut model = train(
            &(0..20)
                .map(|i| example(vec![0.0; FEATURES.len()], f64::from(u8::from(i % 2 == 0))))
                .collect::<Vec<_>>(),
            "2026-01-02T00:00:00Z",
        )
        .unwrap();
        let plain = model.batch_pass(&[(1, 0.8), (2, 0.8)]);
        assert!((plain - 0.64).abs() < 1e-9);
        model.pair_conf.insert("1:2".to_owned(), (0.5, 5));
        let discounted = model.batch_pass(&[(1, 0.8), (2, 0.8)]);
        assert!((discounted - 0.32).abs() < 1e-9);
        assert_eq!(model.p_conf(2, 1), Some(0.5));
        assert_eq!(model.p_conf(1, 3), None);
    }

    #[test]
    fn infra_reds_are_excluded_but_composition_reds_train() {
        let mk = |run: &str, outcome: &str, class: Option<&str>| crate::decisions::Decision {
            schema: crate::decisions::DECISION_SCHEMA.to_owned(),
            at: "2026-01-01T00:00:00Z".to_owned(),
            started_at: "2026-01-01T00:00:00Z".to_owned(),
            run: run.to_owned(),
            mode: "run".to_owned(),
            validation: 1,
            purpose: "batch".to_owned(),
            changes: vec![crate::decisions::DecisionChange {
                pr: 1,
                head: "h".to_owned(),
                ready_at: None,
                ready_to_land_s: None,
                features: crate::decisions::Features::default(),
                predicted_pass: None,
            }],
            commit: "c".to_owned(),
            base: None,
            width: 1,
            speculation: None,
            predicted_pass: None,
            validator: crate::decisions::ValidatorUsed::default(),
            cost: crate::decisions::Cost::default(),
            selection: "all".to_owned(),
            outcome: outcome.to_owned(),
            red_class: class.map(str::to_owned),
            failed: Vec::new(),
            culprit: None,
            landed: None,
            reconstructed: false,
        };
        let decisions = vec![
            mk("r1", "red", Some("infra")),
            mk("r2", "red", Some("composition")),
            mk("r3", "green", None),
        ];
        let examples = examples(
            &decisions,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            examples.len(),
            2,
            "infra excluded, composition + green kept"
        );
        assert!(examples.iter().any(|e| e.y == 0.0));
    }

    fn sized_features(lines: u64, files: usize, peripheral: bool) -> Features {
        Features {
            lines_added: Some(lines),
            lines_deleted: Some(0),
            files: Some(files),
            components: vec!["rs".to_owned()],
            peripheral,
            ..Features::default()
        }
    }

    fn decided(at: &str, pr: i64, outcome: &str, features: Features) -> Decision {
        Decision {
            schema: crate::decisions::DECISION_SCHEMA.to_owned(),
            at: at.to_owned(),
            started_at: at.to_owned(),
            run: format!("run-{at}"),
            mode: "run".to_owned(),
            validation: 1,
            purpose: "batch".to_owned(),
            changes: vec![crate::decisions::DecisionChange {
                pr,
                head: "h".to_owned(),
                ready_at: None,
                ready_to_land_s: None,
                features,
                predicted_pass: None,
            }],
            commit: "c".to_owned(),
            base: None,
            width: 1,
            speculation: None,
            predicted_pass: None,
            validator: crate::decisions::ValidatorUsed::default(),
            cost: crate::decisions::Cost::default(),
            selection: "all".to_owned(),
            outcome: outcome.to_owned(),
            red_class: None,
            failed: Vec::new(),
            culprit: None,
            landed: None,
            reconstructed: false,
        }
    }

    #[test]
    fn submit_time_vector_equals_training_vector_for_a_decided_change() {
        let decisions = vec![
            decided(
                "2026-01-01T00:00:00Z",
                7,
                "green",
                sized_features(10, 2, true),
            ),
            decided(
                "2026-01-02T00:00:00Z",
                8,
                "red",
                sized_features(900, 40, false),
            ),
            decided(
                "2026-01-03T00:00:00Z",
                7,
                "green",
                sized_features(20, 3, true),
            ),
        ];
        // Two heads before the target validation (one resubmission) and one
        // after it, which neither construction may count.
        let signals = BTreeMap::from([(
            7,
            vec![
                serde_json::json!({"kind": "head", "at": "2025-12-31T00:00:00Z"}),
                serde_json::json!({"kind": "head", "at": "2026-01-02T12:00:00Z"}),
                serde_json::json!({"kind": "head", "at": "2026-01-04T00:00:00Z"}),
            ],
        )]);
        let items = BTreeMap::new();
        let trained = examples(&decisions, &signals, &items, &BTreeMap::new());
        assert_eq!(trained.len(), 3);
        for (i, target) in decisions.iter().enumerate() {
            let context =
                ServeContext::new(History::from_decisions(&decisions[..i], &items), &signals);
            let change = &target.changes[0];
            let served = context.vector(&change.features, change.pr, "", target.width, &target.at);
            let training = trained
                .iter()
                .find(|e| e.pr == change.pr && e.at == target.at)
                .unwrap();
            assert_eq!(
                served, training.x,
                "vector for {} at {}",
                change.pr, target.at
            );
        }
        let target = &trained[2];
        assert_eq!(target.x[12], 2.0f64.ln(), "one resubmission before it");
    }

    #[test]
    fn first_submissions_spread_off_the_base_rate_and_score_better_than_it() {
        // Every change appears once, so under the old serving path each was
        // a first submission with no history vector and got the base rate.
        let decisions: Vec<Decision> = (0..60)
            .map(|i| {
                let big = i % 2 == 0;
                let at = format!("2026-{:02}-{:02}T00:00:00Z", 1 + i / 28, 1 + i % 28);
                decided(
                    &at,
                    100 + i as i64,
                    if big { "red" } else { "green" },
                    if big {
                        sized_features(4000, 60, false)
                    } else {
                        sized_features(5, 1, true)
                    },
                )
            })
            .collect();
        let items = BTreeMap::new();
        let signals = BTreeMap::new();
        let model = train(
            &examples(&decisions, &signals, &items, &BTreeMap::new()),
            "2026-03-01T00:00:00Z",
        )
        .unwrap();
        // Serve each change as of just before its own decision (the model
        // weights are in-sample; the history each vector reads is causal).
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (i, decision) in decisions.iter().enumerate() {
            let context =
                ServeContext::new(History::from_decisions(&decisions[..i], &items), &signals);
            let change = &decision.changes[0];
            let served = context.vector(
                &change.features,
                change.pr,
                "",
                decision.width,
                &decision.at,
            );
            let y = f64::from(u8::from(decision.outcome == "green"));
            before.push((model.base_rate, y));
            after.push((model.predict_raw(&served), y));
        }
        assert!(before.iter().all(|(p, _)| *p == model.base_rate));
        let (min, max) = after.iter().fold((1.0f64, 0.0f64), |(lo, hi), (p, _)| {
            (lo.min(*p), hi.max(*p))
        });
        assert!(max - min > 0.2, "predictions spread only {min}..{max}");
        let served = evaluate(&after, model.base_rate);
        let fallback = evaluate(&before, model.base_rate);
        assert!(
            served.brier < fallback.brier,
            "served brier {} vs base-rate {}",
            served.brier,
            fallback.brier
        );
        // A brand-new change under the full history: small scores above the
        // base rate, huge below it, neither equal to it.
        let context = ServeContext::new(History::from_decisions(&decisions, &items), &signals);
        let small = model.predict_raw(&context.vector(
            &sized_features(5, 1, true),
            999,
            "",
            1,
            "2026-03-02T00:00:00Z",
        ));
        let huge = model.predict_raw(&context.vector(
            &sized_features(4000, 60, false),
            999,
            "",
            1,
            "2026-03-02T00:00:00Z",
        ));
        assert!(small > model.base_rate + 0.05, "small {small}");
        assert!(huge < model.base_rate - 0.05, "huge {huge}");
    }

    #[test]
    fn too_little_history_refuses_to_train() {
        assert!(train(&[example(vec![0.0; FEATURES.len()], 1.0)], "t").is_err());
    }
}
