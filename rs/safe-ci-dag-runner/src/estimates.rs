//! Profile-store FEEDBACK: turn recorded per-step samples into planning estimates.
//!
//! Direct port of `py/safe_ci_dag_runner/estimates.py`; the derived numbers and the
//! `plan --format json` / `plan` text output are BYTE-IDENTICAL to the Python build for a given
//! store + DAG (cross-tested in `cross/differential.py`). This is the READING half of the
//! learned-duration profile store (ds-7pzdgm / ds-afzsqf); the writing half already ships in
//! [`crate::perflog`].
//!
//! For the current `(machine_id, container_class)` it derives, per step:
//! * `est_duration_s` — a contention-discounted, MAD-trimmed MEDIAN of the recorded `elapsed_s`
//!   (the step's INTRINSIC / uncontended duration). Median, not mean, so one slow sample cannot
//!   drag it; discounted by whatever contention column the store carries.
//! * `rss_estimate_bytes` — a robust HIGH-WATER (90th-percentile, nearest-rank via INTEGER rank
//!   arithmetic so it is cross-identical) of the recorded `peak_bytes` for the memory model.
//!
//! Sparse/missing data degrades to `None` (the caller falls back to the DAG hint); a malformed
//! cell is skipped, never coerced.

use std::collections::HashMap;
use std::path::Path;

use crate::io::json_str;
use crate::model::{DagConfig, ResourceHint, Step};
use crate::perflog::{container_class, machine_id, parse_csv_line};

/// Environment overrides for the feedback identity (mirrors the Python constants). Let a test (or
/// a caller pinning heterogeneous-but-equivalent runners) force the
/// `step_profiles_<machine>_<container>.csv` the reader loads.
pub const MACHINE_ID_ENV: &str = "SAFE_CI_DAG_RUNNER_MACHINE_ID";
pub const CONTAINER_CLASS_ENV: &str = "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS";

/// Minimum recorded samples before the store overrides the DAG hint for a step.
pub const DEFAULT_MIN_SAMPLES: i64 = 1;

/// MAD-trim: drop duration samples more than this many MADs from the median before re-medianing.
const MAD_TRIM_K: f64 = 3.5;
/// RSS high-water percentile as an exact integer fraction (num/den) — no floating `ceil`.
const RSS_PCTL_NUM: i64 = 9;
const RSS_PCTL_DEN: i64 = 10;
/// Clamp a contention fraction so a bogus signal cannot discount a duration to zero.
const MAX_CONTENTION: f64 = 0.95;

/// Contention percentage columns understood by the reader, in priority order (see Python's
/// `_CONTENTION_PCT_COLUMNS`).
const CONTENTION_PCT_COLUMNS: [&str; 3] =
    ["pct_other", "psi_cpu_some_avg10", "cpu_pressure_some_avg10"];

/// Which scheduling planner to use for dispatch ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Planner {
    GreedyLpt,
    CriticalPath,
}

impl Planner {
    /// The canonical string form (matches the Python `Planner.<X>.value`).
    pub fn value(self) -> &'static str {
        match self {
            Planner::GreedyLpt => "greedy-lpt",
            Planner::CriticalPath => "critical-path",
        }
    }

    /// Parse the canonical string form, or `None` for an unknown value.
    pub fn from_value(text: &str) -> Option<Planner> {
        match text {
            "greedy-lpt" => Some(Planner::GreedyLpt),
            "critical-path" => Some(Planner::CriticalPath),
            _ => None,
        }
    }
}

/// Aggregated store estimates for ONE step, from its recorded samples.
#[derive(Debug, Clone)]
pub struct StepSamples {
    pub step: String,
    pub samples: i64,
    pub est_duration_s: Option<f64>,
    pub rss_estimate_bytes: Option<i64>,
}

/// The `(machine_id, container_class)` the feedback reader selects the store file by: the current
/// host's, unless the env overrides are set. Both builds resolve this identically.
pub fn feedback_identity() -> (String, String) {
    let mid = std::env::var(MACHINE_ID_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(machine_id);
    let cc = std::env::var(CONTAINER_CLASS_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(container_class);
    (mid, cc)
}

fn median(sorted_values: &[f64]) -> f64 {
    let n = sorted_values.len();
    let mid = n / 2;
    if n % 2 == 1 {
        sorted_values[mid]
    } else {
        (sorted_values[mid - 1] + sorted_values[mid]) / 2.0
    }
}

fn sort_f64(values: &mut [f64]) {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
}

/// MAD-trimmed median (mirrors Python's `_robust_median`): median after dropping samples more than
/// `MAD_TRIM_K` MADs from the median; falls back to the plain median when the MAD is zero.
fn robust_median(values: &[f64]) -> f64 {
    let mut xs = values.to_vec();
    sort_f64(&mut xs);
    let m = median(&xs);
    let mut deviations: Vec<f64> = xs.iter().map(|x| (x - m).abs()).collect();
    sort_f64(&mut deviations);
    let mad = median(&deviations);
    if mad > 0.0 {
        let cutoff = MAD_TRIM_K * mad;
        let mut kept: Vec<f64> = xs
            .iter()
            .copied()
            .filter(|x| (x - m).abs() <= cutoff)
            .collect();
        if !kept.is_empty() {
            sort_f64(&mut kept);
            return median(&kept);
        }
    }
    m
}

/// The 90th percentile of a non-empty int slice by NEAREST-RANK with integer arithmetic (mirrors
/// Python's `_high_percentile`): `rank = ceil(num*n/den) == (num*n + den - 1) / den`, clamped to
/// `1..=n`, returning the rank-th smallest.
fn high_percentile(values: &[i64]) -> i64 {
    let mut xs = values.to_vec();
    xs.sort_unstable();
    let n = xs.len() as i64;
    let mut rank = (RSS_PCTL_NUM * n + RSS_PCTL_DEN - 1) / RSS_PCTL_DEN;
    if rank < 1 {
        rank = 1;
    }
    if rank > n {
        rank = n;
    }
    xs[(rank - 1) as usize]
}

/// Parse the affinity (core) width out of a `container_class` like `affinity316_cpu-max-...` ->
/// `316`. `None` if the shape is unexpected.
fn affinity_width(container_class: &str) -> Option<i64> {
    let rest = container_class.strip_prefix("affinity")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

fn parse_float(cell: Option<&String>) -> Option<f64> {
    match cell {
        Some(s) if !s.is_empty() => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn parse_int(cell: Option<&String>) -> Option<i64> {
    match cell {
        Some(s) if !s.is_empty() => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// The fraction of machine capacity taken by OTHER work during a sample, from whichever
/// contention column is present (see [`CONTENTION_PCT_COLUMNS`]). `0.0` when no usable signal;
/// clamped to [`MAX_CONTENTION`]. Mirrors Python's `_contention_fraction`.
fn contention_fraction(row: &HashMap<String, String>, affinity: Option<i64>) -> f64 {
    let mut fraction: Option<f64> = None;
    for col in CONTENTION_PCT_COLUMNS {
        if let Some(pct) = parse_float(row.get(col)) {
            fraction = Some(pct / 100.0);
            break;
        }
    }
    if fraction.is_none() {
        if let Some(external) = parse_float(row.get("external_cores")) {
            if let Some(width) = affinity {
                if width > 0 {
                    fraction = Some(external / width as f64);
                }
            }
        }
    }
    if fraction.is_none() {
        if let Some(co) = parse_float(row.get("co_tenants")) {
            if co > 0.0 {
                fraction = Some(co / (co + 1.0));
            }
        }
    }
    match fraction {
        None => 0.0,
        Some(f) if f < 0.0 => 0.0,
        Some(f) if f > MAX_CONTENTION => MAX_CONTENTION,
        Some(f) => f,
    }
}

/// Read the per-step profile CSV for `(machine_id, container_class)` under `profile_dir` and
/// aggregate the samples per step into robust estimates. Returns `{}` when the file is absent
/// (the caller then falls back to DAG hints). Mirrors Python's `load_step_samples`.
pub fn load_step_samples(
    profile_dir: &Path,
    machine_id: &str,
    container_class: &str,
) -> HashMap<String, StepSamples> {
    let path = profile_dir.join(format!("step_profiles_{machine_id}_{container_class}.csv"));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    let affinity = affinity_width(container_class);
    let mut lines = text.lines();
    let header: Vec<String> = match lines.next() {
        Some(h) => parse_csv_line(h),
        None => return HashMap::new(),
    };
    let index: HashMap<String, usize> = header
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();

    let mut durations: HashMap<String, Vec<f64>> = HashMap::new();
    let mut peaks: HashMap<String, Vec<i64>> = HashMap::new();
    let mut counts: HashMap<String, i64> = HashMap::new();

    for line in lines {
        if line.is_empty() {
            continue;
        }
        let cells = parse_csv_line(line);
        // Build a name->value map for the contention lookups (mirrors csv.DictReader).
        let row: HashMap<String, String> = index
            .iter()
            .map(|(name, &i)| (name.clone(), cells.get(i).cloned().unwrap_or_default()))
            .collect();
        let step = match row.get("step") {
            Some(s) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        *counts.entry(step.clone()).or_insert(0) += 1;
        if let Some(elapsed) = parse_float(row.get("elapsed_s")) {
            if elapsed >= 0.0 {
                let intrinsic = elapsed * (1.0 - contention_fraction(&row, affinity));
                durations.entry(step.clone()).or_default().push(intrinsic);
            }
        }
        if let Some(peak) = parse_int(row.get("peak_bytes")) {
            if peak >= 0 {
                peaks.entry(step.clone()).or_default().push(peak);
            }
        }
    }

    let mut result: HashMap<String, StepSamples> = HashMap::new();
    for (step, n) in counts {
        let est = durations.get(&step).map(|d| robust_median(d));
        let rss = peaks.get(&step).map(|p| high_percentile(p));
        result.insert(
            step.clone(),
            StepSamples {
                step,
                samples: n,
                est_duration_s: est,
                rss_estimate_bytes: rss,
            },
        );
    }
    result
}

// --------------------------------------------------------------------------- planner

/// The resolved estimate + planner metadata for one step (what the plan display shows).
#[derive(Debug, Clone)]
pub struct PlanEntry {
    pub tag: String,
    pub est_duration_s: f64,
    pub est_source: String,
    pub rss_estimate_bytes: Option<i64>,
    pub rss_source: String,
    pub bottom_level_s: f64,
    pub samples: i64,
}

/// A complete plan: per-step resolved estimates, the dispatch order, and the critical path.
#[derive(Debug, Clone)]
pub struct Plan {
    pub planner: Planner,
    pub order: Vec<String>,
    pub critical_path: Vec<String>,
    pub critical_path_length_s: f64,
    pub entries: Vec<PlanEntry>,
}

impl Plan {
    fn by_tag(&self) -> HashMap<String, &PlanEntry> {
        self.entries.iter().map(|e| (e.tag.clone(), e)).collect()
    }
}

struct Resolved {
    est: f64,
    est_source: &'static str,
    rss: Option<i64>,
    rss_source: &'static str,
    samples: i64,
}

fn resolved_estimate(step: &Step, samples: Option<&StepSamples>, min_samples: i64) -> Resolved {
    let n = samples.map(|s| s.samples).unwrap_or(0);
    let store_ok = samples.is_some() && n >= min_samples;

    let (est, est_source) = match samples {
        Some(s) if store_ok && s.est_duration_s.is_some() => (s.est_duration_s.unwrap(), "store"),
        _ if step.hint.est_duration_s != 0.0 => (step.hint.est_duration_s, "hint"),
        _ => (step.hint.est_duration_s, "default"),
    };

    let (rss, rss_source) = match samples {
        Some(s) if store_ok && s.rss_estimate_bytes.is_some() => (s.rss_estimate_bytes, "store"),
        _ if step.hint.rss_baseline_bytes.is_some() => (step.hint.rss_baseline_bytes, "hint"),
        _ => (None, "none"),
    };

    Resolved {
        est,
        est_source,
        rss,
        rss_source,
        samples: n,
    }
}

/// Map each step tag to the tags that DEPEND on it (its downstream successors), in cfg order.
fn successors(cfg: &DagConfig) -> HashMap<String, Vec<String>> {
    let mut succ: HashMap<String, Vec<String>> =
        cfg.steps.iter().map(|s| (s.tag(), Vec::new())).collect();
    for step in &cfg.steps {
        let tag = step.tag();
        for dep in &step.deps {
            if let Some(v) = succ.get_mut(dep) {
                v.push(tag.clone());
            }
        }
    }
    succ
}

/// Bottom-level (longest remaining est-weighted path to a sink) for every step; memoized DFS.
fn bottom_levels(
    cfg: &DagConfig,
    est: &HashMap<String, f64>,
    succ: &HashMap<String, Vec<String>>,
) -> HashMap<String, f64> {
    let mut bottom: HashMap<String, f64> = HashMap::new();

    fn visit(
        tag: &str,
        est: &HashMap<String, f64>,
        succ: &HashMap<String, Vec<String>>,
        bottom: &mut HashMap<String, f64>,
    ) -> f64 {
        if let Some(v) = bottom.get(tag) {
            return *v;
        }
        let mut value = est.get(tag).copied().unwrap_or(0.0);
        if let Some(children) = succ.get(tag) {
            if !children.is_empty() {
                let best = children
                    .iter()
                    .map(|w| visit(w, est, succ, bottom))
                    .fold(f64::NEG_INFINITY, f64::max);
                value += best;
            }
        }
        bottom.insert(tag.to_string(), value);
        value
    }

    for step in &cfg.steps {
        visit(&step.tag(), est, succ, &mut bottom);
    }
    bottom
}

/// `true` if `a` should sort BEFORE `b` under "greatest bottom-level, tie-break smallest tag".
fn better_bottom(a: &str, b: &str, bottom: &HashMap<String, f64>) -> bool {
    let ba = bottom.get(a).copied().unwrap_or(0.0);
    let bb = bottom.get(b).copied().unwrap_or(0.0);
    if ba != bb {
        ba > bb
    } else {
        a < b
    }
}

/// The longest est-weighted path through the DAG (mirrors Python's `_critical_path`).
fn critical_path(
    cfg: &DagConfig,
    bottom: &HashMap<String, f64>,
    succ: &HashMap<String, Vec<String>>,
) -> (Vec<String>, f64) {
    if cfg.steps.is_empty() {
        return (Vec::new(), 0.0);
    }
    let tags: Vec<String> = cfg.steps.iter().map(|s| s.tag()).collect();
    let mut start = tags[0].clone();
    for t in &tags[1..] {
        if better_bottom(t, &start, bottom) {
            start = t.clone();
        }
    }
    let length = bottom.get(&start).copied().unwrap_or(0.0);
    let mut path = vec![start.clone()];
    let mut current = start;
    loop {
        let children = match succ.get(&current) {
            Some(c) if !c.is_empty() => c,
            _ => break,
        };
        let mut next = children[0].clone();
        for c in &children[1..] {
            if better_bottom(c, &next, bottom) {
                next = c.clone();
            }
        }
        path.push(next.clone());
        current = next;
    }
    (path, length)
}

/// The deterministic dispatch order (mirrors Python's `_plan_order`).
fn plan_order(
    cfg: &DagConfig,
    planner: Planner,
    est: &HashMap<String, f64>,
    bottom: &HashMap<String, f64>,
) -> Vec<String> {
    let mut tags: Vec<String> = cfg.steps.iter().map(|s| s.tag()).collect();
    match planner {
        Planner::CriticalPath => {
            // by bottom_level DESC, ties by tag ASC.
            tags.sort_by(|a, b| {
                let ba = bottom.get(a).copied().unwrap_or(0.0);
                let bb = bottom.get(b).copied().unwrap_or(0.0);
                bb.partial_cmp(&ba)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.cmp(b))
            });
        }
        Planner::GreedyLpt => {
            // STABLE reverse sort by est (ties keep registration/cfg order), matching Python's
            // sorted(..., reverse=True). Vec::sort_by is stable.
            tags.sort_by(|a, b| {
                let ea = est.get(a).copied().unwrap_or(0.0);
                let eb = est.get(b).copied().unwrap_or(0.0);
                eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
    tags
}

/// Resolve every step's estimate and build the plan for `planner` (mirrors Python's `build_plan`).
pub fn build_plan(
    cfg: &DagConfig,
    store_samples: &HashMap<String, StepSamples>,
    planner: Planner,
    min_samples: i64,
) -> Plan {
    let mut resolved: HashMap<String, Resolved> = HashMap::new();
    let mut est: HashMap<String, f64> = HashMap::new();
    for step in &cfg.steps {
        let r = resolved_estimate(step, store_samples.get(&step.tag()), min_samples);
        est.insert(step.tag(), r.est);
        resolved.insert(step.tag(), r);
    }
    let succ = successors(cfg);
    let bottom = bottom_levels(cfg, &est, &succ);
    let (critical, length) = critical_path(cfg, &bottom, &succ);
    let order = plan_order(cfg, planner, &est, &bottom);
    let entries: Vec<PlanEntry> = cfg
        .steps
        .iter()
        .map(|step| {
            let tag = step.tag();
            let r = &resolved[&tag];
            PlanEntry {
                tag: tag.clone(),
                est_duration_s: r.est,
                est_source: r.est_source.to_string(),
                rss_estimate_bytes: r.rss,
                rss_source: r.rss_source.to_string(),
                bottom_level_s: bottom.get(&tag).copied().unwrap_or(0.0),
                samples: r.samples,
            }
        })
        .collect();
    Plan {
        planner,
        order,
        critical_path: critical,
        critical_path_length_s: length,
        entries,
    }
}

/// Return a copy of `cfg` whose per-step hints carry the plan's resolved estimates. `rss` is
/// overridden ONLY when the store won (mirrors Python's `apply_plan_to_config`).
pub fn apply_plan_to_config(cfg: &DagConfig, plan: &Plan) -> DagConfig {
    let by_tag = plan.by_tag();
    let mut new_cfg = cfg.clone();
    new_cfg.steps = cfg
        .steps
        .iter()
        .map(|step| {
            let tag = step.tag();
            let mut s = step.clone();
            if let Some(entry) = by_tag.get(&tag) {
                let rss = if entry.rss_source == "store" {
                    entry.rss_estimate_bytes
                } else {
                    step.hint.rss_baseline_bytes
                };
                s.hint = ResourceHint {
                    resources: step.hint.resources.clone(),
                    est_duration_s: entry.est_duration_s,
                    rss_baseline_bytes: rss,
                    hard_mem_max_bytes: step.hint.hard_mem_max_bytes,
                    classification: step.hint.classification,
                    preferred_inner_jobs: step.hint.preferred_inner_jobs,
                    measured_effective_cores: step.hint.measured_effective_cores,
                    measured_cpu_utilization: step.hint.measured_cpu_utilization,
                };
            }
            s
        })
        .collect();
    new_cfg
}

// --------------------------------------------------------------------------- rendering

/// Fixed 3-decimal seconds, byte-identical to the Python `f"{value:.3f}"`.
fn fmt_secs(value: f64) -> String {
    format!("{value:.3}")
}

fn human_bytes(n: Option<i64>) -> String {
    let mut value = match n {
        Some(v) => v as f64,
        None => return "-".to_string(),
    };
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if value < 1024.0 || unit == "TiB" {
            return if unit == "B" {
                format!("{} B", value as i64)
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{} B", n.unwrap_or(0))
}

fn json_str_list(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let inner: Vec<String> = items
        .iter()
        .map(|item| format!("    {}", json_str(item)))
        .collect();
    format!("[\n{}\n  ]", inner.join(",\n"))
}

/// Canonical, machine-readable plan JSON (2-space indent), byte-identical to Python's
/// `plan_to_json`. Computed floats are emitted as fixed-3-decimal STRINGS.
pub fn plan_to_json(plan: &Plan) -> String {
    let by_tag = plan.by_tag();
    let mut parts: Vec<String> = vec![
        "{".to_string(),
        format!("  \"planner\": {},", json_str(plan.planner.value())),
        format!(
            "  \"critical_path\": {},",
            json_str_list(&plan.critical_path)
        ),
        format!(
            "  \"critical_path_length_s\": \"{}\",",
            fmt_secs(plan.critical_path_length_s)
        ),
        format!("  \"order\": {},", json_str_list(&plan.order)),
    ];
    let mut steps_json: Vec<String> = Vec::with_capacity(plan.order.len());
    for tag in &plan.order {
        let entry = by_tag[tag];
        let rss = match entry.rss_estimate_bytes {
            Some(n) => n.to_string(),
            None => "null".to_string(),
        };
        steps_json.push(format!(
            "    {{\n      \"tag\": {},\n      \"est_duration_s\": \"{}\",\n      \"est_source\": {},\n      \"rss_estimate_bytes\": {},\n      \"rss_source\": {},\n      \"bottom_level_s\": \"{}\",\n      \"samples\": {}\n    }}",
            json_str(&entry.tag),
            fmt_secs(entry.est_duration_s),
            json_str(&entry.est_source),
            rss,
            json_str(&entry.rss_source),
            fmt_secs(entry.bottom_level_s),
            entry.samples,
        ));
    }
    if steps_json.is_empty() {
        parts.push("  \"steps\": []".to_string());
    } else {
        parts.push("  \"steps\": [".to_string());
        parts.push(steps_json.join(",\n"));
        parts.push("  ]".to_string());
    }
    parts.push("}".to_string());
    parts.join("\n")
}

/// A compact, human-readable plan for the terminal, byte-identical to Python's `plan_to_text`.
pub fn plan_to_text(plan: &Plan) -> String {
    let by_tag = plan.by_tag();
    let headers = [
        "step",
        "est_duration_s",
        "source",
        "rss_estimate",
        "rss_source",
        "bottom_level_s",
        "samples",
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(plan.order.len());
    for tag in &plan.order {
        let entry = by_tag[tag];
        rows.push(vec![
            tag.clone(),
            fmt_secs(entry.est_duration_s),
            entry.est_source.clone(),
            human_bytes(entry.rss_estimate_bytes),
            entry.rss_source.clone(),
            fmt_secs(entry.bottom_level_s),
            entry.samples.to_string(),
        ]);
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let fmt_row = |cells: &[String]| -> String {
        let mut parts: Vec<String> = Vec::with_capacity(headers.len());
        for (i, cell) in cells.iter().enumerate() {
            let w = widths[i];
            if i == 0 {
                parts.push(format!("{cell:<w$}"));
            } else {
                parts.push(format!("{cell:>w$}"));
            }
        }
        parts.join("  ")
    };
    let header_cells: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    let mut lines: Vec<String> = vec![
        format!("plan: {}", plan.planner.value()),
        "per-step estimates (source: store = learned from the profile store; hint = DAG-authored; default = none):".to_string(),
        fmt_row(&header_cells),
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  "),
    ];
    for row in &rows {
        lines.push(fmt_row(row));
    }
    let crit = if plan.critical_path.is_empty() {
        "(none)".to_string()
    } else {
        plan.critical_path.join(" -> ")
    };
    lines.push(format!(
        "critical path ({}s): {}",
        fmt_secs(plan.critical_path_length_s),
        crit
    ));
    let order = if plan.order.is_empty() {
        "(none)".to_string()
    } else {
        plan.order.join(", ")
    };
    lines.push(format!("scheduled order: {order}"));
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ResourceHint;
    use std::collections::BTreeMap;

    fn mk(group: &str, job: &str, deps: &[&str], est: f64) -> Step {
        Step {
            group: group.into(),
            job: job.into(),
            desc: String::new(),
            description: String::new(),
            cmd: "true".into(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            hint: ResourceHint {
                est_duration_s: est,
                ..Default::default()
            },
            networkonly: false,
            engine_only: false,
            timeout: 1800,
            jobs_flag: None,
        }
    }

    #[test]
    fn robust_median_discards_outlier() {
        // one huge outlier must not move the median (MAD-trim).
        assert_eq!(robust_median(&[3.0, 3.0, 3.0, 3.0, 100.0]), 3.0);
        assert_eq!(robust_median(&[2.0, 4.0]), 3.0);
    }

    #[test]
    fn high_percentile_nearest_rank() {
        assert_eq!(high_percentile(&[10]), 10);
        // n=10, rank = (9*10+9)/10 = 9 -> 9th smallest (1-indexed) = 90.
        assert_eq!(
            high_percentile(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100]),
            90
        );
        // n=3, rank = (27+9)/10 = 3 -> the max.
        assert_eq!(high_percentile(&[5, 9, 7]), 9);
    }

    #[test]
    fn contention_discounts_duration() {
        // A step run under 50% other-work contention: intrinsic ~= elapsed * 0.5.
        let mut row: HashMap<String, String> = HashMap::new();
        row.insert("pct_other".into(), "50.0".into());
        assert!((contention_fraction(&row, Some(8)) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn critical_path_planner_differs_from_lpt() {
        // prep(1) -> heavy(10); solo(5) independent. LPT dispatches heavy/solo/prep by est;
        // critical-path dispatches prep first (bottom_level 11 > solo 5 > heavy 10 order).
        let cfg = DagConfig {
            steps: vec![
                mk("g", "prep", &[], 1.0),
                mk("g", "heavy", &["g.prep"], 10.0),
                mk("g", "solo", &[], 5.0),
            ],
            ..Default::default()
        };
        let empty: HashMap<String, StepSamples> = HashMap::new();
        let lpt = build_plan(&cfg, &empty, Planner::GreedyLpt, DEFAULT_MIN_SAMPLES);
        let cp = build_plan(&cfg, &empty, Planner::CriticalPath, DEFAULT_MIN_SAMPLES);
        assert_eq!(lpt.order, vec!["g.heavy", "g.solo", "g.prep"]);
        assert_eq!(cp.order, vec!["g.prep", "g.heavy", "g.solo"]);
        assert_eq!(cp.critical_path, vec!["g.prep", "g.heavy"]);
        assert_ne!(lpt.order, cp.order);
    }
}
