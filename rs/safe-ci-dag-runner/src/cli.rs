//! Command-line interface for safe-ci-dag-runner.
//!
//! Subcommands (matching `py/safe_ci_dag_runner/cli.py`):
//!   run --dag FILE    run a DAG (exit 0 iff every step passes)
//!   list --dag FILE   list the steps
//!   ascii --dag FILE  draw the DAG as ASCII art
//!   dot --dag FILE    emit Graphviz DOT
//!   json --dag FILE   re-emit the DAG as canonical JSON
//!   quickstart        print a self-contained getting-started guide
//!
//! `list`, `ascii`, `dot`, and `json` stdout is BYTE-IDENTICAL to the Python build; `--help`
//! and `quickstart` wording may differ but the structure and exit codes (0 / 1 / 2 / 3) match.
//!
//! Cgroup boxing is ON by default (this tool's primary purpose): `run` re-execs inside a
//! transient `systemd-run --user --scope` and caps each step in its own child cgroup. When
//! cgroup-v2 + a working systemd `--user` scope are unavailable the run ERRORS (exit 3);
//! `--allow-cgroup-failure` downgrades to a best-effort UNBOXED run with a visible warning.
//! `--perf-dir DIR` writes per-step + whole-run resource-usage CSVs.

use std::io::{IsTerminal, Read};
use std::path::Path;
use std::sync::Arc;

use crate::cgroup::{install_scope_teardown, is_in_scope, reexec_in_scope, CgroupManager, Cgroups};
use crate::io::{dag_from_json, dag_to_json, DagJsonError};
use crate::model::{step_classification, DagConfig};
use crate::perflog::{append_step_profiles, PerfWindow};
use crate::scheduler::{run_dag_boxed, BoxedCgroups};
use crate::sizing::{cpu_count, jobs_for_budget, parse_size};
use crate::viz::{to_ascii, to_dot};
use crate::{PROG, VERSION};

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
         \x20 list       list the steps\n\
         \x20 ascii      draw the DAG as ASCII art\n\
         \x20 dot        emit Graphviz DOT (pipe to `dot -Tsvg`)\n\
         \x20 json       re-emit the DAG as canonical JSON\n\
         \x20 quickstart print a self-contained getting-started guide\n\n\
         {examples}\n\
         \x20 {e1}\n\
         \x20 {e2}\n\
         \x20 {e3}\n",
        banner = banner(c),
        usage = c.bold("usage"),
        commands = c.bold("commands"),
        examples = c.bold("examples"),
        e1 = ex(&format!("{PROG} quickstart")),
        e2 = ex(&format!("{PROG} run --dag dag.json")),
        e3 = ex(&format!("{PROG} ascii --dag dag.json")),
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
{schema}  {schema_note}\n  \
step:   group, job, desc, cmd, deps[], env{{}}, timeout, jobs_flag, networkonly, engine_only, hint{{}}\n  \
hint:   resources{{name:int}}, est_duration_s, rss_baseline_bytes, hard_mem_max_bytes,\n          classification(\"cpu-bound\"|\"latency-bound\"|\"light\"), preferred_inner_jobs\n  \
top:    resource_caps{{name:int}}, mem_cap_factor, mem_cap_floor_bytes,\n          outer_mem_safety_factor, default_step_timeout, default_jobs_flag\n  \
jobs_flag: appended with a step preferred_inner_jobs; \"-j\"->\"-j 4\", \"-j%d\"->\"-j4\", \"--jobs=\"->\"--jobs=4\"\n\n\
{what}\n  \
- concurrent scheduling in longest-first order, honoring deps + resource caps\n  \
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
    let text = if dag_arg == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| LoadError(format!("{e}")))?;
        buf
    } else {
        std::fs::read_to_string(Path::new(dag_arg)).map_err(|e| {
            // Match Python's message shape: "[Errno 2] No such file or directory: 'path'".
            LoadError(format!("{e}: '{dag_arg}'"))
        })?
    };
    dag_from_json(&text).map_err(LoadError::from)
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
    max_mem: Option<String>,
    perf_dir: Option<String>,
    keep_going: bool,
    cgroups: bool,
    allow_cgroup_failure: bool,
    verbosity: i64,
    quiet: bool,
}

fn parse_run_args(rest: &[String]) -> Result<RunArgs, String> {
    let mut a = RunArgs {
        dag: None,
        jobs: None,
        max_mem: None,
        perf_dir: None,
        keep_going: false,
        cgroups: false,
        allow_cgroup_failure: false,
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
            "--max-mem" => a.max_mem = Some(take_value(inline, &mut i)?),
            "--perf-dir" => a.perf_dir = Some(take_value(inline, &mut i)?),
            "-k" | "--keep-going" => a.keep_going = true,
            "--cgroups" => a.cgroups = true,
            "--allow-cgroup-failure" => a.allow_cgroup_failure = true,
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
fn resolve_cgroups(allow_failure: bool) -> Result<BoxedCgroups, i32> {
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

fn cmd_run(cfg: &DagConfig, a: &RunArgs, c: &Palette) -> i32 {
    // Cgroup boxing is ON by default (may re-exec into a systemd scope and not return).
    let cgroups = match resolve_cgroups(a.allow_cgroup_failure) {
        Ok(cg) => cg,
        Err(code) => return code,
    };
    if a.cgroups {
        eprintln!("{PROG}: note: --cgroups is a deprecated no-op (boxing is ON by default)");
    }

    let jobs = select_jobs(cfg, a);
    let verbosity = if a.quiet { 0 } else { a.verbosity };

    let perf_dir = a.perf_dir.as_deref().filter(|s| !s.is_empty());
    let git = git_sha();
    let window = perf_dir.map(|d| PerfWindow::start(Path::new(d), &git));

    let result = run_dag_boxed(cfg, jobs, a.keep_going, verbosity, cgroups);
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

    if let Some(d) = perf_dir {
        if let Some(w) = &window {
            w.finish(
                if result.ok { "pass" } else { "fail" },
                result.outcomes.len(),
                jobs,
            );
        }
        let loc = append_step_profiles(
            Path::new(d),
            &result.step_profile_rows,
            &git,
            jobs,
            None,
            "unverified",
            "local",
        );
        match loc {
            Some(p) => eprintln!("{PROG}: perf CSVs written under {d} (e.g. {})", p.display()),
            None => eprintln!("{PROG}: WARNING no perf CSVs were written under {d}"),
        }
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
        "list" | "ascii" | "dot" | "json" => {
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
                _ => unreachable!(),
            }
            0
        }
        other => {
            eprintln!(
                "usage: {PROG} [-h] [--version] <command> ...\n\
                 {PROG}: error: argument <command>: invalid choice: '{other}' \
                 (choose from run, list, ascii, dot, json, quickstart)"
            );
            2
        }
    }
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
