#!/usr/bin/env python3
"""Prove that one subset of the suite works with nothing else from the suite installed.

The package checks prove that each distribution installs alone and answers ``--help``. This check
asks the next question a user asks: with only this subset installed, does its quickstart workflow
actually run? Each subset's projects are built as wheels from a copy of their source and installed
into a fresh virtual environment that cannot see the interpreter's own site-packages, with only
their declared third-party dependencies copied in beside them. Commands then run with an empty
environment: a fresh ``HOME``, and a ``PATH`` holding that environment's ``bin`` plus every
ordinary system command except those the suite (or Herdr) installs and those the subset promises
not to need, wherever the validating host happens to have them. The check first proves that no
other suite command resolves, that no sibling package imports, and that every forbidden system
command is absent, then runs the subset's scenario.

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
import importlib.util
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import venv
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


def suite_packages() -> tuple[str, ...]:
    """Import names of every Python distribution in the suite: each py/ directory with a project."""

    return tuple(
        sorted(
            manifest.parent.name
            for manifest in PY_ROOT.glob("*/pyproject.toml")
            if (manifest.parent / "__init__.py").is_file()
        )
    )


@dataclass
class Sandbox:
    """One subset's private installation and the environment its commands run in."""

    root: Path
    #: The virtual environment's site-packages, where the subset is installed.
    site: Path
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
        """Run one command in the sandbox and return its stdout; any other exit status fails.

        Each command leads its own process group, so a timeout kills everything it started.
        """

        env = dict(self.env)
        if extra_env:
            env.update(extra_env)
        try:
            process = subprocess.Popen(
                argv,
                cwd=cwd or self.work,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
        except FileNotFoundError as exc:
            raise CheckError(f"{argv[0]}: not installed in this subset's environment") from exc
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
            raise CheckError(f"timed out after {timeout}s: {' '.join(argv)}") from exc
        if process.returncode != expect or "Traceback (most recent call last)" in stdout + stderr:
            raise CheckError(
                f"{' '.join(argv)}: exit {process.returncode}, expected {expect}\n"
                f"stdout:\n{stdout}\nstderr:\n{stderr}"
            )
        return stdout


@dataclass(frozen=True)
class Subset:
    """What a user installs for one standalone use, and the workflow that proves it works."""

    name: str
    summary: str
    #: Source directories under py/ installed into the subset's private environment.
    projects: tuple[str, ...]
    #: Third-party distributions the projects declare, copied from the validating interpreter.
    dependencies: tuple[str, ...]
    #: Commands the subset itself provides; every other suite command must be absent.
    commands: tuple[str, ...]
    scenario: Callable[[Sandbox], None]
    #: Ordinary system commands the subset claims not to need; they are withheld from its PATH.
    withheld: tuple[str, ...] = ()


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
    probe = box.site / "wrkslots" / "examples" / "liveness_probe.py"
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
        summary="worktree slots for any agents, with no agentctl, Herdr, systemd, or sudo",
        projects=("wrkslots",),
        dependencies=(),
        commands=("wrkslots",),
        scenario=wrkslots_scenario,
        withheld=(
            "sudo", "systemd-run", "systemctl", "busctl", "fuse2fs", "mkfs.ext4", "mount",
            "umount",
        ),
    ),
)


# ---- installation -------------------------------------------------------------------------------


def _run_host(argv: list[str], what: str) -> None:
    """Run one installation step with the validating interpreter's environment."""

    proc = subprocess.run(
        argv,
        env={**os.environ, "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1"},
        text=True,
        capture_output=True,
        check=False,
        timeout=300,
    )
    if proc.returncode != 0:
        raise CheckError(f"{what} failed:\n{proc.stdout}{proc.stderr}")


def _install_projects(subset: Subset, root: Path, python: Path) -> None:
    """Build each project as a wheel from a private copy, then install it into the environment.

    Building from a copy keeps build output out of the source tree. The wheel is installed with
    the environment's own interpreter as the target, so its console scripts start that
    interpreter and nothing else.
    """

    if importlib.util.find_spec("setuptools") is None:
        raise CheckError(
            "the validating interpreter needs setuptools (the projects' build backend), because "
            "this check builds offline"
        )
    wheels = root / "wheels"
    for directory in subset.projects:
        copy = root / "src" / directory
        shutil.copytree(
            PY_ROOT / directory,
            copy,
            ignore=shutil.ignore_patterns("__pycache__", "build", "*.egg-info", "tests"),
        )
        _run_host(
            [sys.executable, "-m", "pip", "wheel", "--quiet", "--no-deps", "--no-index",
             "--no-build-isolation", "--wheel-dir", str(wheels), str(copy)],
            f"building {directory}",
        )
    for wheel in sorted(wheels.glob("*.whl")):
        _run_host(
            [sys.executable, "-m", "pip", "--python", str(python), "install", "--quiet",
             "--no-deps", "--no-index", str(wheel)],
            f"installing {wheel.name}",
        )


def _copy_dependencies(subset: Subset, site: Path) -> None:
    """Copy each declared third-party distribution's installed files, and nothing else."""

    for name in subset.dependencies:
        try:
            distribution = importlib.metadata.distribution(name)
        except importlib.metadata.PackageNotFoundError as exc:
            raise CheckError(f"{subset.name} needs {name}, which this interpreter lacks") from exc
        base = Path(str(distribution.locate_file("")))
        copied = 0
        for relative in distribution.files or ():
            if relative.parts and relative.parts[0] == "..":
                continue
            source = base / relative
            if source.is_file():
                destination = site / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)
                copied += 1
        _require(copied > 0, f"{name}: its installed file list names no files to copy")


def _system_tools(root: Path, excluded: frozenset[str]) -> Path:
    """Link every ordinary system command except the excluded ones into one PATH directory."""

    farm = root / "system-bin"
    farm.mkdir()
    for directory in SYSTEM_DIRECTORIES:
        try:
            with os.scandir(directory) as iterator:
                entries = sorted(iterator, key=lambda entry: entry.name)
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

    environment = root / "venv"
    venv.EnvBuilder(with_pip=False, system_site_packages=False, symlinks=True).create(environment)
    python = environment / "bin" / "python"
    found_site = subprocess.run(
        [str(python), "-I", "-c", "import sysconfig; print(sysconfig.get_paths()['purelib'])"],
        text=True, capture_output=True, check=True, timeout=60,
    )
    site = Path(found_site.stdout.strip())
    _install_projects(subset, root, python)
    _copy_dependencies(subset, site)
    withheld = frozenset(subset.withheld)
    farm = _system_tools(root, every_command | withheld)
    home = root / "home"
    for directory in (home, root / "tmp", root / "work"):
        directory.mkdir()
    env = {
        "HOME": str(home),
        "PATH": f"{environment / 'bin'}:{farm}",
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
    box = Sandbox(root, site, env)

    path = env["PATH"]
    for command in sorted(every_command):
        found = shutil.which(command, path=path)
        if command in subset.commands:
            _require(
                found is not None and Path(found).parent == environment / "bin",
                f"{subset.name}: its own command {command} is not installed",
            )
        else:
            _require(found is None, f"{subset.name}: foreign suite command visible: {found}")
    for command in sorted(withheld):
        found = shutil.which(command, path=path)
        _require(found is None, f"{subset.name}: withheld system command visible: {found}")
    for command in subset.commands:
        shebang = (environment / "bin" / command).read_text(encoding="utf-8").splitlines()[0]
        _require(
            shebang == f"#!{python}",
            f"{subset.name}: {command} starts {shebang!r}, not the environment's interpreter",
        )
    own = set(subset.projects)
    absent = [package for package in suite_packages() if package not in own]
    box.run(
        ["python3", "-c",
         "import importlib.util, sys; "
         "assert sys.prefix != sys.base_prefix, 'not the subset environment'; "
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
