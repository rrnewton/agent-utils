"""`#2390 gate-admission-dir-out-of-tmp`: gate admission state leaves the host temp dir.

The new default is a per-user state directory. While the old host-temp directory is a real
directory gates keep using it; `gate admission-dir --migrate` moves it at a quiet moment and
leaves a symlink, so an older gate.py and readers of the old default share one admission domain.
Every path here is redirected into tmp_path: no test touches the real old path. Waits are on
conditions with a generous hang backstop.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from types import ModuleType

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = REPO_ROOT / "scripts"
HANG_BACKSTOP_S = 120.0


@pytest.fixture
def gate(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> ModuleType:
    if str(SCRIPTS) not in sys.path:
        sys.path.insert(0, str(SCRIPTS))
    spec = importlib.util.spec_from_file_location("_gate_admission_dir_under_test", SCRIPTS / "gate.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    monkeypatch.setattr(module, "LEGACY_DIR", tmp_path / "tmp" / "dev-agent-utils-gate-admission")
    monkeypatch.setattr(module, "DEFAULT_DIR", tmp_path / "state" / "gate-admission")
    (tmp_path / "tmp").mkdir()
    monkeypatch.delenv("GATE_ADMISSION_DIR", raising=False)
    return module


def _paths(gate: ModuleType) -> tuple[Path, Path]:
    legacy: Path = gate.LEGACY_DIR
    new: Path = gate.DEFAULT_DIR
    return legacy, new


def test_state_dir_follows_the_override_the_unmigrated_directory_or_the_new_default(
    gate: ModuleType, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    legacy, new = _paths(gate)
    assert gate.state_dir() == new and not new.exists(), "resolution never creates anything"
    legacy.mkdir()
    assert gate.state_dir() == legacy, "a real old directory is still in use until migrated"
    legacy.rmdir()
    new.mkdir(parents=True)
    legacy.symlink_to(new)
    assert gate.state_dir() == new
    monkeypatch.setenv("GATE_ADMISSION_DIR", str(tmp_path / "chosen"))
    assert gate.state_dir() == tmp_path / "chosen"


def test_a_new_admission_leaves_a_symlink_at_the_free_old_path(gate: ModuleType) -> None:
    legacy, new = _paths(gate)
    admission = gate.Admission(gate.state_dir(), 2)
    assert admission.root == new
    assert legacy.is_symlink() and legacy.resolve() == new.resolve()


def test_a_real_old_directory_made_first_wins_so_there_is_one_domain(gate: ModuleType) -> None:
    legacy, new = _paths(gate)
    legacy.mkdir()  # an older gate.py made it just before this one settled its root
    assert gate.settle_admission_root(new) == legacy
    assert not legacy.is_symlink()


def test_an_older_gate_through_the_old_path_shares_the_lock_and_the_queue(gate: ModuleType) -> None:
    legacy, new = _paths(gate)
    current = gate.Admission(gate.state_dir(), 2)
    older = gate.Admission.__new__(gate.Admission)
    # What an older gate.py does: open the old default path directly, with no settling.
    older.root, older.slots, older._slots_problem = legacy, 2, None
    older._lock = (legacy / "admission.lock").open("a+")
    assert os.fstat(older._lock.fileno()).st_ino == os.fstat(current._lock.fileno()).st_ino
    ticket = {"id": "older-1", "pid": os.getpid(), "cls": "shared", "queued_at": time.time()}
    (legacy / "queue" / "older-1.json").write_text(json.dumps(ticket))
    assert [data["id"] for _, data in current.tickets()] == ["older-1"]
    older._lock.close()


def test_the_lock_is_reopened_when_its_file_was_replaced(gate: ModuleType) -> None:
    admission = gate.Admission(gate.state_dir(), 2)
    lock = admission.root / "admission.lock"
    replacement = admission.root / "admission.lock.new"
    replacement.write_text("")
    os.replace(replacement, lock)
    admission._locked()
    try:
        assert os.fstat(admission._lock.fileno()).st_ino == os.stat(lock).st_ino
    finally:
        admission._unlocked()


def _old_directory(legacy: Path) -> None:
    for folder in ("queue", "runs", "deferred"):
        (legacy / folder).mkdir(parents=True)
    (legacy / "admission.lock").write_text("")
    (legacy / "hold.json").write_text('{"until": 1, "reason": "kept"}')
    (legacy / "history.jsonl").write_text('{"id": "old"}\n')


def test_migration_when_idle_moves_the_state_and_leaves_a_symlink(gate: ModuleType) -> None:
    legacy, new = _paths(gate)
    _old_directory(legacy)
    new.mkdir(parents=True)
    (new / "history.jsonl").write_text('{"id": "new"}\n')
    assert gate.migrate_admission_dir(legacy, new) == 0
    assert legacy.is_symlink() and legacy.resolve() == new.resolve()
    assert (new / "hold.json").read_text() == '{"until": 1, "reason": "kept"}'
    assert (new / "history.jsonl").read_text().splitlines() == ['{"id": "new"}', '{"id": "old"}']
    assert not list(legacy.parent.glob("dev-agent-utils-gate-admission.moved-*"))
    assert gate.state_dir() == new


def test_migration_refuses_while_an_old_entry_is_live(gate: ModuleType) -> None:
    legacy, new = _paths(gate)
    _old_directory(legacy)
    (legacy / "runs" / "live.json").write_text(json.dumps({"id": "live", "pid": os.getpid()}))
    assert gate.migrate_admission_dir(legacy, new) == 1
    assert legacy.is_dir() and not legacy.is_symlink()
    assert (legacy / "runs" / "live.json").exists()


def test_migration_refuses_while_another_process_holds_the_old_lock_open(
    gate: ModuleType, tmp_path: Path
) -> None:
    legacy, new = _paths(gate)
    _old_directory(legacy)
    ready = tmp_path / "ready"
    holder = subprocess.Popen(
        [sys.executable, "-c",
         "import sys; f = open(sys.argv[1], 'a+'); open(sys.argv[2], 'w').close(); sys.stdin.read()",
         str(legacy / "admission.lock"), str(ready)],
        stdin=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + HANG_BACKSTOP_S
        while not ready.exists():
            assert time.monotonic() < deadline, "hang backstop: the lock holder never started"
            time.sleep(0.05)
        assert gate.migrate_admission_dir(legacy, new) == 1
        assert legacy.is_dir() and not legacy.is_symlink()
    finally:
        assert holder.stdin is not None
        holder.stdin.close()
        holder.wait(timeout=HANG_BACKSTOP_S)


def test_a_recreated_old_path_during_the_move_aborts_and_stays_in_use(
    gate: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> None:
    legacy, new = _paths(gate)
    _old_directory(legacy)
    real_rename = os.rename

    def rename_then_race(source: object, target: object) -> None:
        real_rename(str(source), str(target))
        legacy.mkdir()  # another process recreated the old path in the instant

    monkeypatch.setattr(gate.os, "rename", rename_then_race)
    assert gate.migrate_admission_dir(legacy, new) == 1
    assert legacy.is_dir() and not legacy.is_symlink()
    assert gate.state_dir() == legacy


def test_the_subcommand_reports_and_refuses_to_migrate_an_override(
    gate: ModuleType, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    legacy, _new = _paths(gate)
    legacy.mkdir()
    assert gate.run_admission_dir([]) == 0
    assert "real directory (not migrated)" in capsys.readouterr().out
    monkeypatch.setenv("GATE_ADMISSION_DIR", str(tmp_path / "chosen"))
    assert gate.run_admission_dir(["--migrate"]) == 2
    assert legacy.is_dir() and not legacy.is_symlink()


def test_wrkslots_reads_the_directory_gate_py_uses(
    gate: ModuleType, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The wrkslots reader resolves like gate.py: the override, else a real old directory,
    else the per-user state directory (checked against a fake home and the real old-path rule)."""
    sys.path.insert(0, str(REPO_ROOT / "py"))
    from wrkslots import cli as wrkslots

    monkeypatch.setenv("GATE_ADMISSION_DIR", str(tmp_path / "chosen"))
    assert wrkslots._gate_admission_dir() == tmp_path / "chosen"
    monkeypatch.delenv("GATE_ADMISSION_DIR")
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    legacy = Path("/tmp/dev-agent-utils-gate-admission")
    expected = legacy if legacy.is_dir() and not legacy.is_symlink() else (
        tmp_path / "home" / ".local" / "state" / "agent-utils" / "gate-admission")
    assert wrkslots._gate_admission_dir() == expected
