"""A live journaled mutation is busy, not interrupted (#1367).

While a removal deletes its fenced tree outside the registry lock, its
finish journal is outstanding for minutes.  Mutations that met it used to
refuse with "interrupted mutation recorded ... run 'wrkslots recover'
first" and no actor detail, because the journal check never read the
removing process the finish journal records.  These tests pin the three
classifications -- live actor, dead actor, legacy journal with no actor
generation -- and that ``--wait-lock`` waits for a live journal to clear
and then proceeds.  Every wait is on a condition (the journal clearing,
the worker finishing); timeouts only detect a hang.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
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


def _sleeper() -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def _identity_obj(pid: int) -> dict[str, object]:
    identity = cli._read_process_identity(pid)
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def test_live_removal_journal_reports_busy_not_interrupted(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    journal = prepare_removal_journal(project)
    config = _config(project)

    # The interrupted remover has exited; put a live actor in its place.
    raw = json.loads(journal.read_text(encoding="utf-8"))
    raw["owner"] = _identity_obj(os.getpid())
    cli._write_journal(config, raw)

    with pytest.raises(cli._JournalBusy) as caught:
        cli._assert_no_journal(config)
    message = str(caught.value)
    assert "busy: live 'finish' mutation" in message
    assert f"by pid {os.getpid()} since " in message
    assert raw["finished_at"] in message

    # A concurrent mutation surfaces the same busy report, exit 75.
    hold = [
        "--project-root", str(project), "hold", "slot01", "--reason", "keep",
    ]
    assert cli.main(hold) == 75
    assert "BUSY: busy: live 'finish' mutation" in capsys.readouterr().err


def test_dead_actor_journal_reports_interrupted(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    prepare_removal_journal(project)
    config = _config(project)

    # The removing process exited with the interrupt; nothing is live.
    with pytest.raises(cli.Refusal) as caught:
        cli._assert_no_journal(config)
    assert not isinstance(caught.value, cli._JournalBusy)
    message = str(caught.value)
    assert "interrupted mutation recorded in" in message
    assert "run 'wrkslots recover' first" in message
    assert "kind 'finish'" in message


def test_legacy_journal_without_actor_generation_gets_legacy_wording(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    journal = prepare_removal_journal(project)
    config = _config(project)

    # A journal from before the removing process was recorded (#1121).
    raw = json.loads(journal.read_text(encoding="utf-8"))
    del raw["owner"]
    cli._write_journal(config, raw)

    with pytest.raises(cli.Refusal) as caught:
        cli._assert_no_journal(config)
    message = str(caught.value)
    assert "unknown actor (legacy journal)" in message
    assert "if no wrkslots process is running, run 'wrkslots recover'" in message


def test_wait_lock_waits_for_a_live_journal_then_proceeds(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    made = create(project, slot="slot02", agent="codex-2", branch="codex/task2", bind_owner=False)
    assert made.returncode == 0, made.stderr
    journal_path = prepare_removal_journal(project)
    config = _config(project)

    sleeper = _sleeper()
    try:
        # The interrupted remover has exited; a live actor now owns its
        # finish journal, as a removal still deleting its tree would.
        raw = json.loads(journal_path.read_text(encoding="utf-8"))
        raw["owner"] = _identity_obj(sleeper.pid)
        cli._write_journal(config, raw, journal_path=journal_path)

        adopt = [
            "--project-root", str(project), "--wait-lock", "30",
            "adopt", "slot02",
            "--agent", "codex-2", "--owner-pid", str(os.getpid()),
            "--expected-generation", "1",
        ]
        started = threading.Event()
        outcome: dict[str, object] = {}

        def run_adopt() -> None:
            started.set()
            outcome["returncode"] = cli.main(adopt)

        worker = threading.Thread(target=run_adopt, daemon=True)
        worker.start()
        assert started.wait(timeout=30)
        # The adopt is waiting on the live journal, not refused.
        assert worker.is_alive()
        assert journal_path.exists()

        # The journaled mutation finishes and clears its journal.
        cli._clear_journal(config, raw, journal_path=journal_path)

        worker.join(timeout=30)
        assert not worker.is_alive()
        assert outcome["returncode"] == 0
    finally:
        sleeper.terminate()
        sleeper.wait(timeout=30)
