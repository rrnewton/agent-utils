#!/usr/bin/env python3
"""Admission control for validation gates: counting slots plus an exclusive class.

Drop-in replacement for the one-gate-at-a-time flock line:

    scripts/gate --base origin/main --keep-going
    scripts/gate --exclusive -- python3 vibe-talk/tests/read_aloud_blocks_browser.py

Shared gates run up to --slots (default 2) concurrently; each gate is already
boxed by the validation scheduler's own resource caps, so concurrency here is
admission, not scheduling. A per-host slots file (default
~/.local/etc/agent-utils/gate-slots.toml, override with $GATE_SLOTS_CONFIG)
holding `slots = N` caps every waiter at N, and is re-read on every poll, so
an operator can lower the host's limit (for example while disk is short)
without anyone re-queueing; with no file each waiter keeps its own --slots. An --exclusive run waits until nothing else is
running and blocks new admissions until it finishes, so load-sensitive steps
(timing-sensitive browser walks, solo shard reruns) never share the box.

Waiting callers print their queue position and elapsed wait. Every finished
run appends one JSON line to the state history for gates/hour and wait-time
measurement (`scripts/gate status`). Admission state lives outside the
repository (default /tmp/dev-agent-utils-gate-admission, override with
$GATE_ADMISSION_DIR); tickets and runs are files, reaped when their pid dies,
so a killed lane never holds a slot forever.

When a run finishes and has released its slot, it prunes this checkout's cargo
target dirs of builds nothing can reuse, under cargo's own locks, and reports
the bytes before and after, both under the target dirs and allocated on the
host to the slot image behind them (#940; `scripts/gate.py prune --help`).
--no-prune or GATE_PRUNE_TARGETS=off skips that; GATE_TRIM_COMMAND, when set,
runs after a prune that freed anything, for images mounted without online
discard (for example `wrkslots image trim agent SLOT`).

An admitted run's command runs in its own process group. SIGTERM, SIGINT or
SIGHUP to gate.py forwards SIGTERM to that whole group, waits a grace period
($GATE_ABORT_GRACE_S, default 10 s; twice that for a top-level gate, so a
nested gate can stop its own group first), then SIGKILLs whatever is left,
releases the slot and records the run as `aborted` in the ledger, so a
stopped gate never leaves dagrun and its steps running outside admission.

An admitted run's children see $GATE_ADMISSION_RUN_ID, and never inherit the
host's wrkslots Rust-readers routing opt-in (see `_child_environment`). A
gate invoked from inside an admitted run (validate.py's gate-exclusive phase) releases the
parent's slot before queueing: the parent is blocked on that call, so holding
its slot would deadlock, and the nested call must still wait for — and block —
every other gate as a fresh exclusive ticket.

Per-host blackout windows (#907): a gate must not START inside a configured
window (the shared gate host has a neighbour's nightly job). Windows come
from a per-host file that is never synced between hosts
(default ~/.local/etc/agent-utils/gate.toml, override $GATE_BLACKOUT_CONFIG):

    [[blackout]]
    start = "00:15"
    end = "04:00"
    timezone = "America/New_York"
    reason = "neighbour nightly job"

With no file there is no blackout; a malformed file refuses every start with
a clear error. A gate requested inside a window queues and waits it out,
starting when the window ends, so nothing runs in the window and the lane need
not resubmit; --no-wait-blackout refuses instead (exit 78), naming the window,
its reason and the next allowed start. --ignore-blackout overrides and is
recorded in the ledger. A ticket queued before a window begins likewise keeps
its place and is granted only when the window ends. Running gates are never
touched.

Rerun tickets: --rerun marks a short solo rerun of a few already-run steps
(an exclusive run of at most RERUN_MAX_STEPS `--selected` steps). It sorts
ahead of every ordinary ticket (FIFO among reruns) and still waits for running
gates to finish, so a two-minute rerun no longer waits behind every gate queued
after the one it repeats. $GATE_BLACKOUT_NOW (ISO 8601) overrides the clock
for tests.

Exclusive-phase priority (#1146 gate-exclusive-phase-starvation): that same
unbounded rerun priority starved a full gate's nested exclusive phase, which
queues as a fresh ordinary exclusive ticket. When the phase-priority switch
is on, a genuine nested exclusive gate — $GATE_ADMISSION_RUN_ID naming a
parent whose runs/ ticket is present and whose holder is still alive, the
liveness `_reap` applies before every grant — enqueues with the rerun
ordering mark (the ticket's `rerun` field, which every client generation
already sorts by) plus `nested_phase` and `phase_priority` fields naming the
phase and the switch source. No ordering rule changes: tickets queued later
cannot pass it, and the only reruns that can precede it are those already
waiting when it enqueues. The mark is only the ordering field. Every other
reader of `rerun` keys off the caller's own --rerun flag or the run's class,
never another ticket: the RERUN_MAX_STEPS check and the preflight skip run
only for an explicit --rerun (a phase's five steps keep the nested
preflight wording), blackout handling follows the parent admission, the
ledger kind still derives from the class, and an older client reading a
marked ticket does no max-steps or blackout judgement of it — it only sorts
it, exactly as it sorts a rerun. The switch defaults off: $GATE_PHASE_PRIORITY
(1/0) is the test and override hook and wins; otherwise a per-host file
(default ~/.local/etc/agent-utils/gate-phase-priority.toml, override with
$GATE_PHASE_PRIORITY_CONFIG) holding only `phase_priority = true` turns it
on; a malformed file means off and says why, never refusing a gate. It is
read once at enqueue, so flipping it never rewrites a queued ticket.
Rollout is the coordinators': land with the switch off, watch
`gate.py queue --explain` for a day, enable the first host in a quiet
window, then the second after a clean day.

`gate.py queue [--explain]` lists running runs and queued tickets in grant
order; --explain says for each ticket why it waits: the running runs and the
tickets ahead of it (and why each is ahead), a full shared box, or an active
blackout — including a starvation line when rerun tickets queued after an
unmarked exclusive-phase ticket stand ahead of it.

Preflight: before a top-level run takes a ticket, it runs the repository's
deterministic content checks on this checkout (PREFLIGHT_STEPS: the
client-name guard and the package contract, whose standalone-docs rules and
test censuses fail the same way every time) through the validation graph, in
about 20 seconds. If either fails, the run never queues: the failing steps'
lines are printed, the refusal is recorded in the ledger, and gate.py exits
PREFLIGHT_EXIT. Without the preflight, a lane learned of a one-word docstring
violation only after waiting for a ticket and running a full gate. A nested
gate and a --rerun ticket skip it, because their tree already passed it.
--skip-preflight skips it too, and the ledger records that it did.
$GATE_PREFLIGHT_COMMAND (a shell command) replaces the checks, for tests.
"""
from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import json
import os
import re
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import time
import uuid
from collections import Counter
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

if sys.version_info >= (3, 11):
    import tomllib
else:  # Python 3.10: the tomli package provides the same API
    import tomli as tomllib

DEFAULT_DIR = Path("/tmp/dev-agent-utils-gate-admission")

#: Durable per-run ledger (#827): /tmp history dies with every cleanup, taking the
#: full-gate pass rate with it, so finished runs also append here, outside /tmp.
#: Override with $GATE_LEDGER or --ledger.
DEFAULT_LEDGER = Path("~/.local/state/agent-utils/gate-ledger.jsonl")

_FAILED_STEP = re.compile(r"\[([^\]]+)\] ✗ FAIL")
_PASSED_STEP = re.compile(r"\[([^\]]+)\] ✓ PASS")
_VERDICT = re.compile(r"validate: (OK|PARTIAL|FAILED)")


def state_dir() -> Path:
    return Path(os.environ.get("GATE_ADMISSION_DIR") or DEFAULT_DIR)


def _child_environment(**overrides: str) -> dict[str, str]:
    """Environment for the preflight and the admitted command (#1170).

    A per-host Rust-readers opt-in is for interactive calls on that host,
    not for validation children: a sandbox that lacks the soak launcher
    must never inherit it. Force the explicit opt-out, which the engine
    resolver ranks above the host file, and point the file variable at a
    name that does not exist under the gate scratch directory.
    """

    return {
        **os.environ,
        "WRKSLOTS_RUST_READERS": "0",
        "WRKSLOTS_RUST_READERS_FILE": str(state_dir() / "no-wrkslots-rust-readers"),
        **overrides,
    }


#: Exit status for a start refused by a blackout window (#907).
BLACKOUT_EXIT = 78

#: A --rerun ticket may name at most this many `--selected` steps.
RERUN_MAX_STEPS = 4

#: The validation steps a top-level run passes before it may queue: checks of
#: the tree's content that fail identically on every run, so a failure can never
#: be load or luck, and finding it after a full gate wastes the gate.
PREFLIGHT_STEPS = (
    "repository.hygiene.client-names",
    "python.repository-infrastructure.package-contract",
)

#: Exit status for a run refused by the preflight (EX_DATAERR).
PREFLIGHT_EXIT = 65

DEFAULT_BLACKOUT_CONFIG = Path("~/.local/etc/agent-utils/gate.toml")

#: The host-wide slot cap lives in its own file, not in gate.toml: a checkout
#: older than this key refuses every start when gate.toml holds anything but
#: [[blackout]] tables, and every slot runs its own checkout's gate.py.
DEFAULT_SLOTS_CONFIG = Path("~/.local/etc/agent-utils/gate-slots.toml")


def slots_config_path() -> Path:
    return Path(os.environ.get("GATE_SLOTS_CONFIG") or DEFAULT_SLOTS_CONFIG).expanduser()


def host_slot_cap(path: Path | None = None) -> tuple[int | None, str | None]:
    """The host's slot cap and, when the file is unusable, why.

    No file means no cap. A file that cannot be read as `slots = N` with N a
    positive integer caps at 1: the file exists because someone wanted fewer
    concurrent gates, so a typo must not quietly restore the default.
    """

    path = slots_config_path() if path is None else path
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None, None
    except OSError as exc:
        return 1, f"gate slots config {path} is unreadable ({exc}); capping at 1"
    try:
        raw = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return 1, f"gate slots config {path} is not TOML ({exc}); capping at 1"
    value = raw.get("slots")
    if set(raw) != {"slots"} or isinstance(value, bool) or not isinstance(value, int) or value < 1:
        return 1, f"gate slots config {path} must hold only `slots = N` with N >= 1; capping at 1"
    return value, None


#: The phase-priority switch (#1146) lives in its own per-host file for the
#: same reason as the slot cap: an older gate.py refuses a gate.toml holding
#: anything but [[blackout]] tables and caps at 1 on extra slots-file keys.
DEFAULT_PHASE_PRIORITY_CONFIG = Path("~/.local/etc/agent-utils/gate-phase-priority.toml")


def phase_priority_config_path() -> Path:
    return Path(
        os.environ.get("GATE_PHASE_PRIORITY_CONFIG") or DEFAULT_PHASE_PRIORITY_CONFIG
    ).expanduser()


def phase_priority() -> tuple[bool, str]:
    """Whether nested exclusive phases get the rerun ordering mark, and what
    decided it: (enabled, source). $GATE_PHASE_PRIORITY (1/0) is the test and
    override hook and wins; otherwise the per-host file decides; no file and
    no env means off. A malformed file means off and names itself in the
    source, so `queue --explain` can show it; it never refuses a gate. Every
    source that ends in "; off" is a problem the enqueue warns about."""

    raw = os.environ.get("GATE_PHASE_PRIORITY")
    if raw is not None:
        value = raw.strip().lower()
        if value in ("1", "true", "yes", "on"):
            return True, f"env GATE_PHASE_PRIORITY={raw}"
        if value in ("0", "false", "no", "off", ""):
            return False, f"env GATE_PHASE_PRIORITY={raw} (override off)"
        return False, f"env GATE_PHASE_PRIORITY={raw!r} is not 1/0; off"
    path = phase_priority_config_path()
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return False, f"default off: no host file {path}"
    except OSError as exc:
        return False, f"host file {path} is unreadable ({exc}); off"
    try:
        parsed = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return False, f"host file {path} is not TOML ({exc}); off"
    value = parsed.get("phase_priority")
    if set(parsed) != {"phase_priority"} or not isinstance(value, bool):
        return False, f"host file {path} must hold only `phase_priority = true/false`; off"
    if value:
        return True, f"host file {path}"
    return False, f"host file {path} says phase_priority = false"


def blackout_config_path(explicit: str | None = None) -> Path:
    return Path(
        explicit
        or os.environ.get("GATE_BLACKOUT_CONFIG")
        or DEFAULT_BLACKOUT_CONFIG
    ).expanduser()


class BlackoutConfigError(RuntimeError):
    """The blackout config exists but cannot be used as written."""


@dataclasses.dataclass(frozen=True)
class BlackoutWindow:
    """One daily no-start window in its own time zone; end may pass midnight."""

    start: dt.time
    end: dt.time
    tz: ZoneInfo
    reason: str = ""

    def active_at(self, instant: dt.datetime) -> bool:
        local = instant.astimezone(self.tz).time()
        if self.start < self.end:
            return self.start <= local < self.end
        return local >= self.start or local < self.end

    def next_start_after(self, instant: dt.datetime) -> dt.datetime:
        """The first moment at or after ``instant`` when a start is allowed."""

        if not self.active_at(instant):
            return instant
        local = instant.astimezone(self.tz)
        end_date = local.date()
        if self.start >= self.end and local.time() >= self.start:
            end_date += dt.timedelta(days=1)
        return dt.datetime.combine(end_date, self.end, tzinfo=self.tz)

    def describe(self) -> str:
        return (
            f"{self.start:%H:%M}-{self.end:%H:%M} {self.tz}"
            + (f" ({self.reason})" if self.reason else "")
        )


def _parse_hhmm(value: object, path: Path, field: str) -> dt.time:
    if not isinstance(value, str) or not re.fullmatch(r"([01]\d|2[0-3]):[0-5]\d", value):
        raise BlackoutConfigError(
            f"gate blackout config {path}: {field} must be an \"HH:MM\" time, not {value!r}"
        )
    return dt.time(int(value[:2]), int(value[3:]))


def load_blackout_windows(explicit: str | None = None) -> tuple[BlackoutWindow, ...]:
    """The configured windows; no file means no blackout, a bad file is an error."""

    path = blackout_config_path(explicit)
    if not path.is_file():
        return ()
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise BlackoutConfigError(f"gate blackout config {path} is unreadable: {exc}") from exc
    if not isinstance(raw, dict) or set(raw) - {"blackout"}:
        raise BlackoutConfigError(
            f"gate blackout config {path}: only [[blackout]] tables are allowed"
        )
    entries = raw.get("blackout", [])
    if not isinstance(entries, list):
        raise BlackoutConfigError(f"gate blackout config {path}: blackout must be a list of tables")
    windows: list[BlackoutWindow] = []
    for index, entry in enumerate(entries):
        label = f"blackout #{index + 1}"
        if not isinstance(entry, dict) or not set(entry) <= {"start", "end", "timezone", "reason"}:
            raise BlackoutConfigError(
                f"gate blackout config {path}: {label} may only set start, end, timezone, reason"
            )
        start = _parse_hhmm(entry.get("start"), path, f"{label} start")
        end = _parse_hhmm(entry.get("end"), path, f"{label} end")
        if start == end:
            raise BlackoutConfigError(
                f"gate blackout config {path}: {label} start and end are equal ({start:%H:%M})"
            )
        tz_name = entry.get("timezone")
        if not isinstance(tz_name, str) or not tz_name:
            raise BlackoutConfigError(
                f"gate blackout config {path}: {label} timezone must name a zone"
            )
        try:
            tz = ZoneInfo(tz_name)
        except (ZoneInfoNotFoundError, ValueError) as exc:
            raise BlackoutConfigError(
                f"gate blackout config {path}: {label} timezone {tz_name!r} is unknown"
            ) from exc
        reason = entry.get("reason", "")
        if not isinstance(reason, str):
            raise BlackoutConfigError(
                f"gate blackout config {path}: {label} reason must be a string"
            )
        windows.append(BlackoutWindow(start, end, tz, reason))
    return tuple(windows)


def blackout_now() -> dt.datetime:
    """Now, or the $GATE_BLACKOUT_NOW test override (ISO 8601)."""

    raw = os.environ.get("GATE_BLACKOUT_NOW")
    if not raw:
        return dt.datetime.now(dt.timezone.utc)
    try:
        parsed = dt.datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError as exc:
        raise BlackoutConfigError(f"GATE_BLACKOUT_NOW is not an ISO 8601 time: {raw!r}") from exc
    if parsed.tzinfo is None:
        raise BlackoutConfigError(f"GATE_BLACKOUT_NOW must carry a time zone: {raw!r}")
    return parsed


def active_blackout(
    windows: tuple[BlackoutWindow, ...], instant: dt.datetime
) -> BlackoutWindow | None:
    return next((window for window in windows if window.active_at(instant)), None)


def ledger_path(explicit: str | None = None) -> Path:
    return Path(explicit or os.environ.get("GATE_LEDGER") or DEFAULT_LEDGER).expanduser()


def _utc(unix: float) -> str:
    return dt.datetime.fromtimestamp(unix, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _git_rev_parse(ref: str) -> str | None:
    try:
        done = subprocess.run(
            ["git", "rev-parse", ref], capture_output=True, text=True, timeout=10
        )
        return done.stdout.strip() if done.returncode == 0 else None
    except Exception:
        return None


def _alive(pid: object) -> bool:
    try:
        os.kill(int(str(pid)), 0)
        return True
    except (ProcessLookupError, ValueError, TypeError):
        return False


class Admission:
    def __init__(self, root: Path, slots: int) -> None:
        self.root = root
        self.slots = slots
        self._slots_problem: str | None = None
        (root / "queue").mkdir(parents=True, exist_ok=True)
        (root / "runs").mkdir(parents=True, exist_ok=True)
        self._lock = (root / "admission.lock").open("a+")

    def _locked(self) -> None:
        fcntl.flock(self._lock, fcntl.LOCK_EX)

    def _unlocked(self) -> None:
        fcntl.flock(self._lock, fcntl.LOCK_UN)

    @staticmethod
    def _read(path: Path) -> dict[str, object] | None:
        try:
            data = json.loads(path.read_text())
            return data if isinstance(data, dict) else None
        except Exception:
            return None

    def effective_slots(self) -> int:
        """This waiter's --slots, lowered to the host cap when one is set."""
        cap, problem = host_slot_cap()
        if problem and problem != self._slots_problem:
            print(f"gate: {problem}", flush=True)
        self._slots_problem = problem
        return self.slots if cap is None else min(self.slots, cap)

    def _reap(self) -> None:
        for folder in ("queue", "runs"):
            for path in (self.root / folder).glob("*.json"):
                data = self._read(path)
                if data is None or not _alive(data.get("pid")):
                    path.unlink(missing_ok=True)

    def tickets(self) -> list[tuple[Path, dict[str, object]]]:
        """Queued tickets in grant order: reruns first, then FIFO."""
        out = []
        for path in sorted((self.root / "queue").glob("*.json")):
            data = self._read(path)
            if data is not None:
                out.append((path, data))
        out.sort(key=lambda item: 0 if item[1].get("rerun") else 1)
        return out

    def runs(self) -> list[dict[str, object]]:
        out = []
        for path in sorted((self.root / "runs").glob("*.json")):
            data = self._read(path)
            if data is not None:
                out.append(data)
        return out

    def parent_is_live(self, run_id: str) -> bool:
        """Whether `run_id` names a parent run whose holder still lives: the
        runs/ ticket is present and its pid passes the same `_alive` check
        `_reap` applies before a grant. A stale file left behind, or the env
        var alone, is not a parent (#1146)."""
        data = self._read(self.root / "runs" / f"{run_id}.json")
        return data is not None and _alive(data.get("pid"))

    def enqueue(
        self,
        cls: str,
        command: list[str],
        rerun: bool = False,
        nested_phase: bool = False,
        phase_priority_source: str | None = None,
    ) -> tuple[str, Path]:
        run_id = f"{time.time_ns()}-{uuid.uuid4().hex[:8]}"
        ticket: dict[str, object] = {
            "id": run_id, "pid": os.getpid(), "class": cls,
            "queued_at": time.time(), "command": command, "rerun": rerun,
            "nested_phase": nested_phase, "phase_priority": phase_priority_source,
        }
        self._locked()
        try:
            self._reap()
            path = self.root / "queue" / f"{run_id}.json"
            path.write_text(json.dumps(ticket))
        finally:
            self._unlocked()
        return run_id, path

    def grant_blockers(
        self,
        run_id: str,
        cls: str,
        tickets: list[tuple[Path, dict[str, object]]],
        runs: list[dict[str, object]],
    ) -> list[tuple[str, dict[str, object], str]]:
        """What blocks `run_id` from a grant: (kind, ticket-or-run, reason)
        rows implementing exactly the class rules `try_grant` applies, so
        `queue --explain` can name the same blockers a grant would see."""
        ids = [str(data["id"]) for _, data in tickets]
        if run_id not in ids:
            return []
        ahead = tickets[: ids.index(run_id)]
        if cls == "exclusive":
            return (
                [("ticket", data, "ahead") for _, data in ahead]
                + [("run", run, "running") for run in runs]
            )
        blockers: list[tuple[str, dict[str, object], str]] = [
            ("ticket", data, "ahead") for _, data in ahead if data.get("class") == "exclusive"
        ]
        blockers += [("run", run, "running") for run in runs if run.get("class") == "exclusive"]
        if not blockers and len(runs) >= self.effective_slots():
            blockers += [("run", run, "slots-full") for run in runs]
        return blockers

    def try_grant(self, run_id: str, cls: str) -> bool:
        """Grant admission if FIFO order and the class rules allow it."""
        self._locked()
        try:
            self._reap()
            tickets = self.tickets()
            ids = [str(data["id"]) for _, data in tickets]
            if run_id not in ids:
                return False
            if self.grant_blockers(run_id, cls, tickets, self.runs()):
                return False
            for path, data in tickets:
                if str(data["id"]) == run_id:
                    data["started_at"] = time.time()
                    (self.root / "runs" / path.name).write_text(json.dumps(data))
                    path.unlink()
                    return True
            return False
        finally:
            self._unlocked()

    def position(self, run_id: str) -> tuple[int, int]:
        self._locked()
        try:
            self._reap()
            tickets = self.tickets()
            ids = [str(data["id"]) for _, data in tickets]
            pos = ids.index(run_id) + 1 if run_id in ids else 0
            return pos, len(self.runs())
        finally:
            self._unlocked()

    def finish(self, run_id: str) -> None:
        self._locked()
        try:
            (self.root / "runs" / f"{run_id}.json").unlink(missing_ok=True)
        finally:
            self._unlocked()

    def record(self, event: dict[str, object]) -> None:
        with (self.root / "history.jsonl").open("a") as fh:
            fh.write(json.dumps(event, sort_keys=True) + "\n")

    def history(self) -> list[dict[str, object]]:
        path = self.root / "history.jsonl"
        if not path.exists():
            return []
        out: list[dict[str, object]] = []
        for line in path.read_text(errors="replace").splitlines():
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                out.append(row)
        return out


def _refuse_blackout(window: BlackoutWindow, instant: dt.datetime) -> int:
    nxt = window.next_start_after(instant).astimezone(window.tz)
    print(
        f"gate: blackout {window.describe()}: no gate may start now; "
        f"next allowed start {nxt:%Y-%m-%d %H:%M %Z}; "
        "rerun then, or pass --ignore-blackout to override (recorded)",
        flush=True,
    )
    return BLACKOUT_EXIT


def _selected_steps(command: list[str]) -> list[str]:
    selected: list[str] = []
    for index, word in enumerate(command):
        if word == "--selected" and index + 1 < len(command):
            selected.extend(command[index + 1].split(","))
        elif word.startswith("--selected="):
            selected.extend(word.split("=", 1)[1].split(","))
    return [step for step in selected if step]


def _rerun_problem(command: list[str]) -> str | None:
    """Why `command` is not a short rerun of named steps, or None if it is."""

    selected = _selected_steps(command)
    if not selected:
        return "a rerun names its steps with --selected A,B"
    if len(selected) > RERUN_MAX_STEPS:
        return f"{len(selected)} steps selected; a rerun takes at most {RERUN_MAX_STEPS}"
    return None


def _repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def preflight_command() -> list[str]:
    """The preflight's command: PREFLIGHT_STEPS through the validation graph, or
    $GATE_PREFLIGHT_COMMAND run by the shell."""

    override = os.environ.get("GATE_PREFLIGHT_COMMAND")
    if override:
        return ["/bin/sh", "-c", override]
    root = _repo_root()
    return [str(root / "common" / "bin" / "dagrun"), "run",
            "--dag", str(root / "validation.dag.yaml"), "--keep-going", "--no-profile",
            "--selected", ",".join(PREFLIGHT_STEPS)]


def run_preflight() -> tuple[bool, float, list[str], list[str]]:
    """Run the preflight in this checkout: (passed, seconds, failed steps, lines to show)."""

    started = time.time()
    try:
        proc = subprocess.run(preflight_command(), cwd=_repo_root(), env=_child_environment(),
                              stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, errors="replace")
        returncode, lines = proc.returncode, proc.stdout.splitlines()
    except OSError as exc:
        returncode, lines = 127, [f"gate: preflight could not start: {exc}"]
    seconds = time.time() - started
    if returncode == 0:
        return True, seconds, [], []
    failed = sorted({m.group(1) for line in lines if (m := _FAILED_STEP.search(line))})
    shown = [line for line in lines
             if any(line.startswith(f"[{step}]") for step in failed) and "elapsed, no test" not in line]
    return False, seconds, failed, shown or lines[-40:]


def run_admitted(args: argparse.Namespace, command: list[str]) -> int:
    try:
        windows = load_blackout_windows(getattr(args, "blackout_config", None))
        now = blackout_now()
    except BlackoutConfigError as exc:
        print(f"gate: {exc}", flush=True)
        return 2
    ignore = bool(getattr(args, "ignore_blackout", False))
    parent_run = os.environ.get("GATE_ADMISSION_RUN_ID")
    # A genuine nested gate (validate.py's exclusive phase) names a parent
    # run that holds a ticket in this same admission directory; the variable
    # alone also leaks into unrelated children of a running gate (tests,
    # shells), and those are fresh starts the blackout still refuses.
    nested = bool(parent_run) and (state_dir() / "runs" / f"{parent_run}.json").is_file()
    active = active_blackout(windows, now)
    if active is not None and not ignore and not nested and getattr(args, "no_wait_blackout", False):
        return _refuse_blackout(active, now)
    rerun = bool(getattr(args, "rerun", False))
    if rerun:
        problem = _rerun_problem(command)
        if problem:
            print(f"gate: --rerun refused: {problem}", flush=True)
            return 2
        args.cls = "exclusive"
    if nested:
        preflight = "skipped: nested gate"
    elif rerun:
        preflight = "skipped: rerun"
    elif getattr(args, "skip_preflight", False):
        preflight = "skipped: --skip-preflight"
    else:
        passed, seconds, failed, shown = run_preflight()
        if not passed:
            print(f"gate: preflight FAILED in {seconds:.0f}s, so this run was not queued; "
                  "fix these and run the gate again (--skip-preflight overrides, recorded):",
                  flush=True)
            for line in shown:
                print(line, flush=True)
            now_s = time.time()
            _record_ledger(args, command, now_s, now_s, PREFLIGHT_EXIT, set(), set(failed),
                           "FAILED", kind="preflight-refused", preflight="failed")
            return PREFLIGHT_EXIT
        print(f"gate: preflight passed in {seconds:.0f}s ({', '.join(PREFLIGHT_STEPS)})", flush=True)
        preflight = "passed"
    adm = Admission(state_dir(), args.slots)
    # #1146: the rerun ordering mark needs a live parent, checked with the
    # same liveness as a grant, before the parent slot is released below. An
    # explicit --rerun is never a phase, however it was invoked. The mark is
    # only the ticket's ordering field; `rerun` above stays the caller's own
    # flag for the step check, the preflight wording and blackout handling.
    nested_live = bool(parent_run and adm.parent_is_live(parent_run))
    nested_phase = nested_live and args.cls == "exclusive" and not rerun
    phase_enabled, phase_source = phase_priority() if nested_phase else (False, None)
    phase_marked = nested_phase and phase_enabled
    phase_mark_source = phase_source if phase_marked else None
    if phase_marked:
        print(f"gate: nested exclusive phase queues in the rerun tier ({phase_source})", flush=True)
    elif phase_source is not None and phase_source.endswith("; off"):
        print(f"gate: warning: phase priority off: {phase_source}", flush=True)
    # A gate invoked from inside an admitted gate (validate.py running its
    # gate-exclusive steps) must not queue behind its own parent: the parent
    # process is blocked waiting for this call and will never finish. Release
    # the parent's slot first; this call then queues as a fresh top-level
    # ticket, so an exclusive phase still waits for every other gate and
    # blocks new ones, with no deadlock.
    if parent_run:
        adm.finish(parent_run)
    run_id, _ = adm.enqueue(
        args.cls, command, rerun or phase_marked,
        nested_phase=nested_phase, phase_priority_source=phase_mark_source,
    )
    queued_at = time.time()
    last_pos = -1
    blackout_noted: BlackoutWindow | None = None
    while True:
        if args.wait_timeout and time.time() - queued_at > args.wait_timeout:
            adm.finish(run_id)
            (state_dir() / "queue" / f"{run_id}.json").unlink(missing_ok=True)
            print(f"gate: gave up after {time.time() - queued_at:.0f}s in queue", flush=True)
            return 75
        # A ticket queued before a window begins keeps its place but is not
        # granted during the window: the grant is the start (#907).
        current = None if ignore else active_blackout(windows, blackout_now())
        if current is not None:
            if blackout_noted is not current:
                nxt = current.next_start_after(blackout_now()).astimezone(current.tz)
                print(
                    f"gate: queued ticket waits out blackout {current.describe()}; "
                    f"starts resume {nxt:%Y-%m-%d %H:%M %Z}",
                    flush=True,
                )
                blackout_noted = current
            time.sleep(1.0)
            continue
        if adm.try_grant(run_id, args.cls):
            break
        pos, running = adm.position(run_id)
        if pos != last_pos:
            print(f"gate: queued at position {pos} ({running} running, class={args.cls})", flush=True)
            last_pos = pos
        time.sleep(1.0)
    waited = time.time() - queued_at
    print(f"gate: admitted after {waited:.0f}s (class={args.cls})", flush=True)
    started = time.time()
    rc = 1
    passed_steps: set[str] = set()
    failed_steps: set[str] = set()
    verdict_word: str | None = None
    last_line = ""
    aborted: str | None = None
    previous = {sig: signal.getsignal(sig) for sig in _ABORT_SIGNALS}
    proc: subprocess.Popen[str] | None = None
    try:
        for sig in _ABORT_SIGNALS:
            signal.signal(sig, _raise_aborted)
        proc = subprocess.Popen(
            command,
            env=_child_environment(GATE_ADMISSION_RUN_ID=run_id),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            bufsize=1,
            # Its own process group, so an abort can stop the whole tree.
            preexec_fn=os.setpgrp,
        )
        assert proc.stdout is not None
        for line in proc.stdout:
            sys.stdout.write(line)
            if line.strip():
                last_line = line
            match = _FAILED_STEP.search(line)
            if match:
                failed_steps.add(match.group(1))
            match = _PASSED_STEP.search(line)
            if match:
                passed_steps.add(match.group(1))
            match = _VERDICT.search(line)
            if match:
                verdict_word = match.group(1)
        rc = proc.wait()
    except _Aborted as exc:
        name = signal.Signals(exc.signum).name
        how = _stop_group(proc, _abort_grace(nested)) if proc is not None else "not started"
        aborted = f"{name}; run's process group {how}"
        rc = 128 + exc.signum
        print(f"gate: aborted by {aborted}", flush=True)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        finished = time.time()
        adm.finish(run_id)
        adm.record({"id": run_id, "class": args.cls, "command": command,
                    "queued_at": queued_at, "started_at": started,
                    "finished_at": finished, "wait_s": round(waited, 2),
                    "run_s": round(finished - started, 2), "exit_code": rc,
                    "rerun": rerun, "nested_phase": nested_phase,
                    "phase_priority": phase_mark_source})
        # After the slot is released, so no other gate waits on it (#940). A
        # nested gate leaves it to its parent, which is still building.
        pruned = None if nested or getattr(args, "no_prune", False) else _prune_after_gate()
        if pruned is not None and last_line:
            # The run's verdict stays the log's last line (`tail -1 LOG`).
            sys.stdout.write(last_line if last_line.endswith("\n") else last_line + "\n")
            sys.stdout.flush()
        _record_ledger(args, command, started, finished, rc,
                       passed_steps, failed_steps, verdict_word, pruned, queued_at,
                       preflight=preflight, aborted=aborted, rerun_flag=rerun,
                       nested_phase=nested_phase, phase_priority_source=phase_mark_source)
    return rc


class _Aborted(Exception):
    """gate.py was told to stop while its command ran."""

    def __init__(self, signum: int) -> None:
        super().__init__(signum)
        self.signum = signum


_ABORT_SIGNALS = (signal.SIGTERM, signal.SIGINT, signal.SIGHUP)


def _raise_aborted(signum: int, _frame: object) -> None:
    raise _Aborted(signum)


def _abort_grace(nested: bool) -> float:
    try:
        grace = float(os.environ.get("GATE_ABORT_GRACE_S", "10"))
    except ValueError:
        grace = 10.0
    return max(grace, 0.0) * (1 if nested else 2)


def _group_alive(proc: subprocess.Popen[str]) -> bool:
    proc.poll()
    try:
        os.killpg(proc.pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def _stop_group(proc: subprocess.Popen[str], grace: float) -> str:
    """SIGTERM the command's process group, then SIGKILL what outlives `grace`."""

    for sig in _ABORT_SIGNALS:
        signal.signal(sig, signal.SIG_IGN)
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return "already gone"
    deadline = time.time() + grace
    while time.time() < deadline:
        if not _group_alive(proc):
            return "stopped by SIGTERM"
        time.sleep(0.1)
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        return "stopped by SIGTERM"
    proc.wait()
    return f"SIGKILLed after {grace:.0f}s"


def _record_ledger(
    args: argparse.Namespace,
    command: list[str],
    started: float,
    finished: float,
    rc: int,
    passed_steps: set[str],
    failed_steps: set[str],
    verdict_word: str | None,
    pruned: dict[str, int | None] | None = None,
    queued: float | None = None,
    *,
    kind: str | None = None,
    preflight: str | None = None,
    aborted: str | None = None,
    rerun_flag: bool = False,
    nested_phase: bool = False,
    phase_priority_source: str | None = None,
) -> None:
    """Append one durable ledger line for the finished run (#827). Best effort:
    a ledger failure must never change the gate's own result. `queued_utc` and
    `wait_s` record the time spent in the queue before admission, which
    `started_utc` alone cannot show. `preflight` says whether the preflight
    passed or why it was skipped; a run it refused is a `preflight-refused`
    line, which stats counts apart from gates."""

    try:
        base: str | None = None
        if "--base" in command:
            base = command[command.index("--base") + 1]
        verdict = "aborted" if aborted else {"OK": "pass", "PARTIAL": "partial", "FAILED": "fail"}.get(
            verdict_word or "", "pass" if rc == 0 else "fail"
        )
        if kind is None:
            kind = "exclusive-rerun" if args.cls == "exclusive" else "gate"
        line = {
            "queued_utc": _utc(queued) if queued is not None else None,
            "wait_s": round(started - queued, 2) if queued is not None else None,
            "started_utc": _utc(started),
            "finished_utc": _utc(finished),
            "base": base,
            "base_sha": _git_rev_parse(base) if base else None,
            "head_sha": _git_rev_parse("HEAD"),
            "selection": "selected" if "--selected" in " ".join(command) else "full",
            "kind": kind,
            "steps": len(passed_steps) + len(failed_steps),
            "failed_steps": sorted(failed_steps),
            "rerun_passed": (rc == 0) if kind == "exclusive-rerun" else None,
            "verdict": verdict,
            "exit_code": rc,
            "blackout_ignored": bool(getattr(args, "ignore_blackout", False)),
            "preflight": preflight,
            "aborted": aborted,
            "rerun": rerun_flag,
            "nested_phase": nested_phase,
            "phase_priority": phase_priority_source,
            "log_path": None,
            "target_bytes_before": (pruned or {}).get("target_bytes_before"),
            "target_bytes_after": (pruned or {}).get("target_bytes_after"),
            "host_bytes_before": (pruned or {}).get("host_bytes_before"),
            "host_bytes_after": (pruned or {}).get("host_bytes_after"),
            "trim_exit": (pruned or {}).get("trim_exit"),
        }
        path = ledger_path(getattr(args, "ledger", None))
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as fh:
            fh.write(json.dumps(line, sort_keys=True) + "\n")
    except Exception:
        pass


def show_status(args: argparse.Namespace) -> int:
    adm = Admission(state_dir(), args.slots)
    adm._locked()
    try:
        adm._reap()
        queue = [(str(d["id"]), str(d["class"])) for _, d in adm.tickets()]
        runs = [(str(d["id"]), str(d.get("class"))) for d in adm.runs()]
    finally:
        adm._unlocked()
    rows = adm.history()
    cutoff = time.time() - 24 * 3600
    recent = [r for r in rows if float(str(r.get("finished_at", 0))) >= cutoff]
    waits = sorted(float(str(r["wait_s"])) for r in recent if "wait_s" in r)
    print(f"gate status: {len(runs)} running {runs}, {len(queue)} queued {queue}")
    cap, problem = host_slot_cap()
    print(f"gate status: shared slots {adm.effective_slots()}"
          + (f" (host cap {cap} from {slots_config_path()})" if cap is not None else
             f" (no host cap; {slots_config_path()} absent)")
          + (f"; {problem}" if problem else ""))
    print(f"gate status: {len(recent)} finished in 24h "
          f"({len(recent) / 24.0:.1f}/hour), "
          f"median wait {(waits[len(waits) // 2] if waits else 0):.0f}s, "
          f"max wait {(waits[-1] if waits else 0):.0f}s")
    return 0


def _float_or(value: object, default: float = 0.0) -> float:
    try:
        return float(str(value))
    except (TypeError, ValueError):
        return default


def _age_text(seconds: float) -> str:
    seconds = max(seconds, 0.0)
    if seconds < 90:
        return f"{seconds:.0f}s"
    if seconds < 5400:
        return f"{seconds / 60:.0f}m"
    return f"{seconds / 3600:.1f}h"


def _command_list(data: dict[str, object]) -> list[str]:
    command = data.get("command")
    return [str(word) for word in command] if isinstance(command, list) else []


def _command_summary(data: dict[str, object]) -> str:
    command = _command_list(data)
    selected = _selected_steps(command)
    if selected:
        summary = "--selected " + ",".join(selected)
    elif command:
        summary = shlex.join(command)
    else:
        summary = "(no command recorded)"
    return summary if len(summary) <= 160 else summary[:157] + "..."


def _is_inferred_phase(data: dict[str, object]) -> bool:
    """A ticket from before phase marks that validate.py's exclusive phase
    would have written: an exclusive dagrun at --max-steps 1 over the
    cross.* gate-exclusive steps."""
    if data.get("class") != "exclusive" or data.get("nested_phase"):
        return False
    command = _command_list(data)
    selected = _selected_steps(command)
    max_one = any(
        command[i] == "--max-steps" and i + 1 < len(command) and command[i + 1] == "1"
        for i in range(len(command))
    )
    return bool(max_one and selected and all(step.startswith("cross.") for step in selected))


def _ticket_label(data: dict[str, object]) -> str:
    if data.get("nested_phase"):
        if data.get("rerun"):
            return f"nested exclusive phase (rerun-tier mark on, {data.get('phase_priority')})"
        return "nested exclusive phase (unmarked)"
    if data.get("rerun"):
        return "--rerun ticket"
    if _is_inferred_phase(data):
        return "inferred gate-exclusive phase (ticket predates phase marks)"
    return f"{data.get('class')} ticket"


def _why_ahead(ahead: dict[str, object], ticket: dict[str, object]) -> str:
    if ahead.get("rerun") and not ticket.get("rerun"):
        return "rerun tier"
    if ahead.get("rerun"):
        return "FIFO within rerun tier"
    return "FIFO"


def show_queue(args: argparse.Namespace) -> int:
    try:
        windows = load_blackout_windows(getattr(args, "blackout_config", None))
        now = blackout_now()
    except BlackoutConfigError as exc:
        print(f"gate: {exc}", flush=True)
        return 2
    adm = Admission(state_dir(), args.slots)
    adm._locked()
    try:
        adm._reap()
        tickets = adm.tickets()
        runs = adm.runs()
    finally:
        adm._unlocked()
    enabled, source = phase_priority()
    print(f"gate queue: {len(runs)} running, {len(tickets)} queued (grant order)")
    print(f"gate queue: phase priority {'on' if enabled else 'off'} ({source})")
    cap, problem = host_slot_cap()
    print(f"gate queue: shared slots {adm.effective_slots()}"
          + (f" (host cap {cap} from {slots_config_path()})" if cap is not None else
             f" (no host cap; {slots_config_path()} absent)")
          + (f"; {problem}" if problem else ""))
    blackout = active_blackout(windows, now)
    if blackout is not None:
        nxt = blackout.next_start_after(now).astimezone(blackout.tz)
        print(f"gate queue: blackout {blackout.describe()}: no gate may start; "
              f"starts resume {nxt:%Y-%m-%d %H:%M %Z}")
    now_s = time.time()
    for run in runs:
        print(f"gate queue: running {run.get('id')} class={run.get('class')} "
              f"for {_age_text(now_s - _float_or(run.get('started_at'), now_s))}: "
              f"{_command_summary(run)}")
    for index, (_, data) in enumerate(tickets):
        run_id = str(data.get("id"))
        print(f"gate queue: #{index + 1} {run_id} class={data.get('class')} "
              f"{_ticket_label(data)}, queued "
              f"{_age_text(now_s - _float_or(data.get('queued_at'), now_s))} ago: "
              f"{_command_summary(data)}")
        if not getattr(args, "explain", False):
            continue
        if blackout is not None:
            print("gate queue:   blocked by blackout (see above)")
        else:
            blockers = adm.grant_blockers(run_id, str(data.get("class")), tickets, runs)
            if not blockers:
                print("gate queue:   admissible now; waiting only for its own poll")
            for kind, blocker, reason in blockers:
                blocker_id = str(blocker.get("id"))
                if kind == "ticket":
                    print(f"gate queue:   blocked by ticket {blocker_id} ahead "
                          f"({_why_ahead(blocker, data)})")
                elif reason == "slots-full":
                    print(f"gate queue:   blocked by a full shared box "
                          f"({len(runs)}/{adm.effective_slots()} slots in use): "
                          f"running {blocker_id}")
                else:
                    print(f"gate queue:   blocked by running {blocker_id} "
                          f"(class={blocker.get('class')}, running "
                          f"{_age_text(now_s - _float_or(blocker.get('started_at'), now_s))})")
        if data.get("nested_phase") or _is_inferred_phase(data):
            ahead = tickets[:index]
            starved = sum(
                1 for _, other in ahead
                if other.get("rerun")
                and _float_or(other.get("queued_at")) > _float_or(data.get("queued_at"))
            )
            if starved:
                print(f"gate queue:   starvation: {starved} rerun ticket(s) queued after "
                      "this ticket are ahead of it")
    return 0


def _parse_since(text: str) -> float:
    match = re.fullmatch(r"(\d+)([hd])", text)
    if match:
        hours = int(match.group(1)) * (24 if match.group(2) == "d" else 1)
        return time.time() - hours * 3600
    return dt.datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()


def show_stats(args: argparse.Namespace) -> int:
    path = ledger_path(getattr(args, "ledger", None))
    rows: list[dict[str, object]] = []
    if path.exists():
        for raw in path.read_text(errors="replace").splitlines():
            try:
                row = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                rows.append(row)
    since = _parse_since(args.since) if args.since else 0.0
    rows = [
        row for row in rows
        if dt.datetime.fromisoformat(
            str(row.get("finished_utc", "1970-01-01T00:00:00Z")).replace("Z", "+00:00")
        ).timestamp() >= since
    ]
    aborted = [row for row in rows if row.get("verdict") == "aborted"]
    gates = [row for row in rows if row.get("kind") == "gate" and row.get("verdict") != "aborted"]
    reruns = [row for row in rows if row.get("kind") == "exclusive-rerun"]
    passed = sum(1 for row in gates if row.get("verdict") == "pass")
    partial = sum(1 for row in gates if row.get("verdict") == "partial")
    rate = f"{100.0 * (passed + partial) / len(gates):.1f}%" if gates else "n/a"
    failures: Counter[str] = Counter()
    for row in gates:
        steps = row.get("failed_steps")
        if isinstance(steps, list):
            failures.update(str(step) for step in steps)
    print(f"gate stats: {len(gates)} first-run gate(s), {passed} pass, {partial} partial, "
          f"{len(gates) - passed - partial} fail; first-run pass rate {rate} "
          f"({args.since or 'all time'}, ledger {path})")
    print(f"gate stats: {sum(1 for r in reruns if r.get('rerun_passed'))}/{len(reruns)} "
          "exclusive rerun(s) passed")
    if aborted:
        print(f"gate stats: {len(aborted)} run(s) aborted by a signal (not counted as first runs)")
    refused = sum(1 for row in rows if row.get("kind") == "preflight-refused")
    skipped = sum(1 for row in rows if row.get("preflight") == "skipped: --skip-preflight")
    if refused or skipped:
        print(f"gate stats: preflight refused {refused} run(s) before they queued; "
              f"{skipped} run(s) passed --skip-preflight")
    waits = sorted(float(w) for w in (row.get("wait_s") for row in rows)
                   if isinstance(w, (int, float)))
    if waits:
        print(f"gate stats: queue wait over {len(waits)} run(s): median "
              f"{waits[len(waits) // 2] / 60:.1f} min, max {waits[-1] / 60:.1f} min, "
              f"total {sum(waits) / 3600:.1f} h")
    for step, count in failures.most_common(10):
        print(f"gate stats: failing step {step}: {count}")
    return 0


# ------------------------------------------------- cargo target pruning (#940)
#
# Every slot keeps its own cargo targets, and they grow with every build: a test
# binary, rlib or incremental cache built for a configuration nothing builds any
# more stays on disk under its old hash forever. After each gate, prune what no
# current build can reuse, under cargo's own locks.
#
# The slot filesystems are mounted noatime, so a read leaves no trace, and cargo
# rewrites nothing for a fresh unit: a file's age says when it was BUILT, not
# whether it is still used. So the unit, not the file, is the grain, and use is
# read off cargo's fingerprint graph. Each unit has .fingerprint/<pkg>-<hash>/
# holding its fingerprint (16 hex digits, the u64 little-endian) and a JSON
# record whose "deps" name each dependency's fingerprint, which links the units
# into a graph. A unit nothing depends on is a root (a test binary, a binary, an
# example, a lone library). Roots that are the same target in the same
# configuration (target, profile and mode, features, compile kind, flags) and
# differ only in what they were linked against are builds of one target at
# different times, or alternatives two cargo commands need (one package seen
# through two feature unifications). The newest is kept. Older ones are kept too
# when built within the keep window of the newest with the same compiler, since
# two commands that both still run rebuild their variants within minutes of each
# other. Every unit a kept root reaches is kept, so a dependency built once and
# never touched since lives as long as something uses it, however old its files.
# Everything else goes: its deps/, build/, examples/ and .fingerprint/ entries.
# Units built in the last hour are always kept, whatever the graph says.
#
# Incremental caches never decide freshness (cargo does not consult them), so
# dropping one costs at most a slower next compile of that crate, never a
# rebuild of a fresh unit. Per crate name the newest is kept, and the others go
# once untouched for the keep window.
#
# Safety while cargo runs in the same slot: a target dir is pruned only while
# this process holds cargo's own locks on every profile dir in it, taken without
# waiting. A busy target dir is skipped and reported; a cargo starting meanwhile
# blocks for the few seconds the prune takes. A fingerprint record that does not
# parse skips its target dir rather than guess. A finished gate prunes only its
# own checkout's target dirs; `gate prune` only reports unless given --apply,
# and reaches another checkout only with --other-checkouts.

PRUNE_KEEP_HOURS = 24.0
PRUNE_RECENT_SECONDS = 3600.0
_CARGO_LOCKS = (".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock")
_UNIT_ENTRY = re.compile(r"-([0-9a-f]{16})(?:\.|$)")
_UNIT_DIRS = ("deps", "build", "examples", ".fingerprint")


def _is_target_root(path: Path) -> bool:
    return (path / ".rustc_info.json").is_file() or (path / "CACHEDIR.TAG").is_file()


def target_dirs(top: Path) -> dict[Path, list[Path]]:
    """Cargo target dirs at or below *top* (rs/target holds nested ones such as
    rs/target/validation-tests), each with its profile dirs. A profile dir holds
    .fingerprint and sits in its target dir, or one level down in a --target
    triple dir."""

    found: dict[Path, list[Path]] = {}
    top_depth = len(top.parts)
    for dirpath, dirs, _files in os.walk(top):
        here = Path(dirpath)
        if ".fingerprint" in dirs:
            parent = here.parent
            root = parent if _is_target_root(parent) or parent == top else parent.parent
            found.setdefault(root, []).append(here)
            dirs.clear()
        elif len(here.parts) - top_depth >= 4:
            dirs.clear()
    return {root: sorted(profiles) for root, profiles in sorted(found.items())}


def _tree_bytes(path: Path) -> int:
    total = 0
    for dirpath, _dirs, files in os.walk(path):
        for name in files:
            try:
                total += os.lstat(os.path.join(dirpath, name)).st_blocks * 512
            except OSError:
                pass
    return total


def _lock_profiles(profiles: list[Path]) -> list[int] | None:
    """Cargo's locks on every profile dir, all or nothing, never waiting."""

    held: list[int] = []
    for profile in profiles:
        for name in _CARGO_LOCKS:
            path = profile / name
            if not path.exists():
                continue
            try:
                fd = os.open(path, os.O_RDWR)
            except OSError:
                _unlock(held)
                return None
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                os.close(fd)
                _unlock(held)
                return None
            held.append(fd)
    return held


def _unlock(held: list[int]) -> None:
    for fd in held:
        os.close(fd)


@dataclasses.dataclass
class _Unit:
    profile: Path
    name: str
    mtime: float
    record: dict[str, object] | None = None
    stems: tuple[str, ...] = ()
    fingerprints: frozenset[int] = frozenset()


class _UnreadableFingerprint(Exception):
    pass


def _load_unit(profile: Path, unit_dir: Path) -> _Unit:
    records = sorted(unit_dir.glob("*.json"))
    if not records:
        # Cargo empties a unit's fingerprint when it starts rebuilding it, so an
        # empty one is a build that failed or was killed: cargo rebuilds it anyway.
        return _Unit(profile, unit_dir.name, unit_dir.stat().st_mtime)
    try:
        record = json.loads(records[0].read_text())
    except (OSError, ValueError) as exc:
        raise _UnreadableFingerprint(f"{records[0]}: {exc}") from exc
    if not isinstance(record, dict) or not isinstance(record.get("deps"), list):
        raise _UnreadableFingerprint(f"{records[0]}: no deps list")
    fingerprints: set[int] = set()
    for path in records:
        try:
            fingerprints.add(int.from_bytes(bytes.fromhex(path.with_suffix("").read_text().strip()), "little"))
        except (OSError, ValueError):
            pass
    return _Unit(profile, unit_dir.name, max(p.stat().st_mtime for p in records), record,
                 tuple(p.stem for p in records), frozenset(fingerprints))


def _dep_fingerprints(unit: _Unit) -> list[int]:
    assert unit.record is not None
    deps = unit.record["deps"]
    assert isinstance(deps, list)
    try:
        return [int(dep[3]) for dep in deps]
    except (TypeError, ValueError, IndexError) as exc:
        raise _UnreadableFingerprint(f"{unit.profile / '.fingerprint' / unit.name}: deps {exc}") from exc


def _dead_units(units: list[_Unit], now: float, keep_s: float) -> list[_Unit]:
    by_fingerprint: dict[int, list[int]] = {}
    for index, unit in enumerate(units):
        for value in unit.fingerprints:
            by_fingerprint.setdefault(value, []).append(index)
    edges: dict[int, set[int]] = {}
    depended_on: set[int] = set()
    for index, unit in enumerate(units):
        if unit.record is None:
            continue
        targets = {t for value in _dep_fingerprints(unit) for t in by_fingerprint.get(value, [])}
        edges[index] = targets
        depended_on |= targets
    groups: dict[tuple[object, ...], list[int]] = {}
    for index, unit in enumerate(units):
        if unit.record is None or index in depended_on:
            continue
        r = unit.record
        key = (unit.profile, unit.stems, r.get("features"), r.get("target"), r.get("profile"),
               r.get("compile_kind"), json.dumps(r.get("rustflags")), r.get("config"))
        groups.setdefault(key, []).append(index)
    keep = [i for i, unit in enumerate(units) if now - unit.mtime < PRUNE_RECENT_SECONDS]
    for members in groups.values():
        newest = units[max(members, key=lambda i: units[i].mtime)]
        assert newest.record is not None
        for i in members:
            unit = units[i]
            assert unit.record is not None
            if (unit is newest or (unit.record.get("rustc") == newest.record.get("rustc")
                                   and newest.mtime - unit.mtime <= keep_s)):
                keep.append(i)
    live: set[int] = set()
    while keep:
        index = keep.pop()
        if index not in live:
            live.add(index)
            keep.extend(edges.get(index, ()))
    return [unit for i, unit in enumerate(units) if i not in live]


def _stale_incremental(profiles: list[Path], now: float, keep_s: float) -> list[Path]:
    stale: list[Path] = []
    for profile in profiles:
        crates: dict[str, list[tuple[float, Path]]] = {}
        incremental = profile / "incremental"
        if not incremental.is_dir():
            continue
        for path in incremental.iterdir():
            if path.is_dir() and not path.is_symlink():
                crates.setdefault(path.name.rsplit("-", 1)[0], []).append((path.stat().st_mtime, path))
        for dirs in crates.values():
            dirs.sort()
            stale += [path for mtime, path in dirs[:-1] if now - mtime > keep_s]
    return stale


@dataclasses.dataclass
class PruneResult:
    root: Path
    units: int = 0
    incremental: int = 0
    freed: int = 0
    skipped: str | None = None


def _remove(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path, ignore_errors=True)
    else:
        path.unlink(missing_ok=True)


def prune_target_root(root: Path, profiles: list[Path], *, now: float | None = None,
                      keep_hours: float = PRUNE_KEEP_HOURS, dry_run: bool = False) -> PruneResult:
    """Prune one cargo target dir under cargo's locks; see the section comment."""

    now = time.time() if now is None else now
    keep_s = keep_hours * 3600.0
    result = PruneResult(root)
    held = [] if dry_run else _lock_profiles(profiles)
    if held is None:
        result.skipped = "cargo is using it"
        return result
    try:
        try:
            units = [_load_unit(p, d) for p in profiles for d in (p / ".fingerprint").iterdir() if d.is_dir()]
            dead = _dead_units(units, now, keep_s)
        except _UnreadableFingerprint as exc:
            result.skipped = f"unreadable fingerprint {exc}"
            return result
        doomed: list[Path] = []
        for profile in profiles:
            hashes = {u.name.rsplit("-", 1)[-1] for u in dead if u.profile == profile}
            if not hashes:
                continue
            for sub in _UNIT_DIRS:
                folder = profile / sub
                if folder.is_dir():
                    for entry in folder.iterdir():
                        match = _UNIT_ENTRY.search(entry.name)
                        if match and match.group(1) in hashes:
                            doomed.append(entry)
        incremental = _stale_incremental(profiles, now, keep_s)
        result.units, result.incremental = len(dead), len(incremental)
        for path in doomed + incremental:
            result.freed += _tree_bytes(path) if path.is_dir() and not path.is_symlink() else path.lstat().st_blocks * 512
            if not dry_run:
                _remove(path)
    finally:
        _unlock(held)
    return result


def default_prune_dirs() -> list[Path]:
    """This checkout's own cargo target dirs, and never another's;
    $GATE_PRUNE_TARGETS=off disables the prune."""

    if os.environ.get("GATE_PRUNE_TARGETS", "").strip().lower() in {"0", "off", "no", "false"}:
        return []
    repo = Path(__file__).resolve().parent.parent
    return [repo / "rs" / "target", repo / "vibe-talk" / "target"]


def host_allocation(path: Path) -> tuple[Path, int] | None:
    """The host file backing *path*'s file system and the bytes the host has
    allocated to it, when that file system is a loop-mounted image (a slot);
    else None. Bytes freed inside an image reach the host only once they are
    discarded or trimmed, so the target dir's own size can fall while the host
    gains nothing.

    The kernel names the backing file by the path it was attached with, which
    can differ from this process's view of the same file (another mount
    namespace); a path ending in the same components below one of *path*'s
    ancestors, of exactly the device's size, is taken as that file."""

    try:
        device = os.stat(path).st_dev
        sysfs = Path("/sys/dev/block") / f"{os.major(device)}:{os.minor(device)}"
        backing = Path((sysfs / "loop" / "backing_file").read_text().strip())
        size = int((sysfs / "size").read_text().strip()) * 512
    except (OSError, ValueError):
        return None
    tail = backing.parts[1:]
    candidates = [backing] + [ancestor.joinpath(*tail[k:])
                              for ancestor in path.resolve().parents for k in range(1, len(tail))]
    for candidate in candidates:
        try:
            st = candidate.stat()
        except OSError:
            continue
        if stat.S_ISREG(st.st_mode) and st.st_size == size:
            return candidate, st.st_blocks * 512
    return None


def _gib(n: int) -> str:
    return f"{n / 2**30:.2f} GiB"


def _host_allocations(tops: list[Path]) -> dict[Path, int]:
    found: dict[Path, int] = {}
    for top in tops:
        allocation = host_allocation(top) if top.is_dir() else None
        if allocation is not None:
            found[allocation[0]] = allocation[1]
    return found


def _settled_host_allocations(tops: list[Path], *, limit_s: float = 15.0) -> dict[Path, int]:
    """Host allocations once they stop falling. An image's online discards
    complete a few seconds after the journal commits the frees (on one slot, 4
    GiB came back within 10 s, and about half of it 0.3 s after a sync), so
    read once a second until two readings agree, for at most *limit_s*."""

    deadline = time.monotonic() + limit_s
    current = _host_allocations(tops)
    while time.monotonic() < deadline:
        time.sleep(1.0)
        latest = _host_allocations(tops)
        if latest == current:
            break
        current = latest
    return current


def _trim(prefix: str) -> int | None:
    """Run $GATE_TRIM_COMMAND, if set, to hand freed blocks back to the host
    (an image mounted without online discard keeps them until trimmed), and
    return its exit status. Its output is reported; it never fails a gate."""

    command = os.environ.get("GATE_TRIM_COMMAND", "").strip()
    if not command:
        return None
    try:
        done = subprocess.run(shlex.split(command), capture_output=True, text=True, timeout=600)
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        print(f"{prefix}: trim command failed: {exc}", flush=True)
        return -1
    for line in (done.stdout + done.stderr).splitlines():
        print(f"{prefix}: trim: {line}", flush=True)
    print(f"{prefix}: trim command exited {done.returncode}", flush=True)
    return done.returncode


def prune_targets(tops: list[Path], *, keep_hours: float = PRUNE_KEEP_HOURS, dry_run: bool = False,
                  prefix: str = "gate") -> dict[str, int | None]:
    """Prune every target dir under *tops*, print one line per top and one per
    host image behind them, and return the bytes before and after: under the
    target dirs, and allocated on the host to their images (None when they are
    not on a loop-mounted image)."""

    hosts_before = _host_allocations(tops)
    before_total = after_total = removed = 0
    for top in tops:
        if not top.is_dir():
            continue
        before = _tree_bytes(top)
        results = [prune_target_root(root, profiles, keep_hours=keep_hours, dry_run=dry_run)
                   for root, profiles in target_dirs(top).items()]
        freed = sum(r.freed for r in results)
        after = before - freed if dry_run else _tree_bytes(top)
        before_total += before
        after_total += after
        removed += sum(r.units + r.incremental for r in results)
        verb = "would prune" if dry_run else "pruned"
        line = (f"{prefix}: cargo target {top}: {_gib(before)} -> {_gib(after)} ({verb} "
                f"{sum(r.units for r in results)} stale unit(s) and "
                f"{sum(r.incremental for r in results)} incremental cache(s), {_gib(freed)})")
        skipped = [f"{r.root.relative_to(top) if r.root != top else '.'}: {r.skipped}" for r in results if r.skipped]
        if skipped:
            line += "; skipped " + "; ".join(skipped)
        print(line, flush=True)
    hosts_after = hosts_before
    trim_exit = None
    if not dry_run and removed:
        os.sync()  # online discard happens as the journal commits the frees
        trim_exit = _trim(prefix)
        hosts_after = _settled_host_allocations(tops) if hosts_before else {}
    for image, allocated in hosts_before.items():
        print(f"{prefix}: host allocation of {image}: {_gib(allocated)} -> "
              f"{_gib(hosts_after.get(image, allocated))}", flush=True)
    return {
        "target_bytes_before": before_total,
        "target_bytes_after": after_total,
        "host_bytes_before": sum(hosts_before.values()) if hosts_before else None,
        "host_bytes_after": sum(hosts_after.values()) if hosts_after else None,
        "trim_exit": trim_exit,
    }


def _prune_after_gate() -> dict[str, int | None] | None:
    """Best effort: a prune failure must never change the gate's own result."""

    try:
        tops = default_prune_dirs()
        if not tops:
            return None
        return prune_targets(tops)
    except Exception as exc:
        print(f"gate: cargo target prune failed: {exc}", flush=True)
        return None


def run_prune(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="gate prune",
        description=(
            "Prune cargo build artifacts no current build can reuse: older builds of a target, "
            "and everything only they used, plus idle incremental caches. A gate prunes its own "
            "checkout's target dirs when it finishes; this command reports, and with --apply "
            "removes, by hand. Target dirs cargo is using are skipped. For target dirs on a "
            "loop-mounted image it also reports the host's allocation to that image before and "
            "after; with --apply, $GATE_TRIM_COMMAND (if set) runs once anything was freed."
        ),
        epilog=(
            "Examples:\n"
            "  scripts/gate.py prune                  # report for this checkout\n"
            "  scripts/gate.py prune --apply          # prune this checkout\n"
            "  scripts/gate.py prune --other-checkouts ../other/rs/target   # report for another"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("dirs", nargs="*", type=Path,
                        help="cargo target dirs, nested ones included "
                             "(default: this checkout's rs/target and vibe-talk/target)")
    parser.add_argument("--apply", action="store_true",
                        help="remove what the report lists (default: report only, taking no locks)")
    parser.add_argument("--other-checkouts", action="store_true",
                        help="allow dirs outside this checkout, such as another slot's (refused without it)")
    parser.add_argument("--keep-hours", type=float, default=PRUNE_KEEP_HOURS,
                        help=f"keep an older build of a target built within this many hours of the newest, "
                             f"and incremental caches used within it (default {PRUNE_KEEP_HOURS:g})")
    args = parser.parse_args(argv)
    repo = Path(__file__).resolve().parent.parent
    tops = [d.resolve() for d in args.dirs] or [repo / "rs" / "target", repo / "vibe-talk" / "target"]
    outside = [d for d in tops if not d.is_relative_to(repo)]
    if outside and not args.other_checkouts:
        parser.error(f"outside this checkout ({repo}): {', '.join(map(str, outside))}; "
                     "pass --other-checkouts to include them")
    prune_targets(tops, keep_hours=args.keep_hours, dry_run=not args.apply, prefix="gate prune")
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(
        prog="gate",
        description="Gate admission: counting slots plus an exclusive class.",
        epilog=(
            "Subcommands: gate status, gate stats, gate queue [--explain] (why each "
            "queued ticket waits, and the phase-priority switch state), gate prune. "
            "Run each as `gate <name> --help`.\n\n"
            "Blackout windows (#907): no gate starts inside a window from the per-host "
            "config (default ~/.local/etc/agent-utils/gate.toml, override with "
            "$GATE_BLACKOUT_CONFIG or --blackout-config). Format, one or more tables:\n"
            '  [[blackout]]\n  start = "00:15"\n  end = "04:00"\n'
            '  timezone = "America/New_York"\n  reason = "neighbour nightly job"\n'
            "An end at or before the start crosses midnight. With no file there is no "
            "blackout; a malformed file refuses every start (exit 2). A start inside a "
            "window queues and starts when the window ends (--no-wait-blackout refuses "
            "with exit 78 instead, naming the next allowed start); a ticket queued "
            "before a window waits it out too. --ignore-blackout overrides a window "
            "and is recorded in the ledger. $GATE_BLACKOUT_NOW (ISO 8601) overrides "
            "the clock for tests."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--slots", type=int, default=int(os.environ.get("GATE_SLOTS", "2")),
                        help="concurrent shared gates (default 2, or $GATE_SLOTS); a host cap in "
                             "~/.local/etc/agent-utils/gate-slots.toml (`slots = N`, or the file "
                             "$GATE_SLOTS_CONFIG names) lowers it and is re-read on every poll")
    parser.add_argument("--exclusive", dest="cls", action="store_const", const="exclusive", default="shared",
                        help="run alone: wait for an empty box and block new admissions")
    parser.add_argument("--wait-timeout", type=float, default=0.0, help="give up queueing after N seconds (0 = wait forever)")
    parser.add_argument("--blackout-config", default=None,
                        help="blackout config path (default $GATE_BLACKOUT_CONFIG or ~/.local/etc/agent-utils/gate.toml)")
    parser.add_argument("--ignore-blackout", action="store_true",
                        help="start even inside a blackout window; the override is recorded in the ledger")
    parser.add_argument("--no-wait-blackout", action="store_true",
                        help="refuse a start inside a blackout window (exit 78) instead of queueing until it ends")
    parser.add_argument("--rerun", action="store_true",
                        help=f"a short solo rerun of at most {RERUN_MAX_STEPS} already-run steps named with "
                             "--selected: runs exclusive and sorts ahead of every ordinary queued ticket "
                             "(FIFO among reruns); it still waits for running gates. "
                             "Example: gate.py --rerun -- common/bin/dagrun run --dag validation.dag.yaml "
                             "--keep-going --selected vibe-talk.check.fold-latency-browser")
    parser.add_argument("--skip-preflight", action="store_true",
                        help="queue without the preflight. Before queueing, a top-level run checks "
                             "this checkout with the validation steps " + " and ".join(PREFLIGHT_STEPS)
                             + " (no consuming-project names; standalone package docs and complete "
                             "test censuses), ~20 s, and refuses to queue (exit "
                             f"{PREFLIGHT_EXIT}) if either fails, printing the failing lines. A nested "
                             "gate and --rerun skip it. The skip is recorded in the ledger")
    parser.add_argument("--ledger", default=None,
                        help="durable ledger path (default ~/.local/state/agent-utils/gate-ledger.jsonl, or $GATE_LEDGER)")
    parser.add_argument("--no-prune", action="store_true",
                        help="skip the cargo target prune a finished run does (also $GATE_PRUNE_TARGETS=off; "
                             "see `gate prune --help`)")
    if argv[:1] == ["status"]:
        return show_status(parser.parse_args([]))
    if argv[:1] == ["queue"]:
        queue_parser = argparse.ArgumentParser(
            prog="gate queue",
            description=(
                "List running runs and queued tickets in grant order. The header "
                "shows the phase-priority switch (#1146): on/off and what decided "
                "it ($GATE_PHASE_PRIORITY=1/0, else `phase_priority = true` in the "
                "per-host file $GATE_PHASE_PRIORITY_CONFIG or "
                "~/.local/etc/agent-utils/gate-phase-priority.toml; default off). "
                "An older gate.py reads the same tickets: it sorts by the same "
                "`rerun` field and ignores the phase fields."),
            epilog=(
                "Examples:\n"
                "  scripts/gate.py queue             # grant order, marks and ages\n"
                "  scripts/gate.py queue --explain   # why each ticket waits"),
            formatter_class=argparse.RawDescriptionHelpFormatter,
        )
        queue_parser.add_argument("--explain", action="store_true",
                                  help="for each queued ticket, name what blocks it: the running "
                                       "runs it waits for, the tickets ahead of it and why each is "
                                       "ahead (rerun tier or FIFO), a full shared box, or an active "
                                       "blackout; an unmarked exclusive-phase ticket also gets a "
                                       "starvation line counting reruns queued after it that stand "
                                       "ahead of it")
        queue_parser.add_argument("--slots", type=int,
                                  default=int(os.environ.get("GATE_SLOTS", "2")),
                                  help="concurrent shared gates for the full-box check "
                                       "(default 2, or $GATE_SLOTS); a host cap lowers it")
        queue_parser.add_argument("--blackout-config", default=None,
                                  help="blackout config path (default $GATE_BLACKOUT_CONFIG or "
                                       "~/.local/etc/agent-utils/gate.toml)")
        return show_queue(queue_parser.parse_args(argv[1:]))
    if argv[:1] == ["prune"]:
        return run_prune(argv[1:])
    if argv[:1] == ["stats"]:
        stats_parser = argparse.ArgumentParser(prog="gate stats")
        stats_parser.add_argument("--since", default=None,
                                  help="window: Nh, Nd, or an ISO date/time (default all time)")
        stats_parser.add_argument("--ledger", default=None, help="ledger path (see --ledger)")
        return show_stats(stats_parser.parse_args(argv[1:]))
    if "--" in argv:
        cut = argv.index("--")
        args = parser.parse_args(argv[:cut])
        command = argv[cut + 1:]
    else:
        # No explicit command: gate flags are consumed here, the rest belong
        # to validate.py (the drop-in replacement for the flock line).
        args, rest = parser.parse_known_args(argv)
        command = [sys.executable, str(Path(__file__).with_name("validate.py")), *rest]
    if not command:
        parser.error("no command to run")
    return run_admitted(args, command)


if __name__ == "__main__":
    raise SystemExit(main())
