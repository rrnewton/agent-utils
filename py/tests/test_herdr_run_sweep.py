"""Evidence gathering for the reaper, and the process-identity binding it depends on.

The point of these tests is that the reaper must be able to FIRE. A sweep that returns "UNKNOWN,
UNKNOWN, UNKNOWN" is indistinguishable from a correct sweep over a healthy workspace, so every case
here asserts the verdict it planted, and the planted-stale case asserts a non-zero STALE count.
"""

from __future__ import annotations

import json
import os
from typing import cast

from herdr_run.client import HerdrClient
from herdr_run.config import Config
from herdr_run.identity import parse_start_ticks, probe_process
from herdr_run.reap import Verdict
from herdr_run.sweep import build_evidence, load_run_records, pane_ids_in, sweep
from tests.herdr_fake import FakeHerdrClient

BOOT = "3f2b1c8e-0000-4000-8000-000000000001"


# --- /proc/<pid>/stat parsing ------------------------------------------------------------------


def test_start_ticks_come_from_field_22() -> None:
    # Seven tokens follow the comm (fields 3..9), then the numbers 10..22 in order, so the value
    # standing in field 22 is literally 22. Anything else means the field numbering slipped.
    line = "1234 (bash) S 1 1234 1234 0 -1 4194304 " + " ".join(str(n) for n in range(10, 23))
    assert parse_start_ticks(line) == 22


def test_a_comm_containing_spaces_and_parentheses_does_not_shift_the_fields() -> None:
    """This is why the split is at the LAST ')' and not on whitespace."""
    honest = "77 (bash) S " + " ".join(str(n) for n in range(3, 40))
    hostile = "77 (my prog (v2)) S " + " ".join(str(n) for n in range(3, 40))
    assert parse_start_ticks(hostile) == parse_start_ticks(honest)


def test_a_truncated_stat_line_is_unknown_not_a_guess() -> None:
    assert parse_start_ticks("77 (bash) S 1 2 3") is None
    assert parse_start_ticks("no parenthesis here") is None
    assert parse_start_ticks("77 (bash) S " + " ".join(["x"] * 30)) is None


def test_an_absent_process_is_gone_and_an_unreadable_one_is_not(tmp_path: object) -> None:
    """"Gone" authorises reaping; "could not tell" must never be mistaken for it."""
    proc = str(tmp_path)
    absent = probe_process(4242, proc_root=proc)
    assert absent.gone is True and absent.error is None

    os.makedirs(os.path.join(proc, "77"))
    with open(os.path.join(proc, "77", "stat"), "w", encoding="utf-8") as handle:
        handle.write("garbage without the right shape\n")
    unreadable = probe_process(77, proc_root=proc)
    assert unreadable.gone is False
    assert unreadable.error is not None


def test_this_process_can_be_bound_against_the_real_proc() -> None:
    probe = probe_process(os.getpid())
    assert probe.gone is False and probe.error is None
    assert probe.start_ticks is not None and probe.start_ticks > 0


# --- the sweep ---------------------------------------------------------------------------------


def _config(root: str, **kwargs: object) -> Config:
    base: dict[str, object] = {"project_root": root, "spool_dir": "spool"}
    base.update(kwargs)
    return Config(**base)  # type: ignore[arg-type]


def _client(fake: FakeHerdrClient) -> HerdrClient:
    return cast(HerdrClient, fake)


def _record(
    *,
    pane_id: str,
    agent: str = "kvm",
    workspace: str = "agent-cmds",
    tab_label: str | None = None,
    exit_code: int | None = 0,
    shell_pid: int = 4242,
    boot_id: str | None = BOOT,
    start_ticks: int | None = 900,
) -> dict[str, object]:
    return {
        "pane_id": pane_id,
        "agent": agent,
        "exit_code": exit_code,
        "workspace": {"label": workspace, "id": "w1"},
        "tab": {"label": agent if tab_label is None else tab_label, "id": "w1:t1"},
        "readiness": {
            "shell_pid": shell_pid,
            "boot_id": boot_id,
            "shell_start_ticks": start_ticks,
        },
    }


def _write_spool(root: str, records: list[dict[str, object]]) -> None:
    runs = os.path.join(root, "spool", "runs")
    for index, record in enumerate(records):
        directory = os.path.join(runs, f"20260819T00000{index}-agent-{index}")
        os.makedirs(directory)
        with open(os.path.join(directory, "meta.json"), "w", encoding="utf-8") as handle:
            json.dump(record, handle)


def _fake_with_pane(pane_id: str, *, shell_pid: int) -> FakeHerdrClient:
    """A live session holding exactly one workspace/tab/pane with a chosen shell pid."""
    fake = FakeHerdrClient()
    fake.create_workspace(label="agent-cmds", cwd="/tmp")
    workspace = next(iter(fake.workspaces.values()))
    tab = next(iter(workspace.tabs.values()))
    tab.label = "kvm"
    tab.pane_ids[:] = [pane_id]
    fake.shell_pids[pane_id] = shell_pid
    return fake


def _proc_with(tmp_path: str, pid: int, ticks: int) -> str:
    proc = os.path.join(tmp_path, "proc")
    os.makedirs(os.path.join(proc, "sys", "kernel", "random"), exist_ok=True)
    with open(os.path.join(proc, "sys", "kernel", "random", "boot_id"), "w", encoding="utf-8") as h:
        h.write(BOOT + "\n")
    if pid:
        os.makedirs(os.path.join(proc, str(pid)), exist_ok=True)
        with open(os.path.join(proc, str(pid), "stat"), "w", encoding="utf-8") as handle:
            handle.write(f"{pid} (bash) S " + " ".join(["0"] * 18) + f" {ticks}\n")
    return proc


def test_load_run_records_skips_a_corrupt_entry_without_losing_the_rest(tmp_path: object) -> None:
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1"), _record(pane_id="w1:p2")])
    broken = os.path.join(root, "spool", "runs", "20260819T999999-broken")
    os.makedirs(broken)
    with open(os.path.join(broken, "meta.json"), "w", encoding="utf-8") as handle:
        handle.write("{not json")

    records = load_run_records(_config(root))
    assert pane_ids_in(records) == ("w1:p1", "w1:p2")


def test_a_husk_pane_whose_shell_died_is_reaped(tmp_path: object) -> None:
    """POSITIVE CONTROL. Herdr still lists the tab; the shell that owned it no longer exists."""
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 0, 0)  # no /proc/4242: the shell is positively gone

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    assert plan.counts()["STALE"] == 1
    assert [decision.pane_id for decision in plan.reapable] == ["w1:p1"]


def test_a_live_shell_is_spared(tmp_path: object) -> None:
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 4242, 900)

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    assert plan.counts()["SHELL_ALIVE"] == 1
    assert plan.reapable == ()


def test_an_unfinished_run_beats_a_dead_shell(tmp_path: object) -> None:
    """The agent-is-thinking case, planted with the shell ALSO gone so only R1 can spare it."""
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1"), _record(pane_id="w1:p1", exit_code=None)])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 0, 0)

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    assert plan.counts()["IN_FLIGHT"] == 1
    assert plan.counts()["STALE"] == 0


def test_an_unreadable_proc_entry_is_unknown_not_stale(tmp_path: object) -> None:
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 0, 0)
    os.makedirs(os.path.join(proc, "4242"))
    with open(os.path.join(proc, "4242", "stat"), "w", encoding="utf-8") as handle:
        handle.write("truncated\n")

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    assert plan.counts()["UNKNOWN"] == 1
    assert plan.counts()["STALE"] == 0


def test_a_tab_retargeted_out_of_the_schema_is_out_of_scope(tmp_path: object) -> None:
    """Scope is recomputed from the CURRENT config, not read back from the record."""
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 0, 0)

    plan = sweep(_client(fake), _config(root, tab_name="{project}-{agent}"), proc_root=proc)
    assert plan.counts()["OUT_OF_SCOPE"] == 1
    assert plan.counts()["STALE"] == 0


def test_a_pane_in_another_workspace_is_out_of_scope(tmp_path: object) -> None:
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1", workspace="someone-elses")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 0, 0)

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    assert plan.counts()["OUT_OF_SCOPE"] == 1
    assert plan.reapable == ()


def test_a_pane_herdr_no_longer_lists_is_unknown(tmp_path: object) -> None:
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1"), _record(pane_id="w1:pGONE")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 4242, 900)

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    verdicts = {decision.pane_id: decision.verdict for decision in plan.decisions}
    assert verdicts["w1:pGONE"] == Verdict.UNKNOWN
    assert verdicts["w1:p1"] == Verdict.SHELL_ALIVE


def test_an_unanswering_herdr_reaps_nothing(tmp_path: object) -> None:
    """"herdr did not answer" must never be read as "every pane is gone"."""
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    fake.fail_pane_list = True
    proc = _proc_with(root, 0, 0)

    plan = sweep(_client(fake), _config(root), proc_root=proc)
    assert plan.counts()["STALE"] == 0
    assert plan.counts()["UNKNOWN"] == 1
    # And it must say the SERVER did not answer, not that herdr no longer lists the pane. The
    # verdict is the same either way, but the second sentence tells an operator the tabs are
    # already gone -- which is the opposite of what happened.
    assert "not answering" in plan.declined[0].reason


def test_an_empty_spool_reports_zero_of_everything(tmp_path: object) -> None:
    root = str(tmp_path)
    plan = sweep(_client(FakeHerdrClient()), _config(root), proc_root=str(tmp_path))
    counts = plan.counts()
    assert counts["considered"] == 0
    # Every verdict is present even at zero, so an inert sweep still prints its own shape.
    assert set(counts) >= {"STALE", "IN_FLIGHT", "SHELL_ALIVE", "UNKNOWN", "OUT_OF_SCOPE"}


def test_evidence_carries_the_recorded_identity_the_policy_needs(tmp_path: object) -> None:
    root = str(tmp_path)
    _write_spool(root, [_record(pane_id="w1:p1")])
    fake = _fake_with_pane("w1:p1", shell_pid=4242)
    proc = _proc_with(root, 4242, 900)

    evidence = build_evidence(_client(fake), _config(root), proc_root=proc)
    assert len(evidence) == 1
    recorded = evidence[0].recorded_shell
    assert recorded is not None and recorded.is_bound(), "a bare pid can never authorise anything"
