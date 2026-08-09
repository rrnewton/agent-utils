//! Profile-derived runtime estimates and execution planners.

// Profile-store FEEDBACK: turn recorded per-step samples into planning estimates.
//
// Direct port of `py/safe_ci_dag_runner/estimates.py`; the derived numbers and the
// `plan --format json` / `plan` text output are BYTE-IDENTICAL to the Python build for a given
// store + DAG (cross-tested in `cross/differential.py`). This is the READING half of the
// learned-duration profile store (ds-7pzdgm / ds-afzsqf); the writing half already ships in
// [`crate::perflog`].
//
// For the current `(machine_id, container_class)` it derives, per step:
// * `est_duration_s` — a contention-discounted, MAD-trimmed MEDIAN of the recorded `elapsed_s`
//   (the step's INTRINSIC / uncontended duration). Median, not mean, so one slow sample cannot
//   drag it; discounted by whatever contention column the store carries.
// * `rss_estimate_bytes` — a robust HIGH-WATER (90th-percentile, nearest-rank via INTEGER rank
//   arithmetic so it is cross-identical) of the recorded `peak_bytes` for the memory model.
//
// Sparse/missing data degrades to `None` (the caller falls back to the DAG hint); a malformed
// cell is skipped, never coerced.

use std::collections::HashMap;
use std::path::Path;

use crate::io::json_str;
use crate::model::{DagConfig, ResourceHint, Step};
use crate::perflog::{container_class, machine_id, parse_csv_line};
use crate::sizing::schedulable_peak_mem_bytes_widths;

/// Environment variable that overrides the machine component of the feedback identity.
pub const MACHINE_ID_ENV: &str = "SAFE_CI_DAG_RUNNER_MACHINE_ID";
/// Environment variable that overrides the container-class component of the feedback identity.
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

// Contention percentage columns understood by the reader, in priority order (see Python's
// `_CONTENTION_PCT_COLUMNS`).
const CONTENTION_PCT_COLUMNS: [&str; 3] =
    ["pct_other", "psi_cpu_some_avg10", "cpu_pressure_some_avg10"];

/// Which scheduling planner to use for dispatch ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Planner {
    /// Dispatch longest estimated steps first, breaking ties by tag.
    GreedyLpt,
    /// Dispatch by decreasing dependency-graph bottom level.
    CriticalPath,
    // CPA (Radulescu & van Gemund 2001): a two-phase moldable allocator that first picks each
    // step's inner-jobs width by balancing the critical path against the per-core area over the
    // MEASURED speedup curves, then list-schedules by critical-path order at the allocated widths.
    // See `common/docs/safe-ci-dag-runner/PLANNER_DESIGN.md`.
    /// Allocate measured parallel widths before critical-path list scheduling.
    Cpa,
}

impl Planner {
    /// Return the canonical command-line and serialized value.
    pub fn value(self) -> &'static str {
        match self {
            Planner::GreedyLpt => "greedy-lpt",
            Planner::CriticalPath => "critical-path",
            Planner::Cpa => "cpa",
        }
    }

    /// Parse the canonical string form, or `None` for an unknown value.
    pub fn from_value(text: &str) -> Option<Planner> {
        match text {
            "greedy-lpt" => Some(Planner::GreedyLpt),
            "critical-path" => Some(Planner::CriticalPath),
            "cpa" => Some(Planner::Cpa),
            _ => None,
        }
    }
}

/// Aggregated store estimates for ONE step, from its recorded samples.
#[derive(Debug, Clone)]
pub struct StepSamples {
    /// Fully qualified step tag.
    pub step: String,
    /// Number of profile rows aggregated for the step.
    pub samples: i64,
    /// Robust contention-adjusted wall-time estimate in seconds.
    pub est_duration_s: Option<f64>,
    /// Robust high-water resident-memory estimate in bytes.
    pub rss_estimate_bytes: Option<i64>,
}

/// One recorded step measurement reduced to the fields consumed by the estimators.
///
/// Raw wall time and contention remain separate so [`Sample::intrinsic_s`] can calculate the
/// uncontended duration used by robust aggregation.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Observed wall time in seconds, when recorded.
    pub elapsed_s: Option<f64>,
    /// Clamped fraction of the sample attributed to ambient contention.
    pub contention: f64,
    /// Total user plus system CPU time in seconds, when recorded.
    pub cpu_s: Option<f64>,
    /// Observed average CPU concurrency, when recorded.
    pub effective_cores: Option<f64>,
    /// Cgroup CPU throttling time in seconds, when recorded.
    pub throttled_s: Option<f64>,
    /// Peak resident memory in bytes, when recorded.
    pub peak_bytes: Option<i64>,
}

impl Sample {
    /// The contention-discounted (intrinsic) wall the estimator medians, or `None` when this sample
    /// carried no `elapsed_s`.
    pub fn intrinsic_s(&self) -> Option<f64> {
        self.elapsed_s.map(|e| e * (1.0 - self.contention))
    }
}

/// Aggregation key containing a step tag and its inner-job width.
///
/// Width zero represents a missing, invalid, or non-positive recorded width. Such samples still
/// contribute to step estimates but are excluded from speedup-curve fitting.
pub type BucketKey = (String, i64);

/// Reduce one raw profile row to the normalized sample consumed by all estimators.
pub fn sample_from_row(row: &HashMap<String, String>, affinity: Option<i64>) -> Sample {
    let elapsed_s = parse_float(row.get("elapsed_s")).filter(|e| *e >= 0.0);
    let cpu_s = match (
        parse_float(row.get("user_s")),
        parse_float(row.get("sys_s")),
    ) {
        (Some(u), Some(s)) if u >= 0.0 && s >= 0.0 => Some(u + s),
        _ => None,
    };
    Sample {
        elapsed_s,
        contention: contention_fraction(row, affinity),
        cpu_s,
        effective_cores: parse_float(row.get("effective_cores")).filter(|e| *e >= 0.0),
        throttled_s: parse_float(row.get("throttled_s")).filter(|t| *t >= 0.0),
        peak_bytes: parse_int(row.get("peak_bytes")).filter(|p| *p >= 0),
    }
}

/// The `inner_jobs` bucket component for a row: the parsed positive width, else `0`.
fn row_inner_jobs(row: &HashMap<String, String>) -> i64 {
    match parse_int(row.get("inner_jobs")) {
        Some(j) if j > 0 => j,
        _ => 0,
    }
}

/// Cells that, when explicitly affirmative, mean the step did NOT complete its work.
const FAILURE_FLAG_COLUMNS: [&str; 2] = ["timed_out", "cpu_timed_out"];

/// True only for an explicit affirmative cell; a blank or absent cell is NOT a failure.
fn is_truthy_flag(value: Option<&String>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("true") | Some("1") | Some("yes")
    )
}

/// True when a profile row is a TIMING MEASUREMENT rather than a record of a failed step.
///
/// A timed-out run's duration is the moment the guard fired, and an OOM-killed run's duration is
/// the moment the kernel intervened; neither is how long the work takes. Admitting them fits the
/// speedup curve partly to failures, and a step that dies fast at every width then looks exactly
/// like a step that is flat and very quick.
///
/// FAIL-OPEN ON SILENCE, BY DESIGN. Only an explicit failure signal rejects a row: `ok` explicitly
/// falsy, a non-zero parseable `returncode`, an affirmative `timed_out` / `cpu_timed_out`, or a
/// positive `oom_kills`. A row whose verdict cells are absent or blank is ACCEPTED, because a store
/// may carry no verdict columns at all and a gate that rejected silence would leave the model with
/// nothing -- trading a wrong answer for no answer.
pub fn row_is_measurement(row: &HashMap<String, String>) -> bool {
    if let Some(ok) = row.get("ok") {
        let ok = ok.trim().to_ascii_lowercase();
        if !ok.is_empty() && !matches!(ok.as_str(), "true" | "1" | "yes") {
            return false;
        }
    }
    if let Some(rc) = row.get("returncode").and_then(|v| v.trim().parse::<i64>().ok()) {
        if rc != 0 {
            return false;
        }
    }
    if FAILURE_FLAG_COLUMNS
        .iter()
        .any(|col| is_truthy_flag(row.get(*col)))
    {
        return false;
    }
    if let Some(oom) = row.get("oom_kills").and_then(|v| v.trim().parse::<i64>().ok()) {
        if oom > 0 {
            return false;
        }
    }
    true
}

/// Group raw profile rows into uncapped per-step, per-width sample lists.
///
/// Rows without a non-empty `step` cell are ignored, and so are rows recording a FAILED step
/// rather than a measurement (see [`row_is_measurement`]): a timed-out or OOM-killed duration is
/// not a timing.
pub fn bucketize_rows(
    rows: &[HashMap<String, String>],
    affinity: Option<i64>,
) -> HashMap<BucketKey, Vec<Sample>> {
    let mut buckets: HashMap<BucketKey, Vec<Sample>> = HashMap::new();
    for row in rows {
        let step = match row.get("step") {
            Some(s) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        if !row_is_measurement(row) {
            continue;
        }
        let key: BucketKey = (step, row_inner_jobs(row));
        buckets
            .entry(key)
            .or_default()
            .push(sample_from_row(row, affinity));
    }
    buckets
}

/// Aggregate sample buckets into robust per-step estimates across every recorded width.
pub fn step_samples_from_buckets(
    buckets: &HashMap<BucketKey, Vec<Sample>>,
) -> HashMap<String, StepSamples> {
    let mut durations: HashMap<String, Vec<f64>> = HashMap::new();
    let mut peaks: HashMap<String, Vec<i64>> = HashMap::new();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for ((step, _inner), samples) in buckets {
        *counts.entry(step.clone()).or_insert(0) += samples.len() as i64;
        for sample in samples {
            if let Some(intrinsic) = sample.intrinsic_s() {
                durations.entry(step.clone()).or_default().push(intrinsic);
            }
            if let Some(peak) = sample.peak_bytes {
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

/// The `(machine_id, container_class)` the feedback reader selects the store file by: the current
/// host's identity unless the corresponding environment overrides are set.
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

// A robust central estimate of contention-/noise-inflated samples (mirrors Python's
// `_robust_median`). At THREE or more samples: MAD-trimmed median (drop samples more than
// `MAD_TRIM_K` MADs from the median, falling back to the plain median when the MAD is zero). At
// FEWER than three samples MAD-trim provably cannot reject an outlier — the median of two points
// sits midway between them, so any symmetric cutoff keeps BOTH and the estimate collapses to their
// MEAN, which a single slow sample drags by half its excess (`[5, 100] -> 52.5`, inverting a real
// speedup). Since these quantities can only be INFLATED by contention/noise, the smaller
// observation is the better intrinsic estimate, so at `n < 3` we return the MINIMUM: robust to one
// upward (slow/contended) outlier at `n == 2` and self-healing to the MAD-trimmed median as samples
// accumulate. Caller guarantees a non-empty slice.
fn robust_median(values: &[f64]) -> f64 {
    let mut xs = values.to_vec();
    sort_f64(&mut xs);
    if xs.len() < 3 {
        return xs[0]; // sorted ascending -> min (robust to a slow outlier; see doc comment)
    }
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

// The 90th percentile of a non-empty int slice by NEAREST-RANK with integer arithmetic (mirrors
// Python's `_high_percentile`): `rank = ceil(num*n/den) == (num*n + den - 1) / den`, clamped to
// `1..=n`, returning the rank-th smallest.
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
/// `316`. `None` if the shape is unexpected. Shared with the summary (its speedup core budget).
pub(crate) fn affinity_width(container_class: &str) -> Option<i64> {
    let rest = container_class.strip_prefix("affinity")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

// Trim the surrounding ASCII whitespace that Python's `str.strip` removes over the same
// five-character set (tab, newline, form-feed, carriage-return, space) and return the cleaned
// token, or `None` when it is empty. Rust's `str::parse` already rejects the non-ASCII characters,
// `_` separators, and out-of-`i64` magnitudes that Python's permissive `float()` / `int()` would
// otherwise accept, so trimming is all that is needed for the two builds to accept EXACTLY the same
// numeric tokens from a store cell (mirrors `_clean_numeric_cell` in the Python port).
fn clean_numeric_cell(cell: Option<&String>) -> Option<&str> {
    let token = cell?.trim_matches(|c: char| c.is_ascii_whitespace());
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn parse_float(cell: Option<&String>) -> Option<f64> {
    // Reject non-finite values (`inf`/`-inf`/`nan`, and overflowing literals like `1e400` -> inf):
    // Python's `float()` accepts the same set, so both builds must drop them to stay byte-identical
    // AND to keep one bogus cell from poisoning a median or a contention fraction.
    clean_numeric_cell(cell)
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn parse_int(cell: Option<&String>) -> Option<i64> {
    clean_numeric_cell(cell).and_then(|t| t.parse::<i64>().ok())
}

// The fraction of machine capacity taken by OTHER work during a sample, from whichever
// contention column is present (see [`CONTENTION_PCT_COLUMNS`]). `0.0` when no usable signal;
// clamped to [`MAX_CONTENTION`]. Mirrors Python's `_contention_fraction`.
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

/// A loaded store: the column->value row maps plus the parsed affinity (core) width.
pub(crate) type LoadedStore = (Vec<HashMap<String, String>>, Option<i64>);

// Read `<profile_dir>/step_profiles_<machine_id>_<container_class>.csv` into a list of
// column->value row maps plus the parsed affinity (core) width, or `None` when the file is absent.
// The single CSV-reading path shared by [`load_step_samples`] and [`load_step_speedups`] (DRY);
// mirrors Python's `_load_store`. Also used by the summary builder to read a store into rows.
pub(crate) fn load_store(
    profile_dir: &Path,
    machine_id: &str,
    container_class: &str,
) -> Option<LoadedStore> {
    let path = profile_dir.join(format!("step_profiles_{machine_id}_{container_class}.csv"));
    let text = std::fs::read_to_string(&path).ok()?;
    let affinity = affinity_width(container_class);
    let mut lines = text.lines();
    let header: Vec<String> = match lines.next() {
        Some(h) => parse_csv_line(h),
        None => return Some((Vec::new(), affinity)),
    };
    let mut rows: Vec<HashMap<String, String>> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let cells = parse_csv_line(line);
        let row: HashMap<String, String> = header
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), cells.get(i).cloned().unwrap_or_default()))
            .collect();
        rows.push(row);
    }
    Some((rows, affinity))
}

/// Load and aggregate profile samples for one machine and container class.
///
/// Returns an empty map when the corresponding profile file is absent.
pub fn load_step_samples(
    profile_dir: &Path,
    machine_id: &str,
    container_class: &str,
) -> HashMap<String, StepSamples> {
    let (rows, affinity) = match load_store(profile_dir, machine_id, container_class) {
        Some(x) => x,
        None => return HashMap::new(),
    };
    step_samples_from_buckets(&bucketize_rows(&rows, affinity))
}

// --------------------------------------------------------------------------- speedup model

/// A level must be at least this many times faster than the previous (fewer-thread) level to make
/// the extra threads worthwhile; below this the marginal speedup has plateaued (a knee).
const SPEEDUP_MIN_MARGINAL_GAIN: f64 = 1.15;
/// If total CPU-seconds grow by more than this factor between two consecutive levels, the step does
/// materially more total work per added thread (a work-conservation stop signal).
const SPEEDUP_MAX_WORK_GROWTH: f64 = 1.5;
/// A step needs at least this many DISTINCT inner_jobs levels (with wall data) to model a curve.
const SPEEDUP_MIN_LEVELS: usize = 2;

/// One measured point on a step's parallel speedup curve.
#[derive(Debug, Clone)]
pub struct SpeedupLevel {
    /// Inner worker width used by the samples.
    pub inner_jobs: i64,
    /// Number of wall-time samples contributing to this level.
    pub samples: i64,
    /// Robust contention-adjusted wall time in seconds.
    pub wall_s: f64,
    /// Robust total CPU time in seconds, when available.
    pub cpu_s: Option<f64>,
    /// Robust observed CPU concurrency, when available.
    pub effective_cores: Option<f64>,
    /// Robust throttling time in seconds, when available.
    pub throttled_s: Option<f64>,
    /// Wall-time speedup relative to the narrowest measured level.
    pub speedup: f64,
}

/// A fitted step speedup curve and its recommended worker width.
#[derive(Debug, Clone)]
pub struct StepSpeedup {
    /// Fully qualified step tag.
    pub step: String,
    /// Narrowest measured inner-job width.
    pub baseline_inner_jobs: i64,
    /// Widest measured width before the scaling knee or configured core limit.
    pub recommended_inner_jobs: i64,
    /// Effective CPU concurrency measured at the recommended width.
    pub measured_effective_cores: Option<f64>,
    /// Curve levels ordered by increasing inner-job width.
    pub levels: Vec<SpeedupLevel>,
}

/// Per-level `(inner_jobs, samples, wall, cpu, effective_cores, throttled)` tuple.
type RawLevel = (i64, i64, f64, Option<f64>, Option<f64>, Option<f64>);

// Assemble a [`StepSpeedup`] from per-level tuples SORTED ascending by inner_jobs. Deterministic
// across builds (only compares robust medians of identical inputs). Mirrors Python's
// `_build_step_speedup`.
fn build_step_speedup(
    step: String,
    raw_levels: &[RawLevel],
    core_budget: Option<i64>,
) -> StepSpeedup {
    let baseline_j = raw_levels[0].0;
    let baseline_wall = raw_levels[0].2;
    let mut levels: Vec<SpeedupLevel> = Vec::with_capacity(raw_levels.len());
    let mut recommended = baseline_j;
    let mut still_scaling = true;
    let mut prev_wall = baseline_wall;
    let mut prev_cpu = raw_levels[0].3;
    let mut eff_by_j: HashMap<i64, Option<f64>> = HashMap::new();
    for (idx, &(j, n, wall, cpu, eff, throttled)) in raw_levels.iter().enumerate() {
        let speedup = if wall > 0.0 {
            baseline_wall / wall
        } else {
            1.0
        };
        levels.push(SpeedupLevel {
            inner_jobs: j,
            samples: n,
            wall_s: wall,
            cpu_s: cpu,
            effective_cores: eff,
            throttled_s: throttled,
            speedup,
        });
        eff_by_j.insert(j, eff);
        if idx > 0 && still_scaling {
            let gain = if wall > 0.0 { prev_wall / wall } else { 1.0 };
            let work_growth = match (cpu, prev_cpu) {
                (Some(c), Some(pc)) if pc > 0.0 => Some(c / pc),
                _ => None,
            };
            let within_budget = core_budget.is_none_or(|b| j <= b);
            let work_ok = work_growth.is_none_or(|w| w <= SPEEDUP_MAX_WORK_GROWTH);
            if gain >= SPEEDUP_MIN_MARGINAL_GAIN && work_ok && within_budget {
                recommended = j;
            } else {
                still_scaling = false;
            }
        }
        prev_wall = wall;
        prev_cpu = cpu;
    }
    StepSpeedup {
        step,
        baseline_inner_jobs: baseline_j,
        recommended_inner_jobs: recommended,
        measured_effective_cores: eff_by_j.get(&recommended).copied().flatten(),
        levels,
    }
}

/// Load and fit parallel speedup curves from a profile store.
///
/// Only steps with enough distinct positive widths receive a model. The recommended width remains
/// within both the measured scaling knee and the machine core budget encoded by the container
/// class.
pub fn load_step_speedups(
    profile_dir: &Path,
    machine_id: &str,
    container_class: &str,
) -> HashMap<String, StepSpeedup> {
    let (rows, affinity) = match load_store(profile_dir, machine_id, container_class) {
        Some(x) => x,
        None => return HashMap::new(),
    };
    step_speedups_from_buckets(&bucketize_rows(&rows, affinity), affinity)
}

/// Fit parallel speedup curves from normalized per-step, per-width buckets.
pub fn step_speedups_from_buckets(
    buckets: &HashMap<BucketKey, Vec<Sample>>,
    core_budget: Option<i64>,
) -> HashMap<String, StepSpeedup> {
    let mut walls: HashMap<BucketKey, Vec<f64>> = HashMap::new();
    let mut cpus: HashMap<BucketKey, Vec<f64>> = HashMap::new();
    let mut effs: HashMap<BucketKey, Vec<f64>> = HashMap::new();
    let mut thrs: HashMap<BucketKey, Vec<f64>> = HashMap::new();
    for ((step, inner), samples) in buckets {
        if *inner <= 0 {
            continue;
        }
        let key: BucketKey = (step.clone(), *inner);
        for sample in samples {
            if let Some(intrinsic) = sample.intrinsic_s() {
                walls.entry(key.clone()).or_default().push(intrinsic);
            }
            if let Some(c) = sample.cpu_s {
                cpus.entry(key.clone()).or_default().push(c);
            }
            if let Some(e) = sample.effective_cores {
                effs.entry(key.clone()).or_default().push(e);
            }
            if let Some(t) = sample.throttled_s {
                thrs.entry(key.clone()).or_default().push(t);
            }
        }
    }
    let mut by_step: HashMap<String, Vec<i64>> = HashMap::new();
    for (step, inner) in walls.keys() {
        by_step.entry(step.clone()).or_default().push(*inner);
    }
    let mut result: HashMap<String, StepSpeedup> = HashMap::new();
    for (step, widths) in by_step {
        let mut levels_j = widths;
        levels_j.sort_unstable();
        levels_j.dedup();
        if levels_j.len() < SPEEDUP_MIN_LEVELS {
            continue;
        }
        let mut raw_levels: Vec<RawLevel> = Vec::with_capacity(levels_j.len());
        for inner in &levels_j {
            let key = (step.clone(), *inner);
            let wall_samples = &walls[&key];
            let median_of = |m: &HashMap<BucketKey, Vec<f64>>| -> Option<f64> {
                m.get(&key)
                    .filter(|v| !v.is_empty())
                    .map(|v| robust_median(v))
            };
            raw_levels.push((
                *inner,
                wall_samples.len() as i64,
                robust_median(wall_samples),
                median_of(&cpus),
                median_of(&effs),
                median_of(&thrs),
            ));
        }
        let model = build_step_speedup(step.clone(), &raw_levels, core_budget);
        result.insert(step, model);
    }
    result
}

// --------------------------------------------------------------------------- planner

/// The resolved estimate + planner metadata for one step (what the plan display shows).
#[derive(Debug, Clone)]
pub struct PlanEntry {
    /// Fully qualified step tag.
    pub tag: String,
    /// Resolved step duration in seconds.
    pub est_duration_s: f64,
    /// Source label for the duration estimate.
    pub est_source: String,
    /// Resolved resident-memory estimate in bytes.
    pub rss_estimate_bytes: Option<i64>,
    /// Source label for the memory estimate.
    pub rss_source: String,
    /// Longest estimated path from this step to a sink, in seconds.
    pub bottom_level_s: f64,
    /// Number of stored profile samples for this step.
    pub samples: i64,
    /// The learned parallel-speedup curve for this step, or `None` when the store has fewer than
    /// two inner_jobs widths for it.
    pub speedup: Option<StepSpeedup>,
    /// The inner-jobs width the CPA allocator assigned this step, or `None` for the non-allocating
    /// planners. This is the `-j` the step actually runs with under `--planner cpa` (baked into the
    /// command via [`apply_plan_to_config`]).
    pub alloc_inner_jobs: Option<i64>,
}

/// Whole-DAG metrics produced by the parallel-width allocator.
#[derive(Debug, Clone)]
pub struct Allocation {
    /// Total core budget used by allocation and simulation.
    pub core_budget: i64,
    /// Sum of allocated width multiplied by modeled wall time.
    pub area_s: f64,
    /// Work-area lower bound, `area_s / core_budget`.
    pub area_bound_s: f64,
    /// Modeled duration of the allocated critical path.
    pub critical_path_s: f64,
    /// Maximum of the work-area and critical-path lower bounds.
    pub lower_bound_s: f64,
    /// Makespan from deterministic list-schedule simulation.
    pub modeled_makespan_s: f64,
    /// Stable reason the allocation loop stopped widening steps.
    pub stop_reason: String,
}

/// A complete plan: per-step resolved estimates, the dispatch order, and the critical path.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Planner used to produce this result.
    pub planner: Planner,
    /// Deterministic dispatch order of fully qualified step tags.
    pub order: Vec<String>,
    /// Fully qualified tags on the estimated critical path.
    pub critical_path: Vec<String>,
    /// Estimated critical-path duration in seconds.
    pub critical_path_length_s: f64,
    /// Per-step estimates and allocation metadata.
    pub entries: Vec<PlanEntry>,
    /// The CPA allocator summary (`--planner cpa` only), else `None`.
    pub allocation: Option<Allocation>,
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

// The longest est-weighted path through the DAG (mirrors Python's `_critical_path`).
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

// The deterministic dispatch order (mirrors Python's `_plan_order`).
fn plan_order(
    cfg: &DagConfig,
    planner: Planner,
    est: &HashMap<String, f64>,
    bottom: &HashMap<String, f64>,
) -> Vec<String> {
    let mut tags: Vec<String> = cfg.steps.iter().map(|s| s.tag()).collect();
    match planner {
        // CPA list-schedules by critical-path order at the allocated weights, so it uses the same
        // bottom-level ordering as CriticalPath (PLANNER_DESIGN.md §5.7).
        Planner::CriticalPath | Planner::Cpa => {
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

// --------------------------------------------------------------------------- CPA allocator

// Stop-reason labels the CPA gradient loop can end on (PLANNER_DESIGN.md §5.8). Deterministic
// given the same store + DAG + budgets, so the two builds report the same one bit-for-bit.
const CPA_BALANCED: &str = "balanced";
const CPA_KNEE_EXHAUSTED: &str = "knee-exhausted";
const CPA_CORE_CAPPED: &str = "core-capped";
const CPA_MEM_CAPPED: &str = "mem-capped";
const CPA_FIXED_POINT: &str = "fixed-point";

// Per-step admissible width set `W_i` (ascending) and the MEASURED wall `T_i(p)` at each admissible
// width. Mirrors Python's `_cpa_admissible` (PLANNER_DESIGN.md §5.2): a curve step admits its
// measured widths up to the knee (`recommended_inner_jobs`) and the core budget `P`; a curveless
// step is rigid at `min(hint or 1, P)` with `T_i` the resolved scalar estimate.
#[allow(clippy::type_complexity)]
fn cpa_admissible(
    cfg: &DagConfig,
    speedups: &HashMap<String, StepSpeedup>,
    est: &HashMap<String, f64>,
    core_budget: i64,
) -> (
    HashMap<String, Vec<i64>>,
    HashMap<String, HashMap<i64, f64>>,
) {
    let mut admissible: HashMap<String, Vec<i64>> = HashMap::new();
    let mut wall: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for step in &cfg.steps {
        let tag = step.tag();
        match speedups.get(&tag) {
            Some(sp) if !sp.levels.is_empty() => {
                let knee_ok: Vec<&SpeedupLevel> = sp
                    .levels
                    .iter()
                    .filter(|l| l.inner_jobs <= sp.recommended_inner_jobs)
                    .collect();
                let within: Vec<&SpeedupLevel> = knee_ok
                    .iter()
                    .copied()
                    .filter(|l| l.inner_jobs <= core_budget)
                    .collect();
                // Fall back to the narrowest measured width if EVERY admissible width exceeds P.
                let chosen: Vec<&SpeedupLevel> = if within.is_empty() {
                    knee_ok[..1].to_vec()
                } else {
                    within
                };
                let mut widths: Vec<i64> = chosen.iter().map(|l| l.inner_jobs).collect();
                widths.sort_unstable();
                let w: HashMap<i64, f64> =
                    chosen.iter().map(|l| (l.inner_jobs, l.wall_s)).collect();
                admissible.insert(tag.clone(), widths);
                wall.insert(tag, w);
            }
            _ => {
                let pref = step.hint.preferred_inner_jobs;
                let mut ww = match pref {
                    Some(p) if p > 0 => p,
                    _ => 1,
                };
                if core_budget > 0 && ww > core_budget {
                    ww = core_budget;
                }
                if ww < 1 {
                    ww = 1;
                }
                admissible.insert(tag.clone(), vec![ww]);
                let mut w: HashMap<i64, f64> = HashMap::new();
                w.insert(ww, est.get(&tag).copied().unwrap_or(0.0));
                wall.insert(tag, w);
            }
        }
    }
    (admissible, wall)
}

/// The next-larger admissible width after `current` in the ascending `widths`, or `None`.
fn cpa_next_width(widths: &[i64], current: i64) -> Option<i64> {
    widths.iter().copied().find(|&w| w > current)
}

// Worst-case concurrent memory footprint (bytes) with the given PER-STEP widths (PLANNER_DESIGN.md
// §5.6).
fn cpa_footprint(cfg: &DagConfig, widths: &HashMap<String, i64>) -> i64 {
    schedulable_peak_mem_bytes_widths(cfg, cfg.steps.len() as i64, widths).0
}

/// `(widths, admissible, wall, stop_reason)` from the CPA gradient loop.
type CpaResult = (
    HashMap<String, i64>,
    HashMap<String, Vec<i64>>,
    HashMap<String, HashMap<i64, f64>>,
    String,
);

// The CPA gradient loop (PLANNER_DESIGN.md §5.3-5.8), mirroring Python's `_cpa_allocate`: seed
// every step at `min(W_i)` then, while `T_CP > area/P`, widen the critical-path step with the
// greatest measured wall-reduction-per-added-core (ties by smallest tag) subject to the core and
// memory constraints, until the terms balance or nothing can widen. Deterministic across builds
// (identical f64 ops in cfg order, integer core arithmetic, total tag tie-break).
fn cpa_allocate(
    cfg: &DagConfig,
    speedups: &HashMap<String, StepSpeedup>,
    est: &HashMap<String, f64>,
    core_budget: i64,
    mem_budget: Option<i64>,
) -> CpaResult {
    let p = if core_budget > 0 { core_budget } else { 1 };
    let (admissible, wall) = cpa_admissible(cfg, speedups, est, p);
    let mut widths: HashMap<String, i64> = cfg
        .steps
        .iter()
        .map(|s| {
            let tag = s.tag();
            let w0 = admissible[&tag][0];
            (tag, w0)
        })
        .collect();
    let succ = successors(cfg);
    let mut stop_reason = CPA_FIXED_POINT.to_string();
    let max_iters: usize = cfg
        .steps
        .iter()
        .map(|s| admissible[&s.tag()].len().saturating_sub(1))
        .sum::<usize>()
        + 2;
    for _ in 0..max_iters {
        let weight: HashMap<String, f64> = cfg
            .steps
            .iter()
            .map(|s| {
                let tag = s.tag();
                let w = widths[&tag];
                (tag.clone(), wall[&tag][&w])
            })
            .collect();
        let bottom = bottom_levels(cfg, &weight, &succ);
        let (cp, t_cp) = critical_path(cfg, &bottom, &succ);
        let mut area = 0.0;
        for s in &cfg.steps {
            let tag = s.tag();
            area += widths[&tag] as f64 * weight[&tag];
        }
        let t_a = area / p as f64;
        if t_cp <= t_a {
            stop_reason = CPA_BALANCED.to_string();
            break;
        }
        let mut widenable: Vec<String> = cfg
            .steps
            .iter()
            .map(|s| s.tag())
            .filter(|tag| {
                cp.contains(tag) && cpa_next_width(&admissible[tag], widths[tag]).is_some()
            })
            .collect();
        if widenable.is_empty() {
            stop_reason = CPA_KNEE_EXHAUSTED.to_string();
            break;
        }
        widenable.sort();
        let mut best_tag: Option<String> = None;
        let mut best_gain = 0.0f64;
        let mut blocked_mem = false;
        // Iterate tag-ascending and keep the FIRST maximum, so ties resolve to the smallest tag.
        for tag in &widenable {
            let nxt = cpa_next_width(&admissible[tag], widths[tag]).unwrap();
            if nxt > p {
                continue; // single-step core cap (cross-step budget is the dispatch gate)
            }
            if let Some(budget) = mem_budget {
                let mut tentative = widths.clone();
                tentative.insert(tag.clone(), nxt);
                if cpa_footprint(cfg, &tentative) > budget {
                    blocked_mem = true;
                    continue;
                }
            }
            let cur = widths[tag];
            let gain = (wall[tag][&cur] - wall[tag][&nxt]) / (nxt - cur) as f64;
            if best_tag.is_none() || gain > best_gain {
                best_tag = Some(tag.clone());
                best_gain = gain;
            }
        }
        match best_tag {
            None => {
                stop_reason = if blocked_mem {
                    CPA_MEM_CAPPED.to_string()
                } else {
                    CPA_CORE_CAPPED.to_string()
                };
                break;
            }
            Some(tag) => {
                let nxt = cpa_next_width(&admissible[&tag], widths[&tag]).unwrap();
                widths.insert(tag, nxt);
            }
        }
    }
    (widths, admissible, wall, stop_reason)
}

/// Allocate an inner-job width to every step using measured speedup curves.
///
/// The allocator balances critical-path reduction against total work while respecting the core
/// and optional memory budgets.
pub fn allocate_widths(
    cfg: &DagConfig,
    speedups: &HashMap<String, StepSpeedup>,
    est: &HashMap<String, f64>,
    core_budget: i64,
    mem_budget: Option<i64>,
) -> HashMap<String, i64> {
    cpa_allocate(cfg, speedups, est, core_budget, mem_budget).0
}

// A deterministic greedy list-schedule of the allocated widths -> the MODELED makespan (mirrors
// Python's `_cpa_simulate_makespan`). Launches ready steps (deps done, core budget
// `Σ running widths + p_i <= P`, named resources free) in `order`, advancing to the next finish
// event; a step wider than the whole budget runs alone (deadlock guard). Respecting deps AND the
// core budget makes the result `>= max(T_CP, area/P)` (PLANNER_DESIGN.md §2). Same f64 ops in
// canonical `order` as the Python build, so the 3-decimal makespan is byte-identical.
fn cpa_simulate_makespan(
    cfg: &DagConfig,
    widths: &HashMap<String, i64>,
    weight: &HashMap<String, f64>,
    order: &[String],
    core_budget: i64,
) -> f64 {
    let p = if core_budget > 0 { core_budget } else { 1 };
    let by_tag: HashMap<String, &Step> = cfg.steps.iter().map(|s| (s.tag(), s)).collect();
    let mut done: HashMap<String, f64> = HashMap::new();
    let mut running: HashMap<String, f64> = HashMap::new();
    let mut res_avail: HashMap<String, i64> = cfg
        .resource_caps
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let mut cores_used: i64 = 0;
    let mut now = 0.0f64;
    let mut pending: std::collections::HashSet<String> =
        cfg.steps.iter().map(|s| s.tag()).collect();
    while !pending.is_empty() || !running.is_empty() {
        let mut launched = true;
        while launched {
            launched = false;
            for tag in order {
                if !pending.contains(tag) {
                    continue;
                }
                let step = by_tag[tag];
                if !step.deps.iter().all(|d| done.contains_key(d)) {
                    continue;
                }
                let w = widths[tag];
                if !(cores_used + w <= p || cores_used == 0) {
                    continue;
                }
                let res_free = step
                    .hint
                    .resources
                    .iter()
                    .all(|(r, n)| res_avail.get(r).copied().unwrap_or(0) >= *n);
                if !res_free {
                    continue;
                }
                running.insert(tag.clone(), now + weight[tag]);
                pending.remove(tag);
                cores_used += w;
                for (r, n) in &step.hint.resources {
                    *res_avail.entry(r.clone()).or_insert(0) -= n;
                }
                launched = true;
            }
        }
        if running.is_empty() {
            break;
        }
        let finish = running.values().copied().fold(f64::INFINITY, f64::min);
        now = finish;
        for tag in order {
            if running.get(tag).copied() == Some(finish) {
                done.insert(tag.clone(), finish);
                running.remove(tag);
                cores_used -= widths[tag];
                let step = by_tag[tag];
                for (r, n) in &step.hint.resources {
                    *res_avail.entry(r.clone()).or_insert(0) += n;
                }
            }
        }
    }
    done.values().copied().fold(0.0, f64::max)
}

// Two-phase CPA plan (PLANNER_DESIGN.md §4): allocate widths, then critical-path list-schedule at
// the allocated weights `T_i(p_i)`. Mirrors Python's `_build_cpa_plan`.
fn build_cpa_plan(
    cfg: &DagConfig,
    resolved: &HashMap<String, Resolved>,
    est: &HashMap<String, f64>,
    speedups: &HashMap<String, StepSpeedup>,
    succ: &HashMap<String, Vec<String>>,
    core_budget: Option<i64>,
    mem_budget: Option<i64>,
) -> Plan {
    let p = match core_budget {
        Some(b) if b > 0 => b,
        _ => 1,
    };
    let (widths, _admissible, wall, stop_reason) = cpa_allocate(cfg, speedups, est, p, mem_budget);
    let weight: HashMap<String, f64> = cfg
        .steps
        .iter()
        .map(|s| {
            let tag = s.tag();
            let w = widths[&tag];
            (tag.clone(), wall[&tag][&w])
        })
        .collect();
    let bottom = bottom_levels(cfg, &weight, succ);
    let (critical, t_cp) = critical_path(cfg, &bottom, succ);
    let order = plan_order(cfg, Planner::CriticalPath, &weight, &bottom);
    let mut area = 0.0;
    for s in &cfg.steps {
        let tag = s.tag();
        area += widths[&tag] as f64 * weight[&tag];
    }
    let t_a = area / p as f64;
    let lower_bound = if t_cp >= t_a { t_cp } else { t_a };
    let modeled = cpa_simulate_makespan(cfg, &widths, &weight, &order, p);
    let allocation = Allocation {
        core_budget: p,
        area_s: area,
        area_bound_s: t_a,
        critical_path_s: t_cp,
        lower_bound_s: lower_bound,
        modeled_makespan_s: modeled,
        stop_reason,
    };
    let entries: Vec<PlanEntry> = cfg
        .steps
        .iter()
        .map(|s| {
            let tag = s.tag();
            let r = &resolved[&tag];
            let has_curve = speedups
                .get(&tag)
                .map(|sp| !sp.levels.is_empty())
                .unwrap_or(false);
            PlanEntry {
                tag: tag.clone(),
                est_duration_s: weight[&tag],
                est_source: if has_curve {
                    "store".to_string()
                } else {
                    r.est_source.to_string()
                },
                rss_estimate_bytes: r.rss,
                rss_source: r.rss_source.to_string(),
                bottom_level_s: bottom.get(&tag).copied().unwrap_or(0.0),
                samples: r.samples,
                speedup: speedups.get(&tag).cloned(),
                alloc_inner_jobs: Some(widths[&tag]),
            }
        })
        .collect();
    Plan {
        planner: Planner::Cpa,
        order,
        critical_path: critical,
        critical_path_length_s: t_cp,
        entries,
        allocation: Some(allocation),
    }
}

/// Resolve estimates and build a complete execution plan.
///
/// Speedup curves are attached for display. Under [`Planner::Cpa`] they also drive width allocation
/// within the supplied core and memory budgets; the other planners use only dispatch ordering.
pub fn build_plan(
    cfg: &DagConfig,
    store_samples: &HashMap<String, StepSamples>,
    planner: Planner,
    min_samples: i64,
    speedups: &HashMap<String, StepSpeedup>,
    core_budget: Option<i64>,
    mem_budget: Option<i64>,
) -> Plan {
    let mut resolved: HashMap<String, Resolved> = HashMap::new();
    let mut est: HashMap<String, f64> = HashMap::new();
    for step in &cfg.steps {
        let r = resolved_estimate(step, store_samples.get(&step.tag()), min_samples);
        est.insert(step.tag(), r.est);
        resolved.insert(step.tag(), r);
    }
    let succ = successors(cfg);
    if planner == Planner::Cpa {
        return build_cpa_plan(
            cfg,
            &resolved,
            &est,
            speedups,
            &succ,
            core_budget,
            mem_budget,
        );
    }
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
                speedup: speedups.get(&tag).cloned(),
                alloc_inner_jobs: None,
            }
        })
        .collect();
    Plan {
        planner,
        order,
        critical_path: critical,
        critical_path_length_s: length,
        entries,
        allocation: None,
    }
}

/// Return a configuration whose resource hints contain a plan's resolved estimates.
///
/// Stored memory replaces the original hint only when it was selected as the estimate source.
/// Allocating plans also install each chosen inner-job width as the preferred width.
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
                let inner = entry.alloc_inner_jobs.or(step.hint.preferred_inner_jobs);
                s.hint = ResourceHint {
                    resources: step.hint.resources.clone(),
                    est_duration_s: entry.est_duration_s,
                    rss_baseline_bytes: rss,
                    hard_mem_max_bytes: step.hint.hard_mem_max_bytes,
                    classification: step.hint.classification,
                    preferred_inner_jobs: inner,
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

// Fixed 3-decimal seconds, byte-identical to the Python `f"{value:.3f}"`. Shared with the summary
// serializer so every float in the summary JSON uses the SAME fixed-precision formatting.
pub(crate) fn fmt_secs(value: f64) -> String {
    format!("{value:.3}")
}

/// JSON value for an optional fixed-3-decimal number: `null` or a quoted string.
fn opt_secs_json(value: Option<f64>) -> String {
    match value {
        None => "null".to_string(),
        Some(v) => format!("\"{}\"", fmt_secs(v)),
    }
}

// One speedup-curve level as a single-line JSON object (byte-identical to the Python build).
fn speedup_level_json(level: &SpeedupLevel) -> String {
    format!(
        "{{\"inner_jobs\": {}, \"wall_s\": \"{}\", \"speedup\": \"{}\", \"cpu_s\": {}, \"effective_cores\": {}, \"throttled_s\": {}, \"samples\": {}}}",
        level.inner_jobs,
        fmt_secs(level.wall_s),
        fmt_secs(level.speedup),
        opt_secs_json(level.cpu_s),
        opt_secs_json(level.effective_cores),
        opt_secs_json(level.throttled_s),
        level.samples,
    )
}

// The `"speedup"` field value for a step in the plan JSON: `null` or a nested object with the
// recommended width, achieved cores, and the full measured curve. Indented to embed after
// `"speedup": ` at the step object's 6-space field indent. Mirrors Python's `_speedup_to_json`.
fn speedup_to_json(speedup: &Option<StepSpeedup>) -> String {
    let sp = match speedup {
        None => return "null".to_string(),
        Some(s) => s,
    };
    let levels: Vec<String> = sp
        .levels
        .iter()
        .map(|l| format!("          {}", speedup_level_json(l)))
        .collect();
    format!(
        "{{\n        \"baseline_inner_jobs\": {},\n        \"recommended_inner_jobs\": {},\n        \"measured_effective_cores\": {},\n        \"levels\": [\n{}\n        ]\n      }}",
        sp.baseline_inner_jobs,
        sp.recommended_inner_jobs,
        opt_secs_json(sp.measured_effective_cores),
        levels.join(",\n"),
    )
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

// The `"allocation"` field value in the plan JSON: `null` (non-CPA planners) or a nested object
// with the core budget, area / critical-path terms, makespan lower bound + modeled makespan, and
// the stop reason. Mirrors Python's `_allocation_to_json`.
fn allocation_to_json(alloc: &Option<Allocation>) -> String {
    let a = match alloc {
        None => return "null".to_string(),
        Some(a) => a,
    };
    format!(
        "{{\n    \"stop_reason\": {},\n    \"core_budget\": {},\n    \"area_s\": \"{}\",\n    \"area_bound_s\": \"{}\",\n    \"critical_path_s\": \"{}\",\n    \"lower_bound_s\": \"{}\",\n    \"modeled_makespan_s\": \"{}\"\n  }}",
        json_str(&a.stop_reason),
        a.core_budget,
        fmt_secs(a.area_s),
        fmt_secs(a.area_bound_s),
        fmt_secs(a.critical_path_s),
        fmt_secs(a.lower_bound_s),
        fmt_secs(a.modeled_makespan_s),
    )
}

/// Serialize a plan to canonical two-space-indented JSON.
///
/// Computed floating-point values are fixed-three-decimal strings. Allocation fields are `null`
/// for planners that do not allocate parallel widths.
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
        format!(
            "  \"allocation\": {},",
            allocation_to_json(&plan.allocation)
        ),
    ];
    let mut steps_json: Vec<String> = Vec::with_capacity(plan.order.len());
    for tag in &plan.order {
        let entry = by_tag[tag];
        let rss = match entry.rss_estimate_bytes {
            Some(n) => n.to_string(),
            None => "null".to_string(),
        };
        let alloc = match entry.alloc_inner_jobs {
            Some(n) => n.to_string(),
            None => "null".to_string(),
        };
        steps_json.push(format!(
            "    {{\n      \"tag\": {},\n      \"est_duration_s\": \"{}\",\n      \"est_source\": {},\n      \"rss_estimate_bytes\": {},\n      \"rss_source\": {},\n      \"bottom_level_s\": \"{}\",\n      \"samples\": {},\n      \"alloc_inner_jobs\": {},\n      \"speedup\": {}\n    }}",
            json_str(&entry.tag),
            fmt_secs(entry.est_duration_s),
            json_str(&entry.est_source),
            rss,
            json_str(&entry.rss_source),
            fmt_secs(entry.bottom_level_s),
            entry.samples,
            alloc,
            speedup_to_json(&entry.speedup),
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

/// Render a compact, deterministic plan table for a terminal.
pub fn plan_to_text(plan: &Plan) -> String {
    let by_tag = plan.by_tag();
    let is_cpa = plan.allocation.is_some();
    let mut headers: Vec<&str> = vec![
        "step",
        "est_duration_s",
        "source",
        "rss_estimate",
        "rss_source",
        "bottom_level_s",
        "samples",
    ];
    if is_cpa {
        headers.push("alloc_inner_jobs");
    }
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(plan.order.len());
    for tag in &plan.order {
        let entry = by_tag[tag];
        let mut row = vec![
            tag.clone(),
            fmt_secs(entry.est_duration_s),
            entry.est_source.clone(),
            human_bytes(entry.rss_estimate_bytes),
            entry.rss_source.clone(),
            fmt_secs(entry.bottom_level_s),
            entry.samples.to_string(),
        ];
        if is_cpa {
            row.push(match entry.alloc_inner_jobs {
                Some(n) => n.to_string(),
                None => "-".to_string(),
            });
        }
        rows.push(row);
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
    lines.extend(allocation_text_lines(plan));
    lines.extend(speedup_text_lines(plan));
    lines.join("\n") + "\n"
}

// The one-line CPA allocator summary for [`plan_to_text`] (`--planner cpa` only): the stop reason,
// the core budget, and the balancing terms — critical path vs. per-core area, the makespan lower
// bound, and the modeled makespan. Empty for the non-allocating planners. Mirrors Python's
// `_allocation_text_lines`.
fn allocation_text_lines(plan: &Plan) -> Vec<String> {
    let a = match &plan.allocation {
        None => return Vec::new(),
        Some(a) => a,
    };
    vec![format!(
        "allocator (cpa): {}; P={} cores; critical-path={}s, area/P={}s, lower-bound={}s, modeled-makespan={}s",
        a.stop_reason,
        a.core_budget,
        fmt_secs(a.critical_path_s),
        fmt_secs(a.area_bound_s),
        fmt_secs(a.lower_bound_s),
        fmt_secs(a.modeled_makespan_s),
    )]
}

// The optional parallel-speedup section for [`plan_to_text`]: one row per step that HAS a learned
// curve (>=2 inner_jobs widths). Empty when no step has a model, so a store without multi-width
// samples renders exactly as before. Mirrors Python's `_speedup_text_lines`.
fn speedup_text_lines(plan: &Plan) -> Vec<String> {
    let by_tag = plan.by_tag();
    let modeled: Vec<(&String, &StepSpeedup)> = plan
        .order
        .iter()
        .filter_map(|tag| {
            by_tag
                .get(tag)
                .and_then(|e| e.speedup.as_ref().map(|s| (tag, s)))
        })
        .collect();
    if modeled.is_empty() {
        return Vec::new();
    }
    let headers = [
        "step",
        "rec_inner_jobs",
        "eff_cores",
        "speedup@rec",
        "curve(inner_jobs->speedup)",
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(modeled.len());
    for (tag, sp) in &modeled {
        let knee = sp
            .levels
            .iter()
            .find(|l| l.inner_jobs == sp.recommended_inner_jobs)
            .map(|l| l.speedup)
            .unwrap_or(1.0);
        let eff = match sp.measured_effective_cores {
            Some(e) => format!("{e:.3}"),
            None => "-".to_string(),
        };
        let curve = sp
            .levels
            .iter()
            .map(|l| format!("{}:{:.2}x", l.inner_jobs, l.speedup))
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(vec![
            (*tag).clone(),
            sp.recommended_inner_jobs.to_string(),
            eff,
            format!("{knee:.2}x"),
            curve,
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
    let mut out: Vec<String> = vec![
        String::new(),
        "parallel-speedup model (recommended inner_jobs = best wall within the knee + core budget; speedup@rec = speedup at that width):".to_string(),
        fmt_row(&header_cells),
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  "),
    ];
    for row in &rows {
        out.push(fmt_row(row));
    }
    out
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
            cpu_timeout: 0,
            jobs_flag: None,
        }
    }

    #[test]
    fn apply_plan_preserves_all_non_hint_step_fields() {
        // Recurrence guard, mirroring the Python test of the same name (environment-independent, so
        // it fires without cgroups too): the planner rewrites ONLY a step's hint; every other field
        // must survive verbatim. The Python build once dropped `cpu_timeout` here by rebuilding
        // Step field-by-field, silently disabling per-step CPU-time enforcement while this Rust
        // build kept it via clone-and-override. This locks the contract in for BOTH engines.
        let mut step = mk("g", "burn", &[], 3.0);
        step.cmd = "while :; do :; done".into();
        step.env.insert("K".into(), "V".into());
        step.networkonly = true;
        step.engine_only = true;
        step.timeout = 123;
        step.cpu_timeout = 7;
        step.jobs_flag = Some("-J".into());
        let cfg = DagConfig {
            steps: vec![step],
            ..Default::default()
        };
        let empty: HashMap<String, StepSamples> = HashMap::new();
        let no_speedups: HashMap<String, StepSpeedup> = HashMap::new();
        let plan = build_plan(
            &cfg,
            &empty,
            Planner::GreedyLpt,
            DEFAULT_MIN_SAMPLES,
            &no_speedups,
            None,
            None,
        );
        let applied = apply_plan_to_config(&cfg, &plan);
        let out = &applied.steps[0];
        assert_eq!(
            out.cpu_timeout, 7,
            "planner dropped cpu_timeout -> CPU-time enforcement silently off"
        );
        assert_eq!(out.timeout, 123);
        assert!(out.networkonly);
        assert!(out.engine_only);
        assert_eq!(out.jobs_flag.as_deref(), Some("-J"));
        assert_eq!(out.cmd, "while :; do :; done");
        assert_eq!(out.env.get("K").map(String::as_str), Some("V"));
    }

    #[test]
    fn robust_median_discards_outlier() {
        // At n>=3 one huge outlier must not move the median (MAD-trim).
        assert_eq!(robust_median(&[3.0, 3.0, 3.0, 3.0, 100.0]), 3.0);
        assert_eq!(robust_median(&[5.0, 5.0, 100.0]), 5.0);
        // At n<3 MAD-trim cannot reject an outlier; the robust estimate is the MINIMUM (intrinsic
        // value), NOT the mean, so a single slow sample cannot invert a real speedup.
        assert_eq!(robust_median(&[5.0]), 5.0);
        assert_eq!(robust_median(&[5.0, 100.0]), 5.0);
        assert_eq!(robust_median(&[100.0, 5.0]), 5.0);
        assert_eq!(robust_median(&[2.0, 4.0]), 2.0);
    }

    #[test]
    fn parse_float_rejects_non_finite() {
        // inf / -inf / nan / overflowing literals must be dropped so a bogus cell cannot poison a
        // median or contention fraction (they pass a bare `>= 0.0` filter otherwise).
        assert_eq!(parse_float(Some(&"inf".to_string())), None);
        assert_eq!(parse_float(Some(&"-inf".to_string())), None);
        assert_eq!(parse_float(Some(&"nan".to_string())), None);
        assert_eq!(parse_float(Some(&"1e400".to_string())), None);
        assert_eq!(parse_float(Some(&"5.0".to_string())), Some(5.0));
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
    fn parse_helpers_trim_and_reject() {
        // Surrounding ASCII whitespace is trimmed (matches Python's str.strip over the same set).
        assert_eq!(parse_float(Some(&" 50.0 ".to_string())), Some(50.0));
        assert_eq!(parse_int(Some(&"  1000\t".to_string())), Some(1000));
        // PEP-515 underscore separators are rejected in both builds.
        assert_eq!(parse_float(Some(&"1_0.0".to_string())), None);
        assert_eq!(parse_int(Some(&"1_000".to_string())), None);
        // Out-of-i64 magnitudes are rejected (Python's bigint would otherwise keep them).
        assert_eq!(parse_int(Some(&"9999999999999999999999".to_string())), None);
        assert_eq!(parse_int(Some(&(i64::MAX).to_string())), Some(i64::MAX));
        // Empty / whitespace-only / absent cells are None.
        assert_eq!(parse_float(Some(&"   ".to_string())), None);
        assert_eq!(parse_float(None), None);
    }

    #[test]
    fn affinity_width_is_ascii_digit_only() {
        assert_eq!(affinity_width("affinity8_cpu-max"), Some(8));
        assert_eq!(affinity_width("affinity316_x"), Some(316));
        // A Unicode-digit container_class must NOT be misread (returns None, never panics).
        assert_eq!(affinity_width("affinity\u{00b2}_cpu"), None); // superscript two
        assert_eq!(affinity_width("nonaffinity"), None);
        assert_eq!(affinity_width("affinity_cpu"), None);
    }

    #[test]
    fn load_step_samples_trims_and_rejects_hostile_cells() {
        let dir = std::env::temp_dir().join(format!("scdr_est_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,\
                   enforcement_kind,runner_name,step,classification,inner_jobs,elapsed_s,returncode,\
                   ok,timed_out,oom_kills,peak_bytes,thread_peak,pct_other\n\
                   t,m,c,abc,1,abc,unverified,local,g.a,light,1, 8.0 ,0,True,False,0,1000,,0.0\n\
                   t,m,c,abc,1,abc,unverified,local,g.a,light,1,4.0,0,True,False,0,2000,,0.0\n\
                   t,m,c,abc,1,abc,unverified,local,g.a,light,1,10.0,0,True,False,0,3000,, 50.0 \n\
                   t,m,c,abc,1,abc,unverified,local,g.b,light,1,1_0.0,0,True,False,0,1_000,,0.0\n\
                   t,m,c,abc,1,abc,unverified,local,g.b,light,1,4.0,0,True,False,0,9999999999999999999999,,0.0\n\
                   t,m,c,abc,1,abc,unverified,local,g.b,light,1,6.0,0,True,False,0,5000,,0.0\n";
        let path = dir.join("step_profiles_m_c.csv");
        std::fs::write(&path, csv).unwrap();
        let samples = load_step_samples(&dir, "m", "c");
        // g.a: whitespace elapsed trimmed, padded ' 50.0 '% contention trimmed+applied -> [8,4,5].
        let a = &samples["g.a"];
        assert_eq!(a.samples, 3);
        assert_eq!(a.est_duration_s, Some(5.0));
        assert_eq!(a.rss_estimate_bytes, Some(3000));
        // g.b: '1_0.0' rejected -> [4,6]. Two samples cannot MAD-reject an outlier, so the robust
        // estimate is the minimum (4.0); '1_000' + out-of-i64 peak rejected -> [5000].
        let b = &samples["g.b"];
        assert_eq!(b.samples, 3);
        assert_eq!(b.est_duration_s, Some(4.0));
        assert_eq!(b.rss_estimate_bytes, Some(5000));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn contention_discounts_duration() {
        // A step run under 50% other-work contention: intrinsic ~= elapsed * 0.5.
        let mut row: HashMap<String, String> = HashMap::new();
        row.insert("pct_other".into(), "50.0".into());
        assert!((contention_fraction(&row, Some(8)) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn speedup_model_detects_knee_and_linear() {
        let dir = std::env::temp_dir().join(format!("scdr_sp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // b.app: wall halves 1->2 but flattens 2->4 while total CPU-s blows up -> knee at 2.
        // l.step: near-linear halving with flat CPU-s -> widest measured width (4), under budget 16.
        let csv = "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,\
                   enforcement_kind,runner_name,step,classification,inner_jobs,elapsed_s,returncode,\
                   ok,timed_out,oom_kills,peak_bytes,thread_peak,effective_cores,user_s,sys_s,throttled_s\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,b.app,cpu-bound,1,10.0,0,True,False,0,1000,,1.0,10.0,0.2,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,b.app,cpu-bound,1,10.1,0,True,False,0,1000,,1.0,10.1,0.2,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,b.app,cpu-bound,2,5.0,0,True,False,0,1000,,1.98,10.0,0.4,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,b.app,cpu-bound,2,5.05,0,True,False,0,1000,,1.98,10.1,0.4,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,b.app,cpu-bound,4,4.5,0,True,False,0,1000,,3.2,17.5,0.8,1.2\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,b.app,cpu-bound,4,4.6,0,True,False,0,1000,,3.2,17.6,0.9,1.3\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,l.step,cpu-bound,1,8.0,0,True,False,0,1000,,,8.0,0.0,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,l.step,cpu-bound,2,4.0,0,True,False,0,1000,,,8.0,0.0,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,l.step,cpu-bound,4,2.0,0,True,False,0,1000,,,8.1,0.0,0.0\n\
                   t,m,affinity16_cpu-max-max,a,1,a,u,l,s.one,cpu-bound,1,5.0,0,True,False,0,1000,,,5.0,0.0,0.0\n";
        let path = dir.join("step_profiles_m_affinity16_cpu-max-max.csv");
        std::fs::write(&path, csv).unwrap();
        let models = load_step_speedups(&dir, "m", "affinity16_cpu-max-max");
        let knee = &models["b.app"];
        assert_eq!(knee.baseline_inner_jobs, 1);
        assert_eq!(knee.recommended_inner_jobs, 2);
        assert_eq!(knee.measured_effective_cores, Some(1.98));
        assert_eq!(
            knee.levels.iter().map(|l| l.inner_jobs).collect::<Vec<_>>(),
            vec![1, 2, 4]
        );
        assert_eq!(models["l.step"].recommended_inner_jobs, 4);
        // A single measured width is not enough to model a curve.
        assert!(!models.contains_key("s.one"));
        let _ = std::fs::remove_file(&path);
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
        let no_speedups: HashMap<String, StepSpeedup> = HashMap::new();
        let lpt = build_plan(
            &cfg,
            &empty,
            Planner::GreedyLpt,
            DEFAULT_MIN_SAMPLES,
            &no_speedups,
            None,
            None,
        );
        let cp = build_plan(
            &cfg,
            &empty,
            Planner::CriticalPath,
            DEFAULT_MIN_SAMPLES,
            &no_speedups,
            None,
            None,
        );
        assert_eq!(lpt.order, vec!["g.heavy", "g.solo", "g.prep"]);
        assert_eq!(cp.order, vec!["g.prep", "g.heavy", "g.solo"]);
        assert_eq!(cp.critical_path, vec!["g.prep", "g.heavy"]);
        assert_ne!(lpt.order, cp.order);
    }

    // ----------------------------------------------------------------- CPA allocator

    /// Write a synthetic multi-inner_jobs speedup store (affinity16 identity) into a unique temp
    /// dir and return that dir.
    fn write_cpa_store(name: &str, rows: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("scdr_cpa_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,\
                      enforcement_kind,runner_name,step,classification,inner_jobs,elapsed_s,\
                      returncode,ok,timed_out,oom_kills,peak_bytes,thread_peak,effective_cores,\
                      user_s,sys_s,throttled_s\n";
        std::fs::write(
            dir.join("step_profiles_m_affinity16_cpu-max-max.csv"),
            format!("{header}{rows}"),
        )
        .unwrap();
        dir
    }

    fn cpa_widths(plan: &Plan) -> HashMap<String, Option<i64>> {
        plan.entries
            .iter()
            .map(|e| (e.tag.clone(), e.alloc_inner_jobs))
            .collect()
    }

    #[test]
    fn cpa_spreads_cores_on_independent_tasks() {
        // Two INDEPENDENT linear-scaling steps, P=4: the allocator balances T_CP vs area/P and
        // SPREADS cores -> both land at width 2 (not one hogging 4), stopping "balanced".
        let rows = "\
t,m,affinity16_cpu-max-max,a,1,a,u,l,g.a,cpu-bound,1,10.0,0,True,False,0,1000,,,10.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,g.a,cpu-bound,2,5.0,0,True,False,0,1000,,,10.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,g.a,cpu-bound,4,2.5,0,True,False,0,1000,,,10.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,g.b,cpu-bound,1,10.0,0,True,False,0,1000,,,10.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,g.b,cpu-bound,2,5.0,0,True,False,0,1000,,,10.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,g.b,cpu-bound,4,2.5,0,True,False,0,1000,,,10.0,0.0,0.0
";
        let dir = write_cpa_store("spread", rows);
        let speedups = load_step_speedups(&dir, "m", "affinity16_cpu-max-max");
        let cfg = DagConfig {
            steps: vec![mk("g", "a", &[], 10.0), mk("g", "b", &[], 10.0)],
            mem_cap_factor: 1.0,
            mem_cap_floor_bytes: 0,
            ..Default::default()
        };
        let empty: HashMap<String, StepSamples> = HashMap::new();
        let plan = build_plan(
            &cfg,
            &empty,
            Planner::Cpa,
            DEFAULT_MIN_SAMPLES,
            &speedups,
            Some(4),
            None,
        );
        let w = cpa_widths(&plan);
        assert_eq!(w["g.a"], Some(2));
        assert_eq!(w["g.b"], Some(2));
        let alloc = plan.allocation.as_ref().unwrap();
        assert_eq!(alloc.stop_reason, "balanced");
        assert!(alloc.modeled_makespan_s >= alloc.lower_bound_s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cpa_piles_cores_on_the_chain_and_leaves_plateau_narrow() {
        // Chain prep(curveless) -> build(scaling) -> test(scaling) plus independent plateau (side).
        let rows = "\
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,1,40.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,2,20.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,4,10.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,8,5.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.test,cpu-bound,1,16.0,0,True,False,0,1000,,,16.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.test,cpu-bound,2,8.0,0,True,False,0,1000,,,16.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.test,cpu-bound,4,4.0,0,True,False,0,1000,,,16.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.side,cpu-bound,1,9.0,0,True,False,0,1000,,,9.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.side,cpu-bound,2,8.7,0,True,False,0,1000,,,9.0,0.0,0.0
";
        let dir = write_cpa_store("chain", rows);
        let speedups = load_step_speedups(&dir, "m", "affinity16_cpu-max-max");
        let cfg = DagConfig {
            steps: vec![
                mk("c", "prep", &[], 2.0),
                mk("c", "build", &["c.prep"], 40.0),
                mk("c", "test", &["c.build"], 16.0),
                mk("c", "side", &[], 9.0),
            ],
            mem_cap_factor: 1.0,
            mem_cap_floor_bytes: 0,
            ..Default::default()
        };
        let empty: HashMap<String, StepSamples> = HashMap::new();
        let plan = build_plan(
            &cfg,
            &empty,
            Planner::Cpa,
            DEFAULT_MIN_SAMPLES,
            &speedups,
            Some(16),
            None,
        );
        let w = cpa_widths(&plan);
        assert_eq!(w["c.build"], Some(8)); // piled onto the chain
        assert_eq!(w["c.test"], Some(4));
        assert_eq!(w["c.side"], Some(1)); // plateau: never widened
        assert_eq!(w["c.prep"], Some(1)); // curveless: rigid
        let alloc = plan.allocation.as_ref().unwrap();
        assert_eq!(alloc.stop_reason, "knee-exhausted");
        assert!(alloc.modeled_makespan_s < 58.0); // beats the width-1 critical path
        assert!(alloc.modeled_makespan_s >= alloc.lower_bound_s);
    }

    #[test]
    fn cpa_memory_blocks_widening_even_with_free_cores() {
        let rows = "\
t,m,affinity16_cpu-max-max,a,1,a,u,l,m.heavy,cpu-bound,1,40.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,m.heavy,cpu-bound,2,20.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,m.heavy,cpu-bound,4,10.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,m.heavy,cpu-bound,8,5.0,0,True,False,0,1000,,,40.0,0.0,0.0
";
        let dir = write_cpa_store("mem", rows);
        let speedups = load_step_speedups(&dir, "m", "affinity16_cpu-max-max");
        let heavy = Step {
            group: "m".into(),
            job: "heavy".into(),
            desc: String::new(),
            description: String::new(),
            cmd: "true".into(),
            deps: vec!["m.prep".into()],
            env: BTreeMap::new(),
            hint: ResourceHint {
                est_duration_s: 40.0,
                rss_baseline_bytes: Some(3 * 1024i64.pow(3)),
                classification: crate::model::StepClass::CpuBound,
                ..Default::default()
            },
            networkonly: false,
            engine_only: false,
            timeout: 1800,
            cpu_timeout: 0,
            jobs_flag: None,
        };
        let cfg = DagConfig {
            steps: vec![mk("m", "prep", &[], 2.0), heavy],
            mem_cap_factor: 1.0,
            mem_cap_floor_bytes: 0,
            ..Default::default()
        };
        let empty: HashMap<String, StepSamples> = HashMap::new();
        // 5 GiB budget blocks widening m.heavy 4 -> 8 (footprint 3 GiB -> 6 GiB), cores free.
        let capped = build_plan(
            &cfg,
            &empty,
            Planner::Cpa,
            DEFAULT_MIN_SAMPLES,
            &speedups,
            Some(16),
            Some(5 * 1024i64.pow(3)),
        );
        assert_eq!(cpa_widths(&capped)["m.heavy"], Some(4));
        assert_eq!(
            capped.allocation.as_ref().unwrap().stop_reason,
            "mem-capped"
        );
        // With no RAM budget it widens all the way to the knee (8).
        let free = build_plan(
            &cfg,
            &empty,
            Planner::Cpa,
            DEFAULT_MIN_SAMPLES,
            &speedups,
            Some(16),
            None,
        );
        assert_eq!(cpa_widths(&free)["m.heavy"], Some(8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cpa_allocation_is_idempotent() {
        let rows = "\
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,1,40.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,2,20.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,4,10.0,0,True,False,0,1000,,,40.0,0.0,0.0
t,m,affinity16_cpu-max-max,a,1,a,u,l,c.build,cpu-bound,8,5.0,0,True,False,0,1000,,,40.0,0.0,0.0
";
        let dir = write_cpa_store("idem", rows);
        let speedups = load_step_speedups(&dir, "m", "affinity16_cpu-max-max");
        let cfg = DagConfig {
            steps: vec![
                mk("c", "prep", &[], 2.0),
                mk("c", "build", &["c.prep"], 40.0),
            ],
            mem_cap_factor: 1.0,
            mem_cap_floor_bytes: 0,
            ..Default::default()
        };
        let est: HashMap<String, f64> = cfg
            .steps
            .iter()
            .map(|s| (s.tag(), s.hint.est_duration_s))
            .collect();
        let w1 = allocate_widths(&cfg, &speedups, &est, 16, None);
        let w2 = allocate_widths(&cfg, &speedups, &est, 16, None);
        assert_eq!(w1, w2);
        assert_eq!(w1["c.build"], 8);
        assert_eq!(w1["c.prep"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
