"""Bound the FAILURE-DETAIL RENDERING path, not just the capture.

Written to published acceptance criteria rather than to whatever the
implementation happened to achieve:

    capture + rendering     <= 2L + 64 KiB
    failure-detail output   <=  L + 64 KiB

The bound is capture PLUS rendering on purpose. A renderer bounded in isolation
that blows the budget downstream of a bounded capture is the same defect as a
bound with an unbounded stage after it, which is the shape this project kept
finding on 2026-08-19.

EVERY TEST HERE ALSO RUNS AGAINST THE OLD `decode().splitlines()` PATH AND
REQUIRES IT TO FAIL. A suite that passes on both the old and the new renderer
proves nothing about the new one.
"""

from __future__ import annotations

import pathlib
import tracemalloc

import pytest

_SRC = (pathlib.Path(__file__).resolve().parents[1] / "safe_ci_dag_runner/scheduler.py").read_text()
_NS: dict = {}
_FRAG = _SRC[_SRC.index("CAPTURE_TAIL_BYTES = ") : _SRC.index("def _lines_from_pieces(")]
_FRAG += _SRC[_SRC.index("def _lines_from_pieces(") : _SRC.index("def _last_line(")]
exec(compile("import codecs\n" + _FRAG, "scheduler-fragment", "exec"), _NS)  # noqa: S102
BoundedCapture = _NS["_BoundedCapture"]
lines_from_pieces = _NS["_lines_from_pieces"]

L = 4 * 1024 * 1024
COMBINED_CEILING = 2 * L + 64 * 1024
DETAIL_CEILING = L + 64 * 1024

# THE EXACT FOUR-BANNER INCIDENT SHAPE, transcribed from the live 35 MB log
# sampled from a hung run on 2026-08-19 -- not invented text. Two three-line
# banners, one line of each being blank, which is why a third of the incident
# log was blank lines. A fixture built from plausible-looking text has passed in
# this project while production emitted something else.
INCIDENT_BANNER = (
    b"2026-08-19T22:30:04.684925Z  INFO detcore::scheduler: [sched-step5] >>>>>>>\n"
    b"\n"
    b" NONCOMMIT turn 12345, SKIP dettid 5 polling resource Resources { tid: DetPid(5),"
    b' resources: {InternalIOPolling: W}, poll_attempt: 3, fyi: "wait4" }\n'
    b"2026-08-19T22:30:04.684936Z  INFO detcore::scheduler: [scheduler] >>>>>>>\n"
    b"\n"
    b" COMMIT turn 12345, dettid 5 using resources {InternalIOPolling: W}, on previously"
    b" committed 1_767_225_600.152_732_835s\n"
)


def _shape(name: str) -> bytes:
    if name == "incident four-banner":
        return (INCIDENT_BANNER * ((L // len(INCIDENT_BANNER)) + 2))[:L]
    if name == "newline-heavy":
        return b"\n" * L
    if name == "invalid UTF-8":
        return (bytes(range(0x80, 0x100)) * ((L // 128) + 1))[:L]
    if name == "mixed Unicode":
        return ("héllo wörld ✓ 日本語\n".encode() * ((L // 30) + 1))[:L]
    if name == "long-line":
        return b"x" * (L - 1) + b"\n"
    if name == "wrap-boundary":
        # Fills the ring, then wraps by six bytes, so the retained tail spans the
        # seam and rendering must cross it without corrupting a character.
        return b"a" * (L - 7) + b"\n" + b"b" * 6
    raise AssertionError(name)


SHAPES = [
    "incident four-banner",
    "newline-heavy",
    "invalid UTF-8",
    "mixed Unicode",
    "long-line",
    "wrap-boundary",
]


def _capture(blob: bytes) -> BoundedCapture:
    cap = BoundedCapture(L)
    for start in range(0, len(blob), 8192):  # realistic os.read sizes
        cap.append(blob[start : start + 8192])
    return cap


def _peak_new(blob: bytes) -> tuple[int, int]:
    """Peak for capture PLUS incremental rendering. Tracing starts BEFORE the ring
    is constructed, so the ring's own L is counted -- measuring only the renderer
    understates the number that matters and is explicitly rejected."""
    tracemalloc.start()
    cap = _capture(blob)
    count = sum(1 for _ in lines_from_pieces(cap.iter_text()))
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return peak, count


def _peak_old(blob: bytes) -> int:
    """THE MUTATION: the renderer this replaced."""
    tracemalloc.start()
    cap = _capture(blob)
    _ = cap.joined().decode(errors="replace").splitlines()
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return peak


@pytest.mark.parametrize("shape", SHAPES)
def test_capture_plus_rendering_within_combined_ceiling(shape: str) -> None:
    peak, count = _peak_new(_shape(shape))
    assert count > 0, "rendering must emit something; silent dropping is not a fix"
    assert peak <= COMBINED_CEILING, (
        f"{shape}: capture+render {peak:,} = {peak / L:.4f}L exceeds 2L+64KiB"
    )


@pytest.mark.parametrize("shape", SHAPES)
def test_rendering_overhead_within_detail_ceiling(shape: str) -> None:
    """Rendering must add at most 64 KiB on top of the retained L."""
    peak, _ = _peak_new(_shape(shape))
    assert peak - L <= 64 * 1024, (
        f"{shape}: rendering added {peak - L:,} bytes over the retained L"
    )


@pytest.mark.parametrize("shape", SHAPES)
def test_the_old_renderer_reproduces_the_failure(shape: str) -> None:
    """MUTATION PROOF. If this ever passes, these tests have stopped testing.

    Measured overheads of `decode().splitlines()` at L = 4 MiB when written:
        newline-heavy 9.279L, mixed Unicode 4.539L, invalid UTF-8 3.000L,
        incident four-banner 2.494L, long-line 2.000L.
    """
    peak = _peak_old(_shape(shape))
    assert peak > DETAIL_CEILING, (
        f"{shape}: the OLD renderer peaked at {peak:,} = {peak / L:.4f}L, which is "
        f"INSIDE the L+64KiB bar -- the mutation no longer reproduces the failure, "
        f"so these tests prove nothing"
    )


def test_rendering_does_not_drop_content() -> None:
    """Bounded rendering must split, never silently truncate."""
    blob = b"".join(f"line-{i}\n".encode() for i in range(20000))
    cap = _capture(blob)
    rendered = "".join(cap.iter_text())
    assert rendered.endswith("line-19999\n")
    assert "line-19999" in rendered and "line-19998" in rendered


def test_multibyte_character_split_across_the_window_is_not_corrupted() -> None:
    """Incremental decoding, not per-window decode: a character straddling the
    window boundary must not become two replacement characters."""
    blob = "日".encode() * 4000  # 3 bytes each, guaranteed to straddle 2 KiB
    cap = _capture(blob)
    rendered = "".join(cap.iter_text())
    assert "�" not in rendered, "a split multi-byte character was corrupted"
    assert rendered == "日" * 4000
