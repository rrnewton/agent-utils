"""Herdr CLI adapter tests.

These tests stop at the subprocess boundary.  Every invocation is captured by an injected runner;
no real Herdr server or systemd user unit is contacted.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from collections.abc import Sequence

import pytest

import herdr_run.client as client_module
from herdr_run.client import CONTROL_TIMEOUT_SECONDS, HerdrClient, SERVER_UNIT
from herdr_run.errors import EXIT_UNAVAILABLE, HerdrUnavailable


def _completed(
    command: Sequence[str], *, returncode: int = 0, stdout: str = "", stderr: str = ""
) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(list(command), returncode, stdout, stderr)


class RecordingRunner:
    """Return scripted subprocess results while retaining the exact argv."""

    def __init__(self, responses: Sequence[tuple[int, str, str]]) -> None:
        self.responses = list(responses)
        self.calls: list[tuple[str, ...]] = []

    def __call__(self, command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        self.calls.append(tuple(command))
        if not self.responses:
            raise AssertionError(f"unexpected invocation: {list(command)!r}")
        returncode, stdout, stderr = self.responses.pop(0)
        return _completed(command, returncode=returncode, stdout=stdout, stderr=stderr)


def test_default_runner_bounds_control_subprocesses(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed_timeout = 0.0
    observed_start_new_session = False

    class FakeProcess:
        pid = 123
        returncode = 0

        def communicate(self, *, timeout: float) -> tuple[str, str]:
            nonlocal observed_timeout
            observed_timeout = timeout
            return "", ""

    def fake_popen(command: list[str], **kwargs: object) -> FakeProcess:
        nonlocal observed_start_new_session
        assert command == ["fixture-herdr", "status"]
        observed_start_new_session = kwargs.get("start_new_session") is True
        assert kwargs.get("text") is True
        assert kwargs.get("encoding") == "utf-8"
        assert kwargs.get("errors") == "replace"
        return FakeProcess()

    monkeypatch.setattr(subprocess, "Popen", fake_popen)
    client_module.default_runner(("fixture-herdr", "status"))
    assert observed_timeout == CONTROL_TIMEOUT_SECONDS
    assert observed_start_new_session is True


def test_control_timeout_terminates_descendants(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A timed-out wrapper must not leave a helper from its process group running."""
    pid_path = os.path.join(str(tmp_path), "descendant.pid")
    program = '/usr/bin/sleep 60 & child=$!; printf "%s" "$child" > "$1"; wait'
    with pytest.raises(subprocess.TimeoutExpired):
        client_module._bounded_control_command(
            ("/bin/sh", "-c", program, "timeout-probe", pid_path), timeout=0.5
        )

    with open(pid_path, encoding="ascii") as handle:
        descendant_pid = int(handle.read())
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        try:
            with open(f"/proc/{descendant_pid}/stat", encoding="ascii") as handle:
                stat_line = handle.read()
        except FileNotFoundError:
            return
        # A killed grandchild can briefly remain as a zombie until its subreaper collects it. It
        # is no longer executable and therefore satisfies the no-orphaned-work guarantee.
        state = stat_line.rpartition(") ")[2][:1]
        if state == "Z":
            return
        time.sleep(0.01)
    pytest.fail(f"timed-out descendant {descendant_pid} is still running")


def test_fixed_candidate_ignores_a_planted_caller_path(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A caller-controlled PATH entry must never become the binary sent to systemd."""
    root = str(tmp_path)
    account_home = os.path.join(root, "account-home")
    planted_dir = os.path.join(root, "planted-path")
    fixed_dir = os.path.join(account_home, "bin")
    os.makedirs(planted_dir)
    os.makedirs(fixed_dir)
    name = "herdr-run-fixed-candidate-test"
    planted = os.path.join(planted_dir, name)
    fixed = os.path.join(fixed_dir, name)
    for path in (planted, fixed):
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("#!/bin/sh\nexit 99\n")
        os.chmod(path, 0o755)

    runner = RecordingRunner(((0, '{"server":{"running":true}}', ""),))
    monkeypatch.setattr(
        HerdrClient, "_account_home", staticmethod(lambda: account_home)
    )
    monkeypatch.setattr(client_module, "default_runner", runner)

    client = HerdrClient(
        herdr_bin=name,
        environ={"HOME": os.path.join(root, "caller-home"), "PATH": planted_dir},
    )
    assert client.server_running() is True
    assert runner.calls == [(os.path.realpath(fixed), "status", "--json")]
    assert planted not in runner.calls[0]


def test_missing_fixed_candidate_is_typed_unavailable(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    account_home = os.path.join(str(tmp_path), "empty-home")
    os.makedirs(account_home)
    monkeypatch.setattr(
        HerdrClient, "_account_home", staticmethod(lambda: account_home)
    )

    def must_not_run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        raise AssertionError(
            f"missing executable unexpectedly invoked: {list(command)!r}"
        )

    monkeypatch.setattr(client_module, "default_runner", must_not_run)
    client = HerdrClient(herdr_bin="herdr-run-definitely-absent-test-binary")
    with pytest.raises(HerdrUnavailable, match="fixed install locations") as excinfo:
        client.workspace_id_for_label("workspace")
    assert excinfo.value.exit_code == EXIT_UNAVAILABLE


def test_production_timeout_is_typed_unavailable(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    account_home = os.path.join(str(tmp_path), "account-home")
    binary = os.path.join(account_home, "bin", "fixture-herdr")
    os.makedirs(os.path.dirname(binary))
    with open(binary, "w", encoding="utf-8") as handle:
        handle.write("#!/bin/sh\n")
    os.chmod(binary, 0o700)
    monkeypatch.setattr(
        HerdrClient, "_account_home", staticmethod(lambda: account_home)
    )

    def timeout_run(
        command: Sequence[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        raise subprocess.TimeoutExpired(list(command), CONTROL_TIMEOUT_SECONDS)

    monkeypatch.setattr(client_module, "_bounded_control_command", timeout_run)
    client = HerdrClient(herdr_bin="fixture-herdr")
    with pytest.raises(HerdrUnavailable, match="timed out"):
        client.workspace_id_for_label("workspace")


def test_os_error_from_subprocess_is_typed_unavailable() -> None:
    def missing(_command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        raise FileNotFoundError("fixture herdr is absent")

    client = HerdrClient(herdr_bin="fixture-herdr", run=missing)
    with pytest.raises(HerdrUnavailable, match="cannot invoke Herdr") as excinfo:
        client.workspace_id_for_label("workspace")
    assert excinfo.value.exit_code == EXIT_UNAVAILABLE


@pytest.mark.parametrize(
    "stdout",
    [
        "not json",
        "[]",
        "{}",
        '{"result":[]}',
        '{"result":{}}',
        '{"result":{"workspaces":"not-an-array"}}',
        '{"result":{"workspaces":[false]}}',
        '{"result":{"workspaces":[{"label":false,"workspace_id":"w1"}]}}',
        '{"result":{"workspaces":[{"label":"wanted","workspace_id":7}]}}',
    ],
)
def test_malformed_protocol_is_typed_unavailable(stdout: str) -> None:
    runner = RecordingRunner(((0, stdout, ""),))
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    with pytest.raises(HerdrUnavailable) as excinfo:
        client.workspace_id_for_label("wanted")
    assert excinfo.value.exit_code == EXIT_UNAVAILABLE


def test_nonzero_protocol_call_is_typed_unavailable_and_prefers_stderr() -> None:
    runner = RecordingRunner(((23, "less-useful stdout", "decisive stderr"),))
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    with pytest.raises(HerdrUnavailable, match="decisive stderr") as excinfo:
        client.workspace_id_for_label("wanted")
    assert excinfo.value.exit_code == EXIT_UNAVAILABLE


def test_broker_argv_is_exact_and_never_forwards_caller_path(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    result = json.dumps({"result": {"workspaces": []}})
    runner = RecordingRunner(((0, result, ""),))
    monkeypatch.setattr(
        HerdrClient, "_account_home", staticmethod(lambda: "/account/home")
    )
    client = HerdrClient(
        herdr_bin="fixture-herdr",
        broker="systemd-run",
        run=runner,
        environ={"HOME": "/caller/home", "PATH": "/planted/path"},
    )

    assert client.workspace_id_for_label("wanted") is None
    assert runner.calls == [
        (
            "systemd-run",
            "--user",
            "--wait",
            "--pipe",
            "--collect",
            "--quiet",
            "--setenv",
            "HOME=/account/home",
            "fixture-herdr",
            "workspace",
            "list",
        )
    ]
    assert all(not argument.startswith("PATH=") for argument in runner.calls[0])


def test_server_start_argv_is_exact_and_never_forwards_caller_path(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[str, ...]] = []
    statuses = iter((False, True))

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        calls.append(tuple(command))
        if tuple(command) == ("fixture-herdr", "status", "--json"):
            running = next(statuses)
            return _completed(
                command, stdout=json.dumps({"server": {"running": running}})
            )
        return _completed(command)

    monkeypatch.setattr(
        HerdrClient, "_account_home", staticmethod(lambda: "/account/home")
    )
    client = HerdrClient(
        herdr_bin="fixture-herdr",
        run=run,
        environ={"HOME": "/caller/home", "PATH": "/planted/path"},
        sleep=lambda _seconds: None,
    )

    assert client.ensure_server(attempts=1, delay=0.0) is True
    expected_launch = (
        "systemd-run",
        "--user",
        "--collect",
        "--unit",
        SERVER_UNIT,
        "--description",
        "herdr-run Herdr server (outside the agent sandbox)",
        "--setenv",
        "HOME=/account/home",
        "fixture-herdr",
        "server",
    )
    assert calls == [
        ("fixture-herdr", "status", "--json"),
        expected_launch,
        ("fixture-herdr", "status", "--json"),
    ]
    assert all(not argument.startswith("PATH=") for argument in expected_launch)


def test_workspace_creation_is_no_focus() -> None:
    response = json.dumps(
        {
            "result": {
                "workspace": {"workspace_id": "w1"},
                "tab": {"tab_id": "w1:t1"},
                "root_pane": {"pane_id": "w1:p1"},
            }
        }
    )
    runner = RecordingRunner(((0, response, ""),))
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_workspace(label="commands", cwd="/work") == (
        "w1",
        "w1:t1",
        "w1:p1",
    )
    assert runner.calls == [
        (
            "fixture-herdr",
            "workspace",
            "create",
            "--label",
            "commands",
            "--cwd",
            "/work",
            "--no-focus",
        )
    ]


def test_process_info_refuses_a_response_for_another_pane() -> None:
    response = json.dumps(
        {
            "result": {
                "process_info": {
                    "pane_id": "other",
                    "shell_pid": 7,
                    "foreground_process_group_id": 7,
                    "foreground_processes": [],
                }
            }
        }
    )
    client = HerdrClient(
        herdr_bin="fixture-herdr", run=RecordingRunner(((0, response, ""),))
    )
    with pytest.raises(HerdrUnavailable, match="expected 'wanted'"):
        client.process_info("wanted")


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("shell_pid", -1),
        ("shell_pid", 0),
        ("shell_pid", 2_147_483_648),
        ("foreground_process_group_id", -1),
        ("foreground_process_group_id", 0),
        ("foreground_process_group_id", 2_147_483_648),
        ("foreground_pid", -1),
        ("foreground_pid", 0),
        ("foreground_pid", 2_147_483_648),
    ],
)
def test_process_info_refuses_out_of_range_process_ids(
    field: str, value: int
) -> None:
    info: dict[str, object] = {
        "pane_id": "p",
        "shell_pid": 7,
        "foreground_process_group_id": 7,
        "foreground_processes": [{"pid": 7, "name": "sh", "cmdline": "sh"}],
    }
    if field == "foreground_pid":
        info["foreground_processes"] = [
            {"pid": value, "name": "sh", "cmdline": "sh"}
        ]
    else:
        info[field] = value
    response = json.dumps({"result": {"process_info": info}})
    client = HerdrClient(
        herdr_bin="fixture-herdr", run=RecordingRunner(((0, response, ""),))
    )
    with pytest.raises(HerdrUnavailable, match="positive Linux pid_t range"):
        client.process_info("p")


@pytest.mark.parametrize(
    "field", ["shell_pid", "foreground_process_group_id", "foreground_pid"]
)
def test_process_info_refuses_boolean_process_ids(field: str) -> None:
    info: dict[str, object] = {
        "pane_id": "p",
        "shell_pid": 7,
        "foreground_process_group_id": 7,
        "foreground_processes": [{"pid": 7, "name": "sh", "cmdline": "sh"}],
    }
    if field == "foreground_pid":
        info["foreground_processes"] = [
            {"pid": True, "name": "sh", "cmdline": "sh"}
        ]
    else:
        info[field] = True
    response = json.dumps({"result": {"process_info": info}})
    client = HerdrClient(
        herdr_bin="fixture-herdr", run=RecordingRunner(((0, response, ""),))
    )
    with pytest.raises(HerdrUnavailable, match="not an integer"):
        client.process_info("p")


def test_embedded_nul_identifier_is_typed_unavailable() -> None:
    def rejecting_runner(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        if any("\0" in argument for argument in command):
            raise ValueError("embedded null byte")
        raise AssertionError(f"expected a malformed identifier: {list(command)!r}")

    client = HerdrClient(herdr_bin="fixture-herdr", run=rejecting_runner)
    with pytest.raises(HerdrUnavailable, match="cannot invoke Herdr"):
        client.read("bad\0pane")


def test_filtered_pane_list_refuses_another_workspace() -> None:
    response = json.dumps(
        {
            "result": {
                "panes": [{"pane_id": "p1", "tab_id": "t1", "workspace_id": "other"}]
            }
        }
    )
    client = HerdrClient(
        herdr_bin="fixture-herdr", run=RecordingRunner(((0, response, ""),))
    )
    with pytest.raises(HerdrUnavailable, match="expected 'wanted'"):
        client.panes("wanted")
