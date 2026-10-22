"""#341 wrkslots-orphan-retire: removing a dead-owner slot whose agent no authority records.

A slot made for an in-process subagent, or held under a placeholder name, names an agent the
registered liveness command has never recorded. Once its owner process has died nothing can ever
report that agent dead, so plain ``remove`` refuses forever. ``remove`` accepts the coordinator's
explicit, recorded attestation in place of the command's "dead" only for the command's exact
"no record of this agent" answer and a proven-dead owner generation; every other answer and every
other guard keeps its meaning. These tests use the real agentctl probe over an empty registry
unless a case replaces it with exact output.

They run in the lifecycle suite's isolated shards: test_lifecycle.py collects the names in
COLLECTED. This module is deliberately not a test file of its own.
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


def _attested_refusal(
    project: Path, *extra: str, agent: str = AGENT
) -> subprocess.CompletedProcess[str]:
    """An attested remove expected to refuse before its process-use census.

    --no-lock-budget moves the lsof scan under the registry lock, where it runs
    only once the census is reached; a request refused earlier then skips a
    scan that costs seconds on a busy host. The decision under test is the same.
    """

    return _attested_remove(project, "--no-lock-budget", *extra, agent=agent)


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


def test_refused_and_malformed_requests_change_nothing(tmp_path: Path) -> None:
    """Plain remove names the rerun; every incomplete or wrong request refuses; nothing changes.

    One project serves every case: each refusal must leave the slot, its record and the
    event log exactly as they were, which is itself what these cases assert.
    """

    project = _project(tmp_path)
    before = len(_events(project))
    base = ("remove", "slot01", "--coordinator-pid", str(os.getpid()), "--expected-generation", "1")

    plain = t.command(project, *base, "--no-lock-budget")
    assert plain.returncode == 3, plain.stderr
    assert "has no record of agent hold-demo" in plain.stderr
    assert "reason=no-agentctl-record" in plain.stderr
    assert "--attest-agent-stopped hold-demo --attest-note TEXT" in plain.stderr
    assert "an agent registered elsewhere looks unregistered here" in plain.stderr

    unauthorized = t.raw_command(
        project, *base, "--attest-agent-stopped", AGENT, "--attest-note", NOTE
    )
    assert unauthorized.returncode == 3
    assert "requires --coordinator-authorized" in unauthorized.stderr

    for arguments in (
        ("--attest-agent-stopped", AGENT),
        ("--attest-note", NOTE),
        ("--attest-agent-stopped", AGENT, "--attest-note", " "),
        ("--attest-agent-stopped", AGENT, "--attest-note", "a\nb"),
        ("--attest-agent-stopped", AGENT, "--attest-note", "x" * 1025),
        ("--attest-agent-stopped", "Bad Name", "--attest-note", NOTE),
    ):
        refused = t.command(project, *base, *arguments)
        assert refused.returncode == 3, (arguments, refused.stderr)

    other = _attested_refusal(project, agent="someone-else")
    assert other.returncode == 3
    assert "recorded for agent hold-demo" in other.stderr

    for batch in (
        ("retire-pending", "--limit", "1"),
        ("remove-agent-batch", "--slot", "slot01=1"),
    ):
        rejected = t.command(
            project, *batch, "--coordinator-pid", str(os.getpid()), "--attest-agent-stopped", AGENT
        )
        assert rejected.returncode == 2, (batch, rejected.stderr)
        assert "unrecognized arguments: --attest-agent-stopped" in rejected.stderr

    _retained(project)
    assert len(_events(project)) == before


#: Every answer but the bound no-record line: each keeps its ordinary meaning.
OTHER_ANSWERS: tuple[tuple[bytes, int], ...] = (
    (b"alive agent=hold-demo rc=1 reason=active-record-process-alive\n", 1),
    (b"unverifiable agent=hold-demo rc=2 reason=active-record-not-stopped\n", 2),
    (b"unverifiable agent=hold-demo rc=2 reason=rename-incomplete\n", 2),
    (b"unverifiable agent=hold-demo rc=2 reason=archived-record-pane-still-occupied\n", 2),
    (b"unverifiable agent=other rc=2 reason=no-agentctl-record\n", 2),
    (b"unverifiable agent=hold-demo rc=2 reason=no-agentctl-record\nmore\n", 2),
    (b"unverifiable agent=hold-demo rc=2 reason=no-agentctl-record reason=x\n", 2),
    (b"unverifiable agent=hold-demo rc=3 reason=no-agentctl-record\n", 3),
    (b"unverifiable agent=hold-demo rc=2 reason=no-agentctl-registry\n", 2),
)


def test_attested_stop_refuses_every_other_liveness_answer(tmp_path: Path) -> None:
    """Exact outputs, then the real probe over a registered agent and over a missing registry.

    The registered case is #341's acceptance (2): an agent that was registered and might
    still live is not "unregistered", whatever the coordinator attests.
    """

    project = _project(tmp_path)
    for stdout, status in OTHER_ANSWERS:
        t.write_liveness_output(project, stdout, b"", status)
        refused = _attested_refusal(project)
        assert refused.returncode == 3, (stdout, refused.stderr)
        assert "--attest-agent-stopped hold-demo" not in refused.stderr, stdout
        _retained(project)

    shutil.copy(PROBE, project / "liveness.py")
    child = subprocess.Popen(["true"])
    exited = _identity(child.pid)
    child.wait()
    record = project / ".agentctl" / AGENT
    record.mkdir()
    (record / "agent.json").write_text(
        json.dumps({"name": AGENT, "custom_process_identity": exited}), encoding="utf-8"
    )
    registered = _attested_refusal(project)
    assert registered.returncode == 3, registered.stderr
    assert "reason=active-record-not-stopped" in registered.stderr
    assert "--attest-agent-stopped hold-demo" not in registered.stderr
    _retained(project)

    shutil.rmtree(project / ".agentctl")
    missing = _attested_refusal(project)
    assert missing.returncode == 3, missing.stderr
    assert "reason=no-agentctl-registry" in missing.stderr
    _retained(project)


@pytest.mark.parametrize("owner", ["live", "unrecorded", "dead-unexpired", "other-host"])
def test_attested_stop_needs_a_proven_dead_owner_and_an_expired_heartbeat(
    tmp_path: Path, owner: str
) -> None:
    project = _project(tmp_path, owner=owner)
    refused = _attested_refusal(project)
    assert refused.returncode == 3, refused.stderr
    if owner == "dead-unexpired":
        assert "time-to-live has not expired" in refused.stderr
    else:
        # A live, unrecorded or indeterminate owner is not proven dead, so the
        # no-record answer keeps its ordinary meaning and nothing is attested.
        assert "registered liveness authority is unverifiable" in refused.stderr
    _retained(project)


def test_attested_stop_keeps_every_guard_that_protects_work(tmp_path: Path) -> None:
    """A process in the slot, an unread handoff and a hold each still refuse."""

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

    handoff = t.checkout(project).parent / "HANDOFF.md"
    handoff.write_text("notes\n", encoding="utf-8")
    refused = _attested_remove(project)
    assert refused.returncode == 3
    assert "read-handoff" in refused.stderr
    _retained(project)
    handoff.unlink()

    held = t.command(project, "hold", "slot01", "--reason", "keep")
    assert held.returncode == 0, held.stderr
    refused = _attested_remove(project)
    assert refused.returncode == 3, refused.stderr
    assert "hold" in refused.stderr.lower()
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


# --------------------------------------------------- agent names, and slots never started in

#: A name recorded before create refused unusable ones: capitals the probe can never judge.
LEGACY_AGENT = "Legacy-Name"
NEVER_STARTED_NOTE = "slot created under an unusable name; no agent was ever started in it"


def _plant_legacy_agent(project: Path, agent: str = LEGACY_AGENT) -> None:
    """Rewrite slot01's recorded agent to a legacy unusable name, owner untouched.

    Simulates a slot created before create refused such names: the rewrite is an ordinary
    active-state write, exactly what an old create would have left behind.
    """

    config = w._load_config(project, "testhost")
    state = w._load_active(config, require_repository=False)
    record = w._find_record(state, "slot01")
    state = w._replace_record(state, dataclasses.replace(record, agent=agent))
    w._write_active_state(
        config,
        state,
        action="test-legacy-agent-recorded",
        slot="slot01",
        evidence={"planted_legacy_agent": agent},
        require_repository=False,
    )


def _never_started_remove(
    project: Path,
    *,
    agent: str = LEGACY_AGENT,
    note: str = NEVER_STARTED_NOTE,
) -> subprocess.CompletedProcess[str]:
    return t.command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--no-lock-budget",
        "--attest-agent-never-started",
        agent,
        "--attest-note",
        note,
    )


def _never_started_events(project: Path) -> list[Mapping[str, object]]:
    return [
        event
        for event in _events(project)
        if event["kind"] == "active-state-recorded"
        and w._as_mapping(event["payload"], "payload")["action"] == w.NEVER_STARTED_ACTION
    ]


def test_create_adopt_and_transfer_refuse_agent_names_the_tooling_cannot_use(
    tmp_path: Path,
) -> None:
    """The same rule the session tooling applies, at every entry point that records a name."""

    project, _repository, _remote = t.make_project(tmp_path)
    for bad in ("Legacy-Name", "1agent", "agent_name", "agent.name", "a" * 33, "archive"):
        refused = t.create(project, agent=bad, bind_owner=False)
        assert refused.returncode == 3, (bad, refused.stderr)
        assert "agent names must start with a lowercase letter" in refused.stderr

    created = t.create(project, bind_owner=False)
    assert created.returncode == 0, created.stderr
    refused = t.command(
        project,
        "adopt",
        "slot01",
        "--agent",
        "Bad-Name",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert refused.returncode == 3, refused.stderr
    assert "agent names must start with a lowercase letter" in refused.stderr

    adopted = t.command(
        project,
        "adopt",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert adopted.returncode == 0, adopted.stderr
    t.mark_owner_dead(project)
    refused = t.command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "Bad-Name",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert refused.returncode == 3, refused.stderr
    assert "agent names must start with a lowercase letter" in refused.stderr
    # A usable name still transfers: the refusal above was the name, not the operation.
    moved = t.command(
        project,
        "transfer-owner",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )
    assert moved.returncode == 0, moved.stderr


def test_never_started_slot_under_an_unjudgeable_name_removes_on_attestation(
    tmp_path: Path,
) -> None:
    """The planted fixture of the real orphan: unusable name, never adopted, expired TTL."""

    project, repository, _remote = t.make_project(tmp_path)
    created = t.create(project, bind_owner=False)
    assert created.returncode == 0, created.stderr
    checkout = t.checkout(project)
    shutil.copy(PROBE, project / "liveness.py")
    (project / ".agentctl").mkdir()
    t.expire_heartbeat(project)
    _plant_legacy_agent(project)

    plain = t.command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--no-lock-budget",
    )
    assert plain.returncode == 3, plain.stderr
    assert "is one the session tooling cannot use" in plain.stderr
    assert f"--attest-agent-never-started {LEGACY_AGENT}" in plain.stderr
    _retained(project)

    removed = _never_started_remove(project)
    assert removed.returncode == 0, removed.stderr
    assert t.active_slots(project) == []
    assert not checkout.exists()

    events = _events(project)
    [attested] = _never_started_events(project)
    evidence = _evidence(attested)
    assert evidence["basis"] == w.NEVER_STARTED_BASIS
    assert evidence["agent"] == LEGACY_AGENT
    assert evidence["owner"] is None
    assert evidence["owner_state"] == "absent"
    assert evidence["coordinator_authorized"] is True
    assert evidence["note"] == NEVER_STARTED_NOTE
    assert evidence["registered_liveness"] == "unverifiable"
    assert str(evidence["registered_liveness_detail"]) != ""
    started = next(event for event in events if event["kind"] == "reclaim-started")
    assert events.index(started) > events.index(attested)
    reclaim = next(
        event
        for event in events
        if event["kind"] == "active-state-recorded"
        and w._as_mapping(event["payload"], "p")["action"] == "reclaim-evidence-recorded"
    )
    assert _evidence(reclaim)["basis"] == w.NEVER_STARTED_BASIS
    assert _evidence(reclaim)["attestation_note"] == NEVER_STARTED_NOTE
    del repository


def test_never_started_attestation_refuses_when_an_owner_was_recorded(
    tmp_path: Path,
) -> None:
    """An adopted slot later recorded under a legacy name keeps needing its owner proven."""

    project, _repository, _remote = t.make_project(tmp_path)
    assert t.create(project).returncode == 0
    shutil.copy(PROBE, project / "liveness.py")
    (project / ".agentctl").mkdir()
    t.mark_owner_dead(project)
    t.expire_heartbeat(project)
    _plant_legacy_agent(project)
    refused = _never_started_remove(project)
    assert refused.returncode == 3, refused.stderr
    assert "no owner was ever recorded" in refused.stderr
    _retained(project)
    assert _never_started_events(project) == []


def test_never_started_attestation_refuses_a_judgeable_name(tmp_path: Path) -> None:
    """A usable name keeps its ordinary paths; the attestation is for unjudgeable ones only."""

    project, _repository, _remote = t.make_project(tmp_path)
    assert t.create(project, bind_owner=False).returncode == 0
    shutil.copy(PROBE, project / "liveness.py")
    (project / ".agentctl").mkdir()
    t.expire_heartbeat(project)
    refused = _never_started_remove(project, agent="codex-1")
    assert refused.returncode == 3, refused.stderr
    assert "no owner was ever recorded" in refused.stderr
    _retained(project)


def test_never_started_attestation_refuses_alive_and_unexpired_and_bad_requests(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = t.make_project(tmp_path)
    assert t.create(project, bind_owner=False).returncode == 0
    shutil.copy(PROBE, project / "liveness.py")
    (project / ".agentctl").mkdir()
    _plant_legacy_agent(project)
    base = (
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--no-lock-budget",
    )

    unexpired = _never_started_remove(project)
    assert unexpired.returncode == 3, unexpired.stderr
    assert "time-to-live has not expired" in unexpired.stderr

    both = t.command(
        project,
        *base,
        "--attest-agent-stopped",
        LEGACY_AGENT,
        "--attest-agent-never-started",
        LEGACY_AGENT,
        "--attest-note",
        NEVER_STARTED_NOTE,
    )
    assert both.returncode == 3, both.stderr
    assert "cannot be supplied together" in both.stderr

    unauthorized = t.raw_command(
        project,
        *base,
        "--attest-agent-never-started",
        LEGACY_AGENT,
        "--attest-note",
        NEVER_STARTED_NOTE,
    )
    assert unauthorized.returncode == 3, unauthorized.stderr
    assert "requires --coordinator-authorized" in unauthorized.stderr

    other = _never_started_remove(project, agent="someone-else")
    assert other.returncode == 3, other.stderr
    assert f"recorded for agent {LEGACY_AGENT}" in other.stderr

    t.expire_heartbeat(project)
    t.write_liveness_output(
        project, f"alive agent={LEGACY_AGENT} rc=1\n".encode(), b"", 1
    )
    alive = _never_started_remove(project)
    assert alive.returncode == 3, alive.stderr
    _retained(project)
    assert _never_started_events(project) == []


#: What test_lifecycle.py collects from this module.
COLLECTED: tuple[str, ...] = tuple(name for name in sorted(globals()) if name.startswith("test_"))
