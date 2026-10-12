"""Writing to stdout and stderr with every failure accounted for.

``print`` raises a traceback when stdout cannot be written, and ``sys.stdout`` is ``None`` when the
descriptor was closed before the interpreter started. herdr-run promises one behaviour in both
editions instead: a stdout write that fails is reported as ``herdr-run: cannot write stdout:
<reason>`` with exit status 1, whether the reason is a closed descriptor, a full disk, or a reader
that went away. Diagnostics on stderr are best effort, because there is nowhere left to report a
failure to write them.

Every write is encoded as UTF-8 and flushed at once, so output order matches the native edition's
and nothing is left in a buffer for interpreter shutdown to fail on.
"""

from __future__ import annotations

import os
import sys
from typing import TextIO

from herdr_run.errors import HerdrRunError
from herdr_run.textfmt import os_error_text

__all__ = ["OutputError", "diagnostic", "stderr_bytes", "stdout", "stdout_bytes", "write_labelled"]


class OutputError(HerdrRunError):
    """Writing the command's own output failed."""

    exit_code = 1


def _encode(text: str) -> bytes:
    return text.encode("utf-8", errors="surrogateescape")


def _write(stream: TextIO | None, descriptor: int, payload: bytes) -> None:
    """Write ``payload`` through ``stream``'s buffer, or straight to the descriptor without one."""
    buffer = getattr(stream, "buffer", None) if stream is not None else None
    if buffer is not None:
        assert stream is not None
        stream.flush()
        buffer.write(payload)
        buffer.flush()
        return
    if stream is not None:
        stream.write(payload.decode("utf-8", errors="surrogateescape"))
        stream.flush()
        return
    view = memoryview(payload)
    while view:
        try:
            written = os.write(descriptor, view)
        except InterruptedError:
            continue
        view = view[written:]


def write_labelled(descriptor: int, payload: bytes, label: str) -> None:
    """Write bytes to stdout (1) or stderr (2), failing with ``cannot write <label>: <reason>``."""
    stream = sys.stdout if descriptor == 1 else sys.stderr
    try:
        _write(stream, descriptor, payload)
    except (OSError, ValueError) as exc:
        raise OutputError(f"cannot write {label}: {os_error_text(exc)}") from exc


def stdout_bytes(payload: bytes) -> None:
    """Write bytes to stdout, failing with ``cannot write stdout: <reason>``."""
    write_labelled(1, payload, "stdout")


def stdout(text: str) -> None:
    """Write text to stdout as UTF-8, failing with ``cannot write stdout: <reason>``."""
    stdout_bytes(_encode(text))


def stderr_bytes(payload: bytes) -> None:
    """Write a command's captured stderr, failing with ``cannot write stderr: <reason>``."""
    write_labelled(2, payload, "stderr")


def diagnostic(text: str) -> None:
    """Write one diagnostic line to stderr, ignoring failure."""
    try:
        _write(sys.stderr, 2, _encode(text + "\n"))
    except (OSError, ValueError):
        pass
