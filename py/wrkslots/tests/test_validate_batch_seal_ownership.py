"""A validation cleanup retires only the seal file it published."""

from __future__ import annotations

import dataclasses
import json
import os
from collections.abc import Sequence
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    allow_test_host_for_absent_validate_recovery,
    create,
    make_project,
    make_validate_row_absent,
    mark_owner_dead,
    raw_command_with_census_authority_stub,
    remove_completed_validation,
    run_absent_validate_recovery,
    set_liveness,
    stub_validate_batch_censuses,
    write_absent_validate_input,
)


def _prepare(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, Path, Path]:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    config = wrkslots._load_config(str(project), "testhost")
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    return (
        project,
        wrkslots._slot_directory(config, "target", "validate"),
        wrkslots._validate_batch_seal_journal_path(config),
    )


def _remove_command() -> list[str]:
    return [
        "remove",
        "target",
        "--validate-complete",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    ]


def _census_hook(
    monkeypatch: pytest.MonkeyPatch, hook: object
) -> None:
    def shared(
        _paths: Sequence[Path],
        *,
        budget: wrkslots._ReadOnlyCommandBudget | None = None,
        include_owner_cgroups: bool = True,
    ) -> wrkslots._ProcessPathCensus:
        del budget
        assert callable(hook)
        hook()
        return wrkslots._ProcessPathCensus(
            (), (), owner_cgroup_complete=include_owner_cgroups
        )

    monkeypatch.setattr(wrkslots, "_capture_process_path_census", shared)


def test_completed_remove_retires_its_own_seal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, target, seal = _prepare(tmp_path, monkeypatch)
    observed: list[bool] = []
    _census_hook(monkeypatch, lambda: observed.append(seal.is_file()))

    assert remove_completed_validation(project) == 0, capsys.readouterr().err

    assert observed == [True]
    assert not target.exists()
    assert not seal.exists()
    assert active_slots(project) == []


def test_concurrent_remove_leaves_the_running_invocations_seal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, target, seal = _prepare(tmp_path, monkeypatch)
    observed: list[tuple[int, str, bytes, bytes | None, int]] = []

    def second_invocation() -> None:
        # The first invocation holds no lock during this census, and its
        # finish journal does not exist yet.
        sealed = seal.read_bytes()
        second = raw_command_with_census_authority_stub(
            project, *_remove_command()
        )
        observed.append(
            (
                second.returncode,
                second.stderr,
                sealed,
                seal.read_bytes() if seal.exists() else None,
                target.stat().st_mode & 0o777,
            )
        )

    _census_hook(monkeypatch, second_invocation)

    assert remove_completed_validation(project) == 0, capsys.readouterr().err

    assert len(observed) == 1
    returncode, stderr, sealed, after, mode = observed[0]
    assert returncode == 3
    assert "interrupted validation-batch seal recorded" in stderr
    assert after == sealed
    assert mode == 0o700
    assert not target.exists()
    assert not seal.exists()
    assert active_slots(project) == []


def test_remove_leaves_a_seal_that_replaced_its_own(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, target, seal = _prepare(tmp_path, monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    foreign: list[bytes] = []

    def replace_seal() -> None:
        # Another operation retires this invocation's seal and publishes its
        # own while this invocation holds no lock.
        own = json.loads(seal.read_text(encoding="utf-8"))
        seal.unlink()
        wrkslots._write_validate_batch_seal_journal(
            config, {**own, "created_at": "2000-01-01T00:00:00+00:00", "targets": []}
        )
        foreign.append(seal.read_bytes())

    _census_hook(monkeypatch, replace_seal)

    assert remove_completed_validation(project) == 3
    assert "durable seal journal does not match" in capsys.readouterr().err

    assert len(foreign) == 1
    assert seal.read_bytes() == foreign[0]
    assert target.is_dir()
    assert [row["slot"] for row in active_slots(project) if isinstance(row, dict)] == [
        "target"
    ]


@pytest.mark.parametrize(
    "boundary",
    ("after-validate-batch-seal-journal", "after-validate-batch-seal-target"),
)
def test_crashed_invocations_seal_waits_for_explicit_recovery(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    boundary: str,
) -> None:
    project, target, seal = _prepare(tmp_path, monkeypatch)
    original = target.stat().st_mode & 0o777
    assert original != 0o700
    crashed = raw_command_with_census_authority_stub(
        project,
        *_remove_command(),
        env={"WRKSLOTS_TEST_INTERRUPT": boundary},
    )
    assert crashed.returncode == 86, crashed.stderr
    sealed = seal.read_bytes()
    mode = target.stat().st_mode & 0o777

    assert remove_completed_validation(project) == 3
    assert "interrupted validation-batch seal recorded" in capsys.readouterr().err
    assert seal.read_bytes() == sealed
    assert target.stat().st_mode & 0o777 == mode

    recovered = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )
    assert recovered.returncode == 0, recovered.stderr
    assert not seal.exists()
    assert target.stat().st_mode & 0o777 == original

    assert remove_completed_validation(project) == 0, capsys.readouterr().err
    assert not target.exists()
    assert not seal.exists()
    assert active_slots(project) == []


@pytest.mark.parametrize("failure", ("before-replace", "after-replace", "read-back"))
def test_failed_seal_rewrite_still_retires_the_invocations_seal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failure: str,
) -> None:
    project, target, seal = _prepare(tmp_path, monkeypatch)
    original = target.stat().st_mode & 0o777
    real = wrkslots._write_validate_batch_seal_journal
    real_read = wrkslots._read_regular_file_identity
    writes: list[bytes | None] = []
    armed: list[Path] = []

    def write(config: wrkslots.Config, payload: object) -> None:
        # The first write publishes the empty seal; the second adds the target
        # and fails, for example on a full disk.
        assert isinstance(payload, dict)
        if not writes:
            real(config, payload)
            writes.append(seal.read_bytes())
            return
        if failure != "before-replace":
            real(config, payload)
        writes.append(seal.read_bytes())
        if failure == "read-back":
            # The write succeeds; reading the published seal back fails once.
            armed.append(seal)
            return
        raise wrkslots.Refusal("simulated seal write failure")

    def read(
        path: Path, label: str, limit: int
    ) -> tuple[bytes, wrkslots._RegularFileIdentity]:
        if path in armed:
            armed.remove(path)
            raise wrkslots.Refusal("simulated seal write failure")
        return real_read(path, label, limit)

    monkeypatch.setattr(wrkslots, "_write_validate_batch_seal_journal", write)
    monkeypatch.setattr(wrkslots, "_read_regular_file_identity", read)

    assert remove_completed_validation(project) == 3
    assert "simulated seal write failure" in capsys.readouterr().err

    assert len(writes) == 2 and not armed
    assert (writes[1] == writes[0]) == (failure == "before-replace")
    assert not seal.exists()
    assert target.is_dir()
    assert target.stat().st_mode & 0o777 == original
    assert [row["slot"] for row in active_slots(project) if isinstance(row, dict)] == [
        "target"
    ]


@pytest.mark.parametrize("sealer", ("dead", "live"))
def test_seal_whose_target_was_reaped_does_not_deadlock_absent_row_recovery(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    sealer: str,
) -> None:
    """A dead invocation's seal of an already-removed target must not block recovery.

    Host state, 2026-10-04: a validation removal sealed its target, its
    authorizing coordinator (the seal's actor) exited, and the validation
    checkout cleanup then deleted the target
    directory, leaving the seal journal and an ACTIVE row whose directory is
    absent.  'recover' refused because the target was absent while its row
    remained, and 'recover-absent-validate-rows' refused because a seal
    journal existed, each waiting for the other.  With the seal's coordinator
    dead, 'recover' now retires the seal (there is no mode left to restore)
    and keeps the row, and absent-row recovery then retires the row.  While
    the seal's coordinator still runs, 'recover' still refuses and keeps the
    journal.
    """

    project, repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    config = wrkslots._load_config(str(project), "testhost")
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    seal = wrkslots._validate_batch_seal_journal_path(config)

    crashed = raw_command_with_census_authority_stub(
        project,
        *_remove_command(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert crashed.returncode == 86, crashed.stderr
    assert seal.exists()
    record = wrkslots._find_record(wrkslots._load_active(config), "target")
    make_validate_row_absent(project, repository, record)
    assert not wrkslots._slot_directory(config, "target", "validate").exists()
    # The seal records the authorizing coordinator as its actor: this live
    # test process.  The production seal named a coordinator that had exited.
    journal = json.loads(seal.read_text(encoding="utf-8"))
    actor = wrkslots._read_process_identity(os.getpid())
    if sealer == "dead":
        actor = dataclasses.replace(actor, pid=2_147_483_647)
    journal["actor"] = wrkslots._identity_to_obj(actor)
    seal.write_text(json.dumps(journal, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    sealed = seal.read_bytes()

    recovered = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )

    if sealer == "live":
        assert recovered.returncode == 3, recovered.stderr
        assert "is absent while its ACTIVE row remains" in recovered.stderr
        assert "the seal's coordinator is live" in recovered.stderr
        assert seal.read_bytes() == sealed
        assert [row.slot for row in wrkslots._load_active(config).slots] == ["target"]
        return
    assert recovered.returncode == 0, recovered.stderr
    assert "sealed target target was already absent" in recovered.stdout
    assert not seal.exists()
    assert [row.slot for row in wrkslots._load_active(config).slots] == ["target"]

    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    rows = write_absent_validate_input(project, [record])
    assert run_absent_validate_recovery(project, rows, apply=True) == 0, capsys.readouterr().err
    assert active_slots(project) == []
