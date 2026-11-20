"""Bounded recover-absent recovery (#1701).

The batch archives in bounded chunks with the journal cursor
advancing per chunk, so a killed batch resumes from its durable
prefix; the command reports how long it held the registry locks.
Peak RSS on the large fixtures is recorded in the issue file:
~175 MiB at 200-1024 absent rows with an empty event log, and
~216-269 MiB with 20k-50k chained events (multi-MB payloads
included) — whole-history loads, not row count, dominate.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    allow_test_host_for_absent_validate_recovery,
    create,
    make_project,
    make_validate_row_absent,
    mark_recorded_owner_dead,
    run_absent_validate_recovery,
    write_absent_validate_input,
)


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_streamed_fold_matches_materialised_fold(tmp_path: Path) -> None:
    """The streamed fold derives exactly the materialised state (#1931)."""

    project, _repository, _remote = make_project(tmp_path)
    made = create(
        project, slot="gone", agent="validate-a", branch=None,
        slot_type="validate",
    )
    assert made.returncode == 0, made.stderr
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._states_from_events_streamed(config, "testhost") == (
        wrkslots._states_from_events(config, "testhost")
    )
    # A second fold in the same memo scope resumes from the tip and
    # still derives the identical state.
    assert wrkslots._states_from_events_streamed(config, "testhost") == (
        wrkslots._states_from_events(config, "testhost")
    )


def _absent_input(project: Path, slots: list[str]) -> Path:
    audit = wrkslots.main(
        ["--project-root", str(project), "audit", "--format", "json"]
    )
    assert audit == 0
    return Path(project) / "absent.json"


def test_recovery_reports_lock_seconds(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(
        project, slot="gone", agent="validate-a", branch=None,
        slot_type="validate",
    )
    assert made.returncode == 0, made.stderr
    record = mark_recorded_owner_dead(project, "gone")
    make_validate_row_absent(project, repository, record)
    config = wrkslots._load_config(str(project), "testhost")
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    assert run_absent_validate_recovery(
        project, input_path, apply=True, output_format="json"
    ) == 0
    payload = json.loads(capsys.readouterr().out)
    assert [r["outcome"] for r in payload["rows"]] == ["recovered"]
    assert payload["lock_seconds"] >= 0
    state = wrkslots._load_active(config)
    assert [r.slot for r in state.slots] == []


def test_chunked_batch_resumes_from_cursor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A batch interrupted mid-chunk replay completes (#1701)."""
    project, repository, _remote = make_project(tmp_path)
    records = []
    for index in range(3):
        slot = f"gone{index}"
        made = create(
            project, slot=slot, agent=f"validate-{index}", branch=None,
            slot_type="validate",
        )
        assert made.returncode == 0, made.stderr
        record = mark_recorded_owner_dead(project, slot)
        make_validate_row_absent(project, repository, record)
        records.append(record)
    input_path = write_absent_validate_input(project, records)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    capsys.readouterr()

    # Interrupt after the first chunk's journal write, then replay:
    # the durable archive prefix is revalidated, not rewritten.
    class Interrupted(RuntimeError):
        pass

    def interrupt(point: str) -> None:
        if point == "after-absent-validate-row":
            raise Interrupted

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)
    with pytest.raises(Interrupted):
        run_absent_validate_recovery(
            project, input_path, apply=True, output_format="json"
        )
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
    capsys.readouterr()
    assert run_absent_validate_recovery(
        project, input_path, apply=True, output_format="json"
    ) == 0
    config = wrkslots._load_config(str(project), "testhost")
    assert wrkslots._load_active(config).slots == ()
