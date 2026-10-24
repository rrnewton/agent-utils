#!/usr/bin/env python3
"""Prove that one subset of the suite works with nothing else from the suite installed.

The package checks prove that each distribution installs alone and answers ``--help``. This check
asks the next question a user asks: with only this subset installed, does its quickstart workflow
actually run? Each subset is installed from a copy of its source into an empty target directory,
with only its declared third-party dependencies beside it. Commands then run with an empty
environment: a fresh ``HOME``, and a ``PATH`` holding the subset's own commands plus every
ordinary system command except those the suite (or Herdr) installs, wherever the validating host
happens to have them. The check first proves that no other suite command resolves and that no
sibling package imports, then runs the subset's scenario.

A subset that quietly needs another suite tool's binary, configuration, or Herdr fails here
instead of on a stranger's machine. ``ai_docs/transient/2026-10-24-suite-a-la-carte.md`` explains
the subsets and the ones still to add.

    python3 scripts/check_standalone.py --list
    python3 scripts/check_standalone.py --subset wrkslots
    python3 scripts/check_standalone.py --subset tick-hub --keep
"""

from __future__ import annotations

import argparse
import importlib.metadata
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PY_ROOT = REPO_ROOT / "py"

#: Directories searched for ordinary system commands, in PATH order.
SYSTEM_DIRECTORIES = ("/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin")

#: Commands the suite needs but does not install. Herdr is the suite's terminal server; a subset
#: that needs it has to say so by listing it in its own commands.
EXTERNAL_SUITE_COMMANDS = frozenset({"herdr"})

#: Import names of every Python distribution in the suite.
SUITE_PACKAGES = (
    "agentctl",
    "dagrun",
    "herdr_run",
    "parallel_experiment_runner",
    "pr_landing_planner",
    "tick_hub",
    "wrkslots",
    "wrkviz",
)


class CheckError(RuntimeError):
    """One subset failed to work on its own."""


def suite_commands() -> frozenset[str]:
    """Every command name the suite installs, read from the tree so a new tool is covered."""

    names = {entry.name for entry in (PY_ROOT / "bin").iterdir()}
    manifests = sorted((REPO_ROOT / "rs").glob("*/Cargo.toml"))
    manifests.append(REPO_ROOT / "vibe-talk" / "Cargo.toml")
    for manifest in manifests:
        section = ""
        for line in manifest.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if stripped.startswith("["):
                section = stripped
                continue
            match = re.fullmatch(r'name\s*=\s*"([^"]+)"', stripped)
            if section == "[[bin]]" and match:
                names.add(match.group(1))
    return frozenset(names | EXTERNAL_SUITE_COMMANDS)


@dataclass
class Sandbox:
    """One subset's private installation and the environment its commands run in."""

    root: Path
    env: dict[str, str]

    @property
    def work(self) -> Path:
        return self.root / "work"

    def run(
        self,
        argv: list[str],
        *,
        cwd: Path | None = None,
        expect: int = 0,
        timeout: int = 300,
        extra_env: Mapping[str, str] | None = None,
    ) -> str:
        """Run one command in the sandbox and return its stdout; any other exit status fails."""

        env = dict(self.env)
        if extra_env:
            env.update(extra_env)
        try:
            proc = subprocess.run(
                argv,
                cwd=cwd or self.work,
                env=env,
                text=True,
                capture_output=True,
                check=False,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as exc:
            raise CheckError(f"timed out after {timeout}s: {' '.join(argv)}") from exc
        except FileNotFoundError as exc:
            raise CheckError(f"{argv[0]}: not installed in this subset's environment") from exc
        combined = proc.stdout + proc.stderr
        if proc.returncode != expect or "Traceback (most recent call last)" in combined:
            raise CheckError(
                f"{' '.join(argv)}: exit {proc.returncode}, expected {expect}\n"
                f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
            )
        return proc.stdout


@dataclass(frozen=True)
class Subset:
    """What a user installs for one standalone use, and the workflow that proves it works."""

    name: str
    summary: str
    #: Source directories under py/ installed into the subset's private target.
    projects: tuple[str, ...]
    #: Third-party distributions the projects declare, copied from the validating interpreter.
    dependencies: tuple[str, ...]
    #: Commands the subset itself provides; every other suite command must be absent.
    commands: tuple[str, ...]
    scenario: Callable[[Sandbox], None]


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise CheckError(message)


# ---- scenarios ------------------------------------------------------------------------------------

_OPS_YAML = """\
reminders:
  - name: refresh_cache
    cadence_secs: 3600
    emit:
      kind: action
      skill: refresh-cache
      title: refresh the shared cache
health_checks:
  - name: newest_snapshot
    glob: {snapshots}/*.tar
    threshold_secs: 86400
"""


def tick_hub_scenario(box: Sandbox) -> None:
    """The README quick start: a due reminder fires once, and flushed state stops it repeating."""

    (box.work / "ops.yaml").write_text(
        _OPS_YAML.format(snapshots=box.work / "snapshots"), encoding="utf-8"
    )
    command = ["tick-hub", "tick", "--config", "ops.yaml", "--no-header"]
    first = box.run(command)
    _require("ACTION: refresh-cache" in first, f"first tick emitted no action:\n{first}")
    _require("HEALTH: newest_snapshot missing" in first, f"no health line:\n{first}")
    box.run([*command, "--flush"])
    _require((box.work / ".tick-hub" / "state").exists(), "--flush wrote no state file")
    again = box.run(command)
    _require("ACTION:" not in again, f"a flushed reminder fired again:\n{again}")
    _require("emitted 0 instruction(s)" in again, f"unexpected second tick:\n{again}")


_AGENT_SCRIPT = """\
set -eu
wrkslots create s1 --slot-type agent --coordinator-authorized --agent worker-a --task t1 \\
  --purpose 'standalone check' --coordinator-pid "$COORDINATOR_PID" --owner-pid $$ \\
  --repo product=product --branch product=worker-a/t1 --representation worktree
git -C worktrees/slots/s1/product commit -q --allow-empty -m 'standalone work'
printf 'Work committed on worker-a/t1.\\n' > "$HANDOFF"
wrkslots write-handoff s1 --agent worker-a --owner-pid $$ --expected-generation 1 \\
  --from-file "$HANDOFF"
wrkslots release s1 --agent worker-a --owner-pid $$ --expected-generation 1
"""


def wrkslots_scenario(box: Sandbox) -> None:
    """A whole slot lifecycle with the shipped example liveness probe and no session manager.

    The check is the coordinator. Its child shell plays the agent: it creates its own slot (as an
    assigned agent may), commits, writes a handoff, and releases. The coordinator then reads the
    handoff and removes the slot once the project's liveness record says the agent is dead.
    """

    project = box.work / "project"
    remote = box.work / "remote.git"
    product = project / "product"
    product.mkdir(parents=True)
    box.run(["git", "init", "-q", "--bare", str(remote)])
    box.run(["git", "init", "-q", "-b", "main"], cwd=product)
    box.run(["git", "commit", "-q", "--allow-empty", "-m", "init"], cwd=product)
    box.run(["git", "remote", "add", "origin", str(remote)], cwd=product)
    box.run(["git", "push", "-q", "origin", "main"], cwd=product)
    box.run(["git", "fetch", "-q", "origin"], cwd=product)

    # The generic example probe answers from a file per agent; it knows nothing of agentctl.
    probe = box.root / "site" / "wrkslots" / "examples" / "liveness_probe.py"
    tools = project / "tools"
    tools.mkdir()
    shutil.copy2(probe, tools / "liveness_probe.py")
    (tools / "liveness_probe.py").chmod(0o755)
    box.run(
        ["wrkslots", "init", ".", "--worktrees-dir", "worktrees/slots",
         "--liveness-command", "tools/liveness_probe.py"],
        cwd=project,
    )
    coordinator = {"COORDINATOR_PID": str(os.getpid()), "HANDOFF": str(box.work / "handoff.md")}
    box.run(["sh", "-c", _AGENT_SCRIPT], cwd=project, extra_env=coordinator)

    liveness = project / ".agent-liveness"
    liveness.mkdir()
    (liveness / "worker-a").write_text("dead\n", encoding="utf-8")
    pid = str(os.getpid())
    handoff = box.run(["wrkslots", "read-handoff", "s1", "--coordinator-pid", pid], cwd=project)
    _require("Work committed on worker-a/t1." in handoff, f"handoff not returned:\n{handoff}")
    box.run(
        ["wrkslots", "remove", "s1", "--coordinator-pid", pid, "--expected-generation", "1"],
        cwd=project,
        timeout=600,
    )
    status = box.run(["wrkslots", "status"], cwd=project)
    _require(" active=0 " in status, f"slot still active after remove:\n{status}")
    _require(not (project / "worktrees" / "slots" / "s1").exists(), "slot directory remains")
    subject = box.run(["git", "log", "-1", "--format=%s", "worker-a/t1"], cwd=product)
    _require(subject.strip() == "standalone work", f"agent branch lost its work: {subject!r}")


SUBSETS: tuple[Subset, ...] = (
    Subset(
        name="tick-hub",
        summary="recurring duties evaluated by a timer, with no coordinator or agentctl",
        projects=("tick_hub",),
        dependencies=("PyYAML",),
        commands=("tick-hub",),
        scenario=tick_hub_scenario,
    ),
    Subset(
        name="wrkslots",
        summary="worktree slots for any agents, with no agentctl, Herdr, systemd unit, or sudo",
        projects=("wrkslots",),
        dependencies=(),
        commands=("wrkslots",),
        scenario=wrkslots_scenario,
    ),
)


# ---- installation -------------------------------------------------------------------------------


def _install_projects(subset: Subset, root: Path) -> None:
    """Install each project from a private copy, so the source tree gains no build output."""

    for directory in subset.projects:
        copy = root / "src" / directory
        shutil.copytree(
            PY_ROOT / directory,
            copy,
            symlinks=False,
            ignore=shutil.ignore_patterns("__pycache__", "build", "*.egg-info", "tests"),
        )
        proc = subprocess.run(
            [sys.executable, "-m", "pip", "install", "--quiet", "--no-deps", "--no-index",
             "--no-build-isolation", "--target", str(root / "site"), str(copy)],
            env={**os.environ, "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1"},
            text=True,
            capture_output=True,
            check=False,
            timeout=300,
        )
        if proc.returncode != 0:
            raise CheckError(f"installing {directory} failed:\n{proc.stdout}{proc.stderr}")


def _copy_dependencies(subset: Subset, root: Path) -> None:
    """Copy each declared third-party distribution's installed files, and nothing else."""

    target = root / "deps"
    target.mkdir()
    for name in subset.dependencies:
        try:
            distribution = importlib.metadata.distribution(name)
        except importlib.metadata.PackageNotFoundError as exc:
            raise CheckError(f"{subset.name} needs {name}, which this interpreter lacks") from exc
        base = Path(str(distribution.locate_file("")))
        for relative in distribution.files or ():
            if relative.parts and relative.parts[0] == "..":
                continue
            source = base / relative
            if source.is_file():
                destination = target / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)


def _system_tools(root: Path, excluded: frozenset[str]) -> Path:
    """Link every ordinary system command except the suite's into one PATH directory."""

    farm = root / "system-bin"
    farm.mkdir()
    for directory in SYSTEM_DIRECTORIES:
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError:
            continue
        for entry in entries:
            name = entry.name
            if name in excluded or os.path.lexists(farm / name):
                continue
            if os.access(entry.path, os.X_OK) and not entry.is_dir():
                os.symlink(entry.path, farm / name)
    return farm


def _sandbox(subset: Subset, root: Path) -> Sandbox:
    every_command = suite_commands()
    unknown = set(subset.commands) - every_command
    _require(not unknown, f"{subset.name}: commands not in the suite inventory: {sorted(unknown)}")
    _install_projects(subset, root)
    _copy_dependencies(subset, root)
    farm = _system_tools(root, every_command)
    home = root / "home"
    for directory in (home, root / "tmp", root / "work"):
        directory.mkdir()
    site = root / "site"
    env = {
        "HOME": str(home),
        "PATH": f"{site / 'bin'}:{farm}",
        "PYTHONPATH": f"{site}:{root / 'deps'}",
        "PYTHONNOUSERSITE": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TMPDIR": str(root / "tmp"),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_STATE_HOME": str(home / ".local" / "state"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "NO_COLOR": "1",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "standalone",
        "GIT_AUTHOR_EMAIL": "standalone@example.invalid",
        "GIT_COMMITTER_NAME": "standalone",
        "GIT_COMMITTER_EMAIL": "standalone@example.invalid",
        # A fixed machine shard keeps the host's name out of wrkslots state and output.
        "WRKSLOTS_MACHINE": "standalone",
    }
    box = Sandbox(root, env)

    path = env["PATH"]
    for command in sorted(every_command):
        found = shutil.which(command, path=path)
        if command in subset.commands:
            _require(
                found is not None and Path(found).parent == site / "bin",
                f"{subset.name}: its own command {command} is not installed",
            )
        else:
            _require(found is None, f"{subset.name}: foreign suite command visible: {found}")
    own = {directory.replace("-", "_") for directory in subset.projects}
    absent = [package for package in SUITE_PACKAGES if package not in own]
    box.run(
        ["python3", "-c",
         "import importlib.util, sys; "
         "found = [n for n in sys.argv[1:] if importlib.util.find_spec(n) is not None]; "
         "sys.exit(f'sibling packages importable: {found}' if found else 0)",
         *absent]
    )
    return box


def check(subset: Subset, keep: bool) -> float:
    """Install one subset, prove it is alone, run its quickstart and scenario."""

    started = time.monotonic()
    root = Path(tempfile.mkdtemp(prefix=f"standalone-{subset.name}-"))
    try:
        box = _sandbox(subset, root)
        for command in subset.commands:
            guide = box.run([command, "quickstart"])
            _require(bool(guide.strip()), f"{command} quickstart printed nothing")
        subset.scenario(box)
    finally:
        if keep:
            print(f"check_standalone: kept {root}", file=sys.stderr)
        else:
            shutil.rmtree(root, ignore_errors=True)
    return time.monotonic() - started


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Install one subset of the suite alone in a clean environment and run its quickstart "
            "workflow. With no --subset, every subset is checked."
        )
    )
    parser.add_argument(
        "--subset",
        action="append",
        choices=[subset.name for subset in SUBSETS],
        help="subset to check; repeat for several (default: all)",
    )
    parser.add_argument("--list", action="store_true", help="list the subsets and exit")
    parser.add_argument(
        "--keep", action="store_true", help="keep each subset's temporary directory for inspection"
    )
    arguments = parser.parse_args(argv)
    if arguments.list:
        for subset in SUBSETS:
            print(f"{subset.name}: {subset.summary}")
        return 0
    chosen = [s for s in SUBSETS if not arguments.subset or s.name in arguments.subset]
    for subset in chosen:
        try:
            elapsed = check(subset, arguments.keep)
        except (CheckError, OSError, subprocess.SubprocessError) as exc:
            print(f"check_standalone: FAIL: {subset.name}: {exc}", file=sys.stderr)
            return 1
        print(f"check_standalone: {subset.name}: OK ({elapsed:.1f}s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
