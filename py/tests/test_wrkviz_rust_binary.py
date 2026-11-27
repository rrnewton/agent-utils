"""The wrkviz differential's shared Rust build helper (#2318)."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from tests.wrkviz_rust_binary import main, rust_wrkvizd


def _artifact(binary: Path) -> str:
    return json.dumps({
        "reason": "compiler-artifact", "target": {"name": "wrkvizd", "kind": ["bin"]},
        "executable": str(binary),
    })


def test_builds_a_private_locked_job_capped_target_once_per_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary = tmp_path / "release" / "wrkvizd"
    binary.parent.mkdir(parents=True)
    binary.touch()
    builds: list[tuple[list[str], dict[str, str]]] = []

    def build(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        assert kwargs["check"] is True
        env = kwargs["env"]
        assert isinstance(env, dict)
        builds.append((command, env))
        return subprocess.CompletedProcess(command, 0, _artifact(binary))

    monkeypatch.setattr(subprocess, "run", build)
    rust_wrkvizd.cache_clear()
    try:
        assert rust_wrkvizd() == binary
        assert rust_wrkvizd() == binary
        assert len(builds) == 1
        # A new test process must check source freshness, even with an existing executable.
        rust_wrkvizd.cache_clear()
        assert rust_wrkvizd() == binary
        assert len(builds) == 2
    finally:
        rust_wrkvizd.cache_clear()
    command, env = builds[0]
    assert command[:2] == ["cargo", "build"]
    assert "--locked" in command and "--release" in command
    assert command[command.index("-p") + 1] == "wrkviz"
    assert command[command.index("--bin") + 1] == "wrkvizd"
    assert command[command.index("--target-dir") + 1].endswith("/validation-adapters/wrkviz")
    assert env["CARGO_BUILD_JOBS"] == "2"


def test_a_failed_build_cannot_use_an_old_binary(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        raise subprocess.CalledProcessError(1, command, stderr="compile failed")

    monkeypatch.setattr(subprocess, "run", fail)
    rust_wrkvizd.cache_clear()
    try:
        with pytest.raises(subprocess.CalledProcessError):
            rust_wrkvizd()
        assert rust_wrkvizd.cache_info().currsize == 0
    finally:
        rust_wrkvizd.cache_clear()


def test_a_build_that_reports_no_executable_is_refused(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def build(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(
            command, 0, json.dumps({"reason": "build-finished", "success": True})
        )

    monkeypatch.setattr(subprocess, "run", build)
    rust_wrkvizd.cache_clear()
    try:
        with pytest.raises(RuntimeError, match="did not report a wrkvizd executable"):
            rust_wrkvizd()
    finally:
        rust_wrkvizd.cache_clear()


def test_build_step_builds_the_executable_and_names_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """``python.wrkviz.build-rust`` runs the helper as a script before the shards."""
    binary = tmp_path / "release" / "wrkvizd"
    binary.parent.mkdir(parents=True)
    binary.touch()
    monkeypatch.setattr(
        subprocess, "run",
        lambda command, **kwargs: subprocess.CompletedProcess(command, 0, _artifact(binary)),
    )
    rust_wrkvizd.cache_clear()
    try:
        assert main() == 0
    finally:
        rust_wrkvizd.cache_clear()
    assert capsys.readouterr().out == f"{binary}\n"


def test_build_step_failure_shows_the_compiler_diagnostics(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    rendered = "error[E0425]: cannot find value `x` in this scope\n"
    stdout = "\n".join([
        json.dumps({"reason": "compiler-message", "message": {"rendered": rendered}}),
        "not json",
        json.dumps({"reason": "build-finished", "success": False}),
    ])

    def fail(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        raise subprocess.CalledProcessError(
            101, command, output=stdout, stderr="error: could not compile `wrkviz`\n"
        )

    monkeypatch.setattr(subprocess, "run", fail)
    rust_wrkvizd.cache_clear()
    try:
        assert main() == 1
    finally:
        rust_wrkvizd.cache_clear()
    err = capsys.readouterr().err
    assert rendered in err
    assert "error: could not compile `wrkviz`" in err
    assert "Cargo exited 101" in err
