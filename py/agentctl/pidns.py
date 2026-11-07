"""Panes whose processes live outside this process's PID namespace.

Herdr reports a pane's shell and foreground processes by process ID. Those IDs
are numbers in the Herdr server's PID namespace. When this manager runs inside
a container and the server runs on the host, the same numbers name different,
unrelated processes here, so every kernel identity proof built from them would
pin the wrong program.

This module does two things:

* ``server_pid_visible`` decides whether the server is in a foreign namespace.
  The kernel answers through ``SO_PEERCRED``: a peer outside this namespace is
  reported with PID 0.

* ``translate`` maps one pane onto local processes when, and only when, the
  pane's single foreground process on the server side is ``podman exec`` or
  ``docker exec`` into *this* container, and the native session Herdr reports
  for the pane is held open by exactly one local harness process. The pane's
  local shell is that process's exec root (an ancestor whose parent is outside
  this namespace), and the local foreground group is the one that root's
  terminal reports. Anything else is refused, never approximated.
"""
from __future__ import annotations

import os
import re
import socket
import struct
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Protocol

__all__ = [
    "LocalProcess",
    "PidNamespaceView",
    "ProcfsTable",
    "Translation",
    "TranslationRefused",
    "container_exec_target",
    "local_container_id",
    "server_pid_visible",
    "translate",
]

_MAX_PROC_ENTRIES = 1 << 20
_MAX_FDS = 1 << 16
_MAX_ANCESTRY = 64
_MAX_COMMAND_LINE_BYTES = 256 << 10
_SESSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{7,127}\Z")
_KIND = re.compile(r"[a-z][a-z0-9_-]{0,31}\Z")
_CONTAINER_ID = re.compile(r"[0-9a-f]{64}\Z")
_MIN_ID_PREFIX = 12
# Mount roots that podman and docker bind over /etc/hostname for one container.
_HOSTNAME_SOURCE = re.compile(r"/(?:overlay-containers|containers)/([0-9a-f]{64})/(?:userdata/)?hostname\Z")
_ENGINES = frozenset(("podman", "docker"))
# ``exec`` options that consume the following argument when not written ``--opt=value``.
_EXEC_VALUE_OPTIONS = frozenset((
    "-e", "--env", "--env-file", "-u", "--user", "-w", "--workdir",
    "--detach-keys", "--preserve-fds", "--preserve-fd", "--cidfile",
))


class TranslationRefused(Exception):
    """The pane cannot be mapped onto local processes with certainty."""


@dataclass(frozen=True)
class LocalProcess:
    """One process as this namespace sees it."""

    pid: int
    ppid: int
    pgrp: int
    #: Foreground process group of the controlling terminal, or -1 without one.
    tpgid: int
    comm: str
    argv: tuple[str, ...]
    executable: str | None


@dataclass(frozen=True)
class Translation:
    """A pane's processes expressed in this namespace."""

    shell_pid: int
    foreground_pgid: int
    #: ``(pid, name, cmdline, argv0, reported_executable)``, as ``ProcessInfo`` stores it.
    foreground: tuple[tuple[int, str, str, str, str | None], ...]
    #: The process that holds the native session open.
    harness_pid: int
    harness_executable: str | None
    #: Working directory of the local foreground group leader.
    cwd: str | None


class ProcTable(Protocol):
    """Read access to this namespace's process table."""

    def processes(self) -> Mapping[int, LocalProcess] | None:
        """Every process in this namespace by PID, or None when unreadable."""

    def open_files(self, pid: int) -> tuple[str, ...] | None:
        """Paths `pid` holds open, or None when unreadable."""

    def cwd(self, pid: int) -> str | None:
        """Working directory of `pid`, or None when unreadable."""


class ProcfsTable:
    """The real table, read from ``/proc`` with bounded reads."""

    def __init__(self, root: str = "/proc") -> None:
        self._root = root

    def processes(self) -> Mapping[int, LocalProcess] | None:
        """Every process under the procfs root by PID, or None when unreadable."""
        try:
            entries = [entry for entry in os.listdir(self._root)
                       if entry.isascii() and entry.isdigit()]
        except OSError:
            return None
        if len(entries) > _MAX_PROC_ENTRIES:
            return None
        table: dict[int, LocalProcess] = {}
        for entry in entries:
            process = self._process(int(entry))
            if process is not None:
                table[process.pid] = process
        return table

    def _process(self, pid: int) -> LocalProcess | None:
        base = os.path.join(self._root, str(pid))
        try:
            with open(os.path.join(base, "stat"), "rb") as stream:
                raw = stream.read(8193)
        except OSError:
            return None
        parsed = parse_stat(raw)
        if parsed is None or parsed[0] != pid:
            return None
        try:
            with open(os.path.join(base, "cmdline"), "rb") as stream:
                command = stream.read(_MAX_COMMAND_LINE_BYTES + 1)
        except OSError:
            command = b""
        if len(command) > _MAX_COMMAND_LINE_BYTES:
            command = b""
        argv = tuple(item.decode("utf-8", errors="replace")
                     for item in command.rstrip(b"\0").split(b"\0") if item)
        try:
            executable: str | None = os.readlink(os.path.join(base, "exe"))
        except OSError:
            executable = None
        _pid, comm, ppid, pgrp, tpgid = parsed
        return LocalProcess(pid, ppid, pgrp, tpgid, comm, argv, executable)

    def open_files(self, pid: int) -> tuple[str, ...] | None:
        """Paths `pid` holds open under the procfs root, or None when unreadable."""
        directory = os.path.join(self._root, str(pid), "fd")
        try:
            names = os.listdir(directory)
        except OSError:
            return None
        if len(names) > _MAX_FDS:
            return None
        links: list[str] = []
        for name in names:
            try:
                links.append(os.readlink(os.path.join(directory, name)))
            except OSError:
                continue
        return tuple(links)

    def cwd(self, pid: int) -> str | None:
        """Working directory of `pid` under the procfs root, or None when unreadable."""
        try:
            return os.readlink(os.path.join(self._root, str(pid), "cwd"))
        except OSError:
            return None


def parse_stat(raw: bytes) -> tuple[int, str, int, int, int] | None:
    """``(pid, comm, ppid, pgrp, tpgid)`` from one ``/proc/PID/stat`` record."""
    if not raw or len(raw) > 8192:
        return None
    opening = raw.find(b" (")
    closing = raw.rfind(b")")
    if opening <= 0 or closing <= opening + 1 or raw[closing + 1:closing + 2] != b" ":
        return None
    fields = raw[closing + 2:].split()
    if len(fields) < 6:
        return None
    try:
        pid = int(raw[:opening])
        ppid, pgrp, tpgid = int(fields[1]), int(fields[2]), int(fields[5])
    except ValueError:
        return None
    if pid < 1 or ppid < 0 or pgrp < 0 or tpgid < -1:
        return None
    return pid, raw[opening + 2:closing].decode("utf-8", errors="replace"), ppid, pgrp, tpgid


def server_pid_visible(socket_path: str) -> bool | None:
    """Is the process serving ``socket_path`` visible in this PID namespace?

    ``None`` when the socket cannot be reached; callers keep their ordinary
    behavior then, since every Herdr call will fail on its own.
    """
    try:
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    except OSError:
        return None
    try:
        connection.settimeout(2.0)
        connection.connect(socket_path)
        raw = connection.getsockopt(
            socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"),
        )
    except OSError:
        return None
    finally:
        connection.close()
    pid, _uid, _gid = struct.unpack("3i", raw)
    return bool(int(pid) > 0)


def local_container_id(mountinfo: str) -> str | None:
    """This container's full ID, from the bind mount its engine placed on ``/etc/hostname``."""
    found: set[str] = set()
    for line in mountinfo.splitlines():
        fields = line.split()
        if len(fields) < 5 or fields[4] != "/etc/hostname":
            continue
        match = _HOSTNAME_SOURCE.search(fields[3])
        if match is not None:
            found.add(match.group(1))
    return found.pop() if len(found) == 1 else None


def container_exec_target(argv: Sequence[str]) -> str | None:
    """The container named by a ``podman exec`` or ``docker exec`` argument vector."""
    if len(argv) < 3 or os.path.basename(argv[0]) not in _ENGINES or argv[1] != "exec":
        return None
    index = 2
    while index < len(argv):
        token = argv[index]
        if token == "--":
            index += 1
            break
        if not token.startswith("-") or token == "-":
            break
        if token in _EXEC_VALUE_OPTIONS:
            index += 2
            continue
        index += 1
    return argv[index] if index < len(argv) else None


def names_container(reference: str, container_id: str) -> bool:
    """Does an engine argument name this container by full ID or unambiguous ID prefix?"""
    return (_CONTAINER_ID.fullmatch(container_id) is not None
            and _MIN_ID_PREFIX <= len(reference) <= 64
            and container_id.startswith(reference))


def _holds_session(path: str, session: str) -> bool:
    name = os.path.basename(path)
    target = f"{session}.jsonl"
    return name == target or name.endswith("-" + target)


def translate(
    *,
    reported_foreground: Sequence[Sequence[str]],
    session_agent: str | None,
    session_value: str | None,
    container_id: str | None,
    table: ProcTable,
) -> Translation:
    """Map one pane onto local processes, or raise ``TranslationRefused``.

    ``reported_foreground`` holds the argument vector of each foreground process
    Herdr reported for the pane, in the server's namespace.
    """
    if len(reported_foreground) != 1:
        raise TranslationRefused(
            "the pane's foreground is not exactly one container exec process"
        )
    reference = container_exec_target(reported_foreground[0])
    if reference is None:
        raise TranslationRefused("the pane's foreground process is not podman or docker exec")
    if container_id is None:
        raise TranslationRefused("this container's ID cannot be determined")
    if not names_container(reference, container_id):
        raise TranslationRefused(f"the pane executes into another container ({reference!r})")
    if (session_agent is None or session_value is None
            or _KIND.fullmatch(session_agent) is None
            or _SESSION.fullmatch(session_value) is None):
        raise TranslationRefused("Herdr reports no usable native session for the pane")
    processes = table.processes()
    if processes is None:
        raise TranslationRefused("the local process table cannot be read")
    holders: list[LocalProcess] = []
    for process in processes.values():
        names = {os.path.basename(process.executable or "").removesuffix(" (deleted)"),
                 os.path.basename(process.argv[0]) if process.argv else "",
                 process.comm}
        if session_agent not in names:
            continue
        files = table.open_files(process.pid)
        if files is None:
            continue
        if any(_holds_session(path, session_value) for path in files):
            holders.append(process)
    if len(holders) != 1:
        raise TranslationRefused(
            f"expected one local {session_agent} process holding session "
            f"{session_value}, found {len(holders)}"
        )
    harness = holders[0]
    root = harness
    for _depth in range(_MAX_ANCESTRY):
        if root.ppid == 0:
            break
        parent = processes.get(root.ppid)
        if parent is None:
            raise TranslationRefused("the harness's process ancestry is incomplete")
        root = parent
    else:
        raise TranslationRefused("the harness's process ancestry is too deep")
    if root.pid == 1:
        raise TranslationRefused(
            "the harness descends from the container's init process, not an exec session"
        )
    if root.tpgid < 1:
        raise TranslationRefused("the exec session has no foreground process group")
    group = sorted((process for process in processes.values() if process.pgrp == root.tpgid),
                   key=lambda process: process.pid)
    if not group:
        raise TranslationRefused("the exec session's foreground group has no process")
    for member in group:
        ancestor = member
        for _depth in range(_MAX_ANCESTRY):
            if ancestor.pid == root.pid:
                break
            parent = processes.get(ancestor.ppid)
            if parent is None:
                raise TranslationRefused("a foreground process is outside the exec session")
            ancestor = parent
        else:
            raise TranslationRefused("a foreground process is outside the exec session")
    foreground = tuple(
        (member.pid, member.comm, " ".join(member.argv),
         member.argv[0] if member.argv else "", None)
        for member in group
    )
    leader = processes.get(root.tpgid)
    cwd = table.cwd(leader.pid if leader is not None else root.pid)
    return Translation(
        shell_pid=root.pid, foreground_pgid=root.tpgid, foreground=foreground,
        harness_pid=harness.pid, harness_executable=harness.executable, cwd=cwd,
    )


class PidNamespaceView:
    """Whether Herdr's process IDs are local, and how to translate them when not."""

    def __init__(
        self, *, foreign: bool, table: ProcTable | None = None,
        container_id: str | None = None,
    ) -> None:
        self.foreign = foreign
        self.table: ProcTable = ProcfsTable() if table is None else table
        self.container_id = container_id

    @classmethod
    def probe(cls, socket_path: str) -> "PidNamespaceView":
        """Classify the Herdr server at `socket_path` as local or foreign to this PID namespace."""
        visible = server_pid_visible(socket_path)
        if visible is None or visible:
            return cls(foreign=False)
        try:
            with open("/proc/self/mountinfo", encoding="utf-8", errors="replace") as stream:
                mountinfo = stream.read(4 << 20)
        except OSError:
            mountinfo = ""
        return cls(foreign=True, container_id=local_container_id(mountinfo))
