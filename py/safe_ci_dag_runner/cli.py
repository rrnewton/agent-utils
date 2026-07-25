"""Command-line interface for safe-ci-dag-runner.

Subcommands:
  run FILE        run a DAG (exit 0 iff every step passes)
  list FILE       list the steps
  ascii FILE      draw the DAG as ASCII art
  dot FILE        emit Graphviz DOT (pipe to `dot -Tsvg`)
  json FILE       re-emit the DAG as canonical JSON
  quickstart      print a self-contained getting-started guide

A DAG is a JSON file (see `quickstart` for the schema + a runnable example).
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Sequence
from pathlib import Path

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.io import DagJsonError, dag_from_json, dag_to_json
from safe_ci_dag_runner.model import DagConfig, step_classification
from safe_ci_dag_runner.protocols import CgroupManager
from safe_ci_dag_runner.scheduler import run_dag
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
  step:   group, job, desc, cmd, deps[], env{{}}, timeout, networkonly, engine_only, hint{{}}
  hint:   resources{{name:int}}, est_duration_s, rss_baseline_bytes, hard_mem_max_bytes,
          classification("cpu-bound"|"latency-bound"|"light"), preferred_inner_jobs
  top:    resource_caps{{name:int}}, mem_cap_factor, mem_cap_floor_bytes,
          outer_mem_safety_factor, default_step_timeout
  {c.dim('resource_caps bound concurrent demand - e.g. {{"browser":1}} serializes browser steps.')}

{h('What you get')}
  - concurrent scheduling in longest-first order, honoring deps + resource caps
  - a failing step fails the run (exit 1) and, by default, eager-cancels in-flight steps
    ({k('--keep-going')} runs everything runnable and reports all failures)
  - {k('run --cgroups')} adds best-effort Linux cgroup-v2 per-step memory/CPU boxing
    {c.dim('(needs cgroup-v2; without it, steps run un-boxed with a visible warning)')}

{h('Python API')}  {c.dim('(same engine, in code)')}
  from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag, to_ascii
  cfg = DagConfig(steps=(Step("build","app","compile","make build"),))
  print(to_ascii(cfg)); result = run_dag(cfg, jobs=4)   # result.ok, result.outcomes

{h('Exit codes')}  0 = all steps passed | 1 = a step failed | 2 = bad usage / bad DAG file
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
    run_p.add_argument("-k", "--keep-going", action="store_true", help="run all runnable steps even after a failure")
    run_p.add_argument("--cgroups", action="store_true", help="best-effort Linux cgroup-v2 per-step boxing")
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


def _run(cfg: DagConfig, ns: argparse.Namespace, c: Palette) -> int:
    jobs = ns.jobs if isinstance(ns.jobs, int) else (os.cpu_count() or 4)
    verbosity = 0 if bool(ns.quiet) else int(ns.verbosity)
    cgroups: CgroupManager | None = None
    if bool(ns.cgroups):
        from safe_ci_dag_runner.cgroup import Cgroups

        cgroups = Cgroups()
    result = run_dag(
        cfg, jobs=jobs, cgroups=cgroups, keep_going=bool(ns.keep_going), verbosity=verbosity
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
