"""A validation cleanup retires only the seal file it published."""

from __future__ import annotations

import dataclasses
import json
import os
import stat
import subprocess
import sys
from collections.abc import MutableMapping, MutableSequence, Sequence
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
    set_host_context_handoff,
    set_liveness,
    start_host_context_coordinator,
    stub_validate_batch_censuses,
    terminate_process,
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
        # finish journal does not exist yet. It runs in this process, so
        # a real operator process stands in for it: the seal scan only
        # counts wrkslots processes acting on the sealed targets.
        sealed = seal.read_bytes()
        operator = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import time; time.sleep(120)",
                "-m",
                "wrkslots",
                "--project-root",
                str(project),
                "remove",
                "target",
            ]
        )
        try:
            second = raw_command_with_census_authority_stub(
                project, *_remove_command()
            )
        finally:
            operator.kill()
            operator.wait()
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
    # The first invocation is still running, so its seal is in progress,
    # never "interrupted", and recovery is not suggested (#1736).
    assert "in progress (pid " in stderr
    assert "recover" not in stderr
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
    # The crashed invocation is gone even though its coordinator (this
    # test process) lives on: the seal is interrupted, with recovery
    # named, because no wrkslots process acts on its targets (#1736).
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


@pytest.mark.parametrize("sealer", ("dead", "live", "live-with-wrkslots-operator", "live-with-unrelated-wrkslots-operator"))
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
    and keeps the row, and absent-row recovery then retires the row.

    The coordinator is often a long-lived agent session (2026-10-18: a Codex
    lane whose interrupted removal blocked every validate-run for nine hours).
    Only a wrkslots process can be part way through a sealed removal, so with
    the coordinator live the seal is retired the same way when no other
    wrkslots process is alive, and still refused while one is.
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

    operator = None
    if sealer in ("live-with-wrkslots-operator", "live-with-unrelated-wrkslots-operator"):
        # A process whose command line runs wrkslots, standing in for the
        # removal that wrote the seal. A real removal names its slot: an
        # operator on the sealed slot holds the seal open, while one on an
        # unrelated slot (2026-11-02 live-coordinator deadlock) does not.
        # sys.executable, not a "python3" launcher that could leave the real
        # interpreter behind as an orphan when only the launcher is killed.
        operator = subprocess.Popen(
            [
                sys.executable, "-c", "import time; time.sleep(120)",
                "-m", "wrkslots", "--project-root", str(project), "remove",
                "target" if sealer == "live-with-wrkslots-operator" else "other-slot",
            ]
        )
        if sealer == "live-with-wrkslots-operator":
            # The seal's recorded batch process is this operator (#1736):
            # recover must refuse while it lives.
            _rewrite_operator(seal, operator.pid)
            sealed = seal.read_bytes()
    try:
        recovered = raw_command_with_census_authority_stub(
            project,
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
        )
    finally:
        if operator is not None:
            operator.kill()
            operator.wait()

    if sealer == "live-with-wrkslots-operator":
        # Recover refuses a seal whose batch is still running (#1736).
        assert recovered.returncode == 3, recovered.stderr
        assert "in progress (pid " in recovered.stderr
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


@pytest.mark.parametrize("handoff", ("valid", "unrelated-writer"))
@pytest.mark.parametrize("target", ("absent", "present"))
def test_host_context_recover_retires_a_seal_from_a_coordinator_outside_its_ancestry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    target: str,
    handoff: str,
) -> None:
    """'recover' of a seal-only journal must accept the host-context handoff.

    Host state, 2026-10-06: the validation checkout cleanup worker's
    'remove-validate-batch' left a validation-batch seal journal, and every
    later 'validate-run' admission refused until 'wrkslots recover' retired
    it.  ci-hub/bin/wrkslots always runs 'recover' in a host-context service
    that does not descend from the requesting coordinator; it passes the
    coordinator's PID and generation plus a proof pipe whose writer descends
    from that coordinator.  Every other 'recover' path checks that handoff,
    but the seal-only branch checked direct ancestry, so through the wrapper
    it always refused with "coordinator PID N is not in the invoking process
    ancestry" and the seal could never be recovered.  A handoff whose writer
    does not descend from the coordinator must still refuse and leave the
    seal byte for byte.
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
    slot_path = wrkslots._slot_directory(config, "target", "validate")
    original_mode = stat.S_IMODE(slot_path.stat().st_mode)

    crashed = raw_command_with_census_authority_stub(
        project,
        *_remove_command(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert crashed.returncode == 86, crashed.stderr
    assert seal.exists()
    assert stat.S_IMODE(slot_path.stat().st_mode) == 0o700
    if target == "absent":
        record = wrkslots._find_record(wrkslots._load_active(config), "target")
        make_validate_row_absent(project, repository, record)
        assert not slot_path.exists()
    # The production seal named the cleanup worker's coordinator, which had
    # exited before anyone ran 'recover'.
    journal = json.loads(seal.read_text(encoding="utf-8"))
    sealer = wrkslots._read_process_identity(os.getpid())
    journal["actor"] = wrkslots._identity_to_obj(
        dataclasses.replace(sealer, pid=2_147_483_647)
    )
    seal.write_text(json.dumps(journal, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    sealed = seal.read_bytes()

    coordinator, proof_fd = start_host_context_coordinator()
    unrelated: subprocess.Popen[str] | None = None
    try:
        identity = wrkslots._read_process_identity(coordinator.pid)
        # As in the host service, the requesting coordinator is not an
        # ancestor of the process that runs 'recover'.
        with pytest.raises(wrkslots.Refusal, match="not in the invoking process ancestry"):
            wrkslots._assert_caller_process(identity, "coordinator")
        if handoff == "unrelated-writer":
            unrelated = subprocess.Popen(["sleep", "60"], text=True)
            identity = wrkslots._read_process_identity(unrelated.pid)
        set_host_context_handoff(monkeypatch, identity, proof_fd)
        capsys.readouterr()

        result = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-authorized",
                "--coordinator-pid",
                str(identity.pid),
            ]
        )
        captured = capsys.readouterr()
    finally:
        terminate_process(coordinator)
        if unrelated is not None:
            terminate_process(unrelated)
        os.close(proof_fd)

    assert [row.slot for row in wrkslots._load_active(config).slots] == ["target"]
    if handoff == "unrelated-writer":
        assert result == 3, captured.err
        assert "no live writer descended from the coordinator" in captured.err
        assert seal.read_bytes() == sealed
        if target == "present":
            assert stat.S_IMODE(slot_path.stat().st_mode) == 0o700
        return
    assert result == 0, captured.err
    assert "not in the invoking process ancestry" not in captured.err
    assert not seal.exists()
    if target == "absent":
        assert "sealed target target was already absent" in captured.out
        assert not slot_path.exists()
    else:
        assert "recovered validation-batch seals: restored=1 resolved=1" in captured.out
        assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode


# A seal its actor can no longer retire (https://github.com/rrnewton/agent-utils/issues/236).


def _crashed_seal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, Path, Path, int]:
    """A removal killed after it sealed its target, as a killed publisher leaves it."""
    project, target, seal = _prepare(tmp_path, monkeypatch)
    original = target.stat().st_mode & 0o777
    crashed = raw_command_with_census_authority_stub(
        project,
        *_remove_command(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert crashed.returncode == 86, crashed.stderr
    assert seal.is_file()
    assert target.stat().st_mode & 0o777 != original
    return project, target, seal, original


def _rewrite_actor(seal: Path, **fields: object) -> None:
    raw = json.loads(seal.read_text())
    raw["actor"].update(fields)
    if isinstance(raw.get("x"), dict) and isinstance(raw["x"].get("operator"), dict):
        raw["x"]["operator"].update(fields)
    seal.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")


def _rewrite_operator(seal: Path, pid: int) -> None:
    raw = json.loads(seal.read_text())
    raw.setdefault("x", {})["operator"] = wrkslots._identity_to_obj(
        wrkslots._read_process_identity(pid)
    )
    seal.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")


def _classify(project: Path, slot: str) -> subprocess.CompletedProcess[str]:
    return raw_command_with_census_authority_stub(
        project,
        "classify-create-journals",
        slot,
        "--slot-type",
        "validate",
        "--agent",
        f"validate-{slot}",
        "--format",
        "json",
    )


@pytest.mark.parametrize("actor", ("exited", "pid-reused", "earlier-boot"))
def test_a_dead_actors_seal_is_retired_by_the_next_create(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, actor: str
) -> None:
    """Classification passes a proven-dead actor's seal; create retires it.

    The seal records the process that authorized the removal, which in
    production is the killed publisher itself. "exited" records a process that
    has since exited; "pid-reused" names a live PID with another start time,
    so the recorded generation exited. "earlier-boot" ran on this host in
    an earlier boot, which no process survives (#1764).
    """
    project, target, seal, original = _crashed_seal(tmp_path, monkeypatch)
    if actor == "earlier-boot":
        _rewrite_actor(seal, boot_id="00000000-0000-4000-8000-000000000000")
    elif actor == "exited":
        child = subprocess.Popen(["sleep", "60"])
        try:
            exited = wrkslots._read_process_identity(child.pid)
        finally:
            child.kill()
            child.wait()
        _rewrite_actor(seal, **dataclasses.asdict(exited))
    else:
        live = wrkslots._read_process_identity(os.getpid())
        _rewrite_actor(seal, pid=live.pid, start_ticks=live.start_ticks + 1)

    classified = _classify(project, "next")
    assert classified.returncode == 0, classified.stderr

    made = create(project, slot="next", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    assert "recovered the validation-batch seal" in made.stderr
    assert not seal.exists()
    assert target.stat().st_mode & 0o777 == original


@pytest.mark.parametrize("actor", ("live", "other-host"))
def test_a_seal_whose_actor_is_not_proven_dead_here_still_binds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, actor: str
) -> None:
    """A live actor, or one from another machine, still needs recover.

    An actor from an earlier boot on this host is proven dead instead
    (#1764): no process survives a reboot, so the next create retires
    its seal like any dead actor's.
    """
    project, target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    if actor == "live":
        _rewrite_actor(
            seal, **dataclasses.asdict(wrkslots._read_process_identity(os.getpid()))
        )
    elif actor == "other-boot":
        _rewrite_actor(seal, boot_id="00000000-0000-4000-8000-000000000000")
    else:
        _rewrite_actor(seal, host_id="0" * 32)
    sealed = seal.read_bytes()
    mode = target.stat().st_mode & 0o777

    classified = _classify(project, "next")
    assert classified.returncode != 0
    if actor == "live":
        # The rewrite names this live process as actor and operator.
        assert "in progress (pid " in classified.stderr
        assert "recover" not in classified.stderr
    elif actor == "other-boot":
        # An ended boot is dead everywhere in wrkslots.
        assert "interrupted validation-batch seal recorded" in classified.stderr
    else:
        # Another machine proves nothing: no recovery suggestion.
        assert "cannot be proven dead" in classified.stderr
        assert "recover" not in classified.stderr

    made = create(project, slot="next", slot_type="validate", branch=None)
    assert "recovered the validation-batch seal" not in made.stderr
    assert seal.read_bytes() == sealed
    assert target.stat().st_mode & 0o777 == mode


def test_classify_reports_live_validate_batch_seal_as_structured_blocking_reason(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(
        seal, **dataclasses.asdict(wrkslots._read_process_identity(os.getpid()))
    )

    classified = _classify(project, "next")

    assert classified.returncode == 3, classified.stderr
    # The rewrite names this live process as the seal's operator.
    assert "in progress (pid " in classified.stderr
    assert "recover" not in classified.stderr
    payload = json.loads(classified.stdout)
    assert payload["blocking"] is True
    assert payload["schema"] == 2
    reasons = payload["blocking_reasons"]
    assert len(reasons) == 1
    assert reasons[0]["kind"] == "validate-batch-seal"
    assert reasons[0]["operation"] == "remove-validate-batch"
    assert reasons[0]["actor_state"] == "live"
    assert reasons[0]["targets"] == ["target"]
    assert reasons[0]["path"] == str(seal)


def test_classify_dead_validate_batch_seal_is_not_blocking(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(seal, pid=2_147_483_647)

    classified = _classify(project, "next")

    assert classified.returncode == 0, classified.stderr
    payload = json.loads(classified.stdout)
    assert payload["blocking"] is False
    assert "blocking_reasons" not in payload


def test_classify_unknown_actor_seal_reports_unknown_blocking_reason(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(seal, host_id="0" * 32)

    classified = _classify(project, "next")

    assert classified.returncode == 3, classified.stderr
    assert "cannot be proven dead" in classified.stderr
    payload = json.loads(classified.stdout)
    reasons = payload["blocking_reasons"]
    assert len(reasons) == 1
    assert reasons[0]["actor_state"] == "unknown"
    assert reasons[0]["targets"] == ["target"]


def test_classify_other_kind_seal_reports_other_blocking_reason(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    raw = json.loads(seal.read_text())
    raw["kind"] = "some-other-seal"
    seal.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")

    classified = _classify(project, "next")

    assert classified.returncode == 3, classified.stderr
    assert "cannot be proven dead" in classified.stderr
    payload = json.loads(classified.stdout)
    reasons = payload["blocking_reasons"]
    assert len(reasons) == 1
    assert reasons[0]["kind"] == "other"
    assert reasons[0]["actor_state"] == "unknown"


def test_classify_clean_state_success_json_unchanged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    del monkeypatch
    classified = _classify(project, "next")
    assert classified.returncode == 0, classified.stderr
    payload = json.loads(classified.stdout)
    assert payload["blocking"] is False
    assert set(payload) == {"blocking", "foreign_shards", "journals", "requested", "schema"}


def test_only_wrkslots_processes_of_this_project_can_hold_a_seal_open(tmp_path: Path) -> None:
    project = tmp_path / "project"
    other = tmp_path / "other"
    for directory in (project, other):
        directory.mkdir()
    proc = tmp_path / "proc"

    def process(pid: int, argv: Sequence[str], cwd: Path) -> None:
        entry = proc / str(pid)
        entry.mkdir(parents=True)
        (entry / "cmdline").write_bytes(b"\0".join(part.encode() for part in argv) + b"\0")
        (entry / "cwd").symlink_to(cwd)

    process(10, ["python3", "-m", "wrkslots", "--project-root", str(other), "remove"], tmp_path)
    process(11, ["python3", "-m", "pytest", "wrkslots/tests/test_x.py", "remove"], project)
    process(12, ["bash", "/x/ci-hub/bin/wrkslots", "remove-validate-batch"], other)
    # Read-only commands and recover never hold a seal open.
    process(16, ["python3", "-m", "wrkslots", "--project-root", str(project), "doctor"], tmp_path)
    process(17, ["bash", "/x/ci-hub/bin/wrkslots", "recover", "--coordinator-pid", "1"], project)
    assert wrkslots._other_live_wrkslots_processes(project, proc) == []

    process(13, ["python3", "-m", "wrkslots", "--project-root", str(project), "remove"], tmp_path)
    process(14, ["bash", "/x/ci-hub/bin/wrkslots", "remove-validate-batch"], project / ".")
    process(15, ["python3", "/a/py/wrkslots/__main__.py", f"--project-root={project}", "retire-pending"], tmp_path)
    assert sorted(wrkslots._other_live_wrkslots_processes(project, proc)) == [13, 14, 15]
    # With sealed targets named, only a process acting on one of them
    # counts; unrelated removals and creates do not hold the seal open.
    process(18, ["python3", "-m", "wrkslots", "--project-root", str(project), "remove", "sealed-slot"], tmp_path)
    assert wrkslots._other_live_wrkslots_processes(project, proc, targets=("sealed-slot",)) == [18]
    assert wrkslots._other_live_wrkslots_processes(project, proc, targets=("never-mentioned",)) == []


_EARLIER_BOOT = "earlier-boot-before-reboot"
_RENUMBERED_DEV = 1048744  # an anonymous (major 0) device, unlike any live one


def _crash_sealed(project: Path, seal: Path) -> dict[str, object]:
    crashed = raw_command_with_census_authority_stub(
        project,
        *_remove_command(),
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-seal-target"},
    )
    assert crashed.returncode == 86, crashed.stderr
    payload: dict[str, object] = json.loads(seal.read_text(encoding="utf-8"))
    return payload



def _seal_actor(payload: dict[str, object]) -> MutableMapping[str, object]:
    actor = payload["actor"]
    assert isinstance(actor, dict)
    return actor


def _seal_target(payload: dict[str, object]) -> MutableMapping[str, object]:
    targets = payload["targets"]
    assert isinstance(targets, list)
    first = targets[0]
    assert isinstance(first, dict)
    return first


def _seal_identity(target: MutableMapping[str, object]) -> MutableSequence[object]:
    identity = target["identity"]
    assert isinstance(identity, list)
    return identity

def _rewrite_seal(seal: Path, payload: dict[str, object]) -> None:
    seal.write_text(json.dumps(payload), encoding="utf-8")


def _recover(project: Path) -> subprocess.CompletedProcess[str]:
    return raw_command_with_census_authority_stub(
        project,
        "recover",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )


def test_cross_boot_seal_with_renumbered_device_recovers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """The reported shape (#1764): earlier boot, only the device changed.

    The journal is rewritten to a pre-change one (no recorded filesystem
    id) whose actor ran in an earlier boot and whose target device was
    renumbered by the reboot; recover must restore the sealed mode.
    """
    project, target, seal = _prepare(tmp_path, monkeypatch)
    original = target.stat().st_mode & 0o777
    payload = _crash_sealed(project, seal)
    assert target.stat().st_mode & 0o777 == 0o700
    _seal_actor(payload)["boot_id"] = _EARLIER_BOOT
    _seal_identity(_seal_target(payload))[0] = _RENUMBERED_DEV
    _seal_target(payload).pop("fs_id", None)
    _rewrite_seal(seal, payload)

    assert remove_completed_validation(project) == 3
    assert "interrupted validation-batch seal recorded" in capsys.readouterr().err

    recovered = _recover(project)
    assert recovered.returncode == 0, recovered.stderr
    assert not seal.exists()
    assert target.stat().st_mode & 0o777 == original

    assert remove_completed_validation(project) == 0, capsys.readouterr().err
    assert active_slots(project) == []


def test_cross_boot_seal_with_recorded_fs_id_recovers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A new journal's recorded filesystem id proves the same filesystem."""
    project, target, seal = _prepare(tmp_path, monkeypatch)
    original = target.stat().st_mode & 0o777
    payload = _crash_sealed(project, seal)
    assert _seal_target(payload).get("fs_id")
    _seal_actor(payload)["boot_id"] = _EARLIER_BOOT
    _seal_identity(_seal_target(payload))[0] = _RENUMBERED_DEV
    _rewrite_seal(seal, payload)

    recovered = _recover(project)
    assert recovered.returncode == 0, recovered.stderr
    assert not seal.exists()
    assert target.stat().st_mode & 0o777 == original
    assert remove_completed_validation(project) == 0, capsys.readouterr().err


def test_same_boot_seal_with_renumbered_device_still_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No reboot: a changed device is a changed directory, seal binding."""
    project, target, seal = _prepare(tmp_path, monkeypatch)
    payload = _crash_sealed(project, seal)
    _seal_identity(_seal_target(payload))[0] = _RENUMBERED_DEV
    _seal_target(payload).pop("fs_id", None)
    _rewrite_seal(seal, payload)

    recovered = _recover(project)
    assert recovered.returncode != 0
    assert seal.exists()
    assert target.stat().st_mode & 0o777 == 0o700


def test_cross_boot_seal_with_different_handle_still_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A recreated directory reusing the inode has different handle bytes."""
    project, target, seal = _prepare(tmp_path, monkeypatch)
    payload = _crash_sealed(project, seal)
    _seal_actor(payload)["boot_id"] = _EARLIER_BOOT
    _seal_identity(_seal_target(payload))[0] = _RENUMBERED_DEV
    handle = str(_seal_identity(_seal_target(payload))[3])
    _seal_identity(_seal_target(payload))[3] = ("0" if handle[0] != "0" else "1") + handle[1:]
    _seal_target(payload).pop("fs_id", None)
    _rewrite_seal(seal, payload)

    recovered = _recover(project)
    assert recovered.returncode != 0
    assert seal.exists()
    assert target.stat().st_mode & 0o777 == 0o700


def test_cross_boot_seal_with_different_filesystem_still_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A recorded filesystem id that is not the live one proves nothing."""
    project, target, seal = _prepare(tmp_path, monkeypatch)
    payload = _crash_sealed(project, seal)
    _seal_actor(payload)["boot_id"] = _EARLIER_BOOT
    _seal_identity(_seal_target(payload))[0] = _RENUMBERED_DEV
    _seal_target(payload)["fs_id"] = int(str(_seal_target(payload)["fs_id"])) + 1
    _rewrite_seal(seal, payload)

    recovered = _recover(project)
    assert recovered.returncode != 0
    assert seal.exists()
    assert target.stat().st_mode & 0o777 == 0o700


def test_earlier_boot_actor_is_orphaned_regardless_of_pid(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An earlier boot on this host is dead even if its pid looks live."""
    project, _target, seal = _prepare(tmp_path, monkeypatch)
    _crash_sealed(project, seal)
    config = wrkslots._load_config(str(project), "testhost")
    monkeypatch.setattr(
        wrkslots, "_process_state", lambda _identity: ("live", "pid reused live")
    )
    monkeypatch.setattr(
        wrkslots, "_validate_batch_seal_paired_finish", lambda *_a: None
    )

    orphaned, _reason = wrkslots._validate_batch_seal_orphaned(config, seal)
    assert orphaned is False  # journal actor boot is the real (later) boot here

    payload = json.loads(seal.read_text(encoding="utf-8"))
    _seal_actor(payload)["boot_id"] = "boot-before-this-one"
    _rewrite_seal(seal, payload)
    orphaned, reason = wrkslots._validate_batch_seal_orphaned(config, seal)
    assert orphaned is True
    assert "earlier boot" in reason


def test_private_cleanup_identity_matches_cross_boot_rules(tmp_path: Path) -> None:
    """The relaxed match needs inode, handle and filesystem, never device."""
    target = tmp_path / "slot"
    target.mkdir()
    live = wrkslots._stable_directory_identity(target, "slot")
    recorded = (_RENUMBERED_DEV, live[1], live[2], live[3])
    assert wrkslots._private_cleanup_identity_matches(
        target, live, recorded,
        cross_boot=True, recorded_fs_id=wrkslots._filesystem_id(target),
        fs_anchor=None,
    )
    assert not wrkslots._private_cleanup_identity_matches(
        target, live, recorded,
        cross_boot=False, recorded_fs_id=wrkslots._filesystem_id(target),
        fs_anchor=None,
    )
    different_inode = (_RENUMBERED_DEV, live[1] + 1, live[2], live[3])
    assert not wrkslots._private_cleanup_identity_matches(
        target, live, different_inode,
        cross_boot=True, recorded_fs_id=wrkslots._filesystem_id(target),
        fs_anchor=None,
    )
    assert not wrkslots._private_cleanup_identity_matches(
        target, live, recorded,
        cross_boot=True, recorded_fs_id=wrkslots._filesystem_id(target) + 1,
        fs_anchor=None,
    )
    assert wrkslots._private_cleanup_identity_matches(
        target, live, recorded,
        cross_boot=True, recorded_fs_id=None, fs_anchor=tmp_path,
    )

def _adopt_target(project: Path) -> int:
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._find_record(wrkslots._load_active(config), "target")
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "adopt",
            "target",
            "--agent",
            record.agent,
            "--owner-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )


def test_adopt_names_a_live_seal_as_in_progress_without_suggesting_recover(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """#1736: adopt meets a seal whose batch is still alive.

    The seal's actor is the authorizing coordinator (this test
    process); the batch itself is the operator process below. Only
    the two together read as in progress.
    """

    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(
        seal, **dataclasses.asdict(wrkslots._read_process_identity(os.getpid()))
    )
    operator = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import time; time.sleep(120)",
            "-m",
            "wrkslots",
            "--project-root",
            str(project),
            "remove",
            "target",
        ]
    )
    _rewrite_operator(seal, operator.pid)
    sealed = seal.read_bytes()
    try:
        result = _adopt_target(project)
    finally:
        operator.kill()
        operator.wait()

    assert result == 3
    err = capsys.readouterr().err
    assert "in progress (pid " in err
    assert f"pid {operator.pid}" in err
    assert "recover" not in err
    assert seal.read_bytes() == sealed


def test_adopt_calls_a_seal_whose_batch_is_gone_interrupted_even_if_coordinator_lives(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """#1736: a live coordinator alone does not keep a seal in progress."""

    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(
        seal, **dataclasses.asdict(wrkslots._read_process_identity(os.getpid()))
    )
    # The batch process itself stays the crashed (dead) one.
    raw = json.loads(seal.read_text())
    raw["x"]["operator"]["pid"] = 2_147_483_647
    seal.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")
    sealed = seal.read_bytes()

    assert _adopt_target(project) == 3
    err = capsys.readouterr().err
    assert "interrupted validation-batch seal recorded" in err
    assert "wrkslots recover" in err
    assert seal.read_bytes() == sealed


def test_adopt_calls_a_dead_actors_seal_interrupted_and_suggests_recover(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """#1736: only a proven-dead owner earns the interrupted wording."""

    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(seal, pid=2_147_483_647)
    sealed = seal.read_bytes()

    assert _adopt_target(project) == 3
    err = capsys.readouterr().err
    assert "interrupted validation-batch seal recorded" in err
    assert "wrkslots recover" in err
    assert seal.read_bytes() == sealed


def test_a_real_running_batch_is_in_progress_to_adopt_and_recover(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """#1736: an actual remove-validate-batch child owns its seal.

    The batch process is blocked inside its shared census on a FIFO,
    so the seal it published (recording the batch process itself as
    operator) is live for real: adopt and recover both name it in
    progress with the batch pid, and neither suggests or performs a
    recovery against it.
    """

    project, target, seal = _prepare(tmp_path, monkeypatch)
    entered = tmp_path / "batch-entered"
    fifo = tmp_path / "batch-fifo"
    os.mkfifo(fifo)
    launcher = tmp_path / "batch_launcher.py"
    launcher.write_text(
        "import os, sys\n"
        "from pathlib import Path\n"
        "from wrkslots import cli\n"
        "cli._assert_slot_unused = lambda *a, **k: None\n"
        "def census(paths, **kw):\n"
        "    Path(os.environ['BATCH_ENTERED']).touch()\n"
        "    with open(os.environ['BATCH_FIFO'], 'rb') as fh:\n"
        "        fh.read(1)\n"
        "    return cli._ProcessPathCensus((), ())\n"
        "cli._capture_process_path_census = census\n"
        "cli._capture_same_uid_process_path_census = (\n"
        "    lambda paths, **kw: cli._ProcessPathCensus((), ())\n"
        ")\n"
        "cli._production_frozen_validation_authority_commit = (\n"
        "    cli._frozen_validation_authority_commit\n"
        ")\n"
        "cli._validation_run_host_evidence = lambda: ((), ())\n"
        "cli._retained_unit_cgroup_members = lambda units, root=None: dict.fromkeys(units, 0)\n"
        "cli._assert_host_process_view = lambda: None\n"
        "raise SystemExit(cli.main(sys.argv[1:]))\n",
        encoding="utf-8",
    )
    from wrkslots.tests.test_lifecycle import source_environment

    env = source_environment()
    env["BATCH_ENTERED"] = str(entered)
    env["BATCH_FIFO"] = str(fifo)
    batch = subprocess.Popen(
        [
            sys.executable,
            str(launcher),
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--slot",
            "target=1",
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        # Condition wait (hang guard only): the batch is inside its
        # census, holding its seal, when these assertions run.
        for _ in range(1200):
            if entered.exists():
                break
            assert batch.poll() is None, "batch exited before its census"
            import time as _time

            _time.sleep(0.05)
        assert entered.exists(), "batch never reached its census"
        assert seal.is_file()

        assert _adopt_target(project) == 3
        adopt_err = capsys.readouterr().err
        assert f"pid {batch.pid}" in adopt_err
        assert "recover" not in adopt_err
        recovered = raw_command_with_census_authority_stub(
            project,
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert recovered.returncode == 3, recovered.stderr
        assert f"pid {batch.pid}" in recovered.stderr
        assert seal.is_file()
        assert target.is_dir()
    finally:
        with open(fifo, "wb") as handle:
            handle.write(b"x")
        out, err = batch.communicate(timeout=120)

    assert batch.returncode == 0, err
    assert not seal.exists()
    assert not target.exists()
    assert active_slots(project) == []


def test_a_new_seal_still_parses_with_the_old_reader_shape(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The operator lives in "x" (#1736): old readers still retire it.

    An old reader checks the schema-1 exact keys {schema, kind,
    machine, created_at, actor, targets, x} and ignores the contents
    of "x"; it must parse a new seal and judge it orphaned from its
    actor alone, so an old create auto-retires it and an old recover
    can read it.
    """

    _project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(seal, pid=2_147_483_647)
    raw = json.loads(seal.read_text())
    assert set(raw) <= {
        "schema", "kind", "machine", "created_at", "actor", "targets", "x",
    }
    assert raw["schema"] == 1
    assert "operator" in raw["x"]
    # The crashed batch's actor is dead here (its coordinator was the
    # invoking process, which has exited): the old actor-only orphan
    # judgement retires the seal without ever reading "x".
    config = wrkslots._load_config(str(_project), "testhost")
    orphaned, _reason = wrkslots._validate_batch_seal_orphaned(config, seal)
    assert orphaned


def test_adopt_wait_stops_when_the_batch_dies_mid_wait(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Killing the batch mid-wait ends the wait as interrupted (#1736).

    Adopt waits on the seal's pidfd; when the operator process dies
    the waiter wakes at once, re-judges, and refuses as interrupted
    naming recover instead of sleeping out its --wait-lock budget.
    """

    import threading

    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(
        seal, **dataclasses.asdict(wrkslots._read_process_identity(os.getpid()))
    )
    operator = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import time; time.sleep(120)",
            "-m",
            "wrkslots",
            "--project-root",
            str(project),
            "remove",
            "target",
        ]
    )
    _rewrite_operator(seal, operator.pid)
    sealed = seal.read_bytes()
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._find_record(wrkslots._load_active(config), "target")
    outcome: dict[str, int] = {}

    def run_adopt() -> None:
        outcome["rc"] = wrkslots.main(
            [
                "--project-root",
                str(project),
                "--wait-lock",
                "3600",
                "adopt",
                "target",
                "--agent",
                record.agent,
                "--owner-pid",
                str(os.getpid()),
                "--expected-generation",
                "1",
            ]
        )

    worker = threading.Thread(target=run_adopt, daemon=True)
    worker.start()
    worker.join(timeout=2)
    assert worker.is_alive(), "adopt did not wait on the live seal"
    operator.kill()
    operator.wait()
    # The join bound is deliberately far smaller than the 3600s
    # budget: a broken pidfd wake hangs here and fails, never passes.
    worker.join(timeout=30)
    assert not worker.is_alive(), "adopt slept out its budget on a dead batch"
    assert outcome["rc"] == 3
    err = capsys.readouterr().err
    assert "interrupted validation-batch seal recorded" in err
    assert "wrkslots recover" in err
    assert seal.read_bytes() == sealed


def test_zombie_and_reused_pid_operators_are_dead_not_live(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A zombie operator, and a reused pid, are both dead (#1736)."""

    import time as _time

    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    sleeper = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"]
    )
    try:
        _rewrite_operator(seal, sleeper.pid)
        # Reused pid: same live pid, another generation.
        raw = json.loads(seal.read_text())
        raw["x"]["operator"]["start_ticks"] += 999_999
        seal.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")
        assert _adopt_target(project) == 3
        assert "interrupted validation-batch seal recorded" in (
            capsys.readouterr().err
        )

        # Zombie: the recorded generation exited and awaits reaping.
        zombie = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(120)"]
        )
        try:
            _rewrite_operator(seal, zombie.pid)
            zombie.kill()
            deadline = _time.monotonic() + 30
            state = ""
            while _time.monotonic() < deadline:
                text = Path(f"/proc/{zombie.pid}/stat").read_text()
                state = text.rsplit(")", 1)[1].split()[0]
                if state == "Z":
                    break
                _time.sleep(0.01)
            assert state == "Z", "child never became a zombie"
            assert _adopt_target(project) == 3
            err = capsys.readouterr().err
            assert "interrupted validation-batch seal recorded" in err
            assert "in progress" not in err
        finally:
            zombie.wait()
    finally:
        sleeper.kill()
        sleeper.wait()


def test_recover_fails_closed_on_an_unknowable_seal_without_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Recover refuses an unknowable seal unless authorized (#1736)."""

    project, target, seal, original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(seal, host_id="0" * 32)

    refused = raw_command_with_census_authority_stub(
        project, "recover", "--coordinator-pid", str(os.getpid())
    )
    assert refused.returncode == 3, refused.stderr
    assert "--coordinator-authorized" in refused.stderr
    assert seal.exists()

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


def test_an_old_format_seal_without_x_is_recovered_as_before(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A seal with no extension slot recovers exactly as before."""

    project, target, seal, original = _crashed_seal(tmp_path, monkeypatch)
    raw = json.loads(seal.read_text())
    del raw["x"]
    seal.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n")

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


def test_recover_refuses_a_seal_whose_batch_is_still_running(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """#1736: recover never retires a live batch's seal."""

    project, _target, seal, _original = _crashed_seal(tmp_path, monkeypatch)
    _rewrite_actor(
        seal, **dataclasses.asdict(wrkslots._read_process_identity(os.getpid()))
    )
    operator = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import time; time.sleep(120)",
            "-m",
            "wrkslots",
            "--project-root",
            str(project),
            "remove",
            "target",
        ]
    )
    _rewrite_operator(seal, operator.pid)
    sealed = seal.read_bytes()
    try:
        recovered = raw_command_with_census_authority_stub(
            project,
            "recover",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
        )
    finally:
        operator.kill()
        operator.wait()

    assert recovered.returncode == 3, recovered.stderr
    assert "in progress (pid " in recovered.stderr
    assert seal.read_bytes() == sealed
