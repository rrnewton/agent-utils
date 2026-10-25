"""`wrkviz responsiveness`: rebuild a coordinator/child loop from agentctl and harness logs."""

from __future__ import annotations

from datetime import datetime, timezone
import json
from pathlib import Path
import time

import pytest

from wrkviz.cli import main
from wrkviz.responsiveness import (
    ResponsivenessError,
    Sources,
    _command_targets,
    _name_pattern,
    _slot_targets,
    analyze,
    claude_turns,
    message_author,
    muse_session_cwd,
    summary,
    verdict,
)

T0 = datetime(2026, 1, 5, 12, 0, tzinfo=timezone.utc).timestamp()


def iso(minutes: float) -> str:
    return datetime.fromtimestamp(T0 + minutes * 60, timezone.utc).isoformat().replace("+00:00", "Z")


def at(minutes: float) -> float:
    return T0 + minutes * 60


def write_jsonl(path: Path, records: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(record) + "\n" for record in records), encoding="utf-8")


def write_json(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record), encoding="utf-8")


def user(minutes: float, text: str) -> dict[str, object]:
    return {"type": "user", "timestamp": iso(minutes), "message": {"role": "user", "content": text}}


def assistant(minutes: float, text: str = "", command: str | None = None) -> dict[str, object]:
    content: list[dict[str, object]] = []
    if text:
        content.append({"type": "text", "text": text})
    if command is not None:
        content.append({"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": command}})
    return {"type": "assistant", "timestamp": iso(minutes), "message": {"role": "assistant", "content": content}}


def turn_end(minutes: float) -> dict[str, object]:
    return {"type": "system", "subtype": "turn_duration", "timestamp": iso(minutes)}


def sweep(minutes: float) -> dict[str, object]:
    return {"type": "system", "subtype": "scheduled_task_fire", "timestamp": iso(minutes),
            "prompt": "[sweep] read every agent"}


def muse_run(minutes: float, event: dict[str, object]) -> dict[str, object]:
    return {"recorded_at": int(at(minutes) * 1e6), "payload": {"kind": "run", "event": event}}


@pytest.fixture
def team(tmp_path: Path) -> Sources:
    """A coordinator, one Claude child and one Muse child, with every notification path."""

    registry = tmp_path / "registry"
    write_json(registry / "coordinator" / "agent.json", {"name": "coordinator", "harness": "claude"})
    # The Claude child: stopped and archived.
    write_json(registry / "archive" / "cl-a-1" / "agent.json", {
        "name": "cl-a", "harness": "claude", "created_at": at(0), "cwd": "/w/slot-a", "slot": "slot-a",
        "native_session": {"value": "sess-a"},
    })
    # The Muse child: still registered.
    write_json(registry / "mu-b" / "agent.json", {
        "name": "mu-b", "harness": "muse", "created_at": at(0), "cwd": "/w/slot-b", "slot": "slot-b",
    })
    # cl-a tells the coordinator it is waiting (minute 9.5, typed in at 10.5).
    write_json(registry / "coordinator" / "queue" / "processed" / "1.json", {
        "id": "01-1", "text": "cl-a: gate green, may I push?", "queued_at": at(9.5), "confirmed_at": at(10.5)})
    # watch-agents reports mu-b at minute 50.
    write_json(registry / "coordinator" / "queue" / "processed" / "2.json", {
        "id": "agent-watch-abc", "text": "[agentctl watch-agents] 1 agent(s) need attention:\n- mu-b: idle",
        "queued_at": at(50), "confirmed_at": at(50.5)})
    # The coordinator answers cl-a through cl-a's own queue at minute 12.
    write_json(registry / "archive" / "cl-a-1" / "queue" / "processed" / "3.json", {
        "id": "01-3", "text": "[coordinator] push", "queued_at": at(12), "confirmed_at": at(12.2)})

    coordinator = tmp_path / "coord.jsonl"
    write_jsonl(coordinator, [
        user(0, "start the team"), assistant(1, command="agentctl start cl-a"), turn_end(2),
        user(10.5, "cl-a: gate green, may I push?"),
        assistant(11, command="agentctl read cl-a --lines 30"),
        assistant(12, command="agentctl send cl-a --file brief"), turn_end(12.5),
        sweep(40), assistant(40.2, command="agentctl stop cl-a; agentctl read mu-b"), turn_end(41),
        user(50.5, "[agentctl watch-agents] 1 agent(s) need attention:\n- mu-b: idle"),
        assistant(51, command="agentctl stop mu-b"), turn_end(52),
    ])
    projects = tmp_path / "projects"
    write_jsonl(projects / "-w-slot-a" / "sess-a.jsonl", [
        user(1, "brief"), assistant(9, "Gate green; may I push?"), turn_end(10),
        user(12.2, "[coordinator] push"), assistant(14, "Running the gate."), turn_end(15),
        user(30, "<task-notification>gate done</task-notification>"),
        assistant(31, "DONE: landed"), turn_end(32),
    ])
    sessions = tmp_path / "muse" / "sessions"
    write_jsonl(sessions / "2026" / "01" / "05" / "s1" / "session.jsonl", [
        {"recorded_at": int(at(0.1) * 1e6), "payload": {"kind": "route_facts", "record": {"cwd": "/w/slot-b/"}}},
        muse_run(1, {"kind": "started", "prompt": "brief"}),
        muse_run(20, {"kind": "assistant_message_committed", "text": "DONE: shipped"}),
        muse_run(21, {"kind": "terminal"}),
    ])
    return Sources(registry=registry, coordinator_transcript=coordinator, claude_projects=projects,
                   muse_sessions=tmp_path / "muse" / "sessions", sweep_marker="[sweep]")


def test_episodes_follow_each_notification_path(team: Sources) -> None:
    report = analyze(team)
    rows = {(e.agent, round((e.turn_end - T0) / 60)): e for e in report.episodes}
    assert set(rows) == {("cl-a", 10), ("cl-a", 32), ("mu-b", 21)}

    asked = rows[("cl-a", 10)]
    assert asked.verdict == "ask"
    assert asked.path == "child-message"
    assert asked.first_notice == "child-message"
    assert asked.latency(asked.observed) == pytest.approx(60)
    assert asked.latency(asked.acted) == pytest.approx(120)
    assert asked.act_verb == "send"
    assert not asked.parent_busy

    done = rows[("cl-a", 32)]
    assert done.verdict == "DONE"
    assert done.path == "sweep"
    # The stop at minute 40.2 names cl-a; the read in the same command does not stop mu-b.
    assert done.act_verb == "stop"
    assert done.latency(done.acted) == pytest.approx(8.2 * 60)

    muse = rows[("mu-b", 21)]
    assert muse.harness == "muse"
    assert muse.verdict == "DONE"
    # The sweep's read at 40.2 is the first look; the watch notice only came later.
    assert muse.path == "sweep"
    assert muse.first_notice == "sweep"
    assert muse.latency(muse.acted) == pytest.approx(30 * 60)


def test_unconfirmed_message_counts_from_its_arrival_in_the_transcript(team: Sources) -> None:
    # agentctl quarantines a submission it cannot verify; the prompt still reached the coordinator.
    write_json(team.registry / "coordinator" / "queue" / "processed" / "1.json", {
        "id": "01-1", "text": "cl-a: gate green, may I push?", "queued_at": at(9.5)})
    asked = next(e for e in analyze(team).episodes if e.agent == "cl-a" and e.verdict == "ask")
    assert asked.path == "child-message"
    assert asked.first_notice == "child-message"


def test_gaps_split_idle_time_by_what_the_child_waited_for(team: Sources) -> None:
    report = analyze(team)
    kinds = {(g.agent, g.waiting_for): round((g.end - g.start) / 60, 1) for g in report.gaps}
    assert kinds == {
        ("cl-a", "parent"): 2.2,  # asked at 10, the answer typed in at 12.2
        ("cl-a", "self"): 15.0,  # waited on its own gate from 15 to 30
        ("cl-a", "stop"): 8.2,  # DONE at 32, stopped at 40.2
        ("mu-b", "stop"): 30.0,  # DONE at 21, stopped at 51
    }
    figures = summary(report)
    assert figures["child_idle_waiting_on_parent_fraction"] == pytest.approx(round(40.4 / 55.4, 3))
    coordinator = figures["coordinator"]
    assert isinstance(coordinator, dict)
    assert coordinator["inputs"] == {"child-message": 1, "other": 1, "sweep": 1, "watch": 1}


def test_cli_writes_json_summary_and_episode_rows(team: Sources, tmp_path: Path,
                                                   capsys: pytest.CaptureFixture[str]) -> None:
    episodes = tmp_path / "episodes.json"
    status = main([
        "responsiveness", "--registry", str(team.registry),
        "--coordinator-transcript", str(team.coordinator_transcript),
        "--claude-projects", str(team.claude_projects),
        "--muse-sessions", str(team.muse_sessions),
        "--sweep-marker", "[sweep]", "--exclude", "mu-*",
        "--format", "json", "--episodes", str(episodes),
    ])
    assert status == 0
    figures = json.loads(capsys.readouterr().out)
    assert figures["episodes"] == 2
    assert figures["act_latency_by_harness"].keys() == {"claude"}
    rows = json.loads(episodes.read_text(encoding="utf-8"))
    assert [row["path"] for row in rows] == ["child-message", "sweep"]


def test_cli_reports_missing_sources(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    status = main(["responsiveness", "--registry", str(tmp_path),
                   "--coordinator-transcript", str(tmp_path / "missing.jsonl")])
    assert status == 2
    assert "coordinator transcript not found" in capsys.readouterr().err


def test_until_must_carry_an_offset(team: Sources, capsys: pytest.CaptureFixture[str]) -> None:
    status = main(["responsiveness", "--registry", str(team.registry),
                   "--coordinator-transcript", str(team.coordinator_transcript),
                   "--until", "2026-01-05T13:00:00"])
    assert status == 2
    assert "UTC offset" in capsys.readouterr().err


def test_command_targets_bind_each_verb_to_its_own_name() -> None:
    pattern = _name_pattern(["cl-a", "cl-ab", "mu-c"])
    assert _command_targets("agentctl stop cl-a; agentctl read cl-ab --lines 3", pattern) == [
        ("stop", "cl-a"), ("read", "cl-ab")]
    assert _command_targets("for n in cl-a mu-c; do agentctl stop $n; done", pattern) == [
        ("stop", "cl-a"), ("stop", "mu-c")]
    assert _command_targets("agentctl --registry /r send cl-ab --file f", pattern) == [("send", "cl-ab")]


def test_message_author_prefers_the_longest_registered_name() -> None:
    names = ["cl-a", "cl-ab"]
    assert message_author("cl-ab: done", names) == "cl-ab"
    assert message_author("LANDED (cl-a): #1 thing", names) == "cl-a"
    assert message_author("cl-abc: not registered", names) is None


@pytest.mark.parametrize(("reply", "expected"), [
    ("work\nDONE: landed", "DONE"),
    ("BLOCKED: need a decision", "BLOCKED"),
    ("The gate is green. May I push?", "ask"),
    ("Holding for your all-clear.", "await"),
    ("The gate is running.", "progress"),
])
def test_verdict(reply: str, expected: str) -> None:
    assert verdict(reply) == expected


def test_muse_session_without_route_facts_is_skipped(tmp_path: Path) -> None:
    log = tmp_path / "session.jsonl"
    write_jsonl(log, [muse_run(1, {"kind": "started", "prompt": "x"})])
    assert muse_session_cwd(log) is None


def test_transcript_without_turns_is_refused(tmp_path: Path) -> None:
    transcript = tmp_path / "coord.jsonl"
    write_jsonl(transcript, [user(0, "hello")])
    (tmp_path / "registry").mkdir()
    with pytest.raises(ResponsivenessError, match="no completed turn"):
        analyze(Sources(registry=tmp_path / "registry", coordinator_transcript=transcript))


def test_message_text_and_heredocs_are_not_actions() -> None:
    pattern = _name_pattern(["cl-a", "mu-b"])
    brief = "agentctl send mu-b --file - <<'EOF'\nI ran agentctl stop cl-a already\nEOF"
    assert _command_targets(brief, pattern) == [("send", "mu-b")]
    assert _command_targets("agentctl send cl-a 'mu-b already landed'", pattern) == [("send", "cl-a")]
    assert _command_targets("for n in cl-a\nmu-b; do agentctl stop $n; done", pattern) == [
        ("stop", "cl-a"), ("stop", "mu-b")]
    assert _command_targets("git worktree remove cl-a; echo agentctl", pattern) == []


def test_slot_actions_need_wrkslots() -> None:
    assert _slot_targets("/x/wrkslots read-handoff slot-a --coordinator-pid 1") == ["slot-a"]
    assert _slot_targets("env -C /h /x/wrkslots remove --force slot-b") == ["slot-b"]
    assert _slot_targets("git worktree remove slot-a") == []


def test_long_commands_scan_in_bounded_time() -> None:
    pattern = _name_pattern(["cl-a"])
    started = time.monotonic()
    _command_targets("agentctl --a " * 5000, pattern)
    _command_targets("agentctl" * 6000, pattern)
    assert time.monotonic() - started < 2.0


def test_watch_notice_reaches_every_listed_agent(team: Sources) -> None:
    write_json(team.registry / "coordinator" / "queue" / "processed" / "2.json", {
        "id": "agent-watch-abc",
        "text": "[agentctl watch-agents] 2 agent(s) need attention:\n- cl-z: idle\n- mu-b: idle",
        "queued_at": at(25), "confirmed_at": at(25.5)})
    muse = next(e for e in analyze(team).episodes if e.agent == "mu-b")
    assert muse.first_notice == "watch-notice"
    assert muse.path == "sweep"  # the sweep at 40 is the notice nearest before the first look


def test_pre_queued_needs_a_message_already_waiting(team: Sources) -> None:
    # Without the queued message the child's quick resume is not credited to the parent.
    (team.registry / "archive" / "cl-a-1" / "queue" / "processed" / "3.json").unlink()
    transcript = team.coordinator_transcript
    records = [json.loads(line) for line in transcript.read_text(encoding="utf-8").splitlines()]
    kept = [r for r in records if "agentctl send cl-a" not in json.dumps(r)]
    write_jsonl(transcript, kept)
    asked = next(e for e in analyze(team).episodes if e.agent == "cl-a" and e.verdict == "ask")
    assert asked.path != "pre-queued"
    write_json(team.registry / "archive" / "cl-a-1" / "queue" / "processed" / "3.json", {
        "id": "01-3", "text": "[coordinator] push", "queued_at": at(9.9), "confirmed_at": at(12.2)})
    # The read at minute 11 is the first look, so the episode is not pre-queued either way;
    # drop it to see the rule itself.
    write_jsonl(transcript, [r for r in kept if "agentctl read cl-a" not in json.dumps(r)])
    asked = next(e for e in analyze(team).episodes if e.agent == "cl-a" and e.verdict == "ask")
    assert asked.path == "pre-queued"


def test_killed_turn_ends_at_its_last_record(tmp_path: Path) -> None:
    log = tmp_path / "child.jsonl"
    write_jsonl(log, [
        user(0, "brief"), assistant(5, "working"),  # killed: no turn_duration
        user(30, "[coordinator] you were restarted"), assistant(31, "DONE: ok"), turn_end(32),
    ])
    turns = claude_turns(log)
    assert [(round((t.start - T0) / 60), round(((t.end or 0) - T0) / 60), t.trigger) for t in turns] == [
        (0, 5, "first"), (30, 32, "parent")]


def test_one_muse_log_per_child(team: Sources) -> None:
    # A second record in the same slot, registered 23 minutes later, must not take mu-b's log.
    write_json(team.registry / "archive" / "mu-b-2" / "agent.json", {
        "name": "mu-b2", "harness": "muse", "created_at": at(23), "cwd": "/w/slot-b", "slot": "slot-b"})
    report = analyze(team)
    logs = {agent.name: agent.log for agent in report.agents}
    assert logs["mu-b"] is not None and logs["mu-b2"] is None


def test_until_bounds_episodes_and_gaps(team: Sources) -> None:
    report = analyze(team, until=at(15))
    assert [(e.agent, round((e.turn_end - T0) / 60)) for e in report.episodes] == [("cl-a", 10)]
    assert report.gaps and all(gap.end <= at(15) for gap in report.gaps)
