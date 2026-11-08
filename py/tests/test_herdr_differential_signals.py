"""The end-to-end harness hands every edition the same signal dispositions.

``cross/herdr_differential.py`` compares, for every fake-Herdr call, the ignored
signals the call was handed. Both editions hand on the dispositions they were
started with, so a step spawned while the harness process holds ambient ignored
signals (its own, or the C library's once a thread has started) records a
different set from the same step spawned a moment later, and the comparison
went red on the spawn chain (#815). The harness therefore starts every
end-to-end edition with ``_canonical_signal_dispositions``. This pins that the
preexec really produces the same child whatever the parent is holding, and
that without it the ambient set leaks through, so the check can tell the two
apart.
"""

from __future__ import annotations

import ctypes
import importlib
import importlib.util
import os
import signal
import sys
from pathlib import Path
from types import ModuleType

REPO_ROOT = Path(__file__).resolve().parents[2]

_PROBE = ["/bin/grep", "SigIgn", "/proc/self/status"]
_HANDED = (signal.SIGHUP, signal.SIGUSR1, 33)


def _herdr_differential() -> ModuleType:
    # herdr_differential.py imports its sibling modules by bare name, as the harness runs it by path.
    cross = str(REPO_ROOT / "cross")
    if cross not in sys.path:
        sys.path.insert(0, cross)
    spec = importlib.util.spec_from_file_location(
        "_cross_herdr_differential_under_test", REPO_ROOT / "cross" / "herdr_differential.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _own_mask() -> int:
    for line in Path("/proc/self/status").read_text().splitlines():
        if line.startswith("SigIgn:"):
            return int(line.split(":", 1)[1], 16)
    raise AssertionError("no SigIgn line in /proc/self/status")


def _set_ignored(hd: ModuleType, numbers: tuple[int, ...], *, ignored: bool) -> None:
    """Set signals ignored (or back to default) in this process, 32/33 included."""
    number = hd._RT_SIGACTION.get(os.uname().machine)
    if number is not None:
        libc = ctypes.CDLL(None, use_errno=True)
        action = (ctypes.c_uint64 * 4)(1 if ignored else 0, 0, 0, 0)
        for signal_number in numbers:
            libc.syscall(number, signal_number, action, None, 8)
        return
    for signal_number in numbers:
        signal.signal(signal_number, signal.SIG_IGN if ignored else signal.SIG_DFL)


def _child_mask(hd: ModuleType, *, canonical: bool) -> int:
    outcome = hd._run_captured(
        _PROBE,
        REPO_ROOT,
        dict(os.environ),
        # A no-op preexec keeps the ambient dispositions: the harness default
        # (preexec=None) is the canonical set, so leaking needs it explicit.
        preexec=hd._canonical_signal_dispositions if canonical else (lambda: None),
    )
    assert outcome.returncode == 0, outcome.stderr
    return int(outcome.stdout.split(":", 1)[1], 16)


def _bit(number: int) -> int:
    return 1 << (number - 1)


def test_captured_children_get_canonical_dispositions_by_default() -> None:
    # _run_captured is the funnel every harness spawn of an edition passes
    # through; with no preexec named, the child must still start canonical.
    hd = _herdr_differential()
    before = _own_mask()
    try:
        _set_ignored(hd, _HANDED, ignored=True)
        outcome = hd._run_captured(_PROBE, REPO_ROOT, dict(os.environ))
        assert outcome.returncode == 0, outcome.stderr
        assert int(outcome.stdout.split(":", 1)[1], 16) == 0
    finally:
        _set_ignored(hd, _HANDED, ignored=False)
        _set_ignored(hd, tuple(n for n in _HANDED if before & _bit(n)), ignored=True)


def test_readiness_poll_collapse_keeps_distinct_calls_exact() -> None:
    # How many identical readiness readings fit before a wall deadline is scheduling, so the
    # end-to-end herdr-calls comparison collapses them; every distinct call must survive exact
    # and in order, or a real protocol divergence would collapse away with the polls.
    hd = _herdr_differential()
    info = ["pane", "process-info", "--pane", "w1:p1"]
    read = ["pane", "read", "w1:p1", "--source", "recent-unwrapped", "--lines", "4"]
    other_info = ["pane", "process-info", "--pane", "w1:p2"]
    listing = ["workspace", "list"]
    run = ["pane", "run", "w1:p1", "printf x"]

    def collapsed(*argvs: list[str]) -> list[object]:
        calls = [{"argv": argv} for argv in argvs]
        return [call["argv"] for call in hd._collapse_readiness_polls(calls)]

    # A repeated reading (process-info then read) counts once, and so does a repeated single
    # poll, however many times load let an edition repeat it.
    assert collapsed(info, read, info, read, info, read) == [info, read]
    assert collapsed(info, info, info) == [info]
    # Distinct calls are not polls to collapse: another pane's poll, a repeated
    # non-readiness call, and a reading separated by a distinct call all stay, in order.
    assert collapsed(info, read, other_info) == [info, read, other_info]
    assert collapsed(listing, listing) == [listing, listing]
    assert collapsed(info, read, run, info, read) == [info, read, run, info, read]
    assert collapsed() == []


def test_canonical_dispositions_do_not_depend_on_what_the_parent_holds() -> None:
    hd = _herdr_differential()
    before = _own_mask()
    try:
        _set_ignored(hd, _HANDED, ignored=True)
        # The ambient set really is in this process now, and without the
        # canonical preexec it reaches the child: the check can tell.
        assert _child_mask(hd, canonical=False) & _bit(signal.SIGHUP)
        assert _child_mask(hd, canonical=True) == 0
    finally:
        _set_ignored(hd, _HANDED, ignored=False)
        _set_ignored(
            hd,
            tuple(n for n in _HANDED if before & _bit(n)),
            ignored=True,
        )
