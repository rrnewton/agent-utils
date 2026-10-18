"""#341 wrkslots-orphan-retire: removing a dead-owner slot whose agent no authority records.

A slot made for an in-process subagent, or held under a placeholder name, names an agent the
registered liveness command has never recorded. Once its owner process has died nothing can ever
report that agent dead, so plain ``remove`` refuses forever. ``remove`` accepts the coordinator's
explicit, recorded attestation in place of the command's "dead" only for the command's exact
"no record of this agent" answer and a proven-dead owner generation; every other answer and every
other guard keeps its meaning. These tests use the real agentctl probe over an empty registry
unless a case replaces it with exact output.
"""

from __future__ import annotations

import dataclasses
import datetime as dt
import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from wrkslots import cli as w
from wrkslots.tests import test_lifecycle as t

PROBE = Path(t.PACKAGE_ROOT) / "examples" / "agentctl_liveness_probe.py"
AGENT = "hold-demo"
NOTE = "in-process subagent; its task returned at 10:02Z"


def _identity(pid: int) -> dict[str, object]:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
    ticks = int(stat[stat.rindex(")") + 2 :].split()[19])
    boot = Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
    return {"pid": pid, "starttime_ticks": ticks, "boot_id": boot, "version": 1}


def _project(tmp_path: Path, *, owner: str = "dead") -> Path:
    project, repository, _remote = t.make_project(tmp_path)
    made = t.create(project, agent=AGENT, bind_owner=owner != "unrecorded")
    assert made.returncode == 0, made.stderr
    t.commit_task(repository, t.checkout(project), "codex/task")
    if owner == "dead":
        t.mark_owner_dead(project)
    elif owner in ("live", "unrecorded"):
        t.expire_heartbeat(project)
    elif owner == "dead-unexpired":
        t.mark_owner_dead(project, expire=False)
    elif owner == "other-host":
        _owner_on_another_machine(project)
    else:
        raise AssertionError(owner)
    shutil.copy(PROBE, project / "liveness.py")
    (project / ".agentctl").mkdir()
    return project


def _owner_on_another_machine(project: Path) -> None:
    """An owner recorded under another machine identity: its state is indeterminate."""

    config = w._load_config(str(project), "testhost")
    state = w._load_active(config)
    record = state.slots[0]
    assert record.owner is not None
    heartbeat_at = (
        dt.datetime.now(dt.timezone.utc)
        - dt.timedelta(seconds=record.heartbeat_ttl_seconds + 1)
    ).isoformat(timespec="seconds")
    updated = dataclasses.replace(
        record,
        owner=dataclasses.replace(record.owner, host_id="0" * 32),
        heartbeat_at=heartbeat_at,
    )
    w._write_active_state(
        config, w._replace_record(state, updated), action="test-owner-elsewhere", slot=record.slot
    )


def _attested_remove(
    project: Path, *extra: str, agent: str = AGENT, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return t.command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--attest-agent-stopped",
        agent,
        "--attest-note",
        NOTE,
        *extra,
        env=env,
    )


def _events(project: Path) -> list[dict[str, object]]:
    return [dict(event) for event in w._load_events(w._load_config(str(project), "testhost"))]


def _attested_events(project: Path) -> list[dict[str, object]]:
    return [
        event
        for event in _events(project)
        if event["kind"] == "active-state-recorded"
        and w._as_mapping(event["payload"], "payload")["action"] == w.ATTESTED_STOP_ACTION
    ]


def _retained(project: Path) -> None:
    assert t.checkout(project).is_dir()
    assert len(t.active_slots(project)) == 1
    assert _attested_events(project) == []


def _evidence(event: dict[str, object]) -> dict[str, object]:
    payload = w._as_mapping(event["payload"], "payload")
    return dict(w._as_mapping(payload["evidence"], "evidence"))


def test_plain_remove_names_the_attested_rerun_and_changes_nothing(tmp_path: Path) -> None:
    project = _project(tmp_path)
    before = len(_events(project))

    plain = t.remove(project)

    assert plain.returncode == 3, plain.stderr
    assert "has no record of agent hold-demo" in plain.stderr
    assert "reason=no-agentctl-record" in plain.stderr
    assert "--attest-agent-stopped hold-demo --attest-note TEXT" in plain.stderr
    assert "an agent registered elsewhere looks unregistered here" in plain.stderr
    _retained(project)
    assert len(_events(project)) == before


def test_attested_stop_removes_an_owner_dead_slot_whose_agent_has_no_record(
    tmp_path: Path,
) -> None:
    project = _project(tmp_path)

    removed = _attested_remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not t.checkout(project).exists()
    assert t.active_slots(project) == []
    events = _events(project)
    [attested] = _attested_events(project)
    evidence = _evidence(attested)
    assert evidence["basis"] == w.ATTESTED_STOP_BASIS
    assert evidence["note"] == NOTE
    assert evidence["agent"] == AGENT
    assert evidence["generation"] == 1
    assert evidence["owner_state"] == "dead"
    # mark_owner_dead records a boot that ended: dead, not indeterminate.
    assert evidence["owner_detail"] == "the recorded machine boot ended"
    assert evidence["coordinator_authorized"] is True
    assert evidence["registered_liveness"] == "unverifiable"
    assert "reason=no-agentctl-record" in str(evidence["registered_liveness_detail"])
    started = next(event for event in events if event["kind"] == "reclaim-started")
    assert events.index(started) > events.index(attested)
    assert w._as_mapping(started["payload"], "p")["registered_liveness"] == "unverifiable"
    reclaim = next(
        event
        for event in events
        if event["kind"] == "active-state-recorded"
        and w._as_mapping(event["payload"], "p")["action"] == "reclaim-evidence-recorded"
    )
    assert _evidence(reclaim)["basis"] == w.ATTESTED_STOP_BASIS
    assert _evidence(reclaim)["attestation_note"] == NOTE


def test_attested_stop_requires_coordinator_authorization(tmp_path: Path) -> None:
    project = _project(tmp_path)
    refused = t.raw_command(
        project, "remove", "slot01", "--coordinator-pid", str(os.getpid()),
        "--expected-generation", "1", "--attest-agent-stopped", AGENT, "--attest-note", NOTE,
    )
    assert refused.returncode == 3
    assert "requires --coordinator-authorized" in refused.stderr
    _retained(project)


@pytest.mark.parametrize(
    "arguments",
    [
        pytest.param(("--attest-agent-stopped", AGENT), id="no-note"),
        pytest.param(("--attest-note", NOTE), id="no-agent"),
        pytest.param(("--attest-agent-stopped", AGENT, "--attest-note", " "), id="blank-note"),
        pytest.param(("--attest-agent-stopped", AGENT, "--attest-note", "a\nb"), id="two-lines"),
        pytest.param(
            ("--attest-agent-stopped", AGENT, "--attest-note", "x" * 1025), id="too-long"
        ),
        pytest.param(("--attest-agent-stopped", "Bad Name", "--attest-note", NOTE), id="bad-name"),
    ],
)
def test_attested_stop_requires_one_agent_and_one_note_line(
    tmp_path: Path, arguments: tuple[str, ...]
) -> None:
    project = _project(tmp_path)
    refused = t.command(
        project, "remove", "slot01", "--coordinator-pid", str(os.getpid()),
        "--expected-generation", "1", *arguments,
    )
    assert refused.returncode == 3, refused.stderr
    _retained(project)


def test_attested_stop_refuses_another_agent_name(tmp_path: Path) -> None:
    project = _project(tmp_path)
    refused = _attested_remove(project, agent="someone-else")
    assert refused.returncode == 3
    assert "recorded for agent hold-demo" in refused.stderr
    _retained(project)


@pytest.mark.parametrize(
    ("stdout", "status"),
    [
        pytest.param(
            b"alive agent=hold-demo rc=1 reason=active-record-process-alive\n", 1, id="alive"
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=2 reason=active-record-not-stopped\n", 2,
            id="not-stopped",
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=2 reason=rename-incomplete\n", 2, id="rename"
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=2 reason=archived-record-pane-still-occupied\n",
            2,
            id="pane-occupied",
        ),
        pytest.param(
            b"unverifiable agent=other rc=2 reason=no-agentctl-record\n", 2, id="other-agent"
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=2 reason=no-agentctl-record\nmore\n", 2,
            id="two-lines",
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=2 reason=no-agentctl-record reason=x\n", 2,
            id="two-reasons",
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=3 reason=no-agentctl-record\n", 3, id="rc3"
        ),
        pytest.param(
            b"unverifiable agent=hold-demo rc=2 reason=no-agentctl-registry\n", 2,
            id="no-registry",
        ),
    ],
)
def test_attested_stop_refuses_every_other_liveness_answer(
    tmp_path: Path, stdout: bytes, status: int
) -> None:
    project = _project(tmp_path)
    t.write_liveness_output(project, stdout, b"", status)
    refused = _attested_remove(project)
    assert refused.returncode == 3, refused.stderr
    assert "--attest-agent-stopped hold-demo" not in refused.stderr
    _retained(project)


def test_a_registered_agent_the_authority_cannot_verify_still_refuses(
    tmp_path: Path,
) -> None:
    """The real probe over an active record with nothing alive behind it: still refused.

    This is #341's acceptance (2): an agent that was registered and might still live is
    not "unregistered", whatever the coordinator attests.
    """

    project = _project(tmp_path)
    child = subprocess.Popen(["true"])
    exited = _identity(child.pid)
    child.wait()
    record = project / ".agentctl" / AGENT
    record.mkdir()
    (record / "agent.json").write_text(
        json.dumps({"name": AGENT, "custom_process_identity": exited}), encoding="utf-8"
    )
    refused = _attested_remove(project)
    assert refused.returncode == 3, refused.stderr
    assert "reason=active-record-not-stopped" in refused.stderr
    assert "--attest-agent-stopped hold-demo" not in refused.stderr
    _retained(project)


def test_a_missing_registry_is_never_attestable(tmp_path: Path) -> None:
    project = _project(tmp_path)
    (project / ".agentctl").rmdir()
    refused = _attested_remove(project)
    assert refused.returncode == 3, refused.stderr
    assert "reason=no-agentctl-registry" in refused.stderr
    _retained(project)


@pytest.mark.parametrize("owner", ["live", "unrecorded", "dead-unexpired", "other-host"])
def test_attested_stop_needs_a_proven_dead_owner_and_an_expired_heartbeat(
    tmp_path: Path, owner: str
) -> None:
    project = _project(tmp_path, owner=owner)
    refused = _attested_remove(project)
    assert refused.returncode == 3, refused.stderr
    if owner == "dead-unexpired":
        assert "time-to-live has not expired" in refused.stderr
    else:
        # A live, unrecorded or indeterminate owner is not proven dead, so the
        # no-record answer keeps its ordinary meaning and nothing is attested.
        assert "registered liveness authority is unverifiable" in refused.stderr
    _retained(project)


def test_attested_stop_still_refuses_a_process_using_the_slot(tmp_path: Path) -> None:
    project = _project(tmp_path)
    user = subprocess.Popen(["sleep", "60"], cwd=t.checkout(project))
    try:
        refused = _attested_remove(project)
        assert refused.returncode == 3, refused.stderr
        assert str(user.pid) in refused.stderr
        _retained(project)
    finally:
        user.kill()
        user.wait()


def test_attested_stop_still_requires_the_handoff_read(tmp_path: Path) -> None:
    project = _project(tmp_path)
    (t.checkout(project).parent / "HANDOFF.md").write_text("notes\n", encoding="utf-8")
    refused = _attested_remove(project)
    assert refused.returncode == 3
    assert "read-handoff" in refused.stderr
    _retained(project)


def test_attested_stop_refuses_a_held_slot(tmp_path: Path) -> None:
    project = _project(tmp_path)
    held = t.command(project, "hold", "slot01", "--reason", "keep")
    assert held.returncode == 0, held.stderr
    refused = _attested_remove(project)
    assert refused.returncode == 3, refused.stderr
    _retained(project)


def test_batches_never_attest(tmp_path: Path) -> None:
    project = _project(tmp_path)
    pending = t.command(
        project, "retire-pending", "--limit", "1", "--coordinator-pid", str(os.getpid()),
        "--attest-agent-stopped", AGENT,
    )
    assert pending.returncode == 2
    assert "unrecognized arguments: --attest-agent-stopped" in pending.stderr
    batch = t.command(
        project, "remove-agent-batch", "--slot", "slot01=1", "--coordinator-pid", str(os.getpid()),
        "--attest-agent-stopped", AGENT,
    )
    assert batch.returncode == 2
    assert "unrecognized arguments: --attest-agent-stopped" in batch.stderr
    _retained(project)


def test_attestation_is_not_recorded_when_the_authority_reports_dead(tmp_path: Path) -> None:
    project = _project(tmp_path)
    t.write_liveness_output(
        project, b"dead agent=hold-demo rc=0 reason=stopped-by-agentctl\n", b"", 0
    )
    removed = _attested_remove(project)
    assert removed.returncode == 0, removed.stderr
    assert "was not needed" in removed.stderr
    assert _attested_events(project) == []


def _drifted(tmp_path: Path) -> tuple[Path, Path]:
    project = _project(tmp_path)
    tree = t.checkout(project)
    t.git(tree, "checkout", "-q", "-b", "codex/other")
    t.commit_local(tree, "drift.txt", "drift work\n", "unpushed drift")
    return project, tree


def test_attested_stop_never_archives_recorded_checkout_drift(tmp_path: Path) -> None:
    project, tree = _drifted(tmp_path)

    refused = _attested_remove(project, "--archive-drifted-checkouts")

    assert refused.returncode == 3, refused.stderr
    assert "--archive-drifted-checkouts archives drift only when" in refused.stderr
    assert "attestation that an unregistered agent stopped is not the authority's" in (
        refused.stderr
    )
    assert tree.is_dir()
    assert len(t.active_slots(project)) == 1


def test_recovery_never_resumes_archived_drift_on_an_attestation(tmp_path: Path) -> None:
    """A drift removal interrupted on the authority's 'dead' does not resume on a later attestation."""

    project, tree = _drifted(tmp_path)
    # An earlier attested attempt recorded its attestation, then refused at salvage.
    first = _attested_remove(project)
    assert first.returncode == 3, first.stderr
    assert len(_attested_events(project)) == 1
    # The authority then answered dead, and a drift-archiving removal was interrupted.
    t.write_liveness_output(
        project, b"dead agent=hold-demo rc=0 reason=stopped-by-agentctl\n", b"", 0
    )
    interrupted = t.command(
        project, "remove", "slot01", "--coordinator-pid", str(os.getpid()),
        "--expected-generation", "1", "--archive-drifted-checkouts",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-finish-journal"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    # Now the authority has no record of the agent again.
    shutil.copy(PROBE, project / "liveness.py")

    refused = t.command(project, "recover", "--coordinator-pid", str(os.getpid()))

    assert refused.returncode == 3, refused.stderr
    assert "a coordinator's attestation does not archive drift" in refused.stderr
    assert tree.is_dir()
    assert len(t.active_slots(project)) == 1

    t.write_liveness_output(
        project, b"dead agent=hold-demo rc=0 reason=stopped-by-agentctl\n", b"", 0
    )
    recovered = t.command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert recovered.returncode == 0, recovered.stderr
    assert t.active_slots(project) == []


def test_interrupted_attested_removal_recovers_only_while_the_agent_stays_unregistered(
    tmp_path: Path,
) -> None:
    project = _project(tmp_path)
    interrupted = _attested_remove(
        project, env={"WRKSLOTS_TEST_INTERRUPT": "after-remove-worktree"}
    )
    assert interrupted.returncode == 86, interrupted.stderr
    assert len(t.active_slots(project)) == 1

    # A record under the name appears (a new, live registration): recovery refuses.
    record = project / ".agentctl" / AGENT
    record.mkdir()
    (record / "agent.json").write_text(
        json.dumps({"name": AGENT, "custom_process_identity": _identity(os.getpid())}),
        encoding="utf-8",
    )
    refused = t.command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert refused.returncode == 3, refused.stderr
    assert "reports owner alive" in refused.stderr
    assert len(t.active_slots(project)) == 1

    shutil.rmtree(record)
    recovered = t.command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert recovered.returncode == 0, recovered.stderr
    assert t.active_slots(project) == []


def test_recovery_without_a_recorded_attestation_keeps_the_ordinary_refusal(
    tmp_path: Path,
) -> None:
    """An interrupted removal that never attested does not resume on a no-record answer."""

    project = _project(tmp_path)
    t.write_liveness_output(
        project, b"dead agent=hold-demo rc=0 reason=stopped-by-agentctl\n", b"", 0
    )
    interrupted = t.command(
        project, "remove", "slot01", "--coordinator-pid", str(os.getpid()),
        "--expected-generation", "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-remove-worktree"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    shutil.copy(PROBE, project / "liveness.py")

    refused = t.command(project, "recover", "--coordinator-pid", str(os.getpid()))

    assert refused.returncode == 3, refused.stderr
    assert "registered liveness authority is unverifiable" in refused.stderr
    assert len(t.active_slots(project)) == 1


@pytest.mark.parametrize(
    ("stdout", "stderr", "expected"),
    [
        (b"unverifiable agent=a rc=2 reason=no-agentctl-record\n", b"", True),
        (b"", b"unverifiable agent=a rc=2 reason=no-agentctl-record\n", True),
        (b"unverifiable agent=a rc=2 reason=no-agentctl-record\n", b"x\n", False),
        (b"unverifiable agent=a agent=a rc=2 reason=no-agentctl-record\n", b"", False),
        (b"unverifiable agent=b rc=2 reason=no-agentctl-record\n", b"", False),
        (b"unverifiable agent=a rc=1 reason=no-agentctl-record\n", b"", False),
        (b"unverifiable agent=a rc=2 reason=no-agentctl-records\n", b"", False),
        (b"unverifiable agent=a rc=2 reason=no-agentctl-registry\n", b"", False),
        (b"unverifiable agent=a rc=2\n", b"", False),
    ],
)
def test_bound_unregistered_verdict(stdout: bytes, stderr: bytes, expected: bool) -> None:
    verdict = w._bound_unregistered_verdict(stdout.decode(), stderr.decode(), "a")
    assert (verdict is not None) is expected
