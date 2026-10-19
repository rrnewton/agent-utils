"""Generation-safe ownership of headless runner and harness processes.

A process number names a process only until that process is reaped; after that
the kernel may hand the number to anything. Every identity this runtime records
is therefore the triple ``(boot ID, pid, start ticks)``, and every signal goes
through one of two kernel-bound handles:

* a **pidfd**, opened first and only then checked against the recorded start
  ticks and boot, then polled to prove the process was still the same one
  during the check. ``pidfd_send_signal`` on it can never reach a later
  process that reused the number;
* a **per-turn cgroup** whose directory name carries 128 random bits, killed
  with ``cgroup.kill``.

The one numeric signal left is the parent-pinned fallback for hosts without
pidfds: inside the runner, its own unreaped children (the harness leader and
orphans adopted by the child subreaper) are signalled by number while the
runner holds its reap lock. A child that has not been reaped keeps its number.

Harness trees are found, not assumed. A process belongs to the tree that one
turn started when, after a pidfd pins it, one of these is true:

* its initial environment carries this turn's ``AGENTCTL_HARNESS_GENERATION``;
* it is in the leader's session, and the leader still holds that number;
* its parent is the runner, which is a child subreaper, and the runner is
  still the recorded process.

Everything outside those proofs is reported, never signalled.
"""

from __future__ import annotations

import contextlib
import dataclasses
import errno
import os
import re
import secrets
import select
import shutil
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from pathlib import Path
from typing import IO, Optional, Union

from agentctl.procstat import ProcessStat, parse_process_stat

__all__ = [
    "AgentOperationError",
    "CleanupReport",
    "GENERATION_ENV",
    "HarnessRecord",
    "OwnedHarnessProcess",
    "Pin",
    "PinnedProcess",
    "ProcessIdentity",
    "current_boot_id",
    "kill_harness_tree",
    "parse_harness_record",
    "pin",
    "spawn_owned_harness",
    "stop_process",
]

PROC: Path = Path("/proc")
GENERATION_ENV: str = "AGENTCTL_HARNESS_GENERATION"
CGROUP_MODE_ENV: str = "AGENTCTL_HEADLESS_CGROUP"
HARNESS_SCHEMA: str = "agentctl-headless-harness/v1"
RUNNER_SCHEMA: str = "agentctl-headless-runner/v1"
CGROUP_PREFIX: str = "agentctl-harness-"
CONTAINMENT_PROCESS: str = "process"
CONTAINMENT_CGROUP: str = "cgroup"
CONTAINMENT_LEGACY: str = "legacy"
# How long a cleanup waits for SIGKILLed processes to disappear. A process in
# uninterruptible sleep outlives this bound and is reported, not forgotten.
KILL_WAIT_S: float = float(os.environ.get("SUBAGENTS_HARNESS_KILL_WAIT", "10"))
GATE_READY_TIMEOUT_S: float = 10.0

_BOOT_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
_GENERATION = re.compile(r"[0-9a-f]{32}")
_TICKS = re.compile(r"(?:0|[1-9][0-9]{0,19})")
_DEAD_STATES = frozenset(("Z", "X", "x"))
_MAX_PROCESS_ID = (1 << 31) - 1
_MAX_ENVIRON_BYTES = 16 * 1024 * 1024
_PR_SET_CHILD_SUBREAPER = 36
_GATE = Path(__file__).resolve().parent.parent / "_launch_gate.py"


class AgentOperationError(Exception):
    """Structured error for callers that need JSON-friendly failures."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


@dataclasses.dataclass(frozen=True)
class ProcessIdentity:
    """A process number bound to its kernel start ticks and, when known, its boot.

    ``boot_id`` is ``None`` only for records written before boot identities
    were persisted (or rewritten by an older writer); see the legacy policy in
    :func:`pin`.
    """

    pid: int
    started_at: Optional[str]
    boot_id: Optional[str] = None


# --------------------------------------------------------------------------- #
# Kernel evidence
# --------------------------------------------------------------------------- #


def current_boot_id() -> str:
    """Return the canonical Linux boot UUID, refusing unreadable evidence."""
    path = PROC / "sys" / "kernel" / "random" / "boot_id"
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise AgentOperationError(
            "runner_identity_unknown", f"cannot read the Linux boot identity: {exc}"
        ) from exc
    value = raw.decode("ascii", errors="replace").strip()
    if not raw or len(raw) > 128 or _BOOT_ID.fullmatch(value) is None:
        raise AgentOperationError("runner_identity_unknown", "the Linux boot identity is malformed")
    return value


def valid_boot_id(value: object) -> bool:
    """True for a canonical lower-case boot UUID."""
    return isinstance(value, str) and _BOOT_ID.fullmatch(value) is not None


def read_stat(pid: int) -> Optional[ProcessStat]:
    """Return one process's identity fields, or ``None`` only for positive disappearance.

    Unreadable or malformed procfs evidence is not a death certificate and must
    not authorize later process signalling.
    """
    try:
        raw = (PROC / str(pid) / "stat").read_bytes()
    except (FileNotFoundError, ProcessLookupError):
        return None
    except OSError as exc:
        raise AgentOperationError(
            "runner_identity_unknown", f"cannot read runner process {pid} identity: {exc}"
        ) from exc
    parsed = parse_process_stat(raw)
    if parsed is None or parsed.pid != pid:
        raise AgentOperationError("runner_identity_unknown", f"cannot parse runner process {pid} identity")
    return parsed


def _quiet_stat(pid: int) -> Optional[ProcessStat]:
    try:
        return read_stat(pid)
    except AgentOperationError:
        return None


def _scan_processes() -> Iterator[ProcessStat]:
    """Yield every process procfs shows; vanished or unreadable ones are skipped."""
    try:
        entries = os.listdir(PROC)
    except OSError as exc:
        raise AgentOperationError("runner_identity_unknown", f"cannot list {PROC}: {exc}") from exc
    for entry in entries:
        if not entry.isdigit():
            continue
        parsed = _quiet_stat(int(entry))
        if parsed is not None:
            yield parsed


def _same_user(pid: int) -> bool:
    try:
        return os.stat(PROC / str(pid)).st_uid == os.getuid()
    except OSError:
        return False


def carries_generation(pid: int, generation: str) -> bool:
    """True when the process's initial environment carries this turn's generation."""
    needle = f"{GENERATION_ENV}={generation}".encode("ascii")
    try:
        with open(PROC / str(pid) / "environ", "rb") as stream:
            data = stream.read(_MAX_ENVIRON_BYTES)
    except OSError:
        return False
    return needle in data.split(b"\0")


def pidfds_supported() -> bool:
    """True when this interpreter and kernel provide pidfd_open and pidfd_send_signal."""
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        return False
    try:
        descriptor = os.pidfd_open(os.getpid())
    except OSError:
        return False
    os.close(descriptor)
    return True


def _open_pidfd(pid: int) -> Optional[int]:
    """Open a pidfd; ``None`` when the number names no process."""
    for _attempt in range(4):
        try:
            return os.pidfd_open(pid)
        except InterruptedError:
            continue
        except ProcessLookupError:
            return None
    raise InterruptedError("pidfd_open was interrupted repeatedly")


def _fd_readable(descriptor: int, timeout: Optional[float]) -> bool:
    """Poll one descriptor for readability; ``None`` waits without bound."""
    poller = select.poll()
    poller.register(descriptor, select.POLLIN)
    deadline = None if timeout is None else time.monotonic() + max(0.0, timeout)
    while True:
        milliseconds = -1 if deadline is None else int(max(0.0, deadline - time.monotonic()) * 1000)
        try:
            events = poller.poll(milliseconds)
        except InterruptedError:
            continue
        if events:
            return True
        if deadline is not None and time.monotonic() >= deadline:
            return False


# --------------------------------------------------------------------------- #
# Process handles
# --------------------------------------------------------------------------- #


class PinnedProcess:
    """One exact process generation held by a pidfd."""

    def __init__(self, pid: int, descriptor: int, starttime: int) -> None:
        self.pid = pid
        self.starttime = starttime
        self._descriptor: Optional[int] = descriptor

    @property
    def descriptor(self) -> int:
        """The open pidfd; raises once closed."""
        if self._descriptor is None:
            raise ValueError("pidfd is closed")
        return self._descriptor

    def exited(self) -> bool:
        """True once the process has exited (it may still await its parent's reap)."""
        return _fd_readable(self.descriptor, 0)

    def wait(self, timeout: Optional[float]) -> bool:
        """Wait up to ``timeout`` seconds (``None``: without bound) for exit; True when it exited."""
        return _fd_readable(self.descriptor, timeout)

    def send(self, signum: int) -> bool:
        """Signal exactly this process; False when it has exited or refuses the signal.

        A refusal (a member that changed credentials, for example) must not
        abort a cleanup halfway; the member is then reported as remaining.
        """
        try:
            signal.pidfd_send_signal(self.descriptor, signum)
        except (ProcessLookupError, PermissionError):
            return False
        return True

    def close(self) -> None:
        """Release the pidfd."""
        if self._descriptor is not None:
            os.close(self._descriptor)
            self._descriptor = None

    def __enter__(self) -> "PinnedProcess":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()


class ChildHandle:
    """An unreaped direct child of this process, for hosts without pidfds.

    Only this process can reap its child, so while ``reaped`` reports false the
    number still names the same process and a numeric signal is exact.
    """

    def __init__(self, pid: int, starttime: int, reaped: Callable[[], bool]) -> None:
        self.pid = pid
        self.starttime = starttime
        self._reaped = reaped

    def exited(self) -> bool:
        """True once the child has exited; never reaps it."""
        if self._reaped():
            return True
        try:
            status = os.waitid(os.P_PID, self.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        except ChildProcessError:
            return True
        return status is not None

    def wait(self, timeout: Optional[float]) -> bool:
        """Poll for exit up to ``timeout`` seconds (``None``: without bound) without reaping."""
        deadline = None if timeout is None else time.monotonic() + max(0.0, timeout)
        while not self.exited():
            if deadline is not None and time.monotonic() >= deadline:
                return False
            time.sleep(0.02)
        return True

    def send(self, signum: int) -> bool:
        """Signal the child while it is provably unreaped."""
        if self.exited():
            return False
        try:
            os.kill(self.pid, signum)  # parent-pinned: an unreaped child keeps its number
        except ProcessLookupError:
            return False
        return True

    def close(self) -> None:
        """Nothing to release; present for symmetry with :class:`PinnedProcess`."""


ProcessHandle = Union[PinnedProcess, ChildHandle]

ALIVE = "alive"
GONE = "gone"
UNPROVEN = "unproven"


@dataclasses.dataclass
class Pin:
    """Outcome of pinning one recorded identity."""

    verdict: str
    reason: str
    process: Optional[PinnedProcess] = None


def pin(
    identity: ProcessIdentity,
    *,
    legacy_proof: Optional[Callable[[int, ProcessStat], bool]] = None,
) -> Pin:
    """Pin a recorded process, or prove it gone, or say why neither is possible.

    ``GONE`` is a positive death certificate: a different boot, no such
    process, a newer process under the number, or an exited one. ``UNPROVEN``
    never authorizes a signal. A record without a boot identity is legacy: it
    is pinned only when ``legacy_proof`` accepts the process.
    """
    pid = identity.pid
    if not 2 <= pid <= _MAX_PROCESS_ID:
        return Pin(UNPROVEN, f"recorded pid {pid} is not a valid process number")
    boot = current_boot_id()
    if identity.boot_id is not None:
        if not valid_boot_id(identity.boot_id):
            return Pin(UNPROVEN, f"pid {pid} has a malformed recorded boot identity")
        if identity.boot_id != boot:
            if current_boot_id() == boot:
                return Pin(GONE, f"pid {pid} was recorded during an earlier boot")
            return Pin(UNPROVEN, "the boot identity changed while it was read")
    if identity.started_at is not None and _TICKS.fullmatch(identity.started_at) is None:
        return Pin(UNPROVEN, f"pid {pid} has an unreadable recorded start time {identity.started_at!r}")
    if not pidfds_supported():
        current = read_stat(pid)
        if current is None or (
            identity.started_at is not None and str(current.starttime) != identity.started_at
        ):
            return Pin(GONE, f"pid {pid} no longer names the recorded process")
        return Pin(UNPROVEN, "this host does not provide pidfds, so the process cannot be pinned")
    descriptor = _open_pidfd(pid)
    if descriptor is None:
        if current_boot_id() == boot:
            return Pin(GONE, f"pid {pid} has exited")
        return Pin(UNPROVEN, "the boot identity changed while it was read")
    keep = False
    try:
        current = read_stat(pid)
        if current is None:
            if _fd_readable(descriptor, 0):
                return Pin(GONE, f"pid {pid} has exited")
            return Pin(UNPROVEN, f"pid {pid} vanished from procfs while its pidfd was open")
        if identity.started_at is None:
            return Pin(UNPROVEN, f"pid {pid} has no recorded start time, so its generation is unknown")
        if str(current.starttime) != identity.started_at:
            return Pin(GONE, f"pid {pid} now names a newer process")
        if current.state in _DEAD_STATES:
            return Pin(GONE, f"pid {pid} has exited")
        if identity.boot_id is None and (legacy_proof is None or not legacy_proof(pid, current)):
            return Pin(UNPROVEN, f"pid {pid} has a legacy record without a boot identity and is not proven")
        if _fd_readable(descriptor, 0):
            return Pin(GONE, f"pid {pid} has exited")
        keep = True
        return Pin(ALIVE, f"pid {pid} is the recorded process", PinnedProcess(pid, descriptor, current.starttime))
    finally:
        if not keep:
            os.close(descriptor)


def stop_process(
    identity: ProcessIdentity,
    *,
    grace: float,
    legacy_proof: Optional[Callable[[int, ProcessStat], bool]] = None,
    kill_wait: Optional[float] = None,
    label: str = "runner",
) -> bool:
    """SIGTERM, then after ``grace`` SIGKILL, one pinned process.

    Returns False when the process was already gone. An unprovable identity
    raises ``runner_identity_unproven`` (or ``pidfd_unsupported``) without a
    signal, and a process that outlives SIGKILL by ``kill_wait`` raises
    ``runner_stop_incomplete``.
    """
    result = pin(identity, legacy_proof=legacy_proof)
    if result.verdict == GONE:
        return False
    if result.process is None:
        code = "pidfd_unsupported" if not pidfds_supported() else "runner_identity_unproven"
        raise AgentOperationError(code, f"refusing to signal {label}: {result.reason}")
    with result.process as process:
        process.send(signal.SIGTERM)
        if process.wait(grace):
            return True
        process.send(signal.SIGKILL)
        bound = KILL_WAIT_S if kill_wait is None else kill_wait
        if process.wait(bound):
            return True
        state = _quiet_stat(process.pid)
        raise AgentOperationError(
            "runner_stop_incomplete",
            f"{label} pid {process.pid} is still present {bound:g}s after SIGKILL"
            + ("" if state is None else f" (state {state.state})"),
        )


# --------------------------------------------------------------------------- #
# Harness ownership records
# --------------------------------------------------------------------------- #


@dataclasses.dataclass(frozen=True)
class HarnessRecord:
    """The durable ownership of one turn's harness tree."""

    runner: ProcessIdentity
    leader: ProcessIdentity
    process_group: Optional[int]
    generation: Optional[str]
    containment: str
    cgroup: Optional[Path] = None

    @property
    def legacy(self) -> bool:
        """True for a record written before boot identities and generations existed."""
        return self.containment == CONTAINMENT_LEGACY

    def document(self, recorded_at: str) -> dict[str, object]:
        """Serialize, keeping the keys older readers understand."""
        value: dict[str, object] = {
            "schema": HARNESS_SCHEMA,
            "runner_pid": self.runner.pid,
            "runner_started_at": self.runner.started_at,
            "runner_boot_id": self.runner.boot_id,
            "pid": self.leader.pid,
            "started_at": self.leader.started_at,
            "boot_id": self.leader.boot_id,
            "process_group": self.process_group,
            "generation": self.generation,
            "containment": self.containment,
            "recorded_at": recorded_at,
        }
        if self.cgroup is not None:
            value["cgroup"] = str(self.cgroup)
        return value


def _process_number(value: object) -> Optional[int]:
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= _MAX_PROCESS_ID:
        return None
    return value


def parse_harness_record(value: object) -> HarnessRecord:
    """Parse ``active-harness.json``; malformed ownership raises ``harness_identity_unknown``."""

    def bad(detail: str) -> AgentOperationError:
        return AgentOperationError("harness_identity_unknown", f"malformed active harness identity: {detail}")

    if not isinstance(value, Mapping):
        raise bad("not an object")
    pid = _process_number(value.get("pid"))
    started_at = value.get("started_at")
    if pid is None or pid <= 1 or not isinstance(started_at, str):
        raise bad("no valid harness pid and start time")
    runner_pid = _process_number(value.get("runner_pid"))
    runner_started_at = value.get("runner_started_at")
    if runner_pid is None or not (runner_started_at is None or isinstance(runner_started_at, str)):
        raise bad("no valid runner pid")
    if "schema" not in value:
        return HarnessRecord(
            runner=ProcessIdentity(runner_pid, runner_started_at),
            leader=ProcessIdentity(pid, started_at),
            process_group=pid,
            generation=None,
            containment=CONTAINMENT_LEGACY,
        )
    if value.get("schema") != HARNESS_SCHEMA:
        raise bad(f"unsupported schema {value.get('schema')!r}")
    boot_id, runner_boot_id = value.get("boot_id"), value.get("runner_boot_id")
    generation = value.get("generation")
    group = _process_number(value.get("process_group"))
    containment = value.get("containment")
    if not valid_boot_id(boot_id) or not (runner_boot_id is None or valid_boot_id(runner_boot_id)):
        raise bad("no valid boot identity")
    if not isinstance(generation, str) or _GENERATION.fullmatch(generation) is None:
        raise bad("no valid generation")
    if group is None or containment not in (CONTAINMENT_PROCESS, CONTAINMENT_CGROUP):
        raise bad("no valid process group or containment")
    cgroup: Optional[Path] = None
    raw_cgroup = value.get("cgroup")
    if containment == CONTAINMENT_CGROUP:
        if not isinstance(raw_cgroup, str):
            raise bad("cgroup containment without a cgroup path")
        cgroup = Path(raw_cgroup)
        if (not cgroup.is_absolute() or ".." in cgroup.parts
                or cgroup.name != f"{CGROUP_PREFIX}{generation}"):
            raise bad("cgroup path does not name this generation's directory")
    elif raw_cgroup is not None:
        raise bad("process containment with a cgroup path")
    return HarnessRecord(
        runner=ProcessIdentity(
            runner_pid, runner_started_at, runner_boot_id if isinstance(runner_boot_id, str) else None
        ),
        leader=ProcessIdentity(pid, started_at, str(boot_id)),
        process_group=group,
        generation=generation,
        containment=str(containment),
        cgroup=cgroup,
    )


# --------------------------------------------------------------------------- #
# cgroup v2 per-turn containment
# --------------------------------------------------------------------------- #


def cgroup_mode() -> str:
    """The configured per-turn cgroup mode: ``off`` (default), ``auto`` or ``require``."""
    value = os.environ.get(CGROUP_MODE_ENV, "off")
    if value not in ("off", "auto", "require"):
        raise AgentOperationError(
            "invalid_cgroup_mode", f"{CGROUP_MODE_ENV} must be off, auto or require, not {value!r}"
        )
    return value


def _cgroup2_mount() -> Optional[Path]:
    try:
        lines = (PROC / "self" / "mountinfo").read_text().splitlines()
    except OSError:
        return None
    for line in lines:
        left, separator, right = line.partition(" - ")
        fields = left.split()
        if separator and len(fields) >= 5 and right.split()[:1] == ["cgroup2"]:
            return Path(fields[4])
    return None


def _cgroup_of(pid: Union[int, str]) -> Optional[str]:
    try:
        lines = (PROC / str(pid) / "cgroup").read_text().splitlines()
    except OSError:
        return None
    for line in lines:
        if line.startswith("0::"):
            return line[3:]
    return None


def own_cgroup_directory() -> Optional[Path]:
    """This process's cgroup v2 directory, when the unified hierarchy is mounted."""
    mount, relative = _cgroup2_mount(), _cgroup_of("self")
    if mount is None or relative is None or not relative.startswith("/"):
        return None
    return mount / relative.lstrip("/")


def _cgroup_relative(path: Path) -> Optional[str]:
    """The ``/proc/PID/cgroup`` spelling of a directory inside the cgroup v2 mount."""
    mount = _cgroup2_mount()
    if mount is None:
        return None
    try:
        return "/" + str(path.relative_to(mount))
    except ValueError:
        return None


def _cgroup_events(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in (path / "cgroup.events").read_text().splitlines():
        key, _, value = line.partition(" ")
        values[key] = value.strip()
    return values


def _cgroup_members(path: Path) -> list[int]:
    try:
        text = (path / "cgroup.procs").read_text()
    except FileNotFoundError:
        return []
    return [int(item) for item in text.split() if item.isdigit()]


def _remove_cgroup(path: Path) -> None:
    for _attempt in range(50):
        try:
            os.rmdir(path)
            return
        except FileNotFoundError:
            return
        except OSError as exc:
            if exc.errno != errno.EBUSY:
                return
            time.sleep(0.02)


def _kill_cgroup(path: Path, deadline: float) -> tuple[int, list[str]]:
    """Kill every process in one owned cgroup and wait for it to empty."""
    if not path.is_dir():
        return 0, []
    members = [pid for pid in _cgroup_members(path)]
    try:
        populated = _cgroup_events(path).get("populated") == "1"
    except OSError:
        populated = bool(members)
    if populated:
        kill_file = path / "cgroup.kill"
        if kill_file.exists():
            kill_file.write_text("1\n")
        else:
            # Kernels before 5.14 lack cgroup.kill. Membership in this
            # generation's directory is the ownership proof; pin each member
            # and re-check that proof after the pidfd is open.
            expected = _cgroup_relative(path)
            for pid in members:
                stat = _quiet_stat(pid)
                if stat is None or not pidfds_supported():
                    continue
                descriptor = _open_pidfd(pid)
                if descriptor is None:
                    continue
                with PinnedProcess(pid, descriptor, stat.starttime) as process:
                    after = _quiet_stat(pid)
                    if (after is not None and after.starttime == stat.starttime
                            and expected is not None and _cgroup_of(pid) == expected
                            and not process.exited()):
                        process.send(signal.SIGKILL)
        while time.monotonic() < deadline:
            try:
                if _cgroup_events(path).get("populated") == "0":
                    break
            except FileNotFoundError:
                break
            time.sleep(0.02)
    remaining: list[str] = []
    try:
        still = _cgroup_events(path).get("populated") == "1"
    except OSError:
        still = False
    if still:
        for pid in _cgroup_members(path):
            stat = _quiet_stat(pid)
            remaining.append(f"pid {pid}" + ("" if stat is None else f" (state {stat.state})") + f" in {path}")
    else:
        _remove_cgroup(path)
    return len(members), remaining


# --------------------------------------------------------------------------- #
# Harness tree cleanup
# --------------------------------------------------------------------------- #


@dataclasses.dataclass
class CleanupReport:
    """What one harness-tree cleanup signalled, left behind, and could not prove."""

    containment: str
    killed: int = 0
    remaining: list[str] = dataclasses.field(default_factory=list)
    unproven: list[str] = dataclasses.field(default_factory=list)
    converged: bool = True

    @property
    def complete(self) -> bool:
        """True when every proven member of the tree is gone."""
        return not self.remaining

    def describe(self) -> str:
        """One human-readable line for errors and transcripts."""
        parts = [f"{self.killed} process(es) signalled"]
        if self.remaining:
            parts.append("still present after SIGKILL: " + ", ".join(self.remaining))
        if not self.converged:
            parts.append("the census did not converge before the deadline")
        if self.unproven:
            parts.append("not signalled because ownership is unproven: " + ", ".join(self.unproven))
        return "; ".join(parts)


def _children_of_self() -> list[int]:
    me = os.getpid()
    children: list[int] = []
    tasks = PROC / "self" / "task"
    try:
        for task in os.listdir(tasks):
            text = (tasks / task / "children").read_text()
            children.extend(int(item) for item in text.split() if item.isdigit())
        return sorted(set(children))
    except OSError:
        return sorted(stat.pid for stat in _scan_processes() if stat.ppid == me)


def reap_adopted_children(exclude: Sequence[int] = ()) -> int:
    """Reap exited orphans this child subreaper adopted; never touches ``exclude``.

    A process that is not a subreaper adopts nothing, and its children belong
    to whoever started them, so it reaps nothing.
    """
    reaped = 0
    if not _SUBREAPER:
        return reaped
    for pid in _children_of_self():
        if pid in exclude:
            continue
        stat = _quiet_stat(pid)
        if stat is None or stat.state not in _DEAD_STATES:
            continue
        try:
            if os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG) is not None:
                reaped += 1
        except ChildProcessError:
            continue
    return reaped


_SUBREAPER = False


def set_child_subreaper() -> bool:
    """Make this process adopt its descendants' orphans; False when unsupported.

    Only a runner calls this. Its children are then exactly the current
    harness leader and orphans adopted from that tree, which is what lets
    "child of the runner" prove membership. A process that is not a subreaper
    (a test, a CLI) never treats its own children as tree members.
    """
    global _SUBREAPER
    try:
        import ctypes

        libc = ctypes.CDLL(None, use_errno=True)
        libc.prctl.argtypes = [ctypes.c_int, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong]
        libc.prctl.restype = ctypes.c_int
        _SUBREAPER = int(libc.prctl(_PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0)) == 0
    except (OSError, AttributeError):
        _SUBREAPER = False
    return _SUBREAPER


def _describe(handle: ProcessHandle) -> str:
    stat = _quiet_stat(handle.pid)
    return f"pid {handle.pid}" + ("" if stat is None else f" (state {stat.state})")


def kill_harness_tree(
    record: HarnessRecord,
    *,
    leader: Optional[ProcessHandle] = None,
    leader_unreaped: bool = False,
    runner_is_self: bool = False,
    wait_s: Optional[float] = None,
) -> CleanupReport:
    """Kill every proven member of one turn's harness tree, bounded in time.

    Inside the runner, pass the runner's own ``leader`` handle, set
    ``runner_is_self``, and set ``leader_unreaped`` while the runner has not
    reaped the leader (its zombie still holds the session number). Other
    callers pass neither; the leader and the runner are pinned from the record.
    """
    if record.legacy:
        raise AgentOperationError(
            "harness_identity_legacy", "a legacy harness record carries no generation to prove its tree"
        )
    bound = KILL_WAIT_S if wait_s is None else wait_s
    started = time.monotonic()
    deadline = started + bound
    report = CleanupReport(record.containment)
    boot = current_boot_id()
    if record.leader.boot_id != boot:
        # Nothing recorded during an earlier boot can still exist.
        return report
    if record.cgroup is not None:
        if _cgroup_relative(record.cgroup) is None:
            raise AgentOperationError(
                "harness_identity_unknown",
                f"recorded harness cgroup {record.cgroup} is not inside the cgroup v2 hierarchy",
            )
        killed, remaining = _kill_cgroup(record.cgroup, deadline)
        report.killed += killed
        report.remaining.extend(remaining)
    if not pidfds_supported():
        if runner_is_self and isinstance(leader, ChildHandle):
            _peel_children(leader, report, deadline)
            return report
        if _tree_has_candidates(record):
            raise AgentOperationError(
                "pidfd_unsupported",
                f"harness pid {record.leader.pid} may still have live members, and this host does not "
                "provide pidfds to signal them safely",
            )
        return report
    own_leader = leader is None
    if leader is None:
        pinned = pin(record.leader)
        leader = pinned.process
        if leader is not None:
            current = _quiet_stat(leader.pid)
            if current is not None and current.session != record.process_group:
                leader.close()
                raise AgentOperationError(
                    "harness_identity_changed",
                    f"harness pid {record.leader.pid} no longer leads its recorded session {record.process_group}",
                )
    runner_handle: Optional[PinnedProcess] = None
    if not runner_is_self:
        pinned_runner = pin(record.runner)
        runner_handle = pinned_runner.process
    runner_pid = os.getpid() if runner_is_self else record.runner.pid
    members: dict[tuple[int, int], ProcessHandle] = {}
    signalled: set[tuple[int, int]] = set()
    finished = False
    try:
        if leader is not None and not leader.exited():
            # Stopped first: a stopped leader cannot fork, and it keeps holding
            # its session number, so the session proof stays valid.
            leader.send(signal.SIGSTOP)
        census_deadline = started + bound / 2
        report.converged = False
        while True:
            # Stop the world: every round pins and stops the members it finds;
            # stopped processes cannot fork, so an empty round is a fixed point.
            while True:
                if not _census_round(record, leader, leader_unreaped, runner_pid, runner_is_self,
                                     runner_handle, members, report):
                    report.converged = True
                    break
                if time.monotonic() >= census_deadline:
                    report.converged = False
                    break
            for key, handle in members.items():
                if key not in signalled:
                    signalled.add(key)
                    if handle.send(signal.SIGKILL):
                        report.killed += 1
            if leader is not None and (leader.pid, leader.starttime) not in signalled:
                signalled.add((leader.pid, leader.starttime))
                if not leader.exited() and leader.send(signal.SIGKILL):
                    report.killed += 1
            for handle in [*members.values(), *([leader] if leader is not None else [])]:
                handle.wait(max(0.0, deadline - time.monotonic()))
            # Anything forked after the last census (a non-converged tree)
            # is still proven by its generation or parent; go round again.
            before = len(members)
            _census_round(record, None, False, runner_pid, runner_is_self, runner_handle, members, report)
            if len(members) == before or time.monotonic() >= deadline:
                break
        report.remaining.extend(_describe(handle) for handle in members.values() if not handle.exited())
        if leader is not None and not leader.exited():
            report.remaining.append(_describe(leader))
        if runner_is_self:
            reap_adopted_children(exclude=() if leader is None else (leader.pid,))
        finished = True
        return report
    finally:
        if not finished:
            # An unexpected failure must not leave proven members stopped.
            for key, handle in members.items():
                if key not in signalled:
                    handle.send(signal.SIGKILL)
            if leader is not None and (leader.pid, leader.starttime) not in signalled and not leader.exited():
                leader.send(signal.SIGKILL)
        for handle in members.values():
            handle.close()
        if own_leader and leader is not None:
            leader.close()
        if runner_handle is not None:
            runner_handle.close()


def _minimum_start(record: HarnessRecord) -> int:
    """Every member of a tree started no earlier than its leader."""
    started_at = record.leader.started_at or ""
    return int(started_at) if _TICKS.fullmatch(started_at) else 0


def _tree_has_candidates(record: HarnessRecord) -> bool:
    """Read-only: could anything of this tree still be alive?"""
    leader = _quiet_stat(record.leader.pid)
    if (leader is not None and str(leader.starttime) == record.leader.started_at
            and leader.state not in _DEAD_STATES):
        return True
    if record.generation is None:
        return False
    minimum = _minimum_start(record)
    for stat in _scan_processes():
        if (stat.pid != os.getpid() and stat.starttime >= minimum and stat.state not in _DEAD_STATES
                and _same_user(stat.pid) and carries_generation(stat.pid, record.generation)):
            return True
    return False


def _census_round(
    record: HarnessRecord,
    leader: Optional[ProcessHandle],
    leader_unreaped: bool,
    runner_pid: int,
    runner_is_self: bool,
    runner_handle: Optional[PinnedProcess],
    members: dict[tuple[int, int], ProcessHandle],
    report: CleanupReport,
    *,
    stop: bool = True,
) -> int:
    """Pin (and by default SIGSTOP) every not-yet-seen proven member; return how many."""
    me = os.getpid()
    minimum = _minimum_start(record)

    def leader_present() -> bool:
        return leader is not None and (leader_unreaped or not leader.exited())

    def runner_present() -> bool:
        if runner_is_self:
            return _SUBREAPER
        return runner_handle is not None and not runner_handle.exited()

    found = 0
    unproven: set[int] = set()
    for stat in _scan_processes():
        if stat.pid == me or stat.state in _DEAD_STATES:
            continue
        if leader is not None and stat.pid == leader.pid and stat.starttime == leader.starttime:
            continue
        if stat.pid == runner_pid and (runner_is_self or (
                record.runner.started_at is not None and str(stat.starttime) == record.runner.started_at)):
            continue
        if (stat.pid, stat.starttime) in members:
            continue
        reason: Optional[str] = None
        if leader_present() and stat.session == record.process_group:
            reason = "session"
        elif runner_present() and stat.ppid == runner_pid:
            reason = "runner-child"
        elif (record.generation is not None and stat.starttime >= minimum and _same_user(stat.pid)
              and carries_generation(stat.pid, record.generation)):
            reason = "generation"
        if reason is None:
            if stat.session == record.process_group and not leader_present():
                unproven.add(stat.pid)
            continue
        handle = _pin_member(stat, reason, record, runner_pid)
        if handle is None:
            continue
        # The session and parent proofs hold only while their anchor still
        # holds its number; check after the member's own re-check.
        if (reason == "session" and not leader_present()) or (reason == "runner-child" and not runner_present()):
            handle.close()
            continue
        members[(stat.pid, stat.starttime)] = handle
        if stop:
            handle.send(signal.SIGSTOP)
        found += 1
    for pid in sorted(unproven):
        entry = f"pid {pid} (the exited leader's session, without this turn's generation)"
        if entry not in report.unproven:
            report.unproven.append(entry)
    return found


def _pin_member(stat: ProcessStat, reason: str, record: HarnessRecord, runner_pid: int) -> Optional[PinnedProcess]:
    try:
        descriptor = _open_pidfd(stat.pid)
    except OSError:
        return None
    if descriptor is None:
        return None
    process = PinnedProcess(stat.pid, descriptor, stat.starttime)
    after = _quiet_stat(stat.pid)
    proven = (
        after is not None
        and after.starttime == stat.starttime
        and after.state not in _DEAD_STATES
        and (
            (reason == "session" and after.session == record.process_group)
            or (reason == "runner-child" and after.ppid == runner_pid)
            or (reason == "generation" and record.generation is not None
                and carries_generation(stat.pid, record.generation))
        )
        and not process.exited()
    )
    if not proven:
        process.close()
        return None
    return process


def _peel_children(leader: ChildHandle, report: CleanupReport, deadline: float) -> None:
    """Without pidfds, kill this runner's own unreaped children until none remain.

    Killing a child hands its children to this subreaper, so each round
    reaches one more generation of the tree. Every signal targets an unreaped
    child, whose number cannot have been reused.
    """
    report.converged = False

    def live_children() -> list[int]:
        # Without the subreaper role this process's other children are not the
        # tree's; only the leader is reachable.
        candidates = _children_of_self() if _SUBREAPER else [leader.pid]
        return [pid for pid in candidates
                if (stat := _quiet_stat(pid)) is not None and stat.state not in _DEAD_STATES]

    while time.monotonic() < deadline:
        live = live_children()
        if not live:
            report.converged = True
            break
        for pid in live:
            if pid == leader.pid:
                if leader.send(signal.SIGKILL):
                    report.killed += 1
            else:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGKILL)  # parent-pinned: an unreaped adopted child
                    report.killed += 1
        time.sleep(0.02)
        reap_adopted_children(exclude=(leader.pid,))
    for pid in live_children():
        stat = _quiet_stat(pid)
        report.remaining.append(f"pid {pid}" + ("" if stat is None else f" (state {stat.state})"))


# --------------------------------------------------------------------------- #
# Spawning an owned harness
# --------------------------------------------------------------------------- #


class OwnedHarnessProcess(subprocess.Popen[str]):
    """A harness leader whose tree is swept before the leader is reaped.

    ``wait`` and ``poll`` first observe the leader's exit without reaping it,
    kill whatever else of its tree remains while the zombie leader still holds
    its session number, and only then reap. ``communicate`` inherits that
    through ``wait``. All signals use the leader's pidfd (or, without pidfds,
    the parent-pinned child handle).
    """

    ownership: Optional[HarnessRecord] = None
    leader_handle: Optional[ProcessHandle] = None
    cleanup: Optional[CleanupReport] = None

    def _bind(self, ownership: HarnessRecord, handle: ProcessHandle) -> None:
        self.ownership = ownership
        self.leader_handle = handle
        self._reap_lock = threading.RLock()
        self._swept = False

    def _record_report(self, report: CleanupReport) -> None:
        if self.cleanup is None:
            self.cleanup = report
            return
        self.cleanup.killed += report.killed
        self.cleanup.remaining = list(report.remaining)
        self.cleanup.unproven = sorted(set(self.cleanup.unproven) | set(report.unproven))
        self.cleanup.converged = self.cleanup.converged and report.converged

    def leader_exited(self) -> bool:
        """True once the leader has exited, whether or not it is reaped."""
        handle = self.leader_handle
        return self.returncode is not None if handle is None else self.returncode is not None or handle.exited()

    def kill_tree(self) -> CleanupReport:
        """Stop the world, then kill the whole tree (stop, timeout, protocol error)."""
        if self.ownership is None or self.leader_handle is None:
            self.kill()
            return CleanupReport(CONTAINMENT_PROCESS)
        with self._reap_lock:
            report = kill_harness_tree(
                self.ownership, leader=self.leader_handle,
                leader_unreaped=self.returncode is None, runner_is_self=True,
            )
            self._record_report(report)
            return report

    def sweep_after_exit(self) -> Optional[CleanupReport]:
        """Kill what the exited leader left behind, before the leader is reaped."""
        if self.ownership is None or self.leader_handle is None:
            return None
        with self._reap_lock:
            if self._swept or self.returncode is not None or not self.leader_handle.exited():
                return None
            report = kill_harness_tree(
                self.ownership, leader=self.leader_handle, leader_unreaped=True, runner_is_self=True,
            )
            self._swept = report.complete
            self._record_report(report)
            return report

    def poll(self) -> Optional[int]:
        if self.ownership is None or self.leader_handle is None:
            return super().poll()
        with self._reap_lock:
            if self.returncode is not None:
                return self.returncode
            if not self.leader_handle.exited():
                return None
            self.sweep_after_exit()
            return super().poll()

    def wait(self, timeout: Optional[float] = None) -> int:
        if self.ownership is None or self.leader_handle is None:
            return super().wait(timeout)
        if self.returncode is not None:
            return self.returncode
        if not self.leader_handle.wait(timeout):
            raise subprocess.TimeoutExpired(self.args, timeout if timeout is not None else 0.0)
        with self._reap_lock:
            if self.returncode is None:
                self.sweep_after_exit()
            return super().wait()

    def send_signal(self, sig: int) -> None:
        handle = self.leader_handle
        if handle is None:
            super().send_signal(sig)
            return
        with self._reap_lock:
            if self.returncode is None:
                handle.send(sig)

    def terminate(self) -> None:
        self.send_signal(signal.SIGTERM)

    def kill(self) -> None:
        self.send_signal(signal.SIGKILL)

    def finish(self) -> bool:
        """After the turn: True when the tree is gone and its record may be removed."""
        if self.ownership is None:
            return True
        complete = self.cleanup is None or self.cleanup.complete
        if self.returncode is None:
            return False
        if self.ownership.cgroup is not None:
            try:
                populated = _cgroup_events(self.ownership.cgroup).get("populated") == "1"
            except OSError:
                populated = False
            if populated:
                return False
            _remove_cgroup(self.ownership.cgroup)
        if self.leader_handle is not None:
            self.leader_handle.close()
        return complete


def _resolve_executable(argv: Sequence[str], environment: Mapping[str, str], cwd: str) -> None:
    executable = argv[0]
    if os.sep in executable:
        candidate = executable if os.path.isabs(executable) else os.path.join(cwd, executable)
        if os.access(candidate, os.X_OK) and not os.path.isdir(candidate):
            return
    elif shutil.which(executable, path=environment.get("PATH", os.defpath)) is not None:
        return
    raise FileNotFoundError(errno.ENOENT, os.strerror(errno.ENOENT), executable)


def _wait_gate_ready(descriptor: int, handle: ProcessHandle) -> None:
    deadline = time.monotonic() + GATE_READY_TIMEOUT_S
    while True:
        if _fd_readable(descriptor, 0.05):
            break
        if handle.exited():
            raise AgentOperationError("harness_spawn_failed", "the harness launch gate exited before it was ready")
        if time.monotonic() >= deadline:
            raise AgentOperationError("harness_spawn_failed", "the harness launch gate did not become ready")
    if os.read(descriptor, 2) != b"1":
        raise AgentOperationError("harness_spawn_failed", "the harness launch gate published invalid readiness")


def _create_cgroup(generation: str) -> Path:
    parent = own_cgroup_directory()
    if parent is None:
        raise OSError(errno.ENOTSUP, "the cgroup v2 hierarchy is not mounted")
    path = parent / f"{CGROUP_PREFIX}{generation}"
    os.mkdir(path, 0o755)
    return path


def _enter_cgroup(path: Path, pid: int) -> None:
    (path / "cgroup.procs").write_text(f"{pid}\n")
    expected = _cgroup_relative(path)
    if expected is None or _cgroup_of(pid) != expected:
        raise OSError(errno.EIO, f"pid {pid} did not enter {path}")


def spawn_owned_harness(
    argv: Sequence[str],
    *,
    cwd: str,
    stdin: bool,
    runner: ProcessIdentity,
    record: Callable[[HarnessRecord], None],
    on_cgroup_fallback: Optional[Callable[[str], None]] = None,
) -> OwnedHarnessProcess:
    """Start one turn's harness behind the launch gate and return it owned.

    The gate is inert until the runner has a pidfd on it (its own unreaped
    child, so the pidfd is exact), has moved it into this turn's cgroup when
    one is used, and has made ``record`` durable. Only then may it exec the
    harness, so no harness process exists without a durable ownership record,
    and a runner that dies first takes the gate with it (parent-death signal).
    """
    mode = cgroup_mode()
    generation = secrets.token_hex(16)
    environment = {GENERATION_ENV: generation}
    environment.update((key, value) for key, value in os.environ.items() if key != GENERATION_ENV)
    _resolve_executable(argv, environment, cwd)
    activation_read, activation_write = os.pipe2(os.O_CLOEXEC)
    readiness_read, readiness_write = os.pipe2(os.O_CLOEXEC)
    process: Optional[OwnedHarnessProcess] = None
    handle: Optional[ProcessHandle] = None
    cgroup: Optional[Path] = None
    try:
        process = OwnedHarnessProcess(
            [sys.executable, str(_GATE), str(os.getpid()), str(activation_read), str(readiness_write), *argv],
            cwd=cwd,
            stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            pass_fds=(activation_read, readiness_write),
            env=environment,
        )
        os.close(activation_read)
        activation_read = -1
        os.close(readiness_write)
        readiness_write = -1
        spawned = process
        stat = read_stat(spawned.pid)
        if stat is None or stat.ppid != os.getpid() or stat.session != spawned.pid or stat.state in _DEAD_STATES:
            raise AgentOperationError("harness_spawn_failed", "the harness launch gate has no stable identity")
        if pidfds_supported():
            descriptor = _open_pidfd(spawned.pid)
            if descriptor is None:
                raise AgentOperationError("harness_spawn_failed", "the harness launch gate exited before it was pinned")
            handle = PinnedProcess(spawned.pid, descriptor, stat.starttime)
        else:
            handle = ChildHandle(spawned.pid, stat.starttime, lambda: spawned.returncode is not None)
        _wait_gate_ready(readiness_read, handle)
        after = read_stat(spawned.pid)
        if after is None or after.starttime != stat.starttime or handle.exited():
            raise AgentOperationError("harness_spawn_failed", "the harness launch gate changed before activation")
        containment = CONTAINMENT_PROCESS
        if mode != "off":
            try:
                cgroup = _create_cgroup(generation)
                _enter_cgroup(cgroup, spawned.pid)
                containment = CONTAINMENT_CGROUP
            except OSError as exc:
                if cgroup is not None:
                    _remove_cgroup(cgroup)
                    cgroup = None
                if mode == "require":
                    raise AgentOperationError(
                        "harness_cgroup_unavailable", f"{CGROUP_MODE_ENV}=require but no per-turn cgroup: {exc}"
                    ) from exc
                if on_cgroup_fallback is not None:
                    on_cgroup_fallback(f"per-turn cgroup unavailable ({exc}); using process containment")
        ownership = HarnessRecord(
            runner=runner,
            leader=ProcessIdentity(spawned.pid, str(stat.starttime), current_boot_id()),
            process_group=spawned.pid,
            generation=generation,
            containment=containment,
            cgroup=cgroup,
        )
        record(ownership)
        spawned._bind(ownership, handle)
        while True:
            try:
                written = os.write(activation_write, b"1")
                break
            except InterruptedError:
                continue
        if written != 1:
            raise AgentOperationError("harness_spawn_failed", "the harness launch gate was not activated")
        return spawned
    except BaseException:
        if process is not None:
            if activation_write >= 0:
                # Closing activation makes an inert gate exit 125; kill it
                # anyway, through the exact handle while it is unreaped.
                os.close(activation_write)
                activation_write = -1
            if handle is not None:
                handle.send(signal.SIGKILL)
            else:
                subprocess.Popen.kill(process)
            with contextlib.suppress(subprocess.TimeoutExpired):
                subprocess.Popen.wait(process, timeout=5)
            for stream in (process.stdin, process.stdout, process.stderr):
                _close_stream(stream)
            if handle is not None:
                handle.close()
        if cgroup is not None:
            _remove_cgroup(cgroup)
        raise
    finally:
        for descriptor in (activation_read, readiness_write, activation_write, readiness_read):
            if descriptor >= 0:
                with contextlib.suppress(OSError):
                    os.close(descriptor)


def _close_stream(stream: Optional[IO[str]]) -> None:
    if stream is not None:
        with contextlib.suppress(OSError):
            stream.close()
