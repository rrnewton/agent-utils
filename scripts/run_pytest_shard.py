#!/usr/bin/env python3
"""Run one deterministic, family-preserving shard of a pytest selection.

Every shard performs the same collection and partitions the resulting node IDs. Parameterized
cases from one test function stay together, which avoids turning one logical mutation matrix into
concurrent processes. Families are assigned largest-first to the currently lightest shard; ties
are stable by family name and shard index. The weight is the family's recorded duration from
``scripts/pytest_shard_weights.json`` when that file knows the family (see
``scripts/case_timings.py weights``), and its case count scaled by the median recorded
seconds-per-case otherwise -- case count alone when no weights are recorded at all.

Examples:
    cd py
    python3 ../scripts/run_pytest_shard.py --shard 0/6 -- \
        -q -c pyproject.toml --rootdir=. wrkslots/tests/test_lifecycle.py \
        -m 'not ordinary_environment'

    python3 scripts/run_pytest_shard.py --self-test
"""

from __future__ import annotations

import argparse
import faulthandler
import fcntl
import hashlib
import json
import math
import os
import signal
import shutil
import stat
import statistics
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import FrameType

import pytest


_CANCELLATION_SIGNALS = (signal.SIGHUP, signal.SIGINT, signal.SIGTERM)
#: Each shard's private pytest basetemp: ``<tmp>/agent-utils-pytest-<pid>-<shard>-<random>``,
#: with the wrapper's owner lock beside it in ``<basetemp>.owner`` (not inside: pytest empties
#: its basetemp when it starts).
_TEMP_PREFIX = "agent-utils-pytest-"
_OWNER_SUFFIX = ".owner"
#: Keep a failed shard's basetemp for inspection when this is set to 1; it is removed otherwise.
KEEP_ENV = "AGENT_UTILS_KEEP_PYTEST_BASETEMP"
#: How long an unowned basetemp from an older wrapper, or a kept one, survives the sweep.
_STALE_SECONDS = 24 * 3600
_POLL_SECONDS = 0.01
_TERM_GRACE_SECONDS = 1.0
#: Default bound for one test (setup + call + teardown) in every Python shard. The
#: slowest recorded legitimate case is ~275 s; 600 s bounds a hang to one gate slot
#: for minutes instead of hours (#1189). Override per test with
#: ``@pytest.mark.timeout(seconds)`` (0 disables) or per run with --test-timeout.
_DEFAULT_TEST_TIMEOUT_SECONDS = 600.0
_TEST_TIMEOUT_ENV = "AGENT_UTILS_PYTEST_TEST_TIMEOUT"
_SignalHandler = Callable[[int, FrameType | None], object] | int | None


def _family(nodeid: str) -> str:
    """Return the stable test-function identity shared by parameterized cases."""
    return nodeid.split("[", 1)[0]


#: The checked-in family durations the shard assignment balances by, written by
#: ``scripts/case_timings.py weights`` from recorded per-case timings.
WEIGHTS_SCHEMA = "pytest-shard-weights/v1"


def _weights_path() -> Path:
    return Path(__file__).resolve().parent / "pytest_shard_weights.json"


def _load_weights(path: Path | None = None) -> dict[str, float]:
    """Recorded family durations in seconds; empty when none were recorded.

    A present but unreadable or malformed file is a hard error rather than a
    silent fall back to case counts: the balance it was checked in to provide
    would be lost without anyone noticing.
    """
    target = _weights_path() if path is None else path
    if not target.is_file():
        return {}
    try:
        payload = json.loads(target.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"shard weights {target} are unreadable: {error}") from error
    if not isinstance(payload, dict) or payload.get("schema") != WEIGHTS_SCHEMA:
        raise ValueError(f"shard weights {target}: expected schema {WEIGHTS_SCHEMA!r}")
    families = payload.get("families")
    if not isinstance(families, dict):
        raise ValueError(f"shard weights {target}: families must be an object")
    weights: dict[str, float] = {}
    for family, seconds in families.items():
        if (
            not isinstance(family, str)
            or isinstance(seconds, bool)
            or not isinstance(seconds, (int, float))
            or not math.isfinite(seconds)
            or seconds < 0
        ):
            raise ValueError(f"shard weights {target}: bad duration for {family!r}")
        weights[family] = float(seconds)
    return weights


def _assign(
    nodeids: Sequence[str], count: int, weights: dict[str, float] | None = None
) -> tuple[tuple[str, ...], ...]:
    """Partition node IDs deterministically while keeping whole families together."""
    if count < 1:
        raise ValueError("shard count must be positive")

    families: dict[str, list[str]] = {}
    for nodeid in nodeids:
        families.setdefault(_family(nodeid), []).append(nodeid)

    recorded = weights or {}
    known_rates = [
        recorded[family] / len(members)
        for family, members in families.items()
        if family in recorded
    ]
    default_rate = statistics.median(known_rates) if known_rates else 1.0

    def weight(family: str, members: list[str]) -> float:
        if family in recorded:
            return recorded[family]
        return len(members) * default_rate

    loads = [0.0] * count
    shards: list[list[str]] = [[] for _ in range(count)]
    for family, members in sorted(
        families.items(), key=lambda item: (-weight(item[0], item[1]), item[0])
    ):
        target = min(range(count), key=lambda index: (loads[index], index))
        shards[target].extend(sorted(members))
        loads[target] += weight(family, members)
    return tuple(tuple(sorted(shard)) for shard in shards)


class _TimingPlugin:
    """Collect per-case timings for ``scripts/case_timings.py`` when asked.

    Registered by ``pytest_configure`` below whenever a timing directory is
    requested, whether this module drives the run as the shard worker or is
    loaded with ``-p`` by ``scripts/run_component_tests.py``. The inventory
    is the post-shard collection; durations sum each case's stage reports.
    """

    def __init__(self) -> None:
        import case_timings

        self.inventory: list[str] = []
        self.results: dict[str, tuple[str, float]] = {}
        # Captured before the session starts: below, sessionstart marks this
        # process nested so any timing-capable runner a test spawns emits as
        # an alternate-environment rerun instead of a second primary record.
        self.nested = case_timings.is_nested()

    def pytest_sessionstart(self, session: pytest.Session) -> None:
        del session
        import case_timings

        os.environ[case_timings.TIMING_NESTED_ENV] = "1"

    def pytest_collection_finish(self, session: pytest.Session) -> None:
        self.inventory = [item.nodeid for item in session.items]

    def pytest_runtest_logreport(self, report: pytest.TestReport) -> None:
        outcome, duration = self.results.get(report.nodeid, ("", 0.0))
        duration += max(0.0, report.duration)
        if report.failed:
            outcome = "fail" if report.when == "call" else "error"
        elif report.skipped:
            if outcome not in {"fail", "error", "pass"}:
                outcome = "skip"
        elif report.passed and report.when == "call" and outcome not in {"fail", "error"}:
            outcome = "pass"
        self.results[report.nodeid] = (outcome, duration)

    def pytest_sessionfinish(self, session: pytest.Session, exitstatus: int) -> None:
        del session, exitstatus
        import case_timings

        gate = case_timings.current_gate()
        cases = [
            case_timings.CaseRecord("pytest", gate, nodeid, outcome or "error", duration)
            for nodeid, (outcome, duration) in sorted(self.results.items())
        ]
        case_timings.emit_if_requested(
            runner="pytest",
            inventory=self.inventory,
            cases=cases,
            gate=gate,
            nested=self.nested,
        )


#: A run under a memory cap says once, naming the test just finished, when the capped cgroup's
#: memory first passes this fraction of its memory.max (#1203).
_MEMORY_WARN_FRACTION = 0.8
_MIB = 1024 * 1024


def _capped_memory_cgroup(
    proc_cgroup: Path = Path("/proc/self/cgroup"), root: Path = Path("/sys/fs/cgroup")
) -> tuple[Path, int] | None:
    """The nearest cgroup-v2 directory above this process with a finite memory.max, and that cap.

    Under dagrun that is the step's inner cgroup, whose cap an OOM kill enforces; elsewhere there
    is usually none, and nothing is reported.
    """
    try:
        lines = proc_cgroup.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    relative = next((line[3:] for line in lines if line.startswith("0::")), None)
    if relative is None:
        return None
    directory = root / relative.lstrip("/")
    while directory != root and root in directory.parents:
        try:
            raw = (directory / "memory.max").read_text(encoding="utf-8").strip()
        except OSError:
            return None
        if raw.isdecimal():
            return directory, int(raw)
        directory = directory.parent
    return None


def _cgroup_bytes(directory: Path, name: str) -> int | None:
    try:
        raw = (directory / name).read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return int(raw) if raw.isdecimal() else None


@dataclass(eq=False)
class _MemoryPeakPlugin:
    """Say how close a capped run came to its memory cap (#1203 cross-skip-accounting-memory-cap).

    dagrun reports an OOM kill at a step's inner cap as peak = cap, which says nothing about the
    step's real need. So under a finite memory.max this prints, on stderr, one line when the
    capped cgroup first passes 80% of the cap (flushed at once, so it survives a later kill and
    names the test that got there) and the cgroup's memory.peak at the end of the session.
    """

    capped: tuple[Path, int] | None = None
    warned: bool = False

    def pytest_sessionstart(self, session: pytest.Session) -> None:
        del session
        self.capped = _capped_memory_cgroup()

    def pytest_runtest_logreport(self, report: pytest.TestReport) -> None:
        if self.capped is None or self.warned or report.when != "teardown":
            return
        directory, cap = self.capped
        current = _cgroup_bytes(directory, "memory.current")
        if current is not None and current >= cap * _MEMORY_WARN_FRACTION:
            self.warned = True
            # The leading newline ends pytest's unterminated progress line.
            print(
                f"\nrun_pytest_shard: memory {current / _MIB:.1f} MiB of the "
                f"{cap / _MIB:.1f} MiB cap after {report.nodeid}",
                file=sys.stderr,
                flush=True,
            )

    def pytest_sessionfinish(self, session: pytest.Session, exitstatus: int) -> None:
        del session, exitstatus
        if self.capped is None:
            return
        directory, cap = self.capped
        peak = _cgroup_bytes(directory, "memory.peak")
        if peak is not None:
            print(
                f"\nrun_pytest_shard: memory peak {peak / _MIB:.1f} MiB of the "
                f"{cap / _MIB:.1f} MiB cap",
                file=sys.stderr,
                flush=True,
            )


class _PerTestTimeout(AssertionError):
    """One test exceeded its per-test bound; the traceback names where it hung."""


def _env_test_timeout() -> float:
    raw = os.environ.get(_TEST_TIMEOUT_ENV, "")
    if not raw:
        return _DEFAULT_TEST_TIMEOUT_SECONDS
    try:
        value = float(raw)
    except ValueError as error:
        raise ValueError(f"{_TEST_TIMEOUT_ENV} must be a number of seconds: {raw!r}") from error
    if value < 0 or value != value:
        raise ValueError(f"{_TEST_TIMEOUT_ENV} must be >= 0 seconds: {raw!r}")
    return value


# eq=False for the same plugin-identity reason as _ShardPlugin below.
@dataclass(eq=False)
class _TestTimeoutPlugin:
    """Fail one test that runs past its bound instead of holding a gate slot.

    SIGALRM on the main thread interrupts whatever the test is blocked in
    (a pipe read, a sleep, a lock) and raises _PerTestTimeout with a stack
    dump, so the failure names the hang instead of silently eating the shard.
    Fixture/context-manager teardown then runs normally, which is what kills
    children a well-behaved test spawned. Tests on non-main threads and
    platforms without setitimer simply run unbounded rather than failing
    spuriously.
    """

    default_seconds: float = _DEFAULT_TEST_TIMEOUT_SECONDS

    def pytest_configure(self, config: pytest.Config) -> None:
        config.addinivalue_line(
            "markers",
            "timeout(seconds): per-test timeout override for run_pytest_shard (0 disables)",
        )

    def _limit_for(self, item: pytest.Item) -> float:
        marker = item.get_closest_marker("timeout")
        if marker is not None and marker.args:
            return float(marker.args[0])
        return self.default_seconds

    @pytest.hookimpl(wrapper=True)
    def pytest_runtest_protocol(
        self, item: pytest.Item, nextitem: pytest.Item | None
    ) -> object:
        del nextitem
        limit = self._limit_for(item)
        if limit <= 0 or not hasattr(signal, "setitimer"):
            return (yield)
        import threading

        if threading.current_thread() is not threading.main_thread():
            return (yield)

        def _expire(signum: int, frame: FrameType | None) -> None:
            del signum, frame
            faulthandler.dump_traceback(file=sys.stderr)
            raise _PerTestTimeout(
                f"{item.nodeid} exceeded its per-test timeout of {limit:g}s "
                "(run_pytest_shard default; mark @pytest.mark.timeout(seconds) to override)"
            )

        previous_handler = signal.signal(signal.SIGALRM, _expire)
        previous_timer = signal.setitimer(signal.ITIMER_REAL, limit)
        try:
            return (yield)
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, previous_handler)
            if previous_timer[0] > 0:
                signal.setitimer(signal.ITIMER_REAL, previous_timer[0], previous_timer[1])


def _isolate_agentctl_registry() -> None:
    """Default agentctl tests to a scratch registry (#1499 tests-touch-live-registry).

    Direct ``-p run_pytest_shard`` runs have no wrapper to set the guarded
    environment, so the plugin sets the same defaults at configure time. A
    registry the caller or a test already chose is kept.
    """
    if not os.environ.get("AGENTCTL_REGISTRY"):
        os.environ["AGENTCTL_REGISTRY"] = tempfile.mkdtemp(prefix="agent-utils-agentctl-registry-")
    os.environ.setdefault("AGENTCTL_LAYOUT_AUTO_MIGRATE", "0")


def pytest_configure(config: pytest.Config) -> None:
    """Register the timing collector, the per-test timeout and the memory report once each."""
    _isolate_agentctl_registry()

    config.addinivalue_line(
        "markers",
        "timeout(seconds): per-test timeout override for run_pytest_shard (0 disables)",
    )
    if not config.pluginmanager.hasplugin("per-test-timeout"):
        config.pluginmanager.register(
            _TestTimeoutPlugin(_env_test_timeout()), name="per-test-timeout"
        )
    if not config.pluginmanager.hasplugin("memory-peak"):
        config.pluginmanager.register(_MemoryPeakPlugin(), name="memory-peak")
    import case_timings

    if case_timings.requested_directory() is None:
        return
    if config.pluginmanager.hasplugin("case-timings"):
        return
    config.pluginmanager.register(_TimingPlugin(), name="case-timings")


# eq=False keeps object identity for equality and hashing: pytest may hold its plugins in a set.
@dataclass(eq=False)
class _ShardPlugin:
    index: int
    count: int
    # The shard worker is this module run as a script, so the module-level
    # ``pytest_configure`` above never fires in it; the timing collector rides
    # on the shard plugin itself instead. Non-shard runs load this module
    # with ``-p`` and take the ``pytest_configure`` path, so exactly one
    # collector is active per launch mode.
    timing: _TimingPlugin | None = None

    def __post_init__(self) -> None:
        import case_timings

        if case_timings.requested_directory() is not None:
            self.timing = _TimingPlugin()

    def pytest_sessionstart(self, session: pytest.Session) -> None:
        _isolate_agentctl_registry()
        if self.timing is not None:
            self.timing.pytest_sessionstart(session)

    def pytest_collection_finish(self, session: pytest.Session) -> None:
        if self.timing is not None:
            self.timing.pytest_collection_finish(session)

    def pytest_runtest_logreport(self, report: pytest.TestReport) -> None:
        if self.timing is not None:
            self.timing.pytest_runtest_logreport(report)

    def pytest_sessionfinish(self, session: pytest.Session, exitstatus: int) -> None:
        if self.timing is not None:
            self.timing.pytest_sessionfinish(session, exitstatus)

    def pytest_collection_modifyitems(
        self,
        session: pytest.Session,
        config: pytest.Config,
        items: list[pytest.Item],
    ) -> None:
        """Keep only this shard and report all other items as deselected."""
        del session
        assignments = _assign([item.nodeid for item in items], self.count, _load_weights())
        selected_ids = set(assignments[self.index])
        selected = [item for item in items if item.nodeid in selected_ids]
        deselected = [item for item in items if item.nodeid not in selected_ids]
        # xunit2 does not otherwise retain pytest's canonical node ID. Recording both identities
        # makes later timing files lossless and removes the need to reconstruct paths from class
        # names when rebalancing a future run.
        for item in selected:
            item.user_properties.append(("nodeid", item.nodeid))
            item.user_properties.append(("family", _family(item.nodeid)))
        if deselected:
            config.hook.pytest_deselected(items=deselected)
        items[:] = selected
        digest = hashlib.sha256("\n".join(sorted(selected_ids)).encode()).hexdigest()[:12]
        print(
            f"pytest-shard: {self.index + 1}/{self.count}, "
            f"selected={len(selected)}, digest={digest}",
            flush=True,
        )


def _parse_shard(value: str) -> tuple[int, int]:
    try:
        raw_index, raw_count = value.split("/", 1)
        index, count = int(raw_index), int(raw_count)
    except (ValueError, TypeError) as error:
        raise argparse.ArgumentTypeError("shard must have the form INDEX/COUNT") from error
    if count < 1 or index < 0 or index >= count:
        raise argparse.ArgumentTypeError("shard requires COUNT > 0 and 0 <= INDEX < COUNT")
    return index, count


def _restore_owner_access(path: Path) -> None:
    """Make a test-owned tree removable without following its symlinks."""

    if path.is_symlink() or not path.exists():
        return
    path.chmod(path.stat().st_mode | stat.S_IRWXU)
    if path.is_dir():
        for child in path.iterdir():
            _restore_owner_access(child)


def _remove_temp_root(path: Path) -> None:
    """Remove pytest state even when a permissions test made part of it inaccessible."""

    if not path.exists():
        return
    try:
        shutil.rmtree(path)
    except OSError:
        _restore_owner_access(path)
        shutil.rmtree(path)


def _mounts_under(path: Path, mountinfo: Path = Path("/proc/self/mountinfo")) -> list[str]:
    """Mount points inside ``path``: a test killed mid-way can leave a slot image mounted there.

    Removing such a tree would delete the files inside the mounted image and then fail on the
    mount point, so it is left for ``scripts/sweep_test_mounts.py``, which releases the mount.
    """

    prefixes = {f"{os.path.abspath(path)}/", f"{os.path.realpath(path)}/"}
    try:
        lines = mountinfo.read_text(encoding="utf-8", errors="surrogateescape").splitlines()
    except OSError:
        return []
    points = []
    for line in lines:
        fields = line.split()
        if len(fields) > 4:
            point = fields[4].replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
            if any(point.startswith(prefix) for prefix in prefixes):
                points.append(point)
    return points


def _hold_owner_lock(temp_root: Path) -> int:
    """Lock ``<temp_root>.owner`` for as long as this wrapper lives; return its descriptor."""

    descriptor = os.open(
        f"{temp_root}{_OWNER_SUFFIX}", os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o600
    )
    fcntl.flock(descriptor, fcntl.LOCK_EX)
    return descriptor


def _sweep_stale_temp_roots(directory: Path, now: float | None = None) -> list[Path]:
    """Remove basetemps whose wrapper is gone; return what was removed.

    A wrapper that is killed outright (SIGKILL, as a cancelled or timed-out validation step
    is) never reaches its cleanup, so its basetemp outlives it (#1064
    test-temp-and-mount-leaks: one host had 347 of them, 22 GiB). The lock on ``.owner`` dies
    with the wrapper, so a free lock proves the owner is gone. A basetemp kept after a failure
    (``.owner`` reads ``kept``), and one from a wrapper older than the owner lock, are removed
    only once older than a day. Only this user's directories with this wrapper's prefix are
    considered.
    """

    now = time.time() if now is None else now
    removed: list[Path] = []
    try:
        entries = list(directory.iterdir())
    except OSError:
        return removed
    for entry in entries:
        if not entry.name.startswith(_TEMP_PREFIX) or entry.name.endswith(_OWNER_SUFFIX):
            continue
        try:
            info = entry.lstat()
        except OSError:
            continue
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
            continue
        owner = Path(f"{entry}{_OWNER_SUFFIX}")
        old = now - info.st_mtime > _STALE_SECONDS
        try:
            descriptor = os.open(owner, os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW)
        except FileNotFoundError:
            if not old:
                continue  # an older wrapper's, or one between creating it and locking it
            descriptor = None
        except OSError:
            continue
        try:
            if descriptor is not None:
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    continue  # its wrapper is alive
                if os.read(descriptor, 4) == b"kept" and not old:
                    continue
            if _mounts_under(entry):
                continue  # a dead test's mount; the mount sweep releases it first
            try:
                _remove_temp_root(entry)
                owner.unlink(missing_ok=True)
            except OSError:
                continue
            removed.append(entry)
            print(f"run_pytest_shard: removed stale basetemp {entry}", file=sys.stderr)
        finally:
            if descriptor is not None:
                os.close(descriptor)
    return removed


def _signal_worker_group(process: subprocess.Popen[bytes], signum: int) -> None:
    """Signal only the private worker session created by this wrapper."""

    if process.pid <= 1 or process.pid == os.getpgrp():
        raise RuntimeError(f"refusing unsafe pytest worker process group {process.pid}")
    try:
        os.killpg(process.pid, signum)
    except ProcessLookupError:
        pass


def _normalize_returncode(returncode: int) -> int:
    """Report signal death with the conventional shell-compatible status."""

    return returncode if returncode >= 0 else 128 - returncode


def _parse_timeout(value: str) -> float:
    try:
        seconds = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("test timeout must be a number of seconds") from error
    if seconds < 0 or seconds != seconds:
        raise argparse.ArgumentTypeError("test timeout must be >= 0 seconds")
    return seconds


def _run_supervised(
    index: int, count: int, pytest_args: Sequence[str], test_timeout: float | None = None
) -> int:
    """Run pytest in an owned session and remove its basetemp on cancellation.

    Python's default SIGTERM disposition bypasses ``finally`` blocks. Keeping the
    wrapper outside pytest gives it a place to record that signal, stop the whole
    pytest process group, reap its leader, and remove the one temp tree it owns.
    """

    pending: list[int] = []
    process: subprocess.Popen[bytes] | None = None
    temp_root: Path | None = None
    owner: int | None = None
    keep = False
    previous_handlers: dict[signal.Signals, _SignalHandler] = {}

    def record_cancellation(signum: int, _frame: FrameType | None) -> None:
        if not pending:
            pending.append(signum)
        if process is not None:
            _signal_worker_group(process, signum)

    for signum in _CANCELLATION_SIGNALS:
        previous_handlers[signum] = signal.getsignal(signum)
        signal.signal(signum, record_cancellation)
    try:
        _sweep_stale_temp_roots(Path(tempfile.gettempdir()))
        temp_root = Path(tempfile.mkdtemp(prefix=f"{_TEMP_PREFIX}{os.getpid()}-{index}-"))
        owner = _hold_owner_lock(temp_root)
        if pending:
            return 128 + pending[0]
        command = [
            sys.executable,
            str(Path(__file__).resolve()),
            "--shard",
            f"{index}/{count}",
            "--_worker-basetemp",
            str(temp_root),
            *(
                ["--_worker-test-timeout", str(test_timeout)]
                if test_timeout is not None
                else []
            ),
            "--",
            *pytest_args,
        ]
        # The worker holds the owner lock too, so a worker that outlives a killed wrapper keeps
        # its basetemp from the sweep.
        process = subprocess.Popen(
            command,
            env=_guarded_environment(dict(os.environ), temp_root),
            start_new_session=True,
            pass_fds=(owner,),
        )
        shutdown_started: float | None = None
        hard_killed = False
        while True:
            returncode = process.poll()
            if pending:
                if shutdown_started is None:
                    shutdown_started = time.monotonic()
                    _signal_worker_group(process, pending[0])
                if returncode is not None:
                    # The pytest leader may exit before a child which ignored TERM.
                    _signal_worker_group(process, signal.SIGKILL)
                    return 128 + pending[0]
                if (
                    not hard_killed
                    and time.monotonic() - shutdown_started >= _TERM_GRACE_SECONDS
                ):
                    _signal_worker_group(process, signal.SIGKILL)
                    hard_killed = True
            elif returncode is not None:
                keep = returncode != 0 and os.environ.get(KEEP_ENV) == "1"
                return _normalize_returncode(returncode)
            time.sleep(_POLL_SECONDS)
    finally:
        if process is not None and pending:
            _signal_worker_group(process, signal.SIGKILL)
            try:
                process.wait(timeout=_TERM_GRACE_SECONDS)
            except subprocess.TimeoutExpired:
                _signal_worker_group(process, signal.SIGKILL)
                process.wait()
        if temp_root is not None and keep and owner is not None:
            os.ftruncate(owner, 0)
            os.write(owner, b"kept")
            print(
                f"run_pytest_shard: kept the failed shard's basetemp at {temp_root} "
                f"({KEEP_ENV}=1); a later shard removes it after a day",
                file=sys.stderr,
            )
        elif temp_root is not None and (mounts := _mounts_under(temp_root)):
            print(
                f"run_pytest_shard: left {temp_root} in place: {len(mounts)} mount(s) inside it "
                f"(first {mounts[0]}); scripts/sweep_test_mounts.py releases them",
                file=sys.stderr,
            )
        elif temp_root is not None:
            _remove_temp_root(temp_root)
            Path(f"{temp_root}{_OWNER_SUFFIX}").unlink(missing_ok=True)
        if owner is not None:
            os.close(owner)
        for signum, previous in previous_handlers.items():
            signal.signal(signum, previous)


def _process_identity(pid: int) -> tuple[str, int, int] | None:
    """Return Linux process state, process group and start time for a live PID."""

    try:
        raw = Path(f"/proc/{pid}/stat").read_bytes()
    except FileNotFoundError:
        return None
    tail = raw[raw.rfind(b") ") + 2 :].split()
    if len(tail) < 20:
        raise AssertionError(f"short /proc identity record for pid {pid}")
    return tail[0].decode("ascii"), int(tail[2]), int(tail[19])


def _self_test_sigterm_cleanup() -> None:
    """Prove a real wrapper cleans its exact temp tree and worker descendants."""

    fixture_root = Path(tempfile.mkdtemp(prefix="agent-utils-sharder-signal-test-"))
    marker = fixture_root / "child.pid"
    test_file = fixture_root / "test_block.py"
    test_file.write_text(
        """\
import os
from pathlib import Path
import subprocess
import sys
import time


def test_block_until_cancelled():
    child = subprocess.Popen([sys.executable, \"-c\", \"import time; time.sleep(300)\"])
    Path(os.environ[\"SHARD_SIGNAL_TEST_MARKER\"]).write_text(str(child.pid), encoding=\"utf-8\")
    time.sleep(300)
""",
        encoding="utf-8",
    )
    # A private TMPDIR: the wrapper's startup sweep must never act on the host's real one here.
    private_tmp = fixture_root / "tmp"
    private_tmp.mkdir()
    environment = _fixture_environment(private_tmp)
    environment["SHARD_SIGNAL_TEST_MARKER"] = str(marker)
    wrapper: subprocess.Popen[str] | None = None
    child_identity: tuple[str, int, int] | None = None
    child_pid: int | None = None
    temp_root: Path | None = None
    try:
        wrapper = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "--shard",
                "0/1",
                "--",
                "-q",
                str(test_file),
            ],
            cwd=fixture_root,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        deadline = time.monotonic() + 10.0
        prefix = f"agent-utils-pytest-{wrapper.pid}-0-"
        while time.monotonic() < deadline:
            matches = [
                path
                for path in private_tmp.iterdir()
                if path.name.startswith(prefix) and path.is_dir()
            ]
            if len(matches) == 1:
                temp_root = matches[0]
                break
            if wrapper.poll() is not None:
                break
            time.sleep(_POLL_SECONDS)
        assert temp_root is not None, "wrapper did not create its private pytest basetemp"

        while time.monotonic() < deadline and not marker.exists():
            if wrapper.poll() is not None:
                break
            time.sleep(_POLL_SECONDS)
        assert marker.exists(), "blocking pytest fixture did not start"
        child_pid = int(marker.read_text(encoding="utf-8"))
        child_identity = _process_identity(child_pid)
        assert child_identity is not None

        wrapper.send_signal(signal.SIGTERM)
        output = wrapper.communicate(timeout=5.0)[0]
        assert wrapper.returncode == 128 + signal.SIGTERM, output
        assert not temp_root.exists(), f"SIGTERM left basetemp behind: {temp_root}"
        owner_file = Path(f"{temp_root}{_OWNER_SUFFIX}")
        assert not owner_file.exists(), f"SIGTERM left the owner lock behind: {owner_file}"

        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline:
            identity = _process_identity(child_pid)
            if identity is None or identity[0] == "Z" or identity[2] != child_identity[2]:
                break
            time.sleep(_POLL_SECONDS)
        else:
            raise AssertionError(f"SIGTERM left pytest child {child_pid} running")
    finally:
        if wrapper is not None and wrapper.poll() is None:
            wrapper.send_signal(signal.SIGTERM)
            try:
                wrapper.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                wrapper.kill()
                wrapper.wait()
        if child_pid is not None and child_identity is not None:
            current = _process_identity(child_pid)
            if current is not None and current[2] == child_identity[2]:
                pgid = current[1]
                if pgid > 1 and pgid != os.getpgrp():
                    try:
                        os.killpg(pgid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
        _remove_temp_root(fixture_root)


def _self_test_sweep() -> None:
    """The sweep removes exactly the basetemps whose wrapper is gone (#1064)."""

    root = Path(tempfile.mkdtemp(prefix="agent-utils-sharder-sweep-test-"))
    try:
        now = time.time()
        day_old = now - _STALE_SECONDS - 60

        def basetemp(name: str, owner: bytes | None, *, age: float = now) -> Path:
            path = root / f"{_TEMP_PREFIX}{name}"
            path.mkdir()
            (path / "state").write_text("fixture\n", encoding="utf-8")
            if owner is not None:
                Path(f"{path}{_OWNER_SUFFIX}").write_bytes(owner)
            os.utime(path, (age, age))
            return path

        live = basetemp("live", b"")
        live_lock = _hold_owner_lock(live)
        dead = basetemp("dead", b"")
        legacy_fresh = basetemp("legacy-fresh", None)
        legacy_old = basetemp("legacy-old", None, age=day_old)
        kept_fresh = basetemp("kept-fresh", b"kept")
        kept_old = basetemp("kept-old", b"kept", age=day_old)
        other = root / "unrelated-dir"
        other.mkdir()
        try:
            removed = set(_sweep_stale_temp_roots(root, now))
        finally:
            os.close(live_lock)
        assert removed == {dead, legacy_old, kept_old}, sorted(map(str, removed))
        for path in (dead, legacy_old, kept_old):
            assert not path.exists() and not Path(f"{path}{_OWNER_SUFFIX}").exists(), path
        for path in (live, legacy_fresh, kept_fresh, other):
            assert path.exists(), path
        # Once its wrapper is gone, the live one goes too.
        assert _sweep_stale_temp_roots(root, now) == [live]
    finally:
        _remove_temp_root(root)


def _self_test_mounts_under() -> None:
    """A basetemp holding a mount is recognised, by either spelling of its path."""

    fixture_root = Path(tempfile.mkdtemp(prefix="run-pytest-shard-mounts-self-test-"))
    try:
        root = fixture_root / "agent-utils-pytest-1-0-abcdefgh"
        root.mkdir()
        info = fixture_root / "mountinfo"
        info.write_text(
            f"36 25 7:7 / {root}/test_x0/project/worktrees/a\\040b rw - ext4 /dev/loop7 rw\n"
            f"37 25 7:8 / {root}-other/slot rw - ext4 /dev/loop8 rw\n",
            encoding="utf-8",
        )
        assert _mounts_under(root, info) == [f"{root}/test_x0/project/worktrees/a b"]
        assert _mounts_under(fixture_root / "elsewhere", info) == []
    finally:
        _remove_temp_root(fixture_root)


def _guarded_environment(base: dict[str, str], directory: Path) -> dict[str, str]:
    """Copy ``base`` with global git config redirected into ``directory`` (#1474).

    The pytest conftest guard installs the same redirect at session start;
    setting it here covers collection-time and fixture subprocesses that run
    before (or without) that conftest, so no git invocation in a shard can
    write the operator's real ``~/.gitconfig``. A caller-provided
    ``GIT_CONFIG_GLOBAL`` is kept.
    """

    environment = dict(base)
    # Tests must never resolve the live harness registry (#1499
    # tests-touch-live-registry): a bare agentctl call walks ancestors from
    # the working directory, and a lane worktree lives under the harness.
    # Point the default at this shard's scratch and disable auto-migrate,
    # unless the caller already chose a registry.
    environment.setdefault("AGENTCTL_REGISTRY", str(directory / "agentctl-registry"))
    environment.setdefault("AGENTCTL_LAYOUT_AUTO_MIGRATE", "0")
    if "GIT_CONFIG_GLOBAL" not in environment:
        sentinel = directory / "global-gitconfig"
        if not sentinel.exists():
            sentinel.write_bytes(
                b"[user]\n\tname = Agent Utils Tests\n"
                b"\temail = agent-utils-tests@example.invalid\n"
                b"[uploadpack]\n\tallowfilter = true\n"
            )
            sentinel.chmod(0o600)
        environment["GIT_CONFIG_GLOBAL"] = str(sentinel)
    environment.setdefault("GIT_CONFIG_NOSYSTEM", "1")
    return environment


def _fixture_environment(tmp: Path) -> dict[str, str]:
    """The environment for a fixture run of this wrapper: a private TMPDIR, no timing records.

    A fixture shard must not write per-case timing records into the gate's timing directory:
    they would read as primary cases of the self-test's own step (#1093).
    """
    import case_timings

    environment = _guarded_environment({**os.environ, "TMPDIR": str(tmp)}, tmp)
    environment.pop(case_timings.TIMING_DIR_ENV, None)
    return environment


def _self_test_keep_on_failure() -> None:
    """A failed shard's basetemp is removed unless KEEP_ENV asks to keep it (#1064)."""

    fixture_root = Path(tempfile.mkdtemp(prefix="agent-utils-sharder-keep-test-"))
    tmp = fixture_root / "tmp"
    tmp.mkdir()
    test_file = fixture_root / "test_fails.py"
    test_file.write_text("def test_fails(tmp_path):\n    assert False\n", encoding="utf-8")
    try:
        for keep in (False, True):
            environment = _fixture_environment(tmp)
            environment.pop(KEEP_ENV, None)
            if keep:
                environment[KEEP_ENV] = "1"
            run = subprocess.run(
                [sys.executable, str(Path(__file__).resolve()), "--shard", "0/1", "--",
                 "-q", "-p", "no:cacheprovider", str(test_file)],
                cwd=fixture_root, env=environment, capture_output=True, text=True, timeout=120,
            )
            assert run.returncode != 0, run.stdout + run.stderr
            left = sorted(path for path in tmp.iterdir() if path.name.startswith(_TEMP_PREFIX))
            if keep:
                assert len(left) == 2 and "kept the failed shard's basetemp" in run.stderr, left
                assert Path(f"{left[0]}{_OWNER_SUFFIX}").read_bytes() == b"kept"
                for path in left:
                    _remove_temp_root(path) if path.is_dir() else path.unlink()
            else:
                assert left == [], f"a failed shard left {left} without {KEEP_ENV}"
    finally:
        _remove_temp_root(fixture_root)


def _self_test_timeout() -> None:
    """Prove a hanging test fails within its bound instead of holding the shard."""

    fixture_root = Path(tempfile.mkdtemp(prefix="agent-utils-sharder-timeout-"))
    test_file = fixture_root / "test_timeout_proof.py"
    test_file.write_text(
        """\
import time

import pytest


def test_hangs_forever():
    time.sleep(60)


def test_quick():
    assert True


@pytest.mark.timeout(0)
def test_marker_disables_the_bound():
    time.sleep(2)
""",
        encoding="utf-8",
    )
    private_tmp = fixture_root / "tmp"
    private_tmp.mkdir()
    environment = _fixture_environment(private_tmp)
    started = time.monotonic()
    try:
        run = subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "--shard", "0/1",
             "--test-timeout", "1", "--", "-q", "-p", "no:cacheprovider", str(test_file)],
            cwd=fixture_root, env=environment, capture_output=True, text=True, timeout=120,
        )
        elapsed = time.monotonic() - started
        output = run.stdout + run.stderr
        assert run.returncode != 0, output
        assert elapsed < 30, f"hanging test was not bounded (took {elapsed:.1f}s): {output}"
        assert "exceeded its per-test timeout" in output, output
        assert "test_hangs_forever" in output, output
    finally:
        _remove_temp_root(fixture_root)


def _self_test_capped_memory_cgroup() -> None:
    """The memory report finds the nearest finite memory.max above the process (#1203)."""
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch) / "cgroup"
        leaf = root / "run.scope" / "step" / "leaf"
        leaf.mkdir(parents=True)
        (root / "run.scope" / "memory.max").write_text("max\n", encoding="utf-8")
        (root / "run.scope" / "step" / "memory.max").write_text("335544320\n", encoding="utf-8")
        (leaf / "memory.max").write_text("max\n", encoding="utf-8")
        proc_cgroup = Path(scratch) / "proc-self-cgroup"
        proc_cgroup.write_text("0::/run.scope/step/leaf\n", encoding="utf-8")
        assert _capped_memory_cgroup(proc_cgroup, root) == (root / "run.scope" / "step", 335544320)
        (root / "run.scope" / "step" / "memory.max").write_text("max\n", encoding="utf-8")
        assert _capped_memory_cgroup(proc_cgroup, root) is None
        proc_cgroup.write_text("1:name=systemd:/x\n", encoding="utf-8")
        assert _capped_memory_cgroup(proc_cgroup, root) is None


def _self_test() -> int:
    nodeids = [
        "tests/test_a.py::test_matrix[one]",
        "tests/test_a.py::test_matrix[two]",
        "tests/test_b.py::test_one",
        "tests/test_c.py::test_one",
        "tests/test_d.py::test_one",
    ]
    first = _assign(nodeids, 3)
    second = _assign(list(reversed(nodeids)), 3)
    assert first == second
    flattened = [nodeid for shard in first for nodeid in shard]
    assert sorted(flattened) == sorted(nodeids)
    assert len(flattened) == len(set(flattened))
    assert all(
        not ({nodeids[0], nodeids[1]} & set(shard))
        or {nodeids[0], nodeids[1]} <= set(shard)
        for shard in first
    )
    assert max(map(len, first)) - min(map(len, first)) <= 1
    # Recorded durations, not case counts, set the balance when they are known:
    # one 90 s family against nine 10 s families balances 90/90 by duration
    # while case counts alone would put 130 s against 50 s.
    skewed = ["tests/test_a.py::test_big"] + [
        f"tests/test_{letter}.py::test_small" for letter in "bcdefghij"
    ]
    durations = {"tests/test_a.py::test_big": 90.0}
    durations.update({f"tests/test_{letter}.py::test_small": 10.0 for letter in "bcdefghij"})

    def true_loads(shards: tuple[tuple[str, ...], ...]) -> list[float]:
        return sorted(
            sum(durations.get(_family(nodeid), 0.0) for nodeid in shard) for shard in shards
        )

    weighted = _assign(skewed, 2, durations)
    assert true_loads(weighted) == [90.0, 90.0]
    assert true_loads(_assign(skewed, 2)) == [50.0, 130.0]
    assert _assign(list(reversed(skewed)), 2, durations) == weighted
    # A family the weights do not know is still partitioned, whole, exactly once.
    with_unknown = [*skewed, "tests/test_new.py::test_new[x]", "tests/test_new.py::test_new[y]"]
    partitioned = [nodeid for shard in _assign(with_unknown, 2, durations) for nodeid in shard]
    assert sorted(partitioned) == sorted(with_unknown)
    # Weights loading: a missing file means count weights; a malformed one fails closed.
    weights_dir = Path(tempfile.mkdtemp(prefix="agent-utils-sharder-weights-"))
    try:
        assert _load_weights(weights_dir / "absent.json") == {}
        bad_weights = weights_dir / "bad.json"
        bad_weights.write_text('{"schema": "other/v9", "families": {}}', encoding="utf-8")
        try:
            _load_weights(bad_weights)
        except ValueError:
            pass
        else:
            raise AssertionError("accepted shard weights with the wrong schema")
        good_weights = weights_dir / "good.json"
        good_weights.write_text(
            json.dumps({"schema": WEIGHTS_SCHEMA, "families": {"tests/test_a.py::t": 1.5}}),
            encoding="utf-8",
        )
        assert _load_weights(good_weights) == {"tests/test_a.py::t": 1.5}
    finally:
        _remove_temp_root(weights_dir)
    for bad in ("", "1", "a/2", "-1/2", "2/2", "0/0"):
        try:
            _parse_shard(bad)
        except argparse.ArgumentTypeError:
            pass
        else:
            raise AssertionError(f"accepted invalid shard {bad!r}")
    cleanup_root = Path(tempfile.mkdtemp(prefix="agent-utils-sharder-cleanup-"))
    locked = cleanup_root / "locked"
    locked.mkdir()
    (locked / "state").write_text("fixture\n", encoding="utf-8")
    locked.chmod(0)
    _remove_temp_root(cleanup_root)
    assert not cleanup_root.exists()
    # Some pytest releases keep registered plugins in a set, so the plugin must hash, and by
    # identity: two plugins with the same fields are still two plugins. A dataclass's generated
    # equality removes its hash. `#235 pytest-plugin-identity`.
    plugin, twin = _ShardPlugin(index=0, count=1), _ShardPlugin(index=0, count=1)
    assert len({plugin, twin, plugin}) == 2
    assert plugin != twin
    _self_test_sigterm_cleanup()
    _self_test_timeout()
    _self_test_sweep()
    _self_test_mounts_under()
    _self_test_keep_on_failure()
    _self_test_capped_memory_cgroup()
    # The guarded worker environment redirects global git config (#1474) and
    # keeps a caller-provided redirect.
    with tempfile.TemporaryDirectory() as scratch:
        guarded = _guarded_environment({}, Path(scratch))
        assert Path(guarded["GIT_CONFIG_GLOBAL"]).is_file()
        assert guarded["GIT_CONFIG_NOSYSTEM"] == "1"
        assert guarded["AGENTCTL_REGISTRY"] == str(Path(scratch) / "agentctl-registry")
        assert guarded["AGENTCTL_LAYOUT_AUTO_MIGRATE"] == "0"
        kept = _guarded_environment({"GIT_CONFIG_GLOBAL": "/caller/sentinel"}, Path(scratch))
        assert kept["GIT_CONFIG_GLOBAL"] == "/caller/sentinel"
        chosen = _guarded_environment({"AGENTCTL_REGISTRY": "/caller/registry"}, Path(scratch))
        assert chosen["AGENTCTL_REGISTRY"] == "/caller/registry"
    print("run_pytest_shard --self-test: PASSED")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard", type=_parse_shard, metavar="INDEX/COUNT")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--test-timeout", type=_parse_timeout, default=None, metavar="SECONDS",
        help="per-test bound for the shard (0 disables; "
        f"default {_DEFAULT_TEST_TIMEOUT_SECONDS:g}s or ${_TEST_TIMEOUT_ENV})",
    )
    parser.add_argument("--_worker-basetemp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--_worker-test-timeout", type=_parse_timeout, default=None, help=argparse.SUPPRESS)
    parser.add_argument("pytest_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.self_test:
        if (
            args.shard is not None
            or args.pytest_args
            or args._worker_basetemp is not None
            or args.test_timeout is not None
            or args._worker_test_timeout is not None
        ):
            parser.error("--self-test does not accept --shard or pytest arguments")
        return _self_test()
    if args.shard is None:
        parser.error("--shard is required unless --self-test is used")

    pytest_args = list(args.pytest_args)
    if pytest_args[:1] == ["--"]:
        pytest_args = pytest_args[1:]
    if not pytest_args:
        parser.error("pytest arguments are required after --")

    index, count = args.shard
    if args._worker_basetemp is not None:
        timeout = (
            args._worker_test_timeout
            if args._worker_test_timeout is not None
            else _env_test_timeout()
        )
        return int(
            pytest.main(
                [*pytest_args, "--basetemp", str(args._worker_basetemp)],
                plugins=[
                    _ShardPlugin(index=index, count=count),
                    _TestTimeoutPlugin(timeout),
                    _MemoryPeakPlugin(),
                ],
            )
        )
    return _run_supervised(index, count, pytest_args, test_timeout=args.test_timeout)


if __name__ == "__main__":
    raise SystemExit(main())
