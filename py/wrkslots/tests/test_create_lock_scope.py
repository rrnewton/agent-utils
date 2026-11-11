"""create reuses its lock-free storage report on the locked path (#1287 retire-lock-narrow).

After #1276 a plain create validates and scans the fleet with no lock
held. But when a journal was outstanding -- an interrupted mutation's,
a live writer's, or a validation batch's seal -- `create` deferred to
its original locked path, which recomputed the fleet-wide storage
report while holding the exclusive flock on the project root. On a
measured host two back-to-back creates held that flock for 2 min 9 s
and 2 min 35 s this way, printing dozens of retained storage
inconsistencies. The journal classification still needs the lock; the
storage report does not, so it is computed lock-free in every round and
reused under the lock. This test counts the instrumented operations
that run while the exclusive lock is held, never wall time.
"""

from __future__ import annotations

import pytest
from pathlib import Path

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    checkout,
    create,
    create_journal_path,
    make_project,
)
from wrkslots.tests.test_prevalidated_mutation import (
    _LockLedger,
    _create_argv,
    _row,
    _treat_every_event_as_settled,
)


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_create_beside_an_interrupted_create_scans_no_fleet_under_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    interrupted = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/task2",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    assert create_journal_path(project, "slot02").exists()
    _treat_every_event_as_settled(monkeypatch)
    ledger = _LockLedger(monkeypatch, project)

    # The interrupted create keeps the locked classification path...
    assert cli.main(_create_argv(project)) == 0, capsys.readouterr().err
    assert _row(project, "slot03") is not None
    assert checkout(project, "slot03").is_dir()
    assert ledger.count("validate", held=True) >= 1
    assert ledger.count("classify", held=True) >= 1
    # ...but the fleet storage report ran lock-free and was reused:
    # no storage scan of any rows ran while the flock was held.
    assert ledger.count("storage-scan", held=False) >= 1
    assert ledger.storage_rows_under_lock() == []
