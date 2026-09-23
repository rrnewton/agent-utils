"""Exact Herdr allocation commands used by first-class agentctl launches."""
from __future__ import annotations

import json
import os
import shutil
import shlex
import subprocess
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path

from agentctl.client import (
    CustomProcessIdentity,
    HerdrClient,
    ProcessInfo,
    muse_prompt_in_composer,
    muse_prompt_in_transcript,
    muse_prompt_transcript_count,
    muse_startup_metadata,
)
from agentctl.errors import HerdrUnavailable
import pytest


class Runner:
    def __init__(self, response: dict[str, object]) -> None:
        self.response = response
        self.calls: list[list[str]] = []

    def __call__(self, command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        self.calls.append(list(command))
        return subprocess.CompletedProcess(
            command, 0, json.dumps({"result": self.response}), ""
        )


_ENVIRONMENT = (
    "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
    "LITERAL=a b=$(unexpanded)=tail",
)


def test_workspace_creation_passes_literal_environment_before_launch() -> None:
    runner = Runner({
        "workspace": {"workspace_id": "w1"},
        "tab": {"tab_id": "t1"},
        "root_pane": {"pane_id": "p1"},
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_workspace(
        label="subagents", cwd="/work/project", environment=_ENVIRONMENT
    ) == ("w1", "t1", "p1")
    assert runner.calls == [[
        "fixture-herdr", "workspace", "create", "--label", "subagents",
        "--cwd", "/work/project", "--env", _ENVIRONMENT[0],
        "--env", _ENVIRONMENT[1], "--no-focus",
    ]]


def test_tab_creation_passes_literal_environment_before_launch() -> None:
    runner = Runner({
        "tab": {"tab_id": "t1"},
        "root_pane": {"pane_id": "p1", "tab_id": "t1", "workspace_id": "w1"},
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_tab_with_pane(
        workspace_id="w1", label="worker", cwd="/work/project",
        environment=_ENVIRONMENT,
    ) == ("t1", "p1")
    assert runner.calls == [[
        "fixture-herdr", "tab", "create", "--workspace", "w1",
        "--label", "worker", "--cwd", "/work/project",
        "--env", _ENVIRONMENT[0], "--env", _ENVIRONMENT[1], "--no-focus",
    ]]


def _custom_runner(
    screen: str, *, observed_executable: str | None = None,
) -> tuple[HerdrClient, list[list[str]]]:
    calls: list[list[str]] = []
    executable = os.path.realpath("/bin/true")
    observed = observed_executable or executable

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        argv = list(command)
        calls.append(argv)
        if argv[1:3] == ["pane", "process-info"]:
            output = {"result": {"process_info": {
                "pane_id": "p1", "shell_pid": 10, "foreground_process_group_id": 11,
                "foreground_processes": [{
                    "pid": 2_147_483_647, "name": "true", "cmdline": f"{observed} literal",
                    "argv": [observed, "literal"], "executable": observed,
                }],
            }}}
            return subprocess.CompletedProcess(argv, 0, json.dumps(output), "")
        if argv[1:3] == ["pane", "read"]:
            return subprocess.CompletedProcess(argv, 0, screen, "")
        if argv[1:3] == ["pane", "get"]:
            output = {"result": {"pane": {
                "pane_id": "p1", "workspace_id": "w1", "cwd": "/work/project",
                "agent": "muse", "agent_status": "idle", "agent_session": None,
            }}}
            return subprocess.CompletedProcess(argv, 0, json.dumps(output), "")
        return subprocess.CompletedProcess(argv, 0, "", "")

    client = HerdrClient(herdr_bin="fixture-herdr", run=run)
    client._harness_executable = lambda _kind: executable  # type: ignore[assignment]
    return client, calls


def test_custom_muse_launch_uses_literal_shell_quoting_and_exact_pane_report() -> None:
    screen = (
        "Muse Code 1.3.0\n\n"
        "reasoning effort ultra is not available (gate ultra_reasoning_effort is closed); using xhigh\n"
        "────────────────\n❯\n────────────────\n"
        "watermelon-preview · xhigh · /work/project · Auto-review\n"
    )
    client, calls = _custom_runner(screen)
    arguments = ("--reasoning-effort", "ultra", 'literal $(unexpanded) "quotes"')
    client.start_pane_agent("worker", "muse", "p1", arguments, timeout=1)
    assert muse_startup_metadata(screen) == (
        "reasoning effort ultra is not available (gate ultra_reasoning_effort is closed); using xhigh",
        "xhigh",
    )
    pane_run = next(call for call in calls if call[1:3] == ["pane", "run"])
    assert shlex.split(pane_run[4]) == [os.path.realpath("/bin/true"), *arguments]
    report = next(call for call in calls if call[1:3] == ["pane", "report-agent"])
    assert report[3:] == [
        "p1", "--source", "agentctl", "--agent", "muse", "--state", "idle",
        "--message", "agentctl custom harness",
    ]


def test_muse_prompt_must_move_from_composer_to_transcript() -> None:
    prompt = "literal $(unexpanded) delivery\nsecond line"
    header = "Muse Code 1.3.0\n"
    divider = "────────────────\n"
    footer = "watermelon-preview · xhigh · /work/project · Auto-review\n"
    staged = header + divider + f"❯ {prompt}\n" + divider + footer
    accepted = (
        header + f"❯ {prompt}\nWorking...\n" + divider + "❯\n" + divider + footer
    )
    error_redraw = header + divider + f"❯ {prompt}\nError: retry\n" + divider + footer
    assert muse_prompt_in_composer(staged, prompt)
    assert not muse_prompt_in_transcript(staged, prompt)
    assert muse_prompt_in_transcript(accepted, prompt)
    assert not muse_prompt_in_composer(accepted, prompt)
    assert muse_prompt_in_composer(error_redraw, prompt)
    assert not muse_prompt_in_transcript(error_redraw, prompt)
    repeated_staged = (
        header + f"❯ {prompt}\n◆ prior answer\n" + divider
        + f"❯ {prompt}\n" + divider + footer
    )
    cleared_without_submit = (
        header + f"❯ {prompt}\n◆ prior answer\n" + divider + "❯\n" + divider + footer
    )
    assert muse_prompt_transcript_count(repeated_staged, prompt) == 1
    assert muse_prompt_transcript_count(cleared_without_submit, prompt) == 1


def test_custom_muse_launch_never_accepts_a_trust_prompt() -> None:
    client, calls = _custom_runner("Do you trust this workspace?\n❯ Yes\n  No\n")
    with pytest.raises(HerdrUnavailable, match="trust prompt.*no input"):
        client.start_pane_agent("worker", "muse", "p1", (), timeout=1)
    assert not any(call[1:3] == ["pane", "report-agent"] for call in calls)


def test_custom_muse_launch_rejects_bare_prompt_without_idle_footer() -> None:
    client, calls = _custom_runner("A choice dialog\n❯\n")
    with pytest.raises(HerdrUnavailable, match="verified idle composer"):
        client.start_pane_agent("worker", "muse", "p1", (), timeout=0.01)
    assert not any(call[1:3] == ["pane", "report-agent"] for call in calls)


def test_custom_harness_missing_executable_is_a_refusal() -> None:
    client = HerdrClient(herdr_bin="fixture-herdr", run=Runner({}))
    with pytest.raises(HerdrUnavailable, match="not found"):
        client._harness_executable("definitely-not-installed-agentctl-harness")


def test_custom_muse_explicit_executable_is_absolute_and_validated(tmp_path: Path) -> None:
    executable = tmp_path / "muse"
    shutil.copyfile("/bin/true", executable)
    executable.chmod(0o700)
    client = HerdrClient(
        herdr_bin="fixture-herdr",
        run=Runner({}),
        environ={"AGENTCTL_MUSE_BIN": str(executable)},
    )
    assert client._harness_executable("muse") == str(executable.resolve())

    relative = HerdrClient(
        herdr_bin="fixture-herdr",
        run=Runner({}),
        environ={"AGENTCTL_MUSE_BIN": "muse"},
    )
    with pytest.raises(HerdrUnavailable, match="must be an absolute path"):
        relative._harness_executable("muse")

    script = tmp_path / "muse-script"
    script.write_text("#!/bin/sh\n", encoding="utf-8")
    script.chmod(0o700)
    scripted = HerdrClient(
        herdr_bin="fixture-herdr", run=Runner({}),
        environ={"AGENTCTL_MUSE_BIN": str(script)},
    )
    with pytest.raises(HerdrUnavailable, match="native ELF"):
        scripted.start_pane_agent("worker", "muse", "p1", (), timeout=1)


def test_custom_harness_rejects_same_name_at_a_different_executable_path() -> None:
    client, _calls = _custom_runner(
        "────────────────\n❯\n────────────────\nAuto-review\n",
        observed_executable="/tmp/lookalike/true",
    )
    with pytest.raises(HerdrUnavailable, match="not the foreground process"):
        client.verify_custom_harness("p1", "muse")


def test_custom_harness_rejects_matching_report_when_kernel_executable_differs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, _calls = _custom_runner(
        "────────────────\n❯\n────────────────\nAuto-review\n",
    )
    monkeypatch.setattr(
        client, "_process_executable", lambda _pid: os.path.realpath("/bin/false")
    )
    with pytest.raises(HerdrUnavailable, match="not the foreground process"):
        client.verify_custom_harness("p1", "muse")


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_recorded_pane_shell_identity_rejects_every_generation_change() -> None:
    executable = os.path.realpath("/bin/bash")
    shell = subprocess.Popen(
        [executable, "--noprofile", "--norc"],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    try:
        foreground: dict[str, object] = {
            "pid": shell.pid,
            "name": "bash",
            "cmdline": executable,
            "argv": [executable],
            "executable": executable,
        }
        process_info: dict[str, object] = {
            "pane_id": "p1",
            "shell_pid": shell.pid,
            "foreground_process_group_id": shell.pid,
            "foreground_processes": [foreground],
        }
        runner = Runner({"process_info": process_info})
        client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
        identity = client.pane_shell_identity("p1")

        assert client.pane_is_same_idle_shell("p1", identity)
        mismatches = (
            replace(identity, boot_id="ffffffff-ffff-ffff-ffff-ffffffffffff"),
            replace(identity, pid=identity.pid + 1),
            replace(identity, starttime_ticks=identity.starttime_ticks + 1),
            replace(identity, executable_device=identity.executable_device + 1),
            replace(identity, executable_inode=identity.executable_inode + 1),
        )
        assert all(
            not client.pane_is_same_idle_shell("p1", item) for item in mismatches
        )

        foreground["argv"] = [os.path.realpath("/usr/bin/sleep")]
        assert not client.pane_is_same_idle_shell("p1", identity)
    finally:
        shell.terminate()
        shell.wait(timeout=5)


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_real_non_shell_process_cannot_be_adopted_as_a_pane_shell() -> None:
    executable = os.path.realpath("/usr/bin/sleep")
    process = subprocess.Popen([executable, "30"], start_new_session=True)
    try:
        process_info: dict[str, object] = {
            "pane_id": "p1",
            "shell_pid": process.pid,
            "foreground_process_group_id": process.pid,
            "foreground_processes": [{
                "pid": process.pid,
                "name": "sleep",
                "cmdline": f"{executable} 30",
                "argv": [executable, "30"],
                "executable": executable,
            }],
        }
        client = HerdrClient(
            herdr_bin="fixture-herdr", run=Runner({"process_info": process_info})
        )
        observed = client._process_identity(process.pid)
        assert observed is not None
        with pytest.raises(HerdrUnavailable, match="supported identity-bound shell"):
            client.pane_shell_identity("p1")
        assert not client.pane_is_idle_shell("p1")
        assert not client.pane_is_same_idle_shell("p1", observed[0])
    finally:
        process.terminate()
        process.wait(timeout=5)


def test_recorded_custom_process_identity_survives_atomic_executable_replacement(
    tmp_path: Path,
) -> None:
    executable = tmp_path / "muse"
    replacement = tmp_path / "muse.new"
    shutil.copyfile("/bin/sleep", executable)
    executable.chmod(0o700)
    client = HerdrClient(herdr_bin="fixture-herdr", run=Runner({}))
    first = subprocess.Popen([str(executable), "60"], start_new_session=True)
    second: subprocess.Popen[bytes] | None = None
    try:
        metadata = executable.stat()
        first_info = ProcessInfo(
            pane_id="p1", shell_pid=1, foreground_pgid=first.pid,
            foreground=((first.pid, "muse", str(executable), str(executable), None),),
        )
        identity = client._pane_process_identity(
            first_info, str(executable),
            launch_image=(metadata.st_dev, metadata.st_ino),
        )
        assert isinstance(identity, CustomProcessIdentity)
        assert client._pane_process_identity(first_info, str(executable)) == identity

        shutil.copyfile("/bin/sleep", replacement)
        replacement.chmod(0o700)
        os.replace(replacement, executable)

        assert os.readlink(f"/proc/{first.pid}/exe").endswith(" (deleted)")
        assert client._pane_process_identity(first_info, None, identity) == identity
        assert client._pane_process_identity(first_info, str(executable)) is None

        altered_identities = (
            replace(identity, pid=identity.pid + 1),
            replace(identity, starttime_ticks=identity.starttime_ticks + 1),
            replace(identity, boot_id="ffffffff-ffff-ffff-ffff-ffffffffffff"),
            replace(identity, executable_device=identity.executable_device + 1),
            replace(identity, executable_inode=identity.executable_inode + 1),
        )
        assert all(
            client._pane_process_identity(first_info, None, altered) is None
            for altered in altered_identities
        )
        wrong_group = ProcessInfo(
            pane_id="p1", shell_pid=1, foreground_pgid=first.pid + 1,
            foreground=first_info.foreground,
        )
        assert client._pane_process_identity(wrong_group, None, identity) is None
        duplicated = ProcessInfo(
            pane_id="p1", shell_pid=1, foreground_pgid=first.pid,
            foreground=first_info.foreground * 2,
        )
        assert client._pane_process_identity(duplicated, None, identity) is None

        second = subprocess.Popen([str(executable), "60"], start_new_session=True)
        second_info = ProcessInfo(
            pane_id="p1", shell_pid=1, foreground_pgid=second.pid,
            foreground=((second.pid, "muse", str(executable), str(executable), None),),
        )
        assert client._pane_process_identity(second_info, None, identity) is None
    finally:
        for process in (first, second):
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
