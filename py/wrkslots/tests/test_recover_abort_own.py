"""A lane aborts its own half-made create without a coordinator pid (#2017).

Recover ordinarily requires the named coordinator to be an ancestor of
the caller. A lane that created a slot and wants to roll that create
back has no coordinator to name; its authority is the recorded create
journal's owner: the caller must descend from that process identity
(the process lineage that ran `create`), and `_assert_caller_process`
proves that exactly. The hook configuration supplies a hook that only
runs for validate slots, so an agent create never triggers it.
"""

from __future__ import annotations

import pytest
from pathlib import Path

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    checkout,
    command,
    create,
    create_journal_path,
    make_project,
    update_configuration,
)


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_abort_own_create_without_coordinator_pid(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    # A hook that fails leaves a half-made create journal behind.
    update_configuration(project, post_provision_hooks=["false"])

    made = create(project)
    assert made.returncode != 0, made.stderr
    journal = create_journal_path(project)
    assert journal.is_file()

    aborted = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--slot",
            "slot01",
            "--abort-create",
        ]
    )
    assert aborted == 0
    assert active_slots(project) == []
    assert not journal.exists()
    assert not checkout(project).exists()


def test_abort_without_coordinator_refuses_a_foreign_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    update_configuration(project, post_provision_hooks=["false"])
    made = create(project)
    assert made.returncode != 0, made.stderr
    journal = create_journal_path(project)
    assert journal.is_file()

    # Rewrite the recorded owner to a process identity not in the
    # invoking process ancestry: the caller must not be able to pass.
    owner = wrkslots._read_process_identity(999999 % 4096 + 1)
    import json

    raw = json.loads(journal.read_text(encoding="utf-8"))
    raw["owner"] = {
        "pid": owner.pid,
        "start_ticks": owner.start_ticks,
        "boot_id": owner.boot_id,
        "host_id": owner.host_id,
        "cgroup_path": owner.cgroup_path,
    }
    journal.write_text(json.dumps(raw))

    denied = wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--slot",
            "slot01",
            "--abort-create",
        ]
    )
    assert denied != 0
    assert journal.is_file()
    assert checkout(project).exists()
