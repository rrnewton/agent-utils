"""Audit lock-free planning (#797).

The audit takes its registry snapshot under a short shared ACTIVE lock,
plans and censuses outside it, and revalidates before publishing.
Revalidation is per record: a registry change invalidates only the rows
whose own identity changed, appeared, or disappeared; unchanged rows
keep the verdicts measured outside the lock.
"""

from __future__ import annotations

import copy
import json
from collections.abc import Callable
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_audit_state_rust import _fixture, _python


def _active(root: Path) -> Path:
    return root / "worktrees" / "ACTIVE.node-a.json"


def _read_active(root: Path) -> dict[str, object]:
    return cast(dict[str, object], json.loads(_active(root).read_text()))


def _write_active(root: Path, document: dict[str, object]) -> None:
    _active(root).write_text(json.dumps(document))


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
    assert rows["slot-a"]["verdict"] != "UNKNOWN"
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
    assert rows["slot-a"]["verdict"] != "UNKNOWN"
    assert rows["slot-b"]["verdict"] == "UNKNOWN"
    assert any(
        "appeared during audit" in str(reason)
        for reason in cast(list[object], rows["slot-b"]["reasons"])
    )
    assert "slot-b" in cast(list[object], payload["unknown_slots"])


def test_audit_retries_a_census_discarded_by_host_churn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    _add_slot_b(root)
    original = wrkslots_cli._capture_process_path_census
    calls = 0

    def census(paths: object) -> object:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise wrkslots_cli.Refusal(
                "relevant process evidence changed during three liveness attempts"
            )
        return original(paths)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)
    payload = _python(root, tmp_path)
    rows = _rows(payload)
    assert calls == 2
    assert rows["slot-a"]["verdict"] != "UNKNOWN"
    assert rows["slot-b"]["verdict"] != "UNKNOWN"


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
    assert rows["slot-a"]["verdict"] == "UNKNOWN"
    assert any(
        "registry changed during audit" in str(reason)
        for reason in cast(list[object], rows["slot-a"]["reasons"])
    )
    assert rows["slot-b"]["verdict"] != "UNKNOWN"


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
    assert rows["slot-a"]["verdict"] == "UNKNOWN"
    assert rows["slot-b"]["verdict"] != "UNKNOWN"


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
    assert rows["slot-a"]["verdict"] == "UNKNOWN"
    assert rows["slot-b"]["verdict"] != "UNKNOWN"
