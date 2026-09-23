"""Lifecycle regressions for visible subagents, including failures and ownership changes."""
from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from typing import cast

import pytest

from agentctl.client import AgentPaneInfo, CustomProcessIdentity, HerdrClient, Pane
from agentctl.errors import AgentDeliveryError, AgentPending, HerdrUnavailable
from agentctl.subagents import ManagedAgents, environment_entries, harness_arguments
import agentctl.legacy_cli as cli
import agentctl.codex_goal as native_goal


class FakeManagedClient:
    def __init__(self) -> None:
        self.workspace: str | None = None
        self.infos: dict[str, AgentPaneInfo] = {}
        self.presentations: list[Pane] = []
        self.launched: list[tuple[str, str, str, tuple[str, ...]]] = []
        self.environments: list[tuple[str, ...]] = []
        self.closed: list[str] = []
        self.submitted: list[str] = []
        self.serial = 0
        self.offline = False
        self.fail_start = False
        self.custom_dies_after_report = False
        self.custom_fails_after_observation = False
        self.custom_running = False
        self.custom_at_idle_shell = True
        self.custom_identity = CustomProcessIdentity(
            version=1, boot_id="00000000-0000-0000-0000-000000000000",
            pid=200, starttime_ticks=200, executable_device=1, executable_inode=2,
        )

    def workspace_id_for_label(self, label: str) -> str | None:
        assert label == "subagents"
        return self.workspace

    def workspace_label(self, workspace_id: str) -> str:
        assert workspace_id == "w1"
        return "subagents"

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
        self.presentations.append(Pane(pane, tab, workspace_id))
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

    def start_agent(self, name: str, kind: str, pane_id: str, arguments: tuple[str, ...], *, timeout: float) -> None:
        assert timeout > 0
        self.launched.append((name, kind, pane_id, arguments))
        if self.fail_start:
            raise HerdrUnavailable("trust prompt needs attention")
        self.infos[pane_id] = replace(self.infos[pane_id], agent=kind, status="idle", session_agent=kind, session_value=f"session-{self.serial}")

    def start_pane_agent(
        self, name: str, kind: str, pane_id: str, arguments: tuple[str, ...], *, timeout: float,
        on_observed: Callable[[CustomProcessIdentity], None] | None = None,
    ) -> CustomProcessIdentity:
        assert timeout > 0
        self.launched.append((name, kind, pane_id, arguments))
        self.custom_running = True
        if on_observed is not None:
            on_observed(self.custom_identity)
        if self.custom_fails_after_observation:
            raise HerdrUnavailable("trust prompt requires human attention")
        self.infos[pane_id] = replace(
            self.infos[pane_id], agent=kind, status="idle",
        )
        self.custom_running = not self.custom_dies_after_report
        return self.custom_identity

    def verify_custom_harness(
        self, pane_id: str, kind: str,
        expected_identity: CustomProcessIdentity | None = None,
    ) -> None:
        if (self.infos[pane_id].agent not in (None, kind) or not self.custom_running
                or (expected_identity is not None
                    and expected_identity != self.custom_identity)):
            raise HerdrUnavailable("custom harness is not the foreground process")

    def pane_is_idle_shell(self, pane_id: str) -> bool:
        assert pane_id in self.infos
        return self.custom_at_idle_shell

    def agent_pane(self, name: str) -> str:
        matches = [entry[2] for entry in self.launched if entry[0] == name]
        if self.offline or not matches:
            raise HerdrUnavailable("server unavailable or named agent missing")
        return matches[-1]

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        if self.offline:
            raise HerdrUnavailable("server unavailable")
        return self.infos[pane_id]

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        del workspace_id
        if self.offline:
            raise HerdrUnavailable("server unavailable")
        return tuple(self.presentations)

    def prompt_agent(self, pane_id: str, text: str) -> None:
        assert pane_id in self.infos
        self.submitted.append(text)

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        assert pane_id in self.infos and status == "working" and timeout_ms > 0

    def read(self, pane_id: str, *, source: str, lines: int) -> str:
        assert pane_id in self.infos and lines > 0
        return "" if source == "recent-unwrapped" else "human and coordinator transcript\n"

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


def test_session_replacement_and_extra_panes_refuse_mutation(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original = fake.infos["w1:p1"]
    fake.infos["w1:p1"] = replace(original, session_value="another-session")
    with pytest.raises(AgentDeliveryError, match="exactly one live pane"):
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


@pytest.mark.parametrize("field,value", [("goal_message_id", "../../outside"), ("goal_messages", {"../outside": "task"})])
def test_goal_operation_identifiers_cannot_escape_queue(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, field: str, value: object) -> None:
    manager, _fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = manager.registry / "worker" / "agent.json"
    record = json.loads(path.read_text())
    record[field] = value
    path.write_text(json.dumps(record))
    with pytest.raises(AgentDeliveryError, match="invalid goal message"):
        manager.goal("worker")
