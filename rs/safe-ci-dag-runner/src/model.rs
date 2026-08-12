//! Core DAG configuration, step, resource, and result types.

// Core DAG vocabulary for safe-ci-dag-runner.
//
// Pure data + pure helpers, no I/O. A caller describes their build/test graph as a set of
// [`Step`] values (each carrying a [`ResourceHint`]) bundled in a [`DagConfig`], then hands
// it to the runner. Direct port of `py/safe_ci_dag_runner/model.py`; the enum serde values,
// defaults, and the [`step_failure_reason`] precedence + strings are kept identical to the
// Python build so the two are cross-differential-testable.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// Default wall-clock timeout for one step, in seconds.
pub const DEFAULT_STEP_TIMEOUT: i64 = 1800;

/// Default command-line template for a step's inner-job width.
pub const DEFAULT_JOBS_FLAG: &str = "-j";

// Deliberately SMALL default caps for a step that DECLARES NOTHING — the "forcing function".
// An undeclared step is boxed into a tight 1-core / 1-GiB / 10-s-CPU floor, so a real step
// immediately hits the cap and must DECLARE its true needs, generating per-node resource
// metadata EMPIRICALLY (from measured breaches) instead of by guessing. Each applies ONLY
// when the step leaves the matching hint unset; an explicit hint wins. Mirror Python's
// `DEFAULT_SMALL_*` constants.
/// Default one-gibibyte memory cap applied to a step with no declared memory need.
pub const DEFAULT_SMALL_MEM_CAP_BYTES: i64 = 1024i64.pow(3); // 1 GiB inner memory.max
/// Default one-core limit applied to a step with no declared width.
pub const DEFAULT_SMALL_CPU_COUNT: i64 = 1; // 1-core cpu.max
/// Default CPU-time limit applied to a step with no declared budget, in seconds.
pub const DEFAULT_SMALL_CPU_TIMEOUT: i64 = 10; // 10 s CPU-time budget

/// Per-platform CPU-budget multiplier, applied at EXECUTION time to whatever CPU budget is in
/// effect for a step. A CPU second is load-immune (wall = cpu_busy + wait; contention inflates
/// only wait) but it is NOT clock-immune: a slower core retires the same instruction stream over
/// more seconds of CPU occupancy, so identical work legitimately burns more CPU-seconds on an
/// underpowered runner. A graph therefore carries ONE canonical `cpu_timeout` per step and the
/// platform scales it here.
///
/// Applying it at execution — rather than baking a second column of pre-multiplied numbers into
/// the graph — is the whole point: two independently-maintained timeout tables drift, and a step
/// has only one `cpu_timeout` field, so a per-platform column would force declaration authors to
/// pick a single number that is either too tight for the slow platform or too loose for the fast
/// one (hiding the very hangs the budget exists to catch).
///
/// 1.0 is a strict no-op and the default for every execution platform.
pub const DEFAULT_CPU_TIMEOUT_MULTIPLIER: f64 = 1.0;

/// Environment override for [`DEFAULT_CPU_TIMEOUT_MULTIPLIER`], so a CI lane can set the policy
/// once for its whole platform.
pub const CPU_TIMEOUT_MULTIPLIER_ENV: &str = "SAFE_CI_DAG_RUNNER_CPU_TIMEOUT_MULTIPLIER";

/// Companion label naming the platform the multiplier describes; appears verbatim in the breach
/// message.
pub const CPU_TIMEOUT_PLATFORM_ENV: &str = "SAFE_CI_DAG_RUNNER_CPU_TIMEOUT_PLATFORM";

/// How a step uses the machine, used for scheduling decisions.
///
/// The serde/string values (`"cpu-bound"`, `"latency-bound"`, `"light"`) are load-bearing:
/// they appear verbatim in JSON, `list`, `ascii`, and `dot` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepClass {
    /// Throughput scales primarily with CPU allocation.
    CpuBound,
    /// Completion is latency-sensitive rather than throughput-oriented.
    LatencyBound,
    /// The step has no special CPU or latency classification.
    #[default]
    Light,
}

/// Closed vocabulary for nodes deliberately omitted before process spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentionalSkipReason {
    /// The manifest planner selected no test cells for this static bucket.
    EmptyManifestBucket,
}

impl IntentionalSkipReason {
    /// Return the stable serialized reason.
    pub fn value(self) -> &'static str {
        match self {
            Self::EmptyManifestBucket => "empty-manifest-bucket",
        }
    }

    /// Parse one stable serialized reason.
    pub fn from_value(text: &str) -> Option<Self> {
        match text {
            "empty-manifest-bucket" => Some(Self::EmptyManifestBucket),
            _ => None,
        }
    }
}

impl StepClass {
    /// Return the canonical serialized value.
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

/// Why concurrent writes declared by a step are safe.
///
/// These values are deliberately not scheduler resources: artifact-shielded and
/// path-isolated writers retain parallelism instead of collapsing onto one global mutex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteDomainGuarantee {
    /// Consumers are shielded behind an immutable artifact barrier before later writers run.
    ArtifactBarrierDependent,
    /// The node atomically publishes the immutable artifact consumed by shielded nodes.
    ImmutableArtifactBarrier,
    /// The writer uses package/path-disjoint output.
    ExplicitlyIsolated,
    /// The node creates the mutable artifact set consumed by a later barrier.
    ArtifactProducer,
}

impl WriteDomainGuarantee {
    /// Canonical serialized spelling.
    pub fn value(self) -> &'static str {
        match self {
            Self::ArtifactBarrierDependent => "artifact-barrier-dependent",
            Self::ImmutableArtifactBarrier => "immutable-artifact-barrier",
            Self::ExplicitlyIsolated => "explicitly-isolated",
            Self::ArtifactProducer => "artifact-producer",
        }
    }

    /// Parse a canonical spelling.
    pub fn from_value(text: &str) -> Option<Self> {
        match text {
            "artifact-barrier-dependent" => Some(Self::ArtifactBarrierDependent),
            "immutable-artifact-barrier" => Some(Self::ImmutableArtifactBarrier),
            "explicitly-isolated" => Some(Self::ExplicitlyIsolated),
            "artifact-producer" => Some(Self::ArtifactProducer),
            _ => None,
        }
    }
}

/// Closed write-domain vocabulary and omission policy for one DAG.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteDomainPolicy {
    /// Require every step to carry `write_domains`, using `[]` for no protected artifact domains.
    pub require_explicit: bool,
    /// Domain names accepted by this DAG.
    pub allowed_domains: BTreeSet<String>,
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
    /// Scheduling classification for the step.
    pub classification: StepClass,
    /// Internal parallelism width for the step's own command (e.g. a build's `-j`).
    pub preferred_inner_jobs: Option<i64>,
    /// Effective CPU concurrency measured for the preferred width.
    pub measured_effective_cores: Option<f64>,
    /// Measured CPU utilization as a fraction of one allocated-width interval.
    pub measured_cpu_utilization: Option<f64>,
}

/// One node in the DAG: a shell command plus its dependencies and resource hint.
#[derive(Debug, Clone)]
pub struct Step {
    /// Namespace component of the unique step tag.
    pub group: String,
    /// Job component of the unique step tag.
    pub job: String,
    /// Short human-readable label shown in command output.
    pub desc: String,
    /// Optional long-form documentation for this node (default empty). Unlike `desc` — a short
    /// label shown by `list`/`run` — `description` is free-form prose (often multi-line, e.g. a
    /// YAML block scalar) documenting WHY the step exists. It never affects scheduling.
    pub description: String,
    /// Shell command (`bash -c`), run from the run's working directory.
    pub cmd: String,
    /// Tags (`"group.job"`) this step depends on.
    pub deps: Vec<String>,
    /// Environment variables added to the step process.
    pub env: BTreeMap<String, String>,
    /// Resource and scheduling metadata for this step.
    pub hint: ResourceHint,
    /// Skipped when networking is disabled.
    pub networkonly: bool,
    /// Selected only by an engine-only subset preset.
    pub engine_only: bool,
    /// Wall-clock timeout in seconds.
    pub timeout: i64,
    // CPU-time budget in seconds (user+system, from the step's cgroup `cpu.stat`). `0` disables
    // the CPU-time guard, leaving only the wall `timeout`. CPU time is immune to machine load, so
    // this can be set far tighter than a load-tolerant wall timeout without flaking. Mirrors
    // Python's `Step.cpu_timeout`; both runners enforce it identically under cgroup boxing.
    /// User-plus-system CPU-time timeout in seconds; zero disables the guard.
    pub cpu_timeout: i64,
    /// Template for the inner-parallelism flag appended to `cmd` when this step declares
    /// `preferred_inner_jobs`. `None` inherits `DagConfig::default_jobs_flag`; an empty string
    /// disables appending (the step manages its own concurrency). See [`render_jobs_flag`].
    pub jobs_flag: Option<String>,
    /// Typed pre-execution omission, separate from PASS and dependency skip.
    pub skip_reason: Option<IntentionalSkipReason>,
    /// Presence-sensitive declaration: `None` is omitted; `Some(vec![])` explicitly declares no
    /// writes to the policy's protected artifact domains.
    pub write_domains: Option<Vec<String>>,
    /// Structural guarantee used for non-empty write-domain declarations.
    pub write_domain_guarantee: Option<WriteDomainGuarantee>,
}

impl Step {
    /// The step's unique tag, `"group.job"`.
    pub fn tag(&self) -> String {
        format!("{}.{}", self.group, self.job)
    }
}

/// Render an inner-parallelism flag from a template and worker count.
///
/// A `%d` placeholder is substituted directly, a template ending in `=` is concatenated directly,
/// and every other template is separated from the count by one space.
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

/// Return a step command with its effective inner-job flag appended when requested.
///
/// A missing width or empty effective template leaves the command unchanged.
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

/// Resolve a step's scheduling class from its explicit hint and resource demands.
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

/// CANONICAL CPU-time budget (seconds) for a step, before any per-platform scaling: its declared
/// `cpu_timeout` (>0) wins; otherwise the DAG's SMALL default. Both 0 means the guard is
/// disabled. This is the number a graph declares — one table, platform-independent.
pub fn canonical_cpu_timeout(step: &Step, default_cpu_timeout: i64) -> i64 {
    if step.cpu_timeout > 0 {
        step.cpu_timeout
    } else {
        default_cpu_timeout
    }
}

/// Apply a per-platform multiplier to a canonical CPU budget.
///
/// Rounds to whole seconds (the enforcement poll is 1 Hz) and never rounds a live budget down to
/// 0 — that would silently DISABLE the guard on a platform with a small multiplier, turning a
/// scaling policy into an opt-out. A disabled budget (canonical 0) stays disabled.
pub fn scale_cpu_timeout(canonical: i64, multiplier: f64) -> i64 {
    if canonical <= 0 {
        return 0;
    }
    if multiplier == DEFAULT_CPU_TIMEOUT_MULTIPLIER {
        return canonical;
    }
    let scaled = (canonical as f64 * multiplier).round() as i64;
    scaled.max(1)
}

/// CPU-time budget actually ENFORCED for a step on this platform: the canonical budget scaled by
/// the platform multiplier. With the default 1.0 multiplier this is exactly the canonical budget.
pub fn effective_cpu_timeout(step: &Step, default_cpu_timeout: i64, multiplier: f64) -> i64 {
    scale_cpu_timeout(canonical_cpu_timeout(step, default_cpu_timeout), multiplier)
}

/// Resolve the platform CPU-budget multiplier and its label from an explicit value then the
/// environment. A malformed or non-positive environment value is REFUSED rather than silently
/// ignored — a typo that quietly reverted to 1.0 would loosen enforcement invisibly.
pub fn resolve_cpu_timeout_multiplier(explicit: Option<f64>) -> Result<(f64, String), String> {
    let label = std::env::var(CPU_TIMEOUT_PLATFORM_ENV)
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Some(value) = explicit {
        if value <= 0.0 {
            return Err(format!("cpu-timeout multiplier must be > 0, got {value}"));
        }
        return Ok((value, label));
    }
    let raw = std::env::var(CPU_TIMEOUT_MULTIPLIER_ENV).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok((DEFAULT_CPU_TIMEOUT_MULTIPLIER, label));
    }
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("{CPU_TIMEOUT_MULTIPLIER_ENV}={raw:?} is not a number"))?;
    if value <= 0.0 {
        return Err(format!("{CPU_TIMEOUT_MULTIPLIER_ENV}={raw:?} must be > 0"));
    }
    Ok((value, label))
}

/// `" (canonical 3s x2 github-hosted)"` when a platform multiplier scaled the budget, else empty.
/// Silent at 1.0 so the common unscaled message is unchanged.
fn cpu_timeout_policy_suffix(canonical: i64, multiplier: f64, platform: &str) -> String {
    if multiplier == DEFAULT_CPU_TIMEOUT_MULTIPLIER || canonical <= 0 {
        return String::new();
    }
    let rendered = format_multiplier(multiplier);
    let label = if platform.is_empty() {
        String::new()
    } else {
        format!(" {platform}")
    };
    format!(" (canonical {canonical}s x{rendered}{label})")
}

/// Render a multiplier compactly (`2.0 -> "2"`, `1.5 -> "1.5"`) for stable breach strings.
fn format_multiplier(value: f64) -> String {
    let mut s = format!("{value}");
    if let Some(stripped) = s.strip_suffix(".0") {
        s = stripped.to_string();
    }
    s
}

/// Core cap (cgroup `cpu.max`) in effect for a step: its declared `preferred_inner_jobs` wins;
/// otherwise the DAG's SMALL default. Bounds cpu.max ONLY, never the command's inner `-j` flag
/// (which stays keyed to the declared width).
pub fn effective_cpu_count(step: &Step, default_cpu_count: Option<i64>) -> Option<i64> {
    step.hint.preferred_inner_jobs.or(default_cpu_count)
}

// Map a Unix signal number to its name (e.g. `9 -> "SIGKILL"`), matching the names Python's
// `signal.Signals(n).name` produces for the common signals; unknown numbers fall back to
// `"signal N"` exactly like the Python `ValueError` branch.
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
/// Failure-reason precedence is:
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
    cpu_timeout_canonical: i64,
    cpu_timeout_multiplier: f64,
    cpu_timeout_platform: &str,
) -> String {
    if oomed {
        return format!("OOM-KILLED (hit inner MemoryMax; {oom_kills} oom_kill event(s))");
    }
    if cpu_timed_out {
        // When a platform multiplier is in effect the enforced number is NOT the number written
        // in the graph, so the message must carry both plus the policy that connects them.
        let suffix = cpu_timeout_policy_suffix(
            cpu_timeout_canonical,
            cpu_timeout_multiplier,
            cpu_timeout_platform,
        );
        return format!("CPU-TIMEOUT >{cpu_timeout}s cpu{suffix}");
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

/// A complete step graph and its scheduling, memory, and containment policy.
#[derive(Debug, Clone)]
pub struct DagConfig {
    /// Steps in deterministic declaration order.
    pub steps: Vec<Step>,
    // Optional long-form documentation for the WHOLE DAG (default empty). Free-form prose
    // describing the pipeline as a whole; never affects scheduling.
    /// Optional long-form description of the whole DAG.
    pub description: String,
    /// Maximum concurrent capacity for each named scarce resource.
    pub resource_caps: BTreeMap<String, i64>,
    /// Multiplier from a step's measured RSS baseline to its inner memory cap (headroom).
    pub mem_cap_factor: f64,
    /// Lower bound (bytes) on the modeled worst-case footprint. Default 8 GiB.
    pub mem_cap_floor_bytes: i64,
    /// Multiplier applied to the modeled peak to leave headroom. 1.0 = no inflation.
    pub outer_mem_safety_factor: f64,
    /// Default wall-clock timeout for steps, in seconds.
    pub default_step_timeout: i64,
    /// Default inner-parallelism flag template for steps that don't set their own `jobs_flag`.
    pub default_jobs_flag: String,
    /// Deliberately SMALL default inner memory.max (bytes) for a step that declares NO memory
    /// hint (the forcing function; see `DEFAULT_SMALL_MEM_CAP_BYTES`). `None` disables it.
    pub default_step_mem_cap_bytes: Option<i64>,
    /// Deliberately SMALL default core cap (cgroup cpu.max) for a step that declares no inner
    /// width (see `DEFAULT_SMALL_CPU_COUNT`). `None` disables it. Bounds cpu.max only, never
    /// the command's `-j` flag.
    pub default_step_cpu_count: Option<i64>,
    /// Deliberately SMALL default CPU-time budget (seconds) for a step whose `cpu_timeout` is
    /// unset (see `DEFAULT_SMALL_CPU_TIMEOUT`). `0` disables it.
    pub default_step_cpu_timeout: i64,
    /// Execution-time multiplier over the canonical CPU budget for THIS platform (see
    /// `DEFAULT_CPU_TIMEOUT_MULTIPLIER`). Caller/platform policy, never persisted with the graph.
    pub cpu_timeout_multiplier: f64,
    /// Free-form platform label reported alongside the multiplier in a breach message.
    pub cpu_timeout_platform: String,
    /// Fail-closed write-domain policy. Default is disabled for generic DAGs that do not opt in.
    pub write_domain_policy: WriteDomainPolicy,
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
            // The SMALL forcing-function caps are active by default. The declarations-first
            // migration supplied measured budgets for nodes that exceed the floor; an explicit
            // per-step declaration still wins. `--unsafe-no-cgroups` is the deliberately loud
            // escape hatch for an unboxed run.
            default_step_mem_cap_bytes: Some(DEFAULT_SMALL_MEM_CAP_BYTES),
            default_step_cpu_count: Some(DEFAULT_SMALL_CPU_COUNT),
            default_step_cpu_timeout: DEFAULT_SMALL_CPU_TIMEOUT,
            cpu_timeout_multiplier: DEFAULT_CPU_TIMEOUT_MULTIPLIER,
            cpu_timeout_platform: String::new(),
            write_domain_policy: WriteDomainPolicy::default(),
        }
    }
}

impl DagConfig {
    /// Index the steps by their unique fully qualified tags.
    pub fn by_tag(&self) -> BTreeMap<String, &Step> {
        self.steps.iter().map(|s| (s.tag(), s)).collect()
    }
}

/// Return deterministic fail-closed write-domain declaration errors.
///
/// Parsers call this while loading, and scheduler entry points call it again so an
/// in-memory `DagConfig` cannot bypass the file parser.
pub fn write_domain_violations(cfg: &DagConfig) -> Vec<String> {
    let policy = &cfg.write_domain_policy;
    if !policy.require_explicit && policy.allowed_domains.is_empty() {
        return Vec::new();
    }
    let mut bad = Vec::new();
    let by_tag = cfg.by_tag();
    for step in &cfg.steps {
        let Some(domains) = &step.write_domains else {
            if policy.require_explicit {
                bad.push(format!(
                    "{}: missing write_domains (use [] for no protected domains)",
                    step.tag()
                ));
            }
            if step.write_domain_guarantee.is_some() {
                bad.push(format!(
                    "{}: write_domain_guarantee requires write_domains",
                    step.tag()
                ));
            }
            continue;
        };
        let mut seen = BTreeSet::new();
        let duplicates: BTreeSet<String> = domains
            .iter()
            .filter(|name| !seen.insert((*name).clone()))
            .cloned()
            .collect();
        if !duplicates.is_empty() {
            bad.push(format!(
                "{}: duplicate write_domains: {}",
                step.tag(),
                duplicates.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        let unknown: Vec<String> = domains
            .iter()
            .filter(|name| !policy.allowed_domains.contains(*name))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !unknown.is_empty() {
            bad.push(format!(
                "{}: unknown write_domains: {}",
                step.tag(),
                unknown.join(", ")
            ));
        }
        if !domains.is_empty() && step.write_domain_guarantee.is_none() {
            bad.push(format!(
                "{}: nonempty write_domains require write_domain_guarantee",
                step.tag()
            ));
        }
        if domains.is_empty() && step.write_domain_guarantee.is_some() {
            bad.push(format!(
                "{}: write_domains=[] cannot claim a write guarantee",
                step.tag()
            ));
        }
        if step.write_domain_guarantee == Some(WriteDomainGuarantee::ArtifactBarrierDependent) {
            let mut pending = step.deps.clone();
            let mut seen = BTreeSet::new();
            let mut found = false;
            while let Some(tag) = pending.pop() {
                if !seen.insert(tag.clone()) {
                    continue;
                }
                let Some(ancestor) = by_tag.get(&tag) else {
                    continue;
                };
                if ancestor.write_domain_guarantee
                    == Some(WriteDomainGuarantee::ImmutableArtifactBarrier)
                {
                    found = true;
                    break;
                }
                pending.extend(ancestor.deps.iter().cloned());
            }
            if !found {
                bad.push(format!(
                    "{}: artifact-barrier-dependent but no transitive dependency is an \
                     immutable-artifact-barrier",
                    step.tag()
                ));
            }
        }
    }
    bad
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
    /// Tests executed according to the step's complete captured runner output.
    /// `None` means no recognizable test-runner banner, distinct from `Some(0)`.
    pub executed_tests: Option<u64>,
    /// Tests filtered according to the same complete captured output.
    pub filtered_tests: Option<u64>,
    /// Child process exit code; negative for a Unix signal; `None` if never collected.
    pub returncode: Option<i64>,
    /// Human-readable failure reason; `""` when `ok`.
    pub reason: String,
    /// True when eager-exit killed this in-flight step after ANOTHER step failed.
    pub aborted: bool,
}

impl StepOutcome {
    /// Build a passing outcome.
    pub fn passed(
        tag: String,
        duration_s: f64,
        summary: String,
        returncode: Option<i64>,
        executed_tests: Option<u64>,
        filtered_tests: Option<u64>,
    ) -> Self {
        StepOutcome {
            tag,
            ok: true,
            duration_s,
            summary,
            executed_tests,
            filtered_tests,
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
        cpu_timeout_canonical: i64,
        cpu_timeout_multiplier: f64,
        cpu_timeout_platform: &str,
        aborted: bool,
        executed_tests: Option<u64>,
        filtered_tests: Option<u64>,
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
            cpu_timeout_canonical,
            cpu_timeout_multiplier,
            cpu_timeout_platform,
        );
        StepOutcome {
            tag,
            ok: false,
            duration_s,
            summary,
            executed_tests,
            filtered_tests,
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
        executed_tests: Option<u64>,
        filtered_tests: Option<u64>,
    ) -> Self {
        StepOutcome {
            tag,
            ok: false,
            duration_s,
            summary,
            executed_tests,
            filtered_tests,
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
    /// `(tag, stable reason)` for nodes deliberately omitted before process spawn.
    pub intentional_skips: Vec<(String, IntentionalSkipReason)>,
    /// Per-step measurement rows (column -> value) to forward to a metrics sink; empty when no
    /// cgroup manager supplied per-step metrics.
    pub step_profile_rows: Vec<BTreeMap<String, String>>,
    /// The WHOLE RUN hit its outer wall budget and was cut short.
    ///
    /// Distinct from a step's own `timed_out`: no single node necessarily misbehaved, the
    /// combination did. A consumer that records results must be able to tell "this run produced a
    /// verdict about the tree" from "this run was stopped by its own budget with work still
    /// outstanding", and `ok == false` alone cannot.
    pub run_timed_out: bool,
    /// Largest number of step child processes observed alive at the same time. Measured from
    /// successful spawn until wait observes exit, not inferred from jobs or scheduler admission.
    pub max_concurrent_steps: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_enables_small_forcing_caps() {
        let cfg = DagConfig::default();
        assert_eq!(
            cfg.default_step_mem_cap_bytes,
            Some(DEFAULT_SMALL_MEM_CAP_BYTES)
        );
        assert_eq!(cfg.default_step_cpu_count, Some(DEFAULT_SMALL_CPU_COUNT));
        assert_eq!(cfg.default_step_cpu_timeout, DEFAULT_SMALL_CPU_TIMEOUT);
    }

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
            skip_reason: None,
            write_domains: None,
            write_domain_guarantee: None,
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
            skip_reason: None,
            write_domains: None,
            write_domain_guarantee: None,
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
            step_failure_reason(
                Some(-9),
                true,
                2,
                false,
                10,
                false,
                None,
                &[],
                false,
                0,
                0,
                DEFAULT_CPU_TIMEOUT_MULTIPLIER,
                ""
            ),
            "OOM-KILLED (hit inner MemoryMax; 2 oom_kill event(s))"
        );
        // CPU-timeout beats a wall timeout (more specific cause).
        assert_eq!(
            step_failure_reason(
                Some(-9),
                false,
                0,
                true,
                600,
                false,
                None,
                &[],
                true,
                30,
                0,
                DEFAULT_CPU_TIMEOUT_MULTIPLIER,
                ""
            ),
            "CPU-TIMEOUT >30s cpu"
        );
        // timeout beats a signal.
        assert_eq!(
            step_failure_reason(
                Some(-15),
                false,
                0,
                true,
                30,
                false,
                None,
                &[],
                false,
                0,
                0,
                DEFAULT_CPU_TIMEOUT_MULTIPLIER,
                ""
            ),
            "TIMEOUT >30s"
        );
        // negative return code without oom/timeout -> signal name.
        assert_eq!(
            step_failure_reason(
                Some(-9),
                false,
                0,
                false,
                10,
                false,
                None,
                &[],
                false,
                0,
                0,
                DEFAULT_CPU_TIMEOUT_MULTIPLIER,
                ""
            ),
            "received SIGKILL with no validate timeout, pids guard, \
             or child-cgroup OOM recorded"
        );
        // plain non-zero exit.
        assert_eq!(
            step_failure_reason(
                Some(1),
                false,
                0,
                false,
                10,
                false,
                None,
                &[],
                false,
                0,
                0,
                DEFAULT_CPU_TIMEOUT_MULTIPLIER,
                ""
            ),
            "exit 1"
        );
    }
}

#[cfg(test)]
mod cpu_timeout_multiplier_tests {
    //! Per-platform CPU-budget multiplier contract.
    //!
    //! The scaled number and breach string must remain stable across implementations. The
    //! rounding case is not incidental: half-away-from-zero is required at every `.5` tie.
    use super::*;

    fn step(cpu_timeout: i64) -> Step {
        Step {
            group: "g".into(),
            job: "j".into(),
            desc: "d".into(),
            description: String::new(),
            cmd: "true".into(),
            deps: vec![],
            env: std::collections::BTreeMap::new(),
            hint: ResourceHint::default(),
            networkonly: false,
            engine_only: false,
            timeout: DEFAULT_STEP_TIMEOUT,
            cpu_timeout,
            jobs_flag: None,
            skip_reason: None,
            write_domains: None,
            write_domain_guarantee: None,
        }
    }

    #[test]
    fn unity_is_a_strict_no_op() {
        assert_eq!(DEFAULT_CPU_TIMEOUT_MULTIPLIER, 1.0);
        let cfg = DagConfig::default();
        assert_eq!(cfg.cpu_timeout_multiplier, DEFAULT_CPU_TIMEOUT_MULTIPLIER);
        assert_eq!(cfg.cpu_timeout_platform, "");
        let s = step(30);
        assert_eq!(canonical_cpu_timeout(&s, DEFAULT_SMALL_CPU_TIMEOUT), 30);
        assert_eq!(
            effective_cpu_timeout(
                &s,
                DEFAULT_SMALL_CPU_TIMEOUT,
                DEFAULT_CPU_TIMEOUT_MULTIPLIER
            ),
            30
        );
    }

    #[test]
    fn declared_and_default_budgets_both_scale() {
        assert_eq!(
            effective_cpu_timeout(&step(30), DEFAULT_SMALL_CPU_TIMEOUT, 2.0),
            60
        );
        assert_eq!(
            effective_cpu_timeout(&step(30), DEFAULT_SMALL_CPU_TIMEOUT, 1.5),
            45
        );
        // The forcing-function floor is a budget like any other.
        assert_eq!(effective_cpu_timeout(&step(0), 10, 2.0), 20);
    }

    #[test]
    fn rounding_matches_python_half_away_from_zero() {
        // These exact pairs are asserted in the Python suite too. Banker's rounding would give
        // 4 / 10 / 8 / 2 / 14 for the .5 ties and diverge from this engine.
        assert_eq!(scale_cpu_timeout(3, 1.5), 5);
        assert_eq!(scale_cpu_timeout(7, 1.5), 11);
        assert_eq!(scale_cpu_timeout(5, 1.5), 8);
        assert_eq!(scale_cpu_timeout(1, 1.5), 2);
        assert_eq!(scale_cpu_timeout(9, 1.5), 14);
        assert_eq!(scale_cpu_timeout(3, 2.0), 6);
    }

    #[test]
    fn scaling_can_never_become_an_opt_out() {
        // A disabled budget stays disabled; multiplying must not invent a guard.
        assert_eq!(scale_cpu_timeout(0, 2.0), 0);
        assert_eq!(scale_cpu_timeout(-5, 2.0), 0);
        // A sub-unity multiplier must never round a live budget down to 0 (= guard removed).
        assert_eq!(scale_cpu_timeout(3, 0.1), 1);
        assert_eq!(scale_cpu_timeout(1, 0.01), 1);
    }

    #[test]
    fn scaled_breach_names_canonical_multiplier_and_platform() {
        let reason = step_failure_reason(
            Some(-9),
            false,
            0,
            false,
            600,
            false,
            None,
            &[],
            true,
            60,
            30,
            2.0,
            "github-hosted",
        );
        assert_eq!(
            reason,
            "CPU-TIMEOUT >60s cpu (canonical 30s x2 github-hosted)"
        );
    }

    #[test]
    fn breach_is_unchanged_at_unity_and_platform_label_is_optional() {
        let unscaled = step_failure_reason(
            Some(-9),
            false,
            0,
            false,
            600,
            false,
            None,
            &[],
            true,
            30,
            30,
            DEFAULT_CPU_TIMEOUT_MULTIPLIER,
            "github-hosted",
        );
        assert_eq!(unscaled, "CPU-TIMEOUT >30s cpu");
        let unlabelled = step_failure_reason(
            Some(-9),
            false,
            0,
            false,
            600,
            false,
            None,
            &[],
            true,
            45,
            30,
            1.5,
            "",
        );
        assert_eq!(unlabelled, "CPU-TIMEOUT >45s cpu (canonical 30s x1.5)");
    }

    #[test]
    fn oom_still_outranks_a_scaled_cpu_timeout() {
        let reason = step_failure_reason(
            Some(-9),
            true,
            2,
            false,
            600,
            false,
            None,
            &[],
            true,
            60,
            30,
            2.0,
            "github-hosted",
        );
        assert!(reason.starts_with("OOM-KILLED"), "{reason}");
    }

    #[test]
    fn a_nonpositive_explicit_multiplier_is_refused() {
        assert!(resolve_cpu_timeout_multiplier(Some(0.0)).is_err());
        assert!(resolve_cpu_timeout_multiplier(Some(-1.0)).is_err());
        assert_eq!(resolve_cpu_timeout_multiplier(Some(1.5)).unwrap().0, 1.5);
    }
}
