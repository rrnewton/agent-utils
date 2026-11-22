"""Owner retire of an absent-directory row (#2083).

``remove --retire-absent-row`` deletes only the registry row, only
for the slot's verified live owner (recorded owner process live,
--agent naming the row's agent), only when the slot directory is
wholly absent and no mutation journal is outstanding. Everyone
else, and every other shape, refuses with the row preserved.
"""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import test_lifecycle as t  # type: ignore[import-not-found]  # noqa: E402


def _project(tmp_path: Path) -> Path:
    project: Path
    project, _repository, _remote = t.make_project(tmp_path)
    made = t.create(project)
    assert made.returncode == 0, made.stderr
    slot_dir = project / "worktrees" / "slot01"
    assert slot_dir.is_dir()
    shutil.rmtree(slot_dir)
    return project


def _remove_absent(project: Path, agent: str) -> object:
    return t.command(
        project, "remove", "slot01",
        "--coordinator-pid", str(os.getpid()),
        "--expected-generation", "1",
        "--retire-absent-row", "--agent", agent,
    )


def _slots(project: Path) -> list[str]:
    import json

    active = project / "worktrees" / "ACTIVE.testhost.json"
    payload = json.loads(active.read_text(encoding="utf-8"))
    return sorted(row["slot"] for row in payload["slots"])


def test_live_owner_retires_absent_row(tmp_path: Path) -> None:
    project = _project(tmp_path)
    done = _remove_absent(project, "codex-1")
    assert done.returncode == 0, done.stderr  # type: ignore[attr-defined]
    assert _slots(project) == []


def test_other_agent_cannot_retire_absent_row(tmp_path: Path) -> None:
    project = _project(tmp_path)
    done = _remove_absent(project, "other-agent")
    assert done.returncode == 3, done.stderr  # type: ignore[attr-defined]
    assert "only its own owner" in done.stderr  # type: ignore[attr-defined]
    assert _slots(project) == ["slot01"]


def test_dead_owner_cannot_retire_absent_row(tmp_path: Path) -> None:
    project, _repository, _remote = t.make_project(tmp_path)
    made = t.create(project)
    assert made.returncode == 0, made.stderr
    t.mark_owner_dead(project)
    shutil.rmtree(project / "worktrees" / "slot01")
    done = _remove_absent(project, "codex-1")
    assert done.returncode == 3, done.stderr  # type: ignore[attr-defined]
    assert "not live" in done.stderr  # type: ignore[attr-defined]
    assert _slots(project) == ["slot01"]


def test_present_directory_refuses_absent_row_retire(tmp_path: Path) -> None:
    project, _repository, _remote = t.make_project(tmp_path)
    made = t.create(project)
    assert made.returncode == 0, made.stderr
    done = _remove_absent(project, "codex-1")
    assert done.returncode == 3, done.stderr  # type: ignore[attr-defined]
    assert "is present" in done.stderr  # type: ignore[attr-defined]
    assert _slots(project) == ["slot01"]
