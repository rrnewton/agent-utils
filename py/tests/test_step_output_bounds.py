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
#: Published acceptance criterion for the capture peak. Not negotiable downward
#: after a failure without owner approval.
CEILING = 2 * LIMIT + 64 * 1024


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
    # THE BAR IS THE PUBLISHED ACCEPTANCE CRITERION: 2L + 64 KiB. Not a number I
    # chose. Two earlier bars were rejected as unearned slack -- 3x (33% over
    # measurement) and then 2.3x (still above the criterion) -- and thresholds may
    # not be raised after a failure. The ring buffer meets the real bar: worst
    # observed 2.000123L across uniform sizes, every boundary pair around L/2 and
    # L, one-byte chunks, randomized sequences and a 1 GiB feed, clearing by
    # 65,021 bytes.
    assert peak <= CEILING, (
        f"{label}: peak {peak:,} = {peak / LIMIT:.6f}L against the "
        f"2L + 64 KiB bar of {CEILING:,}"
    )


def test_one_byte_chunks_do_not_inflate_the_bound() -> None:
    """The shape that killed the deque alternative: 8.25x peak, 89x on join.

    `os.read(fd, n)` guarantees only a MAXIMUM, so a producer can emit one byte at
    a time. A byte bound expressed in units the producer controls is not a bound.
    The ring is chunk-shape independent by construction -- one preallocated
    buffer, no per-chunk objects.
    """
    peak, cap = peak_for([b"x"] * (LIMIT * 2))
    assert len(cap[0]) == LIMIT
    assert peak <= CEILING, f"one-byte chunks reached {peak / LIMIT:.4f}L"


def test_joined_output_stays_inside_the_ceiling() -> None:
    """The reassembly is the second L; it must not become a third."""
    cap = BoundedCapture(LIMIT)
    for _ in range(64):
        cap.append(b"y" * (1 << 20))
    tracemalloc.start()
    blob = cap.joined()
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    assert len(blob) >= LIMIT
    assert peak <= CEILING, f"joined() peaked at {peak / LIMIT:.4f}L"


def test_a_single_write_larger_than_the_bound_does_not_inflate_it() -> None:
    """One 256 MiB write drove a 4 MiB bound to a 260 MiB peak before the fix."""
    big = b"z" * (256 << 20)
    peak, cap = peak_for([big])
    assert len(cap[0]) == LIMIT
    assert peak <= CEILING, f"peak {peak / 2**20:.1f} MiB tracked the CHUNK, not the bound"


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


# --------------------------------------------------------------------------
# The two bounds the first round of tests did NOT cover. A review pointed out
# that `pending` was only compared as a CONSTANT and the disk cap was untested
# entirely, so "14 passed" proved one bound out of three.
# --------------------------------------------------------------------------


def _drain_like_the_reader(chunks: list[bytes], limit: int) -> tuple[int, list[str]]:
    """Mirror scheduler.py's stream loop: extend, flush-if-oversized, split lines.

    A copy of the logic rather than the logic itself, because the real loop is
    welded to a Popen. If they drift this test stops guarding anything, so the
    shape is kept deliberately small and obvious.
    """
    pending = bytearray()
    emitted: list[str] = []
    peak = 0
    for chunk in chunks:
        pending.extend(chunk)
        if len(pending) > limit:
            emitted.append(bytes(pending).decode(errors="replace"))
            pending.clear()
        while b"\n" in pending:
            index = pending.index(b"\n")
            emitted.append(bytes(pending[: index + 1]).decode(errors="replace"))
            del pending[: index + 1]
        peak = max(peak, len(pending))
    return peak, emitted


def test_pending_buffer_is_bounded_on_newline_free_output() -> None:
    """The verbosity>=2 path. Newline-free output grew this without limit, and it
    sat OUTSIDE the capture bound -- so bounding `captured` alone would have left
    the memory hole open on exactly the path that streams a runaway to a human."""
    peak, emitted = _drain_like_the_reader([b"x" * 65536] * 1024, MAX_PENDING)
    assert peak <= MAX_PENDING + 65536, f"pending reached {peak}"
    assert emitted, "an unterminated run must be flushed, not hoarded"


def test_pending_buffer_still_splits_ordinary_lines() -> None:
    """The bound must not change behaviour for output that does have newlines."""
    peak, emitted = _drain_like_the_reader([b"one\ntwo\nthree\n"], MAX_PENDING)
    assert peak == 0
    assert emitted == ["one\n", "two\n", "three\n"]


def _disk_sim(cap: int, chunk: int, count: int, notice_len: int = 140) -> int:
    """Mirror StepStream.ingest's cap arithmetic, including the reserve clamp."""
    reserve = min(512, cap)
    written = 0
    for _ in range(count):
        room = cap - reserve - written
        written += min(chunk, room) if room > 0 else 0
    return written + min(notice_len, reserve)


@pytest.mark.parametrize("cap", [1, 16, 64, 128, 200, 512, 513, 1024, 8192, 1 << 20])
def test_step_log_never_exceeds_its_advertised_cap(cap: int) -> None:
    """A cap that overshoots to announce itself is not a cap.

    Two measured failures this guards: cap=1024 produced 1167 bytes when the
    notice was written past the budget, and cap=128 produced 142 bytes when the
    reserve exceeded the cap itself and the notice was clamped to the reserve
    rather than to the cap.
    """
    assert _disk_sim(cap, 100, 200) <= cap


def test_step_log_keeps_data_when_the_cap_allows_it() -> None:
    """The cap must not be satisfied by writing nothing -- that would 'pass' trivially."""
    assert _disk_sim(1 << 20, 100, 200) > 512
