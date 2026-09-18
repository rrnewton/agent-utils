"""Read bounded JSON event frames from one persistent command process."""

from __future__ import annotations

import json
import math
import os
import select
import signal
import socket
import subprocess
import threading
import time
from collections.abc import Sequence
from typing import IO

from agentctl.errors import HerdrUnavailable
from agentctl.jsonx import as_mapping


_CLOSE_GRACE_SECONDS = 2.0


class InputStreamError(HerdrUnavailable):
    """An explicit command or framing failure, distinct from an ordinary timeout."""

    def __init__(self, code: str, detail: str) -> None:
        self.code = code
        self.detail = detail
        super().__init__(f"{code}: {detail}")


def _object_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON object key")
        result[key] = value
    return result


def _nonfinite(value: str) -> object:
    raise ValueError("nonfinite JSON value")


class EventCommandStream:
    """Send one JSON request and read newline-delimited JSON event objects.

    The command runs with literal arguments in its own process group. Its stdin
    receives the request followed by a newline and then EOF. Output is bounded
    by max_frame_bytes per frame; the subscription write must finish within
    start_timeout seconds. Provider message and cursor semantics belong to
    the caller, which must persist accepted input before advancing its cursor.

    The wait method returns at most one object. No background queue accumulates
    events: pipe backpressure bounds producers while the caller processes a
    result. Stderr is drained separately and discarded, so diagnostics cannot
    block the event pipe or expose credentials through exception messages.

    One thread owns wait. Other threads may call wake or close.
    Closing or a protocol failure gives the command up to two seconds after
    SIGTERM to unsubscribe, then kills and reaps its remaining process group.
    Partial frames survive ordinary timeouts; EOF is always an explicit error.
    """

    def __init__(
        self,
        command: Sequence[str],
        request: dict[str, object],
        *,
        max_frame_bytes: int = 1024 * 1024,
        start_timeout: float = 5.0,
    ) -> None:
        if (isinstance(command, (str, bytes)) or not command
                or any(not isinstance(argument, str) or "\0" in argument for argument in command)
                or not command[0]):
            raise ValueError("event command must be a nonempty argument sequence without NUL bytes")
        if isinstance(max_frame_bytes, bool) or not isinstance(max_frame_bytes, int) or max_frame_bytes <= 0:
            raise ValueError("event frame byte limit must be a positive integer")
        if not math.isfinite(start_timeout) or start_timeout <= 0:
            raise ValueError("event command startup timeout must be finite and positive")
        try:
            payload = (json.dumps(as_mapping(request, "subscription request"),
                                  ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")
        except (TypeError, ValueError, UnicodeError, RecursionError) as exc:
            raise ValueError("subscription request must be a valid JSON object") from exc
        if len(payload) - 1 > max_frame_bytes:
            raise ValueError("subscription request exceeds the event frame byte limit")
        self._max_frame = max_frame_bytes
        self._buffer = bytearray()
        self._process: subprocess.Popen[bytes] | None = None
        self._stdin: IO[bytes] | None = None
        self._stdout: IO[bytes] | None = None
        self._stderr: IO[bytes] | None = None
        self._pidfd: int | None = None
        self._exited = False
        self._closed = threading.Event()
        self._released = False
        self._io_lock = threading.RLock()
        self._wake_read, self._wake_write = socket.socketpair()
        self._wake_read.setblocking(False)
        self._wake_write.setblocking(False)
        try:
            self._process = subprocess.Popen(
                list(command), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, start_new_session=True, bufsize=0,
            )
            self._stdin, self._stdout, self._stderr = (
                self._process.stdin, self._process.stdout, self._process.stderr,
            )
            try:
                self._pidfd = os.pidfd_open(self._process.pid)
            except (AttributeError, OSError):
                # Platforms without process descriptors use bounded checks
                # which retain the child identity until group cleanup.
                pass
            assert self._stdin is not None and self._stdout is not None and self._stderr is not None
            for pipe in (self._stdin, self._stdout, self._stderr):
                os.set_blocking(pipe.fileno(), False)
            self._send(payload, time.monotonic() + start_timeout)
        except BaseException as exc:
            self.close()
            if isinstance(exc, OSError):
                raise InputStreamError("command_failed", "could not start or subscribe to the event command") from exc
            raise

    def _drain_wake(self) -> None:
        try:
            self._wake_read.recv(65536)
        except BlockingIOError:
            pass

    def _drain_stderr(self) -> None:
        pipe = self._stderr
        if pipe is None:
            return
        try:
            chunk = os.read(pipe.fileno(), 65536)
        except BlockingIOError:
            return
        if not chunk:
            pipe.close()
            self._stderr = None

    def _read_stdout(self) -> None:
        pipe = self._stdout
        if pipe is None:
            raise InputStreamError("stream_closed", "the event command output is closed")
        capacity = self._max_frame + 1 - len(self._buffer)
        if capacity <= 0:
            raise InputStreamError("frame_limit_exceeded", "event frame exceeds the byte limit")
        try:
            chunk = os.read(pipe.fileno(), min(65536, capacity))
        except BlockingIOError:
            return
        if not chunk:
            detail = "the event command closed its output"
            if self._buffer:
                detail += " in the middle of a frame"
            raise InputStreamError("stream_eof", detail)
        self._buffer.extend(chunk)

    def _readers(self, *, include_stdout: bool = True) -> list[int]:
        readers = [self._wake_read.fileno()]
        if include_stdout and self._stdout is not None:
            readers.append(self._stdout.fileno())
        if self._stderr is not None:
            readers.append(self._stderr.fileno())
        if self._pidfd is not None and not self._exited:
            readers.append(self._pidfd)
        return readers

    def _read_ready(self, readable: list[int]) -> bool:
        if self._wake_read.fileno() in readable:
            self._drain_wake()
            return False
        if self._stderr is not None and self._stderr.fileno() in readable:
            self._drain_stderr()
        if self._stdout is not None and self._stdout.fileno() in readable:
            self._read_stdout()
        if self._pidfd is not None and self._pidfd in readable:
            self._exited = True
        return True

    def _child_exited(self) -> bool:
        if self._pidfd is None and self._process is not None:
            result = os.waitid(os.P_PID, self._process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            self._exited = result is not None
        return self._exited

    def _send(self, payload: bytes, deadline: float) -> None:
        pipe = self._stdin
        assert pipe is not None
        pending = memoryview(payload)
        while pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise InputStreamError("subscription_timeout", "event command did not accept its subscription request")
            readers = self._readers(include_stdout=b"\n" not in self._buffer)
            try:
                readable, writable, _ = select.select(readers, [pipe.fileno()], [], remaining)
            except InterruptedError:
                continue
            if not self._read_ready(readable):
                raise InputStreamError("stream_closed", "event command subscription was interrupted")
            if writable:
                try:
                    sent = os.write(pipe.fileno(), pending)
                except BlockingIOError:
                    continue
                pending = pending[sent:]
        pipe.close()
        self._stdin = None

    def _next_frame(self, deadline: float) -> bytes | None:
        polled = False
        while True:
            newline = self._buffer.find(b"\n")
            if newline >= 0:
                if newline > self._max_frame:
                    raise InputStreamError("frame_limit_exceeded", "event frame exceeds the byte limit")
                frame = bytes(self._buffer[:newline])
                del self._buffer[:newline + 1]
                return frame
            if len(self._buffer) > self._max_frame:
                raise InputStreamError("frame_limit_exceeded", "event frame exceeds the byte limit")
            if self._child_exited():
                # Deliver complete output already written by the command, but
                # do not mistake a descendant holding its pipe open for a live
                # subscription after the owning command has exited.
                assert self._stdout is not None
                readable, _, _ = select.select([self._stdout.fileno()], [], [], 0)
                if readable:
                    self._read_stdout()
                    continue
                detail = "the event command exited"
                if self._buffer:
                    detail += " in the middle of a frame"
                raise InputStreamError("stream_eof", detail)
            remaining = max(0.0, deadline - time.monotonic())
            if polled and remaining == 0:
                return None
            try:
                readable, _, _ = select.select(
                    self._readers(), [], [], remaining if self._pidfd is not None else min(remaining, 0.1),
                )
            except InterruptedError:
                continue
            polled = True
            if not readable:
                if self._pidfd is None and time.monotonic() < deadline:
                    continue
                return None
            if not self._read_ready(readable):
                return None

    @staticmethod
    def _decode(frame: bytes) -> dict[str, object]:
        try:
            document = as_mapping(
                json.loads(frame.decode("utf-8"), object_pairs_hook=_object_pairs, parse_constant=_nonfinite),
                "input event",
            )
            # Escaped lone surrogates can pass the JSON decoder but cannot be
            # persisted or forwarded as UTF-8 source messages.
            json.dumps(document, ensure_ascii=False, allow_nan=False).encode("utf-8")
            return document
        except (UnicodeError, ValueError, TypeError, RecursionError) as exc:
            raise InputStreamError("invalid_frame", "event command emitted an invalid UTF-8 JSON object") from exc

    def wait(self, timeout: float) -> list[dict[str, object]]:
        """Wait up to timeout seconds for one event, or return an empty list.

        Timeout and explicit wakeup preserve buffered input. EOF and malformed
        frames close the stream and raise InputStreamError. Reconnect by
        creating a new stream with the caller's last durably accepted cursor.
        """
        if not math.isfinite(timeout) or timeout < 0:
            raise ValueError("event wait timeout must be finite and nonnegative")
        with self._io_lock:
            if self._closed.is_set():
                raise InputStreamError("stream_closed", "cannot wait on a closed input stream")
            try:
                frame = self._next_frame(time.monotonic() + timeout)
                return [] if frame is None else [self._decode(frame)]
            except InputStreamError as exc:
                self.close()
                if (exc.code == "stream_eof" and self._process is not None
                        and self._process.returncode is not None and self._process.returncode >= 0):
                    raise InputStreamError("stream_eof",
                                           f"{exc.detail}; exit status {self._process.returncode}") from exc
                raise
            except OSError as exc:
                self.close()
                raise InputStreamError("command_io_failed", "event command stream I/O failed") from exc

    def wake(self) -> None:
        """Interrupt a wait from another thread without discarding event frames."""
        try:
            self._wake_write.send(b"x")
        except (BlockingIOError, OSError):
            pass

    def _terminate_leader(self, process: subprocess.Popen[bytes]) -> None:
        # Popen.terminate/send_signal poll the child first, which can reap it
        # and release its PID before we have killed the remaining group.
        try:
            os.kill(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        deadline = time.monotonic() + _CLOSE_GRACE_SECONDS
        while not self._child_exited():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            readers = [pipe.fileno() for pipe in (self._stdout, self._stderr) if pipe is not None]
            if self._pidfd is not None:
                readers.append(self._pidfd)
            try:
                readable, _, _ = select.select(
                    readers, [], [], remaining if self._pidfd is not None else min(remaining, 0.1),
                )
            except InterruptedError:
                continue
            # Cleanup may flush output before unsubscribing. Discard bounded
            # chunks without decoding or buffering shutdown frames, so a full
            # pipe cannot prevent the command from completing its cleanup.
            for attribute in ("_stdout", "_stderr"):
                pipe = self._stdout if attribute == "_stdout" else self._stderr
                if pipe is not None and pipe.fileno() in readable:
                    try:
                        chunk = os.read(pipe.fileno(), 65536)
                    except BlockingIOError:
                        continue
                    if not chunk:
                        pipe.close()
                        if attribute == "_stdout":
                            self._stdout = None
                        else:
                            self._stderr = None
            if self._pidfd is not None and self._pidfd in readable:
                self._exited = True

    def close(self) -> None:
        """Allow bounded command cleanup, then kill its remaining group and reap."""
        self._closed.set()
        self.wake()
        with self._io_lock:
            if self._released:
                return
            self._released = True
            process = self._process
            try:
                if process is not None:
                    # Keep the child unreaped until its entire group is killed:
                    # its PID cannot be reused for an unrelated process group.
                    try:
                        self._terminate_leader(process)
                    finally:
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        try:
                            process.wait(timeout=2)
                        except subprocess.TimeoutExpired as exc:
                            raise InputStreamError("cleanup_timeout", "event command did not exit after termination") from exc
            finally:
                for pipe in (self._stdin, self._stdout, self._stderr):
                    if pipe is not None:
                        pipe.close()
                self._stdin = self._stdout = self._stderr = None
                self._buffer.clear()
                if self._pidfd is not None:
                    os.close(self._pidfd)
                    self._pidfd = None
                self._wake_read.close()
                self._wake_write.close()
