"""Audit lock-free planning (#797).

The audit takes its registry snapshot under a short shared ACTIVE lock,
plans and censuses outside it, and revalidates the registry revision
before publishing; a change in between marks every row UNKNOWN.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_audit_state_rust import _fixture, _python


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


def test_audit_registry_change_marks_rows_unknown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path / "case")
    active = root / "worktrees" / "ACTIVE.node-a.json"
    original = wrkslots_cli._capture_process_path_census

    def census(paths: object) -> object:
        document = json.loads(active.read_text())
        document["revision"] = int(document["revision"]) + 1
        active.write_text(json.dumps(document))
        return original(paths)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots_cli, "_capture_process_path_census", census)
    payload = _python(root, tmp_path)
    rows = payload["slots"]
    assert isinstance(rows, list) and rows
    for row in rows:
        assert isinstance(row, dict)
        assert row["verdict"] == "UNKNOWN"
        assert any(
            "registry changed during audit" in str(reason)
            for reason in row["reasons"]
        )
    assert payload["state"] == "unknown"
