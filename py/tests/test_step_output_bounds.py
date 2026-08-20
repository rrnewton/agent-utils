"""Assert the runner's three output bounds. Written because a review pointed out
that the suite passing proved NONE of them: the bounds landed with no test, so
green said only that nothing else broke.

Each test names the measured failure it guards, so a future reader can tell a
real regression from a tightened tolerance.
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tracemalloc

import pytest

_ROOT = pathlib.Path(__file__).resolve().parents[1]
_SRC = (_ROOT / "safe_ci_dag_runner/scheduler.py").read_text()
# Extract just the bounded-capture fragment: importing the whole scheduler drags
# in the runner's full dependency graph, and these are pure-data assertions.
_NS: dict = {}
exec(  # noqa: S102
    compile(
        _SRC[_SRC.index("CAPTURE_TAIL_BYTES = ") : _SRC.index("def _last_line(")],
        "scheduler-fragment",
        "exec",
    ),
    _NS,
)
BoundedCapture = _NS["_BoundedCapture"]
CAPTURE_TAIL_BYTES = _NS["CAPTURE_TAIL_BYTES"]
MAX_PENDING = _NS["_MAX_PENDING_LINE_BYTES"]

LIMIT = 4 * 1024 * 1024


def peak_for(chunks: list[bytes], limit: int = LIMIT) -> tuple[int, "BoundedCapture"]:
    """Peak traced memory for feeding ``chunks``, with the chunks pre-allocated.

    Allocating the chunks BEFORE tracing starts is deliberate: the caller's own
    buffer is not this class's to bound, and counting it would flatter or damn
    the result depending on chunk size rather than on retention.
    """
    cap = BoundedCapture(limit)
    tracemalloc.start()
    for chunk in chunks:
        cap.append(chunk)
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return peak, cap


@pytest.mark.parametrize(
    "label,size,count",
    [
        # The shape a review found and the previous implementation failed on: a
        # chunk ONE BYTE under the limit. append-then-trim peaked at 17.001 MiB
        # against a 4 MiB bound (4.25x). It is the worst case precisely because
        # the buffer is nearly full when a nearly-full chunk arrives.
        ("limit-1", LIMIT - 1, 8),
        ("limit-2", LIMIT - 2, 8),
        ("limit exactly", LIMIT, 8),
        ("limit+1", LIMIT + 1, 8),
        ("half+1", LIMIT // 2 + 1, 16),
        ("three-quarters", 3 * LIMIT // 4, 16),
        ("1 MiB", 1 << 20, 256),
        ("64 KiB", 64 << 10, 1024),
    ],
)
def test_capture_peak_stays_near_the_bound(label: str, size: int, count: int) -> None:
    peak, cap = peak_for([b"z" * size] * count)
    assert len(cap[0]) == LIMIT, f"{label}: retained {len(cap[0])}, want exactly the limit"
    # 3x is the assertion, not the measurement. Observed worst across this sweep
    # after the trim-before-append fix is 2.25x (64 KiB and 1 MiB chunks, where
    # bytearray amortised growth dominates); limit-1 is now 1.00x. The headroom
    # between 2.25 and 3 exists so an allocator change is not a test failure,
    # NOT because a larger peak would be acceptable.
    assert peak <= 3 * LIMIT, f"{label}: peak {peak / 2**20:.3f} MiB against a {LIMIT / 2**20} MiB bound"


def test_a_single_write_larger_than_the_bound_does_not_inflate_it() -> None:
    """One 256 MiB write drove a 4 MiB bound to a 260 MiB peak before the fix."""
    big = b"z" * (256 << 20)
    peak, cap = peak_for([big])
    assert len(cap[0]) == LIMIT
    assert peak <= 2 * LIMIT, f"peak {peak / 2**20:.1f} MiB tracked the CHUNK, not the bound"


def test_dropped_bytes_are_counted_and_surfaced() -> None:
    """A truncated dump that reads as complete is the failure being prevented."""
    _, cap = peak_for([b"y" * (1 << 20)] * 64)
    assert cap.dropped == 64 * (1 << 20) - LIMIT
    joined = cap.joined()
    assert b"earlier byte(s) of this step's output are NOT shown" in joined
    assert str(cap.dropped).encode() in joined


def test_no_notice_when_nothing_was_dropped() -> None:
    _, cap = peak_for([b"y" * 1024])
    assert cap.dropped == 0
    assert cap.joined() == b"y" * 1024


def test_empty_capture_is_safe_for_every_consumer() -> None:
    """_last_line() reverses it and the failure path joins it; both run on empty."""
    cap = BoundedCapture(LIMIT)
    assert len(cap) == 0 and not cap and cap.joined() == b""
    assert list(reversed(cap)) == [] and list(cap) == []


def test_sequence_protocol_supports_last_line() -> None:
    """_last_line takes a Sequence and calls reversed(); __iter__ alone is not enough."""
    _, cap = peak_for([b"a" * 4096])
    assert next(iter(reversed(cap))).endswith(b"a")
    assert cap[0] == cap[-1]


def test_pending_line_bound_is_smaller_than_the_capture_bound() -> None:
    """They bound different things: what is BUFFERED awaiting a newline, versus
    what is KEPT. The stream buffer must not be able to dominate the total."""
    assert 0 < MAX_PENDING <= CAPTURE_TAIL_BYTES
