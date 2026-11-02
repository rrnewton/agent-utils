"""The agentctl differential builds its Rust executables once per process."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from tests.rust_binary import rust_agentctl


def test_rust_agentctl_builds_once_per_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary = tmp_path / "explicit-target" / "release" / "agentctl"
    binary.parent.mkdir(parents=True)
    binary.touch()
    builds: list[list[str]] = []

    def build(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        assert kwargs["check"] is True
        assert command[command.index("--target-dir") + 1].endswith(
            "/validation-adapters/agentctl"
        )
        builds.append(command)
        return subprocess.CompletedProcess(command, 0, json.dumps({
            "reason": "compiler-artifact", "target": {"name": "agentctl", "kind": ["bin"]},
            "executable": str(binary),
        }))

    monkeypatch.setattr(subprocess, "run", build)
    rust_agentctl.cache_clear()
    try:
        assert rust_agentctl() == binary
        assert rust_agentctl() == binary
        assert len(builds) == 1
        # A new test process must check source freshness, even with an existing executable.
        rust_agentctl.cache_clear()
        assert rust_agentctl() == binary
        assert len(builds) == 2
    finally:
        rust_agentctl.cache_clear()


def test_rust_agentctl_failed_build_cannot_use_an_old_binary(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def fail(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        raise subprocess.CalledProcessError(1, command, stderr="compile failed")

    monkeypatch.setattr(subprocess, "run", fail)
    rust_agentctl.cache_clear()
    try:
        with pytest.raises(subprocess.CalledProcessError):
            rust_agentctl()
        assert rust_agentctl.cache_info().currsize == 0
    finally:
        rust_agentctl.cache_clear()
