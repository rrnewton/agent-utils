"""The cadence / due-logic engine and the per-reminder fired-state store.

The due-logic is PURE and deterministic: given the reminder set, the last-fired epochs, and an
explicit ``now``, it decides which reminders should be CHECKED this tick. ``now`` is a parameter (not
``time.time()``) so tests can pin the clock at exact cadence boundaries.

The fired-state store is a tiny ``key=last_fired_epoch`` text file (one line per reminder), the same
crash-safe, human-readable format the tool generalizes. A missing key means "never fired" = due.
"""

from __future__ import annotations

import re
from collections.abc import Mapping, Sequence
from pathlib import Path

from tick_hub.model import EVERY_TICK, Reminder

_LINE_RE = re.compile(r"^([^=\s]+)=([0-9]+)$")


def is_due(name: str, cadence_secs: int, now: int, last_fired: Mapping[str, int]) -> bool:
    """True iff the named reminder should be checked this tick.

    A cadence of :data:`~tick_hub.model.EVERY_TICK` (0) is always due. Otherwise the reminder is due
    when at least ``cadence_secs`` have elapsed since it last fired; a reminder with no recorded
    last-fired epoch has never run and is due."""
    if cadence_secs <= EVERY_TICK:
        return True
    last = last_fired.get(name)
    if last is None:
        return True
    return (now - last) >= cadence_secs


def due_reminders(
    reminders: Sequence[Reminder], now: int, last_fired: Mapping[str, int]
) -> list[Reminder]:
    """The subset of ``reminders`` due this tick, in registration order (pure)."""
    return [r for r in reminders if is_due(r.name, r.cadence_secs, now, last_fired)]


def load_fired_state(path: Path) -> dict[str, int]:
    """Load the ``key=last_fired_epoch`` store. Missing file / unparsable lines yield an empty or
    partial map — a reminder absent from the map is simply treated as never-fired (= due)."""
    state: dict[str, int] = {}
    if not path.is_file():
        return state
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return state
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        m = _LINE_RE.match(line)
        if m:
            state[m.group(1)] = int(m.group(2))
    return state


def persist_fired_state(path: Path, state: Mapping[str, int]) -> None:
    """Atomically write the fired-state store (temp file + rename)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    lines = ["# tick-hub fired-state — key=last_fired_epoch (managed by tick-hub)"]
    for key in sorted(state):
        lines.append(f"{key}={state[key]}")
    tmp.write_text("\n".join(lines) + "\n", encoding="utf-8")
    tmp.replace(path)
