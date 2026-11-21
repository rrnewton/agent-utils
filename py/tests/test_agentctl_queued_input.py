"""Queued UI inputs an exit would drop (#2045)."""
from __future__ import annotations

import pytest

from agentctl.errors import AgentDeliveryError
from agentctl.queued_input import check_queued_ui_input, queued_ui_entries

MUSE_SCREEN = "◆ Queued input\n└ stale recovery request, do not act on it\n\n❯\n"
CODEX_SCREEN = "Queued follow-up inputs\n  ↳ review the diff when free\n"
CLAUDE_SCREEN = "❯ Press up to edit queued messages\n"


def test_muse_queued_block_refuses_and_names_the_entry() -> None:
    assert queued_ui_entries("muse", MUSE_SCREEN) == [
        "stale recovery request, do not act on it"
    ]
    with pytest.raises(AgentDeliveryError, match="stale recovery request"):
        check_queued_ui_input("lane-a", "muse", MUSE_SCREEN, discard=False)


def test_discard_records_what_was_dropped() -> None:
    assert check_queued_ui_input("lane-a", "muse", MUSE_SCREEN, discard=True) == [
        "stale recovery request, do not act on it"
    ]


def test_codex_and_claude_queued_forms_are_recognised() -> None:
    assert queued_ui_entries("codex", CODEX_SCREEN) == ["review the diff when free"]
    assert queued_ui_entries("claude", CLAUDE_SCREEN) == [
        "1 queued message(s) whose text the harness does not show on screen"
    ]
    with pytest.raises(AgentDeliveryError):
        check_queued_ui_input("lane-a", "codex", CODEX_SCREEN, discard=False)


def test_no_block_and_unparsable_screens() -> None:
    assert check_queued_ui_input("lane-a", "muse", "❯\n", discard=False) == []
    with pytest.raises(AgentDeliveryError, match="cannot parse"):
        check_queued_ui_input(
            "lane-a", "muse", "◆ Queued input\n(no rows this read can parse)\n", discard=False
        )
    with pytest.raises(AgentDeliveryError, match="could not be read"):
        check_queued_ui_input("lane-a", "muse", "", discard=False)
