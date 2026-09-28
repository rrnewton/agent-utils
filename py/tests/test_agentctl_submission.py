"""Verified prompt submission against a fake terminal agent and a fake Herdr process."""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Sequence
from pathlib import Path

import pytest

from agentctl.agent import Target, drain, enqueue
from agentctl.client import AgentPaneInfo, HerdrClient
from agentctl.errors import HerdrUnavailable
from agentctl.submission import (
    PromptNotStaged,
    PromptStagedNotSubmitted,
    composer_view,
    submit_verified,
)

_PASTE_START = "\x1b[200~"
_PASTE_END = "\x1b[201~"
_RULE = "─" * 60


def _dim(text: str) -> str:
    return f"\x1b[2m{text}\x1b[0m"


class FakeAgent:
    """A terminal agent composer with a controllable key-drop and redraw lag."""

    def __init__(self, harness: str, *, busy: bool = False) -> None:
        self.harness = harness
        self.busy = busy
        self.composer = ""
        self.transcript: list[str] = ["• earlier output"]
        self.queued: list[str] = []
        self.submitted: list[str] = []
        self.steered: list[str] = []
        self.pastes: list[str] = []
        self.keys: list[str] = []
        self.drop_keys = 0
        self.paste_visible_after_reads = 0
        self.redraw_lag_reads = 0
        self.stale_screen: str | None = None
        self.dialog = False
        self._pending_reads = 0
        self._paste_counter = 0
        self._placeholder: str | None = None

    # Terminal operations.
    def read_screen(self, pane_id: str) -> str:
        del pane_id
        if self.stale_screen is not None and self._pending_reads > 0:
            self._pending_reads -= 1
            return self.stale_screen
        self.stale_screen = None
        if self.paste_visible_after_reads > 0:
            self.paste_visible_after_reads -= 1
            return self._render(hide_composer=True)
        return self._render()

    def send_text(self, pane_id: str, text: str) -> None:
        del pane_id
        assert text.startswith(_PASTE_START) and text.endswith(_PASTE_END)
        body = text[len(_PASTE_START):-len(_PASTE_END)]
        self.pastes.append(body)
        self.composer += body
        if self.harness == "claude" and body.count("\n") >= 3:
            self._paste_counter += 1
            self._placeholder = f"[Pasted text #{self._paste_counter} +{body.count(chr(10))} lines]"

    def send_keys(self, pane_id: str, keys: str) -> None:
        del pane_id
        self.keys.append(keys)
        if self.drop_keys > 0:
            self.drop_keys -= 1
            return
        before = self._render()
        text = self.composer
        if not text:
            return
        if self.harness == "claude" and keys == "Enter":
            self.transcript.append(f"❯ {text}")
            (self.queued if self.busy else self.submitted).append(text)
            if not self.busy:
                self.transcript.append("✻ Working… (running UserPromptSubmit hooks…)")
            self.busy = True
        elif self.harness == "codex" and keys == "Enter":
            self.transcript.append(f"› {text}")
            (self.steered if self.busy else self.submitted).append(text)
            if not self.busy:
                self.transcript.append("• Working (0s • esc to interrupt)")
            self.busy = True
        elif self.harness == "codex" and keys == "Tab" and self.busy:
            self.queued.append(text)
            self.transcript += ["• Queued follow-up inputs", f"  ↳ {text}"]
        else:
            return
        self.composer = ""
        self._placeholder = None
        if self.redraw_lag_reads:
            self.stale_screen = before
            self._pending_reads = self.redraw_lag_reads

    # Rendering.
    def _render(self, *, hide_composer: bool = False) -> str:
        composer = "" if hide_composer else self.composer
        lines = list(self.transcript)
        if self.dialog:
            return "\n".join(lines + ["Replace goal?", "  › 1. Replace", "  2. Cancel"])
        if self.harness == "claude":
            if self.busy and not any("Working" in line for line in lines):
                lines.append("✻ Working… (running PreToolUse hooks…)")
            lines.append(_RULE)
            if composer and self._placeholder and not hide_composer:
                lines.append(f"❯ {self._placeholder}")
            elif composer:
                first, *rest = composer.split("\n")
                lines.append(f"❯ {first}")
                lines += [f"  {line}" for line in rest]
            elif self.queued:
                lines.append("❯ \x1b[7mP\x1b[0m" + _dim("ress up to edit queued messages"))
            else:
                lines.append("❯\xa0")
            lines.append(_RULE)
            footer = "  ⏵⏵ auto mode on"
            if self.busy:
                footer += " · esc to interrupt"
            lines.append(footer)
        else:
            if composer:
                first, *rest = composer.split("\n")
                lines.append(f"› {first}")
                lines += [f"  {line}" for line in rest]
            else:
                lines.append("› " + _dim("Ask Codex to do anything"))
            lines.append("")
            if self.busy and composer:
                lines.append("  tab to queue message                 99% context")
            else:
                lines.append("  GPT default · /tmp/project")
        return "\n".join(lines) + "\n"


class Clock:
    def __init__(self) -> None:
        self.now = 1000.0

    def sleep(self, seconds: float) -> None:
        self.now += seconds

    def monotonic(self) -> float:
        return self.now


def _submit(agent: FakeAgent, text: str, **kwargs: float) -> object:
    clock = Clock()
    return submit_verified(
        agent, "w1:p1", agent.harness, text,
        sleep=clock.sleep, monotonic=clock.monotonic, **kwargs,
    )


def test_screen_parser_separates_placeholder_from_draft() -> None:
    agent = FakeAgent("claude")
    view = composer_view("claude", agent._render())
    assert view is not None and view.composer_solid.strip() == ""
    agent.queued.append("x")
    view = composer_view("claude", agent._render())
    assert view is not None and view.composer_solid.strip() == ""
    assert "Press up" not in view.composer_solid and "ress up" in view.composer
    agent.composer = "draft"
    view = composer_view("claude", agent._render())
    assert view is not None and view.composer_solid.strip() == "draft"
    codex = FakeAgent("codex")
    view = composer_view("codex", codex._render())
    assert view is not None and view.composer_solid.strip() == ""
    assert "Ask Codex" in view.composer


@pytest.mark.parametrize("harness", ["claude", "codex"])
def test_dropped_submit_key_is_retried_without_retyping(harness: str) -> None:
    agent = FakeAgent(harness)
    agent.drop_keys = 1
    receipt = _submit(agent, "please run the report")
    assert agent.pastes == ["please run the report"]
    assert agent.keys == ["Enter", "Enter"]
    assert agent.submitted == ["please run the report"]
    assert getattr(receipt, "key_presses") == 2


@pytest.mark.parametrize("harness", ["claude", "codex"])
def test_submission_already_accepted_is_never_repeated(harness: str) -> None:
    # The agent accepts the first key but redraws late; no second key may be sent
    # merely because the next screen reads still show the staged text.
    agent = FakeAgent(harness)
    agent.redraw_lag_reads = 3
    receipt = _submit(agent, "exactly once")
    assert agent.pastes == ["exactly once"]
    assert agent.keys == ["Enter"]
    assert agent.submitted == ["exactly once"]
    assert agent.queued == [] and agent.steered == []
    assert getattr(receipt, "key_presses") == 1


@pytest.mark.parametrize("harness", ["claude", "codex"])
def test_late_redraw_retry_lands_on_an_empty_composer(harness: str) -> None:
    # A redraw slower than the first retry interval draws a second key, which
    # reaches an empty composer and therefore cannot submit anything twice.
    agent = FakeAgent(harness)
    agent.redraw_lag_reads = 8
    _submit(agent, "exactly once")
    assert agent.pastes == ["exactly once"]
    assert agent.keys == ["Enter", "Enter"]
    assert agent.submitted == ["exactly once"]
    assert agent.queued == [] and agent.steered == []


def test_busy_claude_queues_with_enter() -> None:
    agent = FakeAgent("claude", busy=True)
    receipt = _submit(agent, "follow up while busy")
    assert agent.keys == ["Enter"]
    assert agent.queued == ["follow up while busy"]
    assert "above the composer" in getattr(receipt, "evidence")


def test_busy_codex_queues_with_tab_and_never_steers() -> None:
    agent = FakeAgent("codex", busy=True)
    agent.drop_keys = 1
    receipt = _submit(agent, "queue this behind the turn")
    assert agent.keys == ["Tab", "Tab"]
    assert agent.queued == ["queue this behind the turn"]
    assert agent.steered == [] and agent.submitted == []
    assert getattr(receipt, "key") == "Tab"


def test_idle_codex_submits_with_enter() -> None:
    agent = FakeAgent("codex")
    _submit(agent, "start now")
    assert agent.keys == ["Enter"]
    assert agent.submitted == ["start now"]


def test_submit_key_that_never_lands_times_out_as_staged() -> None:
    agent = FakeAgent("claude")
    agent.drop_keys = 1_000
    with pytest.raises(PromptStagedNotSubmitted, match="NOT submitted") as caught:
        _submit(agent, "stuck prompt", submit_timeout=20.0)
    assert agent.pastes == ["stuck prompt"]
    assert agent.composer == "stuck prompt"
    assert agent.submitted == []
    # Backoff 0.5, 1, 2, 4, 4, ... bounds the key count within the deadline.
    assert 5 <= len(agent.keys) <= 8
    assert f"{len(agent.keys)} Enter key press(es)" in str(caught.value)


def test_existing_draft_is_refused_before_typing() -> None:
    agent = FakeAgent("claude")
    agent.composer = "a human's half-written note"
    with pytest.raises(PromptNotStaged, match="refusing to append"):
        _submit(agent, "new prompt")
    assert agent.pastes == [] and agent.keys == []


def test_unrecognised_screen_is_refused_before_typing() -> None:
    agent = FakeAgent("codex")
    agent.dialog = True
    with pytest.raises(PromptNotStaged, match="recognisable codex composer"):
        _submit(agent, "new prompt")
    assert agent.pastes == [] and agent.keys == []


def test_escape_characters_are_refused_before_typing() -> None:
    agent = FakeAgent("claude")
    with pytest.raises(PromptNotStaged, match="escape"):
        _submit(agent, "bad \x1b[201~ text")
    assert agent.pastes == []


def test_paste_that_never_appears_sends_no_submit_key() -> None:
    agent = FakeAgent("claude")
    agent.paste_visible_after_reads = 1_000
    with pytest.raises(HerdrUnavailable, match="no submission key was sent") as caught:
        _submit(agent, "invisible")
    assert not isinstance(caught.value, (PromptNotStaged, PromptStagedNotSubmitted))
    assert agent.keys == []


def test_slow_paste_is_waited_for_before_the_key() -> None:
    agent = FakeAgent("claude")
    agent.paste_visible_after_reads = 5
    _submit(agent, "slow terminal")
    assert agent.keys == ["Enter"]
    assert agent.submitted == ["slow terminal"]


def test_long_paste_placeholder_counts_as_staged() -> None:
    agent = FakeAgent("claude")
    text = "\n".join(f"line {index}" for index in range(12))
    agent.drop_keys = 1
    _submit(agent, text)
    assert agent.keys == ["Enter", "Enter"]
    assert agent.submitted == [text]


def _completed(
    command: Sequence[str], returncode: int = 0, stdout: str = "", stderr: str = "",
) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(list(command), returncode, stdout, stderr)


class FakeHerdrProcess:
    """Answer the Herdr subcommands that a prompt delivery issues."""

    def __init__(self, agent: FakeAgent) -> None:
        self.agent = agent
        self.calls: list[tuple[str, ...]] = []

    def __call__(self, command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        args = tuple(command[1:])
        self.calls.append(args)
        pane = args[2] if len(args) > 2 else ""
        if args[:2] == ("agent", "prompt"):
            # Native prompt: paste, then an unverified Enter a moment later.
            self.agent.send_text(pane, f"{_PASTE_START}{args[3]}{_PASTE_END}")
            self.agent.send_keys(pane, "Enter")
            return _completed(command, stdout='{"result":{"type":"agent_prompted"}}')
        if args[:2] == ("pane", "send-text"):
            self.agent.send_text(pane, args[3])
            return _completed(command)
        if args[:2] == ("pane", "send-keys"):
            self.agent.send_keys(pane, args[3])
            return _completed(command)
        if args[:2] == ("pane", "read"):
            assert "--format" in args and "ansi" in args
            return _completed(command, stdout=self.agent.read_screen(pane))
        if args[:2] == ("agent", "wait"):
            if self.agent.busy:
                event = {"result": {"agent": {"pane_id": pane, "agent_status": "working"}}}
                return _completed(command, stdout=json.dumps(event))
            return _completed(command, returncode=1, stderr="timed out waiting for working")
        raise AssertionError(f"unexpected Herdr invocation: {args!r}")


class ProcessBackedClient(HerdrClient):
    def __init__(self, agent: FakeAgent) -> None:
        self.process = FakeHerdrProcess(agent)
        super().__init__(herdr_bin="fixture-herdr", run=self.process)
        self.agent = agent

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        return AgentPaneInfo(pane_id, "w1", "/work/project", self.agent.harness,
                             "idle", None, None)


@pytest.mark.parametrize("harness", ["claude", "codex"])
def test_queue_delivers_through_a_dropped_submit_key(tmp_path: Path, harness: str) -> None:
    agent = FakeAgent(harness)
    agent.drop_keys = 1
    client = ProcessBackedClient(agent)
    root = tmp_path / "queue"
    target = Target(pane_id="w1:p1")
    enqueue(str(root), "deliver me once", message_id="m1")
    result = drain(client, target, str(root), working_timeout=1.0)
    assert result.delivered == ("m1",), result
    assert result.quarantined == ()
    assert agent.submitted == ["deliver me once"]
    assert agent.pastes == ["deliver me once"]
    assert not any(call[:2] == ("agent", "prompt") for call in client.process.calls)


def test_queue_keeps_prompt_pending_when_nothing_was_typed(tmp_path: Path) -> None:
    agent = FakeAgent("claude")
    agent.composer = "someone else's draft"
    client = ProcessBackedClient(agent)
    root = tmp_path / "queue"
    enqueue(str(root), "wait your turn", message_id="m1")
    result = drain(client, Target(pane_id="w1:p1"), str(root), working_timeout=1.0)
    assert result.outcome == "pending"
    assert result.pending == ("m1",)
    assert result.delivered == () and result.quarantined == ()
    assert "refusing to append" in (result.blocked or "")
    assert agent.pastes == [] and agent.keys == []
    document = json.loads((root / "inbox" / "m1.json").read_text())
    assert document["delivery_state"] == "pending"
    assert "possibly_submitted" not in document
    assert not os.listdir(root / "inflight")

    agent.composer = ""
    result = drain(client, Target(pane_id="w1:p1"), str(root), working_timeout=1.0)
    assert result.delivered == ("m1",)
    assert agent.submitted == ["wait your turn"]


def test_queue_quarantines_a_prompt_left_staged(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import agentctl.submission as submission

    monkeypatch.setattr(submission, "SUBMIT_TIMEOUT_SECONDS", 0.3)
    agent = FakeAgent("claude")
    agent.drop_keys = 1_000
    client = ProcessBackedClient(agent)
    root = tmp_path / "queue"
    enqueue(str(root), "never lands", message_id="m1")
    result = drain(client, Target(pane_id="w1:p1"), str(root), working_timeout=1.0)
    assert result.quarantined == ("m1",)
    assert result.outcome == "possibly_submitted"
    failed = json.loads((root / "failed" / "m1.json").read_text())
    assert "still staged" in failed["delivery_error"]
    assert "NOT submitted" in failed["delivery_error"]
    assert agent.pastes == ["never lands"]


def test_slash_commands_keep_the_native_prompt_path() -> None:
    agent = FakeAgent("codex")
    client = ProcessBackedClient(agent)
    assert client.prompt_agent("w1:p1", "/goal ship it") is None
    assert client.process.calls[0][:2] == ("agent", "prompt")
