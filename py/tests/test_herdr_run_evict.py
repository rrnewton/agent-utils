"""Least-recently-used replacement of idle tabs at the pane cap.

The pure decisions -- order, idle verdict, run-time rendering, stat parsing, refusal text -- are
pinned by ``rs/herdr-run/testdata/eviction_cases.json``, which the Rust unit tests load too, so the
two editions cannot drift apart silently. The behavioural tests below drive :func:`evict_one`
against the in-memory Herdr fake and a fake ``/proc`` written into the test's temporary directory.
"""

from __future__ import annotations

import dataclasses
import fcntl
import json
import os
from pathlib import Path
from typing import cast

import pytest

import herdr_run.state as state
from herdr_run import audit
from herdr_run.client import HerdrClient, Pane, ProcessInfo
from herdr_run.config import Config
from herdr_run.errors import HerdrUnavailable
from herdr_run.evict import (
    Candidate,
    Eviction,
    SessionScan,
    TtlCandidate,
    describe_skipped,
    evict_one,
    judge_idle,
    lru_candidates,
    parse_session_id,
    retire_idle_tabs,
    run_time,
    scan_session,
    survey,
    survey_workspace,
    ttl_candidates,
)
from herdr_run.readiness import assess_process
from herdr_run.session import _enforce_pane_cap
from herdr_run.sweep import load_run_records
from tests.herdr_fake import FakeHerdrClient, FakeTab, FakeWorkspace

_CASES_PATH = Path(__file__).resolve().parents[2] / "rs" / "herdr-run" / "testdata" / "eviction_cases.json"
CASES = json.loads(_CASES_PATH.read_text(encoding="utf-8"))


@pytest.fixture(autouse=True)
def _isolated_account_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(state, "_account_home", lambda: str(tmp_path / "account-home"))


# --- the shared golden cases -------------------------------------------------------------------


@pytest.mark.parametrize("case", CASES["order"], ids=lambda case: case["name"])
def test_golden_lru_order(case: dict[str, object]) -> None:
    panes = [Pane(pane_id=p, tab_id=t, workspace_id="w1") for p, t in case["panes"]]  # type: ignore[attr-defined]
    got = lru_candidates(panes, case["records"])  # type: ignore[arg-type]
    expected = [Candidate(**item) for item in case["expected"]]  # type: ignore[attr-defined]
    assert got == expected


@pytest.mark.parametrize("case", CASES["idle"], ids=lambda case: case["name"])
def test_golden_idle_verdicts(case: dict[str, dict[str, object]]) -> None:
    process = case["process"]
    info = ProcessInfo(
        pane_id="p1",
        shell_pid=cast(int, process["shell_pid"]),
        foreground_pgid=cast(int, process["foreground_pgid"]),
        foreground=tuple(tuple(entry) for entry in cast(list[list[object]], process["foreground"])),  # type: ignore[misc]
    )
    session = case["session"]
    others = session["others"]
    scan = SessionScan(
        shell_pid=info.shell_pid,
        shell_sid=cast("int | None", session["shell_sid"]),
        others=None if others is None else tuple(cast(list[int], others)),
    )
    idle, reason = judge_idle(assess_process(info, Config()), scan)
    assert {"idle": idle, "reason": reason} == case["expected"]


@pytest.mark.parametrize(("run_id", "expected"), CASES["run_time"])
def test_golden_run_time(run_id: str, expected: str | None) -> None:
    assert run_time(run_id) == expected


@pytest.mark.parametrize(("stat", "expected"), CASES["session_id"])
def test_golden_session_id(stat: str, expected: int | None) -> None:
    assert parse_session_id(stat) == expected


@pytest.mark.parametrize("case", CASES["refusal"], ids=lambda case: str(len(case["skipped"])))
def test_golden_refusal(case: dict[str, object]) -> None:
    skipped = [tuple(pair) for pair in cast(list[list[str]], case["skipped"])]
    assert describe_skipped(skipped) == case["expected"]  # type: ignore[arg-type]


@pytest.mark.parametrize("case", CASES["ttl"], ids=lambda case: case["name"])
def test_golden_idle_ttl_verdicts(case: dict[str, object]) -> None:
    panes = [Pane(pane_id=p, tab_id=t, workspace_id="w1") for p, t in case["panes"]]  # type: ignore[attr-defined]
    got = ttl_candidates(
        panes,
        case["records"],  # type: ignore[arg-type]
        now=cast(float, case["now"]),
        ttl_seconds=cast(float, case["ttl_seconds"]),
    )
    expected = [TtlCandidate(**item) for item in case["expected"]]  # type: ignore[attr-defined]
    assert got == expected


# --- a small world: a workspace, a run spool, and a fake /proc ----------------------------------


class World:
    """One workspace of single-pane tabs, a project spool, and a fake ``/proc``."""

    def __init__(self, tmp_path: Path, panes: list[str]) -> None:
        self.project = tmp_path / "project"
        self.project.mkdir()
        self.proc = tmp_path / "proc"
        self.proc.mkdir()
        self.config = Config(project_root=str(self.project))
        self.fake = FakeHerdrClient()
        tabs = {f"t-{pane}": FakeTab(f"t-{pane}", pane, "w1", [pane]) for pane in panes}
        self.fake.workspaces["w1"] = FakeWorkspace("w1", "commands", tabs)
        # Another workspace that must never be touched, even though its only tab is idle and old.
        self.fake.workspaces["w2"] = FakeWorkspace(
            "w2", "elsewhere", {"t-other": FakeTab("t-other", "other", "w2", ["other"])}
        )
        self._next_pid = 1000
        for pane in panes:
            self.busy(pane)

    def write_stat(self, pid: int, sid: int) -> None:
        directory = self.proc / str(pid)
        directory.mkdir(exist_ok=True)
        (directory / "stat").write_text(f"{pid} (bash) S 1 {pid} {sid} 34816 {pid} 4194560\n")

    def busy(self, pane: str) -> None:
        """The pane's shell is running a command in the foreground."""
        self.fake.busy_panes.add(pane)

    def idle(self, pane: str, *, background: int = 0) -> int:
        """Give the pane a shell that leads its own session, with ``background`` jobs in it."""
        self.fake.busy_panes.discard(pane)
        pid = self._next_pid
        self._next_pid += 10
        self.fake.shell_pids[pane] = pid
        self.write_stat(pid, pid)
        for offset in range(1, background + 1):
            self.write_stat(pid + offset, pid)
        return pid

    def record(self, pane: str, run_id: str, agent: str) -> None:
        directory = self.project / ".herdr-run" / "runs" / run_id
        directory.mkdir(parents=True)
        document = {"agent": agent, "pane_id": pane, "run_id": run_id, "exit_code": 0}
        (directory / "meta.json").write_text(json.dumps(document))

    def evict(self, agent: str = "newcomer") -> Eviction | list[tuple[str, str]]:
        return evict_one(cast(HerdrClient, self.fake), self.config, "w1", agent, str(self.proc))

    def audit_entries(self) -> list[dict[str, object]]:
        path = audit.audit_path(self.config.project_root, self.config.spool_dir)
        if not os.path.exists(path):
            return []
        with open(path, encoding="utf-8") as handle:
            return [json.loads(line) for line in handle]


def test_session_scan_lists_other_members_in_order_and_fails_closed(tmp_path: Path) -> None:
    world = World(tmp_path, [])
    world.write_stat(300, 300)
    world.write_stat(305, 300)
    world.write_stat(301, 300)
    world.write_stat(310, 310)
    assert scan_session(300, str(world.proc)) == SessionScan(300, 300, (301, 305))
    assert scan_session(310, str(world.proc)) == SessionScan(310, 310, ())
    # Not a leader: the membership is never listed, so the verdict cannot be "alone".
    assert scan_session(305, str(world.proc)) == SessionScan(305, 300, None)
    # No stat at all.
    assert scan_session(999, str(world.proc)) == SessionScan(999, None, None)


def test_the_least_recently_used_idle_tab_is_closed_and_logged(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    world = World(tmp_path, ["a", "b", "c"])
    for pane in ("a", "b", "c"):
        world.idle(pane)
    world.record("a", "20261001T120000-alpha-1", "alpha")
    world.record("b", "20261001T090000-bravo-2", "bravo")
    world.record("c", "20261001T100000-charlie-3", "charlie")

    eviction = world.evict()
    assert isinstance(eviction, Eviction)
    assert eviction.candidate.pane_id == "b"
    assert world.fake.closed_tabs == ["t-b"]
    stderr = capsys.readouterr().err
    assert (
        "herdr-run: replaced idle tab t-b (pane b, agent bravo, last run 2026-10-01T09:00:00Z) "
        "to make room for 'newcomer'"
    ) in stderr
    [entry] = world.audit_entries()
    assert entry["verdict"] == "EVICTED"
    assert entry["agent"] == "newcomer"
    assert entry["command"] == "tab close t-b"
    assert entry["pane_id"] == "b"
    assert entry["tab_id"] == "t-b"
    assert entry["workspace_id"] == "w1"
    assert entry["evicted_agent"] == "bravo"
    assert entry["last_run"] == "20261001T090000-bravo-2"
    assert entry["last_run_at"] == "2026-10-01T09:00:00Z"
    assert "no other process in session" in str(entry["detail"])

    # The next one goes in order, too.
    second = world.evict("second")
    assert isinstance(second, Eviction)
    assert world.fake.closed_tabs == ["t-b", "t-c"]
    # The other workspace is never even considered.
    assert "other" not in world.fake.probed


def test_panes_with_no_record_are_replaced_before_recorded_ones(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    world = World(tmp_path, ["old", "fresh"])
    world.idle("old")
    world.idle("fresh")
    world.record("old", "20200101T000000-ancient-1", "ancient")

    eviction = world.evict()
    assert isinstance(eviction, Eviction)
    assert eviction.candidate.pane_id == "fresh"
    assert eviction.candidate.last_run is None
    assert "agent unknown, last run none recorded" in capsys.readouterr().err
    [entry] = world.audit_entries()
    assert entry["evicted_agent"] is None
    assert entry["last_run_at"] is None


def test_a_tab_running_a_command_is_never_closed(tmp_path: Path) -> None:
    world = World(tmp_path, ["busy", "idle"])
    world.idle("idle")
    world.record("busy", "20200101T000000-oldest-1", "oldest")
    world.record("idle", "20261001T000000-newest-2", "newest")

    eviction = world.evict()
    assert isinstance(eviction, Eviction)
    assert eviction.candidate.pane_id == "idle"
    assert world.fake.closed_tabs == ["t-idle"]


def test_every_tab_busy_refuses_and_closes_nothing(tmp_path: Path) -> None:
    world = World(tmp_path, ["a", "b"])
    outcome = world.evict()
    assert outcome == [
        ("a", "foreground pgid 200 != shell pid 100; running: git"),
        ("b", "foreground pgid 200 != shell pid 100; running: git"),
    ]
    assert world.fake.closed_tabs == []
    assert world.audit_entries() == []


def test_a_background_job_in_the_shell_session_keeps_the_tab_open(tmp_path: Path) -> None:
    world = World(tmp_path, ["jobs"])
    pid = world.idle("jobs", background=2)
    outcome = world.evict()
    assert outcome == [
        ("jobs", f"session {pid} still holds 2 other process(es): {pid + 1}, {pid + 2}")
    ]
    assert world.fake.closed_tabs == []


def test_a_pane_reserved_by_another_herdr_run_is_skipped_without_probing(tmp_path: Path) -> None:
    world = World(tmp_path, ["held", "free"])
    world.idle("held")
    world.idle("free")
    world.record("free", "20261001T000000-later-1", "later")
    with state.open_lock_file(state.pane_lock_path("held")) as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        eviction = world.evict()
    assert isinstance(eviction, Eviction)
    assert eviction.candidate.pane_id == "free"
    assert world.fake.probed == ["free"]


def test_a_split_tab_is_never_closed(tmp_path: Path) -> None:
    world = World(tmp_path, ["solo"])
    world.fake.workspaces["w1"].tabs["t-split"] = FakeTab("t-split", "split", "w1", ["left", "right"])
    world.idle("left")
    world.idle("right")
    world.record("solo", "20261001T000000-solo-1", "solo")
    outcome = world.evict()
    assert outcome == [
        ("left", "tab t-split holds 2 panes; only an unsplit tab is replaced"),
        ("right", "tab t-split holds 2 panes; only an unsplit tab is replaced"),
        ("solo", "foreground pgid 200 != shell pid 100; running: git"),
    ]
    assert world.fake.closed_tabs == []


def test_a_failed_close_moves_on_to_the_next_idle_tab(tmp_path: Path) -> None:
    world = World(tmp_path, ["stuck", "next"])
    world.idle("stuck")
    world.idle("next")
    world.record("next", "20261001T000000-next-1", "next")
    world.fake.failing_close.add("t-stuck")
    eviction = world.evict()
    assert isinstance(eviction, Eviction)
    assert eviction.candidate.pane_id == "next"
    assert world.fake.closed_tabs == ["t-next"]


def test_the_survey_judges_every_tab_as_the_cap_does_and_closes_nothing(tmp_path: Path) -> None:
    world = World(tmp_path, ["busy", "newer", "jobs", "older"])
    world.fake.workspaces["w1"].tabs["t-split"] = FakeTab("t-split", "split", "w1", ["left", "right"])
    world.idle("newer")
    jobs = world.idle("jobs", background=1)
    world.idle("older")
    world.idle("left")
    world.idle("right")
    world.record("busy", "20261001T080000-a-1", "a")
    world.record("older", "20261001T090000-b-2", "b")
    world.record("newer", "20261001T100000-c-3", "c")
    panes = world.fake.panes("w1")
    records = load_run_records(world.config)
    found = survey(cast(HerdrClient, world.fake), world.config, panes, records, str(world.proc))
    assert found.replaceable == ["older", "newer"]
    assert sorted(found.kept) == [
        ("busy", "foreground pgid 200 != shell pid 100; running: git"),
        ("jobs", f"session {jobs} still holds 1 other process(es): {jobs + 1}"),
        ("left", "tab t-split holds 2 panes; only an unsplit tab is replaced"),
        ("right", "tab t-split holds 2 panes; only an unsplit tab is replaced"),
    ]
    assert world.fake.closed_tabs == []
    assert world.audit_entries() == []
    # The first replaceable tab is the one the cap closes.
    eviction = world.evict()
    assert isinstance(eviction, Eviction)
    assert eviction.candidate.pane_id == "older"


def test_the_survey_neither_waits_for_nor_takes_a_pane_lock(tmp_path: Path) -> None:
    world = World(tmp_path, ["held"])
    world.idle("held")
    with state.open_lock_file(state.pane_lock_path("held")) as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        found = survey(cast(HerdrClient, world.fake), world.config, world.fake.panes("w1"), [], str(world.proc))
    assert found.replaceable == ["held"]
    assert world.fake.probed == ["held"]


def test_surveying_an_all_busy_workspace_yields_the_cap_refusal(tmp_path: Path) -> None:
    world = World(tmp_path, ["a", "b"])
    config = dataclasses.replace(world.config, workspace="commands")
    found = survey_workspace(cast(HerdrClient, world.fake), config, [], str(world.proc))
    assert found.replaceable == []
    assert describe_skipped(found.kept) == (
        "None of its 2 tab(s) could be replaced: pane a: foreground pgid 200 != shell pid 100; "
        "running: git; pane b: foreground pgid 200 != shell pid 100; running: git."
    )


def test_surveying_a_missing_workspace_raises_unavailable(tmp_path: Path) -> None:
    world = World(tmp_path, ["a"])
    config = dataclasses.replace(world.config, workspace="nowhere")
    with pytest.raises(HerdrUnavailable, match='herdr has no workspace labelled "nowhere"'):
        survey_workspace(cast(HerdrClient, world.fake), config, [], str(world.proc))


class _StickyCloseClient(FakeHerdrClient):
    """``tab close`` reports success, but the listing never changes."""

    def close_tab(self, tab_id: str) -> None:
        self.closed_tabs.append(tab_id)
        assert len(self.closed_tabs) <= 10, f"runaway eviction: {self.closed_tabs}"


def test_the_cap_stops_closing_when_the_listing_never_shrinks(tmp_path: Path) -> None:
    world = World(tmp_path, ["a", "b", "c", "d"])
    sticky = _StickyCloseClient()
    sticky.workspaces = world.fake.workspaces
    sticky.busy_panes = world.fake.busy_panes
    sticky.shell_pids = world.fake.shell_pids
    world.idle("a")
    config = Config(project_root=str(world.project), max_panes=3)
    with pytest.raises(HerdrUnavailable, match="max_panes is 3"):
        _enforce_pane_cap(cast(HerdrClient, sticky), config, "w1", "newcomer", str(world.proc))
    # One close per pane over the cap, plus the one that makes room: never an endless loop.
    assert sticky.closed_tabs == ["t-a", "t-a"]


# --- the idle lifetime -----------------------------------------------------------------------------
#
# The finish a TTL verdict reads is the completion marker's mtime, so these plant a real
# ``exit_code`` file beside each ``meta.json`` and set its mtime, then judge at a fixed ``now``.

NOW = 1_800_000_000.0


class TtlWorld(World):
    """A world whose config names the fake's workspace, with finished-run planting."""

    def __init__(self, tmp_path: Path, panes: list[str], *, ttl: float = 900.0) -> None:
        super().__init__(tmp_path, panes)
        self.config = dataclasses.replace(
            self.config, workspace="commands", idle_tab_ttl_seconds=ttl
        )

    def finished(
        self,
        pane: str,
        run_id: str,
        agent: str,
        finished_at: float,
        *,
        exit_code: int | None = 0,
        marker: bool = True,
        workspace: str = "commands",
        tab_id: str | None = None,
    ) -> None:
        """Plant one run record, and its completion marker aged to ``finished_at``."""
        directory = self.project / ".herdr-run" / "runs" / run_id
        directory.mkdir(parents=True)
        document = {
            "agent": agent,
            "pane_id": pane,
            "run_id": run_id,
            "exit_code": exit_code,
            "tab": {"id": tab_id or f"t-{pane}", "label": agent},
            "workspace": {"id": "w1", "label": workspace},
        }
        (directory / "meta.json").write_text(json.dumps(document))
        if marker:
            marker_path = directory / "exit_code"
            marker_path.write_text(f"{0 if exit_code is None else exit_code}\n")
            stamp = int(finished_at * 1_000_000_000)
            os.utime(marker_path, ns=(stamp, stamp))

    def retire(self, **kwargs: object):  # type: ignore[no-untyped-def]
        return retire_idle_tabs(
            cast(HerdrClient, self.fake),
            self.config,
            actor="tester",
            now=NOW,
            proc_root=str(self.proc),
            **kwargs,  # type: ignore[arg-type]
        )


def test_an_expired_idle_tab_is_retired_and_logged(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    world = TtlWorld(tmp_path, ["old", "fresh"])
    world.idle("old")
    world.idle("fresh")
    world.finished("old", "20261001T090000-alpha-1", "alpha", NOW - 901)
    world.finished("fresh", "20261001T100000-beta-2", "beta", NOW - 899)

    result = world.retire()
    assert [tab.pane_id for tab in result.closed] == ["old"]
    assert world.fake.closed_tabs == ["t-old"]
    assert result.kept == (("fresh", "last run finished within the idle TTL"),)
    assert (
        "herdr-run: retired idle tab t-old (pane old, agent alpha, last run "
        "2026-10-01T09:00:00Z) idle past the 900s TTL"
    ) in capsys.readouterr().err
    [entry] = world.audit_entries()
    assert entry["verdict"] == "RETIRED"
    assert entry["agent"] == "tester"
    assert entry["retired_agent"] == "alpha"
    assert entry["last_run"] == "20261001T090000-alpha-1"


def test_a_finish_exactly_at_the_ttl_boundary_keeps_the_tab(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["edge"])
    world.idle("edge")
    world.finished("edge", "20261001T090000-alpha-1", "alpha", NOW - 900)
    result = world.retire()
    assert result.closed == ()
    assert result.kept == (("edge", "last run finished within the idle TTL"),)


def test_expired_but_busy_tabs_are_never_retired(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["running", "jobs", "split-a"])
    world.fake.workspaces["w1"].tabs["t-split"] = FakeTab(
        "t-split", "split", "w1", ["left", "right"]
    )
    jobs_pid = world.idle("jobs", background=1)
    world.idle("left")
    world.idle("right")
    # "running" stays in busy_panes from construction: its shell runs a command.
    world.finished("running", "20261001T080000-a-1", "a", NOW - 5000)
    world.finished("jobs", "20261001T080100-b-2", "b", NOW - 5000)
    world.finished("left", "20261001T080200-c-3", "c", NOW - 5000, tab_id="t-split")

    result = world.retire()
    assert result.closed == ()
    assert world.fake.closed_tabs == []
    reasons = dict(result.kept)
    assert reasons["running"] == "foreground pgid 200 != shell pid 100; running: git"
    assert reasons["jobs"] == f"session {jobs_pid} still holds 1 other process(es): {jobs_pid + 1}"
    assert reasons["left"] == "tab t-split holds 2 panes; only an unsplit tab is replaced"


def test_a_locked_expired_tab_is_kept_without_probing(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["held"])
    world.idle("held")
    world.finished("held", "20261001T090000-alpha-1", "alpha", NOW - 5000)
    with state.open_lock_file(state.pane_lock_path("held")) as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = world.retire()
    assert result.closed == ()
    assert result.kept == (("held", "another herdr-run holds the pane lock"),)
    assert world.fake.probed == []


def test_an_unfinished_run_and_an_unrecorded_pane_are_left_alone(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["thinking", "ghost"])
    world.idle("thinking")
    world.idle("ghost")
    world.finished("thinking", "20261001T090000-alpha-1", "alpha", NOW - 5000)
    world.finished(
        "thinking", "20261001T100000-alpha-2", "alpha", NOW - 4000, exit_code=None, marker=False
    )
    result = world.retire()
    assert result.closed == ()
    # The unrecorded ghost pane is not a candidate at all: reap's population, not this one's.
    assert result.kept == (
        ("thinking", "1 run(s) for this pane have no recorded exit_code"),
    )


def test_a_late_completion_is_read_from_the_marker(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["late"])
    world.idle("late")
    # Timed out when recorded (null exit code); the marker landed afterwards, long ago.
    world.finished("late", "20261001T090000-alpha-1", "alpha", NOW - 5000, exit_code=None)
    result = world.retire()
    assert [tab.pane_id for tab in result.closed] == ["late"]


def test_ttl_zero_disables_retirement(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["ancient"], ttl=0.0)
    world.idle("ancient")
    world.finished("ancient", "20201001T090000-alpha-1", "alpha", NOW - 10_000_000)
    result = world.retire()
    assert result.closed == ()
    assert result.kept == (("ancient", "the idle TTL is disabled"),)


def test_a_dry_run_judges_and_closes_nothing(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["old"])
    world.idle("old")
    world.finished("old", "20261001T090000-alpha-1", "alpha", NOW - 5000)
    result = world.retire(dry_run=True)
    assert result.closed == ()
    assert [tab.pane_id for tab in result.would_close] == ["old"]
    assert world.fake.closed_tabs == []
    assert world.audit_entries() == []


def test_the_limit_bounds_examination_oldest_finish_first(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["newest", "oldest", "middle"])
    for pane, run_id, age in (
        ("newest", "20261001T120000-n-1", 1000),
        ("oldest", "20261001T090000-o-2", 5000),
        ("middle", "20261001T100000-m-3", 3000),
    ):
        world.idle(pane)
        world.finished(pane, run_id, pane, NOW - age)
    result = world.retire(limit=2)
    assert [tab.pane_id for tab in result.closed] == ["oldest", "middle"]
    assert result.kept == (
        ("newest", "not examined: this sweep's limit of 2 tab(s) was reached"),
    )


def test_an_out_of_scope_record_and_the_excluded_pane_are_kept(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["foreign", "mine"])
    world.idle("foreign")
    world.idle("mine")
    world.finished(
        "foreign", "20261001T090000-alpha-1", "alpha", NOW - 5000, workspace="someone-elses"
    )
    world.finished("mine", "20261001T090000-beta-2", "beta", NOW - 5000)
    result = world.retire(exclude_pane_id="mine")
    assert result.closed == ()
    assert result.kept == (
        ("foreign", 'tab "alpha" in workspace "someone-elses" is not one of ours'),
    )


def test_a_missing_workspace_reports_instead_of_closing(tmp_path: Path) -> None:
    world = TtlWorld(tmp_path, ["old"])
    world.config = dataclasses.replace(world.config, workspace="nowhere")
    result = world.retire()
    assert result.closed == ()
    assert result.listing_error == 'herdr has no workspace labelled "nowhere"'
