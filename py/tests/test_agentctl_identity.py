"""Recipient identity around every input effect, anchors, rename transactions, and doctor."""
from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path
from typing import cast

import pytest

import agentctl.subagents as subagents
from agentctl.client import AgentPaneInfo, Pane
from agentctl.errors import AgentDeliveryError, AgentPending, AgentPossiblySubmitted, HerdrUnavailable
from agentctl.subagents import ManagedAgents

from .test_herdr_subagents import FakeManagedClient, setup


def _started(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str = "worker",
) -> tuple[ManagedAgents, FakeManagedClient, str]:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start(name, cwd=str(tmp_path), harness="claude")
    record = manager.get(name)
    assert record.pane_id is not None
    return manager, fake, record.pane_id


def _rows(report: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], report["records"])


def _findings(report: dict[str, object]) -> dict[str, list[str]]:
    return {str(row["name"]): cast(list[str], row["findings"]) for row in _rows(report)}


def _failed_documents(manager: ManagedAgents, name: str) -> list[dict[str, object]]:
    failed = Path(manager._queue(name)) / "failed"
    return [json.loads(path.read_text(encoding="utf-8"))
            for path in sorted(failed.glob("*.json"))]


# Anchors recorded by start.

def test_start_pins_terminal_and_harness_process(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    record = manager.get("worker")
    assert record.terminal_id == fake.infos[pane].terminal_id
    assert record.harness_identity is not None
    assert record.harness_identity.pid == fake.harness_pids[pane]


# The guard before and after each effect.

def test_harness_replaced_before_send_types_nothing_and_stays_pending(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    fake.harness_pids[pane] = 999  # another program now runs in the same terminal
    with pytest.raises(AgentPending, match="no longer the foreground"):
        manager.send("worker", "for the original agent")
    assert "for the original agent" not in fake.submitted


def test_replacement_after_the_last_check_is_quarantined_as_probable_misroute(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)

    def replace_harness(effect_pane: str) -> None:
        fake.harness_pids[effect_pane] = 999

    fake.before_effect = replace_harness
    with pytest.raises(AgentPossiblySubmitted, match="PROBABLE MISROUTE"):
        manager.send("worker", "raced message")
    documents = _failed_documents(manager, "worker")
    assert [document["text"] for document in documents] == ["raced message"]
    assert documents[0]["probable_misroute"] is True
    assert pane in fake.harness_pids


def test_label_drift_refuses_input(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake, _pane = _started(tmp_path, monkeypatch)
    record = manager.get("worker")
    assert record.tab_id is not None
    fake.labels[record.tab_id] = "someone-else"
    with pytest.raises(AgentPending, match="tab label is 'someone-else'"):
        manager.send("worker", "hello")


def test_terminal_change_refuses_input(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    fake.infos[pane] = replace(fake.infos[pane], terminal_id="term-restarted")
    with pytest.raises(AgentPending, match="terminal is 'term-restarted'"):
        manager.send("worker", "hello")


def test_expect_capability_is_used_and_herdr_refusal_types_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    record = manager.get("worker")
    fake.expect_supported = True
    manager.send("worker", "first")
    assert fake.expected_terminals[-1] == record.terminal_id

    def swap_terminal(effect_pane: str) -> None:
        fake.infos[effect_pane] = replace(fake.infos[effect_pane], terminal_id="term-other")

    fake.before_effect = swap_terminal
    with pytest.raises(AgentPending, match="expectation_failed"):
        manager.send("worker", "second")
    assert "second" not in fake.submitted
    assert _failed_documents(manager, "worker") == []


def test_without_the_capability_no_expectation_is_sent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, _pane = _started(tmp_path, monkeypatch)
    manager.send("worker", "first")
    assert fake.expected_terminals == [None]


# Legacy records and explicit anchoring.

def _strip_anchors(manager: ManagedAgents, fake: FakeManagedClient, name: str) -> None:
    record = manager.get(name)
    record.terminal_id = None
    record.harness_identity = None
    record.session_agent = record.session_value = None
    manager._save(record)
    fake.infos[record.pane_id or ""] = replace(
        fake.infos[record.pane_id or ""], session_agent=None, session_value=None,
    )


def test_unanchored_record_refuses_input_until_anchored(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    _strip_anchors(manager, fake, "worker")
    with pytest.raises(AgentPending, match="agentctl anchor worker"):
        manager.send("worker", "queued until anchored")
    result = manager.anchor("worker")
    assert result["harness_pid"] == fake.harness_pids[pane] and result["replaced"] is False
    manager.drain("worker")
    assert fake.submitted[-1] == "queued until anchored"


def test_anchor_refuses_a_changed_harness_without_replace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    fake.harness_pids[pane] = 999
    with pytest.raises(AgentDeliveryError, match="rerun with --replace"):
        manager.anchor("worker")
    assert manager.anchor("worker", replace=True)["replaced"] is True
    manager.send("worker", "after explicit re-anchor")


def test_duplicate_claim_is_refused_at_anchor(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    manager.start("other", cwd=str(tmp_path), harness="claude")
    other = manager.get("other")
    other.pane_id = pane
    other.terminal_id = None
    manager._save(other)
    with pytest.raises(AgentDeliveryError, match="already registered as"):
        manager.anchor("worker", replace=True)


# Rename.

def test_rename_moves_registry_name_herdr_name_and_label_together(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch, "old")
    tab = manager.get("old").tab_id or ""
    result = manager.rename("old", "new")
    assert result["herdr_steps"] == "done" and result["recovered"] is False
    assert not manager._directory("old").exists()
    record = manager.get("new")
    assert record.pane_id == pane
    assert [entry["name"] for entry in record.name_history] == ["old"]
    assert fake.agent_names()["new"] == pane and "old" not in fake.agent_names()
    assert fake.labels[tab] == "new"
    assert not list((manager.registry / ".renames").glob("*.json"))
    manager.send("new", "work under the new name")
    assert fake.submitted[-1] == "work under the new name"
    with pytest.raises(AgentDeliveryError, match="unknown agent"):
        manager.send("old", "nobody")


@pytest.mark.parametrize("prepare,message", [
    ("registered", "already registered"),
    ("herdr-name", "already named"),
    ("label", "already labelled"),
    ("unanchored", "agentctl anchor old"),
])
def test_rename_preconditions_refuse_before_any_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, prepare: str, message: str,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch, "old")
    tab = manager.get("old").tab_id or ""
    if prepare == "registered":
        manager.start("new", cwd=str(tmp_path), harness="claude")
    elif prepare == "herdr-name":
        fake.launched.append(("new", "claude", pane, ()))
        fake.launched.append(("old", "claude", pane, ()))
    elif prepare == "label":
        fake.presentations.append(Pane("w1:p99", "w1:t99", "w1"))
        fake.infos["w1:p99"] = AgentPaneInfo(
            "w1:p99", "w1", str(tmp_path), None, "unknown", None, None,
            terminal_id="term-99", tab_id="w1:t99",
        )
        fake.labels["w1:t99"] = "new"
    else:
        _strip_anchors(manager, fake, "old")
    with pytest.raises(AgentDeliveryError, match=message):
        manager.rename("old", "new")
    assert manager._directory("old").exists()
    assert fake.labels[tab] == "old"
    assert not (manager.registry / ".renames").exists() or not list(
        (manager.registry / ".renames").glob("*.json"))


def test_crash_after_journal_blocks_both_names_and_rerun_completes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch, "old")
    real = fake.rename_tab

    def crash(tab_id: str, label: str) -> None:
        raise HerdrUnavailable("simulated crash inside rename")

    monkeypatch.setattr(fake, "rename_tab", crash)
    with pytest.raises(HerdrUnavailable, match="simulated crash"):
        manager.rename("old", "new")
    assert len(list((manager.registry / ".renames").glob("*.json"))) == 1
    for name in ("old", "new"):
        with pytest.raises(AgentDeliveryError, match="rerun `agentctl rename old new`"):
            manager.send(name, "blocked")
    with pytest.raises(AgentDeliveryError, match="incomplete"):
        manager.start("new", cwd=str(tmp_path), harness="claude")
    with pytest.raises(AgentDeliveryError, match="rerun exactly"):
        manager.rename("old", "other")
    monkeypatch.setattr(fake, "rename_tab", real)
    result = manager.rename("old", "new")
    assert result["recovered"] is True
    assert [entry["name"] for entry in manager.get("new").name_history] == ["old"]
    assert manager.get("new").pane_id == pane


def test_crash_between_publication_and_move_recovers_with_one_history_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake, _pane = _started(tmp_path, monkeypatch, "old")
    real = subagents._rename_directory_noreplace_at
    calls = {"count": 0}

    def crash_once(*arguments: object) -> None:
        calls["count"] += 1
        if calls["count"] == 1:
            raise AgentDeliveryError("simulated crash before the directory move")
        real(*arguments)  # type: ignore[arg-type]

    monkeypatch.setattr(subagents, "_rename_directory_noreplace_at", crash_once)
    with pytest.raises(AgentDeliveryError, match="simulated crash"):
        manager.rename("old", "new")
    on_disk = json.loads((manager._directory("old") / "agent.json").read_text(encoding="utf-8"))
    assert on_disk["name"] == "new"  # content published inside OLD, not moved
    manager.rename("old", "new")
    history = manager.get("new").name_history
    assert [entry["name"] for entry in history] == ["old"]


def test_recovery_refuses_a_replaced_harness(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch, "old")
    monkeypatch.setattr(fake, "rename_tab", lambda tab, label: (_ for _ in ()).throw(
        HerdrUnavailable("simulated crash")))
    with pytest.raises(HerdrUnavailable):
        manager.rename("old", "new")
    monkeypatch.undo()
    fake.harness_pids[pane] = 999
    with pytest.raises(AgentDeliveryError, match="anchored harness process"):
        manager.rename("old", "new")


def test_recovery_with_the_pane_gone_finishes_the_registry_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch, "old")
    real = fake.rename_tab
    monkeypatch.setattr(fake, "rename_tab", lambda tab, label: (_ for _ in ()).throw(
        HerdrUnavailable("simulated crash")))
    with pytest.raises(HerdrUnavailable):
        manager.rename("old", "new")
    monkeypatch.setattr(fake, "rename_tab", real)
    fake.presentations = [entry for entry in fake.presentations if entry.pane_id != pane]
    result = manager.rename("old", "new")
    assert result["herdr_steps"] == "skipped-pane-missing"
    assert manager._directory("new").exists() and not manager._directory("old").exists()


def test_recovery_refuses_when_both_directories_exist(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, _pane = _started(tmp_path, monkeypatch, "old")
    monkeypatch.setattr(fake, "rename_tab", lambda tab, label: (_ for _ in ()).throw(
        HerdrUnavailable("simulated crash")))
    with pytest.raises(HerdrUnavailable):
        manager.rename("old", "new")
    manager._directory("new").mkdir(mode=0o700)
    with pytest.raises(AgentDeliveryError, match="both agent directories"):
        manager.rename("old", "new")


def test_adopted_rename_changes_only_the_registry_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.workspace = "w1"
    fake.serial = 40
    fake.presentations.append(Pane("w1:p40", "w1:t40", "w1"))
    fake.labels["w1:t40"] = "human-chosen"
    fake.infos["w1:p40"] = AgentPaneInfo(
        "w1:p40", "w1", str(tmp_path), "claude", "idle", None, None,
        terminal_id="term-40", tab_id="w1:t40",
    )
    manager.adopt("foreign", pane_id="w1:p40", expected_workspace="subagents",
                  expected_cwd=str(tmp_path), harness="claude")
    result = manager.rename("foreign", "renamed")
    assert result["herdr_steps"] == "done"
    assert fake.labels["w1:t40"] == "human-chosen"
    assert manager.get("renamed").adapter == "herdr-foreign"


# Doctor.

def test_doctor_reports_drift_and_repairs_only_presentation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    assert manager.doctor()["clean"] is True
    tab = manager.get("worker").tab_id or ""
    fake.labels[tab] = "retitled-by-hand"
    report = manager.doctor()
    assert report["clean"] is False
    assert _rows(report)[0]["findings"] == ["label-mismatch"]
    assert fake.labels[tab] == "retitled-by-hand"  # read-only by default
    repaired = manager.doctor(repair_labels=True)
    assert _rows(repaired)[0]["repaired"] is True and fake.labels[tab] == "worker"
    fake.labels[tab] = "retitled-by-hand"
    fake.harness_pids[pane] = 999
    report = manager.doctor(repair_labels=True)
    assert set(_findings(report)["worker"]) == {"label-mismatch", "harness-replaced"}
    assert "repaired" not in _rows(report)[0] and fake.labels[tab] == "retitled-by-hand"


def test_doctor_finds_missing_panes_legacy_records_and_unmanaged_tabs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake, pane = _started(tmp_path, monkeypatch)
    manager.start("legacy", cwd=str(tmp_path), harness="claude")
    _strip_anchors(manager, fake, "legacy")
    fake.presentations.append(Pane("w1:p77", "w1:t77", "w1"))
    fake.infos["w1:p77"] = AgentPaneInfo(
        "w1:p77", "w1", str(tmp_path), None, "unknown", None, None,
        terminal_id="term-77", tab_id="w1:t77",
    )
    fake.labels["w1:t77"] = "hand-made"
    fake.presentations = [entry for entry in fake.presentations if entry.pane_id != pane]
    report = manager.doctor()
    findings = _findings(report)
    assert findings["worker"] == ["pane-missing"]
    assert findings["legacy"] == ["unanchored"]
    assert {"finding": "unmanaged-tab", "tab_id": "w1:t77", "label": "hand-made"} in cast(
        list[dict[str, object]], report["workspace"])


def test_doctor_reports_duplicate_claims(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, _fake, pane = _started(tmp_path, monkeypatch)
    manager.start("other", cwd=str(tmp_path), harness="claude")
    other = manager.get("other")
    other.pane_id = pane
    manager._save(other)
    findings = _findings(manager.doctor())
    assert "duplicate-claim" in findings["worker"] and "duplicate-claim" in findings["other"]


def test_doctor_reports_an_incomplete_rename(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake, _pane = _started(tmp_path, monkeypatch, "old")
    monkeypatch.setattr(fake, "rename_tab", lambda tab, label: (_ for _ in ()).throw(
        HerdrUnavailable("simulated crash")))
    with pytest.raises(HerdrUnavailable):
        manager.rename("old", "new")
    report = manager.doctor()
    assert report["journals"] == [{"old": "old", "new": "new"}]
    assert "rename-incomplete" in _findings(report)["old"]
    assert report["clean"] is False
