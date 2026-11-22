"""Absent-validate-row pre-checks run before the registry locks (#2088).

`recover-absent-validate-rows --apply` used to take every registry
lock, parse the whole event history, and only then refuse at the
Git-registration check. The cheap refusal checks now run from an
event-index-proven ACTIVE snapshot *before* the locks. The test runs
the apply command while the caller holds every registry lock: a
refusable row must return promptly with the same refusal, proving no
lock is taken and no locked path is entered, instead of waiting.
"""

from __future__ import annotations

import pytest

from pathlib import Path

from wrkslots import cli as wrkslots
from wrkslots.tests.test_eden import (
    _absent_args,
    _absent_project,
    _eden_validate_record,
    _write_row,
)


def test_refusable_row_never_takes_the_registry_locks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    _write_row(config, record)
    # The Git-registration/storage check is the seam the field report
    # names; refusable is what matters here, and it must fire pre-lock.
    def refuse(_config: object, _records: object) -> object:
        raise wrkslots.Refusal("Git still registers absent validation checkout")

    monkeypatch.setattr(
        wrkslots, "_assert_absent_validate_rows_storage", refuse
    )
    input_path = root / "absent-validate-rows.json"
    input_path.write_text(
        '{"schema": 1, "rows": [{"machine": "node-a", "slot": '
        f'"{record.slot}", "generation": {record.generation}, '
        f'"record_sha256": '
        f'"{wrkslots._record_sha256(record)}"}}]}}'
    )
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "node-a")
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots, "_absent_validate_privileged_path_match", lambda *_a, **_k: None
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())

    created = []

    def fail_if_locked(*args: object, **kwargs: object) -> object:
        created.append((args, kwargs))
        raise AssertionError("the pre-check took the registry locks")

    monkeypatch.setattr(wrkslots, "_mutation_locks", fail_if_locked)
    with pytest.raises(wrkslots.Refusal):
        wrkslots._cmd_recover_absent_validate_rows(
            _absent_args(root, input_path, apply=True)
        )
    assert created == []
