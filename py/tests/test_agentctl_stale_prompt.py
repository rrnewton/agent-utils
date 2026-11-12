"""#1372 chat-stale-prompt-wedges-inbox: Python parity for the chat-age contract.

The Rust edition is primary; where the Python edition has the queue-status
path it reports the same oldest-pending-chat fields. The classification of
an owner (chat-bridge) prompt mirrors Rust `owner_prompt`.
"""

from __future__ import annotations

from agentctl.agent import _is_owner_prompt

CHAT_ID = "chat-" + "a" * 64


def test_chat_request_ids_are_owner_prompts() -> None:
    assert _is_owner_prompt(CHAT_ID, {"id": CHAT_ID, "text": "hi"})
    assert _is_owner_prompt("note-1", {"id": CHAT_ID, "text": "hi"})


def test_agent_notes_and_short_ids_are_not_owner_prompts() -> None:
    assert not _is_owner_prompt("note-1", {"id": "note-1", "text": "status"})
    assert not _is_owner_prompt("chat-short", {"id": "chat-short", "text": "hi"})
    assert not _is_owner_prompt(
        "chat-" + "A" * 64, {"id": "chat-" + "A" * 64, "text": "hi"})


def test_console_input_is_an_owner_prompt() -> None:
    document = {"id": "console", "text": "[from human@host-a via agentctl]\nhello"}
    assert _is_owner_prompt("console", document)
