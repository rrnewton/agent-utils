"""A validation row's liveness is answered by its run, not by an agent name.

Validation launchers register a run label such as ``validate-<slot>`` as the
row's agent.  No process carries that label, so the configured agent-liveness
command cannot answer for it: on 2026-10-05 it reported every one of 54
validation rows on one host ``unverifiable``, and ``remove --validate-complete``
refused every completed checkout whose launcher had already exited.  These
tests pin the replacement authority -- retained run handles, their exact
process generations and units, and user-systemd state -- in both directions:
removal succeeds when the run is provably over even though the agent-name
command is unverifiable, and it refuses whenever the run may still be using the
checkout even though the agent-name command says dead.
"""

from __future__ import annotations

import contextlib
import dataclasses
import json
import os
import subprocess
import sys
import time
import uuid
from collections.abc import Iterator, Mapping, Sequence, Set as AbstractSet
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    allow_test_host_for_absent_validate_recovery,
    checkout,
    create,
    expire_heartbeat,
    interrupt_validate_batch,
    make_project,
    mark_owner_dead,
    prepare_absent_agent_row,
    prepare_absent_validate_row,
    prepare_dead_validate_slots,
    run_absent_agent_recovery,
    run_absent_validate_recovery,
    set_liveness,
    stub_validate_batch_censuses,
    write_absent_validate_input,
)


# These tests supply the host's process and user-systemd evidence themselves,
# below the seam that the suite's idle-host default replaces.
pytestmark = pytest.mark.validation_run_evidence

RUN_UNIT = "validate-run-0001.service"


def _unit(**overrides: str) -> dict[str, str]:
    value = {
        "Id": RUN_UNIT,
        "LoadState": "loaded",
        "ActiveState": "inactive",
        "SubState": "dead",
        "MainPID": "0",
        "ControlGroup": f"/user.slice/app.slice/{RUN_UNIT}",
        "WorkingDirectory": "",
        "ExecStart": "",
        "Environment": "",
        "PendingJob": "no",
    }
    value.update(overrides)
    return value


def _prepare(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    agent_liveness: str,
    units: Sequence[Mapping[str, str]] = (),
    processes: Sequence[wrkslots._AbsentProcessObservation] = (),
) -> tuple[Path, Path]:
    """Create one completed validation row whose launcher has exited."""

    project, _repository, _remote = make_project(tmp_path)
    made = create(
        project,
        agent="validate-slot01",
        slot_type="validate",
        branch=None,
    )
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, agent_liveness)
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: tuple(units))
    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_process_snapshot",
        lambda **_kwargs: tuple(processes),
    )
    # The supplied host's retained units have no control-group members; a
    # test that examines members supplies them itself.
    monkeypatch.setattr(wrkslots, "_retained_unit_cgroup_members", _no_members)
    return project, checkout(project, slot_type="validate")


class _MemberReader(Protocol):
    def __call__(
        self, units: AbstractSet[str], *, root: Path | None = None
    ) -> Mapping[str, int]: ...


def _no_members(
    units: AbstractSet[str], *, root: Path | None = None
) -> Mapping[str, int]:
    return dict.fromkeys(units, 0)


def _write_run_handle(
    project: Path,
    tree: Path,
    *,
    process_identity: Mapping[str, object] | None = None,
    unit: str = RUN_UNIT,
) -> Path:
    handle = project / "ignored" / "validate" / "runs" / (
        unit.removesuffix(".service") + ".json"
    )
    handle.parent.mkdir(parents=True, exist_ok=True)
    value: dict[str, object] = {"checkout": str(tree), "unit": unit}
    if process_identity is not None:
        value["process_identity"] = dict(process_identity)
    handle.write_text(json.dumps(value), encoding="utf-8")
    return handle


def _remove_completed(project: Path) -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )


def _assert_retained(project: Path, tree: Path) -> None:
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_completed_validation_removal_does_not_ask_agent_liveness_about_a_run_label(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The incident: the agent-name command cannot answer for a run label."""

    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="unverifiable",
        units=(_unit(),),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    assert removed == 0, capsys.readouterr().err
    assert not tree.exists()
    assert active_slots(project) == []


def test_completed_validation_removal_refuses_while_the_run_unit_is_active(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(ActiveState="active", SubState="running"),),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority reports the run may still use slot slot01" in error
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in error
    _assert_retained(project, tree)


@pytest.mark.parametrize("spelling", ["symlink", "descendant", "parent-step"])
def test_a_handle_naming_the_slot_by_another_path_binds_to_the_row(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    spelling: str,
) -> None:
    """A handle whose checkout reaches the slot under another path is its run.

    Its checkout is compared with the row's paths as a unit word is: a
    symlink to the slot, a directory inside it, and a ``..`` after a
    symlink into it all name the row, so the handle's active unit keeps
    the slot.
    """

    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(ActiveState="active", SubState="running"),),
    )
    (tree / "product").mkdir(exist_ok=True)
    if spelling == "symlink":
        checkout = tmp_path / "alias"
        checkout.symlink_to(tree)
    elif spelling == "descendant":
        checkout = tree / "product"
    else:
        link = tmp_path / "inside"
        link.symlink_to(tree / "product")
        checkout = link / ".."
    _write_run_handle(project, checkout)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority reports the run may still use slot slot01" in error
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_while_the_run_job_is_queued(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(PendingJob="yes"),),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_a_live_run_process_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(os.getpid()))
    assert start_ticks is not None
    _write_run_handle(
        project,
        tree,
        process_identity={
            "pid": os.getpid(),
            "start_ticks": start_ticks,
            "boot_id": wrkslots._boot_id(Path("/proc")),
        },
    )

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert f"has live exact process generation {os.getpid()} for row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_a_process_left_in_the_run_cgroup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    straggler = wrkslots._AbsentProcessObservation(
        pid=os.getpid() + 100_000,
        start_ticks=17,
        cgroup_path=f"/user.slice/app.slice/{RUN_UNIT}/payload",
        mount_namespace="mnt:[test]",
    )
    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(),),
        processes=(straggler,),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_an_active_unit_naming_the_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A run with no retained handle is still visible through its unit."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    unrecorded = _unit(
        Id="unrecorded-run.service",
        ActiveState="active",
        SubState="running",
        WorkingDirectory=str(tree),
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (unrecorded,))

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "user-systemd unit unrecorded-run.service names validation row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_unreadable_run_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")

    def unreadable() -> tuple[Mapping[str, str], ...]:
        raise wrkslots.Refusal("cannot enumerate user-systemd state: no user bus")

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", unreadable)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority is unverifiable for slot slot01" in error
    assert "no user bus" in error
    _assert_retained(project, tree)


def test_audit_judges_a_validation_row_by_its_run(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Audit and remove ask the same authority, so they cannot disagree."""

    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="unverifiable", units=(_unit(),)
    )
    _write_run_handle(project, tree)
    expire_heartbeat(project)

    def audit() -> str:
        code = wrkslots.main(
            ["--project-root", str(project), "audit", "--format", "json"]
        )
        captured = capsys.readouterr()
        assert code == 0, captured.err
        return captured.out

    finished = json.loads(audit())
    phases = {phase["name"]: phase for phase in finished["metrics"]["phases"]}
    assert phases["liveness"]["work"] == {
        "batch_invocations": 0,
        "legacy_invocations": 0,
        "subjects": 0,
        "validation_run_subjects": 1,
    }
    row = finished["slots"][0]
    assert row["liveness_state"] == "dead"
    assert row["verdict"] == "DELETABLE", row["reasons"]

    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (_unit(ActiveState="active", SubState="running"),),
    )
    running = json.loads(audit())
    row = running["slots"][0]
    assert row["liveness_state"] == "alive"
    assert row["verdict"] == "BLOCKED"
    assert any(
        f"retained validation unit {RUN_UNIT} may still use row slot01" in reason
        for reason in row["reasons"]
    )


def _live_identity() -> dict[str, object]:
    start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(os.getpid()))
    assert start_ticks is not None
    return {
        "pid": os.getpid(),
        "start_ticks": start_ticks,
        "boot_id": wrkslots._boot_id(Path("/proc")),
    }


@pytest.mark.parametrize(
    ("duplicated", "trailer"),
    [
        # A later null identity would hide the live process generation.
        ("process_identity", '"process_identity": null'),
        # A later checkout would move the handle off this row entirely.
        ("checkout", '"checkout": "/elsewhere/unrelated-checkout"'),
    ],
)
def test_completed_validation_removal_refuses_a_handle_with_a_duplicate_field(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    duplicated: str,
    trailer: str,
) -> None:
    """A JSON object with two values for one field has no single meaning.

    A permissive decoder keeps the last value.  Here the first values say the
    run's process is live on this row's checkout, and the appended duplicate
    would make the row look dead.  The handle is unreadable evidence instead.
    """

    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    handle = _write_run_handle(project, tree, process_identity=_live_identity())
    text = handle.read_text(encoding="utf-8")
    assert text.endswith("}")
    handle.write_text(f"{text[:-1]}, {trailer}}}", encoding="utf-8")

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority is unverifiable for slot slot01" in error
    assert f"duplicate key {duplicated!r}" in error
    _assert_retained(project, tree)


def _slot_directory(project: Path) -> Path:
    config = wrkslots._load_config(str(project), "testhost")
    return wrkslots._slot_directory(config, "slot01", "validate")


@pytest.mark.parametrize(
    ("property_name", "spelling"),
    [
        ("ExecStart", "{parent}/./{name}"),
        ("ExecStart", "--checkout={parent}//{name}/product"),
        ("WorkingDirectory", "{parent}/elsewhere/../{name}"),
        ("Environment", "RUN_ROOT={parent}/./{name}/./product"),
        ("RequiresMountsFor", "{parent}/{name}/"),
    ],
)
def test_completed_validation_removal_refuses_a_queued_unit_naming_an_alias_of_the_row(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    property_name: str,
    spelling: str,
) -> None:
    """A queued job names the row through a different spelling of its path.

    The job has not started, so no process or handle shows it yet.  Its unit
    evidence names the row's directory with ``/./``, ``//`` or ``x/..``
    steps; each spelling is the same path, so the row may still be used.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    slot_directory = _slot_directory(project)
    alias = spelling.format(parent=slot_directory.parent, name=slot_directory.name)
    queued = _unit(Id="queued-run.service", PendingJob="yes", **{property_name: alias})
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (queued,))

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority reports the run may still use slot slot01" in error
    assert "user-systemd unit queued-run.service names validation row slot01" in error
    _assert_retained(project, tree)


@pytest.mark.parametrize("sibling", ["slot010", "slot01-old", "slot01.bak", "slot01_next"])
def test_completed_validation_removal_ignores_an_active_unit_naming_a_sibling_row(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    sibling: str,
) -> None:
    """``slot010`` begins with the text ``slot01`` but is a different path."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    other = _slot_directory(project).parent / sibling
    running = _unit(
        Id="sibling-run.service",
        ActiveState="active",
        SubState="running",
        WorkingDirectory=str(other),
        ExecStart=f"/usr/bin/env\n--checkout={other}/product",
        Environment=f"RUN_ROOT={other}",
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (running,))

    removed = _remove_completed(project)

    assert removed == 0, capsys.readouterr().err
    assert not tree.exists()
    assert active_slots(project) == []


def test_unit_words_name_a_row_by_path_identity(tmp_path: Path) -> None:
    """A unit names a row when one of its words is the row's path or inside it.

    Words are compared as whole paths: lexically normalized, symlink
    resolved, relative ones resolved against the unit's working directory,
    and by file identity where the path exists.  A longer path that merely
    contains the row's text is a different path.
    """

    project = tmp_path / "project"
    row = project / "worktrees" / "validate" / "slot01"
    row.mkdir(parents=True)
    (project / "worktrees" / "validate" / "slot010").mkdir()
    alias = tmp_path / "alias"
    alias.symlink_to(row)
    outer = tmp_path / "outer"
    outer.symlink_to(project)

    def names(**unit: str) -> bool:
        return wrkslots._UnitPathResolver().names(unit, wrkslots._row_path_identity(row))

    # One shell command holds several paths; a later ``x/..`` step does not
    # hide the row path that an earlier word names.
    assert names(ExecStart=f"cd {project}/worktrees/tmp/../validate/slot01 && ls build/../out")
    assert names(Environment=f"PATH=/usr/bin:{row}:/bin")
    assert names(ExecStart=f"--checkout={row}//product")
    # A symlink to the row, or to one of its ancestors, is the row.
    assert names(ExecStart=f"{alias}/product")
    assert names(ExecStart=f"{outer}/worktrees/validate/slot01")
    # A relative word is resolved against the unit's working directory.
    assert names(WorkingDirectory=str(project), ExecStart="git\n-C\nworktrees/validate/slot01")
    assert names(WorkingDirectory=f"!{row}/build", ExecStart="make\n..")
    assert names(WorkingDirectory=f"!{row}")
    # A longer path containing the row's text is another path.
    assert not names(ExecStart=f"/other{row}")
    assert not names(ExecStart=f"{row}+other")
    assert not names(ExecStart=f"{row}0")
    assert not names(WorkingDirectory=str(project), ExecStart="worktrees/validate/slot010")

    # A row spelled through a symlink matches its resolved spelling.
    real = tmp_path / "real"
    (real / "worktrees").mkdir(parents=True)
    link = tmp_path / "link"
    link.symlink_to(real)
    linked = wrkslots._row_path_identity(link / "worktrees" / "validate" / "slot01")
    assert wrkslots._UnitPathResolver().names(
        {"ExecStart": f"{real}/worktrees/validate/slot01"}, linked
    )


def test_a_parent_step_after_a_symlink_leaves_the_symlink_target(tmp_path: Path) -> None:
    """``link/..`` is the parent of the link's target, as the kernel reads it.

    Lexical normalization would drop the symlink with the ``..`` and read
    the link's own directory instead.
    """

    row = tmp_path / "project" / "worktrees" / "validate" / "slot01"
    (row / "product" / "build").mkdir(parents=True)
    inside = tmp_path / "inside"
    inside.symlink_to(row / "product")
    deep = tmp_path / "deep"
    deep.symlink_to(row / "product" / "build")
    alias = tmp_path / "alias"
    alias.symlink_to(row)

    def names(**unit: str) -> bool:
        return wrkslots._UnitPathResolver().names(unit, wrkslots._row_path_identity(row))

    assert names(ExecStart=f"{inside}/..")
    assert names(ExecStart=f"make -C {deep}/../..")
    assert names(WorkingDirectory=str(deep), ExecStart="git\n-C\n../..\nstatus")
    # The parent of the row itself is not inside the row.
    assert not names(ExecStart=f"{alias}/..")
    # A row recorded through a symlink and ``..`` is the directory it reaches.
    stepped = wrkslots._row_path_identity(inside / "..")
    assert wrkslots._UnitPathResolver().names({"ExecStart": f"{row}/out"}, stepped)


def test_a_row_path_with_a_space_is_one_argument(tmp_path: Path) -> None:
    """An argv element, option value or quoted shell word is one path.

    Each line of a property is one element; splitting it at whitespace
    alone would break a row path that contains a space into two
    unrelated words.
    """

    row = tmp_path / "with space" / "project" / "worktrees" / "validate" / "slot01"
    row.mkdir(parents=True)

    def names(**unit: str) -> bool:
        return wrkslots._UnitPathResolver().names(unit, wrkslots._row_path_identity(row))

    assert names(ExecStart=f"make\n-C\n{row}/product")
    assert names(ExecStart=f"tool\n--checkout={row}")
    assert names(ExecStart=f"/bin/sh\n-c\ncd '{row}/product' && make")
    assert names(ExecStart=f'/bin/sh\n-c\nmake --checkout="{row}"')
    assert names(Environment=f"CHECKOUT={row}/build")
    assert not names(ExecStart=f"make\n-C\n{row}0")
    assert not names(ExecStart=f"/bin/sh\n-c\ncd '{row}0' && make")


def test_a_row_path_holding_separators_is_one_option_or_assignment_value(
    tmp_path: Path,
) -> None:
    """A row path holding ``:`` and a space is whole in an option or assignment.

    Splitting at separators breaks it apart, so the value after ``=`` (or
    after a separator that ``/`` follows) is a candidate as a whole, and
    the row's own spellings are found as text wherever they stand.
    """

    row = tmp_path / "with: space" / "project" / "worktrees" / "validate" / "slot01"
    row.mkdir(parents=True)
    alias = tmp_path / "alias: link"
    alias.symlink_to(row)

    def names(**unit: str) -> bool:
        return wrkslots._UnitPathResolver().names(unit, wrkslots._row_path_identity(row))

    assert names(ExecStart=f"tool\n--checkout={row}")
    assert names(Environment=f"CHECKOUT={row}")
    assert names(ExecStart=f"/bin/sh\n-c\nmake --checkout='{row}/product'")
    assert names(Environment=f"PATH=/usr/bin:{row}:/bin")
    assert names(ExecStart=f"tool\n--checkout={alias}/product")
    assert names(Environment=f"SEARCH=/usr/lib;{alias}/lib")
    assert not names(ExecStart=f"tool\n--checkout={row}0")
    assert not names(Environment=f"CHECKOUT=/other{row}")
    assert not names(Environment=f"CHECKOUT={row}+other")

    # A row path holding a newline spans two lines of the property, and so
    # two elements, but is still found whole.
    broken = tmp_path / "with\nnewline" / "slot01"
    broken.mkdir(parents=True)
    assert wrkslots._UnitPathResolver().names(
        {"ExecStart": f"make\n-C\n{broken}/product"}, wrkslots._row_path_identity(broken)
    )


def test_resolving_a_long_path_looks_up_a_bounded_part_of_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A path longer than any system call accepts costs a bounded number of
    lookups, and the resolver keeps only the ancestors that exist.

    The path lies below a directory that does not exist, so no lookup below
    that directory can succeed; resolving all of its 3,002 components, and
    keeping each of their ancestor spellings, is work that grows with the
    square of its length.
    """

    row = tmp_path / "row"
    row.mkdir()
    identity = wrkslots._row_path_identity(row)
    deep = f"/nonexistent-{uuid.uuid4().hex}/" + "a/" * 3000 + "end"
    lookups: list[str] = []
    resolutions: list[str] = []
    real_stat, real_lstat, real_realpath = os.stat, os.lstat, os.path.realpath

    def counted_stat(path: str) -> os.stat_result:
        lookups.append(path)
        return real_stat(path)

    def counted_lstat(path: str) -> os.stat_result:
        lookups.append(path)
        return real_lstat(path)

    def counted_realpath(path: str) -> str:
        resolutions.append(path)
        return real_realpath(path)

    resolver = wrkslots._UnitPathResolver()
    with monkeypatch.context() as patch:
        patch.setattr(os, "stat", counted_stat)
        patch.setattr(os, "lstat", counted_lstat)
        patch.setattr(os.path, "realpath", counted_realpath)
        named = resolver.path_names(deep, identity)

    assert not named
    assert len(lookups) <= wrkslots._PATH_MAX // 2 + 2, len(lookups)
    assert resolutions and max(map(len, resolutions)) < wrkslots._PATH_MAX
    assert sum(map(len, resolver._files)) < 2 * wrkslots._PATH_MAX


def test_a_long_path_still_names_its_row(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Bounding the lookups keeps every reading that names a row.

    A long path inside the row, inside it through a symlink, or back at it
    after as many ``..`` steps as it has components names the row; one
    beside the row does not.  A lookup that fails for one directory only
    (here, permission to stat it) does not end the ancestor walk, so the
    row's own file is still found below it.
    """

    row = tmp_path / "row"
    row.mkdir()
    alias = tmp_path / "alias"
    alias.symlink_to(row)
    tail = "a/" * 3000 + "end"
    identity = wrkslots._row_path_identity(row)
    resolver = wrkslots._UnitPathResolver()

    assert resolver.path_names(f"{row}/{tail}", identity)
    assert resolver.path_names(f"{alias}/{tail}", identity)
    assert resolver.path_names(f"{alias}/{tail}/" + "../" * 3001, identity)
    assert resolver.path_names(f"/{tail}/" + "../" * 3001 + str(alias)[1:], identity)
    assert not resolver.path_names(f"{tmp_path}/other/{tail}", identity)
    assert wrkslots._UnitPathResolver().names(
        {"ExecStart": f"tool\n--checkout={alias}/{tail}"}, identity
    )

    by_file = wrkslots._RowPathIdentity(row, ("/elsewhere",), identity.file)
    parent = os.path.realpath(tmp_path)
    denied: list[str] = []
    real_stat = os.stat

    def stat(path: str) -> os.stat_result:
        if path == parent:
            denied.append(path)
            raise PermissionError(13, "Permission denied", path)
        return real_stat(path)

    with monkeypatch.context() as patch:
        patch.setattr(os, "stat", stat)
        assert wrkslots._UnitPathResolver().path_names(f"{row}/product", by_file)
    assert denied == [parent]


def test_one_resolver_reads_each_unit_property_once_for_every_row(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Several rows judged against one unit share the parsing of its strings."""

    rows = []
    for name in ("slot01", "slot02", "slot03"):
        (tmp_path / name).mkdir()
        rows.append(wrkslots._row_path_identity(tmp_path / name))
    unit = _unit(ExecStart="/bin/tool\n--checkout=/srv/other", Environment="HOME=/home/x")
    parsed: list[str] = []
    real_words = wrkslots._unit_property_words

    def words(value: str) -> tuple[str, ...]:
        parsed.append(value)
        return real_words(value)

    monkeypatch.setattr(wrkslots, "_unit_property_words", words)
    resolver = wrkslots._UnitPathResolver()

    assert not any(resolver.names(unit, row) for row in rows)
    assert sorted(parsed) == sorted(set(unit.values()))


def test_the_retained_handle_census_bounds_its_path_matching_by_time(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Matching a handle's paths with the rows counts against the census's
    time bound, and the census refuses once that bound has passed.

    The clock is simulated: matching the one handle's checkout takes longer
    than the whole bound, after its read was already checked.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    _write_run_handle(project, tree)
    config = wrkslots._load_config(str(project), "testhost")
    rows = [(record, (tree,)) for record in wrkslots._load_active(config).slots]
    assert [handle.unit for handle in wrkslots._retained_handles_for_absent_rows(
        config, rows
    )["slot01"]] == [RUN_UNIT]

    real_monotonic = time.monotonic
    elapsed = [0.0]
    real_path_names = wrkslots._UnitPathResolver.path_names

    def slow_path_names(
        self: wrkslots._UnitPathResolver, joined: str, row: wrkslots._RowPathIdentity
    ) -> bool:
        elapsed[0] += wrkslots._RETAINED_HANDLE_CENSUS_SECONDS + 60.0
        return real_path_names(self, joined, row)

    with monkeypatch.context() as patch:
        patch.setattr(time, "monotonic", lambda: real_monotonic() + elapsed[0])
        patch.setattr(wrkslots._UnitPathResolver, "path_names", slow_path_names)
        with pytest.raises(
            wrkslots.Refusal, match="retained validation handle census exceeded its time bound"
        ):
            wrkslots._retained_handles_for_absent_rows(config, rows)
    assert elapsed[0] > wrkslots._RETAINED_HANDLE_CENSUS_SECONDS


UNREADABLE_PID = 4_000_017


def _write_unreadable_generation_handle(
    project: Path, tree: Path, monkeypatch: pytest.MonkeyPatch, evidence: str
) -> None:
    """Record a run process whose generation or boot cannot be read."""

    _write_run_handle(
        project,
        tree,
        process_identity={
            "pid": UNREADABLE_PID,
            "start_ticks": 17,
            "boot_id": wrkslots._boot_id(Path("/proc")),
        },
    )
    if evidence == "boot-id":
        def boot_id(_proc_root: Path) -> str:
            raise wrkslots.Refusal("cannot read the machine boot id: test")

        # Only the run authority reads the boot id after this point in the
        # direct call below.
        monkeypatch.setattr(wrkslots, "_boot_id", boot_id)
        return
    real_start_ticks = wrkslots._process_start_ticks

    def start_ticks(pid_dir: Path) -> int | None:
        if pid_dir.name == str(UNREADABLE_PID):
            raise wrkslots.Refusal(
                f"process generation is indeterminate because {pid_dir / 'stat'} "
                "is unreadable: test"
            )
        return real_start_ticks(pid_dir)

    monkeypatch.setattr(wrkslots, "_process_start_ticks", start_ticks)


@pytest.mark.parametrize(
    ("evidence", "detail"),
    [
        ("boot-id", "cannot read the machine boot id"),
        ("pid-generation", "process generation is indeterminate"),
    ],
)
def test_unreadable_run_process_evidence_is_unverifiable_not_alive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    evidence: str,
    detail: str,
) -> None:
    """Evidence that cannot be read is not evidence of a run.

    ``alive`` tells the operator to stop the run's unit, which is the wrong
    remedy when nothing shows a run at all.
    """

    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    config = wrkslots._load_config(str(project), "testhost")
    records = wrkslots._load_active(config).slots
    _write_unreadable_generation_handle(project, tree, monkeypatch, evidence)

    states = wrkslots._validation_run_liveness_states(config, records)

    assert len(states) == 1
    state, message = next(iter(states.values()))
    assert state == "unverifiable", message
    assert detail in message


def test_completed_validation_removal_names_unreadable_run_evidence_as_unverifiable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    _write_unreadable_generation_handle(project, tree, monkeypatch, "pid-generation")

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority is unverifiable for slot slot01" in error
    assert "process generation is indeterminate" in error
    assert "stop its unit" not in error
    _assert_retained(project, tree)


def _run_child(cgroup: str, *, pid: int = 4_000_029) -> wrkslots._AbsentProcessObservation:
    return wrkslots._AbsentProcessObservation(
        pid=pid, start_ticks=31, cgroup_path=cgroup, mount_namespace="mnt:[test]"
    )


RUN_CGROUP = f"/user.slice/app.slice/{RUN_UNIT}"


def _judge_with_process_tables(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    before: Sequence[wrkslots._AbsentProcessObservation],
    after: Sequence[wrkslots._AbsentProcessObservation],
) -> tuple[str, str]:
    """Judge one row whose process table changes during the unit enumeration."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    _write_run_handle(project, tree)
    enumerated = False

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal enumerated
        enumerated = True
        # The run started and finished during this enumeration.
        return (_unit(),)

    def processes(**_kwargs: object) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        return tuple(after if enumerated else before)

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", processes)
    config = wrkslots._load_config(str(project), "testhost")
    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )
    assert enumerated
    assert len(states) == 1
    return next(iter(states.values()))


def test_a_run_child_that_appears_during_the_unit_enumeration_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The unit reads inactive and unqueued, but its child is still running.

    The process table read before the enumeration is empty.  Judging the row
    from it alone reports ``dead``, and the later path census does not look
    at the run's control group, so a child holding no path escapes both.
    """

    state, message = _judge_with_process_tables(
        tmp_path, monkeypatch, before=(), after=(_run_child(f"{RUN_CGROUP}/payload"),)
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in message


def test_a_run_child_that_leaves_the_run_cgroup_during_the_enumeration_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The same exact process generation keeps the run's control group."""

    state, message = _judge_with_process_tables(
        tmp_path,
        monkeypatch,
        before=(_run_child(f"{RUN_CGROUP}/payload"),),
        after=(_run_child("/user.slice/app.slice/elsewhere.scope"),),
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in message


def test_a_run_child_that_exited_during_the_enumeration_is_not_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A generation absent from the later table has exited."""

    state, message = _judge_with_process_tables(
        tmp_path, monkeypatch, before=(_run_child(f"{RUN_CGROUP}/payload"),), after=()
    )

    assert state == "dead", message


def test_absent_row_recovery_sees_a_run_child_that_appears_during_the_unit_enumeration(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """recover-absent-validate-rows judges the same evidence the same way.

    Its census reads the process table, then the unit enumeration runs.  A
    run that starts and finishes inside that enumeration leaves its unit
    inactive and unqueued and its child in the unit's control group, which
    only a table read after the enumeration shows.
    """

    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    config = wrkslots._load_config(str(project), "testhost")
    _write_run_handle(
        project, wrkslots._stored_path(config, record.checkouts[0].path, "checkout")
    )
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    enumerated = False

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal enumerated
        enumerated = True
        return (_unit(),)

    def processes(**_kwargs: object) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        return (_run_child(f"{RUN_CGROUP}/payload"),) if enumerated else ()

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", processes)

    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert enumerated
    error = capsys.readouterr().err
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in error


# The tests below leave the host-evidence seam in place: the retained handles,
# this host's real process table (read on both sides of the unit enumeration)
# and the boot id are all read for real, and the run's process is a real child.
# Only the user-systemd enumeration is supplied, because some hosts that run
# this suite have no user bus and a busy host's real unit population changes
# under the enumeration; the enumeration's own parsing has its own tests.  The
# run's unit has a unique name so no real unit or process on the host matches
# it.  Each real process-table read is recorded, so a test can show that the
# judgment read this host's table and saw (or no longer saw) the run's process.


@dataclass(frozen=True)
class _RealHost:
    project: Path
    slot_path: Path
    tree: Path
    unit: str
    tables: list[tuple[wrkslots._AbsentProcessObservation, ...]]

    def tables_with(self, generation: tuple[int, int]) -> int:
        return sum(
            any((process.pid, process.start_ticks) == generation for process in table)
            for table in self.tables
        )


def _cgroup_filesystem_type(mount_point: str) -> str | None:
    """The type of the filesystem this process sees mounted at ``mount_point``."""

    found = None
    for line in Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines():
        fields = line.split(" ")
        if len(fields) > 4 and fields[4] == mount_point and " - " in line:
            # A later mount at the same point hides an earlier one.
            found = line.split(" - ", 1)[1].split(" ")[0]
    return found


def _require_cgroup2_host() -> None:
    """Skip, saying why, where the real control-group read cannot succeed.

    The validation-run authority reads retained units' control-group members
    from the cgroup v2 hierarchy at /sys/fs/cgroup and refuses without one,
    which is the behaviour these host tests would then observe instead.
    """

    kind = _cgroup_filesystem_type("/sys/fs/cgroup")
    if kind != "cgroup2":
        pytest.skip(
            "this host has no cgroup v2 hierarchy at /sys/fs/cgroup (filesystem "
            f"type {kind or 'none'}), so the real retained-unit control-group read "
            "refuses there"
        )


@pytest.mark.parametrize("kind", ["cgroup", "tmpfs", None])
def test_a_host_without_cgroup2_skips_the_real_host_tests_with_a_reason(
    monkeypatch: pytest.MonkeyPatch, kind: str | None
) -> None:
    """A cgroup v1 or absent hierarchy is a stated skip, never a failure or a pass."""

    module = sys.modules[__name__]
    monkeypatch.setattr(module, "_cgroup_filesystem_type", lambda _mount_point: kind)
    with pytest.raises(pytest.skip.Exception) as skipped:
        _require_cgroup2_host()
    assert "no cgroup v2 hierarchy at /sys/fs/cgroup" in str(skipped.value)
    assert f"filesystem type {kind or 'none'}" in str(skipped.value)


def _prepare_real_host(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> _RealHost:
    """One validation row whose recorded owner has exited, on the real host."""

    _require_cgroup2_host()
    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    tree = checkout(project, slot="slot01", slot_type="validate")
    unit = f"wrkslots-test-{uuid.uuid4().hex}.service"
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (_unit(Id=unit, ControlGroup=f"/user.slice/app.slice/{unit}"),),
    )
    # The suite's idle-host default replaces this seam; the marker keeps it.
    assert wrkslots._validation_run_host_evidence.__module__ == wrkslots.__name__
    tables: list[tuple[wrkslots._AbsentProcessObservation, ...]] = []
    real_snapshot = wrkslots._absent_validate_process_snapshot

    def recorded_snapshot(
        proc_root: Path = Path("/proc"), *, include_owner_cgroups: bool = True
    ) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        table = real_snapshot(proc_root, include_owner_cgroups=include_owner_cgroups)
        tables.append(table)
        return table

    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", recorded_snapshot)
    return _RealHost(project, slot_path, tree, unit, tables)


@dataclass(frozen=True)
class _RunProcess:
    child: subprocess.Popen[bytes]
    generation: tuple[int, int]


@contextlib.contextmanager
def _real_run_process(host: _RealHost) -> Iterator[_RunProcess]:
    """Record a live child as the run's process generation in its handle."""

    child = subprocess.Popen(
        ["sleep", "300"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(child.pid))
        assert start_ticks is not None
        _write_run_handle(
            host.project,
            host.tree,
            unit=host.unit,
            process_identity={
                "pid": child.pid,
                "start_ticks": start_ticks,
                "boot_id": wrkslots._boot_id(Path("/proc")),
            },
        )
        yield _RunProcess(child, (child.pid, start_ticks))
    finally:
        if child.poll() is None:
            child.kill()
        child.wait()


def _end_run(host: _RealHost, run: _RunProcess) -> None:
    """The run's process exits and is reaped, so its generation is gone."""

    run.child.kill()
    run.child.wait()
    host.tables.clear()


def _assert_judged_live_run(host: _RealHost, run: _RunProcess, text: str) -> None:
    """The refusal named the run's process, and the real table showed it."""

    assert f"has live exact process generation {run.child.pid} for row slot01" in text
    # Read on both sides of the unit enumeration.
    assert host.tables_with(run.generation) >= 2


def _assert_judged_ended_run(host: _RealHost, run: _RunProcess) -> None:
    assert len(host.tables) >= 2
    assert host.tables_with(run.generation) == 0


def test_real_host_completed_removal_waits_for_the_run_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host = _prepare_real_host(tmp_path, monkeypatch)

    with _real_run_process(host) as run:
        assert _remove_completed(host.project) == 3
        error = capsys.readouterr().err
        assert "validation-run authority reports the run may still use slot slot01" in error
        _assert_judged_live_run(host, run, error)
        _assert_retained(host.project, host.tree)

        _end_run(host, run)
        assert _remove_completed(host.project) == 0, capsys.readouterr().err
        _assert_judged_ended_run(host, run)

    assert not host.slot_path.exists()
    assert active_slots(host.project) == []


def _remove_batch(project: Path) -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "slot01=1",
        ]
    )


def test_real_host_batch_seal_waits_for_the_run_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host = _prepare_real_host(tmp_path, monkeypatch)
    config = wrkslots._load_config(str(host.project), "testhost")

    with _real_run_process(host) as run:
        # A batch retains each refused slot and exits 1.
        assert _remove_batch(host.project) == 1
        output = capsys.readouterr().out
        assert (
            "RETAINED: slot01 reason=validation-run authority reports the run may "
            "still use slot slot01"
        ) in output
        _assert_judged_live_run(host, run, output)
        assert not wrkslots._validate_batch_seal_journal_path(config).exists()
        _assert_retained(host.project, host.tree)

        _end_run(host, run)
        assert _remove_batch(host.project) == 0, capsys.readouterr()
        _assert_judged_ended_run(host, run)

    assert not host.slot_path.exists()
    assert active_slots(host.project) == []


def test_real_host_finish_recovery_waits_for_the_run_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host = _prepare_real_host(tmp_path, monkeypatch)
    interrupt_validate_batch(host.project, monkeypatch, "after-finish-journal", ("slot01",))
    config = wrkslots._load_config(str(host.project), "testhost")
    finish_path = wrkslots._journal_path(config)
    assert finish_path.exists()
    recover = [
        "--project-root",
        str(host.project),
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    ]

    # A run that starts on the checkout after the batch journaled its finish.
    with _real_run_process(host) as run:
        host.tables.clear()
        assert wrkslots.main(recover) == 3
        error = capsys.readouterr().err
        _assert_judged_live_run(host, run, error)
        assert finish_path.exists()
        _assert_retained(host.project, host.tree)

        _end_run(host, run)
        assert wrkslots.main(recover) == 0, capsys.readouterr().err
        _assert_judged_ended_run(host, run)

    assert not host.slot_path.exists()
    assert not finish_path.exists()
    assert active_slots(host.project) == []


@pytest.mark.parametrize(
    ("lease_host", "expected"), [("foreign-host", "unverifiable"), (None, "dead")]
)
def test_an_ownerless_row_is_judged_local_by_its_coordinator_lease(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    lease_host: str | None,
    expected: str,
) -> None:
    """An ownerless row's lease names the host that registered it.

    With no owner, a host check that looks only at the owner accepts a row
    registered on another host and judges it ``dead`` from this host's empty
    evidence.  The same row is local when its lease names this host.
    """

    project, _tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    record = state.slots[0]
    lease = record.coordinator_lease
    if lease_host is not None:
        lease = dataclasses.replace(lease, host_id=lease_host)
    ownerless = dataclasses.replace(record, owner=None, coordinator_lease=lease)
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, ownerless),
        action="test-ownerless",
        slot=record.slot,
    )

    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )

    state_name, message = states[("testhost", "slot01", 1)]
    assert state_name == expected, message
    if expected == "unverifiable":
        assert "belongs to stable host foreign-host" in message


@pytest.mark.parametrize(
    ("view", "detail"),
    [
        ("pid", "not in the host's initial pid namespace"),
        ("cgroup", "not in the host's initial cgroup namespace"),
        ("hidepid", "/proc is mounted with hidepid=invisible"),
    ],
)
def test_a_restricted_process_view_is_unverifiable_not_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, view: str, detail: str
) -> None:
    """A process table that need not show the host's runs is not evidence.

    In a child PID namespace, or under ``hidepid``, a host run and its
    children are missing from /proc; in a child cgroup namespace their
    control-group paths are relative to another root.  The empty table and
    the empty unit list here would otherwise judge the row ``dead``.
    """

    project, _tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    if view == "hidepid":
        monkeypatch.setattr(
            wrkslots,
            "_proc_superblock_options",
            lambda: ("rw", "hidepid=invisible"),
            raising=False,
        )
    else:
        monkeypatch.setattr(
            wrkslots,
            "_namespace_inode",
            lambda name: (
                4_026_532_999 if name == view else os.stat(f"/proc/self/ns/{name}").st_ino
            ),
            raising=False,
        )
    config = wrkslots._load_config(str(project), "testhost")

    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )

    state, message = states[("testhost", "slot01", 1)]
    assert state == "unverifiable", message
    assert detail in message


_HOST_MOUNTS = (
    "1 0 0:1 / / rw - ext4 /dev/root rw\n"
    "2 1 0:2 / /proc rw,nosuid - proc proc rw\n"
)


@pytest.mark.parametrize(
    ("mount", "detail"),
    [
        ("3 2 0:3 / /proc/4242 rw - tmpfs tmpfs rw\n", "tmpfs mount at /proc/4242 masks"),
        ("3 2 0:3 / /proc/4242/fd rw - tmpfs tmpfs rw\n", "mount at /proc/4242/fd masks"),
        (
            "3 2 0:3 / /proc/77/with\\040space rw - tmpfs tmpfs rw\n",
            "mount at /proc/77/with space masks",
        ),
        ("3 2 0:3 / /proc rw - tmpfs tmpfs rw\n", "no process filesystem is mounted at /proc"),
        ("3 2 0:3 / /proc/sys/fs/binfmt_misc rw - binfmt_misc binfmt_misc rw\n", None),
        (
            "3 2 0:3 / /proc/4242 rw - tmpfs tmpfs rw\n4 2 0:4 / /proc rw - proc proc rw\n",
            None,
        ),
        (
            "3 2 0:4 / /proc rw - proc proc rw\n4 3 0:3 / /proc/4242 rw - tmpfs tmpfs rw\n",
            "tmpfs mount at /proc/4242 masks",
        ),
    ],
)
def test_a_mount_over_a_process_directory_in_proc_restricts_the_view(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mount: str, detail: str | None
) -> None:
    """A mount over ``/proc/<pid>`` replaces that process's entries.

    The process table then need not show that run, so the view is
    restricted; a mount stacked over ``/proc`` itself hides the process
    filesystem.  A mount elsewhere below ``/proc`` hides no process, and
    neither does a mount over ``/proc/<pid>`` that a process filesystem
    mounted later over ``/proc`` hides; a mount over ``/proc/<pid>`` of that
    later process filesystem still does.
    """

    table = tmp_path / "mountinfo"
    table.write_text(_HOST_MOUNTS + mount, encoding="utf-8")
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", table)
    initial = dict(wrkslots._INITIAL_NAMESPACE_INODES)
    monkeypatch.setattr(wrkslots, "_namespace_inode", lambda name: initial[name])

    if detail is None:
        wrkslots._assert_host_process_view()
        return
    with pytest.raises(wrkslots.Refusal) as refused:
        wrkslots._assert_host_process_view()
    assert detail in str(refused.value)


def test_this_host_process_view_is_the_host_view() -> None:
    """The suite runs on the host, so the real view passes the proof."""

    wrkslots._assert_host_process_view()


@pytest.mark.parametrize("form", ["symlink", "relative"])
def test_completed_validation_removal_refuses_a_queued_unit_naming_the_row_by_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    form: str,
) -> None:
    """A queued job names the row through a symlink or a relative path.

    Neither spelling contains the row's text, and the job has not started,
    so no process or path census can show it later.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    slot_directory = _slot_directory(project)
    if form == "symlink":
        alias = tmp_path / "checkout-alias"
        alias.symlink_to(slot_directory)
        evidence = {"ExecStart": f"/usr/bin/make\n-C\n{alias}/product"}
    else:
        evidence = {
            "WorkingDirectory": str(slot_directory.parent.parent),
            "ExecStart": f"git\n-C\n{slot_directory.parent.name}/{slot_directory.name}",
        }
    queued = _unit(Id="queued-run.service", PendingJob="yes", **evidence)
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (queued,))

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "user-systemd unit queued-run.service names validation row slot01" in error
    _assert_retained(project, tree)


@pytest.mark.parametrize("form", ["/other{row}", "{row}+other"])
def test_completed_validation_removal_ignores_a_unit_naming_a_longer_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    form: str,
) -> None:
    """A path that contains the row's text is not the row's path."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    other = form.format(row=_slot_directory(project))
    running = _unit(
        Id="other-run.service",
        ActiveState="active",
        SubState="running",
        ExecStart=f"/usr/bin/env\n--checkout={other}",
        Environment=f"RUN_ROOT={other}",
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (running,))

    removed = _remove_completed(project)

    assert removed == 0, capsys.readouterr().err
    assert not tree.exists()
    assert active_slots(project) == []


class _Interrupted(RuntimeError):
    pass


def _interrupt_once_at(monkeypatch: pytest.MonkeyPatch, boundary: str) -> None:
    def interrupt(point: str) -> None:
        if point == boundary:
            monkeypatch.setattr(wrkslots, "_interrupt_for_test", lambda _point: None)
            raise _Interrupted(point)

    monkeypatch.setattr(wrkslots, "_interrupt_for_test", interrupt)


@pytest.mark.parametrize("evidence", ["queued-unit", "run-handle"])
def test_finish_recovery_judges_the_fenced_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    evidence: str,
) -> None:
    """Finish recovery moves the checkout to ``.slot01.fenced.1.<hex>`` and
    deletes it there.

    The interrupted removal's finish journal names the fence before the
    slot is renamed.  A queued job or a run handle naming the fenced
    checkout will use the files recovery is about to delete, although
    neither names the recorded path.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    slot_path = _slot_directory(project)
    _interrupt_once_at(monkeypatch, "after-finish-journal")
    with pytest.raises(_Interrupted):
        _remove_completed(project)
    config = wrkslots._load_config(str(project), "testhost")
    journal_paths = [
        path
        for path in (
            wrkslots._journal_path(config),
            wrkslots._finish_journal_path(config, "slot01"),
        )
        if path.exists()
    ]
    assert len(journal_paths) == 1, journal_paths
    journal = json.loads(journal_paths[0].read_text(encoding="utf-8"))
    assert journal["kind"] == "finish"
    fenced_slot = project / journal["fenced"]
    assert fenced_slot.name.startswith(".slot01.fenced.1.")
    assert not fenced_slot.exists()
    fenced_tree = fenced_slot / tree.relative_to(slot_path)
    if evidence == "queued-unit":
        unit = _unit(
            Id="queued-run.service",
            PendingJob="yes",
            ExecStart=f"/usr/bin/make\n-C\n{fenced_tree}",
        )
        expected = "user-systemd unit queued-run.service names validation row slot01"
    else:
        _write_run_handle(project, fenced_tree)
        unit = _unit(ActiveState="active", SubState="running")
        expected = f"retained validation unit {RUN_UNIT} may still use row slot01"
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (unit,))
    recover = [
        "--project-root",
        str(project),
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    ]

    assert wrkslots.main(recover) == 3
    error = capsys.readouterr().err
    assert "validation-run authority reports the run may still use slot slot01" in error
    assert expected in error
    assert tree.is_dir()
    assert len(active_slots(project)) == 1

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())
    assert wrkslots.main(recover) == 0, capsys.readouterr().err
    assert not fenced_slot.exists()
    assert not slot_path.exists()
    assert active_slots(project) == []


def test_a_unit_naming_a_fenced_checkout_on_disk_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A slot already renamed to its fence is judged at the fenced path.

    Every ``.slot01.fenced.1.*`` sibling on disk is a place the row's files
    may be, whichever operation renamed it there.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    slot_path = _slot_directory(project)
    fenced_slot = slot_path.parent / f".slot01.fenced.1.{uuid.uuid4().hex}"
    slot_path.rename(fenced_slot)
    fenced_tree = fenced_slot / tree.relative_to(slot_path)
    queued = _unit(
        Id="queued-run.service", PendingJob="yes", ExecStart=f"make\n-C\n{fenced_tree}"
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (queued,))
    config = wrkslots._load_config(str(project), "testhost")

    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )

    state, message = states[("testhost", "slot01", 1)]
    assert state == "alive", message
    assert "user-systemd unit queued-run.service names validation row slot01" in message


def _judge_with_unit_enumerations(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    enumerations: Sequence[Sequence[Mapping[str, str]]],
    *,
    register_during: int | None = None,
    members: _MemberReader | None = None,
) -> tuple[str, str]:
    """Judge one row whose user-systemd units change between enumerations.

    The process tables are empty throughout: the run's processes started
    after each table's PID list.  With ``register_during`` set, the run's
    handle is written during that enumeration instead of beforehand.
    ``members`` replaces the retained-unit control-group read.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    if members is not None:
        monkeypatch.setattr(wrkslots, "_retained_unit_cgroup_members", members)
    if register_during is None:
        _write_run_handle(project, tree)
    calls = 0

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal calls
        if calls == register_during:
            _write_run_handle(project, tree)
        value = tuple(enumerations[min(calls, len(enumerations) - 1)])
        calls += 1
        return value

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    config = wrkslots._load_config(str(project), "testhost")
    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )
    assert len(states) == 1
    return next(iter(states.values()))


def test_a_run_that_starts_during_the_later_process_table_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The first enumeration predates the run; the second sees it running."""

    state, message = _judge_with_unit_enumerations(
        tmp_path,
        monkeypatch,
        ((_unit(),), (_unit(ActiveState="active", SubState="running"),)),
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in message


def test_a_run_registered_during_the_evidence_reads_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A handle written after the first handle read is still judged."""

    state, message = _judge_with_unit_enumerations(
        tmp_path,
        monkeypatch,
        ((_unit(ActiveState="active", SubState="running"),),),
        register_during=0,
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in message


def test_a_handle_registered_after_the_first_enumeration_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A handle the later read found is a run the unit states cannot cover.

    The handle is written during the second user-systemd enumeration, and
    its unit is absent from both: it is queued only after them, and its job
    has not started, so no table, unit state or control group shows it.
    """

    state, message = _judge_with_unit_enumerations(
        tmp_path, monkeypatch, ((), ()), register_during=1
    )

    assert state == "alive", message
    assert "for row slot01 appeared while the host evidence was read" in message
    assert f"its unit {RUN_UNIT} may have been queued" in message


def _change_run_handle(handle: Path, tree: Path, change: str) -> None:
    """Rewrite, replace or remove a run handle as a launcher registering again would."""

    if change == "rewritten":
        value = json.loads(handle.read_text(encoding="utf-8"))
        value["checkout"] = str(tree / "after")
        handle.write_text(json.dumps(value), encoding="utf-8")
    elif change == "replaced":
        fresh = handle.with_name(handle.name + ".new")
        fresh.write_bytes(handle.read_bytes())
        os.replace(fresh, handle)
    else:
        handle.unlink()


_HANDLE_CHANGES = {
    "rewritten": "changed",
    "replaced": "changed",
    "removed": "disappeared or stopped naming the row",
}


@pytest.mark.parametrize("change", sorted(_HANDLE_CHANGES))
def test_a_handle_that_changes_during_the_evidence_reads_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    """A handle rewritten, replaced or removed while the host is read is a run event.

    The handle names the row before and after (``rewritten`` moves its
    checkout within the row; ``replaced`` puts a byte-identical file in its
    place), so the projected fields the two reads find can be equal.  The
    unit is absent from both user-systemd enumerations and every process
    table: a run registered again then is queued only after them.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    handle = _write_run_handle(project, tree / "before")
    calls = 0

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal calls
        if calls == 1:
            _change_run_handle(handle, tree, change)
        calls += 1
        return ()

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    config = wrkslots._load_config(str(project), "testhost")
    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )

    state, message = states[("testhost", "slot01", 1)]
    assert state == "alive", message
    assert f"for row slot01 {_HANDLE_CHANGES[change]} while the host evidence" in message
    assert f"its unit {RUN_UNIT} may have been queued" in message
    # With the handle left as it is, the same evidence reads the run as over.
    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )
    assert states[("testhost", "slot01", 1)][0] == "dead", states


def test_a_retained_unit_cgroup_member_missing_from_every_table_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The run started and finished between reads and left a child behind.

    Both enumerations read the unit inactive and unqueued, and neither
    process table holds the child; only its control group does.
    """

    read: list[set[str]] = []

    def members(units: AbstractSet[str], *, root: Path | None = None) -> Mapping[str, int]:
        read.append(set(units))
        return {name: 1 for name in units}

    state, message = _judge_with_unit_enumerations(
        tmp_path, monkeypatch, ((_unit(),), (_unit(),)), members=members
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} control group now holds 1" in message
    assert read == [{RUN_UNIT}]


def test_retained_unit_cgroup_members_counts_the_unit_and_its_descendants(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "cgroup"
    root.mkdir()
    (root / "cgroup.controllers").write_text("cpu memory\n")
    manager = root / "user.slice" / f"user-{os.getuid()}.slice" / f"user@{os.getuid()}.service"
    run = manager / "app.slice" / RUN_UNIT
    (run / "payload").mkdir(parents=True)
    (run / "cgroup.procs").write_text("101\n102\n")
    (run / "payload" / "cgroup.procs").write_text("103\n")
    other = manager / "app.slice" / "other.service"
    other.mkdir()
    (other / "cgroup.procs").write_text("201\n")
    absent = "validate-run-0002.service"
    monkeypatch.setattr(wrkslots, "_user_manager_cgroup", lambda _root: manager)

    assert wrkslots._retained_unit_cgroup_members({RUN_UNIT, absent}, root=root) == {
        RUN_UNIT: 3,
        absent: 0,
    }
    (root / "cgroup.controllers").unlink()
    with pytest.raises(wrkslots.Refusal, match="no cgroup v2 hierarchy"):
        wrkslots._retained_unit_cgroup_members({RUN_UNIT}, root=root)
    assert wrkslots._retained_unit_cgroup_members(set(), root=root) == {}


def test_this_host_user_manager_cgroup_is_read() -> None:
    """On this host the reader finds the manager and a fresh unit is empty."""

    _require_cgroup2_host()
    unit = f"wrkslots-test-{uuid.uuid4().hex}.service"
    assert wrkslots._retained_unit_cgroup_members({unit}) == {unit: 0}
    assert wrkslots._user_manager_cgroup(Path("/sys/fs/cgroup")).name == (
        f"user@{os.getuid()}.service"
    )


def test_an_invisible_user_manager_cgroup_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A user manager missing from the hierarchy is no evidence of no members."""

    root = tmp_path / "cgroup"
    root.mkdir()
    (root / "cgroup.controllers").write_text("cpu memory\n")
    missing = root / "user.slice" / f"user-{os.getuid()}.slice" / f"user@{os.getuid()}.service"
    monkeypatch.setattr(wrkslots, "_user_manager_cgroup", lambda _root: missing)

    with pytest.raises(wrkslots.Refusal, match="service manager control group .* is not visible"):
        wrkslots._retained_unit_cgroup_members({RUN_UNIT}, root=root)


@pytest.mark.parametrize(
    ("mount", "detail"),
    [
        (
            "2 1 0:2 /user.slice /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n",
            "the cgroup v2 mount at /sys/fs/cgroup shows only /user.slice of the hierarchy",
        ),
        (
            "2 1 0:2 / /sys/fs/cgroup rw - tmpfs tmpfs rw\n",
            "no cgroup v2 hierarchy is mounted at /sys/fs/cgroup",
        ),
        (
            "2 1 0:2 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n"
            "3 2 0:3 / /sys/fs/cgroup rw - tmpfs tmpfs rw\n",
            "no cgroup v2 hierarchy is mounted at /sys/fs/cgroup",
        ),
    ],
)
def test_a_partial_cgroup_mount_refuses_the_member_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mount: str, detail: str
) -> None:
    """Only a cgroup v2 mount of the whole hierarchy at /sys/fs/cgroup is read.

    A mount of a subtree shows the control groups below it and none
    outside it, and another file system stacked over the hierarchy hides
    it; either way a unit's control group would read as empty.
    """

    table = tmp_path / "mountinfo"
    table.write_text("1 0 0:1 / / rw - ext4 /dev/root rw\n" + mount, encoding="utf-8")
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", table)

    with pytest.raises(wrkslots.Refusal) as refused:
        wrkslots._retained_unit_cgroup_members({RUN_UNIT})
    assert detail in str(refused.value)


_CGROUP_HIERARCHY = "2 1 0:2 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n"
_RUN_UNIT_CGROUP = (
    f"/sys/fs/cgroup/user.slice/user-{os.getuid()}.slice/user@{os.getuid()}.service"
    f"/app.slice/{RUN_UNIT}"
)


@pytest.mark.parametrize(
    ("mount", "point"),
    [
        (
            "3 2 0:3 / /sys/fs/cgroup/user.slice rw - tmpfs tmpfs rw\n",
            "/sys/fs/cgroup/user.slice",
        ),
        (
            f"3 2 0:3 /empty {_RUN_UNIT_CGROUP}/cgroup.procs rw - tmpfs tmpfs rw\n",
            f"{_RUN_UNIT_CGROUP}/cgroup.procs",
        ),
    ],
    ids=["file-system-over-a-slice", "file-over-a-member-list"],
)
def test_a_visible_mount_below_the_cgroup_hierarchy_refuses_the_member_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mount: str, point: str
) -> None:
    """A mount below /sys/fs/cgroup replaces the control groups it covers.

    The hierarchy-root proof passes for both tables: the mount at
    /sys/fs/cgroup is the whole cgroup v2 tree.  A file system over a slice
    shows none of its control groups, and an empty file bind-mounted over a
    unit's ``cgroup.procs`` lists no member, so the unit would read empty.
    """

    table = tmp_path / "mountinfo"
    table.write_text(
        "1 0 0:1 / / rw - ext4 /dev/root rw\n" + _CGROUP_HIERARCHY + mount, encoding="utf-8"
    )
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", table)

    with pytest.raises(wrkslots.Refusal) as refused:
        wrkslots._retained_unit_cgroup_members({RUN_UNIT})
    assert f"a tmpfs mount at {point} covers part of the cgroup v2 hierarchy" in str(
        refused.value
    )


def test_a_hidden_mount_below_the_cgroup_hierarchy_is_not_read_as_a_cover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A mount that a later whole-hierarchy mount at /sys/fs/cgroup hides
    covers nothing that path lookup reaches, so the hierarchy is whole."""

    covered = (
        "1 0 0:1 / / rw - ext4 /dev/root rw\n"
        + _CGROUP_HIERARCHY
        + "3 2 0:3 / /sys/fs/cgroup/user.slice rw - tmpfs tmpfs rw\n"
    )
    table = tmp_path / "mountinfo"
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", table)
    table.write_text(covered, encoding="utf-8")
    with pytest.raises(wrkslots.Refusal, match="covers part of the cgroup v2 hierarchy"):
        wrkslots._assert_whole_cgroup2_hierarchy(Path("/sys/fs/cgroup"))

    table.write_text(
        covered + "4 2 0:2 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n", encoding="utf-8"
    )
    wrkslots._assert_whole_cgroup2_hierarchy(Path("/sys/fs/cgroup"))


@pytest.mark.parametrize(
    ("table", "check"),
    [
        (
            "2 1 0:2 / /proc rw - proc proc rw\n"
            "3 2 0:3 / /proc/4242 rw - tmpfs tmpfs rw\n"
            "4 2 0:2 / /proc rw - proc proc rw\n",
            "process-view",
        ),
        (
            "2 1 0:2 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n"
            "3 2 0:3 / /sys/fs/cgroup/user.slice rw - tmpfs tmpfs rw\n"
            "4 2 0:2 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n",
            "cgroup-hierarchy",
        ),
    ],
)
def test_a_hidden_mount_is_not_a_cover_where_the_root_is_not_a_mount_point(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, table: str, check: str
) -> None:
    """A process whose root is not a mount point sees no mount at "/", and
    the mounts on the unlisted mount above the table are where lookup
    starts.  Mount 4, stacked over the same point later, hides mount 3, so
    neither proof reads mount 3 as a cover; without mount 4 both refuse.
    """

    path = tmp_path / "mountinfo"
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", path)
    initial = dict(wrkslots._INITIAL_NAMESPACE_INODES)
    monkeypatch.setattr(wrkslots, "_namespace_inode", lambda name: initial[name])

    def prove() -> None:
        if check == "process-view":
            wrkslots._assert_host_process_view()
        else:
            wrkslots._assert_whole_cgroup2_hierarchy(Path("/sys/fs/cgroup"))

    covered = "".join(table.splitlines(keepends=True)[:2])
    path.write_text(covered, encoding="utf-8")
    with pytest.raises(wrkslots.Refusal, match="masks|covers part"):
        prove()
    path.write_text(table, encoding="utf-8")
    prove()
    entries = wrkslots._mount_entries(table.encode(), "table")
    hidden = wrkslots._visible_mount(entries, entries[1].point)
    assert hidden is not None and hidden.mount_id == 4


@pytest.mark.parametrize("check", ["process-view", "cgroup-hierarchy"])
def test_finding_the_visible_mounts_of_a_large_table_is_linear(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, check: str
) -> None:
    """3,000 mounts below /proc or /sys/fs/cgroup, all hidden by a later
    mount stacked over that point, are checked in one pass over the table.

    Finding each entry's visible mount anew rebuilt the whole mount graph
    per entry, so this table (about 160 KB, inside the 4 MiB bound) took
    31 seconds; the bound here leaves room for a loaded host.
    """

    point = "/proc" if check == "process-view" else "/sys/fs/cgroup"
    fstype = "proc" if check == "process-view" else "cgroup2"
    lines = ["1 0 0:1 / / rw - ext4 /dev/root rw\n", f"2 1 0:2 / {point} rw - {fstype} x rw\n"]
    lines.extend(
        f"{10 + index} 2 0:3 / {point}/{100000 + index} rw - tmpfs tmpfs rw\n"
        for index in range(3_000)
    )
    lines.append(f"9999 2 0:2 / {point} rw - {fstype} x rw\n")
    path = tmp_path / "mountinfo"
    path.write_text("".join(lines), encoding="utf-8")
    monkeypatch.setattr(wrkslots, "_SELF_MOUNTINFO", path)
    initial = dict(wrkslots._INITIAL_NAMESPACE_INODES)
    monkeypatch.setattr(wrkslots, "_namespace_inode", lambda name: initial[name])

    started = time.monotonic()
    if check == "process-view":
        wrkslots._assert_host_process_view()
    else:
        wrkslots._assert_whole_cgroup2_hierarchy(Path(point))
    assert time.monotonic() - started < 5.0


def test_this_host_counts_the_member_of_this_process_unit() -> None:
    """The real read finds this test process in its own unit's control group.

    The unit is the deepest ``.scope`` or ``.service`` part of this
    process's control group below its user manager, so a reader that
    counted nothing would fail here.
    """

    _require_cgroup2_host()
    if os.stat("/proc/self/ns/cgroup").st_ino != dict(wrkslots._INITIAL_NAMESPACE_INODES)[
        "cgroup"
    ]:
        pytest.skip(
            "this process is not in the initial cgroup namespace, so its control-group "
            "path is not a path in the host hierarchy"
        )
    path = next(
        line.split(":", 2)[2]
        for line in Path("/proc/self/cgroup").read_text(encoding="utf-8").splitlines()
        if line.startswith("0::")
    )
    parts = Path(path).parts
    manager = f"user@{os.getuid()}.service"
    if manager not in parts:
        pytest.skip(
            f"this process's control group is not below {manager}, so no unit of the "
            "user manager holds it"
        )
    units = [
        part
        for part in parts[parts.index(manager) + 1 :]
        if part.endswith((".scope", ".service"))
    ]
    if not units:
        pytest.skip(f"this process's control group names no unit below {manager}")

    members = wrkslots._retained_unit_cgroup_members({units[-1]})

    assert members[units[-1]] >= 1, (units[-1], members)


def _restrict_pid_namespace(monkeypatch: pytest.MonkeyPatch) -> None:
    """This process is in a child PID namespace."""

    real = wrkslots._namespace_inode
    monkeypatch.setattr(
        wrkslots,
        "_namespace_inode",
        lambda name: 4_026_532_999 if name == "pid" else real(name),
    )


def _prepare_absent_validate_recovery(
    project: Path, repository: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, Path]:
    """One absent validation row with a retained run handle, and its input."""

    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    config = wrkslots._load_config(str(project), "testhost")
    _write_run_handle(
        project, wrkslots._stored_path(config, record.checkouts[0].path, "checkout")
    )
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (_unit(),))
    monkeypatch.setattr(wrkslots, "_retained_unit_cgroup_members", _no_members)
    return project, input_path


def _one_member(units: AbstractSet[str], *, root: Path | None = None) -> Mapping[str, int]:
    return dict.fromkeys(units, 1)


_RECOVERY_EVIDENCE = ["restricted-view", "cgroup-member", "rewritten-handle"]


def _late_recovery_evidence(
    project: Path, tree: Path, monkeypatch: pytest.MonkeyPatch, evidence: str
) -> str:
    """Make the host evidence restricted or late; return the expected refusal."""

    if evidence == "restricted-view":
        _restrict_pid_namespace(monkeypatch)
        return "not in the host's initial pid namespace"
    if evidence == "cgroup-member":
        monkeypatch.setattr(wrkslots, "_retained_unit_cgroup_members", _one_member)
        return f"retained validation unit {RUN_UNIT} control group now holds 1"
    handle = project / "ignored" / "validate" / "runs" / (
        RUN_UNIT.removesuffix(".service") + ".json"
    )
    calls = 0

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal calls
        if calls == 0:
            _change_run_handle(handle, tree, "rewritten")
        calls += 1
        return (_unit(),)

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    return "changed while the host evidence was read"


@pytest.mark.parametrize("evidence", _RECOVERY_EVIDENCE)
def test_absent_validate_row_recovery_reads_the_same_late_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    evidence: str,
) -> None:
    """recover-absent-validate-rows proves the host view and reads members.

    In a child PID namespace a run is missing from the process table, and a
    run's child can be missing from every table but still in its control
    group.  Either way the empty tables are no evidence that the row is free.
    A run handle rewritten while the host is read is a run registered again,
    whose unit may be queued after the enumerations.
    """

    project, repository, _remote = make_project(tmp_path)
    project, input_path = _prepare_absent_validate_recovery(project, repository, monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    record = next(
        record for record in wrkslots._load_active(config).slots if record.slot == "gone"
    )
    tree = wrkslots._stored_path(config, record.checkouts[0].path, "checkout")
    expected = _late_recovery_evidence(project, tree, monkeypatch, evidence)

    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert expected in capsys.readouterr().err

    monkeypatch.undo()
    _prepare_absent_validate_recovery_host(project, monkeypatch)
    assert run_absent_validate_recovery(project, input_path, apply=False) == 0, (
        capsys.readouterr().err
    )


def _prepare_absent_validate_recovery_host(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (_unit(),))
    monkeypatch.setattr(wrkslots, "_retained_unit_cgroup_members", _no_members)


@pytest.mark.parametrize("evidence", _RECOVERY_EVIDENCE)
def test_absent_agent_row_recovery_reads_the_same_late_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    evidence: str,
) -> None:
    """recover-absent-agent-row reads the evidence through the same helper.

    A validation run may use an agent checkout as well, so a retained run
    handle naming the agent row's checkout is judged as for a validation row.
    """

    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository)
    config = wrkslots._load_config(str(project), "testhost")
    tree = wrkslots._stored_path(config, record.checkouts[0].path, "checkout")
    _write_run_handle(project, tree)
    _prepare_absent_validate_recovery_host(project, monkeypatch)
    expected = _late_recovery_evidence(project, tree, monkeypatch, evidence)

    assert run_absent_agent_recovery(project, record, apply=False) == 3
    assert expected in capsys.readouterr().err

    monkeypatch.undo()
    _prepare_absent_validate_recovery_host(project, monkeypatch)
    assert run_absent_agent_recovery(project, record, apply=False) == 0, (
        capsys.readouterr().err
    )


def test_deferred_batch_completion_judges_the_fenced_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A batch completes each removal after a census outside the locks.

    ``remove-validate-batch`` fences the slot to ``.slot01.fenced.1.<hex>``,
    takes a fresh census of the fence outside the mutation locks, and then
    completes the removal under them (``_complete_prepared_private_finish``).
    A job queued against the fenced checkout in that gap names no recorded
    path, and the completion deletes the files it will use.
    """

    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    tree = checkout(project, slot="slot01", slot_type="validate")
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda **_k: ())
    monkeypatch.setattr(wrkslots, "_retained_unit_cgroup_members", _no_members, raising=False)
    boundaries: list[str] = []
    monkeypatch.setattr(wrkslots, "_interrupt_for_test", boundaries.append)
    queue = True

    def units() -> tuple[Mapping[str, str], ...]:
        fences = sorted(slot_path.parent.glob(".slot01.fenced.1.*"))
        if not queue or "after-validate-batch-fresh-census" not in boundaries:
            return ()
        assert len(fences) == 1, fences
        fenced_tree = fences[0] / tree.relative_to(slot_path)
        return (
            _unit(
                Id="queued-run.service",
                PendingJob="yes",
                ExecStart=f"/usr/bin/make\n-C\n{fenced_tree}",
            ),
        )

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)

    assert _remove_batch(project) == 1
    output = capsys.readouterr().out
    assert "after-validate-batch-fresh-census" in boundaries
    assert "RETAINED: slot01" in output
    assert "user-systemd unit queued-run.service names validation row slot01" in output
    _assert_retained(project, tree)

    queue = False
    boundaries.clear()
    assert _remove_batch(project) == 0, capsys.readouterr()
    assert "after-validate-batch-fresh-census" in boundaries
    assert not slot_path.exists()
    assert active_slots(project) == []
