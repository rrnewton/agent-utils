"""Identity and ownership checks for adopting already-running Herdr agents."""
from __future__ import annotations

import json
import threading
from dataclasses import replace
from pathlib import Path
from typing import cast

import pytest

from agentctl import cli
from agentctl.client import AgentPaneInfo, HerdrClient
from agentctl.errors import AgentDeliveryError
from agentctl.sessions import Sessions
from agentctl.subagents import AgentRecord
import agentctl.codex_goal as native_goal
from .test_herdr_subagents import FakeManagedClient


def setup_foreign(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, *, native_session: bool = True,
) -> tuple[Sessions, FakeManagedClient, str]:
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    fake.workspace = "w1"
    tab = fake.create_tab(workspace_id="w1", label="foreign", cwd=str(tmp_path))
    pane = fake.presentations[-1].pane_id
    fake.infos[pane] = AgentPaneInfo(
        pane, "w1", str(tmp_path), "codex", "idle",
        "codex" if native_session else None,
        "native-session" if native_session else None,
    )
    sessions = Sessions(cast(HerdrClient, fake), tmp_path / "registry")

    def goal(session: str, _command: object = None) -> dict[str, object] | None:
        assert session == "native-session"
        objectives = [text[6:] for text in fake.submitted if text.startswith("/goal ")]
        return {"status": "active", "objective": objectives[-1]} if objectives else None

    monkeypatch.setattr(native_goal, "get_goal", goal)
    assert tab == fake.presentations[-1].tab_id
    return sessions, fake, pane


def adopt(sessions: Sessions, pane: str, cwd: Path, *, name: str = "foreign") -> dict[str, object]:
    return sessions.adopt(name, pane_id=pane, expected_workspace="subagents",
                          expected_cwd=str(cwd), harness="codex")


def test_adopted_agent_supports_named_operations_and_preserves_native_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    result = adopt(sessions, pane, tmp_path)
    assert result["adapter"] == "herdr-foreign"
    assert result["pane_id"] == pane
    assert result["session_agent"] == "codex"
    assert result["session_value"] == "native-session"
    assert result["capabilities"] == [
        "send", "status", "read", "wait", "stop", "attach", "pause", "resume",
        "terminal-snapshot", "drain", "goal", "bind-session",
    ]
    assert [row["name"] for row in sessions.list()] == ["foreign"]

    sessions.send_session("foreign", "follow up")
    assert fake.submitted == ["follow up"]
    assert sessions.read_session("foreign") == "human and coordinator transcript\n"
    assert sessions.wait("foreign", timeout=0)["agent_status"] == "idle"
    assert sessions.goal("foreign", "finish adopted work")["native_status"] == "active"
    assert sessions.bind_session("foreign", "native-session")["source"] == "explicit"
    assert fake.submitted[-1] == "/goal finish adopted work"

    binding = json.loads((tmp_path / "registry/foreign/queue/target.json").read_text())
    assert binding["kind"] == "session"
    assert binding["agent"] == "codex"
    assert binding["value"] == "native-session"


def test_stop_unregisters_foreign_agent_without_closing_or_mutating_runtime(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    original = adopt(sessions, pane, tmp_path)
    sessions.send_session("foreign", "retained request")
    presentation = fake.presentations[0]

    stopped = sessions.stop("foreign")

    assert stopped["runtime_preserved"] is True
    assert stopped["pane_closed"] is False and stopped["tab_closed"] is False
    assert fake.closed == []
    assert fake.presentations == [presentation]
    assert fake.infos[pane].agent == "codex"
    assert sessions.list() == []
    archive = Path(str(stopped["archive"]))
    saved = json.loads((archive / "agent.json").read_text())
    assert saved["adapter"] == "herdr-foreign"
    assert saved["token"] == original["token"]
    assert (archive / "output.json").is_file()
    assert list((archive / "queue/processed").iterdir())


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ({"agent": None}, "agent is None"),
        ({"agent": "claude"}, "expected 'codex'"),
        ({"workspace": "wrong"}, "workspace is 'subagents'"),
        ({"cwd": "wrong"}, "cwd is"),
        ({"session": "wrong"}, "exactly one live pane"),
    ],
)
def test_adopt_refuses_live_identity_mismatch_without_registering(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    change: dict[str, str | None], message: str,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    info = fake.infos[pane]
    if "agent" in change:
        fake.infos[pane] = replace(info, agent=change["agent"])
    workspace = str(change.get("workspace", "subagents"))
    cwd = tmp_path
    if change.get("cwd") == "wrong":
        cwd = tmp_path / "other"
        cwd.mkdir()
    session = str(change["session"]) if "session" in change else None

    with pytest.raises(AgentDeliveryError, match=message):
        sessions.adopt("foreign", pane_id=pane, expected_workspace=workspace,
                       expected_cwd=str(cwd), harness="codex", session=session)

    assert not (sessions.registry / "foreign").exists()
    assert fake.closed == [] and fake.submitted == []


def test_adopt_refuses_duplicate_pane_and_keeps_first_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, pane = setup_foreign(tmp_path, monkeypatch)
    original = adopt(sessions, pane, tmp_path)
    with pytest.raises(AgentDeliveryError, match="already registered as 'foreign'"):
        adopt(sessions, pane, tmp_path, name="second")
    assert sessions.get("foreign").token == original["token"]
    assert not (sessions.registry / "second").exists()


def test_same_provider_local_session_id_is_allowed_for_different_harnesses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    adopt(sessions, pane, tmp_path)
    fake.create_tab(workspace_id="w1", label="claude", cwd=str(tmp_path))
    claude = fake.presentations[-1].pane_id
    fake.infos[claude] = AgentPaneInfo(
        claude, "w1", str(tmp_path), "claude", "idle",
        "claude", "native-session",
    )
    result = sessions.adopt(
        "claude", pane_id=claude, expected_workspace="subagents",
        expected_cwd=str(tmp_path), harness="claude",
    )
    assert result["session_agent"] == "claude"
    assert result["session_value"] == "native-session"
    assert {row["name"] for row in sessions.list()} == {"foreign", "claude"}


def test_adopt_refuses_same_harness_session_held_by_headless_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, pane = setup_foreign(tmp_path, monkeypatch)
    sessions._prepare()
    (sessions.registry / "headless").mkdir(mode=0o700)
    sessions._save(AgentRecord(
        "headless", "headless-token", "codex", str(tmp_path), 1.0,
        lifecycle="running", session_value="native-session",
        adapter="turn-runner", mode="headless", backend="tmux",
        runtime_home=str(tmp_path / "runtime"),
    ))
    with pytest.raises(AgentDeliveryError, match="already registered as 'headless'"):
        adopt(sessions, pane, tmp_path)
    assert not (sessions.registry / "foreign").exists()


def test_interactive_start_refuses_resume_claimed_by_adopted_agent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    adopt(sessions, pane, tmp_path)
    presentation = fake.presentations[0]
    with pytest.raises(AgentDeliveryError, match="already registered as 'foreign'"):
        sessions.start(
            "owned", cwd=str(tmp_path), harness="codex",
            resume="native-session",
        )
    assert not (sessions.registry / "owned").exists()
    assert fake.presentations == [presentation]
    assert fake.launched == [] and fake.closed == []


def test_conflicting_started_pane_remains_stoppable_if_initial_close_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    adopt(sessions, pane, tmp_path)
    original_start, original_close = fake.start_agent, fake.close_pane

    def start_agent(
        name: str, kind: str, pane_id: str, arguments: tuple[str, ...], *, timeout: float,
    ) -> None:
        original_start(name, kind, pane_id, arguments, timeout=timeout)
        fake.infos[pane_id] = replace(
            fake.infos[pane_id], session_agent="codex",
            session_value="native-session",
        )

    def fail_close(_pane: str) -> None:
        raise AgentDeliveryError("close failed")

    monkeypatch.setattr(fake, "start_agent", start_agent)
    monkeypatch.setattr(fake, "close_pane", fail_close)
    with pytest.raises(AgentDeliveryError, match="could not close"):
        sessions.start("owned", cwd=str(tmp_path), harness="codex")
    failed = sessions.get("owned")
    assert failed.lifecycle == "launch_failed"
    assert failed.session_agent is None and failed.session_value is None
    monkeypatch.setattr(fake, "close_pane", original_close)
    assert sessions.stop("owned")["pane_closed"] is True
    assert sessions.status("foreign")["agent_status"] == "idle"


def test_headless_start_stops_runtime_if_session_is_claimed_by_adopted_agent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, pane = setup_foreign(tmp_path, monkeypatch)
    adopt(sessions, pane, tmp_path)
    actions: list[str] = []

    def worker(
        record: AgentRecord, action: str, **_options: object,
    ) -> dict[str, object]:
        actions.append(action)
        return {"record": {"mode": "headless", "backend": "tmux",
                           "session_id": "native-session"}, "result": {}}

    monkeypatch.setattr(sessions, "_worker", worker)
    with pytest.raises(AgentDeliveryError, match="already registered as 'foreign'"):
        sessions.start_session(
            "headless", cwd=str(tmp_path), mode="headless", backend="tmux",
        )
    failed = sessions.get("headless")
    assert failed.lifecycle == "launch_failed"
    assert failed.session_value is None
    assert actions == ["start", "stop"]
    assert sessions.status("foreign")["agent_status"] == "idle"


def test_adopt_refuses_a_reported_session_visible_in_two_live_panes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    fake.create_tab(workspace_id="w1", label="duplicate", cwd=str(tmp_path))
    duplicate = fake.presentations[-1].pane_id
    fake.infos[duplicate] = AgentPaneInfo(
        duplicate, "w1", str(tmp_path), "codex", "idle",
        "codex", "native-session",
    )
    with pytest.raises(AgentDeliveryError, match="exactly one live pane"):
        adopt(sessions, pane, tmp_path)
    assert not (sessions.registry / "foreign").exists()


@pytest.mark.parametrize("native_session", [True, False])
def test_adopt_archives_failed_generation_if_identity_changes_after_save(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, native_session: bool,
) -> None:
    sessions, fake, pane = setup_foreign(
        tmp_path, monkeypatch, native_session=native_session
    )
    original = fake.pane_info

    def changing(pane_id: str) -> AgentPaneInfo:
        info = original(pane_id)
        if (sessions.registry / "foreign/agent.json").exists():
            return replace(info, session_agent="codex",
                           session_value="replacement-session")
        return info

    monkeypatch.setattr(fake, "pane_info", changing)
    with pytest.raises(AgentDeliveryError, match="was not registered"):
        adopt(sessions, pane, tmp_path)

    assert not (sessions.registry / "foreign").exists()
    archives = list((sessions.registry / "archive").glob(
        "foreign-*-adopt-failed/agent.json"
    ))
    assert len(archives) == 1
    saved = json.loads(archives[0].read_text())
    assert saved["lifecycle"] == "adopt_failed"
    assert saved["error"]
    assert fake.closed == [] and fake.submitted == []


def test_adopt_without_reported_session_can_be_bound_for_native_goal_inspection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch, native_session=False)
    result = adopt(sessions, pane, tmp_path)
    assert result["session_value"] is None
    assert sessions.goal("foreign")["native_status"] == "unverified"
    sessions.bind_session("foreign", "native-session")
    assert sessions.goal("foreign")["native_status"] == "absent"
    sessions.send_session("foreign", "still pane-bound")
    binding = json.loads((tmp_path / "registry/foreign/queue/target.json").read_text())
    assert binding["kind"] == "pane" and binding["pane_id"] == pane
    assert fake.submitted == ["still pane-bound"]


def test_bind_session_refuses_an_authoritative_owner_in_another_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    adopt(sessions, pane, tmp_path)
    fake.create_tab(workspace_id="w1", label="sessionless", cwd=str(tmp_path))
    second = fake.presentations[-1].pane_id
    fake.infos[second] = AgentPaneInfo(
        second, "w1", str(tmp_path), "codex", "idle", None, None,
    )
    sessions.adopt(
        "second", pane_id=second, expected_workspace="subagents",
        expected_cwd=str(tmp_path), harness="codex",
    )
    with pytest.raises(AgentDeliveryError, match="already registered as 'foreign'"):
        sessions.bind_session("second", "native-session")
    assert sessions.get("second").goal_session_id is None


def test_bind_session_refuses_unregistered_live_session_contradiction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(
        tmp_path, monkeypatch, native_session=False
    )
    adopt(sessions, pane, tmp_path)
    fake.create_tab(workspace_id="w1", label="unregistered", cwd=str(tmp_path))
    unregistered = fake.presentations[-1].pane_id
    fake.infos[unregistered] = AgentPaneInfo(
        unregistered, "w1", str(tmp_path), "codex", "idle",
        "codex", "reported-elsewhere",
    )
    with pytest.raises(AgentDeliveryError, match="another or ambiguous live pane"):
        sessions.bind_session("foreign", "reported-elsewhere")
    assert sessions.get("foreign").goal_session_id is None


def test_concurrent_bind_session_allows_exactly_one_provider_local_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, pane = setup_foreign(
        tmp_path, monkeypatch, native_session=False
    )
    adopt(sessions, pane, tmp_path)
    fake.create_tab(workspace_id="w1", label="second", cwd=str(tmp_path))
    second = fake.presentations[-1].pane_id
    fake.infos[second] = AgentPaneInfo(
        second, "w1", str(tmp_path), "codex", "idle", None, None,
    )
    sessions.adopt(
        "second", pane_id=second, expected_workspace="subagents",
        expected_cwd=str(tmp_path), harness="codex",
    )
    barrier = threading.Barrier(3)
    outcomes: list[str] = []
    outcome_lock = threading.Lock()

    def bind(name: str) -> None:
        barrier.wait()
        try:
            sessions.bind_session(name, "shared-session")
        except AgentDeliveryError:
            outcome = "refused"
        else:
            outcome = "bound"
        with outcome_lock:
            outcomes.append(outcome)

    threads = [threading.Thread(target=bind, args=(name,))
               for name in ("foreign", "second")]
    for thread in threads:
        thread.start()
    barrier.wait()
    for thread in threads:
        thread.join(2)
        assert not thread.is_alive()
    assert sorted(outcomes) == ["bound", "refused"]
    claims = [sessions.get(name).goal_session_id
              for name in ("foreign", "second")]
    assert claims.count("shared-session") == 1


def test_cli_adopt_requires_identity_and_stop_is_non_destructive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    sessions, fake, pane = setup_foreign(tmp_path, monkeypatch)
    monkeypatch.setattr(cli, "HerdrClient", lambda **_kwargs: cast(HerdrClient, fake))
    registry = str(sessions.registry)
    assert cli.main(["adopt", "foreign", "--pane", pane, "--workspace", "subagents",
                     "--cwd", str(tmp_path), "--harness", "codex",
                     "--registry", registry]) == 0
    adopted = json.loads(capsys.readouterr().out)
    assert adopted["adapter"] == "herdr-foreign"
    assert cli.main(["stop", "foreign", "--registry", registry]) == 0
    stopped = json.loads(capsys.readouterr().out)
    assert stopped["runtime_preserved"] is True
    assert fake.closed == []


def test_adopt_help_names_every_required_identity_assertion(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as result:
        cli.parser().parse_args(["adopt", "--help"])
    assert result.value.code == 0
    output = capsys.readouterr().out
    for value in ("--pane", "--workspace", "--cwd", "--harness", "--session",
                  "without taking ownership"):
        assert value in output
