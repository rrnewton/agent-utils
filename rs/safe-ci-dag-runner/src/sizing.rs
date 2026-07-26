//! Memory footprint model + memory-aware `-j` selection (pure functions over a [`DagConfig`]).
//!
//! Direct port of `py/safe_ci_dag_runner/sizing.py`. Rather than a flat per-job RAM estimate,
//! it enumerates which steps can actually co-run (no transitive dependency between them, and
//! their summed scarce-resource demand fits the caps) and takes the worst-case sum of their
//! per-step memory caps. That yields an exact "largest `-jN` that fits budget M". The chosen
//! `-j` and footprint MUST equal the Python build's for the same DAG (cross-tested).

use std::collections::{HashMap, HashSet};

use crate::model::{step_classification, DagConfig, Step, StepClass};

/// Inner-cgroup MemoryMax for a step, or `None` if uncharacterized. An explicit hard cap
/// wins; otherwise `factor x` the RSS baseline. A zero (or absent) baseline yields `None`,
/// matching Python's falsy `if base` guard.
pub fn step_mem_cap_bytes(step: &Step, mem_cap_factor: f64) -> Option<i64> {
    if let Some(hard) = step.hint.hard_mem_max_bytes {
        return Some(hard);
    }
    match step.hint.rss_baseline_bytes {
        Some(base) if base != 0 => Some((base as f64 * mem_cap_factor) as i64),
        _ => None,
    }
}

/// Per-step cap scaled for internal parallelism.
///
/// Conservative `P x J` model: an explicit hard cap and non-CPU-bound steps keep the base
/// cap; CPU-bound steps scale linearly above J=4.
pub fn step_mem_cap_for_inner_jobs(
    step: &Step,
    inner_jobs: Option<i64>,
    mem_cap_factor: f64,
) -> i64 {
    let cap = step_mem_cap_bytes(step, mem_cap_factor).unwrap_or(0);
    match inner_jobs {
        Some(jobs)
            if step.hint.hard_mem_max_bytes.is_none()
                && step_classification(step) == StepClass::CpuBound =>
        {
            // Python: max(cap, int(cap * inner_jobs / 4)) with true (float) division.
            let scaled = (cap as f64 * jobs as f64 / 4.0) as i64;
            cap.max(scaled)
        }
        _ => cap,
    }
}

/// Map each step tag to the set of all tags it transitively depends on.
pub fn transitive_deps(steps: &[Step]) -> HashMap<String, HashSet<String>> {
    let direct: HashMap<String, Vec<String>> =
        steps.iter().map(|s| (s.tag(), s.deps.clone())).collect();
    let mut result: HashMap<String, HashSet<String>> = HashMap::new();

    fn visit(
        tag: &str,
        direct: &HashMap<String, Vec<String>>,
        result: &mut HashMap<String, HashSet<String>>,
    ) -> HashSet<String> {
        if let Some(cached) = result.get(tag) {
            return cached.clone();
        }
        let mut deps: HashSet<String> = direct
            .get(tag)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let direct_deps: Vec<String> = deps.iter().cloned().collect();
        for dep in direct_deps {
            for t in visit(&dep, direct, result) {
                deps.insert(t);
            }
        }
        result.insert(tag.to_string(), deps.clone());
        deps
    }

    for tag in direct.keys() {
        visit(tag, &direct, &mut result);
    }
    result
}

/// Generate all `k`-combinations of the indices `0..n` (in lexicographic order), matching
/// Python's `itertools.combinations`.
fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    if k == 0 || k > n {
        if k == 0 {
            out.push(Vec::new());
        }
        return out;
    }
    let mut idx: Vec<usize> = (0..k).collect();
    loop {
        out.push(idx.clone());
        // Find the rightmost index that can be incremented.
        let mut i = k;
        loop {
            if i == 0 {
                return out;
            }
            i -= 1;
            if idx[i] != i + n - k {
                break;
            }
        }
        idx[i] += 1;
        for j in i + 1..k {
            idx[j] = idx[j - 1] + 1;
        }
    }
}

/// Maximum per-step-cap sum over any scheduler-reachable concurrent set of size `<= jobs`.
///
/// A set is reachable only when no member transitively depends on another and the summed
/// scarce-resource demand fits `cfg.resource_caps`. Only steps with a memory baseline and that
/// are not engine-only participate. Returns `(best_total, chosen_tags)`.
pub fn schedulable_peak_mem_bytes(
    cfg: &DagConfig,
    jobs: i64,
    inner_jobs: Option<i64>,
) -> (i64, Vec<String>) {
    // Participating steps keep cfg order (mirrors Python's insertion-ordered `by_tag`); `tags`
    // and `participating` are parallel, so a combination index selects both at once.
    let participating: Vec<&Step> = cfg
        .steps
        .iter()
        .filter(|s| s.hint.rss_baseline_bytes.is_some() && !s.engine_only)
        .collect();
    let tags: Vec<String> = participating.iter().map(|s| s.tag()).collect();
    let dependencies = transitive_deps(&cfg.steps);

    let width = jobs.max(1).min(tags.len() as i64) as usize;
    let mut best_total: i64 = 0;
    let mut best: Vec<String> = Vec::new();

    for count in 1..=width {
        for combo in combinations(tags.len(), count) {
            // No two members may have a (transitive) dependency relation, either direction.
            let mut dependent = false;
            'pairs: for a in 0..combo.len() {
                for b in (a + 1)..combo.len() {
                    let left = &tags[combo[a]];
                    let right = &tags[combo[b]];
                    let l_in_r = dependencies.get(right).is_some_and(|d| d.contains(left));
                    let r_in_l = dependencies.get(left).is_some_and(|d| d.contains(right));
                    if l_in_r || r_in_l {
                        dependent = true;
                        break 'pairs;
                    }
                }
            }
            if dependent {
                continue;
            }
            // Summed scarce-resource demand must fit every cap.
            let over_cap = cfg.resource_caps.iter().any(|(resource, cap)| {
                let sum: i64 = combo
                    .iter()
                    .map(|&i| {
                        participating[i]
                            .hint
                            .resources
                            .get(resource)
                            .copied()
                            .unwrap_or(0)
                    })
                    .sum();
                sum > *cap
            });
            if over_cap {
                continue;
            }
            let total: i64 = combo
                .iter()
                .map(|&i| {
                    step_mem_cap_for_inner_jobs(participating[i], inner_jobs, cfg.mem_cap_factor)
                })
                .sum();
            if total > best_total {
                best_total = total;
                best = combo.iter().map(|&i| tags[i].clone()).collect();
            }
        }
    }
    (best_total, best)
}

/// Worst-case footprint (bytes) at the given `-j`, clamped to the configured floor.
pub fn jobs_footprint_bytes(cfg: &DagConfig, jobs: i64, inner_jobs: Option<i64>) -> i64 {
    let (peak, _) = schedulable_peak_mem_bytes(cfg, jobs, inner_jobs);
    cfg.mem_cap_floor_bytes
        .max((peak as f64 * cfg.outer_mem_safety_factor) as i64)
}

/// Largest `-jN` (`>=1`, capped at CPU count) whose worst-case footprint fits `budget` bytes.
/// Returns `(jobs, footprint_at_that_jobs)`. Always `>= 1`.
pub fn jobs_for_budget(cfg: &DagConfig, budget: i64) -> (i64, i64) {
    let ncpu = cpu_count();
    let mut best: i64 = 1;
    for n in 1..=ncpu {
        if jobs_footprint_bytes(cfg, n, None) <= budget {
            best = n;
        } else {
            break; // footprint is monotonic non-decreasing in n
        }
    }
    (best, jobs_footprint_bytes(cfg, best, None))
}

/// Number of logical CPUs, matching Python's `os.cpu_count()` (online, affinity-independent).
///
/// Read from `/sys/devices/system/cpu/online` (the same set `sysconf(_SC_NPROCESSORS_ONLN)`
/// reports), so the chosen `-j` agrees with the Python build. Falls back to
/// `available_parallelism`, then 4.
pub fn cpu_count() -> i64 {
    if let Ok(text) = std::fs::read_to_string("/sys/devices/system/cpu/online") {
        if let Some(n) = count_cpu_ranges(text.trim()) {
            return n;
        }
    }
    if let Ok(n) = std::thread::available_parallelism() {
        return n.get() as i64;
    }
    4
}

/// Count CPUs from a Linux cpu-range list like `"0-3,5,7-8"`.
///
/// Shared with `perflog::affinity_width` (which parses `/proc/self/status` `Cpus_allowed_list`),
/// so both the `-j` sizing default and the perf-log `nproc()` use one CPU-range parser.
pub(crate) fn count_cpu_ranges(spec: &str) -> Option<i64> {
    if spec.is_empty() {
        return None;
    }
    let mut total: i64 = 0;
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((lo, hi)) => {
                let lo: i64 = lo.trim().parse().ok()?;
                let hi: i64 = hi.trim().parse().ok()?;
                if hi < lo {
                    return None;
                }
                total += hi - lo + 1;
            }
            None => {
                part.trim().parse::<i64>().ok()?;
                total += 1;
            }
        }
    }
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

/// Parse `'8G'` / `'4096M'` / `'2048K'` / `'12345'` (bytes) into an int, or `None` if bad.
///
/// Mirrors the Python regex `\s*(\d+(?:\.\d+)?)\s*([KkMmGgTt]?)([Bb]?)\s*` fullmatch: optional
/// surrounding and mid whitespace, a decimal number, an optional size unit, and an optional
/// trailing `B`/`b`.
pub fn parse_size(spec: &str) -> Option<i64> {
    if spec.is_empty() {
        return None;
    }
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0usize;
    let n = chars.len();
    let skip_ws = |i: &mut usize| {
        while *i < n && chars[*i].is_whitespace() {
            *i += 1;
        }
    };

    skip_ws(&mut i);
    // Integer part: one or more ASCII digits.
    let start = i;
    while i < n && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return None; // needs at least one digit
    }
    // Optional fractional part: '.' followed by one or more digits.
    if i < n && chars[i] == '.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < n && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j == frac_start {
            return None; // '.' with no digits after is not a match
        }
        i = j;
    }
    let number: String = chars[start..i].iter().collect();
    let value: f64 = number.parse().ok()?;

    skip_ws(&mut i);
    // Optional single size unit.
    let mult: f64 = if i < n {
        match chars[i] {
            'K' | 'k' => {
                i += 1;
                1024.0
            }
            'M' | 'm' => {
                i += 1;
                1024f64.powi(2)
            }
            'G' | 'g' => {
                i += 1;
                1024f64.powi(3)
            }
            'T' | 't' => {
                i += 1;
                1024f64.powi(4)
            }
            _ => 1.0,
        }
    } else {
        1.0
    };
    // Optional trailing B/b (captured but ignored by Python).
    if i < n && (chars[i] == 'B' || chars[i] == 'b') {
        i += 1;
    }
    skip_ws(&mut i);
    if i != n {
        return None; // trailing junk -> no fullmatch
    }
    Some((value * mult) as i64)
}

/// Bytes currently allocatable without swapping (`MemAvailable`), or `None` if unreadable.
pub fn mem_available_bytes() -> Option<i64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: i64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ResourceHint;
    use std::collections::BTreeMap;

    const GIB: i64 = 1024 * 1024 * 1024;

    fn step(group: &str, job: &str, rss: Option<i64>, deps: &[&str], gpu: bool) -> Step {
        let mut resources = BTreeMap::new();
        if gpu {
            resources.insert("gpu".to_string(), 1);
        }
        Step {
            group: group.into(),
            job: job.into(),
            desc: String::new(),
            description: String::new(),
            cmd: "true".into(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            hint: ResourceHint {
                resources,
                rss_baseline_bytes: rss,
                ..Default::default()
            },
            networkonly: false,
            engine_only: false,
            timeout: 1800,
            jobs_flag: None,
        }
    }

    fn cfg() -> DagConfig {
        let mut caps = BTreeMap::new();
        caps.insert("gpu".to_string(), 1);
        DagConfig {
            steps: vec![
                step("g", "A", Some(3 * GIB), &[], false),
                step("g", "B", Some(2 * GIB), &["g.A"], false),
                step("g", "C", Some(4 * GIB), &[], true),
                step("g", "D", Some(GIB), &[], true),
            ],
            description: String::new(),
            resource_caps: caps,
            mem_cap_factor: 1.0,
            outer_mem_safety_factor: 1.0,
            mem_cap_floor_bytes: 0,
            default_step_timeout: 1800,
            default_jobs_flag: "-j".to_string(),
        }
    }

    #[test]
    fn parse_size_cases() {
        assert_eq!(parse_size("8G"), Some(8 * GIB));
        assert_eq!(parse_size("4096M"), Some(4096 * 1024 * 1024));
        assert_eq!(parse_size("2048K"), Some(2048 * 1024));
        assert_eq!(parse_size("12345"), Some(12345));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("nonsense"), None);
        // whitespace + trailing B forms Python's regex accepts.
        assert_eq!(parse_size(" 8 GB "), Some(8 * GIB));
    }

    #[test]
    fn transitive_deps_cases() {
        let deps = transitive_deps(&cfg().steps);
        assert_eq!(deps["g.B"], HashSet::from(["g.A".to_string()]));
        assert!(deps["g.A"].is_empty());
    }

    #[test]
    fn step_mem_cap_hard_override_wins() {
        let mut hint = ResourceHint {
            rss_baseline_bytes: Some(2 * GIB),
            hard_mem_max_bytes: Some(9 * GIB),
            ..Default::default()
        };
        let _ = &mut hint;
        let s = Step {
            group: "g".into(),
            job: "X".into(),
            desc: String::new(),
            description: String::new(),
            cmd: "true".into(),
            deps: vec![],
            env: BTreeMap::new(),
            hint,
            networkonly: false,
            engine_only: false,
            timeout: 1800,
            jobs_flag: None,
        };
        assert_eq!(step_mem_cap_bytes(&s, 1.25), Some(9 * GIB));
    }

    #[test]
    fn schedulable_peak_picks_best_feasible_set() {
        let (total, chosen) = schedulable_peak_mem_bytes(&cfg(), 4, None);
        assert_eq!(total, 7 * GIB);
        let set: HashSet<String> = chosen.into_iter().collect();
        assert_eq!(set, HashSet::from(["g.A".to_string(), "g.C".to_string()]));
    }

    #[test]
    fn jobs_for_budget_monotonic_and_at_least_one() {
        let c = cfg();
        assert_eq!(jobs_footprint_bytes(&c, 1, None), 4 * GIB);
        assert_eq!(jobs_for_budget(&c, 6 * GIB), (1, 4 * GIB));
        assert_eq!(jobs_for_budget(&c, GIB).0, 1);
    }
}
