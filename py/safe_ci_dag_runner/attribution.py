"""Durable step output and conservative test-boundary attribution.

This is the Python peer of the native runner's attribution stream. Evidence is explicit opt-in via
``SAFE_CI_DAG_RUNNER_LOG_DIR``; paths are opened nonblocking/no-follow and accepted only as private,
owned regular files so a stale FIFO or symlink cannot hang or redirect a run.
"""

from __future__ import annotations

import json
import os
import stat
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Literal

LOG_DIR_ENV = "SAFE_CI_DAG_RUNNER_LOG_DIR"
NO_LOGS_ENV = "SAFE_CI_DAG_RUNNER_NO_STEP_LOGS"
_MAX_COMPONENT_BYTES = 255


def sanitize(tag: str) -> str:
    """Injectively encode a UTF-8 tag as an ASCII filename component."""
    safe = bytearray()
    for byte in tag.encode("utf-8"):
        if (
            ord("A") <= byte <= ord("Z")
            or ord("a") <= byte <= ord("z")
            or ord("0") <= byte <= ord("9")
            or byte in b"._-"
        ):
            safe.append(byte)
        else:
            safe.extend(f"~{byte:02x}".encode("ascii"))
    return safe.decode("ascii")


def default_log_dir() -> Path | None:
    """Return the explicitly configured evidence directory, if any."""
    if os.environ.get(NO_LOGS_ENV) == "1":
        return None
    value = os.environ.get(LOG_DIR_ENV, "")
    return Path(value) if value else None


def _private_owned_regular(fd: int) -> bool:
    try:
        metadata = os.fstat(fd)
    except OSError:
        return False
    return (
        stat.S_ISREG(metadata.st_mode)
        and metadata.st_uid == os.geteuid()
        and metadata.st_nlink == 1
        and metadata.st_mode & 0o077 == 0
    )


def _open_private_regular(path: Path, flags: int) -> BinaryIO | None:
    try:
        fd = os.open(
            path,
            flags | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK,
            0o600,
        )
    except OSError:
        return None
    try:
        if not _private_owned_regular(fd):
            os.close(fd)
            return None
        return os.fdopen(fd, "ab", buffering=0)
    except BaseException:
        os.close(fd)
        raise


class RunEvidence:
    """One private journal plus per-step raw output logs."""

    def __init__(self, directory: Path, journal: BinaryIO):
        self.directory = directory
        self._journal = journal
        self._lock = threading.Lock()

    @classmethod
    def open(cls, directory: Path | None) -> "RunEvidence | None":
        """Open private run evidence at ``directory``, or disable it safely.

        Existing paths are accepted only when they are owned by this user and inaccessible to
        group/other users.  Special files, symlinks, and hard links are rejected.
        """
        if directory is None:
            return None
        try:
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            metadata = directory.lstat()
        except OSError as exc:
            print(
                f"[scheduler] WARNING: could not create/inspect evidence directory "
                f"{directory} ({exc}); per-step logs and attribution are disabled for this run.",
                file=sys.stderr,
            )
            return None
        if (
            not stat.S_ISDIR(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or metadata.st_mode & 0o700 != 0o700
            or metadata.st_mode & 0o077 != 0
        ):
            print(
                f"[scheduler] WARNING: evidence directory {directory} is not a private, owned "
                "directory; per-step logs and attribution are disabled for this run.",
                file=sys.stderr,
            )
            return None
        journal_path = directory / "journal.jsonl"
        journal = _open_private_regular(journal_path, os.O_WRONLY | os.O_APPEND)
        if journal is None:
            print(
                f"[scheduler] WARNING: evidence journal {journal_path} is not a private, owned "
                "regular file; per-step logs and attribution are disabled for this run.",
                file=sys.stderr,
            )
            return None
        return cls(directory, journal)

    def record(self, event: str, fields: list[tuple[str, str]]) -> None:
        """Append and flush one structured journal record."""
        millis = time.time_ns() // 1_000_000
        payload: dict[str, str] = {
            "ts": f"{millis // 1000}.{millis % 1000:03}",
            "event": event,
        }
        payload.update(fields)
        line = (json.dumps(payload, separators=(",", ":"), ensure_ascii=False) + "\n").encode()
        with self._lock:
            try:
                self._journal.write(line)
                self._journal.flush()
            except OSError:
                pass

    def open_step_log(self, tag: str) -> BinaryIO | None:
        """Create or truncate the private raw-output log for ``tag``."""
        name = f"{sanitize(tag)}.log"
        if len(name.encode("ascii")) > _MAX_COMPONENT_BYTES:
            return None
        handle = _open_private_regular(self.directory / name, os.O_WRONLY)
        if handle is None:
            return None
        try:
            os.ftruncate(handle.fileno(), 0)
        except OSError:
            handle.close()
            return None
        return handle


@dataclass(frozen=True)
class TestEvent:
    """A conservatively recognized test start or completion boundary."""

    kind: Literal["start", "end"]
    name: str
    verdict: str = ""


def recognize(line: str) -> TestEvent | None:
    """Conservatively recognize explicit, libtest, pytest, or TAP boundaries."""
    raw = line.lstrip().rstrip("\n\r")
    stripped = raw.rstrip()
    if stripped.startswith("##TEST-START "):
        name = stripped.removeprefix("##TEST-START ").strip()
        return TestEvent("start", name) if name else None
    if stripped.startswith("##TEST-END "):
        parts = stripped.removeprefix("##TEST-END ").strip().split(maxsplit=1)
        return TestEvent("end", parts[0], parts[1] if len(parts) > 1 else "end") if parts else None
    if raw.startswith("test "):
        rest = raw.removeprefix("test ")
        if " ... " in rest:
            name, after = rest.split(" ... ", 1)
        elif rest.endswith(" ..."):
            name, after = rest.removesuffix(" ..."), ""
        else:
            name, after = "", ""
        name = name.strip()
        if name and " " not in name:
            verdict = after.strip()
            return TestEvent("end", name, verdict.split()[0]) if verdict else TestEvent("start", name)
    parts = stripped.rsplit(maxsplit=1)
    if len(parts) == 2:
        head, verdict = parts
        if (
            "::" in head
            and " " not in head
            and verdict in {"PASSED", "FAILED", "ERROR", "SKIPPED", "XFAIL", "XPASS"}
        ):
            return TestEvent("end", head, verdict)
    if "::" in stripped and " " not in stripped and len(stripped) > 2:
        return TestEvent("start", stripped)
    for prefix, verdict in (("not ok ", "not ok"), ("ok ", "ok")):
        if stripped.startswith(prefix):
            rest = stripped.removeprefix(prefix)
            if " - " in rest:
                number, name = rest.split(" - ", 1)
                if number.strip().isdigit() and name.strip():
                    return TestEvent("end", name.strip(), verdict)
    return None


@dataclass
class TestTracker:
    """Incremental counts and most-recent test identities for one step."""

    last_started: str | None = None
    last_completed: str | None = None
    completed: int = 0
    started: int = 0

    def observe(self, event: TestEvent) -> None:
        """Apply one recognized boundary to this tracker."""
        if event.kind == "start":
            self.last_started = event.name
            self.started += 1
        else:
            if self.last_started != event.name:
                self.last_started = event.name
                self.started += 1
            self.last_completed = event.name
            self.completed += 1


@dataclass(frozen=True)
class Culprit:
    """Best-effort attribution for the test active when a step stopped."""

    test: str | None
    how: str
    completed: int
    last_completed: str | None

    def describe(self) -> str:
        """Render a concise human-readable attribution summary."""
        last = f", last completed {self.last_completed}" if self.last_completed else ""
        if self.test is not None:
            return (
                f"culprit test {self.test} ({self.how}; {self.completed} test(s) completed "
                f"first{last})"
            )
        return f"no test-level attribution ({self.how}; {self.completed} test(s) completed{last})"


class StepStream:
    """One step's raw log, unterminated output tail, and test tracker."""

    def __init__(self, tag: str, evidence: RunEvidence | None):
        self.tag = tag
        self.evidence = evidence
        self._log = evidence.open_step_log(tag) if evidence is not None else None
        self._partial = ""
        self._tracker = TestTracker()
        self._tail_announced: str | None = None
        self._lock = threading.Lock()

    def ingest(self, data: bytes) -> None:
        """Persist and classify one raw output chunk."""
        with self._lock:
            if self._log is not None:
                try:
                    self._log.write(data)
                    self._log.flush()
                except OSError:
                    pass
            self._partial += data.decode(errors="replace")
            while "\n" in self._partial:
                line, self._partial = self._partial.split("\n", 1)
                self._classify(line.rstrip("\r"))
            if len(self._partial) > 8192:
                self._partial = self._partial[-4096:]
            event = recognize(self._partial)
            if event is not None and event.kind == "start" and self._tail_announced != event.name:
                self._tail_announced = event.name
                self._tracker.observe(event)
                if self.evidence is not None:
                    self.evidence.record("test_start", [("step", self.tag), ("test", event.name)])

    def _classify(self, line: str) -> None:
        event = recognize(line)
        if event is None:
            return
        if self._tail_announced == event.name:
            self._tail_announced = None
            if event.kind == "end":
                self._tracker.last_completed = event.name
                self._tracker.completed += 1
                if self.evidence is not None:
                    self.evidence.record(
                        "test_end",
                        [("step", self.tag), ("test", event.name), ("verdict", event.verdict)],
                    )
            return
        self._tracker.observe(event)
        if self.evidence is not None:
            self.evidence.record(
                "test_start" if event.kind == "start" else "test_end",
                [("step", self.tag), ("test", event.name)]
                + ([] if event.kind == "start" else [("verdict", event.verdict)]),
            )

    def culprit(self) -> Culprit:
        """Return the strongest conservative culprit supported by output so far."""
        with self._lock:
            tail = self._partial.lstrip().rstrip("\n\r")
            event = recognize(tail) if tail.strip() else None
            tracker = TestTracker(**vars(self._tracker))
        if event is not None and event.kind == "start":
            return Culprit(
                event.name,
                "announced in the unterminated output tail, never completed",
                tracker.completed,
                tracker.last_completed,
            )
        if tracker.last_started is not None and tracker.last_completed != tracker.last_started:
            return Culprit(
                tracker.last_started,
                "started and never completed",
                tracker.completed,
                tracker.last_completed,
            )
        how = (
            "no test boundaries were recognized in this step's output"
            if tracker.completed == 0
            else "every recognized test completed; the step hung after the last one"
        )
        return Culprit(None, how, tracker.completed, tracker.last_completed)

    def counts(self) -> TestTracker:
        """Return an immutable-in-practice snapshot of current tracker fields."""
        with self._lock:
            return TestTracker(**vars(self._tracker))
