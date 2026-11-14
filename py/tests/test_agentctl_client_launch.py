"""Exact Herdr allocation commands used by first-class agentctl launches."""
from __future__ import annotations

import errno
import io
import json
import os
import select
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
    AgentIdentity, AgentPaneInfo, CustomProcessIdentity, Pane,
    HerdrClient,
    ProcessInfo,
    muse_auto_review_idle_composer,
    muse_idle_composer,
    muse_prompt_in_composer,
    muse_prompt_in_transcript,
    muse_prompt_is_exact_composer,
    muse_prompt_transcript_count,
    muse_startup_metadata,
    muse_verified_process_composer,
    muse_verified_process_goal_paused,
    muse_verified_process_idle_composer,
    muse_verified_process_paste_in_composer,
    muse_verified_process_paste_is_exact_composer,
    muse_verified_process_prompt_in_composer,
    muse_verified_process_prompt_is_exact_composer,
    muse_verified_process_prompt_transcript_count,
    muse_verified_process_turn_running,
)
from agentctl.errors import HerdrUnavailable
from agentctl.procstat import ProcessStat, parse_process_stat
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


def test_pane_move_captures_reassigned_identity_and_exact_destination() -> None:
    runner = Runner({
        "move_result": {
            "changed": True,
            "previous_pane_id": "w1:p1",
            "pane": {
                "pane_id": "w2:p2", "tab_id": "w2:t2", "workspace_id": "w2",
            },
        },
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    assert client.move_pane_to_new_tab(
        "w1:p1", workspace_id="w2", tab_label="worker",
    ) == Pane("w2:p2", "w2:t2", "w2")
    assert runner.calls == [[
        "fixture-herdr", "pane", "move", "w1:p1", "--new-tab",
        "--workspace", "w2", "--label", "worker", "--no-focus",
    ]]


def test_pane_move_refuses_response_for_different_destination() -> None:
    runner = Runner({
        "move_result": {
            "changed": True,
            "previous_pane_id": "w1:p1",
            "pane": {
                "pane_id": "w3:p2", "tab_id": "w3:t2", "workspace_id": "w3",
            },
        },
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    with pytest.raises(HerdrUnavailable, match="expected 'w2'"):
        client.move_pane_to_new_tab(
            "w1:p1", workspace_id="w2", tab_label="worker",
        )


@pytest.mark.parametrize(
    ("changed", "previous", "message"),
    ((False, "w1:p1", "no move occurred"),
     (True, "wrong", "previous pane identity")),
)
def test_pane_move_refuses_incomplete_transition_proof(
    changed: bool, previous: str, message: str,
) -> None:
    runner = Runner({
        "move_result": {
            "changed": changed,
            "previous_pane_id": previous,
            "pane": {
                "pane_id": "w2:p2", "tab_id": "w2:t2", "workspace_id": "w2",
            },
        },
    })
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    with pytest.raises(HerdrUnavailable, match=message):
        client.move_pane_to_new_tab(
            "w1:p1", workspace_id="w2", tab_label="worker",
        )


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


def test_current_muse_yolo_footer_is_an_idle_composer() -> None:
    screen = (
        "Muse Code at Meta (https://fb.workplace.com/groups/27315719428107177)\n"
        "Using AI Gateway (Meta Model API upstream)\n\n"
        "  Muse Code 1.4.0\n\n"
        "────────────────\n❯\n"
        "────────────────\n"
        "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n"
    )
    assert client_module.muse_idle_composer(screen)
    assert not muse_auto_review_idle_composer(
        screen.replace("Using AI Gateway", "Auto-review was previously enabled")
    )
    ambiguous = screen.replace(
        "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n",
        "  kiki · xhigh · /work/project · Auto-review\n"
        "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n",
    )
    assert not client_module.muse_idle_composer(ambiguous)
    assert not muse_auto_review_idle_composer(ambiguous)
    choice = screen.replace("❯\n", "❯ Yes, continue\n  No, exit\n")
    assert not client_module.muse_idle_composer(choice)


def test_a_paste_muse_wraps_inside_a_word_or_a_path_is_still_the_exact_editor() -> None:
    # Muse 1.4.14 wraps a long line at any character, inside a word or a path (#505
    # agentctl-muse-followup; rs/agentctl/testdata/muse-busy/muse-wrapped-draft.txt), so a
    # proof that joined rows only with a space never saw such a paste staged.
    def frame(editor: str) -> str:
        return ("────────────────\n" + editor + "\n────────────────\n"
                "  watermelon-full-preview · xhigh · /work/project · Launch overrides\n")
    token = "token-" + "abcdefghij" * 3 + " END"
    assert muse_verified_process_paste_is_exact_composer(
        frame("❯ token-abcdefghijabcde\n  fghijabcdefghij END"), token)
    assert muse_verified_process_paste_is_exact_composer(
        frame("❯ break/at/odd/\n  places"), "break/at/odd/places")
    assert muse_verified_process_paste_is_exact_composer(
        frame("❯ keep the self-\n  contained proof"), "keep the self-contained proof")
    assert not muse_verified_process_paste_is_exact_composer(
        frame("❯ break/at/odd/\n  places"), "break/at/odd/placez")


def test_the_issue_1323_wrapped_notice_is_the_exact_editor_and_a_submitted_copy_is_not() -> None:
    # #1323 muse-drawn-prompt-missed: the notice as Muse drew it — wrapped long lines,
    # `❯` on the first row, a two-space continuation indent, the status footer below.
    rule = "─" * 160
    footer = "  watermelon-full-preview · xhigh · /work/project · Launch overrides\n"
    prompt = (
        "[from coordinator via agentctl · reply: agentctl send coordinator]\n"
        "coordinator -> all lanes on this host: the data disk is low and another "
        "team's builds are frozen. Until further notice:\n"
        "(1) Run at most ONE local validate or build at a time per lane, and never "
        "in parallel nested worktrees.\n"
        "(2) As soon as a validate or gate finishes (never mid-validate), delete the "
        "regenerable build output it left in your slot or nested worktrees: cargo "
        "target/ dirs (rs/target, vibe-talk/target, etc.) and page build trees. If "
        "your next validate would rebuild them anyway, you may keep the ones in "
        "your MAIN slot checkout, but delete extra nested-worktree copies.\n"
        "No reply needed; just do it at your next safe point."
    )
    drawn = (
        "◆ prior answer\n" + rule + "\n"
        "❯ [from coordinator via agentctl · reply: agentctl send coordinator]\n"
        "  coordinator -> all lanes on this host: the data disk is low and another "
        "team's builds are frozen. Until further notice:\n"
        "  (1) Run at most ONE local validate or build at a time per lane, and never "
        "in parallel nested worktrees.\n"
        "  (2) As soon as a validate or gate finishes (never mid-validate), delete the "
        "regenerable build output it left in your slot or nested worktrees: cargo target/\n"
        "  dirs (rs/target, vibe-talk/target, etc.) and page build trees. If your next "
        "validate would rebuild them anyway, you may keep the ones in your MAIN slot\n"
        "  checkout, but delete extra nested-worktree copies.\n"
        "  No reply needed; just do it at your next safe point.\n"
        + rule + "\n" + footer
    )
    assert muse_verified_process_paste_is_exact_composer(drawn, prompt)
    assert client_module.muse_verified_process_paste_in_composer(drawn, prompt)
    # A really empty editor holds none of it: the late_paste refusal stands.
    empty = "◆ prior answer\n" + rule + "\n❯\n" + rule + "\n" + footer
    assert not client_module.muse_verified_process_paste_in_composer(empty, prompt)
    # The same prompt submitted — a user turn above an empty editor — is not a draft
    # in the editor, and the pane does not read staged.
    submitted = drawn.replace(
        "◆ prior answer\n" + rule + "\n❯ ", "❯ ", 1,
    ).replace(
        "\n" + rule + "\n" + footer,
        "\n\n◆ Noted. Standing by.\n\n" + rule + "\n❯\n" + rule + "\n" + footer,
        1,
    )
    assert not client_module.muse_verified_process_paste_in_composer(submitted, prompt)
    assert client_module.muse_verified_process_prompt_transcript_count(submitted, prompt) == 1


def test_a_bare_slash_command_under_its_open_menu_is_the_staged_paste() -> None:
    # Measured live on Muse 1.4.16 (#1136 raw-message-bridge): a pasted bare slash
    # command opens the command menu under the editor line, so the editor is the
    # command plus its menu rows, never the command alone; Enter runs the menu's
    # first row. Only that shape, with the typed command as the first row, is staged.
    def frame(editor: str) -> str:
        return ("─" * 40 + "\n" + editor + "\n" + "─" * 40 + "\n"
                "m · xhigh · /w · YOLO\n")
    assert muse_verified_process_paste_is_exact_composer(
        frame("❯ /help\n  /help  Show help"), "/help")
    # The command alone (menu already closed) is still the literal paste.
    assert muse_verified_process_paste_is_exact_composer(
        frame("❯ /help"), "/help")
    # A mere prefix names no command: Enter would run the highlighted row, not this.
    assert not muse_verified_process_paste_is_exact_composer(
        frame("❯ /h\n  /help  Show help"), "/h")
    # The typed command must be the menu's first row, the one Enter runs.
    assert not muse_verified_process_paste_is_exact_composer(
        frame("❯ /help\n  /history  Show history\n  /help  Show help"), "/help")
    # A command with arguments is not staged by its menu.
    assert not muse_verified_process_paste_is_exact_composer(
        frame("❯ /goal clear\n  /goal  Set the goal"), "/goal clear")
    # Ordinary text over slash-led rows is not a command.
    assert not muse_verified_process_paste_is_exact_composer(
        frame("❯ hello\n  /help  Show help"), "hello")
    # A menu row that does not open with a slash is not the command menu.
    assert not muse_verified_process_paste_is_exact_composer(
        frame("❯ /model\n  watermelon-full-preview"), "/model")


def test_resumed_headerless_frame_with_startup_warnings_is_idle() -> None:
    # #946: a resumed Muse scrolls its header off screen and prints the
    # abnormal-end and effort-fallback warnings; the pinned-process proof
    # must still read the idle composer, including when a benign startup
    # line lands in the footer region.
    screen = (
        "◆ prior answer\n"
        "warning: the previous run of this session ended abnormally around "
        "2026-11-06 10:09 UTC-08:00 (no clean shutdown was recorded). It was resumed\n"
        "cleanly.\n"
        "reasoning effort ultra is not available (gate ultra_reasoning_effort is "
        "closed); using xhigh\n"
        "────────────────\n❯\n"
        "────────────────\n"
        "  watermelon-full-preview · xhigh · /work/project · YOLO\n"
        "Skills: 82 loaded · 1 warning · 3 details hidden (ctrl+o to expand)\n"
    )
    assert not client_module.muse_idle_composer(screen)
    assert muse_verified_process_idle_composer(screen)
    busy = screen.replace(
        "◆ prior answer\n", "◆ prior answer\n◇ Thinking (3s · esc to interrupt)\n")
    assert not muse_verified_process_idle_composer(busy)
    assert not muse_verified_process_idle_composer(screen.replace("❯\n", "❯ draft\n"))
    doubled = screen.replace(
        "Skills: 82 loaded",
        "other · xhigh · /work/project · YOLO\nSkills: 82 loaded")
    assert not muse_verified_process_idle_composer(doubled)


def test_muse_launch_overrides_footer_is_an_idle_composer() -> None:
    # Captured 2026-10-19 from a live Muse launched with extra harness
    # arguments (--approval-mode never among them): the footer reports
    # Launch overrides, and this frame timed out start as launch_failed
    # (#382 muse-approval-never-ready).
    screen = (
        "Muse Code 1.4.0\n"
        "◆ prior answer\n\n"
        "────────────────\n❯\n"
        "────────────────\n"
        "  watermelon-full-preview · xhigh · /work/project · Launch overrides\n"
    )
    assert client_module.muse_idle_composer(screen)
    assert muse_verified_process_idle_composer(screen)
    assert not muse_auto_review_idle_composer(screen)
    ambiguous = screen.replace(
        "Launch overrides\n",
        "Launch overrides\n  kiki · xhigh · /work/project · YOLO\n",
    )
    assert not client_module.muse_idle_composer(ambiguous)
    choice = screen.replace("❯\n", "❯ Yes, continue\n  No, exit\n")
    assert not client_module.muse_idle_composer(choice)


_MUSE_FRAMES = Path(__file__).resolve().parents[2] / "rs/agentctl/testdata/muse-frames.json"


def _captured_muse() -> tuple[dict[str, str], dict[str, str]]:
    """The captured frames of rs/agentctl/testdata/muse-frames.json and the exact texts pasted."""
    corpus = json.loads(_MUSE_FRAMES.read_text(encoding="utf-8"))
    return corpus["frames"], corpus["briefs"]


def test_captured_launch_overrides_frames_carry_the_delivery_proofs() -> None:
    # Muse 1.4.12 launched with --trust-workspace --approval-mode never (#395
    # muse-launch-overrides-input): an empty editor, then a 3214-character brief that Muse
    # collapses to a placeholder, then the submitted turn.
    frames, briefs = _captured_muse()
    brief = briefs["long"]
    idle = frames["launch-overrides-idle"]
    assert client_module.muse_idle_composer(idle)
    assert muse_verified_process_idle_composer(idle)
    assert not muse_auto_review_idle_composer(idle)
    # The same editor under the Auto-review footer.
    auto_review = idle.replace("· Launch overrides", "· Auto-review")
    assert muse_auto_review_idle_composer(auto_review)
    assert muse_verified_process_idle_composer(auto_review)

    staged = frames["launch-overrides-placeholder"]
    assert "[Pasted Content 3214 chars]" in staged
    assert client_module._muse_paste_length(brief) == 3214
    # Muse counts a tab as 4 and drops other C0 controls and DEL (#1085
    # muse-send-staged-unsubmitted), as the Rust edition does.
    body = "x" * 1700
    for text, shown in [
        ("\t" + body, 1704),
        ("a\tb\t" + body, 1710),
        (body + "\r\nq", 1702),
        (body + "\rq", 1702),
        (body + "\x0cq", 1701),
        (body + "\x07q", 1701),
        (body + "\x7fq", 1701),
    ]:
        assert client_module._muse_paste_length(text) == shown
    assert muse_verified_process_paste_is_exact_composer(staged, brief)
    assert muse_verified_process_paste_in_composer(staged, brief)
    # The literal-text proofs cannot see a collapsed paste.
    assert not muse_prompt_is_exact_composer(staged, brief)
    # The count must be exact.
    assert not muse_verified_process_paste_is_exact_composer(staged, brief[1:])
    assert not muse_verified_process_paste_is_exact_composer(staged, brief + "x")
    assert not muse_verified_process_idle_composer(staged)
    assert muse_verified_process_composer(staged)
    assert muse_verified_process_prompt_transcript_count(staged, brief) == 0

    # Right after Enter a hook notice shares the footer line, and the turn ends at Muse's
    # progress marker instead of an answer.
    submitted = frames["launch-overrides-submitted"]
    assert "Launch overrides   Hook · UserPromptSubmit" in submitted
    assert "◈ Thinking (3s · esc to interrupt)" in submitted
    assert muse_verified_process_prompt_transcript_count(submitted, brief) == 1
    assert not muse_verified_process_paste_in_composer(submitted, brief)
    # The empty editor below Muse's progress line is a running turn, never idle input.
    assert not client_module.muse_idle_composer(submitted)
    assert not muse_verified_process_idle_composer(submitted)
    assert client_module.muse_verified_process_turn_running(submitted)
    assert not muse_auto_review_idle_composer(
        submitted.replace("· Launch overrides", "· Auto-review"),
    )
    double_checking = submitted.replace(
        "◈ Thinking (3s · esc to interrupt)", "◈ Double checking (1s · esc to interrupt)",
    )
    assert not muse_verified_process_idle_composer(double_checking)
    # A finished answer below the turn leaves the editor idle again.
    assert client_module.muse_idle_composer(
        submitted.replace("◈ Thinking (3s · esc to interrupt)", "◆ RECEIVED 40"),
    )
    assert muse_verified_process_prompt_transcript_count(
        submitted.replace("◈ Thinking", "Still thinking"), brief,
    ) == 0

    answered = frames["launch-overrides-answered"]
    assert muse_prompt_transcript_count(answered, brief) == 1
    assert muse_verified_process_prompt_transcript_count(answered, brief) == 1
    assert not muse_verified_process_paste_in_composer(answered, brief)
    assert client_module.muse_idle_composer(answered)


def test_captured_headerless_frames_count_pastes_in_code_points() -> None:
    # After a few turns Muse 1.4 has scrolled its versioned header out of the frame, so only
    # the verified-process proofs, which rely on the pinned process instead, can apply.
    frames, briefs = _captured_muse()
    staged = frames["headerless-placeholder"]
    brief = briefs["headerless"]
    assert "Muse Code 1." not in staged
    empty = staged.replace("❯ [Pasted Content 2939 chars]", "❯")
    assert not client_module.muse_idle_composer(empty)
    assert muse_verified_process_idle_composer(empty)
    assert muse_verified_process_paste_is_exact_composer(staged, brief)

    # Read while Muse's paste hint shared the footer line.
    hinted = frames["headerless-placeholder-paste-hint"]
    hint_brief = briefs["paste-hint"]
    assert "Launch overrides             Paste again to expand for editing" in hinted
    assert muse_verified_process_paste_is_exact_composer(hinted, hint_brief)
    assert muse_verified_process_composer(hinted)
    assert not muse_verified_process_idle_composer(hinted)
    # The footer is structural: whatever follows a gap of three spaces is a notice, and the text
    # before it needs three non-empty fields, whatever they say.
    footer = "watermelon-full-preview · xhigh · ~/work/project · Launch overrides"
    for line in (
        f"{footer}   Paste again to expand for editing",
        f"{footer}   Goal (paused)",
        f"{footer}   Paste again to expand",
        f"{footer}   Hook",
        f"{footer}X   anything at all",
    ):
        assert client_module._muse_status_mode(line) is not None, line
    assert client_module._muse_status_mode(
        "model · high · /work · YOLO\u3000\t Hook · UserPromptSubmit"
    ) == "YOLO"
    for line in (
        "   Paste again to expand for editing",
        "Goal (blocked · 3m) 95% █████████▌",
        "◇ Thinking (16s · esc to interrupt)",
        "model · · /work · YOLO",
        "model · xhigh",
        "Goal (paused)",
        "",
    ):
        assert client_module._muse_status_mode(line) is None, line

    # 3175 code points, 3824 bytes and 3234 UTF-16 units with one CRLF: Muse shows 3174.
    unicode = briefs["unicode-crlf"]
    assert len(unicode) == 3175 and unicode.count("\r\n") == 1
    staged = frames["headerless-unicode-placeholder"]
    assert "[Pasted Content 3174 chars]" in staged
    assert muse_verified_process_paste_is_exact_composer(staged, unicode)
    assert muse_verified_process_paste_is_exact_composer(staged, unicode.replace("\r\n", "\n"))
    assert not muse_verified_process_paste_is_exact_composer(
        staged, unicode.replace("\r\n", "\n\n"),
    )
    answered = frames["headerless-unicode-answered"]
    assert muse_prompt_transcript_count(answered, unicode) == 0
    assert muse_verified_process_prompt_transcript_count(answered, unicode) == 1
    assert not muse_verified_process_paste_in_composer(answered, unicode)


def test_a_running_muse_turn_is_never_an_idle_composer() -> None:
    # #678 muse-bridge-latency: Muse 1.4's progress row animates through ◆ ◇ ◈, and a
    # foreground command draws its hint below it, so neither the glyph nor the last row decides.
    footer = "  watermelon-full-preview · xhigh · /work/project · Launch overrides\n"
    rule = "────────────────\n"
    for glyph in ("◆", "◇", "◈"):
        for below in ("", "└ (ctrl+b to send to background)\n"):
            screen = (
                f"Muse Code 1.4.15\n❯ run the step\n"
                f"{glyph} Running command · Sleep — running (15s · esc to interrupt)\n"
                f"{below}{rule}❯\n{rule}{footer}"
            )
            assert not client_module.muse_idle_composer(screen), (glyph, below)
            assert not muse_verified_process_idle_composer(screen), (glyph, below)
    finished = f"Muse Code 1.4.15\n❯ run the step\n◆ Worked for 2m 00s\n{rule}❯\n{rule}{footer}"
    assert client_module.muse_idle_composer(finished)


def test_a_muse_terminals_panel_row_is_never_a_second_footer() -> None:
    # #678 muse-bridge-latency: a sub-agent's panel row has `·` fields like a footer.
    rule = "────────────────\n"
    screen = (
        "❯ count the docs\n◆ Worked for 40s\n"
        f"{rule}❯\n{rule}"
        "main · ↓ to select\n"
        "└ ◇ counter ·3  shared · Run the shell command `sleep 30`, then count  0s\n"
        "  watermelon-full-preview · medium · /work/project · Launch overrides\n"
    )
    assert muse_verified_process_idle_composer(screen)


def test_verified_headerless_muse_composer_distinguishes_buffered_input() -> None:
    divider = "─" * 40
    footer = "kiki · xhigh · /work/project · YOLO\n"
    idle = f"old transcript\n{divider}\n❯\n{divider}\n{footer}"
    assert not client_module.muse_idle_composer(idle)
    assert muse_verified_process_idle_composer(idle)
    assert muse_verified_process_composer(idle)

    prompt = "require the full deterministic-scheduling-review skill"
    staged = idle.replace("❯\n", f"❯ {prompt}\n")
    assert not muse_prompt_is_exact_composer(staged, prompt)
    assert muse_verified_process_prompt_is_exact_composer(staged, prompt)
    assert muse_verified_process_prompt_in_composer(staged, prompt)
    accepted = idle.replace(
        "old transcript\n", f"❯ {prompt}\n◆ Working\n",
    )
    assert muse_verified_process_prompt_transcript_count(accepted, prompt) == 1


def test_verified_muse_paused_goal_requires_footer_region() -> None:
    divider = "─" * 40
    footer = "kiki · xhigh · /work/project · YOLO"
    paused = (
        f"Muse Code 1.4.0\n{divider}\n❯\n{divider}\n"
        f"Goal (paused)\n{footer}\n"
    )
    transcript_echo = (
        f"Muse Code 1.4.0\nGoal (paused)\n{divider}\n❯\n{divider}\n{footer}\n"
    )
    assert muse_verified_process_goal_paused(paused)
    assert not muse_verified_process_goal_paused(transcript_echo)


# Footers read from live Muse panes with an active goal (2026-10-28): Muse right-aligns the
# goal's elapsed time, state and progress bar on the footer line itself.
_GOAL_FOOTERS = (
    "watermelon-full-preview · xhigh · ~/work/project · YOLO          Goal (4h 28m) 86% ████████▋░",
    "watermelon-full-preview · xhigh · ~/work/project · Auto-review"
    + " " * 77 + "Goal (blocked · 3m) 95% █████████▌",
    "watermelon-full-preview · xhigh · ~/work/project · YOLO   Goal (45m) 47% ████▊░░░░░",
    "watermelon-full-preview · xhigh · ~/work/project · YOLO   Goal (13m) 34% ███▍░░░░░░",
)


def test_an_active_goal_notice_on_the_footer_leaves_the_editor_readable() -> None:
    divider = "─" * 40
    for footer in _GOAL_FOOTERS:
        idle = f"◆ prior answer\n{divider}\n❯\n{divider}\n  {footer}\n"
        assert muse_verified_process_idle_composer(idle), footer
        assert muse_idle_composer(f"Muse Code 1.4.0\n{idle}"), footer
        assert not muse_verified_process_goal_paused(idle), footer
        running = idle.replace(
            "◆ prior answer\n", "◆ prior answer\n◇ Thinking (16s · esc to interrupt)\n",
        )
        assert not muse_verified_process_idle_composer(running), footer
        assert muse_verified_process_turn_running(running), footer
        doubled = idle.replace(f"  {footer}\n", f"  {footer}\n  kiki · high · /work · YOLO\n")
        assert not muse_verified_process_idle_composer(doubled), footer
    quoted = f"{_GOAL_FOOTERS[0]}\n{divider}\n❯\n{divider}\n"
    assert not muse_verified_process_idle_composer(quoted)
    alone = f"old\n{divider}\n❯\n{divider}\nGoal (45m) 47% ████▊░░░░░\n"
    assert not muse_verified_process_idle_composer(alone)


def test_a_footer_with_an_unknown_model_effort_or_mode_is_still_a_footer() -> None:
    divider = "─" * 40
    for footer in (
        "some-future-model-v9 · turbo-max · /work/project · Dangerously skip everything",
        "GPT-6.1 Sol ultra · ultra · ~/work/project · YOLO",
        "m · e · /d",
        "m · e · /d · Mode · and another field",
    ):
        idle = f"old transcript\n{divider}\n❯\n{divider}\n{footer}\n"
        assert muse_verified_process_idle_composer(idle), footer
    for not_footer in ("model · · /work · YOLO", "model · xhigh", "model with spaces · E · /w · "):
        idle = f"old transcript\n{divider}\n❯\n{divider}\n{not_footer}\n"
        assert not muse_verified_process_idle_composer(idle), not_footer


def test_a_paused_goal_on_the_footer_line_still_pauses() -> None:
    divider = "─" * 40
    footer = "kiki · xhigh · /work/project · YOLO"
    paused = f"old\n{divider}\n❯\n{divider}\n{footer}   Goal (paused)\n"
    assert muse_verified_process_goal_paused(paused)
    active = paused.replace("Goal (paused)", "Goal (45m) 47% ████▊░░░░░")
    assert not muse_verified_process_goal_paused(active)
    assert muse_verified_process_idle_composer(active)


def test_muse_prompt_must_move_from_composer_to_transcript() -> None:
    prompt = "literal $(unexpanded) delivery\nsecond line"
    header = "Muse Code 1.3.0\n"
    divider = "────────────────\n"
    footer = "watermelon-preview · xhigh · /work/project · Auto-review\n"
    staged = header + divider + f"❯ {prompt}\n" + divider + footer
    accepted = (
        header + f"❯ {prompt}\nWorking...\n" + divider + "❯\n" + divider + footer
    )
    accepted_current = (
        header + f"❯ {prompt}\n◆ Working...\n" + divider + "❯\n" + divider + footer
    )
    error_redraw = header + divider + f"❯ {prompt}\nError: retry\n" + divider + footer
    assert muse_prompt_in_composer(staged, prompt)
    assert not muse_prompt_in_transcript(staged, prompt)
    assert muse_prompt_in_transcript(accepted, prompt)
    assert muse_prompt_in_transcript(accepted_current, prompt)
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


def _suggestion13_process_response(foreground: dict[str, object]) -> dict[str, object]:
    return {"process_info": {
        "pane_id": "p1", "shell_pid": 10, "foreground_process_group_id": 11,
        "foreground_processes": [foreground],
    }}


@pytest.mark.parametrize("variant", ["absent", "null", "empty"])
def test_suggestion13_missing_foreground_argv_remains_unknown_metadata(variant: str) -> None:
    foreground: dict[str, object] = {
        "pid": 2_147_483_647, "name": "muse", "cmdline": os.path.realpath("/bin/true"),
        "executable": os.path.realpath("/bin/true"),
    }
    if variant != "absent":
        foreground["argv"] = None if variant == "null" else []
    client = HerdrClient(herdr_bin="fixture-herdr", run=Runner(_suggestion13_process_response(foreground)))
    info = client.process_info("p1")
    assert info.foreground == ((2_147_483_647, "muse", os.path.realpath("/bin/true"), "", os.path.realpath("/bin/true")),)
    metadata = os.stat(os.path.realpath("/bin/true"))
    assert client._pane_process_identity(info, os.path.realpath("/bin/true"),
        launch_image=(metadata.st_dev, metadata.st_ino)) is None
    assert client._idle_shell_proof(info) is None


@pytest.mark.parametrize("argv", ["/bin/true", 7, {}, [None], [""], ["/bin/true", 7], ["/bad\0path"]])
def test_suggestion13_supplied_malformed_foreground_argv_still_refuses(argv: object) -> None:
    client = HerdrClient(herdr_bin="fixture-herdr", run=Runner(_suggestion13_process_response({
        "pid": 11, "name": "muse", "cmdline": "/bin/true", "argv": argv,
    })))
    with pytest.raises(HerdrUnavailable, match="foreground process argv"):
        client.process_info("p1")


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_suggestion13_unknown_argv_can_verify_only_kernel_generation_or_opened_launch_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    executable = tmp_path / "muse"
    shutil.copyfile("/usr/bin/sleep", executable)
    executable.chmod(0o700)
    process = subprocess.Popen([str(executable), "30"], start_new_session=True)
    try:
        foreground: dict[str, object] = {
            "pid": process.pid, "name": "untrusted-name", "cmdline": "untrusted-command", "argv": None,
            "executable": "/untrusted/reported-image",
        }
        response = _suggestion13_process_response(foreground)
        process_info = response["process_info"]
        assert isinstance(process_info, dict)
        process_info["foreground_process_group_id"] = process.pid
        runner = Runner(response)
        client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
        info = client.process_info("p1")
        identity = client.harness_identity("p1", "muse")
        assert identity is not None and client.verify_harness_identity("p1", identity)
        assert client._pane_process_identity(info, str(executable)) is None
        image = executable.stat()
        assert client._pane_process_identity(info, str(executable),
            launch_image=(image.st_dev, image.st_ino)) == identity
        assert client._pane_process_identity(info, str(executable),
            launch_image=(image.st_dev, image.st_ino + 1)) is None
        altered = (
            replace(identity, pid=identity.pid + 1), replace(identity, starttime_ticks=identity.starttime_ticks + 1),
            replace(identity, boot_id="ffffffff-ffff-ffff-ffff-ffffffffffff"),
            replace(identity, executable_device=identity.executable_device + 1),
            replace(identity, executable_inode=identity.executable_inode + 1),
        )
        assert all(not client.verify_harness_identity("p1", expected) for expected in altered)
        process_info["foreground_process_group_id"] = process.pid + 1
        assert not client.verify_harness_identity("p1", identity)
        process_info["foreground_process_group_id"] = process.pid

        screen = "Muse Code 1.3.0\n────────────────\n❯\n────────────────\nmodel · high · /work · Auto-review\n"

        def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
            if list(command)[1:3] == ["pane", "read"]:
                runner.calls.append(list(command))
                return subprocess.CompletedProcess(command, 0, screen, "")
            return runner(command)

        monkeypatch.setattr(client, "_harness_executable", lambda _kind: str(executable))
        monkeypatch.setattr(client, "_run", run)
        observed: list[CustomProcessIdentity] = []
        assert client.start_pane_agent("worker", "muse", "p1", (), timeout=1,
            on_observed=observed.append) == identity
        assert observed == [identity]
    finally:
        process.terminate()
        process.wait(timeout=5)


_SUGGESTION13_CODEX_INFO = AgentPaneInfo(
    "p1", "w1", "/work", "codex", "unknown", "codex", "native", "terminal", "t1",
)


def _suggestion13_codex_readiness(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[HerdrClient, AgentPaneInfo, CustomProcessIdentity, list[str]]:
    client = HerdrClient(herdr_bin="fixture-herdr", run=Runner({}))
    info = _SUGGESTION13_CODEX_INFO
    identity = CustomProcessIdentity(1, "00000000-0000-0000-0000-000000000000", 11, 11, 1, 2)
    checks: list[str] = []
    monkeypatch.setattr(client, "harness_identity", lambda _pane, _kind: identity)
    monkeypatch.setattr(client, "verify_harness_identity", lambda _pane, expected: expected == identity)
    monkeypatch.setattr(client, "pane_info", lambda _pane: info)

    def screen(_pane: str) -> str:
        checks.append("screen")
        return "› \x1b[2mAsk Codex to do anything\x1b[0m\n  GPT default · /work\n"

    monkeypatch.setattr(client, "read_screen", screen)
    return client, info, identity, checks


def test_suggestion13_codex_unknown_readiness_keeps_raw_status_and_requires_process_proof(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, _identity, checks = _suggestion13_codex_readiness(monkeypatch)
    assert client.codex_idle_ready(info) and info.status == "unknown"
    assert checks == ["screen"]
    monkeypatch.setattr(client, "harness_identity", lambda _pane, _kind: None)
    assert not client.codex_idle_ready(info) and checks == ["screen"]


@pytest.mark.parametrize("changed", [
    replace(_SUGGESTION13_CODEX_INFO, agent="claude"),
    replace(_SUGGESTION13_CODEX_INFO, status="idle"),
    replace(_SUGGESTION13_CODEX_INFO, status="working"),
    replace(_SUGGESTION13_CODEX_INFO, status="blocked"),
    replace(_SUGGESTION13_CODEX_INFO, terminal_id=None),
    replace(_SUGGESTION13_CODEX_INFO, terminal_id=""),
    replace(_SUGGESTION13_CODEX_INFO, terminal_id="bad\0terminal"),
    replace(_SUGGESTION13_CODEX_INFO, session_agent=None),
    replace(_SUGGESTION13_CODEX_INFO, session_agent="muse"),
    replace(_SUGGESTION13_CODEX_INFO, session_value=None),
    replace(_SUGGESTION13_CODEX_INFO, session_value=""),
    replace(_SUGGESTION13_CODEX_INFO, session_value="bad\0session"),
])
def test_suggestion13_codex_unknown_readiness_rejects_ineligible_live_metadata(
    changed: AgentPaneInfo, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, _info, _identity, checks = _suggestion13_codex_readiness(monkeypatch)
    assert not client.codex_idle_ready(changed) and checks == []


@pytest.mark.parametrize("changed", [
    replace(_SUGGESTION13_CODEX_INFO, pane_id="p2"),
    replace(_SUGGESTION13_CODEX_INFO, workspace_id="w2"),
    replace(_SUGGESTION13_CODEX_INFO, cwd="/other"),
    replace(_SUGGESTION13_CODEX_INFO, agent="claude"),
    replace(_SUGGESTION13_CODEX_INFO, status="working"),
    replace(_SUGGESTION13_CODEX_INFO, session_agent="muse"),
    replace(_SUGGESTION13_CODEX_INFO, session_value="replacement"),
    replace(_SUGGESTION13_CODEX_INFO, terminal_id="replacement"),
    replace(_SUGGESTION13_CODEX_INFO, tab_id="other-tab"),
])
def test_suggestion13_codex_unknown_readiness_rejects_pane_changes_during_screen_read(
    changed: AgentPaneInfo, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, _identity, _checks = _suggestion13_codex_readiness(monkeypatch)
    monkeypatch.setattr(client, "pane_info", lambda _pane: changed)
    assert not client.codex_idle_ready(info)


def test_suggestion13_codex_unknown_readiness_rejects_process_change_during_screen_read(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, _identity, _checks = _suggestion13_codex_readiness(monkeypatch)
    monkeypatch.setattr(client, "verify_harness_identity", lambda _pane, _expected: False)
    assert not client.codex_idle_ready(info)


_SUGGESTION13_START_TIMEOUT = (
    '{"id":"cli:agent:start","error":{"code":"timeout",'
    '"message":"timed out waiting for agent startup"}}'
)


def _suggestion13_timeout_client(
    monkeypatch: pytest.MonkeyPatch, *, returncode: int = 1,
    stdout: str = "", stderr: str = _SUGGESTION13_START_TIMEOUT,
) -> tuple[HerdrClient, AgentPaneInfo, list[list[str]], list[str]]:
    client, info, _identity, screens = _suggestion13_codex_readiness(monkeypatch)
    initial = replace(info, agent=None, session_agent=None, session_value=None)
    # Only `agent start` is recorded: these tests check it is never repeated. A timeout's
    # observation (`agent explain`, `pane read`) gets the same failed reply, so it reports
    # those parts unavailable. No grace period: each proof is observed once.
    calls: list[list[str]] = []
    monkeypatch.setattr(client_module, "CODEX_STARTUP_GRACE_SECONDS", 0.0)

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        if list(command[1:3]) == ["agent", "start"]:
            calls.append(list(command))
        return subprocess.CompletedProcess(command, returncode, stdout, stderr)

    monkeypatch.setattr(client, "_run", run)
    monkeypatch.setattr(client, "pane_info", lambda _pane: info if calls else initial)
    monkeypatch.setattr(client, "agent_identity", lambda name: AgentIdentity(
        name, info.pane_id, info.tab_id, info.terminal_id,
    ))
    return client, info, calls, screens


def test_suggestion13_exact_codex_start_timeout_accepts_verified_unknown_idle_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, screens = _suggestion13_timeout_client(monkeypatch)
    client.start_agent("worker", "codex", info.pane_id, ("--resume", "native"), timeout=1)
    assert calls == [["fixture-herdr", "agent", "start", "worker", "--kind", "codex",
                      "--pane", "p1", "--timeout", "1000", "--", "--resume", "native"]]
    assert screens == ["screen"] and client.pane_info(info.pane_id).status == "unknown"


@pytest.mark.parametrize("session", [None, "goal-conversation"])
def test_exact_codex_start_timeout_accepts_a_resumed_harness_already_working_on_its_goal(
    session: str | None, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, screens = _suggestion13_timeout_client(monkeypatch)
    working = replace(info, status="working", session_agent=session and "codex", session_value=session)
    monkeypatch.setattr(client, "pane_info", lambda _pane: working if calls else replace(
        info, agent=None, session_agent=None, session_value=None))
    client.start_agent("worker", "codex", info.pane_id, ("resume", "goal-conversation", "--no-alt-screen"),
                       timeout=1)
    assert len(calls) == 1 and screens == []


@pytest.mark.parametrize("failure", ["fresh", "other-session", "blocked", "process", "unpinned"])
def test_exact_codex_start_timeout_refuses_working_without_resume_proof(
    failure: str, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, _screens = _suggestion13_timeout_client(monkeypatch)
    working = replace(info, status="blocked" if failure == "blocked" else "working",
                      session_agent="codex" if failure == "other-session" else None,
                      session_value="someone-else" if failure == "other-session" else None)
    monkeypatch.setattr(client, "pane_info", lambda _pane: working if calls else replace(
        info, agent=None, session_agent=None, session_value=None))
    # A working screen is never an idle composer, so only the resume proof could accept it.
    monkeypatch.setattr(client, "read_screen", lambda _pane: "›\n  esc to interrupt\n  GPT · /work\n")
    if failure == "process":
        monkeypatch.setattr(client, "verify_harness_identity", lambda _pane, _expected: False)
    if failure == "unpinned":
        monkeypatch.setattr(client, "harness_identity", lambda _pane, _kind: None)
    arguments = ("--no-alt-screen",) if failure == "fresh" else ("resume", "goal-conversation")
    with pytest.raises(HerdrUnavailable, match="timed out waiting for agent startup"):
        client.start_agent("worker", "codex", info.pane_id, arguments, timeout=1)
    assert len(calls) == 1


def _unnamed_after_timeout(
    client: HerdrClient, info: AgentPaneInfo, monkeypatch: pytest.MonkeyPatch,
) -> list[tuple[str, str]]:
    """Herdr 0.9.3 after a startup timeout: the agent runs in the pane but has no name until
    ``rename_agent`` applies one."""
    renames: list[tuple[str, str]] = []

    def identity(name: str) -> AgentIdentity:
        if not renames:
            raise HerdrUnavailable(
                f'agent get {name!r}: {{"error":{{"code":"agent_not_found",'
                f'"message":"agent target {name} not found"}},"id":"cli:agent:get"}}'
            )
        return AgentIdentity(name, info.pane_id, info.tab_id, info.terminal_id)

    monkeypatch.setattr(client, "agent_identity", identity)
    monkeypatch.setattr(client, "rename_agent", lambda pane, name: renames.append((pane, name)))
    return renames


def test_exact_codex_timeout_names_the_agent_herdr_left_unnamed_once_proved_ready(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, screens = _suggestion13_timeout_client(monkeypatch)
    renames = _unnamed_after_timeout(client, info, monkeypatch)
    client.start_agent("worker", "codex", info.pane_id, ("--no-alt-screen",), timeout=1)
    assert len(calls) == 1 and screens == ["screen"] and renames == [("p1", "worker")]


@pytest.mark.parametrize("screen", [
    "Installing plugins for preset\n", "› human draft\n  GPT · /work\n",
    "›\n  esc to interrupt\n  GPT · /work\n",
])
def test_exact_codex_timeout_names_nothing_before_the_pane_is_proved_ready(
    screen: str, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, _screens = _suggestion13_timeout_client(monkeypatch)
    renames = _unnamed_after_timeout(client, info, monkeypatch)
    monkeypatch.setattr(client, "read_screen", lambda _pane: screen)
    with pytest.raises(HerdrUnavailable, match="the name \"worker\" was not applied"):
        client.start_agent("worker", "codex", info.pane_id, timeout=1)
    assert len(calls) == 1 and renames == []


def test_exact_codex_timeout_waits_a_grace_period_for_the_composer(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, _screens = _suggestion13_timeout_client(monkeypatch)
    monkeypatch.setattr(client_module, "CODEX_STARTUP_GRACE_SECONDS", 15.0)
    renames = _unnamed_after_timeout(client, info, monkeypatch)
    reads: list[str] = []

    def screen(_pane: str) -> str:
        reads.append("screen")
        if len(reads) == 1:
            return "Installing plugins for preset\n"
        return "› \x1b[2mAsk Codex to do anything\x1b[0m\n  GPT default · /work\n"

    monkeypatch.setattr(client, "read_screen", screen)
    client.start_agent("worker", "codex", info.pane_id, timeout=5)
    assert len(calls) == 1 and len(reads) == 2 and renames == [("p1", "worker")]


def test_start_timeout_reports_the_observed_agent_status_name_and_screen(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, _calls, _screens = _suggestion13_timeout_client(monkeypatch)
    _unnamed_after_timeout(client, info, monkeypatch)
    monkeypatch.setattr(client, "read_screen", lambda _pane: "Installing plugins\n")
    screen = "Codex\n\nerror: the argument '--no-alt-screen' cannot be used multiple times\n\n"

    def run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        if list(command[1:3]) == ["agent", "explain"]:
            return subprocess.CompletedProcess(command, 0, (
                "agent: codex\nstate: unknown\nmanifest: bundled 2026.09.23.1\nrule: none\n"
                "fallback_reason: codex_state_ambiguous\n"), "")
        if list(command[1:3]) == ["pane", "read"]:
            return subprocess.CompletedProcess(command, 0, screen, "")
        return subprocess.CompletedProcess(command, 1, "", _SUGGESTION13_START_TIMEOUT)

    monkeypatch.setattr(client, "_run", run)
    monkeypatch.setattr(client, "pane_info", lambda _pane: info)
    with pytest.raises(HerdrUnavailable) as raised:
        client.start_agent("worker", "codex", info.pane_id, timeout=1)
    message = str(raised.value)
    for part in (
        "timed out waiting for agent startup",
        "observed: Herdr reports agent codex with status unknown",
        "(rule none, fallback codex_state_ambiguous)",
        'the name "worker" was not applied',
        "screen: Codex | error: the argument '--no-alt-screen' cannot be used multiple times",
    ):
        assert part in message, (part, message)


def test_startup_screen_evidence_keeps_the_last_rows_and_cuts_long_ones() -> None:
    screen = "".join(f"row {row}\n\n" for row in range(1, 10))
    assert client_module._startup_screen_evidence(screen) == (
        "row 4 | row 5 | row 6 | row 7 | row 8 | row 9"
    )
    assert client_module._startup_screen_evidence("x" * 200 + "é") == "x" * 160 + "…"
    assert client_module._startup_screen_evidence("\n  \n") == ""


@pytest.mark.parametrize("returncode,stdout,stderr", [
    (2, "", _SUGGESTION13_START_TIMEOUT),
    (1, "unexpected output", _SUGGESTION13_START_TIMEOUT),
    (1, "", "timed out waiting for agent startup"),
    (1, "", _SUGGESTION13_START_TIMEOUT.replace('"timeout"', '"agent_not_ready"')),
    (1, "", _SUGGESTION13_START_TIMEOUT.replace('"cli:agent:start"', '"cli:agent:start:timeout"')),
    (1, "", _SUGGESTION13_START_TIMEOUT.replace('"timeout"', '"timeout","extra":true')),
    (1, "", _SUGGESTION13_START_TIMEOUT.replace('"timeout"', '"timeout","code":"timeout"')),
    (1, "", _SUGGESTION13_START_TIMEOUT.replace('"cli:agent:start"', '"cli:agent:start","extra":true')),
])
def test_suggestion13_other_start_errors_never_use_codex_readiness_fallback(
    returncode: int, stdout: str, stderr: str, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, screens = _suggestion13_timeout_client(
        monkeypatch, returncode=returncode, stdout=stdout, stderr=stderr,
    )
    with pytest.raises(HerdrUnavailable, match="agent start 'worker'"):
        client.start_agent("worker", "codex", info.pane_id, timeout=1)
    assert len(calls) == 1 and screens == []


@pytest.mark.parametrize("failure", [
    "draft", "busy", "trust", "process", "name", "terminal", "workspace", "native",
    "occupied-before-start", "final-name",
])
def test_suggestion13_exact_timeout_still_requires_coherent_idle_runtime(
    failure: str, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, _screens = _suggestion13_timeout_client(monkeypatch)
    if failure in ("draft", "busy", "trust"):
        screen = {
            "draft": "› human draft\n  GPT · /work\n",
            "busy": "›\n  esc to interrupt\n  GPT · /work\n",
            "trust": "Trust this folder?\n›\n  GPT · /work\n",
        }[failure]
        monkeypatch.setattr(client, "read_screen", lambda _pane: screen)
    elif failure == "process":
        monkeypatch.setattr(client, "verify_harness_identity", lambda _pane, _expected: False)
    elif failure in ("name", "final-name"):
        named_calls = 0

        def named(name: str) -> AgentIdentity:
            nonlocal named_calls
            named_calls += 1
            wrong = failure == "name" or named_calls > 1
            return AgentIdentity(name, "other" if wrong else "p1", "t1", "terminal")

        monkeypatch.setattr(client, "agent_identity", named)
    else:
        initial = replace(info, agent=None, session_agent=None, session_value=None)
        changed = info
        if failure == "terminal":
            changed = replace(info, terminal_id="replacement")
        elif failure == "workspace":
            changed = replace(info, workspace_id="other")
        elif failure == "native":
            changed = replace(info, session_agent="muse")
        else:
            initial = info
        monkeypatch.setattr(client, "pane_info", lambda _pane: changed if calls else initial)
    with pytest.raises(HerdrUnavailable, match="timed out waiting for agent startup"):
        client.start_agent("worker", "codex", info.pane_id, timeout=1)
    assert len(calls) == 1


@pytest.mark.parametrize("returncode", [0, 1])
def test_suggestion13_unavailable_optional_snapshot_preserves_native_result(
    returncode: int, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, screens = _suggestion13_timeout_client(monkeypatch, returncode=returncode)

    def unavailable(_pane: str) -> AgentPaneInfo:
        raise HerdrUnavailable("optional snapshot unavailable")

    monkeypatch.setattr(client, "pane_info", unavailable)
    if returncode == 0:
        client.start_agent("worker", "codex", info.pane_id, timeout=1)
    else:
        with pytest.raises(HerdrUnavailable, match="timed out waiting for agent startup"):
            client.start_agent("worker", "codex", info.pane_id, timeout=1)
    assert len(calls) == 1 and screens == []


def test_suggestion13_claude_start_timeout_keeps_existing_refusal(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, info, calls, screens = _suggestion13_timeout_client(monkeypatch)
    with pytest.raises(HerdrUnavailable, match="timed out waiting for agent startup"):
        client.start_agent("worker", "claude", info.pane_id, timeout=1)
    assert len(calls) == 1 and screens == []


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


def _fixture_stat(pid: int, ppid: int) -> bytes:
    return f"{pid} (bash) S {ppid} {pid} ".encode() + b"0 " * 16 + b"100 0"


def _fixture_shell(root: Path, pid: int, children: str | None) -> None:
    task = root / str(pid) / "task" / str(pid)
    task.mkdir(parents=True, exist_ok=True)
    if children is not None:
        (task / "children").write_text(children)


def test_no_descendants_fixture_children_listing(tmp_path: Path) -> None:
    _fixture_shell(tmp_path, 100, "101\n")
    assert not HerdrClient._process_has_no_descendants(100, str(tmp_path))
    _fixture_shell(tmp_path, 100, "garbage\n")
    assert not HerdrClient._process_has_no_descendants(100, str(tmp_path))
    _fixture_shell(tmp_path, 100, "")
    assert HerdrClient._process_has_no_descendants(100, str(tmp_path))
    assert not HerdrClient._process_has_no_descendants(999, str(tmp_path))


def test_no_descendants_fixture_fallback_vanishing_shapes(tmp_path: Path) -> None:
    _fixture_shell(tmp_path, 100, None)
    (tmp_path / "100" / "stat").write_bytes(_fixture_stat(100, 1))
    (tmp_path / "200").mkdir()
    (tmp_path / "200" / "stat").write_bytes(_fixture_stat(200, 300))
    assert HerdrClient._process_has_no_descendants(100, str(tmp_path))
    (tmp_path / "101").mkdir()
    (tmp_path / "101" / "stat").write_bytes(_fixture_stat(101, 100))
    assert not HerdrClient._process_has_no_descendants(100, str(tmp_path))
    (tmp_path / "101" / "stat").unlink()
    (tmp_path / "101").rmdir()
    # Empty and vanished records are processes that exited: they do not
    # block the proof. Truncated and stably malformed records that stay
    # readable are inconclusive (not idle), never "no descendants" (#1506).
    (tmp_path / "201").mkdir()
    (tmp_path / "201" / "stat").write_bytes(b"")
    assert HerdrClient._process_has_no_descendants(100, str(tmp_path))
    (tmp_path / "201" / "stat").write_bytes(_fixture_stat(201, 300))
    (tmp_path / "202").mkdir()
    assert HerdrClient._process_has_no_descendants(100, str(tmp_path))
    (tmp_path / "202").rmdir()
    (tmp_path / "201" / "stat").write_bytes(b"201 (sleep) S 1")
    assert not HerdrClient._process_has_no_descendants(100, str(tmp_path))
    (tmp_path / "201" / "stat").write_bytes(b"not a stat record")
    assert not HerdrClient._process_has_no_descendants(100, str(tmp_path))


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


_LivenessFixture = tuple[HerdrClient, CustomProcessIdentity, ProcessStat]


def _liveness_identity() -> CustomProcessIdentity:
    return CustomProcessIdentity(1, "00000000-0000-0000-0000-000000000000", 4242, 99, 10, 11)


@pytest.fixture
def liveness_probe(monkeypatch: pytest.MonkeyPatch) -> _LivenessFixture:
    identity = _liveness_identity()
    observed = ProcessStat(pid=4242, starttime=99, ppid=1, pgrp=4242, session=4242, state="S")
    client = HerdrClient(herdr_bin="fixture-herdr", run=Runner({}))
    monkeypatch.setattr("agentctl.client.os.pidfd_open", lambda _pid: os.open(os.devnull, os.O_RDONLY), raising=False)
    monkeypatch.setattr(client, "_liveness_boot_identity", lambda: identity.boot_id)
    monkeypatch.setattr(client, "_liveness_process_stat", lambda _pid: observed)
    monkeypatch.setattr(client, "_liveness_executable_identity", lambda _pid: (10, 11))
    monkeypatch.setattr(client, "_pidfd_exited", lambda _descriptor: False)
    return client, identity, observed


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_process_liveness_tracks_a_real_child_before_and_after_kill() -> None:
    runner = Runner({})
    client = HerdrClient(herdr_bin="fixture-herdr", run=runner)
    process = subprocess.Popen([os.path.realpath("/usr/bin/sleep"), "30"])
    descriptor = os.pidfd_open(process.pid)
    try:
        observed = client._process_identity(process.pid)
        assert observed is not None
        identity = observed[0]
        assert client.process_liveness(identity) == "alive"
        process.kill()
        poller = select.poll()
        poller.register(descriptor, select.POLLIN)
        assert poller.poll(2000), "the owned child did not exit"
        # Do not reap yet: the pidfd proves death even while /proc still has a zombie.
        assert client.process_liveness(identity) == "dead"
        process.wait(timeout=5)
        assert client.process_liveness(identity) == "dead"
        assert runner.calls == []
    finally:
        os.close(descriptor)
        if process.poll() is None:
            process.kill()
        process.wait(timeout=5)


@pytest.mark.parametrize(
    ("identity", "outcome"),
    [
        (replace(_liveness_identity(), boot_id="ffffffff-ffff-ffff-ffff-ffffffffffff"), "dead"),
        (replace(_liveness_identity(), starttime_ticks=98), "dead"),
        (replace(_liveness_identity(), executable_device=9), "unknown"),
        (replace(_liveness_identity(), executable_inode=12), "unknown"),
        (replace(_liveness_identity(), version=0), "unknown"),
        (replace(_liveness_identity(), boot_id="invalid"), "unknown"),
        (replace(_liveness_identity(), pid=0), "unknown"),
        (replace(_liveness_identity(), pid=2_147_483_648), "unknown"),
        (replace(_liveness_identity(), starttime_ticks=0), "unknown"),
        (replace(_liveness_identity(), executable_inode=0), "unknown"),
    ],
)
def test_process_liveness_distinguishes_generations_from_live_image_changes(
    liveness_probe: _LivenessFixture, identity: CustomProcessIdentity, outcome: str,
) -> None:
    client, _recorded, _observed = liveness_probe
    assert client.process_liveness(identity) == outcome


@pytest.mark.parametrize("state", ["Z", "X", "x"])
def test_process_liveness_does_not_require_a_dead_process_executable(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch, state: str,
) -> None:
    client, identity, observed = liveness_probe
    monkeypatch.setattr(client, "_liveness_process_stat", lambda _pid: replace(observed, state=state))

    def no_executable(_pid: int) -> None:
        raise AssertionError("a dead process has no readable executable")

    monkeypatch.setattr(client, "_liveness_executable_identity", no_executable)
    assert client.process_liveness(identity) == "dead"


@pytest.mark.parametrize(
    ("error_number", "outcome"),
    [(errno.ESRCH, "dead"), *[(value, "unknown") for value in (
        errno.EPERM, errno.EACCES, errno.ENOSYS, errno.EINVAL, errno.EIO, errno.EINTR,
    )]],
)
def test_process_liveness_requires_a_positive_pidfd_absence_error(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch,
    error_number: int, outcome: str,
) -> None:
    client, identity, _observed = liveness_probe
    attempts = 0

    def unavailable(_pid: int) -> int:
        nonlocal attempts
        attempts += 1
        raise OSError(error_number, "injected pidfd error")

    monkeypatch.setattr("agentctl.client.os.pidfd_open", unavailable)
    assert client.process_liveness(identity) == outcome
    assert attempts == (4 if error_number == errno.EINTR else 1)


def test_process_liveness_without_pidfds_is_unknown(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, identity, _observed = liveness_probe
    monkeypatch.delattr("agentctl.client.os.pidfd_open")
    assert client.process_liveness(identity) == "unknown"


def test_process_liveness_keeps_pidfd_close_failure_unknown(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, identity, _observed = liveness_probe
    close = os.close

    def close_then_fail(descriptor: int) -> None:
        close(descriptor)
        raise OSError(errno.EIO, "injected close failure")

    monkeypatch.setattr("agentctl.client.os.close", close_then_fail)
    assert client.process_liveness(identity) == "unknown"


def test_process_liveness_retries_interrupted_pidfd_open(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, identity, _observed = liveness_probe
    calls = 0

    def interrupted(_pid: int) -> int:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise InterruptedError(errno.EINTR, "injected interrupt")
        return os.open(os.devnull, os.O_RDONLY)

    monkeypatch.setattr("agentctl.client.os.pidfd_open", interrupted)
    assert client.process_liveness(identity) == "alive"
    assert calls == 2


@pytest.mark.parametrize("method", [
    "_liveness_boot_identity", "_liveness_process_stat", "_liveness_executable_identity", "_pidfd_exited",
])
@pytest.mark.parametrize("failure", [errno.EPERM, errno.EIO, errno.ENOENT, None])
def test_process_liveness_keeps_probe_errors_unknown(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch,
    method: str, failure: int | None,
) -> None:
    client, identity, _observed = liveness_probe

    def unavailable(*_args: int) -> None:
        if failure is None:
            raise ValueError("injected malformed probe")
        raise OSError(failure, "injected unreadable probe")

    monkeypatch.setattr(client, method, unavailable)
    assert client.process_liveness(identity) == "unknown"


@pytest.mark.parametrize("changed", ["boot", "stat", "image", "reused-stat", "zombie-stat"])
def test_process_liveness_refuses_changing_probe_snapshots(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch, changed: str,
) -> None:
    client, identity, observed = liveness_probe
    if changed == "boot":
        boot_ids = iter([identity.boot_id, "ffffffff-ffff-ffff-ffff-ffffffffffff"])
        monkeypatch.setattr(client, "_liveness_boot_identity", lambda: next(boot_ids))
    elif changed == "image":
        images = iter([(10, 11), (10, 12)])
        monkeypatch.setattr(client, "_liveness_executable_identity", lambda _pid: next(images))
    else:
        if changed == "reused-stat":
            first, second = replace(observed, starttime=98), observed
        elif changed == "zombie-stat":
            first, second = replace(observed, state="Z"), observed
        else:
            first, second = observed, replace(observed, starttime=100)
        snapshots = iter([first, second])
        monkeypatch.setattr(client, "_liveness_process_stat", lambda _pid: next(snapshots))
    assert client.process_liveness(identity) == "unknown"


@pytest.mark.parametrize("raw", ["", "invalid", "x" * 129, "00000000-0000-0000-0000-000000000000\0"])
def test_process_liveness_refuses_malformed_boot_reads(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch, raw: str,
) -> None:
    client, identity, _observed = liveness_probe

    def boot_file(path: str, *, encoding: str) -> io.StringIO:
        assert path == "/proc/sys/kernel/random/boot_id" and encoding == "ascii"
        return io.StringIO(raw)

    monkeypatch.setattr(client_module, "open", boot_file, raising=False)
    monkeypatch.setattr(client, "_liveness_boot_identity", HerdrClient._liveness_boot_identity)
    assert client.process_liveness(identity) == "unknown"


@pytest.mark.parametrize("raw", [
    b"", b"4242 (unterminated S 1 2 3", b"4242 (x) S 1 2 3", b"x" * 8193,
    b"4241 (x) S 1 1 1 " + b"0 " * 15 + b"99",
    b"4242 (x) S 1 1 1 " + b"0 " * 16,
])
def test_process_liveness_refuses_malformed_stat_reads(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch, raw: bytes,
) -> None:
    client, identity, _observed = liveness_probe

    def stat_file(path: str, mode: str) -> io.BytesIO:
        assert path == "/proc/4242/stat" and mode == "rb"
        return io.BytesIO(raw)

    monkeypatch.setattr(client_module, "open", stat_file, raising=False)
    monkeypatch.setattr(client, "_liveness_process_stat", HerdrClient._liveness_process_stat)
    assert client.process_liveness(identity) == "unknown"


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_process_liveness_reads_a_vanished_record_as_death_only_with_exit_proof(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch,
) -> None:
    client, identity, _observed = liveness_probe

    def gone(_pid: int) -> ProcessStat:
        raise client_module._ProcessGoneError(_pid)

    monkeypatch.setattr(client, "_liveness_process_stat", gone)
    monkeypatch.setattr(client, "_pidfd_exited", lambda _descriptor: True)
    assert client.process_liveness(identity) == "dead"
    monkeypatch.setattr(client, "_pidfd_exited", lambda _descriptor: False)
    assert client.process_liveness(identity) == "unknown"


@pytest.mark.skipif(not hasattr(os, "pidfd_open"), reason="Linux pidfd identity required")
def test_liveness_process_stat_reads_a_reaped_child_as_gone() -> None:
    # Read under a condition (the child has been reaped), not a timer:
    # the dead pid's record classifies as gone, never a parse refusal (#1495).
    process = subprocess.Popen([os.path.realpath("/usr/bin/sleep"), "30"])
    try:
        own = HerdrClient._liveness_process_stat(os.getpid())
        assert own.pid == os.getpid()
        process.kill()
        process.wait(timeout=5)
        with pytest.raises(client_module._ProcessGoneError):
            HerdrClient._liveness_process_stat(process.pid)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)


@pytest.mark.parametrize(
    ("flags", "outcome"),
    [(select.POLLIN, "dead"), (select.POLLIN | select.POLLHUP, "dead"),
     (select.POLLERR, "unknown"), (select.POLLNVAL, "unknown"),
     (select.POLLHUP, "unknown"), (select.POLLIN | select.POLLERR, "unknown")],
)
def test_process_liveness_requires_a_valid_pidfd_exit_event(
    liveness_probe: _LivenessFixture, monkeypatch: pytest.MonkeyPatch, flags: int, outcome: str,
) -> None:
    client, identity, _observed = liveness_probe

    class Poller:
        descriptor: int = -1

        def register(self, descriptor: int, events: int) -> None:
            assert events == select.POLLIN
            self.descriptor = descriptor

        def poll(self, timeout: int) -> list[tuple[int, int]]:
            assert timeout == 0
            return [(self.descriptor, flags)]

    monkeypatch.setattr("agentctl.client.select.poll", Poller)
    monkeypatch.setattr(client, "_pidfd_exited", HerdrClient._pidfd_exited)
    assert client.process_liveness(identity) == outcome


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
