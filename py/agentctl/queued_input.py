"""The harness's own queued UI inputs, which an exit silently drops.

Muse draws them as a ``◆ Queued input`` block, Codex as queued
follow-up inputs, and Claude Code only as a ``Press up to edit queued
messages`` hint. They live in the TUI, not in agentctl's queue, and no
resume replays them (#2045 agentctl-muse-phantom-background-send), so
restart/stop read the pane for them right before exiting and refuse
while any are present; only an explicit discard proceeds, recorded by
the caller. A screen that cannot be read, or a Muse queued block this
read cannot parse, refuses too (fail closed).
"""
from __future__ import annotations

from agentctl.errors import AgentDeliveryError

_PREVIEW_CHARS = 100


def _preview(text: str) -> str:
    return text.strip()[:_PREVIEW_CHARS]


def queued_ui_entries(harness: str, screen: str) -> list[str]:
    """Each queued UI input's first line as the screen shows it."""
    lines = screen.splitlines()
    entries: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        if line.lstrip().startswith("◆ Queued input"):
            index += 1
            while index < len(lines):
                row = lines[index].strip()
                if not row.startswith("└"):
                    break
                entries.append(_preview(row[1:]))
                index += 1
            continue
        heading = line.strip().lstrip("• ").strip().lower()
        if heading in (
            "queued follow-up inputs",
            "messages to be submitted at end of turn",
            "messages to be submitted after next tool call",
        ):
            index += 1
            while index < len(lines):
                row = lines[index]
                row_stripped = row.strip()
                if (
                    not row_stripped
                    or "edit last queued message" in row_stripped
                    or not row.startswith(" ")
                ):
                    break
                if row_stripped.startswith("↳"):
                    entries.append(_preview(row_stripped[1:]))
                index += 1
            continue
        index += 1
    if entries:
        return entries
    if any("press up to edit queued messages" in line.lower() for line in lines):
        return ["1 queued message(s) whose text the harness does not show on screen"]
    return []


def check_queued_ui_input(
    name: str, harness: str, screen: str, discard: bool
) -> list[str]:
    """Refuse an exit that would drop queued UI inputs.

    Returns the entries discarded under an explicit `discard`; raises
    naming every entry otherwise, and on an unreadable or unparsable
    screen.
    """
    if not screen.strip():
        raise AgentDeliveryError(
            f"{name!r} pane screen could not be read before exit, so its queued "
            "UI input cannot be checked; nothing was exited"
        )
    if (
        harness == "muse"
        and "◆ Queued input" in screen
        and not queued_ui_entries(harness, screen)
    ):
        raise AgentDeliveryError(
            f"{name!r} shows a Muse queued-input block this read cannot parse; "
            f"refusing to exit and drop it unrecorded — inspect the pane with "
            f"`agentctl read {name}`"
        )
    entries = queued_ui_entries(harness, screen)
    if not entries:
        return []
    if discard:
        return entries
    named = "\n".join(f"  - {entry}" for entry in entries)
    raise AgentDeliveryError(
        f"{name!r} has {len(entries)} queued UI input(s) its harness has not "
        f"submitted, and exiting drops them without replay:\n{named}\n"
        "Let them submit, or have the human or coordinator confirm each is "
        "stale and rerun with --discard-queued-input (what was discarded is "
        "then recorded)"
    )
