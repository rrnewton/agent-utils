"""Command-line interface for safe-ci-dag-runner.

Subcommands:
  run --dag FILE    run a DAG under two-level cgroup-v2 boxing (exit 0 iff every step passes;
                    boxing is ON by default — pass --allow-cgroup-failure to run un-boxed)
  list --dag FILE   list the steps
  ascii --dag FILE  draw the DAG as ASCII art
  dot --dag FILE    emit Graphviz DOT (pipe to `dot -Tsvg`)
  json --dag FILE   re-emit the DAG as canonical JSON
  yaml --dag FILE   re-emit the DAG as YAML
  quickstart        print a self-contained getting-started guide

A DAG is a JSON or YAML file (see `quickstart` for the schema + a runnable example). `--dag`
auto-detects the format by extension: `.yaml`/`.yml` load as YAML (isomorphic to the JSON schema,
and additionally allow comments + multi-line block-scalar descriptions), everything else as JSON.
Pass `--dag -` to read a JSON DAG from stdin.
"""

from __future__ import annotations

import argparse
import dataclasses
import os
import resource
import subprocess
import sys
import time
from collections.abc import Mapping, Sequence
from pathlib import Path

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.estimates import (
    Plan,
    Planner,
    apply_plan_to_config,
    build_plan,
    feedback_identity,
    load_step_samples,
    load_step_speedups,
    plan_to_json,
    plan_to_text,
)
from safe_ci_dag_runner.io import (
    DagJsonError,
    dag_from_json,
    dag_from_yaml,
    dag_to_json,
    dag_to_yaml,
)
from safe_ci_dag_runner.model import DagConfig, Step, step_classification
from safe_ci_dag_runner.protocols import CgroupManager, MetricsSink
from safe_ci_dag_runner.scheduler import run_dag
from safe_ci_dag_runner.sizing import jobs_for_budget, parse_size
from safe_ci_dag_runner.viz import to_ascii, to_dot

PROG = "safe-ci-dag-runner"

#: Environment variable overriding the default profile-store location (Feature D). An explicit
#: ``--perf-dir`` still wins over this; ``--no-profile`` disables logging entirely.
PROFILE_DIR_ENV = "SAFE_CI_DAG_RUNNER_PROFILE_DIR"

#: Default profile-store directory, RELATIVE TO THE CURRENT WORKING DIRECTORY, used when neither
#: ``--perf-dir`` nor ``$SAFE_CI_DAG_RUNNER_PROFILE_DIR`` is set and ``--no-profile`` is absent.
#: Created on demand. Runs (and sweeps) auto-append here so profiling data lands somewhere obvious
#: and browsable without any flag.
DEFAULT_PROFILE_DIR = os.path.join(".safe-ci-dag-runner", "profiles")


# --------------------------------------------------------------------------- colors
class Palette:
    """Minimal ANSI colorizer (stdlib only). Disabled for non-ttys and when NO_COLOR is set."""

    def __init__(self, enabled: bool) -> None:
        self.enabled = enabled

    def _wrap(self, code: str, text: str) -> str:
        return f"\033[{code}m{text}\033[0m" if self.enabled else text

    def bold(self, text: str) -> str:
        return self._wrap("1", text)

    def dim(self, text: str) -> str:
        return self._wrap("2", text)

    def green(self, text: str) -> str:
        return self._wrap("32", text)

    def red(self, text: str) -> str:
        return self._wrap("1;31", text)

    def yellow(self, text: str) -> str:
        return self._wrap("33", text)

    def cyan(self, text: str) -> str:
        return self._wrap("36", text)


def _color_enabled(stream: object) -> bool:
    if os.environ.get("NO_COLOR"):
        return False
    isatty = getattr(stream, "isatty", None)
    return bool(callable(isatty) and isatty())


def _banner(c: Palette) -> str:
    return (
        f"{c.bold(PROG)} {c.dim('v' + __version__)}\n"
        "Run a DAG of build/test steps concurrently and safely: two-level cgroup CPU/memory\n"
        "boxing with zombie-free teardown, memory-aware concurrency, always-on resource\n"
        "logging, and DAG visualization."
    )


def _epilog(c: Palette) -> str:
    ex = c.cyan
    return (
        f"{c.bold('examples')}\n"
        f"  {ex(f'{PROG} quickstart')}                       {c.dim('# get started (schema + runnable demo)')}\n"
        f"  {ex(f'{PROG} run --dag dag.json')}               {c.dim('# run it; exit 0 iff all steps pass')}\n"
        f"  {ex(f'{PROG} run --dag dag.json --profile')}     {c.dim('# ...and print a per-step profile table')}\n"
        f"  {ex(f'{PROG} run --dag dag.json --only build.app')} {c.dim('# run EXACTLY one step (not its deps)')}\n"
        f"  {ex(f'{PROG} plan --dag dag.json --planner critical-path')} {c.dim('# show learned estimates + the plan')}\n"
        f"  {ex(f'{PROG} sweep --dag dag.json --step build.app --jobs 1..8')} {c.dim('# parallel-speedup study')}\n"
        f"  {ex(f'{PROG} ascii --dag dag.json')}             {c.dim('# quick ASCII view of the graph')}\n"
        f"  {ex(f'{PROG} dot --dag dag.json | dot -Tsvg -o dag.svg')}\n\n"
        f"{c.dim('Profiling data auto-logs to ./.safe-ci-dag-runner/profiles/ by default')}\n"
        f"{c.dim(f'(override with --perf-dir or ${PROFILE_DIR_ENV}; disable with --no-profile).')}"
    )


# --------------------------------------------------------------------------- quickstart
def _quickstart(c: Palette) -> str:
    h = c.bold
    k = c.cyan
    return f"""{_banner(c)}

{h('1. Install')}
  pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"

{h('2. Write a DAG (JSON)')}  {c.dim('- a list of steps; each depends on others by "group.job" tag')}
  Save as dag.json:
  {{
    "resource_caps": {{"browser": 1}},
    "steps": [
      {{"group": "build", "job": "app", "desc": "compile", "cmd": "echo build && sleep 0.2"}},
      {{"group": "test",  "job": "unit", "desc": "unit tests", "cmd": "echo test && sleep 0.2",
        "deps": ["build.app"]}},
      {{"group": "e2e",   "job": "smoke", "desc": "browser smoke", "cmd": "echo e2e && sleep 0.2",
        "deps": ["build.app"], "hint": {{"resources": {{"browser": 1}}}}}}
    ]
  }}

{h('3. Look at it, then run it')}
  {k(f'{PROG} list  --dag dag.json')}
  {k(f'{PROG} ascii --dag dag.json')}
  {k(f'{PROG} run   --dag dag.json')}        {c.dim('# concurrent, dependency-ordered; exit 0 = all passed')}

{h('Profile & experiment with individual steps')}
  {k(f'{PROG} run   --dag dag.json --profile')}          {c.dim('# print a per-step timing/memory table after the run')}
  {k(f'{PROG} run   --dag dag.json --only test.unit')}   {c.dim('# run EXACTLY that step (NOT its deps; inputs assumed present)')}
  {k(f'{PROG} run   --dag dag.json --only build.app,test.unit')}  {c.dim('# one or more, comma-separated')}
  {k(f'{PROG} sweep --dag dag.json --step build.app --jobs 1..8')}  {c.dim('# run one step at -j1..-j8, print a speedup table')}
  {c.dim('--only drops dependency edges to steps outside the selection (their outputs are')}
  {c.dim('assumed already present); it is for profiling/experimenting on a step in isolation.')}
  {c.dim('sweep passes each width into the step via its jobs_flag and reports wall/user/sys/rss + speedup.')}

{h('Where profiling data lands (by default)')}
  {c.dim('Every run and sweep AUTO-LOGS resource-usage CSVs to a repo-local store:')}
    {k('./.safe-ci-dag-runner/profiles/')}   {c.dim('(created on demand, relative to CWD)')}
  {c.dim(f'Override with {k("--perf-dir DIR")} or ${PROFILE_DIR_ENV}; turn it off with {k("--no-profile")}.')}
  {c.dim('The tool prints exactly where it appended, so the logging is never silent. Consider')}
  {c.dim('gitignoring ./.safe-ci-dag-runner/ (or check it in to keep a history - project choice).')}

{h('Smarter planning: learned estimates + the critical-path planner')}
  {c.dim('The runner FEEDS the recorded store back in at plan time: for each step it derives a')}
  {c.dim('robust est_duration (contention-discounted MEDIAN of past elapsed_s) and an rss estimate')}
  {c.dim('(a high percentile of past peak_bytes), and USES them in place of the DAG-authored hint')}
  {c.dim('when the store has enough samples. So est_duration_s no longer has to be hand-authored -')}
  {c.dim('planning improves automatically as runs accumulate. This also feeds --max-mem sizing.')}
  {k(f'{PROG} plan  --dag dag.json')}                       {c.dim('# show the plan (est + source, rss, bottom-level, order)')}
  {k(f'{PROG} plan  --dag dag.json --planner critical-path')} {c.dim('# order by longest remaining path, not just single est')}
  {k(f'{PROG} plan  --dag dag.json --format json')}          {c.dim('# canonical machine-readable plan (for a CI-optimizing agent)')}
  {k(f'{PROG} run   --dag dag.json --planner critical-path --show-plan')} {c.dim('# print the plan, then run in that order')}
  {c.dim('--planner greedy-lpt (default) launches the longest single step first; critical-path')}
  {c.dim('launches the step with the highest bottom-level (longest remaining est-weighted path).')}
  {c.dim('When the store holds multiple inner-jobs widths for a step, plan/--show-plan also print a')}
  {c.dim('parallel-speedup section: the recommended inner_jobs (best wall before the diminishing-')}
  {c.dim('returns knee + within the core budget), achieved effective_cores, and the speedup curve.')}
  {c.dim(f'Use {k("--no-profile-feedback")} to ignore the store and plan from the DAG hints only.')}

{h('DAG schema')}  {c.dim('(only group/job/cmd are required per step; everything else has defaults)')}
  step:   group, job, desc, description, cmd, deps[], env{{}}, timeout, jobs_flag, networkonly, engine_only, hint{{}}
  hint:   resources{{name:int}}, est_duration_s, rss_baseline_bytes, hard_mem_max_bytes,
          classification("cpu-bound"|"latency-bound"|"light"), preferred_inner_jobs
  top:    description, resource_caps{{name:int}}, mem_cap_factor, mem_cap_floor_bytes,
          outer_mem_safety_factor, default_step_timeout, default_jobs_flag
  {c.dim('desc = short label; description = long-form docs (often multi-line, great in YAML).')}
  {c.dim('YAML: --dag also accepts .yaml/.yml (isomorphic to JSON; allows comments + block-scalar')}
  {c.dim('  descriptions). The yaml subcommand emits YAML; json emits canonical JSON.')}
  {c.dim('resource_caps bound concurrent demand - e.g. {"browser":1} serializes browser steps.')}
  {c.dim('jobs_flag: template appended with a step preferred_inner_jobs, e.g. "-j" -> "-j 4",')}
  {c.dim('  "-j%d" -> "-j4", "--jobs=" -> "--jobs=4", "--num-threads" -> "--num-threads 4".')}

{h('What you get')}
  - concurrent scheduling honoring deps + resource caps, ordered by the chosen {k('--planner')}
    {c.dim('(greedy-lpt = longest single step first; critical-path = longest remaining path first)')}
  - learned est_duration / rss from the profile store override the DAG hints at plan time
    {c.dim('(disable with --no-profile-feedback; inspect with the plan subcommand / --show-plan)')}
  - a failing step fails the run (exit 1) and, by default, eager-cancels in-flight steps
    ({k('--keep-going')} lets already-running steps finish instead; it still stops launching new steps)
  - Linux cgroup-v2 per-step memory/CPU boxing is ON BY DEFAULT (the tool's primary purpose):
    {k('run')} re-execs inside a systemd --user scope and caps each step in its own child cgroup
    {c.dim('(no cgroup-v2 + systemd --user scope? the run errors — pass')} {k('--allow-cgroup-failure')} {c.dim('to run un-boxed)')}
  - {k('run --max-mem 8G')} picks the largest -j whose modeled worst-case RAM fits the budget
    {c.dim('(--jobs, when given, overrides this)')}
  - {k('run --perf-dir DIR')} writes per-step + whole-run resource-usage CSVs into DIR

{h('Python API')}  {c.dim('(same engine, in code)')}
  from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag, to_ascii
  cfg = DagConfig(steps=(Step("build","app","compile","echo build && sleep 0.1"),))
  print(to_ascii(cfg)); result = run_dag(cfg, jobs=4)   # result.ok, result.outcomes

{h('Exit codes')}  0 = all steps passed | 1 = a step failed | 2 = bad usage / bad DAG file
             | 3 = cgroup boxing required but unavailable (use {k('--allow-cgroup-failure')})
"""


# --------------------------------------------------------------------------- rendering
def _render_list(cfg: DagConfig, c: Palette) -> str:
    if not cfg.steps:
        return "(empty DAG)"
    width = max(len(s.tag) for s in cfg.steps)
    lines: list[str] = []
    for step in cfg.steps:
        tag = c.bold(f"{step.tag:<{width}}")  # pad first, then color (keeps alignment)
        cls = c.yellow(f"[{step_classification(step).value}]")
        dep = c.dim("  <- " + ", ".join(step.deps)) if step.deps else ""
        lines.append(f"{tag}  {cls} {step.desc}{dep}")
    return "\n".join(lines)


# --------------------------------------------------------------------------- parser
class _ColorHelp(argparse.RawDescriptionHelpFormatter):
    """Raw formatter so our colored description/epilog render verbatim."""


def build_parser() -> argparse.ArgumentParser:
    c = Palette(_color_enabled(sys.stdout))
    parser = argparse.ArgumentParser(
        prog=PROG,
        formatter_class=_ColorHelp,
        description=_banner(c),
        epilog=_epilog(c),
    )
    parser.add_argument("--version", action="version", version=f"{PROG} {__version__}")
    sub = parser.add_subparsers(dest="command", metavar="<command>")

    run_p = sub.add_parser("run", help="run a DAG (exit 0 iff every step passes)")
    run_p.add_argument(
        "--dag",
        required=True,
        metavar="FILE",
        help="DAG file ('-' = stdin); .yaml/.yml load as YAML, else JSON",
    )
    run_p.add_argument("-j", "--jobs", type=int, default=None, help="max concurrent steps (default: CPU count)")
    run_p.add_argument(
        "--max-mem",
        metavar="SPEC",
        default=None,
        help="RAM budget (e.g. 8G, 4096M); pick the largest -j whose modeled worst-case "
        "footprint fits (ignored when --jobs is given)",
    )
    run_p.add_argument(
        "--only",
        metavar="TAG[,TAG...]",
        default=None,
        help="run EXACTLY the named step(s) and nothing else (comma-separated 'group.job' tags). "
        "Dependency edges to steps OUTSIDE the selection are dropped (their outputs are assumed "
        "already present) - this is for profiling/experimenting on a step in isolation; it does "
        "NOT run the step's dependencies. Errors if a named tag does not exist.",
    )
    run_p.add_argument(
        "--perf-dir",
        metavar="DIR",
        default=None,
        help="write per-step + whole-run resource-usage CSVs into DIR (overrides the default "
        f"profile store and ${PROFILE_DIR_ENV})",
    )
    run_p.add_argument(
        "--no-profile",
        action="store_true",
        help="disable the default auto-logging profile store (do not append profile CSVs anywhere)",
    )
    run_p.add_argument(
        "--profile",
        action="store_true",
        help="after the run, print a per-step profile table (step | wall_s | user_s | sys_s | "
        "rss_hwm | oom | inner_jobs) to the terminal",
    )
    run_p.add_argument(
        "--planner",
        choices=[p.value for p in Planner],
        default=Planner.GREEDY_LPT.value,
        help="dispatch-ordering planner: 'greedy-lpt' (default; longest est_duration first) or "
        "'critical-path' (longest remaining est-weighted path first, i.e. highest bottom-level)",
    )
    run_p.add_argument(
        "--show-plan",
        action="store_true",
        help="before running, print the plan (per-step est_duration + source, rss_estimate, "
        "bottom_level, the critical path, and the scheduled order)",
    )
    run_p.add_argument(
        "--no-profile-feedback",
        action="store_true",
        help="do NOT read the profile store to refine est_duration_s / rss_baseline_bytes at plan "
        "time; use only the DAG-authored hints (for reproducibility)",
    )
    run_p.add_argument(
        "-k",
        "--keep-going",
        action="store_true",
        help="on a failure, let already-running steps finish instead of eager-cancelling them (still stops launching new steps)",
    )
    run_p.add_argument(
        "--cgroups",
        action="store_true",
        help="(deprecated no-op; cgroup-v2 boxing is now ON by default) accepted for compatibility",
    )
    run_p.add_argument(
        "--allow-cgroup-failure",
        action="store_true",
        help="downgrade to a best-effort UNBOXED run (with a visible warning) instead of erroring "
        "when two-level cgroup-v2 + systemd --user scope boxing cannot be established",
    )
    run_p.add_argument("-v", dest="verbosity", action="count", default=1, help="-v: stream child output")
    run_p.add_argument("-q", "--quiet", action="store_true", help="quieter output")

    sweep_p = sub.add_parser(
        "sweep",
        help="parallel-speedup sweep: run ONE step at inner -j1..-jN and print a timing table",
    )
    sweep_p.add_argument(
        "--dag",
        required=True,
        metavar="FILE",
        help="DAG file ('-' = stdin); .yaml/.yml load as YAML, else JSON",
    )
    sweep_p.add_argument(
        "--step",
        required=True,
        metavar="TAG",
        help="the single 'group.job' step to sweep (must exist in the DAG)",
    )
    sweep_p.add_argument(
        "--jobs",
        required=True,
        metavar="RANGE",
        help="inner-parallelism widths to sweep: 'LO..HI' (e.g. 1..8) or a bare 'N' meaning 1..N. "
        "Each width is passed into the step command via its jobs_flag mechanism.",
    )
    sweep_p.add_argument(
        "--repeat",
        type=int,
        default=1,
        metavar="K",
        help="run each width K times and keep the fastest wall time (default: 1)",
    )
    sweep_p.add_argument(
        "--perf-dir",
        metavar="DIR",
        default=None,
        help="append sweep profile CSVs into DIR (overrides the default profile store and "
        f"${PROFILE_DIR_ENV})",
    )
    sweep_p.add_argument(
        "--no-profile",
        action="store_true",
        help="disable the default auto-logging profile store for this sweep",
    )
    sweep_p.add_argument(
        "--allow-cgroup-failure",
        action="store_true",
        help="run UNBOXED (with a warning) instead of erroring when cgroup-v2 boxing is unavailable",
    )
    sweep_p.add_argument("-v", dest="verbosity", action="count", default=0, help="-v: stream child output")

    plan_p = sub.add_parser(
        "plan",
        help="show the plan: per-step est_duration (+ source), rss_estimate, bottom-level, the "
        "critical path, and the scheduled order (does NOT run anything)",
    )
    plan_p.add_argument(
        "--dag",
        required=True,
        metavar="FILE",
        help="DAG file ('-' = stdin); .yaml/.yml load as YAML, else JSON",
    )
    plan_p.add_argument(
        "--planner",
        choices=[p.value for p in Planner],
        default=Planner.GREEDY_LPT.value,
        help="dispatch-ordering planner: 'greedy-lpt' (default) or 'critical-path'",
    )
    plan_p.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="output format: 'text' (human table, default) or 'json' (canonical machine-readable)",
    )
    plan_p.add_argument(
        "--perf-dir",
        metavar="DIR",
        default=None,
        help="read the profile store from DIR (overrides the default store and "
        f"${PROFILE_DIR_ENV}) when deriving learned estimates",
    )
    plan_p.add_argument(
        "--no-profile-feedback",
        action="store_true",
        help="ignore the profile store; show only the DAG-authored hints",
    )

    for cmd, helptext in (
        ("list", "list the steps"),
        ("ascii", "draw the DAG as ASCII art"),
        ("dot", "emit Graphviz DOT (pipe to `dot -Tsvg`)"),
        ("json", "re-emit the DAG as canonical JSON"),
        ("yaml", "re-emit the DAG as YAML"),
    ):
        sp = sub.add_parser(cmd, help=helptext)
        sp.add_argument(
            "--dag",
            required=True,
            metavar="FILE",
            help="DAG file ('-' = stdin); .yaml/.yml load as YAML, else JSON",
        )

    sub.add_parser("quickstart", help="print a self-contained getting-started guide")
    return parser


def _load(dag_arg: str) -> DagConfig:
    if dag_arg == "-":
        # stdin has no filename to auto-detect from: default to JSON.
        return dag_from_json(sys.stdin.read())
    path = Path(dag_arg)
    text = path.read_text(encoding="utf-8")
    # Auto-detect the interchange format by extension: .yaml/.yml -> YAML, else JSON.
    if path.suffix.lower() in (".yaml", ".yml"):
        return dag_from_yaml(text)
    return dag_from_json(text)


def _git_sha() -> str:
    """Best-effort git SHA of the current working directory's repo, or ``"unknown"``.

    Used only to stamp perf-log rows (``--perf-dir``); it must never fail the run, so any
    error (not a repo, git absent, timeout) degrades to ``"unknown"``."""
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    if proc.returncode != 0:
        return "unknown"
    return proc.stdout.strip() or "unknown"


# --------------------------------------------------------------------------- profile store
def _resolve_profile_dir(
    perf_dir_arg: str | None, no_profile: bool
) -> tuple[str | None, str]:
    """Resolve the effective profile-store directory and a human label for its source.

    Precedence (Feature D): ``--no-profile`` disables logging entirely (returns ``(None, ...)``);
    otherwise an explicit ``--perf-dir`` wins; otherwise ``$SAFE_CI_DAG_RUNNER_PROFILE_DIR``;
    otherwise the repo-local default ``./.safe-ci-dag-runner/profiles/``. Auto-logging is ON by
    default so profiling data lands somewhere obvious without any flag; the caller MUST surface
    where it was appended (No Silent Failure)."""
    if no_profile:
        return None, "disabled"
    if perf_dir_arg:
        return perf_dir_arg, "--perf-dir"
    env = os.environ.get(PROFILE_DIR_ENV)
    if env:
        return env, f"${PROFILE_DIR_ENV}"
    return DEFAULT_PROFILE_DIR, "default"


def _resolve_feedback_dir(perf_dir_arg: str | None, no_feedback: bool) -> str | None:
    """The directory the plan-time FEEDBACK reader loads the profile store from, or ``None`` when
    feedback is off.

    Independent of ``--no-profile`` (which only governs WRITING): reading the store to refine
    estimates is a separate concern. Precedence mirrors the write path minus the disable:
    ``--no-profile-feedback`` turns it off; else ``--perf-dir``; else
    ``$SAFE_CI_DAG_RUNNER_PROFILE_DIR``; else the repo-local default store."""
    if no_feedback:
        return None
    if perf_dir_arg:
        return perf_dir_arg
    env = os.environ.get(PROFILE_DIR_ENV)
    if env:
        return env
    return DEFAULT_PROFILE_DIR


def _build_feedback_plan(
    cfg: DagConfig, feedback_dir: str | None, planner: Planner
) -> Plan:
    """Load the profile store (when feedback is on) and build the plan for ``planner``.

    With ``feedback_dir`` set, the store's learned estimates refine each step (store wins when it
    has enough samples; the DAG hint is the fallback) and the per-step parallel-speedup curves are
    attached for the plan display. With ``feedback_dir`` ``None`` the plan reflects the DAG hints
    only."""
    if feedback_dir is not None:
        machine_id, container_class = feedback_identity()
        samples = load_step_samples(feedback_dir, machine_id, container_class)
        speedups = load_step_speedups(feedback_dir, machine_id, container_class)
    else:
        samples = {}
        speedups = {}
    return build_plan(cfg, samples, planner=planner, speedups=speedups)


def _report_profile_written(perf_dir: str, source: str) -> None:
    """Print one visible line naming where profile CSVs were appended (No Silent Failure).

    Lists EXACTLY the files this run/sweep wrote (the deterministic :func:`store_paths` set,
    filtered to files that exist), not a glob of the whole store — a persistent store also holds
    prior runs' other-``container_class`` CSVs for the same machine, and globbing would over-report
    files this run never touched."""
    from safe_ci_dag_runner.perflog import store_paths

    written = sorted(str(p) for p in store_paths(perf_dir) if p.exists())
    if not written:
        print(f"{PROG}: WARNING no profile CSVs were written under {perf_dir}", file=sys.stderr)
        return
    if source == "--perf-dir":
        print(f"{PROG}: perf CSVs written under {perf_dir}:", file=sys.stderr)
    else:
        origin = "default profile store" if source == "default" else f"profile store ({source})"
        print(
            f"{PROG}: profile data appended to the {origin} at {perf_dir} "
            f"(override with --perf-dir or ${PROFILE_DIR_ENV}; disable with --no-profile):",
            file=sys.stderr,
        )
    for path in written:
        print(f"  {path}", file=sys.stderr)


# --------------------------------------------------------------------------- --only selection
class _OnlyError(Exception):
    """A ``--only`` / ``--step`` selection referenced a tag that does not exist."""


def _parse_tag_list(raw: str) -> list[str]:
    """Split a comma-separated ``group.job`` tag list, dropping empty entries."""
    return [tag.strip() for tag in raw.split(",") if tag.strip()]


def _filter_only(cfg: DagConfig, tags: Sequence[str]) -> DagConfig:
    """Return a DAG containing EXACTLY the named steps (Feature A).

    Dependency edges pointing OUTSIDE the selection are dropped (their outputs are assumed
    present); edges among selected steps are preserved so a selected sub-graph still runs in the
    right order. Registration order is preserved, so both language builds filter identically.
    Raises :class:`_OnlyError` if any tag is unknown."""
    by_tag = cfg.by_tag()
    unknown = [tag for tag in tags if tag not in by_tag]
    if unknown:
        known = ", ".join(sorted(by_tag)) or "(none)"
        raise _OnlyError(
            f"--only: unknown step tag(s): {', '.join(unknown)}. Known tags: {known}"
        )
    selected = set(tags)
    new_steps = tuple(
        dataclasses.replace(step, deps=[d for d in step.deps if d in selected])
        for step in cfg.steps
        if step.tag in selected
    )
    return dataclasses.replace(cfg, steps=new_steps)


# --------------------------------------------------------------------------- table rendering
def _human_bytes(n: int | None) -> str:
    """Human-readable byte count (e.g. ``3.5 GiB``); ``"-"`` when unknown."""
    if n is None:
        return "-"
    value = float(n)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            return f"{int(value)} B" if unit == "B" else f"{value:.1f} {unit}"
        value /= 1024.0
    return f"{n} B"


def _render_table(headers: Sequence[str], rows: Sequence[Sequence[str]], c: Palette) -> str:
    """Render a fixed-width table: the first column left-aligned, the rest right-aligned."""
    cols = len(headers)
    widths = [len(h) for h in headers]
    for row in rows:
        for i in range(cols):
            widths[i] = max(widths[i], len(row[i]))

    def fmt(cells: Sequence[str]) -> str:
        return "  ".join(
            f"{cells[i]:<{widths[i]}}" if i == 0 else f"{cells[i]:>{widths[i]}}"
            for i in range(cols)
        )

    lines = [c.bold(fmt(headers)), c.dim("  ".join("-" * w for w in widths))]
    lines.extend(fmt(row) for row in rows)
    return "\n".join(lines)


def _cell_secs(value: object) -> str:
    return f"{float(value):.3f}" if isinstance(value, (int, float)) else "-"


def _cell_secs_from_usec(value: object) -> str:
    return f"{value / 1e6:.3f}" if isinstance(value, int) else "-"


def _cell_bytes(value: object) -> str:
    return _human_bytes(value) if isinstance(value, int) else "-"


def _print_profile_table(rows: Sequence[Mapping[str, object]], c: Palette) -> None:
    """Print the per-step profile table (Feature C) to stdout.

    Columns: step | wall_s | user_s | sys_s | rss_hwm | oom | inner_jobs. ``user_s``/``sys_s``
    come from the per-step cgroup ``cpu.stat`` (present only under boxing) and ``rss_hwm`` from
    the step cgroup ``memory.peak``; each shows ``-`` when unavailable (an un-boxed run)."""
    if not rows:
        print(f"{PROG}: no per-step profile rows to show (nothing ran)", file=sys.stderr)
        return
    headers = ["step", "wall_s", "user_s", "sys_s", "rss_hwm", "oom", "inner_jobs"]
    table: list[list[str]] = []
    for row in rows:
        table.append(
            [
                str(row.get("step", "?")),
                _cell_secs(row.get("elapsed_s")),
                _cell_secs_from_usec(row.get("cpu.user_usec")),
                _cell_secs_from_usec(row.get("cpu.system_usec")),
                _cell_bytes(row.get("peak_bytes")),
                str(row.get("oom_kills", 0)),
                str(row.get("inner_jobs", "-")),
            ]
        )
    print(c.bold("per-step profile:"))
    print(_render_table(headers, table, c))


# --------------------------------------------------------------------------- sweep
@dataclasses.dataclass(frozen=True)
class _SweepMeasure:
    """One measured single-step run at a given inner-parallelism width."""

    wall_s: float
    user_s: float
    sys_s: float
    rss_hwm: int | None
    ok: bool


def _parse_jobs_range(raw: str) -> tuple[int, int]:
    """Parse a sweep width range: ``"LO..HI"`` or a bare ``"N"`` (meaning ``1..N``).

    Returns ``(lo, hi)`` with ``1 <= lo <= hi``. Raises :class:`ValueError` on any malformed or
    out-of-order range so the caller can report a clear usage error."""
    text = raw.strip()
    if ".." in text:
        lo_s, _, hi_s = text.partition("..")
        try:
            lo, hi = int(lo_s), int(hi_s)
        except ValueError:
            # Clean, user-facing message matching the Rust build (never leak Python's raw
            # "invalid literal for int() with base 10: '...'").
            raise ValueError(f"invalid --jobs range {raw!r}: not an integer") from None
    else:
        try:
            hi = int(text)
        except ValueError:
            raise ValueError(f"invalid --jobs {raw!r}: not an integer") from None
        lo = 1
    if lo < 1 or hi < lo:
        raise ValueError(f"invalid --jobs range {raw!r}: need 1 <= LO <= HI")
    return lo, hi


def _run_single_step(
    base_step: Step,
    cfg: DagConfig,
    inner_jobs: int,
    cgroups: CgroupManager | None,
    metrics: MetricsSink | None,
    verbosity: int,
) -> _SweepMeasure:
    """Run ONE step at a fixed inner-parallelism width and measure it.

    The step's ``preferred_inner_jobs`` is overridden to ``inner_jobs`` (so the width flows into
    the command via the jobs_flag mechanism) and its dependencies are cleared (a sweep runs the
    one step in isolation). CPU (user/sys) is measured from ``RUSAGE_CHILDREN`` deltas around the
    run; wall from the step's own recorded elapsed time; peak RSS from the step cgroup
    (``memory.peak``) when boxing is active."""
    step = dataclasses.replace(
        base_step,
        deps=[],
        hint=dataclasses.replace(base_step.hint, preferred_inner_jobs=inner_jobs),
    )
    one = dataclasses.replace(cfg, steps=(step,))
    ru0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    wall_start = time.time()
    result = run_dag(
        one, jobs=1, cgroups=cgroups, metrics=metrics, keep_going=False, verbosity=verbosity
    )
    wall_measured = time.time() - wall_start
    ru1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    user_s = max(ru1.ru_utime - ru0.ru_utime, 0.0)
    sys_s = max(ru1.ru_stime - ru0.ru_stime, 0.0)
    wall_s = wall_measured
    rss: int | None = None
    if result.step_profile_rows:
        row = result.step_profile_rows[0]
        elapsed = row.get("elapsed_s")
        if isinstance(elapsed, (int, float)):
            wall_s = float(elapsed)
        peak = row.get("peak_bytes")
        if isinstance(peak, int):
            rss = peak
    return _SweepMeasure(wall_s=wall_s, user_s=user_s, sys_s=sys_s, rss_hwm=rss, ok=result.ok)


def _cmd_sweep(ns: argparse.Namespace, c: Palette) -> int:
    """Per-step parallel-speedup sweep (Feature B)."""
    dag_arg = ns.dag if isinstance(ns.dag, str) else None
    if dag_arg is None:
        print(f"{PROG}: sweep: --dag FILE is required", file=sys.stderr)
        return 2
    try:
        cfg = _load(dag_arg)
    except (OSError, DagJsonError) as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return 2

    step_tag = str(ns.step)
    by_tag = cfg.by_tag()
    if step_tag not in by_tag:
        known = ", ".join(sorted(by_tag)) or "(none)"
        print(
            f"{PROG}: sweep: unknown --step tag {step_tag!r}. Known tags: {known}",
            file=sys.stderr,
        )
        return 2
    try:
        lo, hi = _parse_jobs_range(str(ns.jobs))
    except ValueError as exc:
        print(f"{PROG}: sweep: {exc}", file=sys.stderr)
        return 2
    repeat = max(1, int(ns.repeat))

    # Cgroup boxing is ON by default here too (so the sweep measures under real boxing).
    cgroups, code = _resolve_cgroup_manager(bool(ns.allow_cgroup_failure))
    if code != 0:
        return code

    perf_dir, source = _resolve_profile_dir(ns.perf_dir, bool(ns.no_profile))
    metrics: MetricsSink | None = None
    if perf_dir is not None:
        from safe_ci_dag_runner.perflog import CsvMetricsSink

        metrics = CsvMetricsSink(perf_dir, git_sha=_git_sha())

    base_step = by_tag[step_tag]
    verbosity = int(ns.verbosity)
    measures: list[tuple[int, _SweepMeasure]] = []
    for jobs in range(lo, hi + 1):
        best: _SweepMeasure | None = None
        for _ in range(repeat):
            m = _run_single_step(base_step, cfg, jobs, cgroups, metrics, verbosity)
            if not m.ok:
                print(
                    f"{PROG}: sweep: step {step_tag!r} FAILED at -j{jobs}; aborting the sweep",
                    file=sys.stderr,
                )
                return 1
            if best is None or m.wall_s < best.wall_s:
                best = m
        assert best is not None
        measures.append((jobs, best))

    _print_sweep_table(step_tag, lo, measures, c)
    if perf_dir is not None:
        _report_profile_written(perf_dir, source)
    return 0


def _print_sweep_table(
    step_tag: str, baseline_jobs: int, measures: Sequence[tuple[int, _SweepMeasure]], c: Palette
) -> None:
    """Print the parallel-speedup sweep table (Feature B) to stdout."""
    baseline_wall = measures[0][1].wall_s if measures else 0.0
    speedup_col = f"speedup(vs j{baseline_jobs})"
    headers = ["jobs", "wall_s", "user_s", "sys_s", "rss_hwm", speedup_col]
    table: list[list[str]] = []
    for jobs, m in measures:
        speedup = f"{baseline_wall / m.wall_s:.2f}x" if m.wall_s > 0 else "-"
        table.append(
            [
                str(jobs),
                f"{m.wall_s:.3f}",
                f"{m.user_s:.3f}",
                f"{m.sys_s:.3f}",
                _human_bytes(m.rss_hwm),
                speedup,
            ]
        )
    print(c.bold(f"parallel-speedup sweep: {step_tag}"))
    print(_render_table(headers, table, c))


def _select_jobs(cfg: DagConfig, ns: argparse.Namespace) -> int:
    """Choose the outer scheduler fan-out (``-j``).

    Precedence: an explicit ``--jobs`` always wins; otherwise ``--max-mem`` picks the largest
    ``-j`` whose modeled worst-case memory footprint fits the budget; otherwise default to the
    CPU count (LOGICAL CPUs, ``os.cpu_count()``). When BOTH ``--jobs`` and ``--max-mem`` are
    given, ``--jobs`` wins and the memory-aware sizing is skipped with a visible note (No Silent
    Failure). When ``--max-mem`` picks the full CPU count only because NO step carries an
    ``rss_baseline_bytes`` (the modeled footprint collapsed to ``mem_cap_floor_bytes``), a note
    explains why the budget did not throttle, so an un-throttled run is never a silent surprise."""
    max_mem = ns.max_mem if isinstance(ns.max_mem, str) and ns.max_mem else None
    if isinstance(ns.jobs, int):
        if max_mem is not None:
            print(
                f"{PROG}: both --jobs and --max-mem given; --jobs={ns.jobs} wins, "
                "--max-mem sizing skipped",
                file=sys.stderr,
            )
        return ns.jobs
    if max_mem is not None:
        budget = parse_size(max_mem)
        if budget is None:
            print(
                f"{PROG}: could not parse --max-mem {max_mem!r}; falling back to CPU count",
                file=sys.stderr,
            )
        else:
            jobs, footprint = jobs_for_budget(cfg, budget)
            print(
                f"{PROG}: --max-mem {max_mem} -> -j{jobs} "
                f"(modeled worst-case {footprint} bytes fits budget {budget} bytes)",
                file=sys.stderr,
            )
            ncpu = os.cpu_count() or 4
            modeled = any(
                s.hint.rss_baseline_bytes is not None and not s.engine_only for s in cfg.steps
            )
            if jobs == ncpu and not modeled:
                print(
                    f"{PROG}: note: no step carries rss_baseline_bytes, so the modeled footprint "
                    f"collapsed to the mem_cap_floor_bytes floor ({cfg.mem_cap_floor_bytes} bytes) "
                    f"and --max-mem did not throttle (-j{jobs} = CPU count); add per-step "
                    "rss_baseline_bytes to enable memory-aware throttling",
                    file=sys.stderr,
                )
            return jobs
    return os.cpu_count() or 4


def _resolve_cgroup_manager(allow_failure: bool) -> tuple[CgroupManager | None, int]:
    """Establish the two-level cgroup-v2 boxing that is this tool's PRIMARY purpose.

    Cgroup boxing is ON BY DEFAULT. This returns ``(manager, 0)`` when boxing is active (the
    caller runs boxed), ``(None, 0)`` for an intentional best-effort UNBOXED run, or
    ``(None, <nonzero>)`` when boxing is REQUIRED but unavailable and the caller must exit with
    that code (No Silent Failure: the reason is printed to stderr first).

    Flow (mirrors DeepScry's validate cgroup bring-up):

    * Already inside the managed scope (``SAFE_CI_IN_SCOPE=1``): construct :class:`Cgroups`. If
      per-step containment came up, install the scope teardown handler and box. If it did not,
      error (or, with ``allow_failure``, warn and run unboxed).
    * Not in a scope, ``allow_failure``: skip the re-exec and run unboxed with a visible warning
      (today's un-boxed behavior).
    * Not in a scope, default: re-exec this process inside a transient ``systemd-run --user
      --scope`` (a delegated cgroup) via :func:`reexec_in_scope`; on success ``execvp`` replaces
      this process and never returns. If it cannot (no cgroup-v2 / no systemd --user scope, or
      skipped in CI), print a clear error and return a nonzero exit code.
    """
    from safe_ci_dag_runner import cgroup as cg

    naming = cg.DEFAULT_NAMING
    if os.environ.get(naming.env_in_scope) == "1":
        manager = cg.Cgroups(naming)
        if manager.enabled:
            cg.install_scope_teardown(naming=naming)
            print(
                f"{PROG}: cgroup boxing ACTIVE (two-level cgroup-v2 scope; per-step memory/CPU caps"
                " + setsid-proof teardown).",
                file=sys.stderr,
            )
            return manager, 0
        if allow_failure:
            print(
                f"{PROG}: warning: inside a scope but per-step cgroup setup failed; running "
                "best-effort UNBOXED (--allow-cgroup-failure).",
                file=sys.stderr,
            )
            return None, 0
        print(
            f"{PROG}: ERROR: inside a managed scope but per-step cgroups could not be set up; "
            "re-run with --allow-cgroup-failure to run UNBOXED.",
            file=sys.stderr,
        )
        return None, 3
    if allow_failure:
        print(
            f"{PROG}: warning: cgroup boxing not established (--allow-cgroup-failure); running "
            "UNBOXED (process-group teardown only, no per-step memory/CPU caps).",
            file=sys.stderr,
        )
        return None, 0
    # Default: boxing is required -> re-exec into a transient systemd --user scope.
    argv = [sys.executable, "-m", "safe_ci_dag_runner", *sys.argv[1:]]
    reexeced_or_skipped = cg.reexec_in_scope(argv, memory_max=None)
    # Only reached when NO exec happened (execvp on success never returns).
    detail = (
        "boxing was skipped (e.g. CI without a systemd --user scope)"
        if reexeced_or_skipped
        else "cgroup-v2 + a working systemd --user scope are unavailable"
    )
    print(
        f"{PROG}: ERROR: cgroup boxing could not be established: {detail}. Cgroup resource boxing "
        "is this tool's primary purpose; re-run with --allow-cgroup-failure to run UNBOXED.",
        file=sys.stderr,
    )
    return None, 3


def _cmd_plan(ns: argparse.Namespace) -> int:
    """Show the plan (per-step estimates + sources, critical path, scheduled order) without
    running anything. Text (human) or canonical JSON (machine-readable, cross-identical)."""
    dag_arg = ns.dag if isinstance(ns.dag, str) else None
    if dag_arg is None:
        print(f"{PROG}: plan: --dag FILE is required", file=sys.stderr)
        return 2
    try:
        cfg = _load(dag_arg)
    except (OSError, DagJsonError) as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return 2
    planner = Planner.from_value(str(ns.planner)) or Planner.GREEDY_LPT
    feedback_dir = _resolve_feedback_dir(ns.perf_dir, bool(ns.no_profile_feedback))
    plan = _build_feedback_plan(cfg, feedback_dir, planner)
    if str(ns.format) == "json":
        print(plan_to_json(plan))
    else:
        sys.stdout.write(plan_to_text(plan))
    return 0


def _run(cfg: DagConfig, ns: argparse.Namespace, c: Palette) -> int:
    # Feature A: --only runs EXACTLY the named step(s). Validate/filter BEFORE cgroup bring-up so a
    # bad tag fails fast (exit 2) without needing a systemd scope.
    only_raw = ns.only if isinstance(ns.only, str) else None
    if only_raw is not None:
        tags = _parse_tag_list(only_raw)
        if not tags:
            print(f"{PROG}: run: --only requires at least one tag", file=sys.stderr)
            return 2
        try:
            cfg = _filter_only(cfg, tags)
        except _OnlyError as exc:
            print(f"{PROG}: {exc}", file=sys.stderr)
            return 2

    cgroups, code = _resolve_cgroup_manager(bool(ns.allow_cgroup_failure))
    if code != 0:
        return code

    # Plan-time profile-store FEEDBACK (ds-7pzdgm / ds-afzsqf): refine each step's est_duration_s
    # and rss_baseline_bytes from the recorded store, then pick the dispatch order for the chosen
    # --planner. The applied cfg (with refined hints) is what both the memory-aware -j sizing below
    # and the scheduler see, so planning improves automatically as runs accumulate.
    planner = Planner.from_value(str(ns.planner)) or Planner.GREEDY_LPT
    feedback_dir = _resolve_feedback_dir(ns.perf_dir, bool(ns.no_profile_feedback))
    plan = _build_feedback_plan(cfg, feedback_dir, planner)
    cfg = apply_plan_to_config(cfg, plan)
    if bool(ns.show_plan):
        sys.stdout.write(plan_to_text(plan))

    jobs = _select_jobs(cfg, ns)
    verbosity = 0 if bool(ns.quiet) else int(ns.verbosity)

    perf_dir, source = _resolve_profile_dir(ns.perf_dir, bool(ns.no_profile))
    metrics: MetricsSink | None = None
    if perf_dir is not None:
        from safe_ci_dag_runner.perflog import CsvMetricsSink

        metrics = CsvMetricsSink(perf_dir, git_sha=_git_sha())

    result = run_dag(
        cfg,
        jobs=jobs,
        cgroups=cgroups,
        metrics=metrics,
        keep_going=bool(ns.keep_going),
        verbosity=verbosity,
        order=list(plan.order),
    )
    passed = sum(1 for o in result.outcomes if o.ok)
    failed = sum(1 for o in result.outcomes if not o.ok and not o.aborted)
    aborted = sum(1 for o in result.outcomes if o.aborted)
    verdict = c.green("PASS") if result.ok else c.red("FAIL")
    print(
        f"{PROG}: {verdict} - {passed} passed, {failed} failed, {aborted} aborted, "
        f"{len(result.skipped)} skipped in {result.wall_s:.1f}s",
        file=sys.stderr,
    )
    if perf_dir is not None:
        _report_profile_written(perf_dir, source)
    # Feature C: --profile prints a per-step profile table to stdout.
    if bool(ns.profile):
        _print_profile_table(result.step_profile_rows, c)
    return 0 if result.ok else 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    ns = parser.parse_args(list(argv) if argv is not None else None)
    c = Palette(_color_enabled(sys.stdout))

    command = ns.command if isinstance(ns.command, str) else None
    if command is None:
        parser.print_help()
        return 0
    if command == "quickstart":
        print(_quickstart(c))
        return 0
    if command == "sweep":
        return _cmd_sweep(ns, c)
    if command == "plan":
        return _cmd_plan(ns)

    dag_arg = ns.dag if isinstance(ns.dag, str) else None
    if dag_arg is None:
        print(f"{PROG}: {command}: --dag FILE is required", file=sys.stderr)
        return 2
    try:
        cfg = _load(dag_arg)
    except (OSError, DagJsonError) as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return 2

    if command == "list":
        print(_render_list(cfg, c))
        return 0
    if command == "ascii":
        sys.stdout.write(to_ascii(cfg))
        return 0
    if command == "dot":
        sys.stdout.write(to_dot(cfg))
        return 0
    if command == "json":
        print(dag_to_json(cfg))
        return 0
    if command == "yaml":
        # dag_to_yaml already ends with a trailing newline.
        sys.stdout.write(dag_to_yaml(cfg))
        return 0
    if command == "run":
        return _run(cfg, ns, c)
    parser.print_help()
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
