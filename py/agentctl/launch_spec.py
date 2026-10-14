"""Replayable launch specifications kept in an agent record.

A launch spec is what ran: the harness, the program, the exact argument vector after
the program, the working directory, the private profile whose environment values the
launch used (names only are stored), the slot policy, the workspace label, and the
native conversation. ``start`` writes one before the harness exists; ``adopt`` and
``anchor`` read it from the pinned harness process's ``/proc/PID/cmdline``. ``revive``
replays it with the conversation selector replaced by an explicit resume, so a change
to a profile after launch does not change what a revived agent runs.

The spec lives in the record under ``launch_spec``. Both editions validate the same
strict shape and preserve the key through every lifecycle operation.
"""
from __future__ import annotations

import math
import os
import re
from collections.abc import Sequence
from typing import cast

from agentctl.errors import AgentDeliveryError

SCHEMA = "agentctl-launch-spec/v1"
FIELD = "launch_spec"
SOURCES = ("start", "adopt", "anchor", "revive")
#: Where the recorded conversation came from: chosen by agentctl, reported by Herdr,
#: read from the harness's own argument vector, or supplied by the caller.
NATIVE_SOURCES = ("asserted", "observed", "argv", "caller")
_KEYS = frozenset({
    "schema", "source", "captured_at", "harness", "executable", "argv", "cwd", "profile",
    "environment_names", "slot", "slot_project", "slot_isolation", "workspace_label",
    "native_session",
})
_KIND = re.compile(r"[a-z][a-z0-9-]{0,63}\Z")
_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}\Z")
_ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
#: Bounds that keep one record far below the record size limit.
MAX_ARGUMENTS = 512
MAX_ARGUMENT_BYTES = 64 << 10
MAX_TOTAL_ARGUMENT_BYTES = 256 << 10
#: Claude options that select, continue or fork a conversation.
_CLAUDE_VALUE_SELECTORS = ("--resume", "--session-id", "-r")
_CLAUDE_FLAG_SELECTORS = ("--continue", "-c", "--fork-session")


def _text(value: object) -> bool:
    return isinstance(value, str) and bool(value) and "\0" not in value


def _bounded_arguments(argv: object) -> bool:
    if not isinstance(argv, list) or len(argv) > MAX_ARGUMENTS:
        return False
    total = 0
    for item in argv:
        if not _text(item):
            return False
        try:
            size = len(cast(str, item).encode("utf-8"))
        except UnicodeEncodeError:
            return False
        if size > MAX_ARGUMENT_BYTES:
            return False
        total += size
    return total <= MAX_TOTAL_ARGUMENT_BYTES


def validate(value: object, *, harness: str, label: str) -> dict[str, object]:
    """Return the spec unchanged after proving its exact shape, or refuse."""
    def refuse(detail: str) -> AgentDeliveryError:
        return AgentDeliveryError(f"invalid launch spec ({detail}) in {label}")

    if not isinstance(value, dict) or set(value) != _KEYS:
        raise refuse("fields")
    spec = cast(dict[str, object], value)
    captured = spec["captured_at"]
    if spec["schema"] != SCHEMA or spec["source"] not in SOURCES:
        raise refuse("schema or source")
    if (isinstance(captured, bool) or not isinstance(captured, (int, float))
            or not math.isfinite(captured) or captured < 0):
        raise refuse("capture time")
    if spec["harness"] != harness or not isinstance(harness, str) or not _KIND.fullmatch(harness):
        raise refuse("harness")
    executable = spec["executable"]
    if executable is not None and not _text(executable):
        raise refuse("executable")
    if not _bounded_arguments(spec["argv"]):
        raise refuse("argv")
    cwd = spec["cwd"]
    if not _text(cwd) or not os.path.isabs(cast(str, cwd)):
        raise refuse("cwd")
    profile = spec["profile"]
    if profile is not None and (not isinstance(profile, str) or not _NAME.fullmatch(profile)):
        raise refuse("profile")
    names = spec["environment_names"]
    if (not isinstance(names, list)
            or any(not isinstance(name, str) or not _ENVIRONMENT_NAME.fullmatch(name) for name in names)
            or names != sorted(set(cast(list[str], names)))):
        raise refuse("environment names")
    slot, project, isolation = spec["slot"], spec["slot_project"], spec["slot_isolation"]
    if slot is not None and not _text(slot):
        raise refuse("slot")
    if project is not None and (not _text(project) or not os.path.isabs(cast(str, project))):
        raise refuse("slot project")
    if isolation not in (None, "userns", "cgroup", "root"):
        raise refuse("slot isolation")
    if slot is None and (project is not None or isolation is not None):
        raise refuse("slot policy without a slot")
    workspace = spec["workspace_label"]
    if workspace is not None and (not _text(workspace) or len(cast(str, workspace).encode("utf-8", "replace")) > 128):
        raise refuse("workspace label")
    native = spec["native_session"]
    if native is not None:
        if (not isinstance(native, dict) or set(native) != {"agent", "value", "source"}
                or native.get("agent") != harness or not _text(native.get("value"))
                or native.get("source") not in NATIVE_SOURCES):
            raise refuse("native session")
    return spec


def build(
    *, source: str, captured_at: float, harness: str, executable: str | None,
    argv: Sequence[str], cwd: str, profile: str | None = None,
    environment_names: Sequence[str] = (), slot: str | None = None,
    slot_project: str | None = None, slot_isolation: str | None = None,
    workspace_label: str | None = None, native_session: tuple[str, str] | None = None,
) -> dict[str, object]:
    """Build one validated spec; ``native_session`` is ``(value, source)``."""
    spec: dict[str, object] = {
        "schema": SCHEMA, "source": source, "captured_at": captured_at,
        "harness": harness, "executable": executable, "argv": list(argv), "cwd": cwd,
        "profile": profile, "environment_names": sorted(set(environment_names)),
        "slot": slot, "slot_project": slot_project, "slot_isolation": slot_isolation,
        "workspace_label": workspace_label,
        "native_session": None if native_session is None else {
            "agent": harness, "value": native_session[0], "source": native_session[1],
        },
    }
    return validate(spec, harness=harness, label="a new launch spec")


def _claude_selector(argument: str) -> tuple[str, str | None] | None:
    """``(option, inline value)`` for a Claude conversation option, else ``None``."""
    key, separator, inline = argument.partition("=")
    if argument.startswith("--") and key in _CLAUDE_VALUE_SELECTORS:
        return key, inline if separator else None
    if argument in _CLAUDE_FLAG_SELECTORS or (argument.startswith("--") and key in _CLAUDE_FLAG_SELECTORS):
        return key, None
    if argument == "-r":
        return "-r", None
    if argument.startswith("-r") and not argument.startswith("--"):
        return "-r", argument[2:]
    if argument.startswith("-c") and not argument.startswith("--"):
        raise AgentDeliveryError(
            f"recorded Claude argument {argument!r} combines a conversation flag and cannot be replayed"
        )
    return None


def _split(harness: str, argv: Sequence[str]) -> tuple[list[str], list[str]]:
    """Separate the conversation selectors from the rest, refusing anything ambiguous.

    Returns ``(selected conversation values, remaining arguments)``.
    """
    rest: list[str] = []
    values: list[str] = []
    items = list(argv)
    if harness == "claude":
        index = 0
        while index < len(items):
            selector = _claude_selector(items[index])
            if selector is None:
                rest.append(items[index])
                index += 1
                continue
            option, inline = selector
            if option in _CLAUDE_FLAG_SELECTORS:
                index += 1
                continue
            if inline is None:
                if index + 1 >= len(items) or items[index + 1].startswith("-"):
                    raise AgentDeliveryError(
                        f"recorded Claude {option} has no conversation value; it opens a picker and cannot be replayed"
                    )
                inline = items[index + 1]
                index += 2
            else:
                index += 1
            if not inline:
                raise AgentDeliveryError(f"recorded Claude {option} has an empty conversation value")
            values.append(inline)
        return values, rest
    if harness == "codex":
        if items[:1] == ["resume"]:
            if len(items) < 2 or items[1].startswith("-"):
                raise AgentDeliveryError(
                    "recorded Codex resume has no conversation id (for example --last) and cannot be replayed"
                )
            values.append(items[1])
            items = items[2:]
        if "resume" in items:
            raise AgentDeliveryError("recorded Codex arguments name resume in an unsupported position")
        return values, items
    if harness == "muse":
        if items[-2:-1] == ["resume"]:
            values.append(items[-1])
            items = items[:-2]
        if "resume" in items:
            raise AgentDeliveryError("recorded Muse arguments name resume in an unsupported position")
        return values, items
    raise AgentDeliveryError(f"launch specs replay Claude, Codex and Muse conversations, not {harness!r}")


def argv_conversation(harness: str, argv: Sequence[str]) -> str | None:
    """The one conversation the argument vector selects, or ``None``; refuse a conflict."""
    values, _rest = _split(harness, argv)
    if len(set(values)) > 1:
        raise AgentDeliveryError("recorded arguments select more than one conversation")
    return values[0] if values else None


def replay_arguments(harness: str, argv: Sequence[str], conversation: str) -> tuple[str, ...]:
    """The recorded arguments with every selector replaced by one explicit resume."""
    if not _text(conversation):
        raise AgentDeliveryError("a replay needs a nonempty native conversation id")
    _values, rest = _split(harness, argv)
    if harness == "claude":
        return ("--resume", conversation, *rest)
    if harness == "codex":
        return ("resume", conversation, *rest)
    return (*rest, "resume", conversation)


def program_matches(harness: str, executable: str | None) -> bool:
    """Is the recorded program the harness itself, so Herdr's kind launch reproduces it?"""
    if executable is None:
        return True
    name = os.path.basename(executable)
    return name.removesuffix(" (deleted)") == harness
