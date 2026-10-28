"""The sender header that opens every prompt ``agentctl send`` delivers (#609 sender-header-mandatory).

A line typed into an agent's composer says nothing about who typed it: a status line from a lane,
a request from the coordinator and a stray paste all read alike. Every prompt ``agentctl send``
delivers therefore opens with one line naming its sender and route, whatever the path::

    [from cl-foo via agentctl · reply: agentctl send cl-foo]
    [from human@host-a via agentctl]

The grammar is ``[from SENDER via ROUTE( · KEY: VALUE)*]`` on a line of its own. SENDER holds no
white space, ``·`` or ``]``; ROUTE and each VALUE hold no ``·`` or ``]``. Fields after the route
are optional and a reader ignores keys it does not know; ``reply`` appears only when SENDER is an
agent that can be answered with ``agentctl send``. Other tools that hand agents messages may write
the same grammar with a route of their own.

The header is mandatory: no option suppresses it. Only the chat bridge's owner messages, which
carry their own opening and never pass through ``agentctl send``, go without. A one-line harness
slash command (see :func:`is_slash_command`) is a command for the harness rather than a message to
the agent, and a header would stop the harness reading it as one, so it is sent as written.

The header attributes messages between cooperating agents of one user; it is not authentication.
``AGENTCTL_AGENT`` in particular is taken on trust.
"""
from __future__ import annotations

import os
import re
import socket
import sys
from dataclasses import dataclass, field
from pathlib import Path

#: Route of an ordinary ``agentctl send``.
ROUTE = "agentctl"
#: Route of ``agentctl send --urgent``.
URGENT_ROUTE = "agentctl --urgent"
#: Separator between the header's fields.
SEPARATOR = " · "

#: Harness programs whose process, found among the caller's ancestors, names an unregistered
#: agent as the sender.
_HARNESSES = ("claude", "codex", "muse")
_AGENT_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}")
_RESERVED = ("archive", "agents", "locks")
_FIELD_KEY = re.compile(r"[a-z-]+")


@dataclass(frozen=True)
class Sender:
    """Who sent a prompt; ``agent`` when it can be answered by name with ``agentctl send``."""

    label: str
    agent: bool


def render(sender: Sender, route: str) -> str:
    """The header line, without a trailing newline."""
    if sender.agent:
        return f"[from {sender.label} via {route}{SEPARATOR}reply: agentctl send {sender.label}]"
    return f"[from {sender.label} via {route}]"


_COMMAND_NAME_TAIL = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_:-")


def is_slash_command(text: str) -> bool:
    """Whether ``text`` is a harness slash command, which is sent without a header.

    One line holding, after any spaces or tabs, ``/``, an ASCII letter, then ASCII letters,
    digits, ``_``, ``:`` or ``-``, and then nothing or a space and arguments. A path such as
    ``/tmp/build.log`` or a second line makes it a message, which gets the header.
    """
    line = text.lstrip(" \t")
    if "\n" in line or "\r" in line or not line.startswith("/"):
        return False
    name = line[1:].split(" ", 1)[0]
    return (bool(name) and name[0].isascii() and name[0].isalpha()
            and all(character in _COMMAND_NAME_TAIL for character in name))


def stamp(text: str, sender: Sender, route: str) -> str:
    """``text`` opened with the header; a slash command is returned as is.

    The header is always added, even when ``text`` already opens with one: a forwarded message
    keeps the header it came with below the forwarder's own, and a header written by hand into a
    body can never stand first.
    """
    if is_slash_command(text):
        return text
    return f"{render(sender, route)}\n{text}"


@dataclass(frozen=True)
class Header:
    """A parsed header line."""

    sender: str
    route: str
    fields: tuple[tuple[str, str], ...] = field(default_factory=tuple)

    def field(self, key: str) -> str | None:
        """The value of field ``key``, when present."""
        return next((value for name, value in self.fields if name == key), None)


def _whitespace(text: str) -> bool:
    return any(character.isspace() for character in text)


def parse(line: str) -> Header | None:
    """Parse one header line (the first line of a delivered prompt); ``None`` when it is not one."""
    if not line.startswith("[from ") or not line.endswith("]"):
        return None
    inner = line[len("[from "):-1]
    if "]" in inner or "\n" in inner:
        return None
    parts = inner.split(SEPARATOR)
    sender, via, route = parts[0].partition(" via ")
    if (not via or not sender or _whitespace(sender) or "·" in sender
            or not route.strip() or "·" in route):
        return None
    fields: list[tuple[str, str]] = []
    for part in parts[1:]:
        key, colon, value = part.partition(": ")
        if (not colon or not _FIELD_KEY.fullmatch(key) or not value.strip() or "·" in value):
            return None
        fields.append((key, value))
    return Header(sender, route, tuple(fields))


def has_header(text: str) -> bool:
    """Whether ``text`` opens with a header line."""
    return parse(text.split("\n", 1)[0].removesuffix("\r")) is not None


def resolve(*, registered: str | None, environment: str | None,
            harness: tuple[str, int] | None, interactive: bool, host: str) -> Sender:
    """The sender, from the first source that names one.

    The caller's registry identity, then ``AGENTCTL_AGENT`` when it is a valid agent name, then
    the nearest harness ancestor as ``HARNESS:PID@HOST``, then ``human@HOST`` when standard input
    is a terminal, else ``unknown@HOST``.
    """
    if registered is not None:
        return Sender(registered, True)
    if (environment is not None and _AGENT_NAME.fullmatch(environment)
            and environment not in _RESERVED):
        return Sender(environment, True)
    if harness is not None:
        return Sender(f"{harness[0]}:{harness[1]}@{host}", False)
    return Sender(f"human@{host}" if interactive else f"unknown@{host}", False)


def resolve_current(registered: str | None) -> Sender:
    """:func:`resolve` for this process, with ``registered`` already looked up in the registry."""
    try:
        interactive = sys.stdin is not None and os.isatty(sys.stdin.fileno())
    except (OSError, ValueError):
        interactive = False
    return resolve(registered=registered, environment=os.environ.get("AGENTCTL_AGENT"),
                   harness=harness_ancestor(Path("/proc"), os.getpid()),
                   interactive=interactive, host=short_host())


def short_host() -> str:
    """The host name up to its first dot, or ``localhost`` when it cannot be read."""
    try:
        name = socket.gethostname().split(".", 1)[0]
    except OSError:
        name = ""
    return name if name and not _whitespace(name) and "]" not in name else "localhost"


def _parent(proc_root: Path, pid: int) -> int | None:
    """The parent of ``pid``, from field 4 of ``/proc/PID/stat`` (after the parenthesised name)."""
    try:
        stat = (proc_root / str(pid) / "stat").read_text(encoding="utf-8", errors="replace")
        return int(stat.rsplit(")", 1)[1].split()[1])
    except (OSError, IndexError, ValueError):
        return None


def harness_ancestor(proc_root: Path, pid: int) -> tuple[str, int] | None:
    """The nearest proper ancestor of ``pid`` whose program is a coding-agent harness."""
    current = _parent(proc_root, pid)
    for _ in range(64):
        if current is None or current <= 1:
            return None
        try:
            comm = (proc_root / str(current) / "comm").read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None
        program = comm.removesuffix("\n")
        if program in _HARNESSES:
            return program, current
        current = _parent(proc_root, current)
    return None


def boot_id() -> str:
    """This boot's identifier, as process identities record it."""
    with open("/proc/sys/kernel/random/boot_id", encoding="ascii") as stream:
        return stream.read().strip()


def ancestry(proc_root: Path = Path("/proc"), pid: int | None = None) -> list[tuple[int, int]]:
    """``(pid, starttime_ticks)`` of ``pid`` (default: this process) and its ancestors, nearest first.

    Read twice; a chain that changed between the reads (a parent that exited and was reparented,
    or a reused identifier) is empty rather than one that never existed.
    """
    first = _walk(proc_root, os.getpid() if pid is None else pid)
    return first if first == _walk(proc_root, os.getpid() if pid is None else pid) else []


def _walk(proc_root: Path, pid: int) -> list[tuple[int, int]]:
    links: list[tuple[int, int]] = []
    current = pid
    while current and len(links) < 4096:
        try:
            stat = (proc_root / str(current) / "stat").read_text(encoding="utf-8", errors="replace")
            fields = stat.rsplit(")", 1)[1].split()
            parent, start = int(fields[1]), int(fields[19])
        except (OSError, IndexError, ValueError):
            # An unreadable start fails the walk; an unreadable ancestor ends the chain.
            if not links:
                return []
            break
        # A parent starts before its child; a later start is a reused identifier.
        if links and start > links[-1][1]:
            return []
        links.append((current, start))
        if parent == current:
            break
        current = parent
    return links
