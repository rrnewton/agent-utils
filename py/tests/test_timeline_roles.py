"""``wrkviz.roles`` (library-only in v1): classify lane-sessions by role."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wrkviz.roles import (
    Classification,
    LaneSession,
    RolesError,
    Sources,
    ToolMix,
    classify_fleet,
    load_lanes,
    load_overrides,
    run,
    scan_tool_mix,
)

REG = "hostA"
ROOT = "/home/me/project"
SLOT = "/home/me/project/worktrees/slots/feat"

IMPLEMENT_BRIEF = (
    "[from lead via agentctl · reply: agentctl send lead]\n"
    "Implement the change for the widget in your slot. Commit only in your slot. "
    "Run make validate. End final message with DONE: or BLOCKED: line."
)
REVIEW_BRIEF = (
    "[from lead via agentctl · reply: agentctl send lead]\n"
    "Review PR #91 on branch feat-x. Read-only: do not edit. "
    "Give a verdict: approve or request changes, findings only. "
    "End final message with DONE: or BLOCKED: line."
)


def lane(name: str, **overrides: object) -> LaneSession:
    fields: dict[str, object] = {"registry": REG, "name": name, "token": f"tok-{name}"}
    fields.update(overrides)
    return LaneSession(**fields)  # type: ignore[arg-type]


def by_name(rows: list[Classification], name: str) -> Classification:
    return next(row for row in rows if row.name == name)


def lead_lane() -> LaneSession:
    return lane(
        "lead", cwd=ROOT, designated_coordinator=True,
        incoming=tuple(f"[from sender{i} via agentctl] status" for i in range(5)),
        tool_mix=ToolMix(lines=90, lane_management=30, lane_starts=5, read_search=10),
    )


def implementor_lane(name: str = "mu-feat", parent: str = "lead") -> LaneSession:
    return lane(
        name, cwd=SLOT, parent_name=parent, brief=IMPLEMENT_BRIEF,
        incoming=(IMPLEMENT_BRIEF,), last_reply="All done.\nDONE: shipped",
        tool_mix=ToolMix(lines=120, edit=20, commit=3, read_search=10, test_build=10),
        has_log=True,
    )


def reviewer_lane(name: str = "rev-pr91", parent: str = "lead") -> LaneSession:
    return lane(
        name, cwd=SLOT, parent_name=parent, brief=REVIEW_BRIEF,
        incoming=(REVIEW_BRIEF,), last_reply="Verdict: request changes.\nDONE: reviewed",
        tool_mix=ToolMix(lines=80, read_search=30, review_verdict=2, test_build=3),
        has_log=True,
    )


def test_coordinator_implementor_reviewer_discrimination() -> None:
    rows = classify_fleet([lead_lane(), implementor_lane(), reviewer_lane()])

    lead = by_name(rows, "lead")
    assert (lead.role, lead.coordinator_kind) == ("coordinator", "lead")
    assert lead.confidence_bp == 9_800  # designated, whatever its profile says

    implementor = by_name(rows, "mu-feat")
    assert implementor.role == "implementor"
    assert implementor.coordinator_kind is None
    assert implementor.confidence_bp >= 5_500
    assert {signal.kind for signal in implementor.signals} >= {
        "topology-slot", "brief-implement", "behaviour-edits", "behaviour-commits"}

    reviewer = by_name(rows, "rev-pr91")
    assert reviewer.role == "reviewer"
    assert reviewer.confidence_bp >= 5_500
    assert "implementor" in {role for role, _ in reviewer.alternatives}


def test_non_designated_coordinator_from_children_and_behaviour() -> None:
    boss = lane(
        "boss", cwd=ROOT,
        incoming=tuple(f"[from sender{i} via agentctl] report" for i in range(4)),
        tool_mix=ToolMix(lines=90, lane_management=24, lane_starts=3, read_search=8),
    )
    rows = classify_fleet([boss, implementor_lane("mu-a", "boss"),
                           implementor_lane("mu-b", "boss")])
    verdict = by_name(rows, "boss")
    assert (verdict.role, verdict.coordinator_kind) == ("coordinator", "lead")


def test_liaison_and_lieutenant_sub_labels() -> None:
    chat = tuple(
        [f"[from owner via slack] <chat_message>question {i}</chat_message>"
         for i in range(6)]
        + ["[from lead via agentctl] please post the summary"]
    )
    liaison = lane(
        "liaison", cwd=ROOT, parent_name="lead", profile="coordinator",
        in_restore=True, brief=chat[0], incoming=chat,
        tool_mix=ToolMix(lines=60, chat=20, read_search=2), has_log=True,
    )
    sub = lane(
        "subcoord", cwd=ROOT, parent_name="lead",
        tool_mix=ToolMix(lines=70, lane_management=20, lane_starts=3, read_search=5),
    )
    rows = classify_fleet([lead_lane(), liaison, sub,
                           implementor_lane("mu-under-sub", "subcoord")])

    assert (by_name(rows, "liaison").role,
            by_name(rows, "liaison").coordinator_kind) == ("coordinator", "liaison")
    assert (by_name(rows, "subcoord").role,
            by_name(rows, "subcoord").coordinator_kind) == ("coordinator", "lieutenant")
    assert by_name(rows, "lead").coordinator_kind == "lead"


def test_liaison_retired_when_lane_holds_working_reports() -> None:
    chatty = lane(
        "chatty", cwd=ROOT, parent_name="lead",
        brief="[from owner via slack] <chat_message>hi</chat_message>",
        incoming=("[from owner via slack] <chat_message>hi</chat_message>",) * 6,
        tool_mix=ToolMix(lines=60, chat=20, lane_management=12, lane_starts=2),
        has_log=True,
    )
    rows = classify_fleet([lead_lane(), chatty, implementor_lane("mu-kid", "chatty")])
    verdict = by_name(rows, "chatty")
    assert verdict.coordinator_kind != "liaison"
    assert any(signal.kind == "liaison-blocked" for signal in verdict.signals)


@pytest.mark.parametrize("name,profile", [
    ("rev-widget", None),
    ("mu-widget", None),
    ("coord-widget", None),
    ("plain-widget", "worker"),
    ("plain-widget-two", "coordinator"),
    ("musereview9", "muse-watermelon-max"),
])
def test_name_and_profile_alone_stay_unclassified(
    name: str, profile: str | None
) -> None:
    rows = classify_fleet([lead_lane(), lane(name, parent_name="lead", profile=profile)])
    verdict = by_name(rows, name)
    assert verdict.role == "unclassified"
    assert verdict.confidence_bp < 5_500
    assert verdict.alternatives  # the weak guess is still on the record


def test_name_traps_do_not_read_as_reviewer() -> None:
    for name in ("mu-revive", "muse-watermelon-preview"):
        verdict = by_name(classify_fleet([lane(name, parent_name="lead")]), name)
        assert verdict.role == "unclassified"
        assert not any(
            signal.kind == "name" and signal.label == "reviewer"
            for signal in verdict.signals
        )


def test_empty_and_degenerate_sessions() -> None:
    empty = lane("plain")
    failed = lane("never-ran", lifecycle="launch_failed")
    blank_log = lane("blank", has_log=True, tool_mix=ToolMix(lines=3))
    rows = classify_fleet([empty, failed, blank_log])
    for name in ("plain", "never-ran", "blank"):
        verdict = by_name(rows, name)
        assert verdict.role == "unclassified", name
        assert verdict.confidence_bp == 0, name
    assert any(signal.kind == "no-session"
               for signal in by_name(rows, "never-ran").signals)


def test_standing_worker_profile_lane_abstains() -> None:
    # A long-lived harness-root lane with a worker profile is none of the three jobs
    # on that evidence; the profile must not make it an implementor.
    hands = lane("hands", cwd=ROOT, parent_name="lead", profile="worker",
                 in_restore=True)
    verdict = by_name(classify_fleet([lead_lane(), hands]), "hands")
    assert verdict.role == "unclassified"


def test_read_only_recon_and_audit_are_not_reviewers() -> None:
    recon = lane(
        "mu-recon", cwd=SLOT, parent_name="lead",
        brief="[from lead via agentctl] Read-only recon: do not edit. Audit the widget "
              "subsystem and report findings. End final message with DONE: line.",
        incoming=("[from lead via agentctl] audit the widget subsystem",),
        tool_mix=ToolMix(lines=50, read_search=20), has_log=True,
    )
    verdict = by_name(classify_fleet([lead_lane(), recon]), "mu-recon")
    assert verdict.role == "unclassified"


def test_declared_overrides_win() -> None:
    hands = lane("hands", cwd=ROOT, parent_name="lead", profile="worker",
                 in_restore=True)
    remote = lane("remote-coord", cwd=ROOT)
    rows = classify_fleet(
        [lead_lane(), hands, remote],
        overrides={"hands": ("implementor", None),
                   "remote-coord": ("coordinator", "lieutenant")},
    )
    assert by_name(rows, "hands").role == "implementor"
    assert by_name(rows, "hands").confidence_bp == 10_000
    assert by_name(rows, "remote-coord").coordinator_kind == "lieutenant"


def write_json(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record), encoding="utf-8")


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


@pytest.fixture
def registry(tmp_path: Path) -> tuple[Path, Path]:
    """A designated coordinator, one slot implementor with a log, one failed launch."""

    root = tmp_path / "registry"
    write_json(root / "coordinator.json",
               {"name": "lead", "schema": "agentctl-coordinator/v1", "token": "tok-lead"})
    write_json(root / "restore.json",
               {"coordinator": "lead", "agents": ["liaison"]})
    write_json(root / "agents" / "agent.json", {"layout": 2})  # tripwire, not a record
    write_json(root / "agents" / "lead" / "agent.json", {
        "name": "lead", "token": "tok-lead", "harness": "claude", "profile": None,
        "cwd": str(tmp_path / "project"), "created_at": 100.0, "lifecycle": "running",
        "native_session": {"value": "sess-lead"},
    })
    slot = tmp_path / "project" / "worktrees" / "slots" / "feat"
    write_json(root / "agents" / "mu-feat" / "agent.json", {
        "name": "mu-feat", "token": "tok-feat", "harness": "claude",
        "profile": "muse-ultra", "cwd": str(slot), "created_at": 200.0,
        "lifecycle": "stopped", "native_session": {"value": "sess-feat"},
        "created_by": {"agent": "lead", "registry": str(root)},
    })
    write_json(root / "agents" / "mu-feat" / "queue" / "processed" / "1.json",
               {"text": IMPLEMENT_BRIEF, "queued_at": 200.0})
    write_json(root / "archive" / "never-ran-tok" / "agent.json", {
        "name": "never-ran", "token": "tok-never", "harness": "muse",
        "created_at": 300.0, "lifecycle": "launch_failed",
        "created_by": {"agent": "lead"},
    })
    projects = tmp_path / "projects"
    lines = (
        ['{"type":"assistant","message":{"content":"edit_file write_file"}}'] * 8
        + ['{"type":"assistant","message":{"content":"git commit -m x"}}'] * 2
        + ['{"type":"assistant","message":{"content":"read_file"}}'] * 4
        + ['{"type":"assistant","message":{"content":"make validate"}}'] * 4
    )
    write_text(projects / "-project" / "sess-feat.jsonl", "\n".join(lines) + "\n")
    return root, projects


def test_load_lanes_reads_registry(registry: tuple[Path, Path]) -> None:
    root, _ = registry
    lanes = load_lanes(root, "hostA")
    assert [lane.name for lane in lanes] == ["lead", "mu-feat", "never-ran"]
    lead, feat, _ = lanes
    assert lead.designated_coordinator and lead.profile is None
    assert feat.parent_name == "lead" and feat.in_slot
    assert feat.brief == IMPLEMENT_BRIEF


def test_scan_tool_mix_counts_shapes(registry: tuple[Path, Path]) -> None:
    _, projects = registry
    mix = scan_tool_mix(projects / "-project" / "sess-feat.jsonl")
    assert (mix.edit, mix.commit, mix.read_search, mix.test_build) == (8, 2, 4, 4)
    assert scan_tool_mix(projects / "missing.jsonl") == ToolMix()


def test_run_json_and_file(
    registry: tuple[Path, Path], tmp_path: Path
) -> None:
    root, projects = registry
    out = tmp_path / "roles.json"
    text = run(
        Sources(
            registries=[("hostA", root)],
            claude_projects=[("hostA", projects)],
        ),
        "json",
        out,
    )
    report = json.loads(text)
    assert report == json.loads(out.read_text(encoding="utf-8"))
    roles = {row["name"]: row for row in report["lanes"]}
    assert roles["lead"]["role"] == "coordinator"
    assert roles["lead"]["coordinator_kind"] == "lead"
    assert roles["mu-feat"]["role"] == "implementor"
    assert isinstance(roles["mu-feat"]["confidence_bp"], int)
    assert roles["never-ran"]["role"] == "unclassified"
    assert report["counts"] == {
        "coordinator/lead": 1, "implementor": 1, "unclassified": 1}


def test_run_missing_registry_errors(tmp_path: Path) -> None:
    with pytest.raises(RolesError, match="agent registry not found"):
        run(Sources(registries=[("hostA", tmp_path / "nope")]), "markdown", None)


def test_load_overrides_shapes_and_errors(tmp_path: Path) -> None:
    path = tmp_path / "overrides.json"
    write_json(path, {
        "plain": "reviewer",
        "hostA:sub": {"coordinator_kind": "lieutenant"},
        "sess-1": {"role": "coordinator", "coordinator_kind": "liaison"},
    })
    assert load_overrides(path) == {
        "plain": ("reviewer", None),
        "hostA:sub": ("coordinator", "lieutenant"),
        "sess-1": ("coordinator", "liaison"),
    }
    write_json(path, {"plain": "wizard"})
    with pytest.raises(RolesError, match="unknown role"):
        load_overrides(path)
    write_json(path, {"plain": {"role": "reviewer", "coordinator_kind": "liaison"}})
    with pytest.raises(RolesError, match="unknown coordinator kind"):
        load_overrides(path)
