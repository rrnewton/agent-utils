"""Audit lock-free planning (#797) and registry lock fairness (#1172).

The audit takes its registry snapshot under a short shared ACTIVE lock,
plans and censuses outside it, and revalidates before publishing.
Revalidation is per record: a registry change invalidates only the rows
whose own identity changed, appeared, or disappeared; unchanged rows
keep the verdicts measured outside the lock.

A writer waiting for the registry lock holds the writer-intent turnstile,
so readers that arrive after it queue behind it instead of starving it.
"""

from __future__ import annotations

import contextlib
import copy
import fcntl
import io
import json
import os
import re
import shutil
import subprocess
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_audit_state_rust import AU_ROOT, _fixture, _git, _python

# Registry judgment, not process evidence: see the pinned_process_evidence fixture.
pytestmark = pytest.mark.usefixtures("pinned_process_evidence")


def _active(root: Path) -> Path:
    return root / "worktrees" / "ACTIVE.node-a.json"


def _read_active(root: Path) -> dict[str, object]:
    return cast(dict[str, object], json.loads(_active(root).read_text()))


def _write_active(root: Path, document: dict[str, object]) -> None:
    # Replace the registry atomically, as the product does, so an audit reading it meanwhile sees
    # the old document or the new one, never a torn one (#997). The staged copy lives outside the
    # control directory: one beside the registry is a partial update the product refuses.
    staged = root / "ACTIVE.staged.json"
    staged.write_text(json.dumps(document))
    os.replace(staged, _active(root))


def _add_slot_b(root: Path) -> None:
    document = _read_active(root)
    slots = cast(list[dict[str, object]], document["slots"])
    record = copy.deepcopy(slots[0])
    record["slot"] = "slot-b"
    record["agent"] = "agent-slot-b"
    checkouts = cast(list[dict[str, object]], record["checkouts"])
    checkouts[0]["path"] = "worktrees/slot-b/repo"
    slots.append(record)
    _write_active(root, document)


def _during_census(monkeypatch: pytest.MonkeyPatch, mutate: Callable[[], None]) -> None:
    original = wrkslots_cli._capture_process_path_census

    def census(paths: object) -> object:
        mutate()
        return original(paths)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)


def _rows(payload: dict[str, object]) -> dict[str, dict[str, object]]:
    rows = cast(list[dict[str, object]], payload["slots"])
    return {str(row["slot"]): row for row in rows}


# Unpinned: the real census runs while the writer lock is probed.
@pytest.mark.real_process_evidence
def test_audit_measures_outside_the_state_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    control = root / "worktrees"
    observed: dict[str, bool] = {}
    original = wrkslots_cli._capture_process_path_census

    def census(paths: object) -> object:
        # A writer's exclusive lock must be available while the audit is
        # measuring: the audit may not hold the shared lock here.
        try:
            with wrkslots_cli._locked(
                control / "ACTIVE", exclusive=True, wait_seconds=0
            ):
                observed["writer_lock_acquired"] = True
        except wrkslots_cli.Refusal:
            observed["writer_lock_acquired"] = False
        return original(paths)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)
    payload = _python(root, tmp_path)
    assert observed["writer_lock_acquired"] is True
    assert payload["slots"], "fixture audit produced no rows"


def test_audit_revision_only_change_retains_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")

    def mutate() -> None:
        document = _read_active(root)
        document["revision"] = cast(int, document["revision"]) + 1
        _write_active(root, document)

    _during_census(monkeypatch, mutate)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert rows["slot-a"]["verdict"] != "UNKNOWN", rows["slot-a"]
    assert not any(
        "registry changed during audit" in str(reason)
        for reason in cast(list[object], rows["slot-a"]["reasons"])
    )


def test_audit_unrelated_creation_retains_unchanged_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")

    def mutate() -> None:
        document = _read_active(root)
        slots = cast(list[dict[str, object]], document["slots"])
        record = copy.deepcopy(slots[0])
        record["slot"] = "slot-b"
        record["agent"] = "agent-slot-b"
        checkouts = cast(list[dict[str, object]], record["checkouts"])
        checkouts[0]["path"] = "worktrees/slot-b/repo"
        slots.append(record)
        document["revision"] = cast(int, document["revision"]) + 1
        _write_active(root, document)

    _during_census(monkeypatch, mutate)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert rows["slot-a"]["verdict"] != "UNKNOWN", rows["slot-a"]
    assert rows["slot-b"]["verdict"] == "UNKNOWN", rows["slot-b"]
    assert any(
        "appeared during audit" in str(reason)
        for reason in cast(list[object], rows["slot-b"]["reasons"])
    )
    assert "slot-b" in cast(list[object], payload["unknown_slots"])


@pytest.mark.real_process_evidence
def test_audit_does_not_retry_a_discarded_census(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)
    calls = 0

    def census(paths: object) -> object:
        nonlocal calls
        calls += 1
        raise wrkslots_cli.Refusal(
            "relevant process evidence changed during three liveness attempts"
        )

    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    # One capture attempt set per audit: the capture's own internal
    # retries are unchanged, but the audit no longer multiplies its
    # budget by retrying the whole capture until a wall window ends.
    # The failure stays fail-closed for rows that needed the census.
    assert calls == 1
    assert rows["slot-a"]["verdict"] == "UNKNOWN", rows["slot-a"]
    assert rows["slot-b"]["verdict"] == "UNKNOWN", rows["slot-b"]


@pytest.mark.real_process_evidence
def test_audit_skips_census_when_no_row_can_use_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    document = _read_active(root)
    # A fresh heartbeat means no row can reach the branch that consults
    # the census, so a host-wide process scan cannot change a verdict.
    cast(list[dict[str, object]], document["slots"])[0][
        "heartbeat_at"
    ] = wrkslots_cli._utc_now()
    _write_active(root, document)

    def census(paths: object) -> object:
        raise AssertionError("census ran although no row can consult it")

    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert rows["slot-a"]["verdict"] == "BLOCKED"
    phases = {
        str(phase["name"]): phase
        for phase in cast(
            list[dict[str, object]],
            cast(dict[str, object], payload["metrics"])["phases"],
        )
    }
    census_phase = phases["process-census"]
    assert cast(dict[str, object], census_phase["work"])["census_attempted"] == 0


def test_audit_own_record_change_marks_only_that_row_unknown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)

    def mutate() -> None:
        document = _read_active(root)
        cast(list[dict[str, object]], document["slots"])[0]["task"] = "changed during audit"
        document["revision"] = cast(int, document["revision"]) + 1
        _write_active(root, document)

    _during_census(monkeypatch, mutate)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert rows["slot-a"]["verdict"] == "UNKNOWN", rows["slot-a"]
    assert any(
        "registry changed during audit" in str(reason)
        for reason in cast(list[object], rows["slot-a"]["reasons"])
    )
    assert rows["slot-b"]["verdict"] != "UNKNOWN", rows["slot-b"]


def test_audit_heartbeat_renewal_marks_only_that_row_unknown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)

    def mutate() -> None:
        document = _read_active(root)
        cast(list[dict[str, object]], document["slots"])[0]["heartbeat_at"] = "2026-10-26T02:00:00Z"
        document["revision"] = cast(int, document["revision"]) + 1
        _write_active(root, document)

    _during_census(monkeypatch, mutate)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert rows["slot-a"]["verdict"] == "UNKNOWN", rows["slot-a"]
    assert rows["slot-b"]["verdict"] != "UNKNOWN", rows["slot-b"]


def test_audit_unregistered_directory_appearing_late_is_attention(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    original = wrkslots_cli._audit_cache_census

    def census(*args: object, **kwargs: object) -> object:
        (root / "worktrees" / "slot-late").mkdir(exist_ok=True)
        return original(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_audit_cache_census", census)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert "slot-late" in rows
    assert rows["slot-late"]["owner_state"] == "unregistered"
    assert rows["slot-late"]["verdict"] == "BLOCKED"
    assert "slot-late" in cast(list[object], payload["attention_slots"])


def test_audit_seal_appearing_marks_only_affected_row_unknown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)
    original = wrkslots_cli._audit_validate_batch_seal_evidence
    calls = {"count": 0}

    def evidence(config: object) -> object:
        calls["count"] += 1
        if calls["count"] == 1:
            return original(config)  # type: ignore[arg-type]
        return ((("node-a", "slot-a", 1, Path("/seal/slot-a")),), ())

    monkeypatch.setattr(
        wrkslots_cli, "_audit_validate_batch_seal_evidence", evidence
    )
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert rows["slot-a"]["verdict"] == "UNKNOWN", rows["slot-a"]
    assert rows["slot-b"]["verdict"] != "UNKNOWN", rows["slot-b"]


def _reader_stream(
    subject: Path, stop: threading.Event, *, hold: float, offset: float
) -> threading.Thread:
    """Overlapping new-style readers: one always holds the lock while the stream runs."""

    def run() -> None:
        time.sleep(offset)
        while not stop.is_set():
            with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=30):
                time.sleep(hold)

    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    return thread


def _timed_writer(subject: Path, wait_seconds: float) -> float:
    started = time.monotonic()
    with wrkslots_cli._locked(subject, exclusive=True, wait_seconds=wait_seconds):
        return time.monotonic() - started


def test_waiting_writer_is_not_starved_by_overlapping_readers(tmp_path: Path) -> None:
    subject = tmp_path / "ACTIVE"
    stop = threading.Event()
    # Three readers each holding 0.6 s, staggered by 0.2 s: without the turnstile some reader
    # always holds the shared lock, and the writer's poll never finds it free.
    readers = [_reader_stream(subject, stop, hold=0.6, offset=0.2 * i) for i in range(3)]
    try:
        time.sleep(1.0)
        waited = _timed_writer(subject, wait_seconds=20)
    finally:
        stop.set()
        for reader in readers:
            reader.join(timeout=30)
    # Bounded by the readers already inside (at most one hold each), plus polling.
    assert waited < 3.0, waited


def test_old_style_reader_weakens_fairness_but_never_deadlocks(tmp_path: Path) -> None:
    subject = tmp_path / "ACTIVE"
    lock_path = wrkslots_cli._lock_path(subject)
    stop = threading.Event()
    old_done = threading.Event()

    def old_reader() -> None:
        # An older release: a shared flock on the registry lock, no turnstile.
        fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            for _ in range(4):
                fcntl.flock(fd, fcntl.LOCK_SH)
                time.sleep(0.4)
                fcntl.flock(fd, fcntl.LOCK_UN)
                time.sleep(0.05)
        finally:
            os.close(fd)
            old_done.set()

    old = threading.Thread(target=old_reader, daemon=True)
    old.start()
    readers = [_reader_stream(subject, stop, hold=0.3, offset=0.1 * i) for i in range(2)]
    try:
        time.sleep(0.3)
        waited = _timed_writer(subject, wait_seconds=20)
        # New readers still get the lock after the writer.
        with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=10):
            pass
    finally:
        stop.set()
        for reader in readers:
            reader.join(timeout=30)
        old.join(timeout=30)
    assert old_done.is_set()
    assert waited < 20, waited


def test_stale_intent_file_does_not_block_readers_or_writers(tmp_path: Path) -> None:
    subject = tmp_path / "ACTIVE"
    intent = wrkslots_cli._registry_intent_path(subject)
    assert intent is not None
    # A crashed writer leaves the file; only a held flock means intent.
    intent.write_text("")
    with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=0):
        pass
    with wrkslots_cli._locked(subject, exclusive=True, wait_seconds=0):
        pass


def test_reader_defers_to_a_stuck_writer_intent_only_briefly(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    subject = tmp_path / "ACTIVE"
    intent = wrkslots_cli._registry_intent_path(subject)
    assert intent is not None
    monkeypatch.setattr(wrkslots_cli, "_REGISTRY_INTENT_DEFER_SECONDS", 0.5)
    fd = os.open(intent, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        # A writer that announced itself and never proceeds.
        fcntl.flock(fd, fcntl.LOCK_EX)
        started = time.monotonic()
        with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=10):
            waited = time.monotonic() - started
        # A zero-wait reader does not defer at all.
        with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=0):
            pass
        # Another writer queues behind it and reports busy by name.
        with pytest.raises(wrkslots_cli.Refusal, match="another writer is waiting"):
            with wrkslots_cli._locked(subject, exclusive=True, wait_seconds=0.2):
                pass
    finally:
        os.close(fd)
    assert 0.4 <= waited < 5, waited


def test_nested_reader_does_not_queue_behind_a_writer_waiting_for_it(tmp_path: Path) -> None:
    subject = tmp_path / "ACTIVE"
    writer_waiting = threading.Event()
    result: dict[str, object] = {}

    def writer() -> None:
        writer_waiting.set()
        try:
            result["waited"] = _timed_writer(subject, wait_seconds=10)
        except wrkslots_cli.Refusal as exc:
            result["error"] = exc

    with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=0):
        thread = threading.Thread(target=writer, daemon=True)
        thread.start()
        writer_waiting.wait(5)
        time.sleep(0.3)
        started = time.monotonic()
        # The same thread reads again through a second descriptor: it must not wait for the
        # writer, which waits for this very thread.
        with wrkslots_cli._locked(subject, exclusive=False, wait_seconds=5):
            nested = time.monotonic() - started
    thread.join(timeout=30)
    assert nested < 1.0, nested
    assert "error" not in result, result


def _cli(root: Path, *argv: str) -> str:
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream):
        assert wrkslots_cli.main(["--project-root", str(root), *argv]) == 0
    return stream.getvalue()


def _add_events(root: Path, pairs: int) -> None:
    for _ in range(pairs):
        _cli(root, "hold", "--reason", "fixture", "slot-a")
        _cli(root, "unhold", "slot-a")


@pytest.mark.parametrize("command", ["doctor", "status", "audit", "unpushed"])
def test_readers_verify_events_before_the_shared_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, command: str
) -> None:
    root = _fixture(tmp_path / "case")
    _add_events(root, 3)
    # Every event file counts as stable, so the memo keeps all of them.
    monkeypatch.setattr(wrkslots_cli, "_EVENT_MEMO_STABLE_NS", 0)
    original_read = wrkslots_cli._event_from_path
    original_locked = wrkslots_cli._locked
    reads = {"outside": 0, "inside": 0}
    inside = {"depth": 0}

    def read(*args: object, **kwargs: object) -> object:
        reads["inside" if inside["depth"] else "outside"] += 1
        return original_read(*args, **kwargs)  # type: ignore[arg-type]

    @contextlib.contextmanager
    def locked(subject: Path, **kwargs: object) -> Iterator[None]:
        with original_locked(subject, **kwargs):  # type: ignore[arg-type]
            inside["depth"] += subject.name == "ACTIVE"
            try:
                yield
            finally:
                inside["depth"] -= subject.name == "ACTIVE"

    monkeypatch.setattr(wrkslots_cli, "_event_from_path", read)
    monkeypatch.setattr(wrkslots_cli, "_locked", locked)
    argv = [command, "--format", "json"]
    if command == "audit":
        argv += ["--cache-census-state", str(tmp_path / "census.json")]
    _cli(root, *argv)
    assert reads["outside"] >= 7, reads
    assert reads["inside"] == 0, reads


def test_event_appended_after_the_prewarm_is_still_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    _add_events(root, 2)
    monkeypatch.setattr(wrkslots_cli, "_EVENT_MEMO_STABLE_NS", 0)
    config = wrkslots_cli._load_config(str(root), None)
    original_locked = wrkslots_cli._locked
    appended = {"done": False}

    @contextlib.contextmanager
    def locked(subject: Path, **kwargs: object) -> Iterator[None]:
        if subject.name == "ACTIVE" and not appended["done"]:
            # A writer lands between the unlocked prewarm and the locked read.
            appended["done"] = True
            wrkslots_cli._write_event_file(
                config, config.machine, "slot-held", {"generation": 1, "reason": "late", "slot": "slot-a"}
            )
        with original_locked(subject, **kwargs):  # type: ignore[arg-type]
            yield

    monkeypatch.setattr(wrkslots_cli, "_locked", locked)
    payload = json.loads(_cli(root, "doctor", "--format", "json"))
    assert appended["done"]
    # The fixture's import event, two hold/unhold pairs and the late hold.
    assert payload["event_counts"] == {"node-a": 6}, payload["event_counts"]
    rows = {str(row["slot"]): row for row in payload["active"]}
    assert rows["slot-a"]["hold_reason"] == "late", rows["slot-a"]


def test_status_leaves_no_context_behind_for_its_caller(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    _cli(root, "status", "--format", "json")
    # A later process observation in this process must report, not collect into
    # the finished status's list.
    assert wrkslots_cli._TEARING_DOWN_PIDNS.get() is None
    assert wrkslots_cli._VERBOSE_DIAGNOSTICS.get() is False


def _liveness_hook(
    monkeypatch: pytest.MonkeyPatch, during: Callable[[int, list[str]], None]
) -> list[list[str]]:
    """Run ``during`` inside each liveness query, i.e. in a reader's measuring phase."""

    original = wrkslots_cli._owner_liveness_states
    calls: list[list[str]] = []

    def liveness(config: object, records: object) -> object:
        slots = sorted(record.slot for record in records)  # type: ignore[attr-defined]
        calls.append(slots)
        during(len(calls), slots)
        return original(config, records)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_owner_liveness_states", liveness)
    return calls


def _set_task(root: Path, task: str) -> None:
    document = _read_active(root)
    cast(list[dict[str, object]], document["slots"])[0]["task"] = task
    document["revision"] = cast(int, document["revision"]) + 1
    _write_active(root, document)


def _writer_lock_free(root: Path) -> bool:
    try:
        with wrkslots_cli._locked(root / "worktrees" / "ACTIVE", exclusive=True, wait_seconds=0):
            return True
    except wrkslots_cli.Refusal:
        return False


@pytest.mark.parametrize("command", ["status", "doctor"])
def test_reader_remeasures_a_row_changed_while_it_measured(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, command: str
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)
    free: list[bool] = []

    def during(call: int, slots: list[str]) -> None:
        # The measuring phase holds no registry lock: a writer gets in and changes one row.
        free.append(_writer_lock_free(root))
        if call == 1:
            _set_task(root, "changed while measuring")

    calls = _liveness_hook(monkeypatch, during)
    payload = json.loads(_cli(root, command, "--format", "json"))
    rows = {str(row["slot"]): row for row in payload["active"]}
    # Published from the registry as it is now, not a mix of the two instants.
    assert rows["slot-a"]["task"] == "changed while measuring", rows["slot-a"]
    # The repeat asks liveness again only for the row that changed.
    assert calls == [["slot-a", "slot-b"], ["slot-a"]], calls
    assert free == [True, True], free


@pytest.mark.parametrize("command", ["status", "doctor"])
def test_reader_sees_an_event_written_while_it_measured(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, command: str
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)
    _add_events(root, 1)

    def during(call: int, slots: list[str]) -> None:
        if call == 1:
            # Only the event log changes: the row itself is untouched.
            _cli(root, "hold", "--reason", "placed while measuring", "slot-a")

    calls = _liveness_hook(monkeypatch, during)
    payload = json.loads(_cli(root, command, "--format", "json"))
    rows = {str(row["slot"]): row for row in payload["active"]}
    assert rows["slot-a"]["hold_reason"] == "placed while measuring", rows["slot-a"]
    # Nothing a liveness answer depends on changed, so it is not asked again.
    assert calls == [["slot-a", "slot-b"]], calls


@pytest.mark.parametrize("command", ["status", "doctor"])
def test_reader_measures_under_the_lock_when_the_registry_keeps_changing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, command: str
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)
    free: list[bool] = []

    def during(call: int, slots: list[str]) -> None:
        # A well-behaved writer: it changes the row whenever it can take the lock.
        allowed = _writer_lock_free(root)
        free.append(allowed)
        if allowed:
            _set_task(root, f"change {call}")

    calls = _liveness_hook(monkeypatch, during)
    payload = json.loads(_cli(root, command, "--format", "json"))
    rows = {str(row["slot"]): row for row in payload["active"]}
    retries = wrkslots_cli._PHASED_READ_RETRIES
    # One measurement, the retries, then the last one under the lock.
    assert len(calls) == retries + 2, calls
    assert free == [True] * (retries + 1) + [False], free
    assert rows["slot-a"]["task"] == f"change {retries + 1}", rows["slot-a"]


def test_registry_read_identity_sees_content_not_metadata(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    config = wrkslots_cli._load_config(str(root), None)
    before = wrkslots_cli._registry_read_identity(config)
    active = _active(root)
    status = active.stat()
    document = _read_active(root)
    cast(list[dict[str, object]], document["slots"])[0]["task"] = "same length!"[: len("do work")]
    active.write_text(json.dumps(document))
    os.utime(active, ns=(status.st_atime_ns, status.st_mtime_ns))
    assert wrkslots_cli._registry_read_identity(config) != before
    # Lock files, which every reader opens, do not count.
    (root / "worktrees" / "ACTIVE.global.intent.lock").write_text("")
    after = wrkslots_cli._registry_read_identity(config)
    assert wrkslots_cli._registry_read_identity(config) == after


def _add_worktree_slot(root: Path, name: str, **changes: object) -> None:
    """Another slot like slot-a, on its own branch and pushed, with ``changes`` to its row."""

    base = root.parent
    document = _read_active(root)
    slots = cast(list[dict[str, object]], document["slots"])
    record = copy.deepcopy(slots[0])
    checkout = cast(list[dict[str, object]], record["checkouts"])[0]
    branch = f"suite/{name}"
    _git(
        base / "upstream-repo", "worktree", "add", "-q", "-b", branch,
        str(root / "worktrees" / name / "repo"), str(checkout["head"]),
    )
    _git(base / "upstream-repo", "push", "-q", "origin", branch)
    record.update({"slot": name, "agent": f"agent-{name}", **changes})
    checkout.update({"path": f"worktrees/{name}/repo", "branch": branch})
    slots.append(record)
    _write_active(root, document)


def _verdict_mix_fixture(base: Path) -> Path:
    root = _fixture(base)
    for index in range(4):
        _add_worktree_slot(root, f"plain-{index}")
    _add_worktree_slot(root, "fresh", heartbeat_at=wrkslots_cli._utc_now())
    _add_worktree_slot(root, "held")
    _add_worktree_slot(root, "moved")
    _add_worktree_slot(root, "absent")
    _git(root / "worktrees" / "moved" / "repo", "checkout", "-q", "-b", "elsewhere")
    shutil.rmtree(root / "worktrees" / "absent")
    _git(base / "upstream-repo", "worktree", "prune")
    _cli(root, "hold", "--reason", "fixture", "held")
    return root


def _comparable_audit(root: Path, tmp_path: Path, jobs: int) -> dict[str, object]:
    payload: dict[str, object] = json.loads(
        _cli(
            root, "audit", "--format", "json", "--jobs", str(jobs),
            "--cache-census-state", str(tmp_path / f"census-{jobs}.json"),
        )
    )
    payload.pop("metrics")
    payload.pop("disk")  # free space moves between the two runs
    for row in cast(list[dict[str, object]], payload["slots"]):
        row.pop("heartbeat_age_seconds", None)
        row["reasons"] = [
            re.sub(r"\b\d+s\b", "Ns", str(reason))
            for reason in cast(list[object], row["reasons"])
        ]
    return payload


@pytest.mark.parametrize("census_fails", [False, True])
def test_pooled_audit_matches_serial_audit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, census_fails: bool
) -> None:
    root = _verdict_mix_fixture(tmp_path / "case")
    if census_fails:
        def census(paths: object, **_options: object) -> object:
            raise wrkslots_cli.Refusal("fixture census refused")

        monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)
    original = wrkslots_cli._audit_record
    threads: dict[int, set[str]] = {}
    jobs_now = {"value": 0}

    def judged(*args: object, **kwargs: object) -> object:
        threads.setdefault(jobs_now["value"], set()).add(threading.current_thread().name)
        time.sleep(0.05)  # long enough that a pool overlaps rows
        return original(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_audit_record", judged)
    results = {}
    for jobs in (1, 4):
        jobs_now["value"] = jobs
        results[jobs] = _comparable_audit(root, tmp_path, jobs)
    assert results[4] == results[1]
    verdicts = {
        str(row["slot"]): row["verdict"]
        for row in cast(list[dict[str, object]], results[1]["slots"])
    }
    # Without a census nothing can be cleared for removal or recovery.
    expected = {"BLOCKED", "HELD"} | (
        {"UNKNOWN"} if census_fails else {"DELETABLE", "RECOVERABLE"}
    )
    assert expected <= set(verdicts.values()), verdicts
    assert threads[1] == {threading.current_thread().name}, threads
    assert len(threads[4]) > 1, threads


def test_audit_refuses_jobs_below_one(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _fixture(tmp_path / "case")
    code = wrkslots_cli.main(
        ["--project-root", str(root), "audit", "--format", "json", "--jobs", "0"]
    )
    assert code != 0
    assert "--jobs must be at least 1" in capsys.readouterr().err


def test_other_edition_accepts_the_same_jobs_option(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")

    def run(jobs: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                str(rust_wrkslotsd()), "audit-text", "--root", str(root),
                "--cache-census-state", str(tmp_path / "rust-census.json"),
                "--format", "json", "--jobs", jobs,
            ],
            cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        )

    accepted = run("4")
    assert accepted.returncode == 0, accepted.stderr
    assert json.loads(accepted.stdout)["slots"]
    refused = run("0")
    assert refused.returncode != 0
    assert "--jobs must be at least 1" in refused.stderr
