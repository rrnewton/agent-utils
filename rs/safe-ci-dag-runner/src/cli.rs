//! Command-line interface for safe-ci-dag-runner.
//!
//! Subcommands (matching `py/safe_ci_dag_runner/cli.py`):
//!   run --dag FILE    run a DAG (exit 0 iff every step passes)
//!   list --dag FILE   list the steps
//!   ascii --dag FILE  draw the DAG as ASCII art
//!   dot --dag FILE    emit Graphviz DOT
//!   json --dag FILE   re-emit the DAG as canonical JSON
//!   yaml --dag FILE   re-emit the DAG as YAML
//!   quickstart        print a self-contained getting-started guide
//!   --userguide       print the full embedded user guide (the complete reference)
//!
//! `--dag FILE` auto-detects the input format by extension: `.yaml`/`.yml` load as YAML (which is
//! ISOMORPHIC to the JSON schema — same model), everything else as JSON. `--dag -` reads JSON from
//! stdin.
//!
//! `list`, `ascii`, `dot`, and `json` stdout is BYTE-IDENTICAL to the Python build; `--help`,
//! `quickstart`, and `yaml` wording may differ (YAML byte-output is not cross-identical, only YAML
//! *loading* is) but the structure and exit codes (0 / 1 / 2 / 3) match.
//!
//! Cgroup boxing is ON by default (this tool's primary purpose): `run` re-execs inside a
//! transient `systemd-run --user --scope` and caps each step in its own child cgroup. When
//! cgroup-v2 + a working systemd `--user` scope are unavailable the run ERRORS (exit 3);
//! `--allow-cgroup-failure` downgrades to a best-effort UNBOXED run with a visible warning.
//! `--perf-dir DIR` writes per-step + whole-run resource-usage CSVs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::cgroup::{
    apply_core_box, install_scope_teardown, is_in_scope, reexec_in_scope, CgroupManager, Cgroups,
};
use crate::estimates::{
    apply_plan_to_config, build_plan, feedback_identity, load_step_samples, load_step_speedups,
    plan_to_json, plan_to_text, Plan, Planner, DEFAULT_MIN_SAMPLES,
};
use crate::io::{dag_from_json, dag_from_yaml, dag_to_json, dag_to_yaml, DagJsonError};
use crate::model::{step_classification, DagConfig, Step};
use crate::perflog::{append_step_profiles, child_cpu_seconds, PerfWindow};
use crate::profile_enrich::container_core_budget;
use crate::scheduler::{run_dag_boxed, run_dag_boxed_ordered, BoxedCgroups};
use crate::sizing::{cpu_count, jobs_for_budget, parse_size};
use crate::summary::{self, Summary, DEFAULT_MAX_BUCKETS, DEFAULT_RESERVOIR_K};
use crate::sync::{self, SyncBackend};
use crate::viz::{to_ascii, to_dot};
use crate::{ENFORCEMENT_CAPABILITIES, PROG, VERSION};

/// Environment variable overriding the default profile-store location (Feature D). An explicit
/// `--perf-dir` still wins over this; `--no-profile` disables logging entirely.
const PROFILE_DIR_ENV: &str = "SAFE_CI_DAG_RUNNER_PROFILE_DIR";

/// Default profile-store directory, RELATIVE TO THE CURRENT WORKING DIRECTORY, used when neither
/// `--perf-dir` nor `$SAFE_CI_DAG_RUNNER_PROFILE_DIR` is set and `--no-profile` is absent. Created
/// on demand; runs and sweeps auto-append here so profiling data lands somewhere obvious.
const DEFAULT_PROFILE_DIR: &str = ".safe-ci-dag-runner/profiles";

/// The full user guide, embedded at compile time. `scripts/embed_userguides.py` (run by `./setup`)
/// copies the single source `common/docs/safe-ci-dag-runner/USER_GUIDE.md` to this crate-internal
/// path; keeping it UNDER `src/` is what makes the `include_str!` target survive `cargo package` /
/// crates.io (an `include_str!` pointing OUTSIDE the crate would break packaging). The bytes match
/// the Python build's package-resource copy, so `--userguide` is byte-identical across builds.
const USERGUIDE: &str = include_str!("embedded_userguide.md");

// --------------------------------------------------------------------------- colors

/// Minimal ANSI colorizer. Disabled for non-ttys and when `NO_COLOR` is set.
struct Palette {
    enabled: bool,
}

impl Palette {
    fn wrap(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }
    fn bold(&self, t: &str) -> String {
        self.wrap("1", t)
    }
    fn dim(&self, t: &str) -> String {
        self.wrap("2", t)
    }
    fn green(&self, t: &str) -> String {
        self.wrap("32", t)
    }
    fn red(&self, t: &str) -> String {
        self.wrap("1;31", t)
    }
    fn yellow(&self, t: &str) -> String {
        self.wrap("33", t)
    }
    fn cyan(&self, t: &str) -> String {
        self.wrap("36", t)
    }
}

fn color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

fn banner(c: &Palette) -> String {
    format!(
        "{} {}\n\
         Run a DAG of build/test steps concurrently and safely: dependency- and\n\
         resource-aware scheduling with eager-exit on first failure, memory-aware\n\
         concurrency, and DAG visualization.",
        c.bold(PROG),
        c.dim(&format!("v{VERSION}"))
    )
}

fn help_text(c: &Palette) -> String {
    let ex = |s: &str| c.cyan(s);
    format!(
        "{banner}\n\n\
         {usage}\n  {PROG} <command> [options]\n\n\
         {commands}\n\
         \x20 run        run a DAG (exit 0 iff every step passes)\n\
         \x20 sweep      parallel-speedup sweep of ONE step (inner -j1..-jN + timing table)\n\
         \x20 plan       show learned estimates + the scheduled order (does NOT run anything)\n\
         \x20 list       list the steps\n\
         \x20 ascii      draw the DAG as ASCII art\n\
         \x20 dot        emit Graphviz DOT (pipe to `dot -Tsvg`)\n\
         \x20 json       re-emit the DAG as canonical JSON\n\
         \x20 yaml       re-emit the DAG as YAML\n\
         \x20 quickstart print a self-contained getting-started guide\n\
         \x20 --userguide print the full embedded user guide (the complete reference)\n\n\
         {examples}\n\
         \x20 {e1}\n\
         \x20 {e2}\n\
         \x20 {e3}\n\
         \x20 {e6}\n\
         \x20 {e4}\n\
         \x20 {e5}\n\n\
         {profiling}\n",
        banner = banner(c),
        usage = c.bold("usage"),
        commands = c.bold("commands"),
        examples = c.bold("examples"),
        e1 = ex(&format!("{PROG} quickstart")),
        e2 = ex(&format!("{PROG} run --dag dag.json --profile")),
        e3 = ex(&format!("{PROG} run --dag dag.json --only build.app")),
        e6 = ex(&format!("{PROG} plan --dag dag.json --planner critical-path")),
        e4 = ex(&format!("{PROG} sweep --dag dag.json --step build.app --jobs 1..8")),
        e5 = ex(&format!("{PROG} ascii --dag dag.json")),
        profiling = c.dim(
            "Profiling data auto-logs to ./.safe-ci-dag-runner/profiles/ by default\n \
             (override with --perf-dir or $SAFE_CI_DAG_RUNNER_PROFILE_DIR; disable with --no-profile)."
        ),
    )
}

fn quickstart(c: &Palette) -> String {
    let h = |s: &str| c.bold(s);
    let k = |s: &str| c.cyan(s);
    format!(
        "{banner}\n\n\
{i1}\n  pip install \"git+https://github.com/rrnewton/agent-utils#subdirectory=py\"\n  \
(or build the Rust binary: cargo build --release)\n\n\
{i2}  {deps_note}\n  Save as dag.json:\n  {{\n    \"resource_caps\": {{\"browser\": 1}},\n    \"steps\": [\n      \
{{\"group\": \"build\", \"job\": \"app\", \"desc\": \"compile\", \"cmd\": \"echo build && sleep 0.2\"}},\n      \
{{\"group\": \"test\",  \"job\": \"unit\", \"desc\": \"unit tests\", \"cmd\": \"echo test && sleep 0.2\",\n        \"deps\": [\"build.app\"]}},\n      \
{{\"group\": \"e2e\",   \"job\": \"smoke\", \"desc\": \"browser smoke\", \"cmd\": \"echo e2e && sleep 0.2\",\n        \"deps\": [\"build.app\"], \"hint\": {{\"resources\": {{\"browser\": 1}}}}}}\n    ]\n  }}\n\n\
{i3}\n  {r1}\n  {r2}\n  {r3}\n\n\
{studies}\n  {s1}\n  {s2}\n  {s3}\n  {studies_note}\n\n\
{store}\n  {store_dir}\n  {store_note}\n\n\
{planning}\n  {pl1}\n  {pl2}\n  {pl3}\n  {plan_note}\n\n\
{schema}  {schema_note}\n  \
step:   group, job, desc, description, cmd, deps[], env{{}}, timeout, jobs_flag, networkonly, engine_only, hint{{}}\n  \
hint:   resources{{name:int}}, est_duration_s, rss_baseline_bytes, hard_mem_max_bytes,\n          classification(\"cpu-bound\"|\"latency-bound\"|\"light\"), preferred_inner_jobs\n  \
top:    description, resource_caps{{name:int}}, mem_cap_factor, mem_cap_floor_bytes,\n          outer_mem_safety_factor, default_step_timeout, default_jobs_flag\n  \
desc = short label; description = long-form docs (often multi-line, great in YAML)\n  \
jobs_flag: appended with a step preferred_inner_jobs; \"-j\"->\"-j 4\", \"-j%d\"->\"-j4\", \"--jobs=\"->\"--jobs=4\"\n  \
yaml: --dag also accepts .yaml/.yml (isomorphic to JSON; allows comments + multi-line block-scalar descriptions); the `yaml` subcommand emits YAML\n\n\
{what}\n  \
- concurrent scheduling honoring deps + resource caps, ordered by the chosen --planner\n  \
- learned est_duration / rss from the profile store override the DAG hints at plan time\n    (disable with --no-profile-feedback; inspect with the plan subcommand / --show-plan)\n  \
- a failing step fails the run (exit 1) and, by default, eager-cancels in-flight steps\n    ({keepgoing} lets already-running steps finish instead; it still stops launching new steps)\n  \
- {maxmem} picks the largest -j whose modeled worst-case RAM fits the budget\n\n\
{note}  cgroup-v2 per-step boxing is ON by default; {acf} downgrades to a best-effort\n        unboxed run. {perfdir} writes per-step + whole-run resource-usage CSVs.\n\n\
{exits}  0 = all steps passed | 1 = a step failed | 2 = bad usage / bad DAG file | 3 = cgroup\n           boxing required but unavailable (use {acf})\n",
        banner = banner(c),
        i1 = h("1. Install"),
        i2 = h("2. Write a DAG (JSON)"),
        deps_note = c.dim("- a list of steps; each depends on others by \"group.job\" tag"),
        i3 = h("3. Look at it, then run it"),
        r1 = k(&format!("{PROG} list  --dag dag.json")),
        r2 = k(&format!("{PROG} ascii --dag dag.json")),
        r3 = k(&format!("{PROG} run   --dag dag.json")),
        studies = h("Profile & experiment with individual steps"),
        s1 = k(&format!("{PROG} run   --dag dag.json --profile        # per-step timing/memory table after the run")),
        s2 = k(&format!("{PROG} run   --dag dag.json --only test.unit # run EXACTLY that step (NOT its deps)")),
        s3 = k(&format!("{PROG} sweep --dag dag.json --step build.app --jobs 1..8  # -j1..-j8 speedup table")),
        studies_note = c.dim(
            "--only drops dependency edges to steps outside the selection (inputs assumed present); \
             sweep passes each width into the step via its jobs_flag and reports wall/user/sys/rss + speedup."
        ),
        store = h("Where profiling data lands (by default)"),
        store_dir = k("./.safe-ci-dag-runner/profiles/   (created on demand, relative to CWD)"),
        store_note = c.dim(
            "Every run and sweep AUTO-LOGS resource-usage CSVs here; override with --perf-dir or \
             $SAFE_CI_DAG_RUNNER_PROFILE_DIR, disable with --no-profile. The tool prints where it \
             appended (never silent). Consider gitignoring ./.safe-ci-dag-runner/."
        ),
        planning = h("Smarter planning: learned estimates + the critical-path planner"),
        pl1 = k(&format!(
            "{PROG} plan  --dag dag.json                        # show est (+ source), rss, bottom-level, order"
        )),
        pl2 = k(&format!(
            "{PROG} plan  --dag dag.json --planner critical-path # order by longest remaining path"
        )),
        pl3 = k(&format!(
            "{PROG} run   --dag dag.json --planner critical-path --show-plan  # print the plan, then run it"
        )),
        plan_note = c.dim(
            // Hand-wrapped to ~90 columns (matching the Python quickstart) so the note reads
            // cleanly on a narrow terminal instead of as one long unbroken line. Each `\n  `
            // starts a new 2-space-indented line; the trailing `\` then eats the source newline.
            "The runner FEEDS the profile store back at plan time: a robust est_duration\n  \
             (contention-discounted MEDIAN of past elapsed_s) and an rss estimate (high\n  \
             percentile of past peak_bytes) OVERRIDE the DAG hint once enough samples exist,\n  \
             so est_duration_s need not be hand-authored and --max-mem sizing improves\n  \
             automatically. --planner greedy-lpt (default) launches the longest single step\n  \
             first; critical-path launches the highest bottom-level (longest remaining\n  \
             est-weighted path) first. When the store holds multiple inner-jobs widths for a\n  \
             step, plan/--show-plan also print a parallel-speedup section: the recommended\n  \
             inner_jobs (best wall before the diminishing-returns knee + within the core\n  \
             budget), achieved effective_cores, and the speedup curve.\n  \
             --no-profile-feedback ignores the store (DAG hints only)."
        ),
        schema = h("DAG schema"),
        schema_note = c.dim("(only group/job/cmd are required per step; everything else has defaults)"),
        what = h("What you get"),
        keepgoing = k("--keep-going"),
        maxmem = k("run --max-mem 8G"),
        acf = k("--allow-cgroup-failure"),
        perfdir = k("run --perf-dir DIR"),
        note = h("Note"),
        exits = h("Exit codes"),
    )
}

// --------------------------------------------------------------------------- rendering

fn render_list(cfg: &DagConfig, c: &Palette) -> String {
    if cfg.steps.is_empty() {
        return "(empty DAG)".to_string();
    }
    let width = cfg.steps.iter().map(|s| s.tag().len()).max().unwrap_or(1);
    let mut lines: Vec<String> = Vec::with_capacity(cfg.steps.len());
    for step in &cfg.steps {
        let tag = c.bold(&format!("{:<width$}", step.tag(), width = width));
        let cls = c.yellow(&format!("[{}]", step_classification(step).value()));
        let dep = if step.deps.is_empty() {
            String::new()
        } else {
            c.dim(&format!("  <- {}", step.deps.join(", ")))
        };
        lines.push(format!("{tag}  {cls} {}{dep}", step.desc));
    }
    lines.join("\n")
}

// --------------------------------------------------------------------------- dag loading

fn load(dag_arg: &str) -> Result<DagConfig, LoadError> {
    if dag_arg == "-" {
        // stdin has no filename to auto-detect from: default to JSON.
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| LoadError(format!("{e}")))?;
        return dag_from_json(&buf).map_err(LoadError::from);
    }
    let text = std::fs::read_to_string(Path::new(dag_arg)).map_err(|e| {
        // Match Python's message shape: "[Errno 2] No such file or directory: 'path'".
        LoadError(format!("{e}: '{dag_arg}'"))
    })?;
    // Auto-detect the interchange format by file extension: .yaml/.yml -> YAML, else JSON.
    if is_yaml_path(dag_arg) {
        dag_from_yaml(&text).map_err(LoadError::from)
    } else {
        dag_from_json(&text).map_err(LoadError::from)
    }
}

/// Whether a `--dag` path names a YAML file (case-insensitive `.yaml`/`.yml`).
fn is_yaml_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
}

struct LoadError(String);

impl From<DagJsonError> for LoadError {
    fn from(e: DagJsonError) -> Self {
        LoadError(e.0)
    }
}

// --------------------------------------------------------------------------- run subcommand

struct RunArgs {
    dag: Option<String>,
    jobs: Option<i64>,
    cores: Option<i64>,
    max_mem: Option<String>,
    only: Option<String>,
    perf_dir: Option<String>,
    no_profile: bool,
    profile: bool,
    planner: String,
    show_plan: bool,
    no_profile_feedback: bool,
    profile_sync: Option<String>,
    profile_sync_direction: String,
    keep_going: bool,
    allow_cgroup_failure: bool,
    unsafe_no_cgroups: bool,
    small_default_cap: bool,
    verbosity: i64,
    quiet: bool,
}

fn parse_run_args(rest: &[String]) -> Result<RunArgs, String> {
    let mut a = RunArgs {
        dag: None,
        jobs: None,
        cores: None,
        max_mem: None,
        only: None,
        perf_dir: None,
        no_profile: false,
        profile: false,
        planner: Planner::GreedyLpt.value().to_string(),
        show_plan: false,
        no_profile_feedback: false,
        profile_sync: None,
        profile_sync_direction: "both".to_string(),
        keep_going: false,
        allow_cgroup_failure: false,
        unsafe_no_cgroups: false,
        small_default_cap: false,
        verbosity: 1,
        quiet: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        // Support both `--flag value` and `--flag=value`.
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let take_value = |inline: Option<String>, i: &mut usize| -> Result<String, String> {
            if let Some(v) = inline {
                Ok(v)
            } else {
                *i += 1;
                rest.get(*i)
                    .cloned()
                    .ok_or_else(|| format!("the argument {key} requires a value"))
            }
        };
        match key.as_str() {
            "--dag" => a.dag = Some(take_value(inline, &mut i)?),
            "-j" | "--jobs" => {
                let v = take_value(inline, &mut i)?;
                a.jobs = Some(
                    v.parse::<i64>()
                        .map_err(|_| format!("--jobs: invalid int value: '{v}'"))?,
                );
            }
            "--cores" => {
                let v = take_value(inline, &mut i)?;
                a.cores = Some(
                    v.parse::<i64>()
                        .map_err(|_| format!("--cores: invalid int value: '{v}'"))?,
                );
            }
            "--max-mem" => a.max_mem = Some(take_value(inline, &mut i)?),
            "--only" => a.only = Some(take_value(inline, &mut i)?),
            "--perf-dir" => a.perf_dir = Some(take_value(inline, &mut i)?),
            "--no-profile" => a.no_profile = true,
            "--profile" => a.profile = true,
            "--planner" => a.planner = validate_planner(take_value(inline, &mut i)?)?,
            "--show-plan" => a.show_plan = true,
            "--no-profile-feedback" => a.no_profile_feedback = true,
            "--profile-sync" => a.profile_sync = Some(take_value(inline, &mut i)?),
            "--profile-sync-direction" => {
                let v = take_value(inline, &mut i)?;
                if !matches!(v.as_str(), "both" | "download" | "upload") {
                    return Err(format!(
                        "--profile-sync-direction: invalid value '{v}' (both|download|upload)"
                    ));
                }
                a.profile_sync_direction = v;
            }
            "-k" | "--keep-going" => a.keep_going = true,
            "--cgroups" => return Err(
                "--cgroups has been removed (cgroup-v2 boxing is ON by default); drop the flag. \
                     Per-node resource limits are DAG fields (cpu_timeout, memory, pids)."
                    .to_string(),
            ),
            "--allow-cgroup-failure" => a.allow_cgroup_failure = true,
            "--unsafe-no-cgroups" => a.unsafe_no_cgroups = true,
            "--small-default-cap" => a.small_default_cap = true,
            "-v" => a.verbosity += 1,
            "-q" | "--quiet" => a.quiet = true,
            other => {
                // Handle a bare `-jN` (no space/=).
                if let Some(n) = other.strip_prefix("-j") {
                    a.jobs = Some(
                        n.parse::<i64>()
                            .map_err(|_| format!("--jobs: invalid int value: '{n}'"))?,
                    );
                } else {
                    return Err(format!("unrecognized argument: {other}"));
                }
            }
        }
        i += 1;
    }
    Ok(a)
}

/// Choose the outer scheduler fan-out (`-j`), mirroring Python's `_select_jobs` including the
/// visible stderr notes (both-given, could-not-parse, and the no-throttle note).
fn select_jobs(cfg: &DagConfig, a: &RunArgs) -> i64 {
    let max_mem = a.max_mem.as_deref().filter(|s| !s.is_empty());
    if let Some(jobs) = a.jobs {
        if max_mem.is_some() {
            eprintln!(
                "{PROG}: both --jobs and --max-mem given; --jobs={jobs} wins, --max-mem sizing skipped"
            );
        }
        return jobs;
    }
    if let Some(mm) = max_mem {
        match parse_size(mm) {
            None => {
                eprintln!("{PROG}: could not parse --max-mem '{mm}'; falling back to CPU count");
            }
            Some(budget) => {
                let (jobs, footprint) = jobs_for_budget(cfg, budget);
                eprintln!(
                    "{PROG}: --max-mem {mm} -> -j{jobs} (modeled worst-case {footprint} bytes fits budget {budget} bytes)"
                );
                let ncpu = cpu_count();
                let modeled = cfg
                    .steps
                    .iter()
                    .any(|s| s.hint.rss_baseline_bytes.is_some() && !s.engine_only);
                if jobs == ncpu && !modeled {
                    eprintln!(
                        "{PROG}: note: no step carries rss_baseline_bytes, so the modeled footprint \
collapsed to the mem_cap_floor_bytes floor ({} bytes) and --max-mem did not throttle (-j{jobs} = CPU count); \
add per-step rss_baseline_bytes to enable memory-aware throttling",
                        cfg.mem_cap_floor_bytes
                    );
                }
                return jobs;
            }
        }
    }
    cpu_count()
}

/// Best-effort git SHA of the current working directory's repo (stamps perf-log rows only).
fn git_sha() -> String {
    match std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
    {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "unknown".to_string()
            } else {
                s
            }
        }
        _ => "unknown".to_string(),
    }
}

/// Establish the two-level cgroup-v2 boxing that is this tool's PRIMARY purpose (mirrors the
/// Python `_resolve_cgroup_manager`). Returns the manager to use (`None` = intentional UNBOXED
/// run), or an `Err(exit_code)` the caller must return when boxing is required but unavailable.
/// May re-exec this process into a systemd scope (never returns on success).
fn resolve_cgroups(allow_failure: bool, unsafe_no_cgroups: bool) -> Result<BoxedCgroups, i32> {
    if unsafe_no_cgroups {
        // Deliberate opt-out (--unsafe-no-cgroups): skip scope bring-up entirely and run unboxed
        // even where boxing is available. Distinct from --allow-cgroup-failure (a capability
        // fallback); logged loudly as a reviewable audit signal. Takes precedence over
        // allow_failure when both are set.
        eprintln!(
            "{PROG}: WARNING: DELIBERATELY UNBOXED via --unsafe-no-cgroups: per-step \
             memory/CPU/pids caps are NOT enforced. This is an explicit, reviewable opt-out of \
             cgroup resource boxing (not a capability fallback)."
        );
        return Ok(None);
    }
    if is_in_scope() {
        let mgr = Cgroups::new();
        if mgr.enabled() {
            install_scope_teardown();
            eprintln!(
                "{PROG}: cgroup boxing ACTIVE (two-level cgroup-v2 scope; per-step memory/CPU caps \
                 + setsid-proof teardown)."
            );
            return Ok(Some(Arc::new(mgr) as Arc<dyn CgroupManager>));
        }
        if allow_failure {
            eprintln!(
                "{PROG}: warning: inside a scope but per-step cgroup setup failed; running \
                 best-effort UNBOXED (--allow-cgroup-failure)."
            );
            return Ok(None);
        }
        eprintln!(
            "{PROG}: ERROR: inside a managed scope but per-step cgroups could not be set up; \
             re-run with --allow-cgroup-failure to run UNBOXED."
        );
        return Err(3);
    }
    if allow_failure {
        eprintln!(
            "{PROG}: warning: cgroup boxing not established (--allow-cgroup-failure); running \
             UNBOXED (process-group teardown only, no per-step memory/CPU caps)."
        );
        return Ok(None);
    }
    // Default: boxing is required -> re-exec into a transient systemd --user scope (never returns
    // on success).
    let reexeced_or_skipped = reexec_in_scope(None, None);
    let detail = if reexeced_or_skipped {
        "boxing was skipped (e.g. CI without a systemd --user scope)"
    } else {
        "cgroup-v2 + a working systemd --user scope are unavailable"
    };
    eprintln!(
        "{PROG}: ERROR: cgroup boxing could not be established: {detail}. Cgroup resource boxing is \
         this tool's primary purpose; re-run with --allow-cgroup-failure to run UNBOXED."
    );
    Err(3)
}

// --------------------------------------------------------------------------- profile store

/// Resolve the effective profile-store directory and a label for its source (Feature D).
///
/// Precedence: `--no-profile` disables logging (returns `(None, "disabled")`); otherwise an
/// explicit `--perf-dir` wins; otherwise `$SAFE_CI_DAG_RUNNER_PROFILE_DIR`; otherwise the
/// repo-local default `./.safe-ci-dag-runner/profiles/`. Auto-logging is ON by default.
fn resolve_profile_dir(perf_dir: Option<&str>, no_profile: bool) -> (Option<String>, &'static str) {
    if no_profile {
        return (None, "disabled");
    }
    if let Some(d) = perf_dir {
        if !d.is_empty() {
            return (Some(d.to_string()), "--perf-dir");
        }
    }
    if let Ok(env) = std::env::var(PROFILE_DIR_ENV) {
        if !env.is_empty() {
            return (Some(env), "env");
        }
    }
    (Some(DEFAULT_PROFILE_DIR.to_string()), "default")
}

/// Validate a `--planner` value, returning the canonical string or a usage error.
fn validate_planner(value: String) -> Result<String, String> {
    match Planner::from_value(&value) {
        Some(p) => Ok(p.value().to_string()),
        None => Err(format!(
            "--planner: invalid choice: '{value}' (choose from greedy-lpt, critical-path, cpa)"
        )),
    }
}

/// The directory the plan-time FEEDBACK reader loads the profile store from, or `None` when
/// feedback is off. Independent of `--no-profile` (which governs WRITING). Mirrors Python's
/// `_resolve_feedback_dir`.
fn resolve_feedback_dir(perf_dir: Option<&str>, no_feedback: bool) -> Option<String> {
    if no_feedback {
        return None;
    }
    if let Some(d) = perf_dir {
        if !d.is_empty() {
            return Some(d.to_string());
        }
    }
    if let Ok(env) = std::env::var(PROFILE_DIR_ENV) {
        if !env.is_empty() {
            return Some(env);
        }
    }
    Some(DEFAULT_PROFILE_DIR.to_string())
}

/// Load the profile store (when feedback is on) and build the plan for `planner`. Mirrors Python's
/// `_build_feedback_plan`. `core_budget` (`P`) and `mem_budget` drive the CPA allocator under
/// `--planner cpa` and are ignored by the other planners.
fn build_feedback_plan(
    cfg: &DagConfig,
    feedback_dir: Option<&str>,
    planner: Planner,
    core_budget: Option<i64>,
    mem_budget: Option<i64>,
) -> Plan {
    match feedback_dir {
        Some(dir) => {
            let (mid, cc) = feedback_identity();
            let samples = load_step_samples(Path::new(dir), &mid, &cc);
            let speedups = load_step_speedups(Path::new(dir), &mid, &cc);
            build_plan(
                cfg,
                &samples,
                planner,
                DEFAULT_MIN_SAMPLES,
                &speedups,
                core_budget,
                mem_budget,
            )
        }
        None => build_plan(
            cfg,
            &std::collections::HashMap::new(),
            planner,
            DEFAULT_MIN_SAMPLES,
            &std::collections::HashMap::new(),
            core_budget,
            mem_budget,
        ),
    }
}

/// Build a plan whose learned estimates come from the mergeable SUMMARY (rather than a CSV store) —
/// the reader half of the sync feature. Mirrors Python's `_build_plan_from_summary`.
fn build_plan_from_summary(
    cfg: &DagConfig,
    summary: &Summary,
    planner: Planner,
    core_budget: Option<i64>,
    mem_budget: Option<i64>,
) -> Plan {
    let samples = summary::step_samples_from_summary(summary);
    let speedups = summary::step_speedups_from_summary(summary, None);
    build_plan(
        cfg,
        &samples,
        planner,
        DEFAULT_MIN_SAMPLES,
        &speedups,
        core_budget,
        mem_budget,
    )
}

/// A summary built from THIS machine's local CSV store (its own not-yet-uploaded history), so a
/// persistent box's local runs also seed the planner. Empty when feedback is off / the store is
/// absent. Mirrors Python's `_local_store_summary`.
fn local_store_summary(feedback_dir: Option<&str>, mid: &str, cc: &str) -> Summary {
    match feedback_dir {
        Some(dir) => summary::summary_from_store(Path::new(dir), mid, cc, DEFAULT_RESERVOIR_K),
        None => summary::empty(mid, cc),
    }
}

/// DOWNLOAD the shared summary, MERGE it with this machine's local store, and build the plan from
/// the result. Returns `None` to fall back to the normal CSV-feedback plan (sync off / a backend
/// failure). A backend failure degrades LOUDLY without failing the run. Mirrors `_sync_seed_plan`.
fn sync_seed_plan(
    cfg: &DagConfig,
    backend: Option<&dyn SyncBackend>,
    feedback_dir: Option<&str>,
    planner: Planner,
    do_download: bool,
    core_budget: Option<i64>,
    mem_budget: Option<i64>,
) -> Option<Plan> {
    let backend = backend?;
    if !do_download {
        return None;
    }
    let (mid, cc) = feedback_identity();
    let downloaded = match backend.download(&mid, &cc) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{PROG}: --profile-sync: download failed, planning from local store only ({e})"
            );
            return None;
        }
    };
    let local = local_store_summary(feedback_dir, &mid, &cc);
    let seed = match summary::merge(
        &downloaded,
        &local,
        DEFAULT_RESERVOIR_K,
        DEFAULT_MAX_BUCKETS,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{PROG}: --profile-sync: {e}; planning from local store only");
            return None;
        }
    };
    let (buckets, total, _largest) = summary::summary_stats(&seed);
    eprintln!(
        "{PROG}: --profile-sync: seeded planner from {} ({buckets} buckets, {total} samples for {mid}/{cc})",
        backend.describe()
    );
    Some(build_plan_from_summary(
        cfg,
        &seed,
        planner,
        core_budget,
        mem_budget,
    ))
}

/// Merge THIS run's per-step samples into the shared summary and publish them. Degrades LOUDLY on
/// failure (a warning) so the run's exit code is preserved but the skip is never silent. Mirrors
/// Python's `_sync_upload`.
fn sync_upload(backend: &dyn SyncBackend, rows: &[std::collections::BTreeMap<String, String>]) {
    let (mid, cc) = feedback_identity();
    let hash_rows: Vec<std::collections::HashMap<String, String>> = rows
        .iter()
        .map(|r| r.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .collect();
    let affinity = crate::estimates::affinity_width(&cc);
    let delta = summary::summary_from_rows(
        &hash_rows,
        &mid,
        &cc,
        affinity,
        DEFAULT_RESERVOIR_K,
        DEFAULT_MAX_BUCKETS,
    );
    match backend.publish(&delta, DEFAULT_RESERVOIR_K, DEFAULT_MAX_BUCKETS) {
        Ok(merged) => {
            let (buckets, total, largest) = summary::summary_stats(&merged);
            eprintln!(
                "{PROG}: --profile-sync: published this run's samples to {} (summary now {buckets} buckets, {total} samples, <= {largest}/bucket)",
                backend.describe()
            );
        }
        Err(e) => eprintln!("{PROG}: --profile-sync: upload failed ({e})"),
    }
}

/// Resolve the `(core_budget, mem_budget)` the CPA allocator balances against, or `(None, None)`
/// for the non-allocating planners (so they do no cgroup/proc reads). Mirrors Python's
/// `_cpa_budgets`.
fn cpa_budgets(planner: Planner, max_mem: Option<&str>) -> (Option<i64>, Option<i64>) {
    if planner != Planner::Cpa {
        return (None, None);
    }
    let mem_budget = max_mem.filter(|s| !s.is_empty()).and_then(parse_size);
    (Some(container_core_budget()), mem_budget)
}

/// Print one visible line naming where profile CSVs were appended (No Silent Failure).
///
/// Lists EXACTLY the files this run/sweep wrote (the deterministic `store_paths` set, filtered to
/// files that exist), not a glob of the whole store — a persistent store also holds prior runs'
/// other-`container_class` CSVs for the same machine, and globbing would over-report files this run
/// never touched.
fn report_profile_written(perf_dir: &str, source: &str) {
    let mut csvs: Vec<String> = crate::perflog::store_paths(Path::new(perf_dir))
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();
    csvs.sort();
    if csvs.is_empty() {
        eprintln!("{PROG}: WARNING no profile CSVs were written under {perf_dir}");
        return;
    }
    if source == "--perf-dir" {
        eprintln!("{PROG}: perf CSVs written under {perf_dir}:");
    } else {
        let origin = match source {
            "default" => "default profile store".to_string(),
            "env" => format!("profile store (${PROFILE_DIR_ENV})"),
            other => format!("profile store ({other})"),
        };
        eprintln!(
            "{PROG}: profile data appended to the {origin} at {perf_dir} (override with --perf-dir \
             or ${PROFILE_DIR_ENV}; disable with --no-profile):"
        );
    }
    for path in csvs {
        eprintln!("  {path}");
    }
}

// --------------------------------------------------------------------------- --only selection

/// Split a comma-separated `group.job` tag list, dropping empty entries.
fn parse_tag_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Return a DAG containing EXACTLY the named steps (Feature A).
///
/// Dependency edges to steps OUTSIDE the selection are dropped (their outputs are assumed
/// present); edges among selected steps are preserved so a selected sub-graph still runs in the
/// right order. Registration order is preserved, matching the Python build. `Err` on unknown tag.
fn filter_only(cfg: &DagConfig, tags: &[String]) -> Result<DagConfig, String> {
    let by_tag: HashSet<String> = cfg.steps.iter().map(|s| s.tag()).collect();
    let unknown: Vec<String> = tags
        .iter()
        .filter(|t| !by_tag.contains(*t))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        let mut known: Vec<String> = by_tag.into_iter().collect();
        known.sort();
        let known_s = if known.is_empty() {
            "(none)".to_string()
        } else {
            known.join(", ")
        };
        return Err(format!(
            "--only: unknown step tag(s): {}. Known tags: {known_s}",
            unknown.join(", ")
        ));
    }
    let selected: HashSet<String> = tags.iter().cloned().collect();
    let mut new_cfg = cfg.clone();
    new_cfg.steps = cfg
        .steps
        .iter()
        .filter(|s| selected.contains(&s.tag()))
        .map(|s| {
            let mut step = s.clone();
            step.deps.retain(|d| selected.contains(d));
            step
        })
        .collect();
    Ok(new_cfg)
}

// --------------------------------------------------------------------------- table rendering

/// Human-readable byte count (e.g. `3.5 GiB`); `-` when unknown.
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

/// Render a fixed-width table: the first column left-aligned, the rest right-aligned.
fn render_table(headers: &[String], rows: &[Vec<String>], c: &Palette) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let fmt_row = |cells: &[String]| -> String {
        let mut parts: Vec<String> = Vec::with_capacity(cols);
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
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    let mut out = vec![c.bold(&fmt_row(headers)), c.dim(&sep.join("  "))];
    for row in rows {
        out.push(fmt_row(row));
    }
    out.join("\n")
}

fn cell_secs(value: Option<&String>) -> String {
    match value.and_then(|s| s.parse::<f64>().ok()) {
        Some(v) => format!("{v:.3}"),
        None => "-".to_string(),
    }
}

fn cell_secs_from_usec(value: Option<&String>) -> String {
    match value.and_then(|s| s.parse::<i64>().ok()) {
        Some(v) => format!("{:.3}", v as f64 / 1e6),
        None => "-".to_string(),
    }
}

fn cell_bytes(value: Option<&String>) -> String {
    match value.and_then(|s| s.parse::<i64>().ok()) {
        Some(v) => human_bytes(Some(v)),
        None => "-".to_string(),
    }
}

/// Print the per-step profile table (Feature C) to stdout.
///
/// Columns: step | wall_s | user_s | sys_s | rss_hwm | oom | inner_jobs. `user_s`/`sys_s` come
/// from the per-step cgroup `cpu.stat` (present only under boxing) and `rss_hwm` from the step
/// cgroup `memory.peak`; each shows `-` when unavailable (an un-boxed run).
fn print_profile_table(rows: &[BTreeMap<String, String>], c: &Palette) {
    if rows.is_empty() {
        eprintln!("{PROG}: no per-step profile rows to show (nothing ran)");
        return;
    }
    let headers: Vec<String> = [
        "step",
        "wall_s",
        "user_s",
        "sys_s",
        "rss_hwm",
        "oom",
        "inner_jobs",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut table: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row in rows {
        table.push(vec![
            row.get("step").cloned().unwrap_or_else(|| "?".to_string()),
            cell_secs(row.get("elapsed_s")),
            cell_secs_from_usec(row.get("cpu.user_usec")),
            cell_secs_from_usec(row.get("cpu.system_usec")),
            cell_bytes(row.get("peak_bytes")),
            row.get("oom_kills")
                .cloned()
                .unwrap_or_else(|| "0".to_string()),
            row.get("inner_jobs")
                .cloned()
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }
    println!("{}", c.bold("per-step profile:"));
    println!("{}", render_table(&headers, &table, c));
}

// --------------------------------------------------------------------------- sweep

/// One measured single-step run at a given inner-parallelism width.
#[derive(Clone, Copy)]
struct SweepMeasure {
    wall_s: f64,
    user_s: f64,
    sys_s: f64,
    rss_hwm: Option<i64>,
    ok: bool,
}

/// Parse a sweep width range: `"LO..HI"` or a bare `"N"` (meaning `1..N`).
fn parse_jobs_range(raw: &str) -> Result<(i64, i64), String> {
    let text = raw.trim();
    let (lo, hi) = match text.split_once("..") {
        Some((a, b)) => {
            let lo = a
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("invalid --jobs range '{raw}': not an integer"))?;
            let hi = b
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("invalid --jobs range '{raw}': not an integer"))?;
            (lo, hi)
        }
        None => {
            let n = text
                .parse::<i64>()
                .map_err(|_| format!("invalid --jobs '{raw}': not an integer"))?;
            (1, n)
        }
    };
    if lo < 1 || hi < lo {
        return Err(format!("invalid --jobs range '{raw}': need 1 <= LO <= HI"));
    }
    Ok((lo, hi))
}

/// Run ONE step at a fixed inner-parallelism width and measure it (see [`cmd_sweep`]).
#[allow(clippy::too_many_arguments)]
fn run_single_step(
    base: &Step,
    cfg: &DagConfig,
    inner_jobs: i64,
    cgroups: &BoxedCgroups,
    perf_dir: Option<&str>,
    git: &str,
    verbosity: i64,
) -> SweepMeasure {
    let mut step = base.clone();
    step.deps.clear();
    step.hint.preferred_inner_jobs = Some(inner_jobs);
    let mut one = cfg.clone();
    one.steps = vec![step];

    let (u0, s0) = child_cpu_seconds();
    let window = perf_dir.map(|d| PerfWindow::start(Path::new(d), git));
    let start = Instant::now();
    let result = run_dag_boxed(&one, 1, false, verbosity, cgroups.clone());
    let measured = start.elapsed().as_secs_f64();
    let (u1, s1) = child_cpu_seconds();
    if let Some(w) = &window {
        w.finish(
            if result.ok { "pass" } else { "fail" },
            result.outcomes.len(),
            inner_jobs,
        );
    }
    if let Some(d) = perf_dir {
        append_step_profiles(
            Path::new(d),
            &result.step_profile_rows,
            git,
            1,
            None,
            "unverified",
            "local",
        );
    }
    let row = result.step_profile_rows.first();
    let wall_s = row
        .and_then(|r| r.get("elapsed_s"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(measured);
    let rss_hwm = row
        .and_then(|r| r.get("peak_bytes"))
        .and_then(|s| s.parse::<i64>().ok());
    SweepMeasure {
        wall_s,
        user_s: (u1 - u0).max(0.0),
        sys_s: (s1 - s0).max(0.0),
        rss_hwm,
        ok: result.ok,
    }
}

/// Print the parallel-speedup sweep table (Feature B) to stdout.
fn print_sweep_table(
    step: &str,
    baseline_jobs: i64,
    measures: &[(i64, SweepMeasure)],
    c: &Palette,
) {
    let baseline_wall = measures.first().map(|(_, m)| m.wall_s).unwrap_or(0.0);
    let headers: Vec<String> = vec![
        "jobs".to_string(),
        "wall_s".to_string(),
        "user_s".to_string(),
        "sys_s".to_string(),
        "rss_hwm".to_string(),
        format!("speedup(vs j{baseline_jobs})"),
    ];
    let mut table: Vec<Vec<String>> = Vec::with_capacity(measures.len());
    for (jobs, m) in measures {
        let speedup = if m.wall_s > 0.0 {
            format!("{:.2}x", baseline_wall / m.wall_s)
        } else {
            "-".to_string()
        };
        table.push(vec![
            jobs.to_string(),
            format!("{:.3}", m.wall_s),
            format!("{:.3}", m.user_s),
            format!("{:.3}", m.sys_s),
            human_bytes(m.rss_hwm),
            speedup,
        ]);
    }
    println!("{}", c.bold(&format!("parallel-speedup sweep: {step}")));
    println!("{}", render_table(&headers, &table, c));
}

struct SweepArgs {
    dag: Option<String>,
    step: Option<String>,
    jobs: Option<String>,
    repeat: i64,
    perf_dir: Option<String>,
    no_profile: bool,
    allow_cgroup_failure: bool,
    unsafe_no_cgroups: bool,
    verbosity: i64,
}

fn parse_sweep_args(rest: &[String]) -> Result<SweepArgs, String> {
    let mut a = SweepArgs {
        dag: None,
        step: None,
        jobs: None,
        repeat: 1,
        perf_dir: None,
        no_profile: false,
        allow_cgroup_failure: false,
        unsafe_no_cgroups: false,
        verbosity: 0,
    };
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let take_value = |inline: Option<String>, i: &mut usize| -> Result<String, String> {
            if let Some(v) = inline {
                Ok(v)
            } else {
                *i += 1;
                rest.get(*i)
                    .cloned()
                    .ok_or_else(|| format!("the argument {key} requires a value"))
            }
        };
        match key.as_str() {
            "--dag" => a.dag = Some(take_value(inline, &mut i)?),
            "--step" => a.step = Some(take_value(inline, &mut i)?),
            "--jobs" => a.jobs = Some(take_value(inline, &mut i)?),
            "--repeat" => {
                let v = take_value(inline, &mut i)?;
                a.repeat = v
                    .parse::<i64>()
                    .map_err(|_| format!("--repeat: invalid int value: '{v}'"))?;
            }
            "--perf-dir" => a.perf_dir = Some(take_value(inline, &mut i)?),
            "--no-profile" => a.no_profile = true,
            "--allow-cgroup-failure" => a.allow_cgroup_failure = true,
            "--unsafe-no-cgroups" => a.unsafe_no_cgroups = true,
            "-v" => a.verbosity += 1,
            other => return Err(format!("unrecognized argument: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

/// Per-step parallel-speedup sweep (Feature B).
fn cmd_sweep(a: &SweepArgs, c: &Palette) -> i32 {
    let dag_arg = match &a.dag {
        Some(d) => d.clone(),
        None => {
            eprintln!("{PROG} sweep: error: the following arguments are required: --dag");
            return 2;
        }
    };
    let step_tag = match &a.step {
        Some(s) => s.clone(),
        None => {
            eprintln!("{PROG} sweep: error: the following arguments are required: --step");
            return 2;
        }
    };
    let jobs_spec = match &a.jobs {
        Some(j) => j.clone(),
        None => {
            eprintln!("{PROG} sweep: error: the following arguments are required: --jobs");
            return 2;
        }
    };
    let cfg = match load(&dag_arg) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{PROG}: {}", e.0);
            return 2;
        }
    };
    let by_tag: HashSet<String> = cfg.steps.iter().map(|s| s.tag()).collect();
    if !by_tag.contains(&step_tag) {
        let mut known: Vec<String> = by_tag.into_iter().collect();
        known.sort();
        let known_s = if known.is_empty() {
            "(none)".to_string()
        } else {
            known.join(", ")
        };
        eprintln!("{PROG}: sweep: unknown --step tag '{step_tag}'. Known tags: {known_s}");
        return 2;
    }
    let (lo, hi) = match parse_jobs_range(&jobs_spec) {
        Ok(range) => range,
        Err(e) => {
            eprintln!("{PROG}: sweep: {e}");
            return 2;
        }
    };
    let repeat = a.repeat.max(1);

    // Cgroup boxing is ON by default here too (so the sweep measures under real boxing).
    let cgroups = match resolve_cgroups(a.allow_cgroup_failure, a.unsafe_no_cgroups) {
        Ok(cg) => cg,
        Err(code) => return code,
    };
    let (perf_dir, source) = resolve_profile_dir(a.perf_dir.as_deref(), a.no_profile);
    let git = git_sha();
    let base = cfg
        .steps
        .iter()
        .find(|s| s.tag() == step_tag)
        .expect("tag presence checked above")
        .clone();

    let mut measures: Vec<(i64, SweepMeasure)> = Vec::new();
    for jobs in lo..=hi {
        let mut best: Option<SweepMeasure> = None;
        for _ in 0..repeat {
            let m = run_single_step(
                &base,
                &cfg,
                jobs,
                &cgroups,
                perf_dir.as_deref(),
                &git,
                a.verbosity,
            );
            if !m.ok {
                eprintln!(
                    "{PROG}: sweep: step '{step_tag}' FAILED at -j{jobs}; aborting the sweep"
                );
                return 1;
            }
            best = Some(match best {
                Some(b) if b.wall_s <= m.wall_s => b,
                _ => m,
            });
        }
        measures.push((jobs, best.expect("repeat >= 1")));
    }

    print_sweep_table(&step_tag, lo, &measures, c);
    if let Some(d) = perf_dir.as_deref() {
        report_profile_written(d, source);
    }
    0
}

// --------------------------------------------------------------------------- plan subcommand

struct PlanArgs {
    dag: Option<String>,
    planner: String,
    format: String,
    perf_dir: Option<String>,
    no_profile_feedback: bool,
    max_mem: Option<String>,
}

fn parse_plan_args(rest: &[String]) -> Result<PlanArgs, String> {
    let mut a = PlanArgs {
        dag: None,
        planner: Planner::GreedyLpt.value().to_string(),
        format: "text".to_string(),
        perf_dir: None,
        no_profile_feedback: false,
        max_mem: None,
    };
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let take_value = |inline: Option<String>, i: &mut usize| -> Result<String, String> {
            if let Some(v) = inline {
                Ok(v)
            } else {
                *i += 1;
                rest.get(*i)
                    .cloned()
                    .ok_or_else(|| format!("the argument {key} requires a value"))
            }
        };
        match key.as_str() {
            "--dag" => a.dag = Some(take_value(inline, &mut i)?),
            "--planner" => a.planner = validate_planner(take_value(inline, &mut i)?)?,
            "--format" => {
                let v = take_value(inline, &mut i)?;
                if v != "text" && v != "json" {
                    return Err(format!(
                        "--format: invalid choice: '{v}' (choose from text, json)"
                    ));
                }
                a.format = v;
            }
            "--perf-dir" => a.perf_dir = Some(take_value(inline, &mut i)?),
            "--no-profile-feedback" => a.no_profile_feedback = true,
            "--max-mem" => a.max_mem = Some(take_value(inline, &mut i)?),
            other => return Err(format!("unrecognized argument: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

/// Show the plan (per-step estimates + sources, critical path, scheduled order) without running.
fn cmd_plan(a: &PlanArgs) -> i32 {
    let dag_arg = match &a.dag {
        Some(d) => d.clone(),
        None => {
            eprintln!("{PROG} plan: error: the following arguments are required: --dag");
            return 2;
        }
    };
    let cfg = match load(&dag_arg) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{PROG}: {}", e.0);
            return 2;
        }
    };
    let planner = Planner::from_value(&a.planner).unwrap_or(Planner::GreedyLpt);
    let feedback_dir = resolve_feedback_dir(a.perf_dir.as_deref(), a.no_profile_feedback);
    let (core_budget, mem_budget) = cpa_budgets(planner, a.max_mem.as_deref());
    let plan = build_feedback_plan(
        &cfg,
        feedback_dir.as_deref(),
        planner,
        core_budget,
        mem_budget,
    );
    if a.format == "json" {
        println!("{}", plan_to_json(&plan));
    } else {
        print!("{}", plan_to_text(&plan));
    }
    0
}

fn cmd_run(cfg: &DagConfig, a: &RunArgs, c: &Palette) -> i32 {
    // Feature A: --only runs EXACTLY the named step(s). Validate/filter BEFORE cgroup bring-up so
    // a bad tag fails fast (exit 2) without needing a systemd scope.
    let filtered = if let Some(only) = a.only.as_deref() {
        let tags = parse_tag_list(only);
        if tags.is_empty() {
            eprintln!("{PROG}: run: --only requires at least one tag");
            return 2;
        }
        match filter_only(cfg, &tags) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("{PROG}: {e}");
                return 2;
            }
        }
    } else {
        None
    };
    let cfg: &DagConfig = filtered.as_ref().unwrap_or(cfg);

    // Cgroup boxing is ON by default (may re-exec into a systemd scope and not return).
    let cgroups = match resolve_cgroups(a.allow_cgroup_failure, a.unsafe_no_cgroups) {
        Ok(cg) => cg,
        Err(code) => return code,
    };

    // Opt-in --cores K: constrain the WHOLE run tree to K least-busy free cores. Apply it HERE,
    // after the boxing re-exec has settled (the re-exec'd in-scope child re-enters cmd_run and
    // applies it there) and BEFORE the scheduler spawns any worker thread or forks any step —
    // threads inherit the creator's affinity and forked steps inherit at fork, so an early
    // application covers the whole descendant tree (cgroup cpuset where delegated, else
    // sched_setaffinity).
    if let Some(k) = a.cores {
        apply_core_box(k);
    }

    // Plan-time profile-store FEEDBACK (ds-7pzdgm / ds-afzsqf): refine each step's est_duration_s
    // and rss_baseline_bytes from the recorded store, then pick the dispatch order for --planner.
    // The applied cfg (refined hints) feeds both the memory-aware -j sizing and the scheduler.
    let planner = Planner::from_value(&a.planner).unwrap_or(Planner::GreedyLpt);
    let feedback_dir = resolve_feedback_dir(a.perf_dir.as_deref(), a.no_profile_feedback);
    let (core_budget, mem_budget) = cpa_budgets(planner, a.max_mem.as_deref());

    // Profile-artifact SYNC: parse the backend once; DOWNLOAD seeds the planner, UPLOAD (after the
    // run) publishes this run's samples. A malformed spec fails fast; a backend I/O failure degrades
    // LOUDLY without failing the run.
    let backend: Option<Box<dyn SyncBackend>> = match a.profile_sync.as_deref() {
        Some(spec) => match sync::parse_backend(spec) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("{PROG}: --profile-sync: {e}");
                return 2;
            }
        },
        None => None,
    };
    let do_download =
        backend.is_some() && matches!(a.profile_sync_direction.as_str(), "both" | "download");
    let do_upload =
        backend.is_some() && matches!(a.profile_sync_direction.as_str(), "both" | "upload");

    let plan = sync_seed_plan(
        cfg,
        backend.as_deref(),
        feedback_dir.as_deref(),
        planner,
        do_download && !a.no_profile_feedback,
        core_budget,
        mem_budget,
    )
    .unwrap_or_else(|| {
        build_feedback_plan(
            cfg,
            feedback_dir.as_deref(),
            planner,
            core_budget,
            mem_budget,
        )
    });
    let mut applied = apply_plan_to_config(cfg, &plan);
    // Opt-in --small-default-cap: turn ON the SMALL forcing-function caps for THIS run only. They
    // are OFF by default so an active cap never wedges a concurrent validate on the shared checkout;
    // this flag supplies the 1-core / 1-GiB / 10-s floor to steps that DECLARE NOTHING (an explicit
    // per-step hint still wins via the effective_* helpers). Announce it so its use is visible in logs.
    if a.small_default_cap {
        applied.default_step_mem_cap_bytes = Some(crate::model::DEFAULT_SMALL_MEM_CAP_BYTES);
        applied.default_step_cpu_count = Some(crate::model::DEFAULT_SMALL_CPU_COUNT);
        applied.default_step_cpu_timeout = crate::model::DEFAULT_SMALL_CPU_TIMEOUT;
        eprintln!(
            "{PROG}: --small-default-cap: undeclared steps boxed to the SMALL default floor \
             (mem {} B / {} core / {} s CPU); declared per-step hints still win",
            crate::model::DEFAULT_SMALL_MEM_CAP_BYTES,
            crate::model::DEFAULT_SMALL_CPU_COUNT,
            crate::model::DEFAULT_SMALL_CPU_TIMEOUT,
        );
    }
    let cfg: &DagConfig = &applied;
    if a.show_plan {
        print!("{}", plan_to_text(&plan));
    }

    let jobs = select_jobs(cfg, a);
    let verbosity = if a.quiet { 0 } else { a.verbosity };

    let (perf_dir, source) = resolve_profile_dir(a.perf_dir.as_deref(), a.no_profile);
    let git = git_sha();
    let window = perf_dir
        .as_deref()
        .map(|d| PerfWindow::start(Path::new(d), &git));

    let result = run_dag_boxed_ordered(
        cfg,
        jobs,
        a.keep_going,
        verbosity,
        cgroups,
        Some(plan.order.clone()),
        core_budget,
    );
    let passed = result.outcomes.iter().filter(|o| o.ok).count();
    let failed = result
        .outcomes
        .iter()
        .filter(|o| !o.ok && !o.aborted)
        .count();
    let aborted = result.outcomes.iter().filter(|o| o.aborted).count();
    let verdict = if result.ok {
        c.green("PASS")
    } else {
        c.red("FAIL")
    };
    eprintln!(
        "{PROG}: {verdict} - {passed} passed, {failed} failed, {aborted} aborted, {} skipped in {:.1}s",
        result.skipped.len(),
        result.wall_s
    );

    if let Some(d) = perf_dir.as_deref() {
        if let Some(w) = &window {
            w.finish(
                if result.ok { "pass" } else { "fail" },
                result.outcomes.len(),
                jobs,
            );
        }
        append_step_profiles(
            Path::new(d),
            &result.step_profile_rows,
            &git,
            jobs,
            None,
            "unverified",
            "local",
        );
        report_profile_written(d, source);
    }

    if do_upload {
        if let Some(b) = backend.as_deref() {
            sync_upload(b, &result.step_profile_rows);
        }
    }

    // Feature C: --profile prints a per-step profile table to stdout.
    if a.profile {
        print_profile_table(&result.step_profile_rows, c);
    }

    if result.ok {
        0
    } else {
        1
    }
}

// --------------------------------------------------------------------------- entry

/// Run the CLI over `argv` (excluding the program name); returns the process exit code.
pub fn run(argv: &[String]) -> i32 {
    let c = Palette {
        enabled: color_enabled(),
    };

    if argv.is_empty() {
        print!("{}", help_text(&c));
        return 0;
    }

    let command = argv[0].as_str();
    let rest = &argv[1..];

    match command {
        "--version" => {
            println!("{PROG} {VERSION}");
            0
        }
        "-h" | "--help" => {
            print!("{}", help_text(&c));
            0
        }
        "quickstart" => {
            println!("{}", quickstart(&c));
            0
        }
        "capabilities" => {
            // Machine-readable enforcement manifest; byte-identical to the Python build and
            // cross-checked, so an enforcement guard in one build but not the other fails `cross`.
            println!("{ENFORCEMENT_CAPABILITIES}");
            0
        }
        "--userguide" => {
            // Write the embedded guide VERBATIM (no added/stripped newline) so it is byte-identical
            // to the Python build's --userguide and to the single source guide.
            print!("{USERGUIDE}");
            0
        }
        "run" => {
            let a = match parse_run_args(rest) {
                Ok(a) => a,
                Err(msg) => {
                    eprintln!("{PROG} run: error: {msg}");
                    return 2;
                }
            };
            let dag_arg = match &a.dag {
                Some(d) => d.clone(),
                None => {
                    eprintln!("{PROG} run: error: the following arguments are required: --dag");
                    return 2;
                }
            };
            let cfg = match load(&dag_arg) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("{PROG}: {}", e.0);
                    return 2;
                }
            };
            cmd_run(&cfg, &a, &c)
        }
        "sweep" => {
            let a = match parse_sweep_args(rest) {
                Ok(a) => a,
                Err(msg) => {
                    eprintln!("{PROG} sweep: error: {msg}");
                    return 2;
                }
            };
            cmd_sweep(&a, &c)
        }
        "plan" => {
            let a = match parse_plan_args(rest) {
                Ok(a) => a,
                Err(msg) => {
                    eprintln!("{PROG} plan: error: {msg}");
                    return 2;
                }
            };
            cmd_plan(&a)
        }
        "summary" => cmd_summary(rest),
        "list" | "ascii" | "dot" | "json" | "yaml" => {
            let dag_arg = match parse_simple_dag(rest) {
                Ok(Some(d)) => d,
                Ok(None) => {
                    eprintln!(
                        "{PROG} {command}: error: the following arguments are required: --dag"
                    );
                    return 2;
                }
                Err(msg) => {
                    eprintln!("{PROG} {command}: error: {msg}");
                    return 2;
                }
            };
            let cfg = match load(&dag_arg) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("{PROG}: {}", e.0);
                    return 2;
                }
            };
            match command {
                "list" => println!("{}", render_list(&cfg, &c)),
                "ascii" => print!("{}", to_ascii(&cfg, None)),
                "dot" => print!("{}", to_dot(&cfg, "dag", None)),
                "json" => println!("{}", dag_to_json(&cfg)),
                // dag_to_yaml already ends with a trailing newline.
                "yaml" => print!("{}", dag_to_yaml(&cfg)),
                _ => unreachable!(),
            }
            0
        }
        other => {
            eprintln!(
                "usage: {PROG} [-h] [--version] <command> ...\n\
                 {PROG}: error: argument <command>: invalid choice: '{other}' \
                 (choose from run, sweep, plan, summary, list, ascii, dot, json, yaml, quickstart, capabilities)"
            );
            2
        }
    }
}

// --------------------------------------------------------------------------- summary subcommand

/// The `summary` subcommand family: build / merge / plan / stats the mergeable profile summary.
/// These primitives make the summary format inspectable and drivable from a script, and are what the
/// cross-language differential exercises for byte-identical serialization + merge. Mirrors Python's
/// `_cmd_summary`.
fn cmd_summary(rest: &[String]) -> i32 {
    let action = match rest.first() {
        Some(a) => a.as_str(),
        None => {
            eprintln!("{PROG}: summary: an action is required (build | merge | plan | stats)");
            return 2;
        }
    };
    let args = &rest[1..];
    match action {
        "build" => cmd_summary_build(args),
        "merge" => cmd_summary_merge(args),
        "plan" => cmd_summary_plan(args),
        "stats" => cmd_summary_stats(args),
        other => {
            eprintln!("{PROG}: summary: unknown action '{other}' (build | merge | plan | stats)");
            2
        }
    }
}

/// Parse `--key value` / `--key=value` flags plus positional args into (flags, positionals).
fn parse_flags(
    rest: &[String],
    value_keys: &[&str],
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut flags: HashMap<String, String> = HashMap::new();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        if let Some(key) = arg.strip_prefix("--") {
            let (k, inline) = match key.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (key.to_string(), None),
            };
            if value_keys.contains(&k.as_str()) {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        rest.get(i)
                            .cloned()
                            .ok_or_else(|| format!("the argument --{k} requires a value"))?
                    }
                };
                flags.insert(k, v);
            } else {
                return Err(format!("unrecognized argument: --{k}"));
            }
        } else {
            positional.push(arg.clone());
        }
        i += 1;
    }
    Ok((flags, positional))
}

fn write_or_print(text: &str, out: Option<&String>) -> i32 {
    match out {
        Some(path) => match std::fs::write(path, format!("{text}\n")) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{PROG}: summary: cannot write {path}: {e}");
                2
            }
        },
        None => {
            println!("{text}");
            0
        }
    }
}

fn cmd_summary_build(args: &[String]) -> i32 {
    let (flags, _pos) = match parse_flags(args, &["perf-dir", "out", "reservoir-cap"]) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{PROG} summary build: error: {e}");
            return 2;
        }
    };
    let cap = flags
        .get("reservoir-cap")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_RESERVOIR_K);
    let feedback_dir = resolve_feedback_dir(flags.get("perf-dir").map(|s| s.as_str()), false);
    let (mid, cc) = feedback_identity();
    let summary = match feedback_dir {
        Some(dir) => summary::summary_from_store(Path::new(&dir), &mid, &cc, cap),
        None => summary::empty(&mid, &cc),
    };
    write_or_print(&summary::to_json(&summary), flags.get("out"))
}

fn cmd_summary_merge(args: &[String]) -> i32 {
    let (flags, files) = match parse_flags(args, &["out", "reservoir-cap"]) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{PROG} summary merge: error: {e}");
            return 2;
        }
    };
    if files.is_empty() {
        eprintln!("{PROG}: summary merge: need at least one file");
        return 2;
    }
    let cap = flags
        .get("reservoir-cap")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_RESERVOIR_K);
    let mut summaries: Vec<Summary> = Vec::with_capacity(files.len());
    for file in &files {
        match std::fs::read_to_string(file) {
            Ok(text) => match summary::from_json(&text) {
                Ok(s) => summaries.push(s),
                Err(e) => {
                    eprintln!("{PROG}: summary merge: {file}: {e}");
                    return 2;
                }
            },
            Err(e) => {
                eprintln!("{PROG}: summary merge: cannot read {file}: {e}");
                return 2;
            }
        }
    }
    let first = &summaries[0];
    match summary::merge_all(
        &summaries,
        &first.machine_id,
        &first.container_class,
        cap,
        DEFAULT_MAX_BUCKETS,
    ) {
        Ok(merged) => write_or_print(&summary::to_json(&merged), flags.get("out")),
        Err(e) => {
            eprintln!("{PROG}: summary merge: {e}");
            2
        }
    }
}

fn cmd_summary_plan(args: &[String]) -> i32 {
    let (flags, _pos) = match parse_flags(args, &["summary", "dag", "planner", "max-mem", "format"])
    {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{PROG} summary plan: error: {e}");
            return 2;
        }
    };
    let summary_file = match flags.get("summary") {
        Some(f) => f,
        None => {
            eprintln!("{PROG} summary plan: error: --summary is required");
            return 2;
        }
    };
    let dag_file = match flags.get("dag") {
        Some(f) => f,
        None => {
            eprintln!("{PROG} summary plan: error: --dag is required");
            return 2;
        }
    };
    let summary = match std::fs::read_to_string(summary_file) {
        Ok(text) => match summary::from_json(&text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{PROG}: summary plan: {e}");
                return 2;
            }
        },
        Err(e) => {
            eprintln!("{PROG}: summary plan: cannot read {summary_file}: {e}");
            return 2;
        }
    };
    let cfg = match load(dag_file) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{PROG}: {}", e.0);
            return 2;
        }
    };
    let planner = flags
        .get("planner")
        .and_then(|p| Planner::from_value(p))
        .unwrap_or(Planner::GreedyLpt);
    let (core_budget, mem_budget) = cpa_budgets(planner, flags.get("max-mem").map(|s| s.as_str()));
    let plan = build_plan_from_summary(&cfg, &summary, planner, core_budget, mem_budget);
    if flags.get("format").map(|s| s.as_str()) == Some("json") {
        println!("{}", plan_to_json(&plan));
    } else {
        print!("{}", plan_to_text(&plan));
    }
    0
}

fn cmd_summary_stats(args: &[String]) -> i32 {
    let (_flags, files) = match parse_flags(args, &[]) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{PROG} summary stats: error: {e}");
            return 2;
        }
    };
    let file = match files.first() {
        Some(f) => f,
        None => {
            eprintln!("{PROG} summary stats: error: a summary FILE is required");
            return 2;
        }
    };
    let summary = match std::fs::read_to_string(file) {
        Ok(text) => match summary::from_json(&text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{PROG}: summary stats: {e}");
                return 2;
            }
        },
        Err(e) => {
            eprintln!("{PROG}: summary stats: cannot read {file}: {e}");
            return 2;
        }
    };
    let (buckets, total, largest) = summary::summary_stats(&summary);
    println!(
        "identity: {}/{}\nbuckets: {buckets}\ntotal_samples: {total}\nmax_bucket_samples: {largest}",
        summary.machine_id, summary.container_class
    );
    0
}

/// Parse the `--dag FILE` argument for the read-only subcommands.
fn parse_simple_dag(rest: &[String]) -> Result<Option<String>, String> {
    let mut dag: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        match arg.split_once('=') {
            Some(("--dag", v)) => dag = Some(v.to_string()),
            _ if arg == "--dag" => {
                i += 1;
                dag = Some(
                    rest.get(i)
                        .cloned()
                        .ok_or_else(|| "the argument --dag requires a value".to_string())?,
                );
            }
            _ => return Err(format!("unrecognized argument: {arg}")),
        }
        i += 1;
    }
    Ok(dag)
}
