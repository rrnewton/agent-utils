"""Verified prompt submission against a fake terminal agent and a fake Herdr process."""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Sequence
from pathlib import Path

import pytest

from typing import cast

from agentctl import cli
from agentctl.errors import AgentDeliveryError, AgentPending, AgentPossiblySubmitted
from agentctl.harness_tui import HarnessTui, State, harness_tui
from agentctl.tui_prompts import answer
from .test_herdr_agent import FakeAgentHerdr, target
from .test_agentctl_sessions import setup
from agentctl.submission import SubmissionReceipt, submit_verified
from agentctl.agent import send

from agentctl.agent import Target, drain, enqueue
from agentctl.client import AgentPaneInfo, HerdrClient
from agentctl.errors import HerdrUnavailable
from agentctl.submission import (
    PromptNotStaged,
    PromptStagedNotSubmitted,
    _corroborated,
    _is_labelled_rule,
    _staged,
    codex_idle_composer,
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
        # Every submission key (Enter or Tab) is dropped; editing keys still work.
        self.drop_submit_keys = False
        self.paste_visible_after_reads = 0
        self.redraw_lag_reads = 0
        self.stale_screen: str | None = None
        self.dialog = False
        # Label Claude draws into the composer's top border, such as a mode name.
        self.top_border_label: str | None = None
        # Glyph Codex draws before its composer: › before v0.159.1, » from it.
        self.codex_marker = "›"
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
        if self.drop_submit_keys and keys in ("Enter", "Tab"):
            return
        # Editing keys with the cursor at the end: ctrl+e moves nothing, ctrl+u deletes the
        # last row (a paste placeholder goes whole), backspace deletes one character.
        if keys == "ctrl+e":
            return
        if keys in ("ctrl+u", "backspace") and self._placeholder is not None:
            self.composer, self._placeholder = "", None
            return
        if keys == "ctrl+u":
            # On an empty last row only Codex removes the line break before it.
            start = self.composer.rfind("\n") + 1
            if start < len(self.composer):
                self.composer = self.composer[:start]
            elif self.harness == "codex":
                self.composer = self.composer[:max(start - 1, 0)]
            return
        if keys == "backspace":
            self.composer = self.composer[:-1]
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
            if self.top_border_label is None:
                lines.append(_RULE)
            else:
                # Coloured as Claude draws it: grey rule, violet label, grey tail.
                lines.append(
                    f"\x1b[38;5;244m{_RULE}\x1b[0m \x1b[38;5;147m{self.top_border_label} "
                    "\x1b[38;5;244m─\x1b[0m"
                )
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
                lines.append(f"{self.codex_marker} {first}")
                lines += [f"  {line}" for line in rest]
            else:
                lines.append(f"{self.codex_marker} " + _dim("Ask Codex to do anything"))
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


class LateComposerAgent(FakeAgent):
    """A freshly launched harness whose composer is drawn only after a few reads."""

    def __init__(self, harness: str, *, hidden_reads: int) -> None:
        super().__init__(harness)
        self.hidden_reads = hidden_reads
        self.dialog = True

    def read_screen(self, pane_id: str) -> str:
        if self.hidden_reads > 0:
            self.hidden_reads -= 1
            if self.hidden_reads == 0:
                self.dialog = False
        return super().read_screen(pane_id)


def test_late_composer_is_awaited_before_typing() -> None:
    agent = LateComposerAgent("claude", hidden_reads=3)
    receipt = _submit(agent, "the brief")
    assert receipt is not None
    assert agent.pastes == ["the brief"] and agent.submitted == ["the brief"]


def test_composer_wait_is_bounded_and_types_nothing() -> None:
    agent = FakeAgent("claude")
    agent.dialog = True
    clock = Clock()
    with pytest.raises(PromptNotStaged, match=r"after 2s .*nothing was typed"):
        submit_verified(
            agent, "w1:p1", "claude", "the brief", composer_timeout=2.0,
            sleep=clock.sleep, monotonic=clock.monotonic,
        )
    assert agent.pastes == [] and agent.keys == []
    assert clock.now - 1000.0 >= 2.0


def test_labelled_claude_top_border_still_frames_the_composer() -> None:
    agent = FakeAgent("claude")
    agent.top_border_label = "ultracode"
    view = composer_view("claude", agent._render())
    assert view is not None, "a label in the top border must not hide the composer"
    assert view.composer_solid.strip() == ""
    assert "ultracode" not in view.transcript
    assert "auto mode on" in view.footer

    receipt = _submit(agent, "reply to the owner")
    assert getattr(receipt, "key_presses") == 1
    assert agent.submitted == ["reply to the owner"]

    busy = FakeAgent("claude", busy=True)
    busy.top_border_label = "ultracode"
    _submit(busy, "after this turn")
    assert busy.queued == ["after this turn"]

    drafted = FakeAgent("claude")
    drafted.top_border_label = "ultracode"
    drafted.composer = "someone else's words"
    with pytest.raises(PromptNotStaged, match="refusing to append"):
        _submit(drafted, "mine")
    assert drafted.pastes == []


def test_labelled_rules_only_frame_a_composer_that_plain_rules_do_not() -> None:
    rule = "─" * 40
    # Plain rules win: a draft row shaped like a labelled rule stays part of the draft.
    view = composer_view("claude", f"• earlier\n{rule}\n❯ first line\n  ── notes ──\n{rule}\n  footer\n")
    assert view is not None and "── notes ──" in view.composer_solid
    assert view.transcript == "• earlier"
    # A labelled rule with no composer marker below it frames nothing.
    assert composer_view("claude", f"{rule} label ─\n  Replace goal?\n{rule}\n") is None
    # Edge runs of rule characters are required on both sides of a label.
    for row in ("label ───", "─── label", "─ x", "──"):
        assert not _is_labelled_rule(row), row
    for row in ("─── label ─", "── label ─────", "───"):
        assert _is_labelled_rule(row), row


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



# Rows Codex v0.159.1 drew at the bottom of its pane, read with
# `herdr pane read --source visible --format ansi`. Escape sequences, attributes and row
# layout are as captured; runs of padding are shortened, and the model name, working
# directory, links and prompt text were replaced with neutral words.
_V159_PROMPT = "Reply with exactly READY and nothing else."
_V159_SHADED_BLANK = "\x1b[0m\x1b[48;2;30;30;30m                    \x1b[0m"
_V159_EMPTY_COMPOSER = (
    "\x1b[0m\x1b[1m\x1b[38;2;190;142;250m\x1b[48;2;30;30;30m»\x1b[0m"
    "\x1b[48;2;30;30;30m \x1b[0m\x1b[2m\x1b[48;2;30;30;30mAsk Codex to do anything"
    "\x1b[0m\x1b[48;2;30;30;30m          \x1b[0m"
)
_V159_STAGED_COMPOSER = (
    "\x1b[0m\x1b[1m\x1b[38;2;190;142;250m\x1b[48;2;30;30;30m»\x1b[0m"
    "\x1b[48;2;30;30;30m Reply with exactly READY and nothing else.          \x1b[0m"
)
_V159_FOOTER = (
    "  \x1b[0m\x1b[38;2;246;226;183mmodel high\x1b[0m"
    "\x1b[38;2;132;132;132m · \x1b[0m\x1b[38;2;171;223;167m~/project\x1b[0m"
    "          \x1b[0m\x1b[38;2;132;132;132m⚠ \x1b[0m"
    "\x1b[38;2;196;167;103m1 warning\x1b[0m\x1b[38;2;132;132;132m · \x1b[0m"
    "\x1b[1m\x1b[38;2;220;220;220mf2\x1b[0m\x1b[38;2;132;132;132m to view\x1b[0m"
)
_V159_SESSION_FOOTER = (
    "  \x1b[0m\x1b[38;5;6msession\x1b[0m\x1b[2m · \x1b[0m"
    "\x1b[38;2;246;226;183mmodel high\x1b[0m\x1b[38;2;132;132;132m · \x1b[0m"
    "\x1b[38;2;171;223;167m~/project\x1b[0m\x1b[38;2;132;132;132m · \x1b[0m"
    "\x1b[38;2;242;205;205mReply READY\x1b[0m          \x1b[0m"
)
_V159_NOTICE = "\x1b[0m\x1b[2m• \x1b[0mTip: /status shows the session."
# The submitted prompt as the transcript shows it: Codex still draws › there.
_V159_SUBMITTED_PROMPT = (
    "\x1b[0m\x1b[1m\x1b[2m\x1b[48;2;40;40;40m› \x1b[0m"
    "\x1b[48;2;40;40;40mReply with exactly READY and nothing else.          \x1b[0m"
)
_V159_HOOK = "\x1b[0m\x1b[2m↳ Hook · \x1b[0mDiscovered 3 skills"
_V159_WORKING = (
    "\x1b[0m\x1b[1m\x1b[38;2;151;151;151m•\x1b[0m \x1b[0m"
    "\x1b[38;2;167;167;167mW\x1b[0m\x1b[38;2;118;118;118mo\x1b[0m"
    "\x1b[38;2;110;110;110mrking\x1b[0m \x1b[0m\x1b[2m(0s • \x1b[0m"
    "\x1b[1m\x1b[38;2;220;220;220mesc\x1b[0m\x1b[2m to interrupt)\x1b[0m"
)
_V159_REPLY = "\x1b[0m\x1b[2m• \x1b[0mREADY"
_V159_WORKED = "\x1b[0m\x1b[2m  Worked for 52s • 12:56 PM\x1b[0m"
_V159_COMPOSER_ROWS = [_V159_SHADED_BLANK, _V159_EMPTY_COMPOSER, _V159_SHADED_BLANK]


def _screen(*rows: str) -> str:
    return "\n".join(rows) + "\n"


# A freshly started session, before any prompt.
_V159_FRESH_IDLE = _screen(_V159_NOTICE, "", " ", *_V159_COMPOSER_ROWS, _V159_FOOTER)
# The same session with the prompt pasted and not yet submitted.
_V159_STAGED = _screen(
    _V159_NOTICE, "", " ", _V159_SHADED_BLANK, _V159_STAGED_COMPOSER, _V159_SHADED_BLANK,
    _V159_FOOTER,
)
# The first redraw after Enter: the prompt moved into the transcript.
_V159_SUBMITTED = _screen(
    _V159_NOTICE, "", _V159_SUBMITTED_PROMPT, "", " ", *_V159_COMPOSER_ROWS, _V159_FOOTER,
)
# The turn is running; the composer stays drawn below the progress row.
_V159_WORKING_SCREEN = _screen(
    _V159_SUBMITTED_PROMPT, "", "", _V159_HOOK, " ", _V159_WORKING, " ",
    *_V159_COMPOSER_ROWS, _V159_FOOTER,
)
# The working screen with its composer rows removed.
_V159_WORKING_WITHOUT_COMPOSER = _screen(
    _V159_SUBMITTED_PROMPT, "", "", _V159_HOOK, " ", _V159_WORKING, " ", _V159_FOOTER,
)
# Idle again after a finished turn, with the session footer.
_V159_SESSION_IDLE = _screen(
    _V159_SUBMITTED_PROMPT, "", _V159_REPLY, "", _V159_WORKED, "", " ",
    *_V159_COMPOSER_ROWS, _V159_SESSION_FOOTER,
)
# The folder-trust question Codex asks before its first composer in a new directory.
_V159_TRUST_PROMPT = _screen(
    _V159_SHADED_BLANK,
    "\x1b[0m\x1b[1m\x1b[48;2;30;30;30m  Folder access\x1b[0m\x1b[48;2;30;30;30m          \x1b[0m",
    "\x1b[0m\x1b[48;2;30;30;30m  \x1b[0m\x1b[2m\x1b[48;2;30;30;30m/tmp/project          "
    "\x1b[0m\x1b[48;2;30;30;30m  \x1b[0m",
    _V159_SHADED_BLANK,
    "\x1b[0m\x1b[48;2;30;30;30m  Trust this folder? Codex can read, edit, and run files here.   \x1b[0m",
    "\x1b[0m\x1b[48;2;30;30;30m  Continue only if you trust these files.          \x1b[0m",
    _V159_SHADED_BLANK,
    "\x1b[0m\x1b[1m\x1b[38;2;0;0;46m\x1b[48;2;99;168;248m› 1. Trust and continue          \x1b[0m",
    "\x1b[0m\x1b[48;2;30;30;30m  2. Quit          \x1b[0m",
    _V159_SHADED_BLANK,
    "\x1b[0m\x1b[48;2;30;30;30m  \x1b[0m\x1b[1m\x1b[38;2;220;220;220m"
    "\x1b[48;2;30;30;30menter\x1b[0m\x1b[2m\x1b[48;2;30;30;30m continue · \x1b[0m"
    "\x1b[1m\x1b[38;2;220;220;220m\x1b[48;2;30;30;30mesc\x1b[0m\x1b[2m"
    "\x1b[48;2;30;30;30m quit\x1b[0m\x1b[48;2;30;30;30m          \x1b[0m",
)


class Replay:
    """Plays captured screens back: before the paste, while staged, and after Enter.

    ``undrawn_reads`` holds the staged screen back for that many reads after the paste,
    as when a busy agent draws a paste late.
    """

    def __init__(self, before: str, staged: str, after: str) -> None:
        self.frames = (before, staged, after)
        self.phase = 0
        self.undrawn_reads = 0
        self.pastes: list[str] = []
        self.keys: list[str] = []

    def read_screen(self, pane_id: str) -> str:
        del pane_id
        if self.phase == 1 and self.undrawn_reads > 0:
            self.undrawn_reads -= 1
            return self.frames[0]
        return self.frames[self.phase]

    def send_text(self, pane_id: str, text: str) -> None:
        del pane_id
        self.pastes.append(text)
        self.phase = 1

    def send_keys(self, pane_id: str, keys: str) -> None:
        del pane_id
        self.keys.append(keys)
        if keys == "Enter":
            self.phase = 2


def _replay_submit(replay: Replay, text: str, harness: str = "codex") -> object:
    clock = Clock()
    return submit_verified(
        replay, "w1:p1", harness, text, sleep=clock.sleep, monotonic=clock.monotonic,
    )


@pytest.mark.parametrize(
    "screen", [_V159_FRESH_IDLE, _V159_SESSION_IDLE], ids=["fresh-start", "after-a-turn"],
)
def test_codex_0_159_1_idle_composer_is_recognised(screen: str) -> None:
    view = composer_view("codex", screen)
    assert view is not None, screen
    assert view.composer_solid.strip() == ""
    assert "Ask Codex to do anything" in view.composer
    assert "~/project" in view.footer
    assert "Ask Codex" not in view.transcript


@pytest.mark.parametrize("screen", [
    _V159_FRESH_IDLE, _V159_SESSION_IDLE, FakeAgent("codex")._render(),
    "›\n  ? for shortcuts\n", "»\n  99% context left\n",
])
def test_suggestion13_codex_unknown_readiness_recognizes_supported_idle_screens(screen: str) -> None:
    assert codex_idle_composer(screen)


@pytest.mark.parametrize("screen", [
    _V159_WORKING_SCREEN, _V159_WORKING_WITHOUT_COMPOSER, _V159_TRUST_PROMPT,
    _V159_STAGED,
    "› a human draft\n  GPT default · /work\n",
    "› \x1b[2m[Pasted text #1 +2 lines]\x1b[0m\n  GPT default · /work\n",
    "›\n  choose one\n",
    "• Working (0s • esc to interrupt)\n›\n  GPT default · /work\n",
    "Queued follow-up inputs\n›\n  GPT default · /work\n",
    "›\n  tab to queue message · /work\n",
    "Approval required\n›\n  GPT default · /work\n",
    "Trust this folder?\n›\n  GPT default · /work\n",
    "›\n  GPT default · /work · paste again to expand\n",
    "›\n  GPT default · /work · esc to cancel\n",
])
def test_suggestion13_codex_unknown_readiness_refuses_busy_drafts_and_dialogs(screen: str) -> None:
    assert not codex_idle_composer(screen)


def test_suggestion13_codex_unknown_readiness_ignores_old_status_words_above_active_rows() -> None:
    screen = "Approval required; esc to interrupt\n" + "old output\n" * 16 + _V159_FRESH_IDLE
    assert codex_idle_composer(screen)


@pytest.mark.parametrize("activity", [
    "Processing inputs (7s)", "• Pondering request (1m 2s • Ctrl+c to interrupt)",
    "◦ Checking context (1h 2m 3s)", "\x1b[2mCompiling reply (7s)\x1b[0m   ",
])
def test_suggestion13_codex_unknown_refuses_hidden_or_remapped_active_timers(activity: str) -> None:
    assert not codex_idle_composer(activity + "\n›\n  GPT default · /work\n")


@pytest.mark.parametrize("queue", [
    "Queued follow-up inputs", "Messages to be submitted after next tool call",
    "Messages to be submitted at end of turn",
])
def test_suggestion13_codex_unknown_refuses_active_timer_above_long_queue(queue: str) -> None:
    screen = "Processing inputs (7s)\n• " + queue + "\n" + "  queued text\n" * 20
    assert not codex_idle_composer(screen + "›\n  GPT default · /work\n")


@pytest.mark.parametrize("activity", [
    "Processing inputs (7s)", "• Pondering request (1m 2s • Ctrl+c to interrupt)",
    "• Working (2s • esc to interrupt)",
])
def test_suggestion13_codex_unknown_ignores_completed_timer_before_later_response(activity: str) -> None:
    assert codex_idle_composer(activity + "\n■ Completed response\n›\n  GPT default · /work\n")


@pytest.mark.parametrize("cue", [
    "enter to submit answer", "enter to submit all", "allow command?",
    "Messages to be submitted after next tool call", "Messages to be submitted at end of turn",
    "Ctrl+c to interrupt", "Do you trust the contents of this directory?",
])
def test_suggestion13_codex_unknown_refuses_manifest_dialog_and_queue_cues(cue: str) -> None:
    assert not codex_idle_composer(cue + "\n›\n  GPT default · /work\n")


@pytest.mark.parametrize("menu", [
    "All Results   Filesystem Only   Plugins", "all RESULTS\nfilesystem ONLY\nPLUGINS",
])
def test_suggestion13_codex_unknown_refuses_manifest_grouped_active_menu(menu: str) -> None:
    assert not codex_idle_composer(menu + "\n»\n  ? for shortcuts\n")


@pytest.mark.parametrize("labels", [
    "All Results", "Filesystem Only", "Plugins", "All Results   Filesystem Only",
    "All Results   Plugins", "Filesystem Only   Plugins",
])
def test_suggestion13_codex_unknown_does_not_treat_partial_menu_labels_as_active_menu(labels: str) -> None:
    assert codex_idle_composer(labels + "\n»\n  ? for shortcuts\n")


def test_suggestion13_codex_unknown_ignores_grouped_menu_outside_active_window() -> None:
    screen = "All Results   Filesystem Only   Plugins\n" + "historical output\n" * 16
    assert codex_idle_composer(screen + "»\n  ? for shortcuts\n")


def test_codex_0_159_1_prompt_is_staged_submitted_and_verified() -> None:
    replay = Replay(_V159_FRESH_IDLE, _V159_STAGED, _V159_SUBMITTED)
    receipt = _replay_submit(replay, _V159_PROMPT)
    assert getattr(receipt, "key") == "Enter"
    assert getattr(receipt, "key_presses") == 1
    assert getattr(receipt, "evidence") == "prompt text appeared above the composer"
    assert replay.pastes == [f"{_PASTE_START}{_V159_PROMPT}{_PASTE_END}"]
    assert replay.keys == ["Enter"]


def test_codex_0_159_1_working_screen_keeps_its_progress_row_out_of_the_draft() -> None:
    view = composer_view("codex", _V159_WORKING_SCREEN)
    assert view is not None
    assert view.composer_solid.strip() == ""
    assert "esc to interrupt" in view.transcript
    assert "tab to queue message" not in view.footer


@pytest.mark.parametrize(
    "screen", [_V159_WORKING_WITHOUT_COMPOSER, _V159_TRUST_PROMPT],
    ids=["working-without-composer", "folder-trust-prompt"],
)
def test_codex_0_159_1_screens_without_a_composer_are_refused_before_typing(screen: str) -> None:
    replay = Replay(screen, screen, screen)
    with pytest.raises(PromptNotStaged, match="nothing was typed"):
        _replay_submit(replay, "hello")
    assert replay.pastes == [] and replay.keys == []


def test_codex_0_159_1_working_screen_without_composer_has_no_view() -> None:
    assert composer_view("codex", _V159_WORKING_WITHOUT_COMPOSER) is None


@pytest.mark.parametrize("marker", ["›", "»"], ids=["before-0.159.1", "0.159.1"])
def test_codex_flows_hold_under_both_composer_markers(marker: str) -> None:
    idle = FakeAgent("codex")
    idle.codex_marker = marker
    view = composer_view("codex", idle._render())
    assert view is not None and view.composer_solid.strip() == ""
    assert getattr(_submit(idle, "start now"), "key") == "Enter"
    assert idle.submitted == ["start now"]

    busy = FakeAgent("codex", busy=True)
    busy.codex_marker = marker
    busy.drop_keys = 1
    assert getattr(_submit(busy, "follow up later"), "key") == "Tab"
    assert busy.queued == ["follow up later"] and busy.steered == []

    drafted = FakeAgent("codex")
    drafted.codex_marker = marker
    drafted.composer = "someone else's words"
    with pytest.raises(PromptNotStaged, match="refusing to append"):
        _submit(drafted, "mine")
    assert drafted.pastes == []

    dialog = FakeAgent("codex")
    dialog.codex_marker = marker
    dialog.dialog = True
    with pytest.raises(PromptNotStaged, match="recognisable codex composer"):
        _submit(dialog, "hello")
    assert dialog.pastes == []


# Rows Claude Code drew at the bottom of its pane while a turn was running, read with
# `herdr pane read --source visible --format ansi`: before a paste, with an 11-line paste
# collapsed into a placeholder, and after one Enter queued that paste. The composer, rule
# and footer rows are as captured, escape sequences included, except that runs of padding
# and rule characters are shortened and the placeholder carries the paste number of the
# session the bridge failed on. The rows above them are simplified. Among the differences:
# their words are neutral, a 10-row file preview and a hint row are left out, the later
# frames reuse the first frame's transcript rows and the staged frame its spinner row, and
# the queued prompt rows lack their trailing reset.
_CLAUDE_BUSY_PROMPT = "\n".join(
    ["Reply with one line for each item below."] + [f"item {n}" for n in range(2, 12)]
)
_CLAUDE_BUSY_TURN = (
    "\x1b[0m\x1b[38;5;114m● \x1b[0m\x1b[1mRead\x1b[0m(notes.md)",
    "\x1b[0m\x1b[38;5;246m  ⎿ \xa0\x1b[0mRead \x1b[0m\x1b[1m45\x1b[0m lines\x1b[0m",
    "",
    "\x1b[0m\x1b[38;5;246m  \x1b[0mRunning the test suite\x1b[0m\x1b[38;5;246m · 3s\x1b[0m",
    "\x1b[0m\x1b[38;5;246m  ⎿  $ cargo test\x1b[0m",
)
_CLAUDE_BUSY_SPINNER = (
    "\x1b[0m\x1b[38;5;174m·\x1b[0m \x1b[0m\x1b[38;5;180mFermenting…\x1b[0m"
    "\x1b[38;5;174m \x1b[0m\x1b[38;5;246m(16m 8s · ↓\x1b[0m \x1b[0m"
    "\x1b[38;5;246m12.3k tokens)\x1b[0m"
)
# The spinner just after Enter, while Claude Code runs its prompt hooks.
_CLAUDE_BUSY_SPINNER_HOOKS = (
    "\x1b[0m\x1b[38;5;174m✢\x1b[0m \x1b[0m\x1b[38;5;216mFermenting…\x1b[0m"
    "\x1b[38;5;174m \x1b[0m\x1b[38;5;246m(running UserPromptSubmit hooks… 8/10 · "
    "16m 9s · ↓ 12.3k tokens)\x1b[0m"
)
_CLAUDE_BUSY_PADDING = " " * 20
_CLAUDE_BUSY_RULE = "\x1b[0m\x1b[38;5;244m" + "─" * 40 + "\x1b[0m"
_CLAUDE_BUSY_WORKING_FOOTER = (
    "  \x1b[0m\x1b[38;5;220m⏵⏵ auto mode on\x1b[0m"
    "\x1b[38;5;246m (shift+tab to cycle) · esc to interrupt · ← 2 agents\x1b[0m"
)
# While a placeholder is in the composer, this replaces the running-turn footer.
_CLAUDE_BUSY_PASTE_FOOTER = "  \x1b[0m\x1b[38;5;246mpaste again to expand\x1b[0m"
# The running turn with an empty composer, before the paste.
_CLAUDE_BUSY_BEFORE = _screen(
    *_CLAUDE_BUSY_TURN, "", _CLAUDE_BUSY_SPINNER, _CLAUDE_BUSY_PADDING, _CLAUDE_BUSY_RULE,
    "\x1b[0m\x1b[38;5;246m❯\xa0\x1b[0m\x1b[7m \x1b[0m",
    _CLAUDE_BUSY_RULE, _CLAUDE_BUSY_WORKING_FOOTER,
)
# The same turn with the prompt pasted: only its placeholder is drawn.
_CLAUDE_BUSY_STAGED = _screen(
    *_CLAUDE_BUSY_TURN, "", _CLAUDE_BUSY_SPINNER, _CLAUDE_BUSY_PADDING, _CLAUDE_BUSY_RULE,
    "\x1b[0m\x1b[38;5;246m❯\xa0\x1b[0m[Pasted text #923 +10 lines]\x1b[0m\x1b[7m \x1b[0m",
    _CLAUDE_BUSY_RULE, _CLAUDE_BUSY_PASTE_FOOTER,
)
# After one Enter: the prompt is queued above the spinner, in full.
_CLAUDE_BUSY_QUEUED = _screen(
    *_CLAUDE_BUSY_TURN, "     ",
    *(
        ("\x1b[0m\x1b[38;5;239m\x1b[48;5;237m❯ " if index == 0 else "\x1b[0m\x1b[48;5;237m  ")
        + f"\x1b[0m\x1b[38;5;246m\x1b[48;5;237m{line}\x1b[0m\x1b[48;5;237m{_CLAUDE_BUSY_PADDING}"
        for index, line in enumerate(_CLAUDE_BUSY_PROMPT.splitlines())
    ),
    "", _CLAUDE_BUSY_SPINNER_HOOKS, _CLAUDE_BUSY_PADDING, _CLAUDE_BUSY_RULE,
    "\x1b[0m\x1b[38;5;246m❯\xa0\x1b[0m\x1b[7mP\x1b[0m\x1b[2mress up to edit queued messages\x1b[0m",
    _CLAUDE_BUSY_RULE, _CLAUDE_BUSY_PASTE_FOOTER,
)


def test_busy_claude_screens_show_the_paste_only_as_a_placeholder() -> None:
    before = composer_view("claude", _CLAUDE_BUSY_BEFORE)
    assert before is not None
    assert before.composer_solid.strip() == ""
    assert "[Pasted" not in before.composer
    assert "esc to interrupt" in before.footer

    pasted = composer_view("claude", _CLAUDE_BUSY_STAGED)
    assert pasted is not None
    assert pasted.composer_solid.strip() == "[Pasted text #923 +10 lines]"
    assert _staged(pasted, _CLAUDE_BUSY_PROMPT, 0)
    # The footer stops saying that a turn is running while the placeholder is drawn.
    assert "esc to interrupt" not in pasted.footer
    assert "paste again to expand" in pasted.footer

    queued = composer_view("claude", _CLAUDE_BUSY_QUEUED)
    assert queued is not None
    assert queued.composer_solid.strip() == ""
    assert not _staged(queued, _CLAUDE_BUSY_PROMPT, 0)
    assert _corroborated(before, queued, _CLAUDE_BUSY_PROMPT) == (
        "prompt text appeared above the composer"
    )


def test_placeholder_left_in_a_busy_claude_composer_holds_the_next_prompt() -> None:
    replay = Replay(_CLAUDE_BUSY_STAGED, _CLAUDE_BUSY_STAGED, _CLAUDE_BUSY_STAGED)
    with pytest.raises(PromptNotStaged, match="refusing to append"):
        _replay_submit(replay, "the next prompt", harness="claude")
    assert replay.pastes == [] and replay.keys == []


# Verified submission once saw no paste in a busy Claude Code composer within 3 s of typing
# it; the paste was there later, unsubmitted, and held later prompts:
# https://github.com/rrnewton/agent-utils/issues/215
def test_busy_claude_paste_drawn_late_is_submitted_with_one_enter() -> None:
    replay = Replay(_CLAUDE_BUSY_BEFORE, _CLAUDE_BUSY_STAGED, _CLAUDE_BUSY_QUEUED)
    replay.undrawn_reads = 45
    receipt = _replay_submit(replay, _CLAUDE_BUSY_PROMPT, harness="claude")
    assert getattr(receipt, "key") == "Enter"
    assert getattr(receipt, "key_presses") == 1
    assert getattr(receipt, "evidence") == "prompt text appeared above the composer"
    # Reads 0.1 s apart missed the placeholder for 4.5 s; the evidence came one poll after
    # the Enter.
    assert getattr(receipt, "elapsed_seconds") == 4.6
    assert replay.pastes == [f"{_PASTE_START}{_CLAUDE_BUSY_PROMPT}{_PASTE_END}"]
    assert replay.keys == ["Enter"]


def test_busy_claude_paste_never_drawn_gives_up_at_the_stage_timeout_without_a_key() -> None:
    replay = Replay(_CLAUDE_BUSY_BEFORE, _CLAUDE_BUSY_STAGED, _CLAUDE_BUSY_QUEUED)
    replay.undrawn_reads = 1_000_000
    clock = Clock()
    with pytest.raises(HerdrUnavailable, match="within 60s; no submission key was sent") as caught:
        submit_verified(
            replay, "w1:p1", "claude", _CLAUDE_BUSY_PROMPT,
            sleep=clock.sleep, monotonic=clock.monotonic,
        )
    assert not isinstance(caught.value, (PromptNotStaged, PromptStagedNotSubmitted))
    # The clock adds 0.1 s per poll in floating point, so the read that reaches the deadline
    # can land one poll after it.
    assert 60.0 <= clock.now - 1000.0 < 60.2
    assert len(replay.pastes) == 1 and replay.keys == []


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
        #: The status Herdr reports; None follows the agent (working while busy).
        self.status: str | None = "idle"

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        status = self.status if self.status is not None else ("working" if self.agent.busy else "idle")
        return AgentPaneInfo(pane_id, "w1", "/work/project", self.agent.harness,
                             status, None, None)


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


def test_a_working_claude_is_typed_into_only_with_deliver_while_working(tmp_path: Path) -> None:
    agent = FakeAgent("claude", busy=True)
    client = ProcessBackedClient(agent)
    client.status = "working"
    root = tmp_path / "queue"
    enqueue(str(root), "queue behind the turn", message_id="m1")
    waited = drain(client, Target(pane_id="w1:p1"), str(root), ready_timeout=0, working_timeout=1.0)
    assert waited.pending == ("m1",) and "did not become idle/done" in (waited.blocked or "")
    assert agent.pastes == []
    result = drain(client, Target(pane_id="w1:p1"), str(root), ready_timeout=0, working_timeout=1.0,
                   deliver_while_working=True)
    assert result.delivered == ("m1",), result
    assert agent.queued == ["queue behind the turn"] and agent.composer == ""


def test_a_working_claude_that_never_queues_the_prompt_gets_it_taken_back_out(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    import agentctl.submission as submission

    monkeypatch.setattr(submission, "SUBMIT_TIMEOUT_SECONDS", 0.3)
    agent = FakeAgent("claude", busy=True)
    agent.drop_submit_keys = True
    client = ProcessBackedClient(agent)
    client.status = "working"
    root = tmp_path / "queue"
    enqueue(str(root), "never queued", message_id="m1")
    result = drain(client, Target(pane_id="w1:p1"), str(root), ready_timeout=0, working_timeout=1.0,
                   deliver_while_working=True)
    assert result.pending == ("m1",) and result.quarantined == (), result
    assert "taken back out of the composer" in (result.blocked or "")
    assert agent.composer == "" and agent.queued == [] and "ctrl+u" in agent.keys
    document = json.loads((root / "inbox" / "m1.json").read_text())
    assert "possibly_submitted" not in document


def test_a_slash_command_still_waits_for_an_idle_claude(tmp_path: Path) -> None:
    agent = FakeAgent("claude", busy=True)
    client = ProcessBackedClient(agent)
    client.status = "working"
    root = tmp_path / "queue"
    enqueue(str(root), "/goal finish", message_id="m1")
    result = drain(client, Target(pane_id="w1:p1"), str(root), ready_timeout=0, working_timeout=1.0,
                   deliver_while_working=True)
    assert result.pending == ("m1",) and agent.pastes == []


def test_clear_composer_empties_a_multi_row_draft_without_submitting() -> None:
    from agentctl.submission import clear_composer

    clock = Clock()
    agent = FakeAgent("claude")
    agent.composer = "first row\nsecond row"
    keys = clear_composer(agent, "w1:p1", "claude", sleep=clock.sleep)
    assert agent.composer == "" and keys == len(agent.keys)
    assert set(agent.keys) <= {"ctrl+e", "ctrl+u", "backspace"}
    assert clear_composer(agent, "w1:p1", "claude", sleep=clock.sleep) == 0


def test_the_cli_passes_deliver_while_working_only_when_asked() -> None:
    from agentctl import cli

    parsed = cli.parser().parse_args(["send", "worker", "--deliver-while-working", "steer"])
    assert parsed.deliver_while_working is True
    parsed = cli.parser().parse_args(["send", "worker", "steer", "--deliver-while-working"])
    assert parsed.deliver_while_working is True
    assert cli.parser().parse_args(["send", "worker", "steer"]).deliver_while_working is False


def test_the_cli_refuses_a_value_starved_option_instead_of_shifting_a_name() -> None:
    """A trailing option without its value is a usage error on every Python.

    The leaf regrouping that lets older argparse intermix options with an
    optional positional must never place the name behind such an option,
    where it would be taken as the missing value.
    """
    from agentctl import cli

    with pytest.raises(SystemExit):
        cli.parser().parse_args(["revive", "worker", "--expected-token"])
    with pytest.raises(SystemExit):
        cli.parser().parse_args(["send", "worker", "--file"])


# Shared terminal form captures and guarded question-answering contracts.
ComposerTerminal = FakeAgent

FIXTURES = Path(__file__).resolve().parents[2] / "rs" / "agentctl" / "tests" / "fixtures" / "tui"


def adapter(name: str = "codex") -> HarnessTui:
    value = harness_tui(name)
    assert value is not None
    return value


def test_shared_state_snapshots() -> None:
    for row in json.loads((FIXTURES / "states.json").read_text()):
        assert adapter(row["harness"]).classify(row["screen"]).state == row["state"]
    assert harness_tui("unknown") is None


@pytest.mark.parametrize("kind", ["options", "async-options", "freeform", "async-freeform", "multi_question_first", "hidden_options_footer", "options_notes_visible"])
def test_upstream_codex_snapshots(kind: str) -> None:
    view = adapter().classify((FIXTURES / f"codex-{kind}.txt").read_text())
    assert view.state == State.QUESTION
    assert view.question is not None and view.question.text
    if kind == "options":
        assert [o.number for o in view.question.options] == [1, 2, 3, 4]
        assert view.question.selected == 1
        assert adapter().answer_plan(view.question, 2, None).keys == ("Down",)
        assert adapter().answer_plan(view.question, None, "Alternative").keys == ("Down", "Down", "Down", "Tab")
    elif kind == "async-options":
        assert [o.number for o in view.question.options] == [1, 2, 3]
        assert view.question.selected == 1
        assert adapter().answer_plan(view.question, 2, None).keys == ("Down",)
    elif kind in ("freeform", "async-freeform"):
        assert adapter().answer_plan(view.question, None, "Details").text == "Details"
    else:
        with pytest.raises(ValueError):
            adapter().answer_plan(view.question, 2, None)


class FormTerminal:
    """Replay a captured form; apply the few actual key effects the adapter requests."""

    def __init__(self, kind: str = "options", *, collapsed: bool = False, drop: bool = False) -> None:
        self.screen = (FIXTURES / f"codex-{kind}.txt").read_text()
        self.collapsed = collapsed
        self.drop = drop
        self.keys: list[str] = []
        self.texts: list[str] = []
        self.seconds = 0.0

    def read_screen(self, pane: str) -> str:
        assert pane == "w1:p1"
        return "• Queued follow-up inputs\n  ? 1 question\n  shift+← to answer" if self.collapsed else self.screen

    def send_keys(self, pane: str, key: str) -> None:
        assert pane == "w1:p1"
        self.keys.append(key)
        if key == "shift+left":
            self.collapsed = False
        elif key in ("Down", "Up"):
            q = adapter().classify(self.screen).question
            assert q is not None and q.selected is not None
            selected = q.selected + (1 if key == "Down" else -1)
            self.screen = self.screen.replace("› ", "  ")
            self.screen = self.screen.replace(f"  {selected}. ", f"› {selected}. ")
        elif key == "Tab":
            self.screen = self.screen.replace("tab to add notes", "tab or esc to clear notes")
            self.screen = self.screen.replace("  tab or esc", "  › Add notes\n\n  tab or esc")
        elif key == "Enter" and not self.drop:
            self.screen = "• Working (2s • esc to interrupt)\n»\n  GPT default · /work/project\n"

    def send_text(self, pane: str, text: str) -> None:
        assert pane == "w1:p1"
        self.texts.append(text)
        value = text.removeprefix("\x1b[200~").removesuffix("\x1b[201~")
        self.screen = self.screen.replace("› Type your answer (optional)", "› " + value)
        self.screen = self.screen.replace("› Add notes", "› " + value)
        self.screen = self.screen.replace("  Type your answer\n", "  " + value + "\n")

    def native_prompt(self, pane: str, text: str) -> None:
        raise AssertionError("answer must not use ordinary prompt delivery")

    def sleep(self, seconds: float) -> None:
        self.seconds += seconds


@pytest.mark.parametrize("kind", ["options", "async-options", "rate-limit"])
@pytest.mark.parametrize("collapsed", [False, True])
def test_option_answer_opens_navigates_and_verifies_dismissal(collapsed: bool, kind: str) -> None:
    terminal = FormTerminal(kind, collapsed=collapsed)
    result = answer(terminal, "w1:p1", adapter(), option=2, verify=lambda p: None,
                    sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert result["answered"] is True
    assert terminal.keys == (["shift+left"] if collapsed else []) + ["Down", "Enter"]
    assert not terminal.texts


def test_rate_limit_reminder_requires_complete_unchanged_visible_menu() -> None:
    screen = (FIXTURES / "codex-rate-limit.txt").read_text()
    question = adapter().classify(screen).question
    assert question is not None and question.selected == 1
    assert question.options[1].text == "Keep current model"
    for broken in (
        screen.replace("  2. Keep current model\n", ""),
        screen.replace("Keep current model\n", "Change current model\n"),
        screen.replace("  2.", "› 2."),
        screen.replace("gpt-6-luna for lower", "a different model for lower"),
        screen.replace("enter select · esc back", "enter confirm"),
        screen + "\n»\n  GPT default · /work/project\n",
    ):
        assert adapter().classify(broken).state != State.QUESTION
    terminal = FormTerminal("rate-limit", drop=True)
    with pytest.raises(AgentPossiblySubmitted):
        answer(terminal, "w1:p1", adapter(), option=2, verify=lambda p: None,
               sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert terminal.keys == ["Down", "Enter"]


@pytest.mark.parametrize("kind", ["freeform", "async-freeform", "options"])
def test_literal_text_answer(kind: str) -> None:
    terminal = FormTerminal(kind)
    assert answer(terminal, "w1:p1", adapter(), text="A literal answer", verify=lambda p: None,
                  sleep=terminal.sleep, monotonic=lambda: terminal.seconds)["answered"] is True
    assert terminal.keys.count("Enter") == 1
    assert terminal.texts == ["\x1b[200~A literal answer\x1b[201~"]


def test_async_question_refuses_stale_capture_and_existing_answer() -> None:
    screen = (FIXTURES / "codex-async-freeform.txt").read_text()
    assert adapter().classify(screen + "\nshell$\n").state != State.QUESTION
    terminal = FormTerminal("async-freeform")
    terminal.screen = screen.replace("Type your answer", "Existing draft")
    with pytest.raises(AgentDeliveryError):
        answer(terminal, "w1:p1", adapter(), text="Replacement", verify=lambda p: None)
    assert not terminal.texts and not terminal.keys


@pytest.mark.parametrize("fault", ["none", "dropped-clear", "changed-question", "changed-recipient", "folded-paste", "dropped-submit"])
def test_explicit_free_text_draft_replacement(fault: str) -> None:
    class DraftTerminal(FormTerminal):
        def send_keys(self, pane: str, key: str) -> None:
            super().send_keys(pane, key)
            if key == "ctrl+u" and fault != "dropped-clear":
                self.screen = self.screen.replace("[Pasted Content 1672 chars]", "Type your answer")
                if fault == "changed-question":
                    self.screen = self.screen.replace("Is there an admission map for these source fixtures?", "A different question?")
        def send_text(self, pane: str, text: str) -> None:
            super().send_text(pane, text)
            if fault == "folded-paste":
                self.screen = self.screen.replace("Short replacement", "[Pasted Content 17 chars]")
    terminal = DraftTerminal("async-freeform", drop=fault == "dropped-submit")
    terminal.screen = terminal.screen.replace("Type your answer", "[Pasted Content 1672 chars]")
    def verify(pane: str) -> None:
        if fault == "changed-recipient" and terminal.keys:
            raise AgentDeliveryError("recipient replaced")
    if fault == "none":
        assert answer(terminal, "w1:p1", adapter(), text="Short replacement", replace_draft=True,
                      verify=verify, sleep=terminal.sleep, monotonic=lambda: terminal.seconds)["answered"] is True
        assert terminal.keys == ["ctrl+u", "Enter"]
    else:
        with pytest.raises(AgentPossiblySubmitted):
            answer(terminal, "w1:p1", adapter(), text="Short replacement", replace_draft=True,
                      verify=verify, sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
        assert terminal.keys == (["ctrl+u", "Enter"] if fault == "dropped-submit" else ["ctrl+u"])
        if fault in ("dropped-clear", "changed-question", "changed-recipient"):
            assert not terminal.texts


@pytest.mark.parametrize("kind,text,option", [("options", "replacement", None),
    ("multi_question_first", "replacement", None), ("async-freeform", "bad\ntext", None),
    ("async-freeform", None, 1)])
def test_draft_replacement_refuses_before_effect(kind: str, text: str | None, option: int | None) -> None:
    terminal = FormTerminal(kind)
    with pytest.raises(AgentDeliveryError):
        answer(terminal, "w1:p1", adapter(), text=text, option=option, replace_draft=True,
               verify=lambda p: None)
    assert not terminal.keys and not terminal.texts


def test_async_question_never_repeats_an_unacknowledged_submit() -> None:
    terminal = FormTerminal("async-freeform", drop=True)
    with pytest.raises(AgentPossiblySubmitted):
        answer(terminal, "w1:p1", adapter(), text="Continue drafting", verify=lambda p: None,
               sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert terminal.keys == ["Enter"]


def test_dropped_submit_is_uncertain_and_never_retried() -> None:
    terminal = FormTerminal(drop=True)
    with pytest.raises(AgentPossiblySubmitted) as exc:
        answer(terminal, "w1:p1", adapter(), option=1, verify=lambda p: None,
               sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert exc.value.exit_code == 76
    assert terminal.keys == ["Enter"]


@pytest.mark.parametrize("kind,option,text", [("options", 99, None), ("options", 4, None),
    ("freeform", None, "\x1bBAD"), ("freeform", None, "two\nlines"),
    ("multi_question_first", 1, None), ("hidden_options_footer", 2, None)])
def test_refusal_before_input(kind: str, option: int | None, text: str | None) -> None:
    terminal = FormTerminal(kind)
    with pytest.raises(AgentDeliveryError) as exc:
        answer(terminal, "w1:p1", adapter(), option=option, text=text, verify=lambda p: None)
    assert exc.value.exit_code == 75
    assert not terminal.keys and not terminal.texts


def test_recipient_failure_before_effect_never_sends() -> None:
    terminal = FormTerminal()
    def refuse(pane: str) -> None:
        raise AgentDeliveryError("recipient replaced")
    with pytest.raises(AgentDeliveryError, match="recipient replaced"):
        answer(terminal, "w1:p1", adapter(), option=1, verify=refuse)
    assert not terminal.keys


def test_question_cannot_be_confused_with_a_menu_or_quoted_transcript() -> None:
    terminal = FormTerminal()
    terminal.screen = "Choose model?\n› 1. Fast\n  enter to select | esc to cancel"
    with pytest.raises(AgentDeliveryError, match="no supported"):
        answer(terminal, "w1:p1", adapter(), option=1, verify=lambda p: None)
    assert not terminal.keys
    for name in ("claude", "codex"):
        assert adapter(name).classify("Transcript: enter to submit answer\n$ shell").state != State.QUESTION


def test_cli_selection_is_mutually_exclusive_and_required() -> None:
    assert cli.parser().parse_args(["prompts", "worker"]).command == "prompts"
    assert cli.parser().parse_args(["answer", "worker", "--text", "x", "--replace-draft"]).replace_draft
    for args in (["answer", "worker"], ["answer", "worker", "--option", "1", "--text", "x"]):
        with pytest.raises(SystemExit):
            cli.parser().parse_args(args)


def test_busy_codex_uses_followup_but_slash_commands_stay_pending(tmp_path: Path) -> None:
    class Busy(FakeAgentHerdr):
        def read(self, pane_id: str, *, source: str, lines: int) -> str:
            if source == "visible":
                return "• Working (2s • esc to interrupt)\n»\n  GPT default · /work/project\n"
            return super().read(pane_id, source=source, lines=lines)
    fake = Busy(["working"])
    result = send(cast(HerdrClient, fake), target(), str(tmp_path), "follow up", ready_timeout=0)
    assert result.delivered
    assert len(fake.runs) == 1
    with pytest.raises(AgentPending):
        send(cast(HerdrClient, fake), target(), str(tmp_path), "/model", ready_timeout=0)
    assert len(fake.runs) == 1


def test_busy_codex_verified_transport_uses_enter(tmp_path: Path) -> None:
    terminal = ComposerTerminal("codex", busy=True)
    class Busy(FakeAgentHerdr):
        def read(self, pane_id: str, *, source: str, lines: int) -> str:
            return terminal.read_screen(pane_id) if source == "visible" else super().read(pane_id, source=source, lines=lines)

        def prompt_agent(self, pane_id: str, text: str) -> SubmissionReceipt:
            return submit_verified(terminal, pane_id, "codex", text)

    result = send(cast(HerdrClient, Busy(["working"])), target(), str(tmp_path), "Act on this follow-up", ready_timeout=0)
    assert result.delivered
    assert terminal.keys == ["Enter"]
    assert terminal.steered == ["Act on this follow-up"]
    assert terminal.queued == []


def test_blocked_send_retains_pending_and_names_question_and_answer(tmp_path: Path) -> None:
    class Blocked(FakeAgentHerdr):
        def read(self, pane_id: str, *, source: str, lines: int) -> str:
            return (FIXTURES / "codex-options.txt").read_text()
    fake = Blocked(["blocked"])
    with pytest.raises(AgentPending, match="Choose an option.*agentctl answer"):
        send(cast(HerdrClient, fake), target(), str(tmp_path), "Later work", ready_timeout=0)
    assert not fake.runs
    assert len(list((tmp_path / "inbox").glob("*.json"))) == 1


def test_managed_inspection_is_read_only_and_paused_answer_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path))
    terminal = FormTerminal()
    monkeypatch.setattr(fake, "read_screen", terminal.read_screen, raising=False)
    before = sorted(str(p.relative_to(tmp_path)) for p in tmp_path.rglob("*"))
    result = sessions.prompts("worker")
    assert result["state"] == State.QUESTION
    assert not terminal.keys and not terminal.texts and not fake.submitted
    assert sorted(str(p.relative_to(tmp_path)) for p in tmp_path.rglob("*")) == before
    sessions.pause("worker")
    assert sessions.prompts("worker")["state"] == State.QUESTION
    with pytest.raises(AgentDeliveryError, match="paused"):
        sessions.prompts("worker", option=1, answering=True)
    assert not terminal.keys


def test_managed_answer_rechecks_process_and_uses_guarded_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path))
    terminal = FormTerminal()
    fake.expect_supported = True
    monkeypatch.setattr(fake, "read_screen", terminal.read_screen, raising=False)
    def keys(pane: str, key: str, *, expect_terminal: str | None = None) -> None:
        fake._effect(pane, expect_terminal)
        terminal.send_keys(pane, key)
    monkeypatch.setattr(fake, "send_keys", keys)
    assert sessions.prompts("worker", option=1, answering=True)["answered"] is True
    assert terminal.keys == ["Enter"]
    assert fake.expected_terminals == ["term-1"]


def test_inspection_reveals_only_and_never_selects_or_submits() -> None:
    from agentctl.tui_prompts import inspect_question
    terminal = FormTerminal(collapsed=True)
    view = inspect_question(terminal, "w1:p1", adapter(), verify=lambda p: None,
                            sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert view.question is not None and view.question.selected == 1
    assert terminal.keys == ["shift+left"] and not terminal.texts
    inspect_question(terminal, "w1:p1", adapter(), verify=lambda p: None)
    assert terminal.keys == ["shift+left"]


def test_paused_managed_inspection_reveals_with_guarded_input_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path))
    sessions.pause("worker")
    terminal = FormTerminal(collapsed=True)
    fake.expect_supported = True
    monkeypatch.setattr(fake, "read_screen", terminal.read_screen, raising=False)
    def keys(pane: str, key: str, *, expect_terminal: str | None = None) -> None:
        fake._effect(pane, expect_terminal)
        terminal.send_keys(pane, key)
    monkeypatch.setattr(fake, "send_keys", keys)
    result = sessions.prompts("worker")
    assert result["state"] == State.QUESTION
    assert terminal.keys == ["shift+left"]
    assert fake.expected_terminals == ["term-1"]
    assert not terminal.texts and not fake.submitted
    with pytest.raises(AgentDeliveryError, match="paused"):
        sessions.prompts("worker", option=2, answering=True)
    assert terminal.keys == ["shift+left"]


def test_inspection_stops_after_reveal_if_the_form_is_unrecognized() -> None:
    from agentctl.tui_prompts import inspect_question
    terminal = FormTerminal(collapsed=True)
    terminal.screen = "Choose model?\n› 1. Fast\n  enter to select | esc to cancel"
    with pytest.raises(AgentPossiblySubmitted, match="question did not open"):
        inspect_question(terminal, "w1:p1", adapter(), verify=lambda p: None,
                         sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert terminal.keys == ["shift+left"] and not terminal.texts


@pytest.mark.parametrize("old,new", [
    ("› 1.", "  1."),
    ("    2.", "  › 2."),
    ("    2.", "    4."),
    ("    3. Other", "    … more options"),
    ("    3. Other", "    3. Other\n    › Existing draft"),
    ("shift+→ main prompt", "shift+→ main prompt\nshell$"),
])
def test_async_options_refuse_incomplete_or_ambiguous_forms(old: str, new: str) -> None:
    terminal = FormTerminal("async-options")
    terminal.screen = terminal.screen.replace(old, new)
    with pytest.raises(AgentDeliveryError):
        answer(terminal, "w1:p1", adapter(), option=2, verify=lambda p: None)
    assert not terminal.keys and not terminal.texts


def test_async_options_never_repeat_uncertain_submit() -> None:
    terminal = FormTerminal("async-options", drop=True)
    with pytest.raises(AgentPossiblySubmitted):
        answer(terminal, "w1:p1", adapter(), option=2, verify=lambda p: None,
               sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    assert terminal.keys == ["Down", "Enter"]


@pytest.mark.parametrize("drop", [False, True])
def test_explicit_matching_wrapped_draft_is_confirmed_without_retyping(drop: bool) -> None:
    terminal = FormTerminal("async-freeform", drop=drop)
    terminal.screen = terminal.screen.replace("Type your answer", "Keep the paid lifecycle and\n  prepare separate boundary companions.")
    text = "Keep the paid lifecycle and prepare separate boundary companions."
    if drop:
        with pytest.raises(AgentPossiblySubmitted):
            answer(terminal, "w1:p1", adapter(), text=text, verify=lambda p: None,
                   sleep=terminal.sleep, monotonic=lambda: terminal.seconds)
    else:
        assert answer(terminal, "w1:p1", adapter(), text=text, verify=lambda p: None,
                      sleep=terminal.sleep, monotonic=lambda: terminal.seconds)["answered"]
    assert terminal.keys == ["Enter"] and not terminal.texts


def test_wrapped_freeform_paste_is_verified_before_single_submit() -> None:
    class Wrapped(FormTerminal):
        def send_text(self, pane: str, text: str) -> None:
            super().send_text(pane, text)
            self.screen = self.screen.replace("paid lifecycle and prepare", "paid lifecycle and\n  prepare")
    terminal = Wrapped("async-freeform")
    assert answer(terminal, "w1:p1", adapter(), text="Keep the paid lifecycle and prepare companions.",
                  verify=lambda p: None, sleep=terminal.sleep, monotonic=lambda: terminal.seconds)["answered"]
    assert terminal.keys == ["Enter"] and len(terminal.texts) == 1


def test_matching_draft_changed_before_submit_is_refused() -> None:
    class Changed(FormTerminal):
        reads = 0
        def read_screen(self, pane: str) -> str:
            self.reads += 1
            if self.reads == 2:
                self.screen = self.screen.replace("Exact answer", "Different draft")
            return super().read_screen(pane)
    terminal = Changed("async-freeform")
    terminal.screen = terminal.screen.replace("Type your answer", "Exact answer")
    with pytest.raises(AgentDeliveryError):
        answer(terminal, "w1:p1", adapter(), text="Exact answer", verify=lambda p: None)
    assert not terminal.keys and not terminal.texts
