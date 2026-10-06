#!/usr/bin/env python3
"""Running check for ``wrkslots init`` that asks an agentctl registry whether an agent is alive.

wrkslots calls this as ``PATH AGENT`` during removal and audit. The exit status is the answer:
0 verified dead, 1 alive, 2 unverifiable. Exit 1 counts as alive only when the whole output is one
line naming ``agent=AGENT`` and ``rc=1``, so every outcome prints exactly one such line.

The registry is ``$AGENTCTL_REGISTRY`` when set, else ``$WRKSLOTS_PROJECT_ROOT/.agentctl``: the
registry a coordinator gets by running agentctl from the project root.

The check fails closed. It reports dead only when all of these hold:

- the agent has no active registry record, and at least one archived record, which agentctl
  writes only after ``agentctl stop`` retired the runtime it owned;
- no archived record's pinned process identity (PID, kernel start time, and boot ID) is alive;
- no readable process of this user still carries an archived record's Herdr pane in
  ``HERDR_PANE_ID``.

An active record whose pinned process or pane is alive is alive. Anything else, including an
active record with nothing visibly alive behind it, a name agentctl never recorded, or an
unreadable record, is unverifiable: agentctl has not confirmed that the agent stopped.

This is one input to removal, not the whole decision. wrkslots still requires its own census of
processes using the slot, its time-to-live, and its Git and path checks.
"""

from __future__ import annotations

import json
import os
import re
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import cast

NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,63}")
IDENTITY_KEYS = ("custom_process_identity", "foreign_shell_identity", "terminal_identity")


class Unverifiable(Exception):
    """Evidence needed for a verdict could not be read."""


def verdict(agent: str, code: int, reason: str) -> int:
    """Print the one result line wrkslots parses and return its exit status."""

    state = {0: "dead", 1: "alive", 2: "unverifiable"}[code]
    print(f"{state} agent={agent} rc={code} reason={reason}")
    return code


def load_record(path: Path) -> dict[str, object]:
    """Read one agentctl ``agent.json`` record."""

    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise Unverifiable(f"cannot read {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise Unverifiable(f"{path} is not a JSON object")
    return cast(dict[str, object], value)


def boot_id() -> str:
    """Return this boot's identifier."""

    try:
        return Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
    except OSError as exc:
        raise Unverifiable(f"cannot read boot id: {exc}") from exc


def start_ticks(pid: int) -> int | None:
    """Return a live process's kernel start time, or None when no such process exists."""

    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="ascii", errors="replace")
    except FileNotFoundError:
        return None
    except OSError as exc:
        raise Unverifiable(f"cannot read /proc/{pid}/stat: {exc}") from exc
    # The command name is parenthesised and may itself contain spaces or parentheses.
    fields = stat[stat.rindex(")") + 2 :].split()
    return int(fields[19])


def identity_alive(record: dict[str, object], current_boot: str) -> bool:
    """Say whether any process identity pinned in the record is still the same live process."""

    for key in IDENTITY_KEYS:
        identity = record.get(key)
        if not isinstance(identity, dict):
            continue
        pid = identity.get("pid")
        ticks = identity.get("starttime_ticks")
        recorded_boot = identity.get("boot_id")
        if not isinstance(pid, int) or not isinstance(ticks, int) or not isinstance(recorded_boot, str):
            raise Unverifiable(f"{key} lacks an integer pid, starttime_ticks, or a boot_id")
        if recorded_boot != current_boot:
            continue  # recorded before this boot; that process cannot still exist
        if start_ticks(pid) == ticks:
            return True
    return False


def processes_with_pane(pane_id: str) -> Iterator[int]:
    """Yield this user's readable processes whose environment names the Herdr pane."""

    needle = b"\0HERDR_PANE_ID=" + pane_id.encode() + b"\0"
    uid = os.getuid()
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if entry.stat().st_uid != uid:
                continue
            environ = (entry / "environ").read_bytes()
        except OSError:
            continue  # exited, or not readable; wrkslots' own slot census still applies
        if needle in b"\0" + environ + b"\0":
            yield int(entry.name)


def check(agent: str, registry: Path) -> int:
    """Return the wrkslots exit status for one agent name."""

    current_boot = boot_id()
    active = registry / agent / "agent.json"
    if active.exists():
        record = load_record(active)
        if identity_alive(record, current_boot):
            return verdict(agent, 1, "active-record-process-alive")
        pane = record.get("pane_id")
        if isinstance(pane, str) and any(True for _ in processes_with_pane(pane)):
            return verdict(agent, 1, "active-record-pane-alive")
        return verdict(agent, 2, "active-record-not-stopped")
    archived = sorted((registry / "archive").glob(f"{agent}-*/agent.json"))
    records = [load_record(path) for path in archived]
    records = [record for record in records if record.get("name") == agent]
    if not records:
        return verdict(agent, 2, "no-agentctl-record")
    for record in records:
        if identity_alive(record, current_boot):
            return verdict(agent, 1, "archived-record-process-alive")
        pane = record.get("pane_id")
        if isinstance(pane, str) and any(True for _ in processes_with_pane(pane)):
            return verdict(agent, 2, "archived-record-pane-still-occupied")
    return verdict(agent, 0, "stopped-by-agentctl")


def main(argv: list[str]) -> int:
    """Validate the arguments and print one verdict line."""

    if len(argv) != 2 or NAME.fullmatch(argv[1]) is None:
        print("usage: agentctl_liveness_probe.py AGENT  (rc=2)", file=sys.stderr)
        return 2
    agent = argv[1]
    override = os.environ.get("AGENTCTL_REGISTRY")
    root = os.environ.get("WRKSLOTS_PROJECT_ROOT")
    if override:
        registry = Path(override)
    elif root:
        registry = Path(root) / ".agentctl"
    else:
        return verdict(agent, 2, "no-registry-location")
    try:
        return check(agent, registry)
    except (Unverifiable, OSError, ValueError, IndexError) as exc:
        return verdict(agent, 2, "error:" + re.sub(r"\s+", "_", str(exc))[:200])


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
