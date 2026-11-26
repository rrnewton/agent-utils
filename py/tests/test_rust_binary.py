"""The agentctl differential builds its Rust executables once per process."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from tests.rust_binary import main, rust_agentctl, rust_agentctl_with_fake


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


def test_build_step_builds_every_adapter_and_names_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # The DAG's python.agentctl.build-rust step (#2306) runs this module as a script, so the
    # shards' own calls find both builds current instead of compiling under their caps.
    release = tmp_path / "release"
    release.mkdir()
    executables = {name: release / name for name in ("agentctl", "chat-subscription-fake-plugin")}
    for executable in executables.values():
        executable.touch()
    packages: list[list[str]] = []

    def build(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        packages.append([command[i + 1] for i, item in enumerate(command) if item == "-p"])
        lines = [
            json.dumps({
                "reason": "compiler-artifact", "target": {"name": name, "kind": ["bin"]},
                "executable": str(path),
            })
            for name, path in executables.items()
        ]
        return subprocess.CompletedProcess(command, 0, "\n".join(lines))

    monkeypatch.setattr(subprocess, "run", build)
    rust_agentctl.cache_clear()
    rust_agentctl_with_fake.cache_clear()
    try:
        assert main() == 0
        assert packages == [["agentctl"], ["agentctl", "chat-subscription-fake"]]
        assert capsys.readouterr().out.splitlines() == [
            str(executables["agentctl"]),
            str(executables["agentctl"]),
            str(executables["chat-subscription-fake-plugin"]),
        ]
    finally:
        rust_agentctl.cache_clear()
        rust_agentctl_with_fake.cache_clear()


def test_build_step_failure_shows_the_compiler_diagnostics(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    diagnostic = json.dumps({
        "reason": "compiler-message",
        "message": {"rendered": "error[E0425]: cannot find value `x`\n"},
    })

    def fail(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        raise subprocess.CalledProcessError(
            101, command, output=diagnostic + "\nnot json\n", stderr="error: could not compile\n"
        )

    monkeypatch.setattr(subprocess, "run", fail)
    rust_agentctl.cache_clear()
    try:
        assert main() == 1
        stderr = capsys.readouterr().err
        assert "error[E0425]: cannot find value `x`" in stderr
        assert "error: could not compile" in stderr
        assert "Cargo exited 101" in stderr
    finally:
        rust_agentctl.cache_clear()
