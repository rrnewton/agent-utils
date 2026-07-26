"""Command-line interface for safe-ci-dag-runner.

Subcommands:
  run --dag FILE    run a DAG under two-level cgroup-v2 boxing (exit 0 iff every step passes;
                    boxing is ON by default — pass --allow-cgroup-failure to run un-boxed)
  list --dag FILE   list the steps
  ascii --dag FILE  draw the DAG as ASCII art
  dot --dag FILE    emit Graphviz DOT (pipe to `dot -Tsvg`)
  json --dag FILE   re-emit the DAG as canonical JSON
  quickstart        print a self-contained getting-started guide

A DAG is a JSON file (see `quickstart` for the schema + a runnable example).
Pass `--dag -` to read the DAG from stdin.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.io import DagJsonError, dag_from_json, dag_to_json
from safe_ci_dag_runner.model import DagConfig, step_classification
from safe_ci_dag_runner.protocols import CgroupManager, MetricsSink
from safe_ci_dag_runner.scheduler import run_dag
from safe_ci_dag_runner.sizing import jobs_for_budget, parse_size
from safe_ci_dag_runner.viz import to_ascii, to_dot

PROG = "safe-ci-dag-runner"


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
        f"  {ex(f'{PROG} quickstart')}                 {c.dim('# get started (schema + runnable demo)')}\n"
        f"  {ex(f'{PROG} run --dag dag.json')}         {c.dim('# run it; exit 0 iff all steps pass')}\n"
        f"  {ex(f'{PROG} ascii --dag dag.json')}       {c.dim('# quick ASCII view of the graph')}\n"
        f"  {ex(f'{PROG} dot --dag dag.json | dot -Tsvg -o dag.svg')}\n"
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

{h('DAG schema')}  {c.dim('(only group/job/cmd are required per step; everything else has defaults)')}
  step:   group, job, desc, description, cmd, deps[], env{{}}, timeout, jobs_flag, networkonly, engine_only, hint{{}}
  hint:   resources{{name:int}}, est_duration_s, rss_baseline_bytes, hard_mem_max_bytes,
          classification("cpu-bound"|"latency-bound"|"light"), preferred_inner_jobs
  top:    description, resource_caps{{name:int}}, mem_cap_factor, mem_cap_floor_bytes,
          outer_mem_safety_factor, default_step_timeout, default_jobs_flag
  {c.dim('desc = short label; description = long-form docs (often multi-line, great in YAML).')}
  {c.dim('resource_caps bound concurrent demand - e.g. {"browser":1} serializes browser steps.')}
  {c.dim('jobs_flag: template appended with a step preferred_inner_jobs, e.g. "-j" -> "-j 4",')}
  {c.dim('  "-j%d" -> "-j4", "--jobs=" -> "--jobs=4", "--num-threads" -> "--num-threads 4".')}

{h('What you get')}
  - concurrent scheduling in longest-first order, honoring deps + resource caps
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
    run_p.add_argument("--dag", required=True, metavar="FILE", help="DAG JSON file ('-' = stdin)")
    run_p.add_argument("-j", "--jobs", type=int, default=None, help="max concurrent steps (default: CPU count)")
    run_p.add_argument(
        "--max-mem",
        metavar="SPEC",
        default=None,
        help="RAM budget (e.g. 8G, 4096M); pick the largest -j whose modeled worst-case "
        "footprint fits (ignored when --jobs is given)",
    )
    run_p.add_argument(
        "--perf-dir",
        metavar="DIR",
        default=None,
        help="write per-step + whole-run resource-usage CSVs into DIR",
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

    for cmd, helptext in (
        ("list", "list the steps"),
        ("ascii", "draw the DAG as ASCII art"),
        ("dot", "emit Graphviz DOT (pipe to `dot -Tsvg`)"),
        ("json", "re-emit the DAG as canonical JSON"),
    ):
        sp = sub.add_parser(cmd, help=helptext)
        sp.add_argument("--dag", required=True, metavar="FILE", help="DAG JSON file ('-' = stdin)")

    sub.add_parser("quickstart", help="print a self-contained getting-started guide")
    return parser


def _load(dag_arg: str) -> DagConfig:
    text = sys.stdin.read() if dag_arg == "-" else Path(dag_arg).read_text(encoding="utf-8")
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


def _run(cfg: DagConfig, ns: argparse.Namespace, c: Palette) -> int:
    cgroups, code = _resolve_cgroup_manager(bool(ns.allow_cgroup_failure))
    if code != 0:
        return code
    jobs = _select_jobs(cfg, ns)
    verbosity = 0 if bool(ns.quiet) else int(ns.verbosity)

    perf_dir = ns.perf_dir if isinstance(ns.perf_dir, str) and ns.perf_dir else None
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
        written = sorted(str(p) for p in Path(perf_dir).glob("*.csv"))
        if written:
            print(f"{PROG}: perf CSVs written under {perf_dir}:", file=sys.stderr)
            for path in written:
                print(f"  {path}", file=sys.stderr)
        else:
            print(
                f"{PROG}: WARNING no perf CSVs were written under {perf_dir}",
                file=sys.stderr,
            )
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
    if command == "run":
        return _run(cfg, ns, c)
    parser.print_help()
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
