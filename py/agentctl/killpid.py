"""Guarded single-PID signalling (#1420 guarded-kill-pid).

The 2026-11-11 outage began when an agent sent SIGTERM to a PID it took for a
stale script's parent; the PID was the host's ``systemd --user`` manager and
every user unit stopped. This module makes the safe kill mechanical: a PID may
be signalled only when its command line and start time still match what the
operator recorded (so a reused PID is refused), and never when it is the user
manager, a ``user@UID.service`` main process, the Herdr server, the main PID
of a systemd unit the caller's team does not own, or a process the caller's
team cannot prove it owns. Ownership is an allowlist: the process's working
directory must sit under the caller's team root (the directory holding the
registry, which covers its slots), or it must be the main PID of a unit the
team's own registry configuration names. Everything else is refused.

All system access goes through the :class:`ProcView`, :class:`UnitView` and
:class:`Killer` protocols, so tests replace ``/proc`` and ``systemctl`` with fixtures and never
signal a real process. The real killer re-reads the process identity and, on
Linux, signals through a pidfd pinned before the recheck, so the signal can
only reach the exact process that was checked.
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from agentctl.procstat import parse_process_stat

__all__ = [
    "KillRefused",
    "KillReport",
    "Killer",
    "OwnedContext",
    "ProcessInfo",
    "ProcView",
    "RealKiller",
    "RealProcView",
    "RealUnitView",
    "SIGNALS",
    "UnitInfo",
    "UnitView",
    "guarded_kill",
    "load_owned_units",
]

#: Signal names ``kill-pid`` accepts; anything else is a usage error.
SIGNALS: dict[str, int] = {
    "TERM": int(signal.SIGTERM),
    "KILL": int(signal.SIGKILL),
    "INT": int(signal.SIGINT),
    "HUP": int(signal.SIGHUP),
    "QUIT": int(signal.SIGQUIT),
    "USR1": int(signal.SIGUSR1),
    "USR2": int(signal.SIGUSR2),
    "CONT": int(signal.SIGCONT),
}

_MAX_CMDLINE_BYTES = 256 << 10
_MAX_UNITS = 4096
_LOG_NAME = "kill-pid.jsonl"


@dataclass(frozen=True)
class ProcessInfo:
    """One process as the guard needs to see it."""

    pid: int
    argv: tuple[str, ...]
    starttime: int
    cwd: str | None
    cgroup: str
    comm: str

    @property
    def cmdline(self) -> str:
        """The argument vector joined the way an operator would record it."""
        return " ".join(self.argv)


@dataclass(frozen=True)
class UnitInfo:
    """One systemd user unit and its main PID (0 when it has none)."""

    name: str
    main_pid: int


@dataclass(frozen=True)
class OwnedContext:
    """What the caller's team owns: its root directory and its unit names."""

    team_root: Path
    owned_units: frozenset[str]
    self_pid: int


@dataclass(frozen=True)
class KillReport:
    """A kill that was performed and logged."""

    pid: int
    signal_name: str
    cmdline: str
    starttime: int
    cwd: str | None
    log_path: Path


class KillRefused(Exception):
    """The guard refused to signal the PID; ``reason`` says why, mechanically."""

    def __init__(self, reason: str, detail: str) -> None:
        super().__init__(f"{reason}: {detail}")
        self.reason = reason
        self.detail = detail


class ProcView(Protocol):
    """Read access to one process's identity, replaced by test doubles."""

    def process(self, pid: int) -> ProcessInfo | None:
        """The process, or None when no such process can be read."""
        ...


class UnitView(Protocol):
    """Read access to the user manager's units, replaced by test doubles."""

    def units(self) -> tuple[UnitInfo, ...]:
        """Every user unit with its main PID."""
        ...


class Killer(Protocol):
    """The one signal send, replaced by test doubles so no real process is signalled."""

    def send(self, pid: int, signum: int, view: ProcView, expected: ProcessInfo) -> None:
        """Signal `pid`, re-verifying it still is `expected` first."""
        ...


class RealProcView:
    """The real view, read from a procfs root (``/proc`` unless a fixture root is given)."""

    def __init__(self, root: str = "/proc") -> None:
        self._root = root

    def process(self, pid: int) -> ProcessInfo | None:
        base = os.path.join(self._root, str(pid))
        try:
            with open(os.path.join(base, "stat"), "rb") as stream:
                stat_raw = stream.read(8193)
        except OSError:
            return None
        parsed = parse_process_stat(stat_raw)
        if parsed is None or parsed.pid != pid:
            return None
        try:
            with open(os.path.join(base, "cmdline"), "rb") as stream:
                raw = stream.read(_MAX_CMDLINE_BYTES + 1)
        except OSError:
            raw = b""
        argv: tuple[str, ...] = ()
        if 0 < len(raw) <= _MAX_CMDLINE_BYTES:
            argv = tuple(
                item.decode("utf-8", errors="replace")
                for item in raw.rstrip(b"\0").split(b"\0")
                if item
            )
        try:
            cwd: str | None = os.readlink(os.path.join(base, "cwd"))
        except OSError:
            cwd = None
        try:
            with open(os.path.join(base, "cgroup"), encoding="utf-8") as stream:
                cgroup = stream.read(65537)[:65536]
        except OSError:
            cgroup = ""
        comm = ""
        opening = stat_raw.find(b" (")
        closing = stat_raw.rfind(b")")
        if 0 <= opening < closing:
            comm = stat_raw[opening + 2 : closing].decode("utf-8", errors="replace")
        return ProcessInfo(
            pid=pid, argv=argv, starttime=parsed.starttime,
            cwd=cwd, cgroup=cgroup, comm=comm,
        )


class RealUnitView:
    """The real view: ``systemctl --user show`` for every loaded service."""

    def __init__(self, systemctl: str = "systemctl") -> None:
        self._systemctl = systemctl

    def units(self) -> tuple[UnitInfo, ...]:
        try:
            completed = subprocess.run(
                [self._systemctl, "--user", "show", "--all",
                 "--property=Id,MainPID", "--type=service"],
                capture_output=True, text=True, timeout=30, check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return ()
        if completed.returncode != 0:
            return ()
        units: list[UnitInfo] = []
        name = ""
        main_pid = 0
        for line in completed.stdout.splitlines() + [""]:
            if not line.strip():
                if name:
                    units.append(UnitInfo(name=name, main_pid=main_pid))
                    if len(units) > _MAX_UNITS:
                        return ()
                name, main_pid = "", 0
            elif line.startswith("Id="):
                name = line[3:]
            elif line.startswith("MainPID="):
                try:
                    main_pid = int(line[8:])
                except ValueError:
                    main_pid = 0
        return tuple(units)


class RealKiller:
    """Signal through a pidfd pinned before the identity recheck, when able."""

    def send(self, pid: int, signum: int, view: ProcView, expected: ProcessInfo) -> None:
        descriptor: int | None = None
        if hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"):
            try:
                descriptor = os.pidfd_open(pid, 0)
            except OSError:
                descriptor = None
        try:
            current = view.process(pid)
            if current is None or not _same_identity(current, expected):
                raise KillRefused(
                    "changed-before-signal",
                    f"pid {pid} changed between the check and the signal; not signalling",
                )
            if descriptor is not None:
                signal.pidfd_send_signal(descriptor, signum)
            else:
                os.kill(pid, signum)
        finally:
            if descriptor is not None:
                os.close(descriptor)


def _same_identity(left: ProcessInfo, right: ProcessInfo) -> bool:
    return (
        left.pid == right.pid
        and left.starttime == right.starttime
        and left.cmdline == right.cmdline
    )


def load_owned_units(registry: Path) -> frozenset[str]:
    """Unit names the team's own registry configuration declares as its own."""
    owned: set[str] = set()
    for filename in ("restore.json", "watch.json"):
        try:
            document: object = json.loads((registry / filename).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(document, dict):
            continue
        units: object = document.get("units")
        if isinstance(units, list):
            for item in units:
                if isinstance(item, str) and item:
                    owned.add(item)
        bridges: object = document.get("bridges")
        if isinstance(bridges, list):
            for item in bridges:
                if isinstance(item, str) and item:
                    owned.add(item)
    return frozenset(owned)


def _is_user_manager(process: ProcessInfo) -> bool:
    argv = process.argv
    if argv and os.path.basename(argv[0]) == "systemd" and "--user" in argv:
        return True
    return "systemd --user" in process.cmdline


def _is_herdr_server(process: ProcessInfo) -> bool:
    argv = process.argv
    if not argv:
        return process.comm == "herdr"
    return os.path.basename(argv[0]) == "herdr" and "server" in argv[1:]


def _other_team_root(cwd: str, team_root: Path) -> bool:
    """A working directory that names another team's registry or slots."""
    if cwd.startswith(str(team_root) + os.sep) or cwd == str(team_root):
        return False
    return "/.agentctl/" in cwd + "/" or "/worktrees/slots/" in cwd + "/"


def evaluate(
    process: ProcessInfo | None,
    *,
    pid: int,
    expect_cmdline: str,
    expect_start: int,
    units: tuple[UnitInfo, ...],
    context: OwnedContext,
) -> ProcessInfo:
    """Check one snapshot against every refusal; return it when it may be killed."""
    if process is None:
        raise KillRefused("no-such-process", f"no readable process with pid {pid}")
    if pid <= 1 or pid == context.self_pid:
        raise KillRefused("protected-pid", f"pid {pid} is never a kill-pid target")
    if expect_cmdline not in process.cmdline:
        raise KillRefused(
            "cmdline-mismatch",
            f"pid {pid} runs {process.cmdline!r}, which does not contain the expected {expect_cmdline!r}",
        )
    if process.starttime != expect_start:
        raise KillRefused(
            "starttime-mismatch",
            f"pid {pid} started at tick {process.starttime}, not {expect_start}; the PID was probably reused",
        )
    if _is_user_manager(process):
        raise KillRefused(
            "user-manager",
            f"pid {pid} is the systemd --user manager; signalling it stops every user unit",
        )
    for unit in units:
        if unit.main_pid == pid and unit.name.startswith("user@"):
            raise KillRefused(
                "user-service",
                f"pid {pid} is the main process of {unit.name}; signalling it stops every user unit",
            )
    if _is_herdr_server(process):
        raise KillRefused(
            "herdr-server",
            f"pid {pid} is the Herdr server; signalling it drops every agent's terminal",
        )
    for unit in units:
        if unit.main_pid == pid and unit.name not in context.owned_units:
            raise KillRefused(
                "foreign-unit-main-pid",
                f"pid {pid} is the main process of unit {unit.name}, which this team does not own",
            )
    cwd = process.cwd
    owned_unit = any(unit.main_pid == pid for unit in units)
    if cwd is not None and (cwd == str(context.team_root) or cwd.startswith(str(context.team_root) + os.sep)):
        return process
    if owned_unit:
        return process
    if cwd is not None and _other_team_root(cwd, context.team_root):
        raise KillRefused(
            "other-team",
            f"pid {pid} works in {cwd}, which belongs to another team's registry or slots",
        )
    raise KillRefused(
        "not-owned",
        f"pid {pid} works in {cwd!r}, outside this team's root {context.team_root}; "
        "kill-pid only signals processes the caller's team can prove it owns",
    )


def guarded_kill(
    *,
    pid: int,
    expect_cmdline: str,
    expect_start: int,
    signal_name: str,
    registry: Path,
    proc: ProcView,
    unit_view: UnitView,
    killer: Killer,
    self_pid: int | None = None,
    now: float | None = None,
) -> KillReport:
    """Guard, signal and log one PID, or raise :class:`KillRefused`."""
    if signal_name not in SIGNALS:
        raise KillRefused("bad-signal", f"unknown signal {signal_name!r}; use one of {sorted(SIGNALS)}")
    if not expect_cmdline:
        raise KillRefused("bad-expectation", "--expect-cmdline must not be empty")
    context = OwnedContext(
        team_root=registry.resolve().parent,
        owned_units=load_owned_units(registry),
        self_pid=os.getpid() if self_pid is None else self_pid,
    )
    units = unit_view.units()
    checked = evaluate(
        proc.process(pid), pid=pid, expect_cmdline=expect_cmdline,
        expect_start=expect_start, units=units, context=context,
    )
    killer.send(pid, SIGNALS[signal_name], proc, checked)
    stamp = time.time() if now is None else now
    record = {
        "schema": "agentctl-kill-pid/v1",
        "time": stamp,
        "pid": pid,
        "signal": signal_name,
        "cmdline": checked.cmdline,
        "starttime": checked.starttime,
        "cwd": checked.cwd,
        "actor": os.environ.get("AGENTCTL_AGENT") or "unknown",
    }
    log_path = registry / _LOG_NAME
    line = json.dumps(record, sort_keys=True) + "\n"
    descriptor = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(descriptor, line.encode("utf-8"))
    finally:
        os.close(descriptor)
    return KillReport(
        pid=pid, signal_name=signal_name, cmdline=checked.cmdline,
        starttime=checked.starttime, cwd=checked.cwd, log_path=log_path,
    )


def run_kill_pid(
    *,
    pid: int,
    expect_cmdline: str,
    expect_start: int,
    signal_name: str,
    registry: Path,
) -> KillReport:
    """The CLI entry: real /proc, real systemctl, real (pidfd) signal."""
    return guarded_kill(
        pid=pid, expect_cmdline=expect_cmdline, expect_start=expect_start,
        signal_name=signal_name, registry=registry,
        proc=RealProcView(), unit_view=RealUnitView(), killer=RealKiller(),
    )
