"""Interrupt-safe ownership of model-backend subprocess groups."""

from __future__ import annotations

import os
import signal
import subprocess
import threading
import time
from collections.abc import Sequence
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator


class BackendProcessStopped(RuntimeError):
    """A backend launch raced with cancellation of its owning operation."""


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
    ) -> subprocess.CompletedProcess[str]:
        """Run one backend in its own process group and retain captured text output."""

        args = tuple(command)
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
                start_new_session=True,
            )
            self._processes.add(process)
        timed_out = False
        try:
            try:
                stdout, stderr = process.communicate(
                    input=input_text, timeout=self._timeout_seconds
                )
            except subprocess.TimeoutExpired:
                timed_out = True
                self._stop_group(process)
                stdout, stderr = process.communicate()
        except BaseException:
            self.terminate_all()
            raise
        finally:
            with self._lock:
                self._processes.discard(process)
        if timed_out:
            note = (
                f"wrkviz stopped the backend call after {self._timeout_seconds:g} s "
                "(--backend-timeout)"
            )
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

    def _stop_group(self, process: subprocess.Popen[str], grace_seconds: float = 2.0) -> None:
        """Terminate one call's process group, then kill it after *grace_seconds*."""

        self._signal_group(process, signal.SIGTERM)
        deadline = time.monotonic() + grace_seconds
        while process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.01)
        self._signal_group(process, signal.SIGKILL)

    @staticmethod
    def _signal_group(process: subprocess.Popen[str], signum: int) -> None:
        if process.poll() is not None:
            return
        try:
            os.killpg(process.pid, signum)
        except ProcessLookupError:
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
def defer_sigint_during_cleanup() -> Iterator[None]:
    """Ignore a repeated Ctrl-C while bounded executor/process cleanup completes."""

    if threading.current_thread() is not threading.main_thread():
        yield
        return
    previous = signal.getsignal(signal.SIGINT)
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    try:
        yield
    finally:
        signal.signal(signal.SIGINT, previous)


__all__ = [
    "TIMED_OUT_EXIT",
    "BackendProcessStopped",
    "BackendProcesses",
    "defer_sigint_during_cleanup",
]
