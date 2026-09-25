"""Lifecycle regressions for visible subagents, including failures and ownership changes."""
from __future__ import annotations

import hashlib
import json
import os
import time
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from typing import cast

import pytest

from agentctl.client import (
    AgentPaneInfo, CustomProcessIdentity, HerdrClient, Pane, PaneMove, PaneShellProof,
)
from agentctl.errors import (
    AgentDeliveryError, AgentPending, AgentPossiblySubmitted, HerdrUnavailable,
)
from agentctl.subagents import ManagedAgents, environment_entries, harness_arguments
import agentctl.legacy_cli as cli
import agentctl.cli as unified_cli
import agentctl.codex_goal as native_goal
import agentctl.agent as agent


class FakeManagedClient:
    def __init__(self) -> None:
        self.workspace: str | None = None
        self.infos: dict[str, AgentPaneInfo] = {}
        self.presentations: list[Pane] = []
        self.launched: list[tuple[str, str, str, tuple[str, ...]]] = []
        self.environments: list[tuple[str, ...]] = []
        self.closed: list[str] = []
        self.submitted: list[str] = []
        self.panes_calls = 0
        self.pane_info_calls = 0
        self.serial = 0
        self.offline = False
        self.fail_start = False
        self.custom_dies_after_report = False
        self.custom_fails_after_observation = False
        self.custom_dies_during_recovery_commit = False
        self.custom_running = False
        self.custom_ready = True
        self.fail_after_move = False
        self.wrong_move_response = False
        self.move_count = 0
        self.replace_terminal_before_move = False
        self.wait_fails = False
        self.working_after_prompt = False
        self.custom_at_idle_shell = True
        self.custom_identity = CustomProcessIdentity(
            version=1, boot_id="00000000-0000-0000-0000-000000000000",
            pid=200, starttime_ticks=200, executable_device=1, executable_inode=2,
        )
        self.foreign_shell_identity = CustomProcessIdentity(
            version=1, boot_id="11111111-2222-3333-4444-555555555555",
            pid=100, starttime_ticks=100, executable_device=3, executable_inode=4,
        )

    def workspace_id_for_label(self, label: str) -> str | None:
        assert label == "subagents"
        return self.workspace

    def workspace_label(self, workspace_id: str) -> str:
        return "subagents" if workspace_id == "w1" else f"label-{workspace_id}"

    def create_workspace(
        self, *, label: str, cwd: str, environment: tuple[str, ...] = (),
    ) -> tuple[str, str, str]:
        self.workspace = "w1"
        tab = self.create_tab(
            workspace_id="w1", label=label, cwd=cwd, environment=environment
        )
        return "w1", tab, self.presentations[-1].pane_id

    def create_tab(
        self, *, workspace_id: str, label: str, cwd: str,
        environment: tuple[str, ...] = (),
    ) -> str:
        del label
        self.environments.append(tuple(environment))
        self.serial += 1
        tab, pane = f"w1:t{self.serial}", f"w1:p{self.serial}"
        self.presentations.append(Pane(pane, tab, workspace_id, f"term-{self.serial}"))
        self.infos[pane] = AgentPaneInfo(pane, workspace_id, cwd, None, "unknown", None, None)
        return tab

    def rename_tab(self, tab_id: str, label: str) -> None:
        del tab_id, label

    def create_tab_with_pane(
        self, *, workspace_id: str, label: str, cwd: str,
        environment: tuple[str, ...] = (),
    ) -> tuple[str, str]:
        tab = self.create_tab(
            workspace_id=workspace_id, label=label, cwd=cwd,
            environment=environment,
        )
        return tab, self.presentations[-1].pane_id

    def move_pane_to_new_tab(
        self, pane_id: str, *, expected_terminal_id: str,
        workspace_id: str, label: str,
    ) -> PaneMove:
        del label
        if self.replace_terminal_before_move:
            self.replace_terminal_before_move = False
            self.presentations = [
                replace(item, terminal_id="replacement-terminal")
                if item.pane_id == pane_id else item
                for item in self.presentations
            ]
        old = next(item for item in self.presentations if item.pane_id == pane_id)
        if old.terminal_id != expected_terminal_id:
            raise HerdrUnavailable("conditional terminal identity changed")
        self.move_count += 1
        moved = Pane(
            f"{workspace_id}:moved", f"{workspace_id}:tab", workspace_id,
            old.terminal_id,
        )
        self.presentations = [
            moved if item.pane_id == pane_id else item for item in self.presentations
        ]
        info = self.infos.pop(pane_id)
        self.infos[moved.pane_id] = replace(
            info, pane_id=moved.pane_id, workspace_id=workspace_id,
        )
        self.launched = [
            (name, kind, moved.pane_id if pane == pane_id else pane, arguments)
            for name, kind, pane, arguments in self.launched
        ]
        if self.fail_after_move:
            self.fail_after_move = False
            raise HerdrUnavailable("simulated lost pane move response")
        return PaneMove(
            "wrong" if self.wrong_move_response else old.pane_id,
            old.tab_id, old.workspace_id, moved,
        )

    def start_agent(self, name: str, kind: str, pane_id: str, arguments: tuple[str, ...], *, timeout: float) -> None:
        assert timeout > 0
        self.launched.append((name, kind, pane_id, arguments))
        if self.fail_start:
            raise HerdrUnavailable("trust prompt needs attention")
        self.infos[pane_id] = replace(self.infos[pane_id], agent=kind, status="idle", session_agent=kind, session_value=f"session-{self.serial}")

    def start_pane_agent(
        self, name: str, kind: str, pane_id: str, arguments: tuple[str, ...], *, timeout: float,
        on_launch_intent: Callable[[str, int, int, tuple[str, ...]], None] | None = None,
        on_observed: Callable[[CustomProcessIdentity], None] | None = None,
    ) -> CustomProcessIdentity:
        assert timeout > 0
        self.launched.append((name, kind, pane_id, arguments))
        self.custom_running = True
        if on_launch_intent is not None:
            on_launch_intent(
                "/opt/agentctl/muse", self.custom_identity.executable_device,
                self.custom_identity.executable_inode,
                ("/opt/agentctl/muse", *arguments),
            )
        if on_observed is not None:
            on_observed(self.custom_identity)
        if self.custom_fails_after_observation:
            raise HerdrUnavailable("trust prompt requires human attention")
        self.infos[pane_id] = replace(
            self.infos[pane_id], agent=kind, status="idle",
        )
        self.custom_running = not self.custom_dies_after_report
        return self.custom_identity

    def recover_pane_agent(
        self, pane_id: str, expected_argv: tuple[str, ...], expected_device: int,
        expected_inode: int, expected_pid: int,
    ) -> CustomProcessIdentity:
        assert pane_id in self.infos
        assert tuple(expected_argv) in {
            ("/opt/agentctl/muse", *self.launched[-1][3]),
            ("muse", *self.launched[-1][3]),
        }
        if (expected_pid != self.custom_identity.pid
                or expected_device != self.custom_identity.executable_device
                or expected_inode != self.custom_identity.executable_inode
                or not self.custom_running):
            raise HerdrUnavailable("custom harness recovery did not match")
        return self.custom_identity

    def commit_recovered_pane_agent(
        self, pane_id: str, kind: str, identity: CustomProcessIdentity,
        commit: Callable[[], None],
    ) -> None:
        self.verify_custom_harness(pane_id, kind, identity)
        commit()
        if self.custom_dies_during_recovery_commit:
            self.custom_dies_during_recovery_commit = False
            self.custom_running = False
        self.verify_custom_harness(pane_id, kind, identity)

    def report_pane_agent(self, pane_id: str, kind: str, state: str) -> None:
        assert state == "idle"
        self.infos[pane_id] = replace(self.infos[pane_id], agent=kind, status=state)

    def verify_custom_harness(
        self, pane_id: str, kind: str,
        expected_identity: CustomProcessIdentity | None = None,
    ) -> None:
        if (self.infos[pane_id].agent not in (None, kind) or not self.custom_running
                or (expected_identity is not None
                    and expected_identity != self.custom_identity)):
            raise HerdrUnavailable("custom harness is not the foreground process")

    def process_generation_absent(
        self, expected: CustomProcessIdentity,
    ) -> bool:
        if expected != self.custom_identity:
            return True
        return not self.custom_running

    def pane_is_idle_shell(self, pane_id: str) -> bool:
        assert pane_id in self.infos
        return self.custom_at_idle_shell

    def pane_shell_identity(self, pane_id: str) -> CustomProcessIdentity:
        assert pane_id in self.infos
        return self.foreign_shell_identity

    def verify_pane_shell_identity(
        self, pane_id: str, expected: CustomProcessIdentity,
    ) -> None:
        assert pane_id in self.infos
        if expected != self.foreign_shell_identity:
            raise HerdrUnavailable("recorded pane shell generation changed")

    def pane_is_same_idle_shell(
        self, pane_id: str, expected: CustomProcessIdentity,
    ) -> bool:
        assert pane_id in self.infos
        return self.custom_at_idle_shell and expected == self.foreign_shell_identity

    def pane_idle_shell_identity(self, pane_id: str) -> PaneShellProof | None:
        assert pane_id in self.infos
        if not self.custom_at_idle_shell:
            return None
        return PaneShellProof(
            self.foreign_shell_identity, str(Path("/bin/bash").resolve())
        )

    def agent_pane(self, name: str) -> str:
        matches = [entry[2] for entry in self.launched if entry[0] == name]
        if self.offline or not matches:
            raise HerdrUnavailable("server unavailable or named agent missing")
        return matches[-1]

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        self.pane_info_calls += 1
        if self.offline:
            raise HerdrUnavailable("server unavailable")
        return self.infos[pane_id]

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        self.panes_calls += 1
        if self.offline:
            raise HerdrUnavailable("server unavailable")
        return tuple(
            pane for pane in self.presentations
            if workspace_id is None or pane.workspace_id == workspace_id
        )

    def prompt_agent(self, pane_id: str, text: str) -> None:
        assert pane_id in self.infos
        self.submitted.append(text)
        if self.working_after_prompt:
            self.infos[pane_id] = replace(self.infos[pane_id], status="working")

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        assert pane_id in self.infos and status == "working" and timeout_ms > 0
        if self.wait_fails:
            raise HerdrUnavailable("simulated lost Herdr status event")

    def read(self, pane_id: str, *, source: str, lines: int) -> str:
        assert pane_id in self.infos and lines > 0
        if source == "recent-unwrapped":
            return ""
        if self.custom_running:
            if self.custom_ready:
                return (
                    "Muse Code at Meta\n  Muse Code 1.4.0\n"
                    "────────────────\n❯\n────────────────\n"
                    "kiki · xhigh · /work · YOLO\n"
                )
            return "Muse Code\nWorking…\n"
        return "human and coordinator transcript\n"

    def close_tab(self, tab_id: str) -> None:
        self.closed.append(tab_id)
        self.presentations = [pane for pane in self.presentations if pane.tab_id != tab_id]

    def close_pane(self, pane_id: str) -> None:
        tab = next(pane.tab_id for pane in self.presentations if pane.pane_id == pane_id)
        self.closed.append(tab)
        self.presentations = [pane for pane in self.presentations if pane.pane_id != pane_id]

    def focus_pane(self, pane_id: str) -> None:
        assert pane_id in self.infos

    def report_agent_session(self, name: str, pane_id: str, kind: str, session_id: str) -> None:
        assert self.agent_pane(name) == pane_id
        self.infos[pane_id] = replace(self.infos[pane_id], session_agent=kind, session_value=session_id)

    def send_keys(self, pane_id: str, keys: str) -> None:
        assert pane_id in self.infos and keys == "Enter"


def setup(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[ManagedAgents, FakeManagedClient]:
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    def goal(_session: str, _command: object = None) -> dict[str, object] | None:
        objectives = [text[6:] for text in fake.submitted if text.startswith("/goal ")]
        return {"status": "active", "objective": objectives[-1]} if objectives else None
    monkeypatch.setattr(native_goal, "get_goal", goal)
    return ManagedAgents(cast(HerdrClient, fake), tmp_path / "registry"), fake


def test_named_workers_share_workspace_but_never_reuse_tabs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    first = manager.start("one", cwd=str(tmp_path), harness="codex", brief="start here")
    second = manager.start("two", cwd=str(tmp_path), harness="claude", model="selected-model")
    assert first["workspace_id"] == second["workspace_id"] == "w1"
    assert first["pane_id"] != second["pane_id"]
    assert fake.submitted == ["start here"]
    assert fake.launched[0][3] == ("--no-alt-screen",)
    assert fake.launched[1][3] == ("--model", "selected-model")
    assert [item["name"] for item in manager.list()] == ["one", "two"]
    with pytest.raises(AgentDeliveryError, match="already registered"):
        manager.start("one", cwd=str(tmp_path))
    assert len(fake.launched) == 2


def test_explicit_workspace_label_never_creates_a_typo_workspace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    monkeypatch.setattr(fake, "workspace_id_for_label", lambda _label: None)
    with pytest.raises(AgentDeliveryError, match="does not exist"):
        manager.start(
            "worker", cwd=str(tmp_path), harness="codex",
            workspace_label="misspelled-project",
        )
    assert fake.presentations == []
    assert fake.launched == []


def test_relocate_preserves_runtime_and_queue_and_commits_new_route(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start("worker", cwd=str(tmp_path), harness="codex")
    record = manager.get("worker")
    record.session_agent = record.session_value = None
    record.session_source = None
    manager._save(record)
    fake.infos[record.pane_id or ""] = replace(
        fake.infos[record.pane_id or ""], session_agent=None, session_value=None,
    )
    manager.send("worker", "before relocation")
    queue_root = manager.registry / "worker/queue"
    queued = agent.enqueue(str(queue_root), "later", message_id="queued")
    queued_artifact = queue_root / "inbox" / f"{queued}.json"
    before = queued_artifact.read_bytes()
    monkeypatch.setattr(fake, "workspace_label", lambda workspace: f"label-{workspace}")

    result = manager.relocate("worker", workspace_id="w2", new_tab=True)

    assert result["workspace_id"] == "w2"
    assert result["terminal_id"] == "term-1"
    assert manager.get("worker").pane_id == "w2:moved"
    assert manager.get("worker").token == started["token"]
    assert queued_artifact.read_bytes() == before
    manager.send("worker", "after relocation")
    assert fake.submitted[0] == "before relocation"
    assert sorted(fake.submitted[1:]) == ["after relocation", "later"]
    binding = json.loads((queue_root / "target.json").read_text())
    assert binding["kind"] == "pane"
    assert binding["pane_id"] == "w2:moved"
    assert not (manager.registry / "worker/relocation.json").exists()
    assert fake.move_count == 1


@pytest.mark.parametrize("lost_response", [True, False])
def test_relocate_recovers_move_before_registry_commit(
    lost_response: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    monkeypatch.setattr(fake, "workspace_label", lambda workspace: f"label-{workspace}")
    fake.fail_after_move = lost_response
    fake.wrong_move_response = not lost_response

    with pytest.raises(HerdrUnavailable if lost_response else AgentDeliveryError):
        manager.relocate("worker", workspace_id="w2", new_tab=True)
    assert (manager.registry / "worker/relocation.json").is_file()
    fake.wrong_move_response = False

    recovered = manager.relocate("worker", workspace_id="w2", new_tab=True)
    assert recovered["pane_id"] == "w2:moved"
    assert fake.move_count == 1
    assert not (manager.registry / "worker/relocation.json").exists()


def test_relocate_reconciles_after_routing_commit_before_journal_removal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start("worker", cwd=str(tmp_path), harness="codex")
    manager.relocate("worker", workspace_id="w2", new_tab=True)
    agent._atomic_json(
        str(manager.registry / "worker/relocation.json"),
        {
            "schema": "agentctl-relocation/v1",
            "token": started["token"],
            "terminal_id": "term-1",
            "old": {"workspace_id": "w1", "tab_id": "w1:t1", "pane_id": "w1:p1"},
            "target_workspace_id": "w2",
            "new_tab": True,
        },
    )

    recovered = manager.relocate("worker", workspace_id="w2", new_tab=True)
    assert recovered["pane_id"] == "w2:moved"
    assert fake.move_count == 1
    assert not (manager.registry / "worker/relocation.json").exists()


def test_relocate_refuses_multi_pane_tab_and_ambiguous_label_without_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    fake.presentations.append(Pane("sibling", "w1:t1", "w1", "term-sibling"))
    fake.infos["sibling"] = AgentPaneInfo(
        "sibling", "w1", str(tmp_path), "codex", "idle",
        "codex", "sibling-session",
    )
    with pytest.raises(AgentDeliveryError, match="multi-pane"):
        manager.relocate("worker", workspace_id="w2", new_tab=True)
    assert not (manager.registry / "worker/relocation.json").exists()

    fake.presentations.pop()
    monkeypatch.setattr(
        fake, "workspace_id_for_label",
        lambda _label: (_ for _ in ()).throw(HerdrUnavailable("ambiguous")),
    )
    with pytest.raises(HerdrUnavailable, match="ambiguous"):
        manager.relocate("worker", workspace_label="project", new_tab=True)
    assert not (manager.registry / "worker/relocation.json").exists()


def test_relocate_conditional_move_refuses_terminal_replacement_in_final_gap(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    monkeypatch.setattr(fake, "workspace_label", lambda workspace: f"label-{workspace}")
    fake.replace_terminal_before_move = True

    with pytest.raises(HerdrUnavailable, match="conditional terminal identity changed"):
        manager.relocate("worker", workspace_id="w2", new_tab=True)

    assert fake.move_count == 0
    assert manager.get("worker").pane_id == "w1:p1"
    assert (manager.registry / "worker/relocation.json").is_file()


def test_health_isolates_dead_and_malformed_records_and_persists_detection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    first = manager.start("dead", cwd=str(tmp_path))
    manager.start("live", cwd=str(tmp_path), harness="claude")
    pane = str(first["pane_id"])
    fake.infos[pane] = replace(
        fake.infos[pane], agent=None, status="unknown",
        session_agent=None, session_value=None,
    )
    malformed = manager.registry / "stale"
    malformed.mkdir(mode=0o700)
    (malformed / "agent.json").write_text("{}\n", encoding="utf-8")
    (malformed / "agent.json").chmod(0o600)

    first_check = manager.health(checked_at=100.25)
    first_sessions = cast(list[dict[str, object]], first_check["sessions"])
    by_name = {str(item["name"]): item for item in first_sessions}
    assert first_check["healthy"] is False
    assert by_name["dead"]["health"] == "unhealthy"
    assert by_name["dead"]["reason_code"] == "expected-harness-missing"
    assert "reports agent None, expected 'codex'" in str(by_name["dead"]["reason"])
    assert by_name["live"]["health"] == "healthy"
    assert by_name["stale"]["health"] == "unknown"
    assert by_name["stale"]["recorded"] is False
    rows, list_healthy = manager.list_with_health()
    listed = {str(item["name"]): item for item in rows}
    assert list_healthy is False
    assert set(listed) == {"dead", "live", "stale"}
    assert listed["stale"]["health"] == "unknown"
    assert "invalid agent record" in str(listed["stale"]["probe_error"])
    with pytest.raises(AgentDeliveryError, match="invalid agent record"):
        manager.status_with_health("stale")

    second_check = manager.health(["dead", "live"], checked_at=200.5)
    second_sessions = cast(list[dict[str, object]], second_check["sessions"])
    second = {str(item["name"]): item for item in second_sessions}["dead"]
    assert second["first_detected_at"] == 100.25
    assert second["last_checked_at"] == 200.5
    durable = json.loads((manager.registry / "dead/health.json").read_text())
    assert durable["last_unhealthy_at"] == 200.5
    assert durable["last_unhealthy_reason"] == second["reason"]


def test_list_quarantines_fifo_agent_record_without_blocking_other_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("live", cwd=str(tmp_path))
    fifo = manager.registry / "blocked"
    fifo.mkdir(mode=0o700)
    os.mkfifo(fifo / "agent.json", mode=0o600)

    started = time.monotonic()
    rows, healthy = manager.list_with_health()

    assert time.monotonic() - started < 1.0
    by_name = {str(row["name"]): row for row in rows}
    assert healthy is False
    assert by_name["live"]["health"] == "healthy"
    assert by_name["blocked"]["health"] == "unknown"
    assert by_name["blocked"]["health_reason_code"] == "registry-or-health-error"


def test_health_transport_failure_is_unknown_not_proof_of_death(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    fake.offline = True
    result = manager.health(["worker"], checked_at=123.0)
    assert result["healthy"] is False
    session = cast(list[dict[str, object]], result["sessions"])[0]
    assert session["health"] == "unknown"
    assert session["reason_code"] == "runtime-probe-failed"


def test_health_does_not_call_transient_agent_detector_failure_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start("worker", cwd=str(tmp_path))
    pane = str(started["pane_id"])
    fake.infos[pane] = replace(
        fake.infos[pane], agent=None, status="unknown",
        session_agent=None, session_value=None,
    )
    fake.custom_at_idle_shell = False
    result = manager.health(["worker"], checked_at=123.0)
    session = cast(list[dict[str, object]], result["sessions"])[0]
    assert session["health"] == "unknown"
    assert session["runtime_state"] == "unknown"
    assert session["reason_code"] == "agent-report-missing"


def test_health_does_not_call_live_muse_process_probe_failure_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    original_verify = fake.verify_custom_harness
    calls = 0

    def fail_first_verification(
        pane_id: str, kind: str,
        expected_identity: CustomProcessIdentity | None = None,
    ) -> None:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise HerdrUnavailable("transient process-info response")
        original_verify(pane_id, kind, expected_identity)

    monkeypatch.setattr(fake, "verify_custom_harness", fail_first_verification)
    result = manager.health(["worker"], checked_at=123.0)
    session = cast(list[dict[str, object]], result["sessions"])[0]
    assert fake.custom_running is True
    assert fake.infos["w1:p1"].agent == "muse"
    assert session["health"] == "unknown"
    assert session["runtime_state"] == "unknown"
    assert session["reason_code"] == "custom-harness-verification-unconfirmed"
    assert calls == 2


def test_health_calls_stale_muse_label_dead_only_from_stable_shell_proof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    assert fake.infos["w1:p1"].agent == "muse"
    fake.custom_running = False
    fake.custom_at_idle_shell = True

    result = manager.health(["worker"], checked_at=123.0)
    session = cast(list[dict[str, object]], result["sessions"])[0]
    assert fake.infos["w1:p1"].agent == "muse"
    assert session["health"] == "unhealthy"
    assert session["runtime_state"] == "dead"
    assert session["reason_code"] == "expected-harness-missing"
    assert "stable idle shell" in str(session["reason"])


@pytest.mark.parametrize("mutation", ["workspace", "tab", "cwd", "session"])
def test_health_does_not_call_reused_idle_shell_pane_this_generation_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start("worker", cwd=str(tmp_path))
    pane = str(started["pane_id"])
    fake.infos[pane] = replace(
        fake.infos[pane], agent=None, status="unknown",
        session_agent=None, session_value=None,
    )
    if mutation == "workspace":
        fake.presentations[0] = replace(fake.presentations[0], workspace_id="replacement")
        fake.infos[pane] = replace(fake.infos[pane], workspace_id="replacement")
    elif mutation == "tab":
        fake.presentations[0] = replace(fake.presentations[0], tab_id="replacement")
    elif mutation == "cwd":
        fake.infos[pane] = replace(fake.infos[pane], cwd=str(tmp_path / "replacement"))
    else:
        fake.infos[pane] = replace(
            fake.infos[pane], session_agent="codex", session_value="replacement",
        )

    result = manager.health(["worker"], checked_at=123.0)
    session = cast(list[dict[str, object]], result["sessions"])[0]
    assert session["health"] == "unhealthy"
    assert session["runtime_state"] == "unknown"
    assert session["reason_code"] == "runtime-identity-mismatch"


def test_harness_arguments_preserve_literals_without_implicit_permission_changes() -> None:
    assert harness_arguments("codex", resume="session", extra=("--config", 'value="$(literal)"')) == (
        "resume", "session", "--no-alt-screen", "--config", 'value="$(literal)"')
    assert harness_arguments("claude", resume="session", model="chosen") == ("--resume", "session", "--model", "chosen")
    assert harness_arguments("gemini", extra=("--model=chosen",)) == ("--model=chosen",)
    with pytest.raises(AgentDeliveryError, match="presets"):
        harness_arguments("gemini", model="chosen")


def test_environment_entries_preserve_literal_values() -> None:
    entries = (
        "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
        'LITERAL= spaces $(unexpanded) "quotes" = remain ',
        "UNICODE=snowman-☃\nnext-line",
        "EMPTY=",
    )
    assert environment_entries(entries) == entries


@pytest.mark.parametrize(
    "entry",
    (
        "MISSING_EQUALS",
        "=value",
        "9START=value",
        "BAD-NAME=value",
        "BAD\0NAME=value",
        "GOOD=bad\0value",
    ),
)
def test_environment_entries_reject_invalid_or_nul_names_and_values(entry: str) -> None:
    with pytest.raises(AgentDeliveryError, match="environment"):
        environment_entries((entry,))


def test_start_sets_environment_only_on_the_created_tab(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    entries = (
        "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
        "LITERAL=a b=$(unexpanded)=tail",
    )
    status = manager.start("worker", cwd=str(tmp_path), environment=entries)
    assert fake.environments == [entries]
    assert "environment" not in status
    assert status["launch_environment_names"] == ["META_CODEX_AI_GATEWAY", "LITERAL"]
    assert status["runtime_ownership"] == "owned"
    assert all(value not in json.dumps(status) for value in entries)
    saved = json.loads((tmp_path / "registry/worker/agent.json").read_text())
    assert "environment" not in saved
    assert all(value not in json.dumps(saved) for value in entries)


def test_invalid_environment_is_refused_before_registry_or_tab_creation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="environment variable name"):
        manager.start("worker", cwd=str(tmp_path), environment=("BAD-NAME=value",))
    assert not (tmp_path / "registry").exists()
    assert fake.environments == []


def test_interactive_muse_refuses_raw_duplicate_structured_options_before_allocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="model field"):
        manager.start(
            "worker", cwd=str(tmp_path), harness="muse", model="structured",
            harness_args=("--model", "raw"),
        )
    with pytest.raises(AgentDeliveryError, match="reasoning effort"):
        manager.start(
            "worker", cwd=str(tmp_path), harness="muse", reasoning_effort="ultra",
            harness_args=("--reasoning-effort=low",),
        )
    assert not manager.registry.exists()
    assert fake.launched == []
    assert fake.environments == []


def test_interactive_start_rejects_string_harness_arguments_before_allocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="harness arguments must be a sequence"):
        manager.start(
            "worker", cwd=str(tmp_path), harness="muse", model="structured",
            harness_args="--model=raw",
        )
    assert not manager.registry.exists()
    assert fake.launched == []
    assert fake.environments == []


def test_direct_start_preserves_unstructured_raw_policy_and_nonoverriding_codex_config(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start(
        "claude-worker", cwd=str(tmp_path), harness="claude",
        harness_args=("--effort=high",),
    )
    manager.start(
        "codex-worker", cwd=str(tmp_path), harness="codex", model="structured",
        harness_args=("-c", "sandbox_mode=read-only"),
    )
    assert fake.launched[0][3] == ("--effort=high",)
    assert fake.launched[1][3] == (
        "--no-alt-screen", "--model", "structured", "-c", "sandbox_mode=read-only",
    )


def test_direct_start_rejects_quoted_config_and_opaque_profile_conflicts_before_allocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="model field"):
        manager.start(
            "worker", cwd=str(tmp_path), model="structured",
            harness_args=('--config="model"="raw"',),
        )
    with pytest.raises(AgentDeliveryError, match="reasoning effort"):
        manager.start(
            "worker", cwd=str(tmp_path), reasoning_effort="ultra",
            harness_args=('-c"model_reasoning_effort"="low"',),
        )
    with pytest.raises(AgentDeliveryError, match="raw Codex profile"):
        manager.start(
            "worker", cwd=str(tmp_path), model="structured",
            harness_args=("--profile", "attacker"),
        )
    with pytest.raises(AgentDeliveryError, match="raw Codex profile"):
        manager.start(
            "worker", cwd=str(tmp_path), reasoning_effort="ultra",
            harness_args=("-pattacker",),
        )
    for arguments in (
        ("-c", "profile=attacker"),
        ("--config=profile=attacker",),
        ('-c"profile"="attacker"',),
    ):
        with pytest.raises(AgentDeliveryError, match="raw Codex profile"):
            manager.start(
                "worker", cwd=str(tmp_path), model="structured",
                harness_args=arguments,
            )
    assert not manager.registry.exists()
    assert fake.launched == []
    assert fake.environments == []


def test_adoption_rejects_muse_before_registry_or_live_pane_access(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.workspace = "w1"
    fake.presentations.append(Pane("w1:p1", "w1:t1", "w1"))
    fake.infos["w1:p1"] = AgentPaneInfo(
        "w1:p1", "w1", str(tmp_path), "muse", "idle", None, None,
    )
    with pytest.raises(AgentDeliveryError, match="adopting Muse is unsupported"):
        manager.adopt(
            "worker", pane_id="w1:p1", expected_workspace="subagents",
            expected_cwd=str(tmp_path), harness="muse",
        )
    assert not manager.registry.exists()
    assert fake.panes_calls == 0
    assert fake.pane_info_calls == 0


def test_failed_launch_status_does_not_retain_environment_values(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    secret = "ACCESS_TOKEN=literal-sensitive-value"
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError, match="trust prompt"):
        manager.start("worker", cwd=str(tmp_path), environment=(secret,))
    record = manager.get("worker")
    assert record.lifecycle == "launch_failed"
    assert record.error == (
        "launch failed with caller-supplied environment; details omitted from status"
    )
    assert secret not in json.dumps(manager.status("worker"))


def test_failed_launch_is_inspectable_and_can_be_archived(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError, match="record and any created tab retained"):
        manager.start("failed", cwd=str(tmp_path))
    assert manager.get("failed").lifecycle == "launch_failed"
    assert "trust prompt" in str(manager.get("failed").error)
    stopped = manager.stop("failed")
    assert fake.closed == ["w1:t1"]
    assert (Path(str(stopped["archive"])) / "output.json").exists()


def test_failed_launch_cleanup_refuses_replacement_harness(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError):
        manager.start("failed", cwd=str(tmp_path))
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], agent="codex", status="idle")
    monkeypatch.setattr(fake, "agent_pane", lambda _name: "w1:p-other")
    with pytest.raises(HerdrUnavailable, match="no longer owns"):
        manager.stop("failed")
    assert fake.closed == []


def test_failed_muse_launch_with_our_stale_pane_report_is_stoppable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.custom_dies_after_report = True
    with pytest.raises(AgentDeliveryError, match="foreground process"):
        manager.start("failed", cwd=str(tmp_path), harness="muse")
    failed = manager.get("failed")
    assert failed.lifecycle == "launch_failed"
    assert failed.pane_reported_by_agentctl is True
    assert manager.stop("failed")["pane_closed"] is True


def test_muse_process_identity_is_saved_before_trust_failure_and_permits_stop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.custom_fails_after_observation = True

    with pytest.raises(AgentDeliveryError, match="trust prompt"):
        manager.start("failed", cwd=str(tmp_path), harness="muse")

    failed = manager.get("failed")
    assert failed.lifecycle == "launch_failed"
    assert failed.pane_reported_by_agentctl is False
    assert failed.custom_process_identity == fake.custom_identity
    assert manager.stop("failed")["pane_closed"] is True


def test_identityless_failed_muse_launch_requires_token_and_exact_pid_to_recover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start(
        "worker", cwd=str(tmp_path), harness="muse", harness_args=("--literal",),
    )
    assert started["launch_executable"] == "/opt/agentctl/muse"
    assert started["launch_argv"] == ["/opt/agentctl/muse", "--literal"]
    assert started["runtime_ownership"] == "owned"
    record = replace(
        manager.get("worker"), lifecycle="launch_failed",
        custom_process_identity=None, pane_reported_by_agentctl=False,
        error="transient process-info response",
    )
    manager._save(record)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], agent=None, status="unknown")

    with pytest.raises(AgentDeliveryError, match="replaced"):
        manager.recover_start(
            "worker", expected_token="wrong-generation",
            expected_pid=fake.custom_identity.pid,
        )
    with pytest.raises(HerdrUnavailable, match="did not match"):
        manager.recover_start(
            "worker", expected_token=str(started["token"]), expected_pid=201,
        )

    fake.custom_dies_during_recovery_commit = True
    with pytest.raises(AgentDeliveryError, match="after identity persistence"):
        manager.recover_start(
            "worker", expected_token=str(started["token"]),
            expected_pid=fake.custom_identity.pid,
        )
    partially_recovered = manager.get("worker")
    assert partially_recovered.lifecycle == "launch_failed"
    assert partially_recovered.custom_process_identity == fake.custom_identity
    assert partially_recovered.pane_reported_by_agentctl is False

    fake.custom_running = True
    # A launch that predated the tagged schema recorded the exact observed
    # process but not the executable-intent fields.  Its process identity is
    # sufficient authority for the image; arguments reconstruct argv.
    legacy = partially_recovered.to_document()
    for field in (
        "launch_profile", "launch_executable", "launch_executable_device",
        "launch_executable_inode", "launch_argv", "launch_environment_names",
        "runtime_ownership", "runner_pid", "runner_started_at", "runner_identity",
    ):
        legacy.pop(field, None)
    legacy["schema"] = 1
    path = manager.registry / "worker" / "agent.json"
    path.write_text(json.dumps(legacy) + "\n", encoding="utf-8")

    recovered = manager.recover_start(
        "worker", expected_token=str(started["token"]),
        expected_pid=fake.custom_identity.pid,
    )
    assert recovered["lifecycle"] == "running"
    assert recovered["probe_error"] is None
    recovered_identity = cast(dict[str, object], recovered["custom_process_identity"])
    assert recovered_identity["pid"] == fake.custom_identity.pid
    assert manager.get("worker").error is None
    assert manager.get("worker").pane_reported_by_agentctl is False
    assert fake.submitted == []


def test_custom_process_identity_outweighs_a_missing_native_agent_label(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start("worker", cwd=str(tmp_path), harness="muse")
    fake.infos["w1:p1"] = replace(
        fake.infos["w1:p1"], agent=None, status="unknown",
    )

    status = manager.status("worker")

    assert started["custom_process_identity"] == status["custom_process_identity"]
    assert status["agent"] == "muse"
    assert status["agent_status"] == "idle"
    assert status["probe_error"] is None


def test_headerless_muse_idle_requires_matching_terminal_status(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    screen = (
        "old transcript after the version header scrolled away\n"
        "────────────────\n❯\n────────────────\n"
        "kiki · xhigh · /work · YOLO\n"
    )
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: screen)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")
    assert manager.status("worker")["agent_status"] == "idle"

    buffered = screen.replace("❯\n", "❯ queued owner prompt\n")
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: buffered)
    assert manager.status("worker")["agent_status"] == "staged"

    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="working")
    assert manager.status("worker")["agent_status"] == "working"


def test_status_does_not_call_claude_idle_while_background_agent_is_working(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="claude")
    divider = "─" * 40
    screen = (
        "✻ Waiting for 1 background agent to finish\n"
        f"{divider}\n❯\n{divider}\n"
        "auto mode on · ← 2 agents · ↓ to manage\n"
        "● main\n◯ reviewer Checking tests 8m\n"
    )
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: screen)

    status = manager.status("worker")

    assert status["agent_status"] == "working"
    assert status["probe_error"] is None


def test_failed_muse_recovery_does_not_report_idle_without_idle_composer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    started = manager.start("worker", cwd=str(tmp_path), harness="muse")
    record = replace(
        manager.get("worker"), lifecycle="launch_failed",
        custom_process_identity=None, pane_reported_by_agentctl=False,
        session_agent=None, session_value=None,
        error="transient process-info response",
    )
    manager._save(record)
    fake.infos["w1:p1"] = replace(
        fake.infos["w1:p1"], agent=None, status="unknown",
        session_agent=None, session_value=None,
    )
    fake.custom_ready = False
    reports: list[tuple[str, str, str]] = []

    def record_report(pane_id: str, kind: str, state: str) -> None:
        reports.append((pane_id, kind, state))

    monkeypatch.setattr(fake, "report_pane_agent", record_report)
    with pytest.raises(AgentDeliveryError, match="no verified idle composer"):
        manager.recover_start(
            "worker", expected_token=str(started["token"]),
            expected_pid=fake.custom_identity.pid,
        )

    partial = manager.get("worker")
    assert partial.lifecycle == "launch_failed"
    assert partial.pane_reported_by_agentctl is False
    assert partial.custom_process_identity == fake.custom_identity
    assert "no verified idle composer" in str(partial.error)
    assert reports == []
    assert fake.submitted == []


def test_unreported_starting_muse_without_identity_is_not_assumed_to_own_idle_shell(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    path = manager.registry / "worker" / "agent.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["lifecycle"] = "starting"
    document["pane_reported_by_agentctl"] = False
    document["custom_process_identity"] = None
    path.write_text(json.dumps(document), encoding="utf-8")
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], agent=None)

    with pytest.raises(HerdrUnavailable, match="cannot prove starting"):
        manager.stop("worker")
    assert fake.closed == []
    assert path.exists()


def test_failed_muse_launch_cleanup_refuses_a_replacement_report(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.custom_dies_after_report = True
    with pytest.raises(AgentDeliveryError):
        manager.start("failed", cwd=str(tmp_path), harness="muse")
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], agent="codex")
    with pytest.raises(HerdrUnavailable, match="foreground process"):
        manager.stop("failed")
    assert fake.closed == []


def test_failed_muse_launch_cleanup_refuses_non_shell_foreground_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.custom_dies_after_report = True
    with pytest.raises(AgentDeliveryError):
        manager.start("failed", cwd=str(tmp_path), harness="muse")
    fake.custom_at_idle_shell = False
    with pytest.raises(HerdrUnavailable, match="foreground process"):
        manager.stop("failed")
    assert fake.closed == []


def test_busy_delivery_stays_pending_and_can_drain(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="working")
    with pytest.raises(AgentPending):
        manager.send("worker", "next turn", ready_timeout=0)
    assert fake.submitted == []
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")
    assert len(manager.drain("worker").delivered) == 1
    assert fake.submitted == ["next turn"]


def test_lost_native_status_event_reconciles_without_reinjecting_prompt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="claude")
    fake.wait_fails = True
    fake.working_after_prompt = True

    result = manager.send("worker", "review this exact change")

    assert result.outcome == "delivered"
    assert fake.submitted == ["review this exact change"]
    queue = manager.registry / "worker" / "queue"
    assert not list((queue / "failed").iterdir())
    assert len(list((queue / "processed").iterdir())) == 1


def test_session_replacement_and_extra_panes_refuse_delivery_or_teardown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original = fake.infos["w1:p1"]
    fake.infos["w1:p1"] = replace(original, session_value="another-session")
    with pytest.raises(AgentDeliveryError, match="native session identity changed"):
        manager.send("worker", "must not reach replacement")
    fake.infos["w1:p1"] = original
    fake.presentations.append(Pane("w1:p2", "w1:t1", "w1"))
    with pytest.raises(AgentDeliveryError, match="ownership changed"):
        manager.stop("worker")
    assert fake.submitted == fake.closed == []


def test_unreachable_server_preserves_state_and_stop_fails(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    fake.offline = True
    row = manager.list()[0]
    assert row["agent_status"] == "unknown"
    assert "server unavailable" in str(row["probe_error"])
    with pytest.raises(HerdrUnavailable):
        manager.stop("worker")
    assert manager.get("worker").lifecycle == "running"


def test_stop_preserves_queue_and_output_and_allows_name_reuse(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    original = manager.start("worker", cwd=str(tmp_path))
    manager.send("worker", "literal\nmultiline")
    assert manager.read("worker") == "human and coordinator transcript\n"
    result = manager.stop("worker")
    archive = Path(str(result["archive"]))
    assert (archive / "agent.json").exists()
    assert list((archive / "queue" / "processed").iterdir())
    assert "human" in (archive / "output.json").read_text()
    assert fake.workspace == "w1"
    assert manager.list() == []
    fresh = manager.start("worker", cwd=str(tmp_path))
    assert fresh["token"] != original["token"]
    assert fresh["pane_id"] != original["pane_id"]


def test_confirmed_missing_pane_can_be_archived(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    fake.presentations.clear()
    assert manager.stop("worker")["tab_closed"] is False
    assert fake.closed == []


def test_goal_is_native_submission_with_honest_requested_metadata(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    result = manager.goal("worker", "finish the task")
    assert fake.submitted == ["/goal finish the task"]
    assert result["native_status"] == "active"
    assert result["source"] == "native"
    assert manager.goal("worker")["goal"] == "finish the task"
    with pytest.raises(AgentDeliveryError, match="single line"):
        manager.goal("worker", "first\nsecond")


@pytest.mark.parametrize("tamper", ("kind", "text"))
def test_goal_delivery_requires_the_exact_tagged_queue_artifact(
    tamper: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    manager.goal("worker", "finish the task")
    record_path = manager.registry / "worker" / "agent.json"
    stored = json.loads(record_path.read_text(encoding="utf-8"))
    identifier = stored["goal"]["message_id"]
    artifact_path = (
        manager.registry / "worker" / "queue" / "processed" / f"{identifier}.json"
    )
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    artifact[tamper] = "message" if tamper == "kind" else "/goal forged task"
    artifact_path.write_text(json.dumps(artifact), encoding="utf-8")

    with pytest.raises(AgentDeliveryError, match="disagrees with its session record"):
        manager.goal("worker")


def test_collapsed_muse_paste_withholds_enter_then_reconciles_exact_user_turn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    prompt = "long literal goal " + ("middle " * 180) + "suffix"
    identifier = "collapsed-goal"
    pasted = False
    keys: list[str] = []
    header = "Muse Code 1.4.0\n"
    divider = "────────────────\n"
    footer = "kiki · xhigh · /work · YOLO\n"

    def send_text(_pane: str, text: str) -> None:
        nonlocal pasted
        assert prompt in text
        pasted = True

    def read(_pane: str, *, source: str, lines: int) -> str:
        assert lines > 0
        if transcript:
            return transcript
        composer = (
            f"❯ [Pasted Content {len(prompt)} chars]" if pasted else "❯"
        )
        return header + divider + composer + "\n" + divider + footer

    transcript = ""
    monkeypatch.setattr(fake, "send_text", send_text, raising=False)
    monkeypatch.setattr(fake, "send_keys", lambda _pane, key: keys.append(key))
    monkeypatch.setattr(fake, "read", read)
    with pytest.raises(AgentPossiblySubmitted) as raised:
        manager.send(
            "worker", prompt, message_id=identifier,
            ready_timeout=0, working_timeout=0,
        )
    assert raised.value.message_id == identifier
    assert keys == []
    failed = manager.registry / "worker/queue/failed" / f"{identifier}.json"
    artifact = failed.read_bytes()
    digest = hashlib.sha256(artifact).hexdigest()

    # Same-length placeholder and an assistant echo are never sufficient.
    transcript = header + f"◆ {prompt}\n" + divider + "❯\n" + divider + footer
    with pytest.raises(AgentDeliveryError, match="exact prompt as a Muse user turn"):
        manager.reconcile_delivery("worker", identifier, digest)
    with pytest.raises(AgentDeliveryError, match="expected-sha256"):
        manager.reconcile_delivery("worker", identifier, "0" * 64)

    # A duplicate copy still in the composer is not accepted as submitted.
    transcript = (
        header + f"❯ {prompt}\n◆ Working\n" + divider
        + f"❯ {prompt}\n" + divider + footer
    )
    with pytest.raises(AgentDeliveryError, match="exact prompt as a Muse user turn"):
        manager.reconcile_delivery("worker", identifier, digest)

    transcript = header + f"❯ {prompt}\n◆ Working\n" + divider + "❯\n" + divider + footer
    sidecar = Path(str(failed) + ".error")
    sidecar.unlink()
    sidecar.mkdir()
    with pytest.raises(AgentDeliveryError, match="reconciled delivery sidecar"):
        manager.reconcile_delivery("worker", identifier, digest)
    processed = manager.registry / "worker/queue/processed" / f"{identifier}.json"
    assert processed.read_bytes() == artifact
    assert not failed.exists()
    sidecar.rmdir()
    # A retry after the committed rename is idempotent and does not request
    # new evidence or send terminal input.
    result = manager.reconcile_delivery("worker", identifier, digest)
    assert result.outcome == "delivered"
    assert result.delivered == (identifier,)
    assert not sidecar.exists()
    assert keys == []


def test_long_lived_muse_uses_unwrapped_editor_evidence_before_enter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    prompt = (
        "require the full deterministic-scheduling-review skill and preserve "
        "the exact wrapped message"
    )
    divider = "────────────────\n"
    footer = "kiki · xhigh · /work · YOLO\n"
    pasted = False
    entered = False
    sources: list[str] = []

    def send_text(_pane: str, text: str) -> None:
        nonlocal pasted
        assert prompt in text
        pasted = True

    def send_keys(_pane: str, key: str) -> None:
        nonlocal entered
        assert key == "Enter"
        entered = True

    def read(_pane: str, *, source: str, lines: int) -> str:
        sources.append(source)
        assert lines > 0
        if source == "visible":
            # The physical viewport wraps inside the hyphenated word.  Status
            # uses it only for the current Muse UI; prompt equality must not.
            editor = (
                "❯ require the full deterministic-\n"
                "  scheduling-review skill and preserve the exact wrapped message"
                if pasted and not entered else "❯"
            )
            return "Muse Code 1.4.0\n" + divider + editor + "\n" + divider + footer
        assert source == "recent-unwrapped"
        if entered:
            return f"❯ {prompt}\n◆ Working\n{divider}❯\n{divider}{footer}"
        editor = f"❯ {prompt}" if pasted else "❯"
        return f"old transcript\n{divider}{editor}\n{divider}{footer}"

    monkeypatch.setattr(fake, "send_text", send_text, raising=False)
    monkeypatch.setattr(fake, "send_keys", send_keys)
    monkeypatch.setattr(fake, "read", read)
    result = manager.send(
        "worker", prompt, message_id="wrapped-goal",
        ready_timeout=0, working_timeout=1,
    )
    assert result.outcome == "delivered"
    assert entered
    assert "recent-unwrapped" in sources


def test_muse_submits_matching_prebuffered_prompt_once_without_reinjection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    prompt = (
        "Use deterministic-scheduling-review and preserve the exact staged "
        "owner prompt"
    )
    divider = "─" * 40
    footer = "kiki · xhigh · /work · YOLO\n"
    staged = (
        "old transcript\n"
        f"{divider}\n"
        "❯ Use deterministic-\n"
        "  scheduling-review and preserve the exact staged owner prompt\n"
        f"{divider}\n{footer}"
    )
    entered = False
    paste_calls: list[str] = []
    keys: list[str] = []

    def read(_pane: str, *, source: str, lines: int) -> str:
        assert source in ("visible", "recent-unwrapped") and lines > 0
        if entered:
            return (
                "❯ Use deterministic-\n"
                "  scheduling-review and preserve the exact staged owner prompt\n"
                f"◆ Working\n{divider}\n❯\n{divider}\n{footer}"
            )
        return staged

    def send_keys(_pane: str, key: str) -> None:
        nonlocal entered
        keys.append(key)
        assert key == "Enter" and not entered
        entered = True

    monkeypatch.setattr(fake, "read", read)
    monkeypatch.setattr(
        fake, "send_text", lambda _pane, text: paste_calls.append(text),
        raising=False,
    )
    monkeypatch.setattr(fake, "send_keys", send_keys)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")

    assert manager.status("worker")["agent_status"] == "staged"

    result = manager.send(
        "worker", prompt, message_id="prebuffered",
        ready_timeout=0, working_timeout=1,
    )

    assert result.outcome == "delivered"
    assert paste_calls == []
    assert keys == ["Enter"]


def test_claude_staged_prompt_uses_exact_submit_chord_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="claude")
    prompt = "inspect the exact state and continue safely"
    divider = "─" * 40
    state = "idle"
    keys: list[str] = []
    reads: list[tuple[str, int]] = []

    def screen(_pane: str, *, source: str, lines: int) -> str:
        assert source in ("visible", "recent-unwrapped") and lines > 0
        reads.append((source, lines))
        if state == "staged":
            return (
                f"{divider}\n❯ {prompt}\n"
                "  ctrl+x ctrl+s to send now\n"
                f"{divider}\n⏵⏵ auto mode on · esc to interrupt\n"
            )
        if state == "submitted":
            return (
                f"❯ {prompt}\n● Thinking\n"
                f"{divider}\n❯\n{divider}\n"
                "⏵⏵ auto mode on · esc to interrupt\n"
            )
        return f"{divider}\n❯\n{divider}\nauto mode on\n"

    def stage(pane: str, text: str) -> None:
        nonlocal state
        assert pane == "w1:p1" and text == prompt and state == "idle"
        fake.submitted.append(text)
        state = "staged"

    def submit(pane: str, keys_value: str) -> None:
        nonlocal state
        assert pane == "w1:p1" and keys_value == "ctrl+x ctrl+s"
        assert state == "staged"
        keys.append(keys_value)
        state = "submitted"

    monkeypatch.setattr(fake, "read", screen)
    monkeypatch.setattr(fake, "prompt_agent", stage)
    monkeypatch.setattr(fake, "send_keys", submit)

    delivered = manager.send(
        "worker", prompt, message_id="claude-staged",
        ready_timeout=0, working_timeout=1,
    )

    assert delivered.outcome == "delivered"
    assert fake.submitted == [prompt]
    assert keys == ["ctrl+x ctrl+s"]
    assert ("recent-unwrapped", 5000) in reads


def test_claude_new_active_ui_confirms_submission_when_long_turn_left_history(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="claude")
    prompt = "review the exact long prompt whose rendered turn left bounded history"
    divider = "─" * 40
    state = "idle"
    keys: list[str] = []
    reads: list[tuple[str, int]] = []

    def screen(_pane: str, *, source: str, lines: int) -> str:
        assert source in ("visible", "recent-unwrapped") and lines > 0
        reads.append((source, lines))
        if state == "staged":
            return (
                f"{divider}\n❯ {prompt}\n"
                "  ctrl+x ctrl+s to send now\n"
                f"{divider}\nauto mode on\n"
            )
        if state == "submitted":
            # Herdr still reports idle and the long user turn is no longer in
            # the retained screen, but Claude's UI proves a new active state.
            return (
                "● Inspecting repository\n"
                f"{divider}\n❯\n{divider}\n"
                "⏵⏵ auto mode on · esc to interrupt\n"
            )
        return f"{divider}\n❯\n{divider}\nauto mode on\n"

    def stage(_pane: str, text: str) -> None:
        nonlocal state
        assert text == prompt and state == "idle"
        state = "staged"

    def submit(_pane: str, keys_value: str) -> None:
        nonlocal state
        assert keys_value == "ctrl+x ctrl+s" and state == "staged"
        keys.append(keys_value)
        state = "submitted"

    monkeypatch.setattr(fake, "read", screen)
    monkeypatch.setattr(fake, "prompt_agent", stage)
    monkeypatch.setattr(fake, "send_keys", submit)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="idle")

    delivered = manager.send(
        "worker", prompt, message_id="claude-active-transition",
        ready_timeout=0, working_timeout=1,
    )

    assert delivered.outcome == "delivered"
    assert keys == ["ctrl+x ctrl+s"]
    assert ("recent-unwrapped", 5000) in reads


def test_claude_different_staged_prompt_is_not_overwritten_or_submitted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="claude")
    divider = "─" * 40
    staged = (
        f"{divider}\n❯ existing owner prompt\n"
        "  ctrl+x ctrl+s to send now\n"
        f"{divider}\nauto mode on\n"
    )
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: staged)
    monkeypatch.setattr(
        fake, "prompt_agent",
        lambda *_args: pytest.fail("staged input was overwritten"),
    )
    monkeypatch.setattr(
        fake, "send_keys",
        lambda *_args: pytest.fail("staged input was submitted"),
    )

    assert manager.status("worker")["agent_status"] == "staged"
    with pytest.raises(AgentPossiblySubmitted, match="different buffered input"):
        manager.send(
            "worker", "different queued prompt", message_id="different-staged",
            ready_timeout=0, working_timeout=0,
        )


def test_muse_retries_enter_once_only_while_exact_prompt_remains_staged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    prompt = "continue the exact queued task after the first Enter was ignored"
    divider = "─" * 40
    footer = "kiki · xhigh · /work · YOLO\n"
    keys: list[str] = []

    def read(_pane: str, *, source: str, lines: int) -> str:
        assert source in ("visible", "recent-unwrapped") and lines > 0
        if len(keys) >= 2:
            return f"❯ {prompt}\n◆ Thinking\n{divider}\n❯\n{divider}\n{footer}"
        return f"old transcript\n{divider}\n❯ {prompt}\n{divider}\n{footer}"

    monkeypatch.setattr(fake, "read", read)
    monkeypatch.setattr(
        fake, "send_text", lambda *_args, **_kwargs: pytest.fail("must not repaste"),
        raising=False,
    )
    monkeypatch.setattr(fake, "send_keys", lambda _pane, key: keys.append(key))
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")

    result = manager.send(
        "worker", prompt, message_id="second-enter",
        ready_timeout=0, working_timeout=1,
    )

    assert result.outcome == "delivered"
    assert keys == ["Enter", "Enter"]


def test_muse_refuses_different_prebuffered_prompt_without_terminal_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    divider = "─" * 40
    screen = (
        "old transcript\n"
        f"{divider}\n❯ human draft that must be preserved\n{divider}\n"
        "kiki · xhigh · /work · YOLO\n"
    )
    pasted: list[str] = []
    keys: list[str] = []
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: screen)
    monkeypatch.setattr(
        fake, "send_text", lambda _pane, text: pasted.append(text),
        raising=False,
    )
    monkeypatch.setattr(fake, "send_keys", lambda _pane, key: keys.append(key))
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")

    assert manager.status("worker")["agent_status"] == "staged"
    with pytest.raises(AgentPossiblySubmitted, match="different buffered input"):
        manager.send(
            "worker", "queued automation prompt", message_id="blocked-buffer",
            ready_timeout=0, working_timeout=0,
        )
    assert pasted == []
    assert keys == []


def test_muse_rechecks_exact_process_after_prebuffered_prompt_before_enter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    prompt = "staged prompt awaiting exact process recheck"
    divider = "─" * 40
    screen = (
        f"old transcript\n{divider}\n❯ {prompt}\n{divider}\n"
        "kiki · xhigh · /work · YOLO\n"
    )
    recent_reads = 0
    pasted: list[str] = []
    keys: list[str] = []

    def read(_pane: str, *, source: str, lines: int) -> str:
        nonlocal recent_reads
        assert lines > 0
        if source == "recent-unwrapped":
            recent_reads += 1
            if recent_reads == 2:
                fake.custom_running = False
        return screen

    monkeypatch.setattr(fake, "read", read)
    monkeypatch.setattr(
        fake, "send_text", lambda _pane, text: pasted.append(text),
        raising=False,
    )
    monkeypatch.setattr(fake, "send_keys", lambda _pane, key: keys.append(key))
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="done")

    with pytest.raises(AgentPossiblySubmitted, match="foreground process"):
        manager.send(
            "worker", prompt, message_id="identity-recheck",
            ready_timeout=0, working_timeout=0,
        )
    assert recent_reads == 2
    assert pasted == []
    assert keys == []


def test_wait_reports_readiness_and_blocked_prompt(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    assert manager.wait("worker", timeout=0)["agent_status"] == "idle"
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="working")
    with pytest.raises(AgentDeliveryError, match="timed out"):
        manager.wait("worker", timeout=0)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="blocked")
    with pytest.raises(AgentDeliveryError, match="requires attention"):
        manager.wait("worker")


def test_private_state_and_malformed_record_are_rejected(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    record = manager.registry / "worker" / "agent.json"
    document = json.loads(record.read_text())
    document["schema"] = True
    record.write_text(json.dumps(document))
    with pytest.raises(AgentDeliveryError, match="invalid agent record"):
        manager.get("worker")
    record.chmod(0o644)
    with pytest.raises(AgentDeliveryError, match="not private"):
        manager.get("worker")


def test_agent_record_v3_has_one_tagged_launch_and_goal_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    started = manager.start(
        "worker", cwd=str(tmp_path), harness="codex", model="model-a",
        harness_args=("--literal",),
    )
    path = manager.registry / "worker" / "agent.json"
    stored = json.loads(path.read_text(encoding="utf-8"))
    assert stored["schema"] == "agentctl-session/v3"
    assert stored["launch"]["schema"] == "agentctl-launch/v2"
    assert stored["launch"]["permission_mode"] is None
    assert stored["launch"]["argv"] == [
        "codex", "--no-alt-screen", "--model", "model-a", "--literal",
    ]
    assert stored["goal"] == {
        "schema": "agentctl-goal/v1", "objective": None,
        "message_id": None, "native_command": None,
    }
    assert stored["native_session"] == {
        "schema": "agentctl-native-session/v1", "agent": "codex",
        "value": "session-1", "source": "observed",
    }
    for duplicate in (
        "harness", "cwd", "adapter", "mode", "backend", "model", "resume",
        "arguments", "launch_argv", "launch_executable", "runtime_home",
        "runtime_ownership", "runner_pid", "runner_started_at",
        "session_agent", "session_value", "goal_delivery", "goal_session_id",
        "goal_command", "goal_messages", "goal_message_id",
    ):
        assert duplicate not in stored
    assert manager.get("worker").arguments == started["arguments"]


def test_agent_record_v3_reads_profiled_claude_launch_without_flat_harness_field(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start(
        "worker", cwd=str(tmp_path), harness="claude", model="opus",
        launch_profile="claude-opus-55",
    )

    stored = json.loads(
        (manager.registry / "worker" / "agent.json").read_text(encoding="utf-8")
    )
    assert "harness" not in stored
    assert stored["launch"] == {
        "adapter": "herdr",
        "argv": ["claude", "--model", "opus"],
        "backend": "herdr",
        "cwd": str(tmp_path),
        "environment_names": [],
        "executable": None,
        "harness": "claude",
        "mode": "interactive",
        "model": "opus",
        "profile": "claude-opus-55",
        "permission_mode": None,
        "resume": None,
        "runtime_home": None,
        "runtime_ownership": "owned",
        "schema": "agentctl-launch/v2",
    }
    loaded = manager.get("worker")
    assert loaded.launch.harness == "claude"
    assert loaded.launch.profile == "claude-opus-55"


def test_agent_record_v3_rejects_duplicate_nested_launch_keys(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    encoded = path.read_text(encoding="utf-8")
    needle = '"harness": "codex"'
    assert encoded.count(needle) == 1
    path.write_text(
        encoded.replace(needle, '"harness": "codex", "harness": "muse"'),
        encoding="utf-8",
    )

    with pytest.raises(AgentDeliveryError, match="duplicate JSON object key"):
        manager.get("worker")


def test_agent_record_v3_writer_refuses_incomplete_native_session(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    record = manager.get("worker")
    record.session_value = None
    record.session_source = None

    with pytest.raises(AgentDeliveryError, match="incomplete native session identity"):
        record.to_storage_document()


def _downgrade_current_record_to_v2(stored: dict[str, object]) -> dict[str, object]:
    goal = cast(dict[str, object], stored.pop("goal"))
    native = cast(dict[str, object] | None, stored.pop("native_session"))
    stored["schema"] = "agentctl-session/v2"
    stored.update({
        "session_agent": None if native is None else native["agent"],
        "session_value": None if native is None else native["value"],
        "goal": goal["objective"],
        "goal_delivery": None,
        "goal_session_id": None if native is None else native["value"],
        "goal_command": goal["native_command"],
        "goal_messages": {},
        "goal_message_id": goal["message_id"],
    })
    return stored


def test_agent_record_v3_migrates_legacy_launch_spec_at_write_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="claude")
    path = manager.registry / "worker" / "agent.json"
    stored = json.loads(path.read_text(encoding="utf-8"))
    stored["launch"]["schema"] = "agentctl-launch/v1"
    stored["launch"].pop("permission_mode")
    path.write_text(json.dumps(stored), encoding="utf-8")

    loaded = manager.get("worker")
    assert loaded.launch.permission_mode is None
    manager.pause("worker")
    migrated = json.loads(path.read_text(encoding="utf-8"))
    assert migrated["launch"]["schema"] == "agentctl-launch/v2"
    assert migrated["launch"]["permission_mode"] is None


def test_agent_record_v3_migrates_v2_goal_and_native_session_without_duplicates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    stored = _downgrade_current_record_to_v2(
        json.loads(path.read_text(encoding="utf-8"))
    )
    stored.update({
        "goal": "legacy objective",
        "goal_delivery": "delivered",
        "goal_command": ["codex", "app-server", "proxy"],
    })
    path.write_text(json.dumps(stored), encoding="utf-8")

    assert manager.get("worker").goal == "legacy objective"
    manager.pause("worker")
    migrated = json.loads(path.read_text(encoding="utf-8"))
    assert migrated["schema"] == "agentctl-session/v3"
    assert migrated["goal"] == {
        "schema": "agentctl-goal/v1",
        "objective": "legacy objective",
        "message_id": None,
        "native_command": ["codex", "app-server", "proxy"],
    }
    assert migrated["native_session"]["value"] == "session-1"
    for duplicate in (
        "session_agent", "session_value", "goal_delivery", "goal_session_id",
        "goal_command", "goal_messages", "goal_message_id",
    ):
        assert duplicate not in migrated


def test_agent_record_v2_refuses_conflicting_native_session_authorities(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    stored = _downgrade_current_record_to_v2(
        json.loads(path.read_text(encoding="utf-8"))
    )
    stored["goal_session_id"] = "different-session"
    path.write_text(json.dumps(stored), encoding="utf-8")

    with pytest.raises(AgentDeliveryError, match="contradictory native session"):
        manager.get("worker")


def test_agent_record_v3_refuses_native_session_for_another_harness(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    stored = json.loads(path.read_text(encoding="utf-8"))
    stored["native_session"]["agent"] = "claude"
    path.write_text(json.dumps(stored), encoding="utf-8")

    with pytest.raises(AgentDeliveryError, match="native session harness mismatch"):
        manager.get("worker")


@pytest.mark.parametrize("harness,prompt", (
    ("codex", "/goal legacy objective"),
    ("claude", "Your ongoing goal: legacy objective\nWork toward this goal and report completion or blockers."),
))
def test_agent_record_v3_migrates_legacy_goal_artifact_before_dropping_map(
    harness: str, prompt: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness=harness)
    queue = manager.registry / "worker" / "queue"
    identifier = agent.enqueue(
        str(queue), prompt, message_id="legacy-goal",
    )
    artifact_path = queue / "inbox" / f"{identifier}.json"
    legacy_artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    legacy_artifact.pop("kind")
    artifact_path.write_text(json.dumps(legacy_artifact), encoding="utf-8")
    path = manager.registry / "worker" / "agent.json"
    stored = _downgrade_current_record_to_v2(
        json.loads(path.read_text(encoding="utf-8"))
    )
    stored["goal"] = "legacy objective"
    stored["goal_message_id"] = identifier
    stored["goal_messages"] = {identifier: "legacy objective"}
    path.write_text(json.dumps(stored), encoding="utf-8")

    manager.pause("worker")

    migrated = json.loads(path.read_text(encoding="utf-8"))
    assert migrated["schema"] == "agentctl-session/v3"
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    assert artifact["kind"] == "goal"
    assert "goal_messages" not in migrated


@pytest.mark.parametrize("legacy_schema", ("flat-v1", "session-v2"))
def test_legacy_goal_pointer_without_duplicate_map_is_exactly_recovered(
    legacy_schema: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    objective = "recover the retained timerfd worktree"
    prompt = (
        f"Your ongoing goal: {objective}\n"
        "Work toward this goal and report completion or blockers."
    )
    queue = manager.registry / "worker" / "queue"
    identifier = agent.enqueue(queue, prompt, message_id="legacy-goal")
    artifact_path = queue / "inbox" / f"{identifier}.json"
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    artifact.pop("kind")
    artifact_path.write_text(json.dumps(artifact), encoding="utf-8")
    path = manager.registry / "worker" / "agent.json"
    if legacy_schema == "session-v2":
        stored = _downgrade_current_record_to_v2(
            json.loads(path.read_text(encoding="utf-8"))
        )
    else:
        stored = manager.get("worker").to_document()
    stored["goal"] = objective
    stored["goal_delivery"] = "possibly_submitted"
    stored["goal_message_id"] = identifier
    stored["goal_messages"] = {}
    path.write_text(json.dumps(stored), encoding="utf-8")

    assert manager.status("worker")["goal_delivery"] == "pending"
    manager.pause("worker")

    migrated = json.loads(path.read_text(encoding="utf-8"))
    assert migrated["schema"] == "agentctl-session/v3"
    assert json.loads(artifact_path.read_text(encoding="utf-8"))["kind"] == "goal"


@pytest.mark.parametrize("mutation", ("id", "text"))
def test_legacy_goal_pointer_without_map_refuses_mismatched_artifact(
    mutation: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    objective = "recover the retained timerfd worktree"
    prompt = (
        f"Your ongoing goal: {objective}\n"
        "Work toward this goal and report completion or blockers."
    )
    queue = manager.registry / "worker" / "queue"
    identifier = agent.enqueue(queue, prompt, message_id="legacy-goal")
    artifact_path = queue / "inbox" / f"{identifier}.json"
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    artifact.pop("kind")
    artifact[mutation] = "wrong"
    artifact_path.write_text(json.dumps(artifact), encoding="utf-8")
    path = manager.registry / "worker" / "agent.json"
    stored = _downgrade_current_record_to_v2(
        json.loads(path.read_text(encoding="utf-8"))
    )
    stored["goal"] = objective
    stored["goal_delivery"] = "possibly_submitted"
    stored["goal_message_id"] = identifier
    stored["goal_messages"] = {}
    path.write_text(json.dumps(stored), encoding="utf-8")

    with pytest.raises(AgentDeliveryError):
        manager.status("worker")
    with pytest.raises(AgentDeliveryError):
        manager.pause("worker")


def test_current_goal_pointer_never_accepts_an_untagged_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    queue = manager.registry / "worker" / "queue"
    identifier = agent.enqueue(queue, "/goal exact", message_id="current-goal")
    artifact_path = queue / "inbox" / f"{identifier}.json"
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    artifact.pop("kind")
    artifact_path.write_text(json.dumps(artifact), encoding="utf-8")
    record = manager.get("worker")
    record.goal = "exact"
    record.goal_message_id = identifier
    manager._save(record)

    with pytest.raises(AgentDeliveryError, match="disagrees with its session record"):
        manager.status("worker")


def test_send_migrates_legacy_goal_authority_before_queue_delivery(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    queue = manager.registry / "worker" / "queue"
    identifier = agent.enqueue(
        str(queue), "/goal legacy objective", message_id="legacy-goal",
    )
    artifact_path = queue / "inbox" / f"{identifier}.json"
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    artifact.pop("kind")
    artifact_path.write_text(json.dumps(artifact), encoding="utf-8")
    record_path = manager.registry / "worker" / "agent.json"
    stored = _downgrade_current_record_to_v2(
        json.loads(record_path.read_text(encoding="utf-8"))
    )
    stored["goal"] = "legacy objective"
    stored["goal_message_id"] = identifier
    stored["goal_messages"] = {identifier: "legacy objective"}
    record_path.write_text(json.dumps(stored), encoding="utf-8")

    def observe_send(*_args: object, **_kwargs: object) -> agent.QueueResult:
        migrated = json.loads(record_path.read_text(encoding="utf-8"))
        tagged = json.loads(artifact_path.read_text(encoding="utf-8"))
        assert migrated["schema"] == "agentctl-session/v3"
        assert "goal_messages" not in migrated
        assert tagged["kind"] == "goal"
        return agent.QueueResult("ordinary", (), (), ())

    monkeypatch.setattr(agent, "send", observe_send)
    manager.send("worker", "ordinary message")


def test_agent_record_v3_refuses_to_discard_missing_legacy_goal_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    agent.enqueue(
        str(manager.registry / "worker" / "queue"),
        "ordinary message", message_id="ordinary",
    )
    stored = _downgrade_current_record_to_v2(
        json.loads(path.read_text(encoding="utf-8"))
    )
    stored["goal"] = "missing objective"
    stored["goal_message_id"] = "missing-goal"
    stored["goal_messages"] = {"missing-goal": "missing objective"}
    encoded = json.dumps(stored).encode()
    path.write_bytes(encoded)

    with pytest.raises(AgentDeliveryError, match="refusing to discard"):
        manager.pause("worker")
    assert path.read_bytes() == encoded


@pytest.mark.parametrize("collision,value", (
    ("launch_argv", ["replacement"]), ("launch_profile", "replacement"),
    ("argv", ["replacement"]), ("profile", "replacement"),
    ("adapter", "herdr"), ("arguments", ["replacement"]),
    ("runner_pid", 123), ("runner_started_at", "456"),
    ("permission_mode", "bypass"), ("launch_permission_mode", "bypass"),
    ("launch", {}), ("native_session", {}), ("extensions", {}),
    ("goal_delivery", "delivered"),
    ("goal_messages", {"legacy": "objective"}),
    ("goal_session_id", "replacement"),
))
def test_agent_record_v3_extensions_cannot_shadow_launch_authority(
    collision: str, value: object, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = manager.registry / "worker" / "agent.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["extensions"][collision] = value
    path.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(AgentDeliveryError, match="invalid agent record extensions"):
        manager.get("worker")


def test_agent_record_v1_migrates_once_and_refuses_conflicting_argument_authorities(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    legacy = manager.get("worker").to_document()
    legacy["launch_argv"] = []
    legacy["arguments"] = ["--no-alt-screen", "--literal"]
    path.write_text(json.dumps(legacy), encoding="utf-8")
    assert manager.get("worker").launch.argv == (
        "codex", "--no-alt-screen", "--literal",
    )
    manager.pause("worker")
    migrated = json.loads(path.read_text(encoding="utf-8"))
    assert migrated["schema"] == "agentctl-session/v3"
    assert migrated["launch"]["argv"] == [
        "codex", "--no-alt-screen", "--literal",
    ]

    conflicting = manager.get("worker").to_document()
    conflicting["arguments"] = ["--different"]
    path.write_text(json.dumps(conflicting), encoding="utf-8")
    with pytest.raises(AgentDeliveryError, match="contradictory launch arguments"):
        manager.get("worker")


@pytest.mark.parametrize("field", ("launch", "native_session", "extensions"))
def test_agent_record_v1_rejects_reserved_current_schema_fields(
    field: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="codex")
    path = manager.registry / "worker" / "agent.json"
    legacy = manager.get("worker").to_document()
    legacy[field] = {}
    path.write_text(json.dumps(legacy), encoding="utf-8")

    with pytest.raises(AgentDeliveryError, match="reserved current-schema field"):
        manager.get("worker")


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("adapter", "turn-runner"),
        ("mode", "headless"),
        ("backend", "tmux"),
        ("runtime_ownership", "foreign"),
    ],
)
def test_agent_record_v2_refuses_crossed_launch_dimensions(
    field: str, value: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = manager.registry / "worker" / "agent.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["launch"][field] = value
    path.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(AgentDeliveryError, match="inconsistent"):
        manager.get("worker")


def test_malformed_custom_process_identities_are_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path), harness="muse")
    path = manager.registry / "worker" / "agent.json"
    original = json.loads(path.read_text(encoding="utf-8"))
    identity = original["custom_process_identity"]
    assert isinstance(identity, dict)

    variants: list[dict[str, object]] = []
    for field, value in (
        ("version", True),
        ("pid", True),
        ("pid", 2_147_483_648),
        ("starttime_ticks", 1 << 64),
        ("executable_device", 0),
        ("executable_device", 1 << 64),
        ("executable_inode", 1 << 64),
        ("boot_id", "NOT-A-BOOT-ID"),
    ):
        variant = json.loads(json.dumps(original))
        variant["custom_process_identity"][field] = value
        variants.append(variant)
    missing = json.loads(json.dumps(original))
    del missing["custom_process_identity"]["starttime_ticks"]
    variants.append(missing)
    unknown = json.loads(json.dumps(original))
    unknown["custom_process_identity"]["unexpected"] = 1
    variants.append(unknown)
    for record_field, record_value in (
        ("adapter", "herdr"),
        ("harness", "codex"),
        ("pane_id", None),
    ):
        variant = json.loads(json.dumps(original))
        if record_field in ("adapter", "harness"):
            variant["launch"][record_field] = record_value
        else:
            variant[record_field] = record_value
        variants.append(variant)

    for document in variants:
        path.write_text(json.dumps(document), encoding="utf-8")
        with pytest.raises(AgentDeliveryError, match="custom process identity|invalid agent record"):
            manager.get("worker")


@pytest.mark.parametrize("name", ["../outside", "archive", "Has-Capitals", "under_score", "0worker", "a" * 33])
def test_bad_names_do_not_allocate_state(name: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="agent name"):
        manager.start(name, cwd=str(tmp_path))
    assert not manager.registry.exists()


def test_cli_managed_start_and_message_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    monkeypatch.setattr(cli, "HerdrClient", lambda **_kwargs: cast(HerdrClient, fake))
    root = ["--registry", str(manager.registry)]
    assert cli.main(["start", "worker", "--cwd", str(tmp_path), *root]) == 0
    capsys.readouterr()
    message = tmp_path / "brief.txt"
    message.write_text("line one\nline two")
    assert cli.main(["send", "--name", "worker", "--file", str(message), *root]) == 0
    assert fake.submitted == ["line one\nline two"]
    assert cli.main(["stop", "worker", *root]) == 0


def test_cli_parses_exact_delivery_reconciliation_authority() -> None:
    parsed = unified_cli.parser().parse_args([
        "reconcile-delivery", "worker", "message-1",
        "--expected-sha256", "0123456789abcdef" * 4,
    ])
    assert parsed.command == "reconcile-delivery"
    assert parsed.name == "worker"
    assert parsed.message_id == "message-1"
    assert parsed.expected_sha256 == "0123456789abcdef" * 4


def test_trust_prompt_is_not_a_composer_even_when_herdr_reports_idle(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    trust = "Quick safety check: Is this a project you created or one you trust?\n❯ No, exit\nYes, I trust this folder"
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: trust)
    with pytest.raises(AgentDeliveryError, match="trust prompt requires human attention"):
        manager.start("worker", cwd=str(tmp_path), harness="claude", brief="must not confirm the menu")
    assert fake.submitted == []
    assert manager.get("worker").lifecycle == "launch_failed"


def test_replaced_named_agent_is_not_retargeted_in_same_pane(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    monkeypatch.setattr(fake, "agent_pane", lambda _name: "w1:p-other")
    with pytest.raises(AgentPending, match="no longer owns"):
        manager.send("worker", "do not send to replacement", ready_timeout=0)
    with pytest.raises(HerdrUnavailable, match="no longer owns"):
        manager.stop("worker")
    assert fake.submitted == fake.closed == []


def test_explicit_session_binding_preserves_existing_queue_authority(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    record = manager.get("worker")
    record.session_agent = record.session_value = None
    record.session_source = None
    manager._save(record)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], session_agent=None, session_value=None)
    manager.send("worker", "before native binding")
    binding = manager.registry / "worker" / "queue" / "target.json"
    original = binding.read_bytes()
    manager.bind_session("worker", "explicit-session", goal_command=("codex", "app-server", "--stdio"))
    assert binding.read_bytes() == original
    manager.send("worker", "after native binding")
    assert fake.submitted == ["before native binding", "after native binding"]
    with pytest.raises(AgentDeliveryError, match="already bound"):
        manager.bind_session("worker", "different-session")
    assert manager.goal("worker")["native_status"] == "absent"


@pytest.mark.parametrize("correct_objective", [True, False])
def test_goal_confirms_only_its_exact_replacement_menu(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, correct_objective: bool) -> None:
    from agentctl.errors import AgentPossiblySubmitted
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    keys: list[str] = []
    def wait(_pane: str, _status: str, _timeout: int) -> None:
        if not keys:
            raise HerdrUnavailable("working transition not observed")
    monkeypatch.setattr(fake, "wait_agent_status", wait)
    monkeypatch.setattr(fake, "send_keys", lambda _pane, key: keys.append(key))
    objective = "finish this task" if correct_objective else "a different task"
    screen = f"Replace goal?\nNew objective: {objective}\n› 1. Replace current goal  Set the new objective and start it now\n2. Cancel  Keep the current goal\nPress enter to confirm or esc to go back"
    monkeypatch.setattr(fake, "read", lambda *_args, **_kwargs: screen)
    if correct_objective:
        assert manager.goal("worker", "finish this task")["native_status"] == "active"
        assert keys == ["Enter"]
    else:
        with pytest.raises(AgentPossiblySubmitted):
            manager.goal("worker", "finish this task")
        assert keys == []
    assert fake.submitted == ["/goal finish this task"]


def test_queued_goals_keep_confirmation_authority_across_restart(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from agentctl.errors import AgentPossiblySubmitted
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="working")
    for objective in ("first objective", "second objective"):
        with pytest.raises(AgentPending):
            manager.goal("worker", objective, ready_timeout=0)
    assert fake.submitted == []
    restarted = ManagedAgents(cast(HerdrClient, fake), manager.registry)
    fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], status="idle")
    confirmations: list[str] = []
    def wait(_pane: str, _status: str, _timeout: int) -> None:
        if len(confirmations) < len(fake.submitted):
            raise HerdrUnavailable("working transition not observed")
    def screen(*_args: object, **_kwargs: object) -> str:
        objective = fake.submitted[-1][6:]
        return f"Replace goal?\nNew objective: {objective}\n› 1. Replace current goal  Set the new objective and start it now\n2. Cancel  Keep the current goal\nPress enter to confirm or esc to go back"
    monkeypatch.setattr(fake, "wait_agent_status", wait)
    monkeypatch.setattr(fake, "send_keys", lambda _pane, key: confirmations.append(key))
    monkeypatch.setattr(fake, "read", screen)
    assert len(restarted.drain("worker").delivered) == 2
    assert fake.submitted == ["/goal first objective", "/goal second objective"]
    assert confirmations == ["Enter", "Enter"]
    assert restarted.goal("worker")["delivery"] == "delivered"
    assert restarted.status("worker")["goal_delivery"] == "delivered"
    # Matching text alone does not authorize confirmation for an ordinary send.
    with pytest.raises(AgentPossiblySubmitted):
        restarted.send("worker", "/goal second objective")
    assert confirmations == ["Enter", "Enter"]


def test_goal_operation_identifier_cannot_escape_queue(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = manager.registry / "worker" / "agent.json"
    record = json.loads(path.read_text())
    record["goal"]["message_id"] = "../../outside"
    path.write_text(json.dumps(record))
    with pytest.raises(AgentDeliveryError, match="invalid goal message"):
        manager.goal("worker")


def test_legacy_goal_map_identifier_cannot_escape_queue(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = manager.registry / "worker" / "agent.json"
    record = _downgrade_current_record_to_v2(json.loads(path.read_text()))
    record["goal_messages"] = {"../outside": "task"}
    path.write_text(json.dumps(record))
    with pytest.raises(AgentDeliveryError, match="invalid goal messages"):
        manager.goal("worker")
