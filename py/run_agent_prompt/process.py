"""Interrupt-safe ownership of one-shot harness subprocess groups.

Every attempt runs in its own process group. A timeout stops the
whole group, grandchildren included, so a stalled harness cannot
outlive the attempt that launched it.
"""

from __future__ import annotations

import os
import signal
import subprocess
import threading
import time
from collections.abc import Mapping, Sequence
from contextlib import contextmanager
from pathlib import Path
from typing import IO, Iterator


class BackendProcessStopped(RuntimeError):
    """A backend launch raced with cancellation of its owning operation."""


class TerminationSignal(Exception):
    """A SIGTERM or SIGHUP converted into an exception for cleanup."""

    def __init__(self, signum: int) -> None:
        super().__init__(f"received signal {signum}")
        self.signum = signum


#: Exit status reported for a backend call stopped at its time limit (as ``timeout(1)`` does).
TIMED_OUT_EXIT: int = 124


class BackendProcesses:
    """Track backend process groups so the main thread can stop worker-owned calls.

    With *timeout_seconds*, each call that runs longer is stopped (its whole process group:
    terminate, then kill) and reported as a failed call with exit status
    :data:`TIMED_OUT_EXIT`, so a stalled backend fails its batch instead of holding the run.
    """

    def __init__(self, timeout_seconds: float | None = None) -> None:
        if timeout_seconds is not None and timeout_seconds <= 0:
            raise ValueError("backend timeout must be positive")
        self._lock = threading.Lock()
        self._processes: set[subprocess.Popen[str]] = set()
        self._stopped = False
        self._timeout_seconds = timeout_seconds

    def run(
        self,
        command: Sequence[str],
        *,
        input_text: str,
        cwd: Path,
        env: Mapping[str, str] | None = None,
        timeout_seconds: float | None = None,
    ) -> subprocess.CompletedProcess[str]:
        """Run one backend in its own process group and retain captured text output."""

        timeout = self._timeout_seconds if timeout_seconds is None else timeout_seconds
        if timeout is not None and timeout <= 0:
            raise ValueError("backend timeout must be positive")
        args = tuple(command)
        process: subprocess.Popen[str] | None = None
        try:
            with self._lock:
                if self._stopped:
                    raise BackendProcessStopped("backend operation was interrupted")
                process = subprocess.Popen(
                    args,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    cwd=cwd,
                    env=dict(env) if env is not None else None,
                    start_new_session=True,
                )
                self._processes.add(process)
        except BaseException:
            if process is not None:
                self._signal_group(process, signal.SIGKILL)
            raise
        assert process is not None
        stdout_chunks: list[str] = []
        stderr_chunks: list[str] = []
        threads = (
            threading.Thread(
                target=self._feed_stdin, args=(process, input_text), daemon=True
            ),
            threading.Thread(
                target=self._drain_stream, args=(process.stdout, stdout_chunks), daemon=True
            ),
            threading.Thread(
                target=self._drain_stream, args=(process.stderr, stderr_chunks), daemon=True
            ),
        )
        for thread in threads:
            thread.start()
        timed_out = False
        try:
            try:
                process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                self._stop_group(process)
            else:
                if self._group_alive(process):
                    self._stop_group(process)
            stdout, stderr = self._collect_output(
                process, threads, stdout_chunks, stderr_chunks
            )
        except BaseException:
            self.terminate_all()
            raise
        finally:
            self._signal_group(process, signal.SIGKILL)
            with self._lock:
                self._processes.discard(process)
        if timed_out:
            note = f"run-agent-prompt stopped the attempt after {timeout:g} s"
            return subprocess.CompletedProcess(
                args=args,
                returncode=TIMED_OUT_EXIT,
                stdout=stdout,
                stderr=f"{stderr}\n{note}" if stderr else note,
            )
        return subprocess.CompletedProcess(
            args=args,
            returncode=process.returncode,
            stdout=stdout,
            stderr=stderr,
        )

    @staticmethod
    def _feed_stdin(process: subprocess.Popen[str], input_text: str) -> None:
        if process.stdin is None:
            return
        try:
            process.stdin.write(input_text)
            process.stdin.close()
        except (BrokenPipeError, OSError, ValueError):
            pass

    @staticmethod
    def _drain_stream(stream: IO[str] | None, chunks: list[str]) -> None:
        if stream is None:
            return
        try:
            while True:
                chunk = stream.read(8192)
                if not chunk:
                    break
                chunks.append(chunk)
        except (OSError, ValueError):
            pass

    @staticmethod
    def _close_streams(process: subprocess.Popen[str]) -> None:
        for stream in (process.stdin, process.stdout, process.stderr):
            if stream is not None:
                try:
                    stream.close()
                except (BrokenPipeError, OSError, ValueError):
                    pass

    def _collect_output(
        self,
        process: subprocess.Popen[str],
        threads: Sequence[threading.Thread],
        stdout_chunks: list[str],
        stderr_chunks: list[str],
        timeout_seconds: float = 5.0,
    ) -> tuple[str, str]:
        """Collect drained output with a short cap after the leader stops."""

        deadline = time.monotonic() + timeout_seconds
        for thread in threads:
            thread.join(max(0.0, deadline - time.monotonic()))
        if any(thread.is_alive() for thread in threads):
            self._signal_group(process, signal.SIGKILL)
            self._close_streams(process)
            close_deadline = time.monotonic() + 1.0
            for thread in threads:
                thread.join(max(0.0, close_deadline - time.monotonic()))
        return "".join(stdout_chunks), "".join(stderr_chunks)

    def _stop_group(self, process: subprocess.Popen[str], grace_seconds: float = 2.0) -> None:
        """Terminate one call's process group, then kill it after *grace_seconds*."""

        self._signal_group(process, signal.SIGTERM)
        deadline = time.monotonic() + grace_seconds
        while self._group_alive(process) and time.monotonic() < deadline:
            time.sleep(0.01)
        self._signal_group(process, signal.SIGKILL)

    @staticmethod
    def _group_alive(process: subprocess.Popen[str]) -> bool:
        process.poll()  # reap the leader so only live descendants count
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True

    @staticmethod
    def _signal_group(process: subprocess.Popen[str], signum: int) -> None:
        try:
            os.killpg(process.pid, signum)
        except (PermissionError, ProcessLookupError):
            pass

    def terminate_all(self, timeout_seconds: float = 2.0) -> None:
        """Prevent new launches, then terminate and finally kill every live group."""

        with self._lock:
            self._stopped = True
            processes = tuple(self._processes)
        for process in processes:
            self._signal_group(process, signal.SIGTERM)
        deadline = time.monotonic() + timeout_seconds
        while any(process.poll() is None for process in processes):
            if time.monotonic() >= deadline:
                break
            time.sleep(0.01)
        for process in processes:
            self._signal_group(process, signal.SIGKILL)


@contextmanager
def termination_signals_as_exceptions() -> Iterator[None]:
    """Turn SIGTERM and SIGHUP into exceptions so finally cleanup can run."""

    if threading.current_thread() is not threading.main_thread():
        yield
        return
    signums = tuple(
        signum
        for signum in (getattr(signal, "SIGTERM", None), getattr(signal, "SIGHUP", None))
        if isinstance(signum, int)
    )
    previous = {signum: signal.getsignal(signum) for signum in signums}

    def _raise(signum: int, _frame: object) -> None:
        raise TerminationSignal(signum)

    try:
        for signum in signums:
            signal.signal(signum, _raise)
        yield
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


@contextmanager
def defer_signals_during_cleanup() -> Iterator[None]:
    """Ignore termination signals while bounded process/temp cleanup completes."""

    if threading.current_thread() is not threading.main_thread():
        yield
        return
    signums = tuple(
        signum
        for signum in (
            signal.SIGINT,
            getattr(signal, "SIGTERM", None),
            getattr(signal, "SIGHUP", None),
        )
        if isinstance(signum, int)
    )
    previous = {signum: signal.getsignal(signum) for signum in signums}
    try:
        for signum in signums:
            signal.signal(signum, signal.SIG_IGN)
        yield
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


__all__ = [
    "TIMED_OUT_EXIT",
    "BackendProcessStopped",
    "BackendProcesses",
    "TerminationSignal",
    "defer_signals_during_cleanup",
    "termination_signals_as_exceptions",
]
