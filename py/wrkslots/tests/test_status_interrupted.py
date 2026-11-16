"""Status and doctor report an interrupted mutation (#1700).

A schema-2 finish journal (validate_complete false) whose actor has
exited must be visible in status and doctor -- slot, kind, schema,
actor PID, liveness by start-time identity, age, and the exact recover
command -- instead of surfacing only when an unrelated command later
refuses.  A live actor reads as in progress (#1121).  An unrelated
slot's mutation proceeds past a dead actor's finish (#912 completed
for finish journals); the journal's own slot, and global mutations,
still refuse, naming the blocking slot and the recover command.  The
finish is never auto-recovered: resuming it archives and removes the
slot, so only an explicit recover does that.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    create,
    make_project,
    prepare_removal_journal,
)


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _config(project: Path) -> cli.Config:
    return cli._load_config(str(project), "testhost")


def _identity_obj(pid: int) -> dict[str, object]:
    identity = cli._read_process_identity(pid)
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def test_status_reports_dead_finish_as_interrupted(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    journal = prepare_removal_journal(project)
    raw = json.loads(journal.read_text(encoding="utf-8"))
    assert raw["kind"] == "finish"
    assert raw["schema"] == 2
    assert raw["validate_complete"] is False

    code = cli.main(["--project-root", str(project), "status"])
    out = capsys.readouterr().out
    assert code == 0
    assert "INTERRUPTED slot=slot01 kind=finish schema=2" in out
    assert "alive=false" in out
    assert f"actor_pid={raw['owner']['pid']}" in out
    assert (
        'recover="wrkslots recover --coordinator-pid PID --slot slot01"'
        in out
    )

    code = cli.main(
        ["--project-root", str(project), "status", "--format", "json"]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    (report,) = payload["interrupted_journals"]
    assert report["slot"] == "slot01"
    assert report["kind"] == "finish"
    assert report["schema"] == 2
    assert report["actor_pid"] == raw["owner"]["pid"]
    assert report["alive"] is False
    assert report["state"] == "interrupted"
    assert isinstance(report["age_seconds"], int)
    assert (
        report["recover_command"]
        == "wrkslots recover --coordinator-pid PID --slot slot01"
    )


def test_status_reports_live_finish_as_in_progress(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    journal = prepare_removal_journal(project)
    config = _config(project)
    raw = json.loads(journal.read_text(encoding="utf-8"))
    raw["owner"] = _identity_obj(os.getpid())
    cli._write_journal(config, raw)

    code = cli.main(["--project-root", str(project), "status"])
    out = capsys.readouterr().out
    assert code == 0
    assert "IN-PROGRESS slot=slot01 kind=finish" in out
    assert "alive=true" in out
    assert "INTERRUPTED slot=slot01" not in out


def test_doctor_reports_dead_finish(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    prepare_removal_journal(project)

    code = cli.main(["--project-root", str(project), "doctor"])
    out = capsys.readouterr().out
    assert code == 0
    assert "interrupted-mutation: slot01:" in out
    assert "INTERRUPTED slot=slot01 kind=finish" in out
    assert "alive=false" in out

    code = cli.main(
        ["--project-root", str(project), "doctor", "--format", "json"]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    kinds = [item["kind"] for item in payload["findings"]]
    assert "interrupted-mutation" in kinds


def test_unrelated_mutation_proceeds_past_dead_finish(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(
            project, slot="slot02", agent="codex-2", branch="codex/task2"
        ).returncode
        == 0
    )
    journal = prepare_removal_journal(project)
    config = _config(project)

    # The dead finish of slot01 does not block slot02, at the gate...
    cli._assert_no_journal(config, target_slot="slot02")
    assert "proceeding beside the interrupted finish of slot 'slot01'" in (
        capsys.readouterr().err
    )
    # ...or through the real hold command for slot02.
    code = cli.main(
        [
            "--project-root", str(project), "hold", "slot02",
            "--reason", "keep",
        ]
    )
    assert code == 0
    capsys.readouterr()

    # The journal's own slot still refuses, with the exact command.
    with pytest.raises(cli.Refusal) as caught:
        cli._assert_no_journal(config, target_slot="slot01")
    message = str(caught.value)
    assert "interrupted mutation for slot 'slot01'" in message
    assert (
        "run 'wrkslots recover --coordinator-pid PID --slot slot01' first"
        in message
    )
    # A global mutation (no target slot) refuses the same way.
    with pytest.raises(cli.Refusal) as caught:
        cli._assert_no_journal(config)
    assert "interrupted mutation for slot 'slot01'" in str(caught.value)
    # Nothing auto-recovered the finish: its journal is still there.
    assert journal.is_file()
