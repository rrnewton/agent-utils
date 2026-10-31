//! Summarize spend planning and run state, at parity with
//! `py/wrkviz/budget.py`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::json::{as_object, read_json};
use crate::summary_registry::{
    AGENT_LIFETIME_STYLE, PHASE_STYLE, PLAIN_LANGUAGE_ROLLUP_STYLE, PROJECT_OVERVIEW_STYLE,
    TECHNICAL_ROLLUP_STYLE,
};
use crate::token_usage::{load_batch_receipt, TokenUsage};
use crate::transcript_io::write_json_if_changed;

/// Phase job kind.
pub const PHASE_KIND: &str = PHASE_STYLE;
/// Technical rollup kind.
pub const TECHNICAL_ROLLUP_KIND: &str = TECHNICAL_ROLLUP_STYLE;
/// Plain rollup kind.
pub const PLAIN_ROLLUP_KIND: &str = PLAIN_LANGUAGE_ROLLUP_STYLE;
/// Overview kind.
pub const OVERVIEW_KIND: &str = PROJECT_OVERVIEW_STYLE;
/// Agent-name kind.
pub const AGENT_NAME_KIND: &str = AGENT_LIFETIME_STYLE;
/// State file name.
pub const STATE_FILE: &str = "summarize_state.json";
/// State schema version.
pub const STATE_SCHEMA_VERSION: i64 = 1;

fn now_iso() -> String {
    let now = chrono::Utc::now();
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

fn py_float(value: f64) -> String {
    crate::json::py_float_repr(value)
}

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prices {
    /// Input rate.
    pub input: f64,
    /// Output rate.
    pub output: f64,
    /// Cached input rate.
    pub cached_input: f64,
    /// Cache-write input rate.
    pub cache_write_input: f64,
}

impl Prices {
    /// Parse `input=N,output=N[,cached_input=N][,cache_write_input=N]`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut values: BTreeMap<String, f64> = BTreeMap::new();
        let allowed = ["cache_write_input", "cached_input", "input", "output"];
        for part in text.split(',') {
            let Some((name_raw, raw)) = part.split_once('=') else {
                return Err(format!(
                    "--price: expected NAME=USD_PER_MTOK with NAME in ['cache_write_input', 'cached_input', 'input', 'output'], got {}",
                    crate::json::py_repr_str(part.trim())
                ));
            };
            let name = name_raw.trim();
            if !allowed.contains(&name) {
                return Err(format!(
                    "--price: expected NAME=USD_PER_MTOK with NAME in ['cache_write_input', 'cached_input', 'input', 'output'], got {}",
                    crate::json::py_repr_str(part.trim())
                ));
            }
            if values.contains_key(name) {
                return Err(format!("--price: {name} given twice"));
            }
            let value: f64 = raw.parse().map_err(|_| {
                format!(
                    "--price: {name} is not a number: {}",
                    crate::json::py_repr_str(raw)
                )
            })?;
            if value.is_nan() || value < 0.0 || value.is_infinite() {
                return Err(format!(
                    "--price: {name} must be a finite non-negative number"
                ));
            }
            values.insert(name.to_string(), value);
        }
        let (Some(input), Some(output)) = (values.get("input"), values.get("output")) else {
            return Err("--price: input and output rates are required".to_string());
        };
        Ok(Self {
            input: *input,
            output: *output,
            cached_input: *values.get("cached_input").unwrap_or(input),
            cache_write_input: *values.get("cache_write_input").unwrap_or(input),
        })
    }

    fn cost(&self, input: f64, cached: f64, write: f64, output: f64) -> f64 {
        let uncached = (input - cached - write).max(0.0);
        (uncached * self.input
            + cached * self.cached_input
            + write * self.cache_write_input
            + output * self.output)
            / 1_000_000.0
    }

    /// Cost of one usage record.
    pub fn cost_usd(&self, usage: &TokenUsage) -> f64 {
        self.cost(
            usage.input_tokens as f64,
            usage.cached_input_tokens as f64,
            usage.cache_write_input_tokens as f64,
            usage.output_tokens as f64,
        )
    }
}

/// Mean token usage of one job kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JobUsage {
    /// Mean input.
    pub input_tokens: f64,
    /// Mean cached input.
    pub cached_input_tokens: f64,
    /// Mean cache-write input.
    pub cache_write_input_tokens: f64,
    /// Mean output.
    pub output_tokens: f64,
    /// Sample count.
    pub samples: i64,
}

impl JobUsage {
    /// Usage of `jobs` jobs, rounded up to whole tokens.
    pub fn times(&self, jobs: i64) -> Result<TokenUsage, String> {
        let scale = |value: f64| -> i64 { (value * jobs as f64).ceil() as i64 };
        let input = scale(self.input_tokens);
        TokenUsage::new(
            input,
            input.min(scale(self.cached_input_tokens)),
            input.min(scale(self.cache_write_input_tokens)),
            scale(self.output_tokens),
            0,
        )
    }
}

fn default_job_usage(kind: &str) -> JobUsage {
    match kind {
        PHASE_KIND => JobUsage {
            input_tokens: 21_000.0,
            cached_input_tokens: 0.0,
            cache_write_input_tokens: 0.0,
            output_tokens: 2_500.0,
            samples: 0,
        },
        AGENT_NAME_KIND => JobUsage {
            input_tokens: 29_000.0,
            cached_input_tokens: 0.0,
            cache_write_input_tokens: 0.0,
            output_tokens: 900.0,
            samples: 0,
        },
        OVERVIEW_KIND => JobUsage {
            input_tokens: 24_000.0,
            cached_input_tokens: 0.0,
            cache_write_input_tokens: 0.0,
            output_tokens: 1_200.0,
            samples: 0,
        },
        PLAIN_ROLLUP_KIND => JobUsage {
            input_tokens: 50_000.0,
            cached_input_tokens: 0.0,
            cache_write_input_tokens: 0.0,
            output_tokens: 2_900.0,
            samples: 0,
        },
        _ => JobUsage {
            input_tokens: 60_000.0,
            cached_input_tokens: 0.0,
            cache_write_input_tokens: 0.0,
            output_tokens: 3_800.0,
            samples: 0,
        },
    }
}

/// Mean usage per job kind for one backend and model.
#[derive(Clone, Debug)]
pub struct CostModel {
    /// Per-kind means.
    pub per_job: BTreeMap<String, JobUsage>,
    /// Free (heuristic) backend.
    pub free: bool,
}

impl CostModel {
    /// Estimated usage of `jobs` jobs of `kind`.
    pub fn usage(&self, kind: &str, jobs: i64) -> TokenUsage {
        if self.free || jobs <= 0 {
            return TokenUsage::default();
        }
        self.per_job
            .get(kind)
            .copied()
            .unwrap_or_else(|| default_job_usage(kind))
            .times(jobs)
            .unwrap_or_default()
    }

    /// Estimated USD of `jobs` jobs of `kind`.
    pub fn usd(&self, kind: &str, jobs: i64, prices: &Prices) -> f64 {
        if self.free || jobs <= 0 {
            return 0.0;
        }
        let mean = self
            .per_job
            .get(kind)
            .copied()
            .unwrap_or_else(|| default_job_usage(kind));
        prices.cost(
            mean.input_tokens * jobs as f64,
            mean.cached_input_tokens * jobs as f64,
            mean.cache_write_input_tokens * jobs as f64,
            mean.output_tokens * jobs as f64,
        )
    }

    /// Estimate source wording.
    pub fn source(&self, kind: &str) -> String {
        if self.free {
            return "offline backend".to_string();
        }
        match self.per_job.get(kind) {
            Some(item) if item.samples != 0 => format!("mean of {} recorded", item.samples),
            _ => "built-in".to_string(),
        }
    }
}

fn receipt_kind(cache_dir: &Path, first_hash: &str, name_cache: bool) -> Option<String> {
    if name_cache {
        return Some(AGENT_NAME_KIND.to_string());
    }
    let value = read_json(&cache_dir.join(format!("{first_hash}.json"))).ok()?;
    let root = as_object(&value, "summary cache").ok()?;
    let result = as_object(root.get("result")?, "summary cache.result").ok()?;
    let key = result.get("key")?.as_str()?;
    for (prefix, kind) in [
        ("rollup-plain:", PLAIN_ROLLUP_KIND),
        ("rollup:", TECHNICAL_ROLLUP_KIND),
        ("project-overview:", OVERVIEW_KIND),
    ] {
        if key.starts_with(prefix) {
            return Some(kind.to_string());
        }
    }
    Some(PHASE_KIND.to_string())
}

/// Mean usage per job kind from completed receipts.
pub fn calibrate(
    summary_cache: &Path,
    name_cache: &Path,
    backend: &str,
    model: &str,
    reasoning_effort: Option<&str>,
) -> CostModel {
    if backend == "heuristic" {
        return CostModel {
            per_job: BTreeMap::new(),
            free: true,
        };
    }
    let mut totals: BTreeMap<String, [f64; 5]> = BTreeMap::new();
    for (cache_dir, is_names) in [(summary_cache, false), (name_cache, true)] {
        let receipts = cache_dir.join("_usage").join("receipts");
        if !receipts.is_dir() {
            continue;
        }
        let mut paths: Vec<std::path::PathBuf> = match std::fs::read_dir(&receipts) {
            Ok(entries) => entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
                .collect(),
            Err(_) => Vec::new(),
        };
        paths.sort();
        for path in paths {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Some(receipt) = load_batch_receipt(&cache_dir.join("_usage"), stem) else {
                continue;
            };
            if receipt.status != "completed"
                || receipt.usage.is_none()
                || receipt.input_hashes.is_empty()
                || receipt.backend != backend
                || receipt.model != model
                || receipt.reasoning_effort.as_deref() != reasoning_effort
            {
                continue;
            }
            let Some(kind) = receipt_kind(cache_dir, &receipt.input_hashes[0], is_names) else {
                continue;
            };
            let usage = receipt.usage.unwrap();
            let item = totals.entry(kind).or_default();
            item[0] += receipt.input_hashes.len() as f64;
            item[1] += usage.input_tokens as f64;
            item[2] += usage.cached_input_tokens as f64;
            item[3] += usage.cache_write_input_tokens as f64;
            item[4] += usage.output_tokens as f64;
        }
    }
    let per_job = totals
        .into_iter()
        .filter(|(_, values)| values[0] > 0.0)
        .map(|(kind, values)| {
            (
                kind,
                JobUsage {
                    input_tokens: values[1] / values[0],
                    cached_input_tokens: values[2] / values[0],
                    cache_write_input_tokens: values[3] / values[0],
                    output_tokens: values[4] / values[0],
                    samples: values[0] as i64,
                },
            )
        })
        .collect();
    CostModel {
        per_job,
        free: false,
    }
}

/// What one stage of the tree needs.
#[derive(Clone, Debug, Default)]
pub struct StageCount {
    /// Stage name.
    pub name: String,
    /// Job kind.
    pub kind: String,
    /// Total jobs.
    pub total: i64,
    /// Cached jobs.
    pub cached: i64,
    /// Carried-forward jobs.
    pub carried_forward: i64,
    /// Jobs to generate.
    pub to_generate: i64,
}

/// The estimated remaining work and cost of one run.
#[derive(Clone, Debug)]
pub struct SummaryPlan {
    /// Team slug.
    pub team_slug: String,
    /// Stages.
    pub stages: Vec<StageCount>,
    /// Cost model.
    pub costs: CostModel,
    /// Prices, if given.
    pub prices: Option<Prices>,
    /// Budget, if given.
    pub budget_usd: Option<f64>,
    /// Previous unfinished state.
    pub previous_state: Option<Value>,
    /// Recorded request.
    pub request: Option<Value>,
}

fn rounded(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |item| {
        let rounded = (item * 10_000.0).round() / 10_000.0;
        serde_json::Number::from_f64(rounded).map_or(Value::Null, Value::Number)
    })
}

fn comma(value: i64) -> String {
    let text = value.abs().to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index > 0 && (text.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 {
        format!("-{out}")
    } else {
        out
    }
}

impl SummaryPlan {
    /// Estimated usage of one stage.
    pub fn stage_usage(&self, stage: &StageCount) -> TokenUsage {
        self.costs.usage(&stage.kind, stage.to_generate)
    }

    /// Estimated usage of all new work.
    pub fn usage(&self) -> TokenUsage {
        let mut total = TokenUsage::default();
        for stage in &self.stages {
            total += self.stage_usage(stage);
        }
        total
    }

    /// Estimated USD of one stage.
    pub fn stage_usd(&self, stage: &StageCount) -> Option<f64> {
        self.prices
            .map(|prices| self.costs.usd(&stage.kind, stage.to_generate, &prices))
    }

    /// Estimated USD of the whole tree.
    pub fn estimated_usd(&self) -> Option<f64> {
        self.prices.map(|prices| {
            self.stages
                .iter()
                .map(|stage| self.costs.usd(&stage.kind, stage.to_generate, &prices))
                .sum()
        })
    }

    /// Jobs needing a model call.
    pub fn to_generate(&self) -> i64 {
        self.stages.iter().map(|stage| stage.to_generate).sum()
    }

    /// Whether the estimate exceeds the budget.
    pub fn over_budget(&self) -> bool {
        match (self.budget_usd, self.estimated_usd()) {
            (Some(budget), Some(estimate)) => estimate > budget,
            _ => false,
        }
    }

    /// Human-readable plan lines.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "summary plan for {} (before any model call):",
            self.team_slug
        )];
        if let Some(Value::Object(state)) = &self.previous_state {
            out.push(format!(
                "  {}",
                describe_previous_state(state, self.request.as_ref())
            ));
        }
        for stage in &self.stages {
            let usage = self.stage_usage(stage);
            let usd = self.stage_usd(stage);
            let mut parts = vec![
                format!("{} total", stage.total),
                format!("{} cached", stage.cached),
            ];
            if stage.carried_forward != 0 {
                parts.push(format!("{} carried forward", stage.carried_forward));
            }
            parts.push(format!("{} to generate", stage.to_generate));
            let mut cost = String::new();
            if stage.to_generate != 0 {
                cost = format!(
                    "; ~{} in / {} out tokens",
                    comma(usage.input_tokens),
                    comma(usage.output_tokens)
                );
                if let Some(usd) = usd {
                    cost.push_str(&format!(", ~${usd:.2}"));
                }
                cost.push_str(&format!(" ({})", self.costs.source(&stage.kind)));
            }
            out.push(format!("  {}: {}{}", stage.name, parts.join(", "), cost));
        }
        let usage = self.usage();
        let mut total = format!(
            "  total: {} model jobs, ~{} in / {} out tokens",
            self.to_generate(),
            comma(usage.input_tokens),
            comma(usage.output_tokens)
        );
        if let Some(estimate) = self.estimated_usd() {
            total.push_str(&format!(", ~${estimate:.2}"));
        }
        if let Some(budget) = self.budget_usd {
            total.push_str(&format!("; budget ${budget:.2}"));
            if self.estimated_usd().is_some() {
                total.push_str(if self.over_budget() {
                    " (over budget)"
                } else {
                    " (within budget)"
                });
            }
        }
        out.push(total);
        out
    }

    /// Plan as JSON for the run-state file.
    pub fn to_json(&self) -> Value {
        let mut stages = Vec::new();
        for stage in &self.stages {
            let mut item = serde_json::Map::new();
            item.insert("name".into(), Value::String(stage.name.clone()));
            item.insert("kind".into(), Value::String(stage.kind.clone()));
            item.insert("total".into(), stage.total.into());
            item.insert("cached".into(), stage.cached.into());
            item.insert("carried_forward".into(), stage.carried_forward.into());
            item.insert("to_generate".into(), stage.to_generate.into());
            item.insert("estimated_usage".into(), self.stage_usage(stage).to_json());
            item.insert("estimated_usd".into(), rounded(self.stage_usd(stage)));
            stages.push(Value::Object(item));
        }
        let mut out = serde_json::Map::new();
        out.insert("stages".into(), Value::Array(stages));
        out.insert("to_generate".into(), self.to_generate().into());
        out.insert("estimated_usage".into(), self.usage().to_json());
        out.insert("estimated_usd".into(), rounded(self.estimated_usd()));
        out.insert(
            "budget_usd".into(),
            self.budget_usd.map_or(Value::Null, |value| {
                serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
            }),
        );
        Value::Object(out)
    }
}

/// Admit model batches only while the next estimate fits the budget.
pub struct SpendGuard {
    budget_usd: f64,
    prices: Prices,
    costs: CostModel,
    inner: Mutex<(f64, f64, bool)>,
}

impl SpendGuard {
    /// Construct.
    pub fn new(budget_usd: f64, prices: Prices, costs: CostModel) -> Result<Self, String> {
        if budget_usd.is_nan() || budget_usd < 0.0 {
            return Err("budget must be a non-negative number of US dollars".to_string());
        }
        Ok(Self {
            budget_usd,
            prices,
            costs,
            inner: Mutex::new((0.0, 0.0, false)),
        })
    }

    /// Reserve one batch, or `None` once the budget would be crossed.
    pub fn reserve(&self, kind: &str, jobs: i64) -> Option<f64> {
        let estimate = self.costs.usd(kind, jobs, &self.prices);
        let mut state = self.inner.lock().unwrap();
        if state.2 || (state.0 + state.1 + estimate > self.budget_usd + 1e-9) {
            state.2 = true;
            return None;
        }
        state.1 += estimate;
        Some(estimate)
    }

    /// Replace a reservation with actual cost.
    pub fn settle(&self, reservation: f64, usage: Option<TokenUsage>) {
        let actual = usage.map_or(reservation, |item| self.prices.cost_usd(&item));
        let mut state = self.inner.lock().unwrap();
        state.1 -= reservation;
        state.0 += actual;
    }

    /// Settled spend so far.
    pub fn spent_usd(&self) -> f64 {
        self.inner.lock().unwrap().0
    }
}

/// The last run-state record, if present and readable.
pub fn load_state(summary_root: &Path) -> Option<Value> {
    let value = read_json(&summary_root.join(STATE_FILE)).ok()?;
    let state = as_object(&value, STATE_FILE).ok()?;
    if state.get("schema_version").and_then(Value::as_i64) != Some(STATE_SCHEMA_VERSION) {
        // Python compares with ==, so 1.0 and true also pass.
        match state.get("schema_version") {
            Some(Value::Bool(true)) => {}
            Some(Value::Number(number)) if number.as_f64() == Some(1.0) => {}
            _ => return None,
        }
    }
    Some(value)
}

/// Persist the run-state record.
pub fn write_state(
    summary_root: &Path,
    state: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let mut record = state.clone();
    record.insert("schema_version".into(), STATE_SCHEMA_VERSION.into());
    record.insert("updated_at".into(), Value::String(now_iso()));
    write_json_if_changed(&summary_root.join(STATE_FILE), &Value::Object(record))?;
    Ok(())
}

/// The record written before the first model call.
pub fn running_state(
    request: &Value,
    plan: &SummaryPlan,
    previous: Option<&Value>,
) -> serde_json::Map<String, Value> {
    let resumed = match previous {
        Some(Value::Object(state))
            if state.get("status").and_then(Value::as_str) != Some("complete") =>
        {
            let mut item = serde_json::Map::new();
            for key in ["status", "started_at", "updated_at", "stopped", "spent_usd"] {
                item.insert(key.into(), state.get(key).cloned().unwrap_or(Value::Null));
            }
            Value::Object(item)
        }
        _ => Value::Null,
    };
    let mut out = serde_json::Map::new();
    out.insert("status".into(), Value::String("running".into()));
    out.insert("pid".into(), (std::process::id() as i64).into());
    out.insert("started_at".into(), Value::String(now_iso()));
    out.insert("request".into(), request.clone());
    out.insert("plan".into(), plan.to_json());
    out.insert(
        "budget_usd".into(),
        plan.budget_usd.map_or(Value::Null, |value| {
            serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
        }),
    );
    out.insert("spent_usd".into(), Value::Null);
    out.insert("stopped".into(), Value::Null);
    out.insert("resumed_from".into(), resumed);
    out
}

/// One line describing an earlier unfinished run.
pub fn describe_previous_state(
    state: &serde_json::Map<String, Value>,
    request: Option<&Value>,
) -> String {
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let started = state
        .get("started_at")
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => crate::json::py_repr(other),
        })
        .unwrap_or_else(|| "None".to_string());
    let differs = match request {
        Some(request) if state.get("request") != Some(request) => {
            "; it was asked for a different window, rollup levels or model, so the counts below are for this run's request"
        }
        _ => "",
    };
    if status == "running" {
        return format!(
            "resuming: the run started {started} was interrupted before it finished (it was killed or crashed); its finished work is cached{differs}"
        );
    }
    let stopped = state.get("stopped");
    let (stage, reason) = match stopped {
        Some(Value::Object(item)) => (
            item.get("stage").and_then(Value::as_str),
            item.get("reason").and_then(Value::as_str),
        ),
        _ => (None, None),
    };
    format!(
        "resuming: the run started {started} stopped partway{}{}; its finished work is cached{differs}",
        stage.map_or(String::new(), |item| format!(" during {item}")),
        reason.map_or(String::new(), |item| format!(" ({item})"))
    )
}

/// Warning line when the last summarize run did not complete.
pub fn partial_warning(state: Option<&Value>) -> Option<String> {
    let state = state?;
    let obj = as_object(state, STATE_FILE).ok()?;
    let status = obj.get("status").and_then(Value::as_str);
    if status == Some("complete") {
        return None;
    }
    let what = if status == Some("running") {
        "was interrupted"
    } else {
        "stopped partway"
    };
    let stage = match obj.get("stopped") {
        Some(Value::Object(item)) => item.get("stage").and_then(Value::as_str),
        _ => None,
    };
    Some(format!(
        "summaries are partial: the last summarize run {what}{}; run summarize again with that run's arguments (its request in {STATE_FILE}) to finish",
        stage.map_or(String::new(), |item| format!(" during {item}"))
    ))
}

/// Money formatting used by the budget-refusal sentence.
pub fn format_money(value: f64) -> String {
    format!("{value:.2}")
}

/// Python float rendering kept for callers.
pub fn float_text(value: f64) -> String {
    py_float(value)
}
