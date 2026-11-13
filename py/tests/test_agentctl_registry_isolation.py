"""Tests must never write into an ancestor registry (#1499 tests-touch-live-registry).

A lane worktree lives under its harness directory, so agentctl's ancestor
walk from a test working directory finds the harness's live ``.agentctl``.
The gating check below plants a sentinel registry above a fixture run's
working directory and requires it byte-identical afterwards: any bare
write-path call that resolves by the walk moves the sentinel's legacy
agent, exactly as the live registry was moved. No wall clock is used.
"""
from __future__ import annotations

import hashlib
import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
PY_ROOT = REPO_ROOT / "py"


def _manifest(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def _sentinel(tree: Path) -> Path:
    registry = tree / ".agentctl"
    (registry / "restore" / "reports").mkdir(parents=True)
    (registry / "restore" / "reports" / "a.json").write_text('{"boot":"x"}\n', encoding="utf-8")
    worker = registry / "worker"
    worker.mkdir()
    (worker / "agent.json").write_text("{}\n", encoding="utf-8")
    registry.chmod(0o700)
    worker.chmod(0o700)
    return registry


def test_bare_write_run_leaves_ancestor_sentinel_byte_identical(tmp_path: Path) -> None:
    tree = tmp_path / "tree"
    work = tree / "sub"
    work.mkdir(parents=True)
    registry = _sentinel(tree)
    (work / "test_bare_write.py").write_text(
        "from agentctl import cli\n\n\n"
        "def test_bare_revive():\n"
        "    assert cli.main(['revive', '--all']) in (0, 75)\n",
        encoding="utf-8",
    )
    before = _manifest(registry)
    environment = {
        key: value
        for key, value in os.environ.items()
        if key not in ("AGENTCTL_REGISTRY", "AGENTCTL_LAYOUT_AUTO_MIGRATE")
    }
    environment["PYTHONPATH"] = os.pathsep.join((str(PY_ROOT), str(REPO_ROOT / "scripts")))
    run = subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "run_pytest_shard.py"),
         "--shard", "0/1", "--", "-q", "-p", "no:cacheprovider", "test_bare_write.py"],
        cwd=work, env=environment, capture_output=True, text=True, timeout=300,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert _manifest(registry) == before
    assert (registry / "worker" / "agent.json").is_file()
    assert not (registry / "agents").exists()


def test_cross_adapter_env_never_inherits_a_caller_registry() -> None:
    """The cross/differential adapters pass --registry explicitly per case."""
    sys.path.insert(0, str(REPO_ROOT / "cross"))
    try:
        import herdr_agent_differential as cross_env

        environment = cross_env.without_ambient_executables(
            {"AGENTCTL_REGISTRY": "/live/.agentctl", "AGENTCTL_LAYOUT_AUTO_MIGRATE": "1"}
        )
    finally:
        sys.path.remove(str(REPO_ROOT / "cross"))
    assert "AGENTCTL_REGISTRY" not in environment
    assert "AGENTCTL_LAYOUT_AUTO_MIGRATE" not in environment


def test_gate_child_env_isolates_rust_and_python_registries(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The gate/validate child environment is the Rust helpers' isolation."""
    import importlib.util

    spec = importlib.util.spec_from_file_location("gate_isolation", REPO_ROOT / "scripts" / "gate.py")
    assert spec is not None and spec.loader is not None
    gate = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = gate
    spec.loader.exec_module(gate)
    monkeypatch.delenv("AGENTCTL_REGISTRY", raising=False)
    monkeypatch.delenv("AGENTCTL_LAYOUT_AUTO_MIGRATE", raising=False)
    environment = gate._child_environment()
    assert environment["AGENTCTL_REGISTRY"] != "/live/.agentctl"
    assert "agentctl" in environment["AGENTCTL_REGISTRY"]
    assert environment["AGENTCTL_LAYOUT_AUTO_MIGRATE"] == "0"
    monkeypatch.setenv("AGENTCTL_REGISTRY", "/caller/chosen")
    assert gate._child_environment()["AGENTCTL_REGISTRY"] == "/caller/chosen"
