//! Core DAG vocabulary for safe-ci-dag-runner.
//!
//! Pure data + pure helpers, no I/O. A caller describes their build/test graph as a set of
//! [`Step`] values (each carrying a [`ResourceHint`]) bundled in a [`DagConfig`], then hands
//! it to the runner. Direct port of `py/safe_ci_dag_runner/model.py`; the enum serde values,
//! defaults, and the [`step_failure_reason`] precedence + strings are kept identical to the
//! Python build so the two are cross-differential-testable.

use std::collections::BTreeMap;

/// Default per-step timeout (seconds); mirrors Python's `DEFAULT_STEP_TIMEOUT`.
pub const DEFAULT_STEP_TIMEOUT: i64 = 1800;

/// Default template for the inner-parallelism (concurrency) flag appended to a step's command
/// when the step declares `preferred_inner_jobs`. Mirrors Python's `DEFAULT_JOBS_FLAG`.
pub const DEFAULT_JOBS_FLAG: &str = "-j";

/// How a step uses the machine, used for scheduling decisions.
///
/// The serde/string values (`"cpu-bound"`, `"latency-bound"`, `"light"`) are load-bearing:
/// they appear verbatim in JSON, `list`, `ascii`, and `dot` output and must match Python.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepClass {
    CpuBound,
    LatencyBound,
    #[default]
    Light,
}

impl StepClass {
    /// The canonical string form (matches Python's `StepClass.<X>.value`).
    pub fn value(self) -> &'static str {
        match self {
            StepClass::CpuBound => "cpu-bound",
            StepClass::LatencyBound => "latency-bound",
            StepClass::Light => "light",
        }
    }

    /// Parse the canonical string form, or `None` for an unknown value.
    pub fn from_value(text: &str) -> Option<StepClass> {
        match text {
            "cpu-bound" => Some(StepClass::CpuBound),
            "latency-bound" => Some(StepClass::LatencyBound),
            "light" => Some(StepClass::Light),
            _ => None,
        }
    }
}

/// Per-step scheduling knowledge: scarce-resource demand, cost estimate, memory.
///
/// Every field is optional (has a default). With none supplied the runner falls back to a
/// fixed concurrency with no memory model; supplying them enables memory-aware `-j` sizing and
/// longest-processing-time dispatch ordering.
#[derive(Debug, Clone, Default)]
pub struct ResourceHint {
    /// Scarce-resource DEMAND for this step, e.g. `{"browser": 1}`. The runner never lets the
    /// summed demand of concurrently-running steps exceed `DagConfig::resource_caps`.
    pub resources: BTreeMap<String, i64>,
    /// Estimated wall-clock seconds; used only to order ready steps (longest first).
    pub est_duration_s: f64,
    /// Estimated peak resident memory (bytes). `None` excludes the step from the memory model.
    pub rss_baseline_bytes: Option<i64>,
    /// Explicit hard per-step memory cap (bytes); overrides the derived cap when set.
    pub hard_mem_max_bytes: Option<i64>,
    pub classification: StepClass,
    /// Internal parallelism width for the step's own command (e.g. a build's `-j`).
    pub preferred_inner_jobs: Option<i64>,
    pub measured_effective_cores: Option<f64>,
    pub measured_cpu_utilization: Option<f64>,
}

/// One node in the DAG: a shell command plus its dependencies and resource hint.
#[derive(Debug, Clone)]
pub struct Step {
    pub group: String,
    pub job: String,
    pub desc: String,
    /// Optional long-form documentation for this node (default empty). Unlike `desc` — a short
    /// label shown by `list`/`run` — `description` is free-form prose (often multi-line, e.g. a
    /// YAML block scalar) documenting WHY the step exists. It never affects scheduling.
    pub description: String,
    /// Shell command (`bash -c`), run from the run's working directory.
    pub cmd: String,
    /// Tags (`"group.job"`) this step depends on.
    pub deps: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub hint: ResourceHint,
    /// Skipped when networking is disabled.
    pub networkonly: bool,
    /// Selected only by an engine-only subset preset.
    pub engine_only: bool,
    pub timeout: i64,
    /// CPU-time budget in seconds (user+system, from the step's cgroup `cpu.stat`). `0` disables
    /// the CPU-time guard, leaving only the wall `timeout`. CPU time is immune to machine load, so
    /// this can be set far tighter than a load-tolerant wall timeout without flaking. Mirrors
    /// Python's `Step.cpu_timeout`; both runners enforce it identically under cgroup boxing.
    pub cpu_timeout: i64,
    /// Template for the inner-parallelism flag appended to `cmd` when this step declares
    /// `preferred_inner_jobs`. `None` inherits `DagConfig::default_jobs_flag`; an empty string
    /// disables appending (the step manages its own concurrency). See [`render_jobs_flag`].
    pub jobs_flag: Option<String>,
}

impl Step {
    /// The step's unique tag, `"group.job"`.
    pub fn tag(&self) -> String {
        format!("{}.{}", self.group, self.job)
    }
}

/// Render an inner-parallelism (concurrency) flag from a template and a job count.
///
/// Byte-for-byte identical to Python's `render_jobs_flag`. Three forms:
/// * template contains `%d` -> substitute (no auto-space): `"-j%d"` -> `"-j4"`.
/// * template ends with `=` -> concatenate (no space): `"--jobs="` -> `"--jobs=4"`.
/// * otherwise -> space-separated: `"--num-threads"` -> `"--num-threads 4"`, default `"-j"` ->
///   `"-j 4"`.
pub fn render_jobs_flag(template: &str, inner_jobs: i64) -> String {
    if template.contains("%d") {
        return template.replace("%d", &inner_jobs.to_string());
    }
    if template.ends_with('=') {
        return format!("{template}{inner_jobs}");
    }
    format!("{template} {inner_jobs}")
}

/// The jobs-flag template in effect for a step: its own `jobs_flag` overrides the
/// DagConfig-level default; `None` inherits the default.
pub fn effective_jobs_flag<'a>(step: &'a Step, default_jobs_flag: &'a str) -> &'a str {
    step.jobs_flag.as_deref().unwrap_or(default_jobs_flag)
}

/// The step's shell command with its inner-parallelism flag appended, when applicable.
///
/// Appends `<rendered-flag>` (see [`render_jobs_flag`]) when `inner_jobs` is set and the
/// effective template is non-empty; a `None` `inner_jobs` or an empty template leaves the
/// command unchanged. Mirrors Python's `command_with_inner_jobs`.
pub fn command_with_inner_jobs(
    step: &Step,
    default_jobs_flag: &str,
    inner_jobs: Option<i64>,
) -> String {
    match inner_jobs {
        None => step.cmd.clone(),
        Some(n) => {
            let template = effective_jobs_flag(step, default_jobs_flag);
            if template.is_empty() {
                step.cmd.clone()
            } else {
                format!("{} {}", step.cmd, render_jobs_flag(template, n))
            }
        }
    }
}

/// A step's class: an explicit non-default hint wins; a browser-resource step is
/// latency-bound; otherwise light. (Mirrors DeepScry's `step_class`.)
pub fn step_classification(step: &Step) -> StepClass {
    if step.hint.classification != StepClass::Light {
        return step.hint.classification;
    }
    if step.hint.resources.contains_key("browser") {
        return StepClass::LatencyBound;
    }
    StepClass::Light
}

/// Internal parallelism width for a step: an explicit override wins, else the hint.
pub fn preferred_inner_jobs(step: &Step, experiment_override: Option<i64>) -> Option<i64> {
    experiment_override.or(step.hint.preferred_inner_jobs)
}

/// Map a Unix signal number to its name (e.g. `9 -> "SIGKILL"`), matching the names Python's
/// `signal.Signals(n).name` produces for the common signals; unknown numbers fall back to
/// `"signal N"` exactly like the Python `ValueError` branch.
fn signal_name(sig: i64) -> String {
    let name = match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        17 => "SIGCHLD",
        18 => "SIGCONT",
        19 => "SIGSTOP",
        20 => "SIGTSTP",
        21 => "SIGTTIN",
        22 => "SIGTTOU",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        26 => "SIGVTALRM",
        27 => "SIGPROF",
        28 => "SIGWINCH",
        29 => "SIGIO",
        31 => "SIGSYS",
        _ => return format!("signal {sig}"),
    };
    name.to_string()
}

/// Describe a failed step without conflating an external signal with an OOM.
///
/// Precedence (load-bearing for cross-language parity):
/// OOM > CPU-timeout > timeout > pids-guard > detail-capture-failure > signal > exit code.
///
/// A negative `returncode` means the child received a Unix signal; that must never be
/// reported as an OOM.
#[allow(clippy::too_many_arguments)]
pub fn step_failure_reason(
    returncode: Option<i64>,
    oomed: bool,
    oom_kills: i64,
    timed_out: bool,
    timeout: i64,
    pids_guard_tripped: bool,
    pids_guard_reason: Option<&str>,
    detail_write_failure: &[String],
    cpu_timed_out: bool,
    cpu_timeout: i64,
) -> String {
    if oomed {
        return format!("OOM-KILLED (hit inner MemoryMax; {oom_kills} oom_kill event(s))");
    }
    if cpu_timed_out {
        return format!("CPU-TIMEOUT >{cpu_timeout}s cpu");
    }
    if timed_out {
        return format!("TIMEOUT >{timeout}s");
    }
    if pids_guard_tripped {
        return format!("PIDS GUARD ({})", pids_guard_reason.unwrap_or(""));
    }
    if let Some(first) = detail_write_failure.first() {
        return format!("DETAIL CAPTURE FAILED ({first})");
    }
    if let Some(rc) = returncode {
        if rc < 0 {
            let name = signal_name(-rc);
            return format!(
                "received {name} with no validate timeout, pids guard, \
                 or child-cgroup OOM recorded"
            );
        }
    }
    match returncode {
        Some(rc) => format!("exit {rc}"),
        None => "exit None".to_string(),
    }
}

/// A whole DAG plus caller policy.
///
/// `steps` is the graph; `resource_caps` bounds concurrent scarce-resource demand. The
/// memory-model tunables mirror DeepScry's outer-cap behavior and can be left at their
/// defaults.
#[derive(Debug, Clone)]
pub struct DagConfig {
    pub steps: Vec<Step>,
    /// Optional long-form documentation for the WHOLE DAG (default empty). Free-form prose
    /// describing the pipeline as a whole; never affects scheduling.
    pub description: String,
    pub resource_caps: BTreeMap<String, i64>,
    /// Multiplier from a step's measured RSS baseline to its inner memory cap (headroom).
    pub mem_cap_factor: f64,
    /// Lower bound (bytes) on the modeled worst-case footprint. Default 8 GiB.
    pub mem_cap_floor_bytes: i64,
    /// Multiplier applied to the modeled peak to leave headroom. 1.0 = no inflation.
    pub outer_mem_safety_factor: f64,
    pub default_step_timeout: i64,
    /// Default inner-parallelism flag template for steps that don't set their own `jobs_flag`.
    pub default_jobs_flag: String,
}

impl Default for DagConfig {
    fn default() -> Self {
        DagConfig {
            steps: Vec::new(),
            description: String::new(),
            resource_caps: BTreeMap::new(),
            mem_cap_factor: 1.25,
            mem_cap_floor_bytes: 8 * 1024i64.pow(3),
            outer_mem_safety_factor: 1.0,
            default_step_timeout: DEFAULT_STEP_TIMEOUT,
            default_jobs_flag: DEFAULT_JOBS_FLAG.to_string(),
        }
    }
}

impl DagConfig {
    /// Map each step tag to a reference to the step (mirrors Python's `by_tag`).
    pub fn by_tag(&self) -> BTreeMap<String, &Step> {
        self.steps.iter().map(|s| (s.tag(), s)).collect()
    }
}

/// Terminal result of one scheduled step.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    /// The step's tag (`"group.job"`).
    pub tag: String,
    /// Whether the step succeeded (exit 0, not timed out).
    pub ok: bool,
    /// Wall-clock seconds the step ran.
    pub duration_s: f64,
    /// One-line summary extracted from the step's output (`""` when unavailable).
    pub summary: String,
    /// Child process exit code; negative for a Unix signal; `None` if never collected.
    pub returncode: Option<i64>,
    /// Human-readable failure reason; `""` when `ok`.
    pub reason: String,
    /// True when eager-exit killed this in-flight step after ANOTHER step failed.
    pub aborted: bool,
}

impl StepOutcome {
    /// Build a passing outcome.
    pub fn passed(tag: String, duration_s: f64, summary: String, returncode: Option<i64>) -> Self {
        StepOutcome {
            tag,
            ok: true,
            duration_s,
            summary,
            returncode,
            reason: String::new(),
            aborted: false,
        }
    }

    /// Build a failed outcome, deriving `reason` from the shared precedence rule.
    #[allow(clippy::too_many_arguments)]
    pub fn failed(
        tag: String,
        duration_s: f64,
        summary: String,
        returncode: Option<i64>,
        oomed: bool,
        oom_kills: i64,
        timed_out: bool,
        timeout: i64,
        cpu_timed_out: bool,
        cpu_timeout: i64,
        aborted: bool,
    ) -> Self {
        let reason = step_failure_reason(
            returncode,
            oomed,
            oom_kills,
            timed_out,
            timeout,
            false,
            None,
            &[],
            // The Rust scheduler now enforces per-step CPU-time budgets under boxing,
            // at parity with the Python runner; thread the real breach flag through.
            cpu_timed_out,
            cpu_timeout,
        );
        StepOutcome {
            tag,
            ok: false,
            duration_s,
            summary,
            returncode,
            reason,
            aborted,
        }
    }

    /// Build an eager-exit ABORTED outcome (a cancellation, not a genuine failure).
    pub fn aborted_outcome(
        tag: String,
        duration_s: f64,
        summary: String,
        returncode: Option<i64>,
    ) -> Self {
        StepOutcome {
            tag,
            ok: false,
            duration_s,
            summary,
            returncode,
            reason: "ABORTED (eager-exit after another step failed; keep_going lets in-flight steps finish)"
                .to_string(),
            aborted: true,
        }
    }
}

/// Aggregate outcome of a whole DAG run.
#[derive(Debug, Clone, Default)]
pub struct RunResult {
    /// Overall pass/fail (no genuine, non-aborted failure occurred).
    pub ok: bool,
    /// Wall-clock seconds the whole run took.
    pub wall_s: f64,
    /// Per-step terminal results, in dispatch (LPT) order.
    pub outcomes: Vec<StepOutcome>,
    /// Tags whose dependencies failed so they never ran (sorted).
    pub skipped: Vec<String>,
    /// Per-step measurement rows (column -> value) to forward to a metrics sink; empty when no
    /// cgroup manager supplied per-step metrics.
    pub step_profile_rows: Vec<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_browser_promotes_to_latency() {
        let mut hint = ResourceHint::default();
        hint.resources.insert("browser".to_string(), 1);
        let step = Step {
            group: "e2e".into(),
            job: "smoke".into(),
            desc: String::new(),
            description: String::new(),
            cmd: "true".into(),
            deps: vec![],
            env: BTreeMap::new(),
            hint,
            networkonly: false,
            engine_only: false,
            timeout: DEFAULT_STEP_TIMEOUT,
            cpu_timeout: 0,
            jobs_flag: None,
        };
        assert_eq!(step_classification(&step), StepClass::LatencyBound);
    }

    fn bare_step(cmd: &str, jobs_flag: Option<&str>) -> Step {
        Step {
            group: "g".into(),
            job: "j".into(),
            desc: String::new(),
            description: String::new(),
            cmd: cmd.into(),
            deps: vec![],
            env: BTreeMap::new(),
            hint: ResourceHint::default(),
            networkonly: false,
            engine_only: false,
            timeout: DEFAULT_STEP_TIMEOUT,
            cpu_timeout: 0,
            jobs_flag: jobs_flag.map(str::to_string),
        }
    }

    #[test]
    fn render_jobs_flag_forms() {
        assert_eq!(render_jobs_flag("-j", 4), "-j 4");
        assert_eq!(render_jobs_flag("-j%d", 4), "-j4");
        assert_eq!(render_jobs_flag("--jobs=", 8), "--jobs=8");
        assert_eq!(render_jobs_flag("--num-threads", 2), "--num-threads 2");
        assert_eq!(render_jobs_flag("--threads=%d", 3), "--threads=3");
    }

    #[test]
    fn command_with_inner_jobs_appends_and_respects_defaults() {
        // No inner jobs -> unchanged.
        let s = bare_step("make", None);
        assert_eq!(command_with_inner_jobs(&s, "-j", None), "make");
        // Inner jobs + default template.
        assert_eq!(command_with_inner_jobs(&s, "-j", Some(4)), "make -j 4");
        // Step-level jobs_flag overrides the default.
        let s2 = bare_step("cargo build", Some("-j%d"));
        assert_eq!(
            command_with_inner_jobs(&s2, "-j", Some(8)),
            "cargo build -j8"
        );
        // Empty template disables appending.
        let s3 = bare_step("mytool", Some(""));
        assert_eq!(command_with_inner_jobs(&s3, "-j", Some(4)), "mytool");
    }

    #[test]
    fn failure_reason_precedence() {
        // OOM beats a signal.
        assert_eq!(
            step_failure_reason(Some(-9), true, 2, false, 10, false, None, &[], false, 0),
            "OOM-KILLED (hit inner MemoryMax; 2 oom_kill event(s))"
        );
        // CPU-timeout beats a wall timeout (more specific cause).
        assert_eq!(
            step_failure_reason(Some(-9), false, 0, true, 600, false, None, &[], true, 30),
            "CPU-TIMEOUT >30s cpu"
        );
        // timeout beats a signal.
        assert_eq!(
            step_failure_reason(Some(-15), false, 0, true, 30, false, None, &[], false, 0),
            "TIMEOUT >30s"
        );
        // negative return code without oom/timeout -> signal name.
        assert_eq!(
            step_failure_reason(Some(-9), false, 0, false, 10, false, None, &[], false, 0),
            "received SIGKILL with no validate timeout, pids guard, \
             or child-cgroup OOM recorded"
        );
        // plain non-zero exit.
        assert_eq!(
            step_failure_reason(Some(1), false, 0, false, 10, false, None, &[], false, 0),
            "exit 1"
        );
    }
}
