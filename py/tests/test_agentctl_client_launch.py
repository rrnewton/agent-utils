"""Exact Herdr allocation commands used by first-class agentctl launches."""
from __future__ import annotations

import errno
import json
import os
import shutil
import shlex
import signal
import subprocess
import sys
import time
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path

import agentctl.client as client_module
from agentctl.client import (
    claude_active_screen,
    claude_prompt_is_exact_composer,
    claude_prompt_transcript_count,
    claude_staged_composer,
    CustomProcessIdentity,
    HerdrClient,
    ProcessInfo,
    RuntimeGuard,
    muse_idle_composer,
    muse_prompt_in_composer,
    muse_prompt_in_transcript,
    muse_prompt_is_exact_composer,
    muse_prompt_transcript_count,
    muse_startup_metadata,
    muse_verified_process_composer,
    muse_verified_process_goal_paused,
    muse_verified_process_idle_composer,
    muse_verified_process_prompt_in_composer,
    muse_verified_process_prompt_is_exact_composer,
    muse_verified_process_prompt_transcript_count,
)
from agentctl.errors import HerdrUnavailable
from agentctl.procstat import parse_process_stat
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


@pytest.mark.parametrize(
    ("descriptor", "limit", "label"),
    [
        (1, client_module._CONTROL_STDOUT_BYTES, "stdout"),
        (2, client_module._CONTROL_STDERR_BYTES, "stderr"),
    ],
)
def test_control_capture_refuses_output_above_each_byte_bound(
    descriptor: int, limit: int, label: str,
) -> None:
    command = [
        sys.executable,
        "-c",
        (
            "import os\n"
            f"remaining = {limit} + 1\n"
            "chunk = b'x' * 65536\n"
            "while remaining:\n"
            f"    written = os.write({descriptor}, chunk[:min(remaining, len(chunk))])\n"
            "    remaining -= written\n"
        ),
    ]
    with pytest.raises(OSError, match=f"control {label} exceeds {limit} bytes"):
        client_module._bounded_control_command(command, timeout=10)


_ENVIRONMENT = (
    "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
    "LITERAL=a b=$(unexpanded)=tail",
)


def test_workspace_creation_passes_literal_environment_before_launch() -> None:
    runner = Runner({
        "workspace": {"workspace_id": "w1"},
        "tab": {"tab_id": "t1"},
        "root_pane": {"pane_id": "p1", "terminal_id": "term-1"},
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_workspace(
        label="subagents", cwd="/work/project", environment=_ENVIRONMENT
    ) == ("w1", "t1", "p1", "term-1")
    assert runner.calls == [[
        "fixture-herdr", "workspace", "create", "--label", "subagents",
        "--cwd", "/work/project", "--env", _ENVIRONMENT[0],
        "--env", _ENVIRONMENT[1], "--no-focus",
    ]]


def test_tab_creation_passes_literal_environment_before_launch() -> None:
    runner = Runner({
        "tab": {"tab_id": "t1"},
        "root_pane": {"pane_id": "p1", "tab_id": "t1", "workspace_id": "w1",
                      "terminal_id": "term-1"},
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.create_tab_with_pane(
        workspace_id="w1", label="worker", cwd="/work/project",
        environment=_ENVIRONMENT,
    ) == ("t1", "p1", "term-1")
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
    launched_argv: list[str] = [observed, "literal"]

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        argv = list(command)
        calls.append(argv)
        if argv[1:3] == ["pane", "process-info"]:
            output = {"result": {"process_info": {
                "pane_id": "p1", "shell_pid": 10, "foreground_process_group_id": 11,
                "terminal_id": "term-1",
                "foreground_processes": [{
                    "pid": 2_147_483_647, "name": "true", "cmdline": f"{observed} literal",
                    "argv": launched_argv, "executable": observed,
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
        if argv[1:3] == ["pane", "run"]:
            launched_argv[:] = shlex.split(argv[4])
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
    client.start_pane_agent(
        "worker", "muse", "p1", arguments,
        expected_terminal_id="term-1", timeout=1,
    )
    assert muse_startup_metadata(screen) == (
        "reasoning effort ultra is not available (gate ultra_reasoning_effort is closed); using xhigh",
        "xhigh",
    )
    pane_run = next(call for call in calls if call[1:3] == ["pane", "run"])
    assert shlex.split(pane_run[4]) == [os.path.realpath("/bin/true"), *arguments]
    assert pane_run[5:] == ["--expect-terminal-id", "term-1"]
    report = next(call for call in calls if call[1:3] == ["pane", "report-agent"])
    assert report[3:12] == [
        "p1", "--source", "agentctl", "--agent", "muse", "--state", "idle",
        "--expect-terminal-id", "term-1",
    ]
    assert report[12] == "--expect-process-generation"
    assert report[13].startswith(
        "v1:00000000-0000-0000-0000-000000000000:2147483647:"
    )
    assert report[14:] == ["--message", "agentctl custom harness"]


def test_custom_muse_launch_accepts_current_model_footer_without_auto_review() -> None:
    screen = (
        "Muse Code at Meta (https://fb.workplace.com/groups/27315719428107177)\n"
        "Using AI Gateway (Meta Model API upstream)\n\n"
        "  Muse Code 1.4.0\n\n"
        "────────────────\n❯\n────────────────\n"
        "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n"
    )
    client, calls = _custom_runner(screen)
    identity = client.start_pane_agent(
        "worker", "muse", "p1",
        ("--model", "kiki_gb300_mxfp8_6p2_840_nwr", "--reasoning-effort", "xhigh", "--yolo"),
        expected_terminal_id="term-1",
        timeout=1,
    )
    assert identity.pid == 2_147_483_647
    assert any(call[1:3] == ["pane", "report-agent"] for call in calls)


def test_current_muse_footer_does_not_turn_a_choice_into_an_idle_composer() -> None:
    screen = (
        "  Muse Code 1.4.0\n\n"
        "────────────────\n❯ Yes, continue\n  No, exit\n────────────────\n"
        "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n"
    )
    assert not muse_idle_composer(screen)
    assert not muse_idle_composer(screen.replace("❯ Yes, continue\n  No, exit", "❯\n  pending text"))


def test_verified_muse_paused_goal_is_structural_footer_state() -> None:
    divider = "─" * 40
    footer = "kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO"
    paused = (
        f"Muse Code 1.4.0\n{divider}\n❯\n{divider}\n"
        f"Goal (paused)\n{footer}\n"
    )
    transcript_echo = (
        f"Muse Code 1.4.0\nGoal (paused)\n{divider}\n❯\n{divider}\n{footer}\n"
    )
    assert muse_verified_process_goal_paused(paused)
    assert not muse_verified_process_goal_paused(transcript_echo)


def test_claude_active_screen_requires_current_activity_controls() -> None:
    divider = "─" * 40
    screen = (
        "completed output\n✻ Waiting for 1 background agent to finish\n"
        f"{divider}\n❯\n{divider}\n"
        "auto mode on · ← 2 agents · ↓ to manage\n"
        "● main\n◯ reviewer Checking tests 8m\n"
    )
    assert claude_active_screen(screen)
    assert claude_active_screen(screen.replace("Waiting", "Waited").replace(
        "↓ to manage", "esc to interrupt",
    ))
    assert not claude_active_screen(screen.replace("Waiting", "Waited"))
    assert not claude_active_screen(screen.replace("← 2 agents", "← 1 agent"))
    staged_while_active = (
        "● Background command still running\n"
        "✽ Considering… (20m 51s)\n"
        f"{divider}\n"
        "❯ queued follow-up prompt\n"
        "  ctrl+x ctrl+s to send now\n"
        f"{divider}\n"
        "⏵⏵ auto mode on · esc to interrupt · ← 2 agents\n"
    )
    assert claude_active_screen(staged_while_active)
    assert not claude_active_screen(
        screen.replace("❯", "old transcript\n❯", 1).replace(
            "✻ Waiting for 1 background agent to finish\n", "", 1,
        )
    )


def test_claude_staged_prompt_is_distinct_from_a_submitted_turn() -> None:
    divider = "─" * 40
    prompt = "review the deterministic- scheduling contract"
    staged = (
        "● Background command still running\n"
        f"{divider}\n"
        "❯ review the deterministic-\n"
        "  scheduling contract\n"
        "  ctrl+x ctrl+s to send now\n"
        f"{divider}\n"
        "⏵⏵ auto mode on · esc to interrupt · ← 2 agents\n"
    )
    assert claude_staged_composer(staged)
    assert claude_prompt_is_exact_composer(staged, prompt)
    assert claude_prompt_transcript_count(staged, prompt) == 0

    submitted = (
        f"❯ {prompt}\n"
        "● Working on the request\n"
        f"{divider}\n❯\n{divider}\n"
        "⏵⏵ auto mode on · esc to interrupt\n"
    )
    assert not claude_staged_composer(submitted)
    assert not claude_prompt_is_exact_composer(submitted, prompt)
    assert claude_prompt_transcript_count(submitted, prompt) == 1


@pytest.mark.parametrize(
    "footer",
    (
        "",
        "⏵⏵ auto mode on · esc to interrupt\n",
    ),
)
def test_claude_hintless_ruled_composer_never_counts_as_submitted(
    footer: str,
) -> None:
    divider = "─" * 40
    prompt = "do not mistake this staged text for a submitted turn"
    screen = f"{divider}\n❯ {prompt}\n{divider}\n{footer}"

    assert not claude_staged_composer(screen)
    assert not claude_prompt_is_exact_composer(screen, prompt)
    assert claude_prompt_transcript_count(screen, prompt) == 0

    prior = f"❯ {prompt}\n● Prior reply\n{screen}"
    assert claude_prompt_transcript_count(prior, prompt) == 1


def test_terminal_identity_capability_probe_is_read_only_exact_and_cached() -> None:
    calls: list[list[str]] = []

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        argv = list(command)
        calls.append(argv)
        return subprocess.CompletedProcess(
            argv,
            0,
            "options: --expect-terminal-id --expect-process-generation\n",
            "",
        )

    client = HerdrClient(herdr_bin="fixture-herdr", run=run)
    client.require_terminal_identity_support()
    client.require_terminal_identity_support()

    assert calls
    assert all(call[-1] == "--help" for call in calls)
    assert not any(call[1:3] in (["workspace", "create"], ["tab", "create"])
                   for call in calls)
    assert len(calls) == len({tuple(call) for call in calls})


def test_terminal_identity_capability_probe_refuses_missing_process_guard() -> None:
    calls: list[list[str]] = []

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        argv = list(command)
        calls.append(argv)
        return subprocess.CompletedProcess(
            argv, 0, "option: --expect-terminal-id\n", "",
        )

    client = HerdrClient(herdr_bin="fixture-herdr", run=run)
    with pytest.raises(HerdrUnavailable, match="atomic terminal/process generation"):
        client.require_terminal_identity_support()
    assert calls
    assert all(call[-1] == "--help" for call in calls)


def test_interactive_mutation_carries_terminal_and_process_generation() -> None:
    runner = Runner({})
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    identity = CustomProcessIdentity(
        version=1,
        boot_id="11111111-2222-3333-4444-555555555555",
        pid=41,
        starttime_ticks=42,
        executable_device=43,
        executable_inode=44,
    )
    guard = RuntimeGuard("terminal-generation", identity)

    client.send_text("pane-1", "literal text", guard=guard)

    assert runner.calls == [[
        "fixture-herdr", "pane", "send-text", "pane-1", "literal text",
        "--expect-terminal-id", "terminal-generation",
        "--expect-process-generation",
        "v1:11111111-2222-3333-4444-555555555555:41:42:43:44",
    ]]


def test_custom_muse_launch_retries_transient_null_process_argv() -> None:
    executable = os.path.realpath("/bin/true")
    process_probes = 0

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        nonlocal process_probes
        argv = list(command)
        if argv[1:3] == ["pane", "process-info"]:
            process_probes += 1
            process_argv: object = None if process_probes == 1 else [executable]
            output = {"result": {"process_info": {
                "pane_id": "p1", "shell_pid": 10,
                "terminal_id": "term-1",
                "foreground_process_group_id": 11,
                "foreground_processes": [{
                    "pid": 2_147_483_647, "name": "true", "cmdline": executable,
                    "argv": process_argv, "executable": executable,
                }],
            }}}
            return subprocess.CompletedProcess(argv, 0, json.dumps(output), "")
        if argv[1:3] == ["pane", "read"]:
            return subprocess.CompletedProcess(
                argv, 0,
                "Muse Code 1.4.0\n────────────────\n❯\n────────────────\n"
                "watermelon-preview · xhigh · /work/project · Auto-review\n",
                "",
            )
        return subprocess.CompletedProcess(argv, 0, "", "")

    client = HerdrClient(herdr_bin="fixture-herdr", run=run)
    client._harness_executable = lambda _kind: executable  # type: ignore[assignment]
    identity = client.start_pane_agent(
        "worker", "muse", "p1", (),
        expected_terminal_id="term-1", timeout=1,
    )
    assert identity.pid == 2_147_483_647
    assert process_probes >= 2


def test_custom_muse_process_probe_honors_remaining_startup_deadline(
    tmp_path: Path,
) -> None:
    herdr = tmp_path / "blocking-herdr"
    herdr.write_text(
        "#!/usr/bin/python3\n"
        "import sys, time\n"
        "if sys.argv[1:3] == ['pane', 'process-info']:\n"
        "    time.sleep(5)\n"
        "sys.exit(0)\n",
        encoding="utf-8",
    )
    herdr.chmod(0o700)
    client = HerdrClient(
        herdr_bin=str(herdr), environ={"AGENTCTL_MUSE_BIN": "/bin/true"},
    )
    started = time.monotonic()
    with pytest.raises(HerdrUnavailable, match="timed out|deadline"):
        client.start_pane_agent(
            "worker", "muse", "p1", (),
            expected_terminal_id="term-1", timeout=0.1,
        )
    assert time.monotonic() - started < 1.0


def test_muse_prompt_must_move_from_composer_to_transcript() -> None:
    prompt = "literal $(unexpanded) delivery\nsecond line"
    header = "Muse Code 1.3.0\n"
    divider = "────────────────\n"
    footer = "watermelon-preview · xhigh · /work/project · Auto-review\n"
    staged = header + divider + f"❯ {prompt}\n" + divider + footer
    accepted = (
        header + f"❯ {prompt}\n◆ Working...\n" + divider + "❯\n" + divider + footer
    )
    error_redraw = header + divider + f"❯ {prompt}\nError: retry\n" + divider + footer
    assert muse_prompt_in_composer(staged, prompt)
    assert not muse_prompt_in_transcript(staged, prompt)
    assert muse_prompt_in_transcript(accepted, prompt)
    assert not muse_prompt_in_composer(accepted, prompt)
    assert muse_prompt_in_composer(error_redraw, prompt)
    assert not muse_prompt_is_exact_composer(error_redraw, prompt)
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

    long_prompt = "prefix " + ("middle " * 40) + "suffix"
    changed_middle = "prefix " + ("changed " * 40) + "suffix"
    collapsed = header + divider + f"❯ [Pasted Content {len(long_prompt)} chars]\n" + divider + footer
    deceptive = header + divider + f"❯ {changed_middle}\n" + divider + footer
    prefixed = (
        header + f"❯ {long_prompt} extra\n◆ Working\n"
        + divider + "❯\n" + divider + footer
    )
    assert not muse_prompt_in_composer(collapsed, long_prompt)
    assert not muse_prompt_in_composer(deceptive, long_prompt)
    assert not muse_prompt_in_transcript(prefixed, long_prompt)


def test_headerless_muse_composer_requires_verified_process_context() -> None:
    screen = (
        "old transcript after the version header scrolled away\n"
        "────────────────\n❯\n────────────────\n"
        "kiki · xhigh · /work/project · YOLO\n"
    )
    assert not muse_idle_composer(screen)
    assert muse_verified_process_composer(screen)
    assert muse_verified_process_idle_composer(screen)
    prompt = "require the full deterministic-scheduling-review skill"
    staged = screen.replace("❯\n", f"❯ {prompt}\n")
    assert not muse_prompt_is_exact_composer(staged, prompt)
    assert muse_verified_process_prompt_is_exact_composer(staged, prompt)
    assert muse_verified_process_prompt_in_composer(staged, prompt)
    accepted = screen.replace(
        "old transcript after the version header scrolled away\n",
        f"❯ {prompt}\n◆ Working\n",
    )
    assert muse_verified_process_prompt_transcript_count(accepted, prompt) == 1


def test_exact_muse_composer_matches_only_requested_hyphen_soft_wraps() -> None:
    divider = "─" * 40
    footer = "kiki · xhigh · /work/project · YOLO\n"
    prompt = (
        "Use deterministic-scheduling-review and keep this buffered prompt "
        "byte-for-byte"
    )
    wrapped = (
        "old transcript\n"
        f"{divider}\n"
        "❯ Use deterministic-\n"
        "  scheduling-review and keep this buffered prompt byte-for-byte\n"
        f"{divider}\n{footer}"
    )
    assert muse_verified_process_composer(wrapped)
    assert muse_verified_process_prompt_is_exact_composer(wrapped, prompt)
    assert not muse_verified_process_prompt_is_exact_composer(
        wrapped, prompt.replace("byte-for-byte", "byte for byte"),
    )
    accepted = (
        "❯ Use deterministic-\n"
        "  scheduling-review and keep this buffered prompt byte-for-byte\n"
        "◆ Working\n"
        f"{divider}\n❯\n{divider}\n{footer}"
    )
    assert muse_verified_process_prompt_transcript_count(accepted, prompt) == 1

    # Only a physical boundary immediately after the same hyphen may omit the
    # normalization space.  This does not globally erase spaces around '-'.
    weakened = wrapped.replace("deterministic-\n", "deterministic -\n")
    assert not muse_verified_process_prompt_is_exact_composer(weakened, prompt)


def test_custom_muse_launch_never_accepts_a_trust_prompt() -> None:
    client, calls = _custom_runner("Do you trust this workspace?\n❯ Yes\n  No\n")
    with pytest.raises(HerdrUnavailable, match="trust prompt.*no input"):
        client.start_pane_agent(
            "worker", "muse", "p1", (),
            expected_terminal_id="term-1", timeout=1,
        )
    assert not any(call[1:3] == ["pane", "report-agent"] for call in calls)


def test_custom_muse_launch_rejects_bare_prompt_without_idle_footer() -> None:
    client, calls = _custom_runner("A choice dialog\n❯\n")
    with pytest.raises(HerdrUnavailable, match="verified idle composer"):
        client.start_pane_agent(
            "worker", "muse", "p1", (),
            expected_terminal_id="term-1", timeout=0.01,
        )
    assert not any(call[1:3] == ["pane", "report-agent"] for call in calls)


def test_custom_muse_launch_rejects_auto_review_without_versioned_header() -> None:
    client, calls = _custom_runner(
        "────────────────\n❯\n────────────────\n"
        "watermelon-preview · xhigh · /work/project · Auto-review\n"
    )
    with pytest.raises(HerdrUnavailable, match="verified idle composer"):
        client.start_pane_agent(
            "worker", "muse", "p1", (),
            expected_terminal_id="term-1", timeout=0.01,
        )
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
        scripted.start_pane_agent(
            "worker", "muse", "p1", (),
            expected_terminal_id="term-1", timeout=1,
        )


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
def test_legacy_recovery_uses_basename_only_with_exact_image_and_arguments() -> None:
    executable = os.path.realpath("/usr/bin/sleep")
    process = subprocess.Popen([executable, "30"], start_new_session=True)
    try:
        observed = HerdrClient._process_identity(process.pid)
        assert observed is not None
        identity, process_group, _path = observed
        response = {"result": {"process_info": {
            "pane_id": "p1", "shell_pid": 1,
            "terminal_id": "term-1",
            "foreground_process_group_id": process_group,
            "foreground_processes": [{
                "pid": process.pid, "name": "sleep", "cmdline": f"{executable} 30",
                "argv": [executable, "30"], "executable": executable,
            }],
        }}}
        client = HerdrClient(
            herdr_bin="fixture-herdr", run=Runner({"process_info": response["result"]["process_info"]})
        )
        assert client.recover_pane_agent(
            "p1", ("sleep", "30"), identity.executable_device,
            identity.executable_inode, process.pid,
            expected_terminal_id="term-1",
        ) == identity
        with pytest.raises(HerdrUnavailable, match="argv does not exactly match"):
            client.recover_pane_agent(
                "p1", ("sleep", "31"), identity.executable_device,
                identity.executable_inode, process.pid,
                expected_terminal_id="term-1",
            )
    finally:
        process.terminate()
        process.wait(timeout=5)


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_recovered_publication_detects_exit_after_commit_through_pinned_pidfd() -> None:
    executable = os.path.realpath("/usr/bin/sleep")
    process = subprocess.Popen([executable, "30"], start_new_session=True)
    try:
        observed = HerdrClient._process_identity(process.pid)
        assert observed is not None
        identity, process_group, _path = observed
        response = {"result": {"process_info": {
            "pane_id": "p1", "shell_pid": 1,
            "terminal_id": "term-1",
            "foreground_process_group_id": process_group,
            "foreground_processes": [{
                "pid": process.pid, "name": "sleep", "cmdline": executable,
                "argv": [executable, "30"], "executable": executable,
            }],
        }}}

        def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(command, 0, json.dumps(response), "")

        client = HerdrClient(herdr_bin="fixture-herdr", run=run)
        committed = False

        def commit() -> None:
            nonlocal committed
            committed = True
            process.kill()
            process.wait(timeout=5)

        with pytest.raises(HerdrUnavailable, match="exited"):
            client.commit_recovered_pane_agent(
                "p1", "muse", identity, commit,
                expected_terminal_id="term-1",
            )
        assert committed
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)


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


def test_process_stat_parser_treats_comm_as_opaque_bytes() -> None:
    fields = [b"S", b"123", b"888", b"999", *([b"0"] * 15), b"987654321"]
    raw = b"456 (p\xff)\n(\x80) " + b" ".join(fields) + b"\n"
    parsed = parse_process_stat(raw)
    assert parsed is not None
    assert (
        parsed.pid,
        parsed.state,
        parsed.ppid,
        parsed.pgrp,
        parsed.session,
        parsed.starttime,
    ) == (456, "S", 123, 888, 999, 987654321)


@pytest.mark.parametrize(
    "raw",
    [
        b"456 (unterminated S 1 2 3",
        b"456 (x)S 1 2 3",
        b"456 (x) S 1 2 3",
    ],
)
def test_process_stat_parser_refuses_missing_or_truncated_boundaries(raw: bytes) -> None:
    assert parse_process_stat(raw) is None


def test_descendant_census_accepts_an_unrelated_opaque_comm(tmp_path: Path) -> None:
    ready = tmp_path / "opaque-comm-ready"
    script = r'''
import ctypes
import pathlib
import sys
import time
libc = ctypes.CDLL(None, use_errno=True)
if libc.prctl(15, ctypes.c_char_p(b"p\xff)\n(\x80"), 0, 0, 0) != 0:
    raise OSError(ctypes.get_errno(), "PR_SET_NAME")
pathlib.Path(sys.argv[1]).write_text("ready")
time.sleep(60)
'''
    leaf = subprocess.Popen((os.path.realpath("/usr/bin/sleep"), "60"))
    unrelated = subprocess.Popen((sys.executable, "-c", script, str(ready)))
    try:
        deadline = time.monotonic() + 2
        while not ready.exists() and time.monotonic() < deadline:
            time.sleep(0.005)
        assert ready.exists()
        assert HerdrClient._process_has_no_descendants(leaf.pid)
    finally:
        leaf.kill()
        unrelated.kill()
        leaf.wait(timeout=2)
        unrelated.wait(timeout=2)


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_process_identity_maps_pidfd_close_failure_to_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    executable = os.path.realpath("/usr/bin/sleep")
    process = subprocess.Popen([executable, "30"], start_new_session=True)
    real_close = os.close
    injected = False

    def close_then_fail(descriptor: int) -> None:
        nonlocal injected
        real_close(descriptor)
        if not injected:
            injected = True
            raise OSError(errno.EIO, "injected pidfd close failure")

    try:
        monkeypatch.setattr("agentctl.client.os.close", close_then_fail)
        with pytest.raises(HerdrUnavailable, match="cannot close pidfd"):
            HerdrClient._process_identity(process.pid)
        assert injected is True
    finally:
        process.terminate()
        process.wait(timeout=5)


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_real_supported_shell_with_background_descendant_is_not_idle() -> None:
    executable = os.path.realpath("/bin/bash")
    shell = subprocess.Popen(
        [executable, "--noprofile", "--norc"], stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        text=True, start_new_session=True,
    )
    try:
        assert shell.stdin is not None
        shell.stdin.write("/usr/bin/sleep 30 & wait\n")
        shell.stdin.flush()
        def child_exists() -> bool:
            for entry in Path("/proc").iterdir():
                if not entry.name.isdigit():
                    continue
                try:
                    parsed = parse_process_stat((entry / "stat").read_bytes())
                except (FileNotFoundError, PermissionError):
                    continue
                if parsed is not None and parsed.ppid == shell.pid:
                    return True
            return False

        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not child_exists():
            time.sleep(0.01)
        assert child_exists(), "background child did not start"
        process_info: dict[str, object] = {
            "pane_id": "p1",
            "shell_pid": shell.pid,
            "foreground_process_group_id": shell.pid,
            "foreground_processes": [{
                "pid": shell.pid, "name": "bash", "cmdline": executable,
                "argv": [executable], "executable": executable,
            }],
        }
        client = HerdrClient(
            herdr_bin="fixture-herdr", run=Runner({"process_info": process_info})
        )
        identity = client.pane_shell_identity("p1")
        assert not client.pane_is_idle_shell("p1")
        assert not client.pane_is_same_idle_shell("p1", identity)
    finally:
        try:
            os.killpg(shell.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        shell.wait(timeout=5)


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
            foreground=((
                first.pid, "muse", str(executable), (str(executable), "60"), None,
            ),),
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
            foreground=((
                second.pid, "muse", str(executable), (str(executable), "60"), None,
            ),),
        )
        assert client._pane_process_identity(second_info, None, identity) is None
    finally:
        for process in (first, second):
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
