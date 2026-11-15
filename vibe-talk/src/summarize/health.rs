//! Loud summariser health and fallback-chain state. `#1564 summarizer-loud-failure-and-fallback-chain`.
//!
//! One tracker per backend instance, shared by every kind it serves. It decides the ORDER a
//! request tries its kind's configured chain in (skipping cooled models), records each outcome,
//! and turns sustained failure LOUD: an ERROR log under the stable target `summarizer_health`
//! (the deploy watch greps that target) plus the `summarizer_health` section of
//! `GET /api/v1/diagnostics` and the page banner. A single failure never goes loud.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::SummaryError;
use crate::config::SummaryKind;

/// Consecutive all-chain failures that turn a kind loud.
pub const LOUD_CONSECUTIVE: u32 = 3;
/// Outcomes kept in the sliding failure-rate window.
pub const WINDOW_LEN: usize = 20;
/// Least attempts in the window before the rate can turn a kind loud.
pub const WINDOW_MIN_ATTEMPTS: usize = 5;
/// Failure rate (strictly over half counts as `>=` half here via count) that turns a kind loud.
pub const WINDOW_LOUD_FAILURES: usize = 10;
/// How long window entries count, in seconds.
pub const WINDOW_SECONDS: u64 = 600;
/// Consecutive successes that clear a loud kind.
pub const RECOVER_SUCCESSES: u32 = 2;

/// Why a kind is failing, kept in the health state so the headline never sends the owner
/// chasing the model when the cause is upstream starvation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonClass {
    /// The model or its gateway refused, errored, or answered unusably.
    Model,
    /// Upstream reads starved (rate limits), so naming/summaries could not be fed.
    UpstreamStarvation,
    /// Calls timed out.
    Timeout,
}

impl ReasonClass {
    /// The headline word for this cause.
    #[must_use]
    pub fn headline(self) -> &'static str {
        match self {
            Self::Model => "model or gateway error",
            Self::UpstreamStarvation => "Slack rate limit",
            Self::Timeout => "model timeout",
        }
    }
}

/// Classify a failure for the health state.
#[must_use]
pub fn classify(error: &SummaryError) -> ReasonClass {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("rate limit") || text.contains("429") || text.contains("starv") {
        ReasonClass::UpstreamStarvation
    } else if text.contains("timed out")
        || text.contains("timeout")
        || text.contains("did not answer within")
        || text.contains("deadline")
    {
        ReasonClass::Timeout
    } else {
        ReasonClass::Model
    }
}

/// Whether an error says the model itself is gone (withdrawn, renamed, never existed).
#[must_use]
pub fn is_withdrawn(error: &SummaryError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("withdrawn")
        || text.contains("no such model")
        || text.contains("model not found")
        || (text.contains("model") && text.contains("deprecated"))
        || (text.contains("model") && text.contains("unavailable"))
}

/// One kind's health, as diagnostics reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KindHealth {
    /// The kind's configuration key (`message`, `thread_name`, ...).
    pub kind: String,
    /// The configured chain, primary first.
    pub chain: Vec<String>,
    /// The model currently serving this kind.
    pub active_model: String,
    /// `ok` or `degraded` (loud).
    pub state: String,
    /// Why it is loud, when it is.
    pub reason: Option<ReasonClass>,
    /// Consecutive requests that failed on every chain model.
    pub consecutive_failures: u32,
    /// Attempts inside the rate window.
    pub window_attempts: usize,
    /// Failures inside the rate window.
    pub window_failures: usize,
    /// The last failure's words, when any.
    pub last_error: Option<String>,
}

/// Thread-naming starvation counters, folded into the one health report by diagnostics.
/// Filled from `ThreadNames`; `#1563` owns their production.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ThreadNamingHealth {
    /// Threads due now and still unnamed.
    pub unnamed_due: u64,
    /// Of those, unnamed for over an hour.
    pub overdue_unnamed: u64,
    /// Channels whose naming reads are paused after provider failures.
    pub paused_channels: u64,
    /// Longest run of consecutive channel pauses.
    pub max_consecutive_pauses: u64,
    /// `ok` or `degraded` (loud at `overdue_unnamed > 0` or `max_consecutive_pauses >= 3`).
    pub state: String,
}

impl ThreadNamingHealth {
    /// Whether naming starvation is loud.
    #[must_use]
    pub fn loud(&self) -> bool {
        self.state == "degraded"
    }
}

/// The one health report: diagnostics' `summarizer_health`, the page banner's source.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SummarizerHealthReport {
    /// `ok` or `degraded`.
    pub overall: String,
    /// One line naming the active cause and the model in use, when degraded.
    pub headline: Option<String>,
    /// Per-kind model health.
    pub per_kind: Vec<KindHealth>,
    /// Thread-naming starvation, folded in by diagnostics.
    pub thread_naming: ThreadNamingHealth,
}

#[derive(Debug)]
struct KindState {
    chain: Vec<String>,
    active: String,
    cooled_until: HashMap<String, Instant>,
    consecutive_failures: u32,
    consecutive_successes: u32,
    window: VecDeque<(Instant, bool)>,
    loud: bool,
    reason: Option<ReasonClass>,
    last_error: Option<String>,
}

impl KindState {
    fn new(chain: &[String]) -> Self {
        Self {
            chain: chain.to_vec(),
            active: chain.first().cloned().unwrap_or_default(),
            cooled_until: HashMap::new(),
            consecutive_failures: 0,
            consecutive_successes: 0,
            window: VecDeque::new(),
            loud: false,
            reason: None,
            last_error: None,
        }
    }

    fn prune(&mut self, now: Instant) {
        while let Some((at, _)) = self.window.front() {
            if now.duration_since(*at) > Duration::from_secs(WINDOW_SECONDS)
                || self.window.len() > WINDOW_LEN
            {
                self.window.pop_front();
            } else {
                break;
            }
        }
        while self.window.len() > WINDOW_LEN {
            self.window.pop_front();
        }
    }

    fn failures(&self) -> usize {
        self.window.iter().filter(|(_, ok)| !ok).count()
    }

    fn evaluate_loud(&mut self) -> bool {
        // Approved rate rule: >=50% failures over the last WINDOW_LEN attempts, with at
        // least WINDOW_MIN_ATTEMPTS in the window. One blip can trip neither rule.
        let rate_loud =
            self.window.len() >= WINDOW_MIN_ATTEMPTS && self.failures() * 2 >= self.window.len();
        let loud = self.consecutive_failures >= LOUD_CONSECUTIVE || rate_loud;
        if loud && !self.loud {
            self.loud = true;
            return true;
        }
        false
    }
}

/// Shared fallback + health state for one backend instance.
#[derive(Debug)]
pub struct SummarizerHealth {
    cooldown: Duration,
    kinds: Mutex<HashMap<SummaryKind, KindState>>,
}

impl SummarizerHealth {
    /// A tracker that skips a failed model for `cooldown`.
    #[must_use]
    pub fn new(cooldown: Duration) -> Self {
        Self {
            cooldown,
            kinds: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SummaryKind, KindState>> {
        self.kinds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The models to try for this request, chain order, cooled ones skipped. When every
    /// model is cooled, the one whose cool-down ends first.
    #[must_use]
    pub fn order(&self, kind: SummaryKind, chain: &[String], now: Instant) -> Vec<String> {
        let mut kinds = self.lock();
        let state = kinds.entry(kind).or_insert_with(|| KindState::new(chain));
        state.chain = chain.to_vec();
        if !chain.contains(&state.active) {
            state.active = chain.first().cloned().unwrap_or_default();
        }
        let ready: Vec<String> = chain
            .iter()
            .filter(|model| {
                state
                    .cooled_until
                    .get(*model)
                    .is_none_or(|until| *until <= now)
            })
            .cloned()
            .collect();
        if !ready.is_empty() {
            return ready;
        }
        chain
            .iter()
            .min_by_key(|model| state.cooled_until.get(*model).copied())
            .cloned()
            .into_iter()
            .collect()
    }

    /// Note that `from` failed and the request is switching to `to`. Logs the switch.
    pub fn note_switch(&self, kind: SummaryKind, from: &str, to: &str, error: &SummaryError) {
        tracing::warn!(
            target: "summarizer_fallback",
            kind = kind.as_str(),
            from_model = from,
            to_model = to,
            withdrawn = is_withdrawn(error),
            error = %error,
            "summariser falling back to the next model in the chain"
        );
        let mut kinds = self.lock();
        if let Some(state) = kinds.get_mut(&kind) {
            state
                .cooled_until
                .insert(from.to_owned(), Instant::now() + self.cooldown);
        }
    }

    /// Cool a model that just failed (without a switch, e.g. the last in the chain).
    pub fn cool(&self, kind: SummaryKind, model: &str, now: Instant) {
        let mut kinds = self.lock();
        if let Some(state) = kinds.get_mut(&kind) {
            state
                .cooled_until
                .insert(model.to_owned(), now + self.cooldown);
        }
    }

    /// Record a success on `model`: it becomes active and counts toward recovery.
    pub fn record_success(&self, kind: SummaryKind, model: &str, now: Instant) {
        let mut kinds = self.lock();
        let Some(state) = kinds.get_mut(&kind) else {
            return;
        };
        state.cooled_until.remove(model);
        state.active = model.to_owned();
        state.consecutive_failures = 0;
        state.consecutive_successes += 1;
        state.window.push_back((now, true));
        state.prune(now);
        if state.loud && state.consecutive_successes >= RECOVER_SUCCESSES {
            state.loud = false;
            state.reason = None;
            tracing::info!(
                target: "summarizer_health",
                kind = kind.as_str(),
                model = model,
                "summariser recovered; health cleared"
            );
        }
    }

    /// Record a request that failed on every model it tried. Returns true when this
    /// outcome turned the kind loud (and the ERROR line was logged).
    pub fn record_all_failed(&self, kind: SummaryKind, error: &SummaryError, now: Instant) -> bool {
        let mut kinds = self.lock();
        let Some(state) = kinds.get_mut(&kind) else {
            return false;
        };
        state.consecutive_failures += 1;
        state.consecutive_successes = 0;
        state.reason = Some(classify(error));
        state.last_error = Some(error.to_string());
        state.window.push_back((now, false));
        state.prune(now);
        if state.evaluate_loud() {
            tracing::error!(
                target: "summarizer_health",
                kind = kind.as_str(),
                active_model = %state.active,
                chain = %state.chain.join(","),
                consecutive_failures = state.consecutive_failures,
                reason = ?state.reason,
                error = %error,
                "summariser failing loudly: summaries degraded"
            );
            return true;
        }
        false
    }

    /// The per-kind report (without thread naming, which diagnostics folds in).
    #[must_use]
    pub fn report(&self, now: Instant) -> SummarizerHealthReport {
        let mut kinds = self.lock();
        let mut per_kind = Vec::new();
        let mut headline: Option<String> = None;
        let mut overall = "ok";
        for kind in SummaryKind::ALL {
            let Some(state) = kinds.get_mut(&kind) else {
                continue;
            };
            state.prune(now);
            let state_word = if state.loud { "degraded" } else { "ok" };
            if state.loud {
                overall = "degraded";
                if headline.is_none() {
                    headline = Some(match state.reason {
                        Some(ReasonClass::UpstreamStarvation) => {
                            "thread names delayed: Slack rate limit".to_owned()
                        }
                        _ if state.active != state.chain.first().cloned().unwrap_or_default() => {
                            format!(
                                "using {} for {}: {} failed: {}",
                                state.active,
                                kind.as_str(),
                                state.chain.first().cloned().unwrap_or_default(),
                                state.last_error.clone().unwrap_or_default()
                            )
                        }
                        _ => format!(
                            "summaries degraded for {}: {} failed: {}",
                            kind.as_str(),
                            state.chain.first().cloned().unwrap_or_default(),
                            state.last_error.clone().unwrap_or_default()
                        ),
                    });
                }
            }
            per_kind.push(KindHealth {
                kind: kind.as_str().to_owned(),
                chain: state.chain.clone(),
                active_model: state.active.clone(),
                state: state_word.to_owned(),
                reason: state.reason,
                consecutive_failures: state.consecutive_failures,
                window_attempts: state.window.len(),
                window_failures: state.failures(),
                last_error: state.last_error.clone(),
            });
        }
        SummarizerHealthReport {
            overall: overall.to_owned(),
            headline,
            per_kind,
            thread_naming: ThreadNamingHealth {
                state: "ok".to_owned(),
                ..ThreadNamingHealth::default()
            },
        }
    }
}

/// Fold thread-naming starvation into the one report (diagnostics' merge step).
pub fn merge_thread_naming(
    mut report: SummarizerHealthReport,
    naming: ThreadNamingHealth,
) -> SummarizerHealthReport {
    report.thread_naming = naming;
    if report.thread_naming.loud() && report.overall == "ok" {
        report.overall = "degraded".to_owned();
        report.headline = Some(format!(
            "thread names delayed: {} threads unnamed, naming paused on {} channels",
            report.thread_naming.unnamed_due, report.thread_naming.paused_channels
        ));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> SummaryError {
        SummaryError::Transport(text.to_owned())
    }

    fn health() -> (SummarizerHealth, Vec<String>) {
        (
            SummarizerHealth::new(Duration::from_secs(300)),
            vec!["primary".to_owned(), "backup".to_owned()],
        )
    }

    #[test]
    fn one_blip_never_goes_loud() {
        let (health, chain) = health();
        let now = Instant::now();
        let _ = health.order(SummaryKind::Message, &chain, now);
        assert!(!health.record_all_failed(SummaryKind::Message, &err("boom"), now));
        health.record_success(SummaryKind::Message, "primary", now);
        let report = health.report(now);
        assert_eq!(report.overall, "ok");
        assert!(report.headline.is_none());
    }

    #[test]
    fn three_consecutive_all_chain_failures_go_loud_and_recover() {
        let (health, chain) = health();
        let now = Instant::now();
        let _ = health.order(SummaryKind::Message, &chain, now);
        assert!(!health.record_all_failed(SummaryKind::Message, &err("gateway refused"), now));
        assert!(!health.record_all_failed(SummaryKind::Message, &err("gateway refused"), now));
        assert!(health.record_all_failed(SummaryKind::Message, &err("gateway refused"), now));
        let report = health.report(now);
        assert_eq!(report.overall, "degraded");
        assert!(report.headline.as_deref().unwrap_or("").contains("primary"));
        health.record_success(SummaryKind::Message, "backup", now);
        assert_eq!(
            health.report(now).overall,
            "degraded",
            "one success must not clear"
        );
        health.record_success(SummaryKind::Message, "backup", now);
        assert_eq!(health.report(now).overall, "ok");
    }

    #[test]
    fn cooled_primary_is_skipped_then_retried_after_cooldown() {
        let (health, chain) = health();
        let now = Instant::now();
        assert_eq!(health.order(SummaryKind::Message, &chain, now), chain);
        health.cool(SummaryKind::Message, "primary", now);
        assert_eq!(
            health.order(SummaryKind::Message, &chain, now),
            vec!["backup".to_owned()]
        );
        let later = now + Duration::from_secs(301);
        assert_eq!(health.order(SummaryKind::Message, &chain, later), chain);
    }

    #[test]
    fn withdrawn_and_starvation_reasons_are_kept_apart() {
        assert!(is_withdrawn(&err("model not found: small-elk")));
        assert_eq!(
            classify(&err("rate limit (429) from provider")),
            ReasonClass::UpstreamStarvation
        );
        assert_eq!(classify(&err("gateway refused")), ReasonClass::Model);
    }
}
