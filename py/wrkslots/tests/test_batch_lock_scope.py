"""remove-agent-batch commits one record at a time under the flock (#1367, folding #1213).

Each batch item is validated and planned with no lock held (fleet
validation, census, salvage derivation), the registry lock is taken only
to re-check that slot's identity and commit its journal/fence/record
change, and the slow work (fenced deletion, salvage publication) runs
outside it. Counted, never timed, as in #1287.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import cast

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import dead_agent_batch_project
from wrkslots.tests.test_retire_lock_scope import _LockLedger

_NEVER_HELD = ("validate", "storage-scan", "slot-unused", "salvage", "finish-paths", "state-home")


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_remove_agent_batch_validates_nothing_under_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, slots = dead_agent_batch_project(tmp_path, count=2)
    ledger = _LockLedger(monkeypatch, project)

    code = cli.main(
        [
            "--project-root",
            str(project),
            "remove-agent-batch",
            "--coordinator-pid",
            str(os.getpid()),
            *(f"--slot={slot}=1" for slot in slots),
            "--format",
            "json",
        ]
    )

    assert code == 0, capsys.readouterr().err
    payload = json.loads(capsys.readouterr().out)
    assert [item["slot"] for item in cast(list[dict[str, object]], payload["removed"])] == slots
    for label in _NEVER_HELD:
        assert ledger.count(label, held=True) == 0, (label, ledger.calls)
    for label in ("validate", "slot-unused", "salvage", "finish-paths"):
        assert ledger.count(label, held=False) > 0, (label, ledger.calls)
    assert ledger.pregather_probes == [False, False]


def test_plain_remove_validates_nothing_under_the_configuration_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Plain remove gets the same lock-free treatment (#1367 piece 2b).

    The project-root configuration flock (the lock ledger's probe) is
    taken only to read/validate config and to commit the slot's record;
    validation, census, salvage and deletion all run with it released.
    """
    project, _repository, slots = dead_agent_batch_project(tmp_path, count=1)
    ledger = _LockLedger(monkeypatch, project)

    code = cli.main(
        [
            "--project-root",
            str(project),
            "remove",
            slots[0],
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert code == 0, capsys.readouterr().err
    for label in _NEVER_HELD:
        assert ledger.count(label, held=True) == 0, (label, ledger.calls)
    for label in ("validate", "slot-unused", "salvage", "finish-paths"):
        assert ledger.count(label, held=False) > 0, (label, ledger.calls)
    assert ledger.pregather_probes == [False]
