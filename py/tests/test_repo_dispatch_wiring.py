"""Repository-local dispatcher and dependency-smoke wiring contracts."""

from __future__ import annotations

import importlib.util
import stat
import subprocess
import sys
from pathlib import Path
from types import ModuleType


REPO_ROOT = Path(__file__).resolve().parents[2]
RESOLVER_TOOLS = (
    "safe-ci-dag-runner",
    "cpuset-alloc",
    "tick-hub",
    "pr-landing-planner",
    "agent-team-timeline",
    "herdr-run",
)


def _load_check_deps() -> ModuleType:
    path = REPO_ROOT / "scripts" / "check_deps.py"
    spec = importlib.util.spec_from_file_location("_repo_dispatch_check_deps", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_tracked_tool_links_share_the_engine_resolver() -> None:
    """Every command exposed through ``./bin`` has one tracked dispatcher target."""
    common_bin = REPO_ROOT / "common" / "bin"
    for tool in RESOLVER_TOOLS:
        link = common_bin / tool
        assert link.is_symlink(), tool
        assert link.readlink() == Path("engine-resolver"), tool


def test_tracked_python_launchers_are_directly_executable() -> None:
    """Every ``py/bin`` link is also a valid direct source-checkout command."""
    py_bin = REPO_ROOT / "py" / "bin"
    for tool in RESOLVER_TOOLS:
        link = py_bin / tool
        assert link.is_symlink(), tool
        target = link.resolve(strict=True)
        assert target.stat().st_mode & stat.S_IXUSR, tool
        assert target.read_bytes().startswith(b"#!/usr/bin/env python3\n"), tool

        completed = subprocess.run(
            [str(link), "--version"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=15,
        )
        assert completed.returncode == 0, (
            f"{tool}: direct launcher failed with {completed.returncode}: "
            f"{completed.stderr}"
        )


def test_dependency_smoke_covers_cpuset_companion() -> None:
    """The companion command gets every dependency-free startup probe."""
    check_deps = _load_check_deps()
    module = "safe_ci_dag_runner.cpuset_allocator"
    assert module in check_deps.ENTRYPOINT_MODULES

    py_dir = check_deps._py_dir()
    for args in check_deps.DEPFREE_ARGS:
        assert check_deps._check_one(py_dir, module, args) is None
