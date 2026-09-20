"""One-command coordinator launch from an existing Herdr shell pane."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

import agentctl.chat as chat_module
from agentctl.agent import Target
from agentctl.chat import Config, _launch_config, _launch_here
from agentctl.client import AgentPaneInfo


class LaunchClient:
    def __init__(self, *, agent: str | None = None, workspace: str = "w7") -> None:
        self.agent = agent
        self.workspace = workspace

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        assert pane_id == "w7:p3"
        return AgentPaneInfo(pane_id, self.workspace, "/work/project", self.agent,
                             "unknown", None, None)

    def workspace_label(self, workspace_id: str) -> str:
        assert workspace_id == "w7"
        return "project-space"


def _authority() -> dict[str, object]:
    return {
        "space": "spaces/test",
        "allowed_senders": ["users/owner"],
        "transport_command": ["/opt/chat-adapter"],
        "ack_reaction": "🤖",
    }


def test_launch_config_binds_current_pane_and_uses_model_label(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("HERDR_ENV", "1")
    monkeypatch.setenv("HERDR_PANE_ID", "w7:p3")
    monkeypatch.setenv("HERDR_WORKSPACE_ID", "w7")
    monkeypatch.setattr("agentctl.chat.shutil.which", lambda command: "/bin/" + command)
    document = _authority()
    document.update({"agent_name": "stale-name", "target": {"pane_id": "stale"},
                     "agent_label": "stale-label"})

    config = _launch_config(document, LaunchClient(), harness="codex",
                            model="gpt-6-astra", agent_label=None)

    assert config.agent_name is None
    assert config.agent_label == "gpt-6-astra"
    assert config.target == Target(pane_id="w7:p3", expected_agent="codex",
                                   expected_cwd="/work/project", expected_workspace="project-space")
    assert document["agent_name"] == "stale-name"


@pytest.mark.parametrize(("environment", "agent", "message"), [
    ({}, None, "inside the Herdr shell pane"),
    ({"HERDR_ENV": "1", "HERDR_PANE_ID": "w7:p3", "HERDR_WORKSPACE_ID": "wrong"}, None,
     "do not match"),
    ({"HERDR_ENV": "1", "HERDR_PANE_ID": "w7:p3", "HERDR_WORKSPACE_ID": "w7"}, "codex",
     "already hosts an agent"),
])
def test_launch_config_refuses_unsafe_pane_identity(
    monkeypatch: pytest.MonkeyPatch, environment: dict[str, str], agent: str | None, message: str,
) -> None:
    for key in ("HERDR_ENV", "HERDR_PANE_ID", "HERDR_WORKSPACE_ID"):
        monkeypatch.delenv(key, raising=False)
    for key, value in environment.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setattr("agentctl.chat.shutil.which", lambda command: "/bin/" + command)
    with pytest.raises(ValueError, match=message):
        _launch_config(_authority(), LaunchClient(agent=agent), harness="codex",
                       model=None, agent_label=None)


def test_launch_here_owns_bridge_for_coordinator_lifetime(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    config_path = tmp_path / "chat.json"
    config_path.write_text(json.dumps(_authority()), encoding="utf-8")
    target = Target(pane_id="w7:p3", expected_agent="codex",
                    expected_cwd="/work/project", expected_workspace="project-space")
    config = Config("spaces/test", ("users/owner",), target, "gpt-6-astra",
                    transport_command=("/opt/chat-adapter",))
    monkeypatch.setattr(chat_module, "_launch_config", lambda *args, **kwargs: config)
    (tmp_path / "state").mkdir()
    initialized: list[tuple[Path, Config, str | None]] = []
    monkeypatch.setattr(chat_module.Bridge, "initialize",
                        lambda state, value, after=None: initialized.append((state, value, after)))

    class Process:
        def __init__(self, command: list[str], **kwargs: object) -> None:
            self.command, self.kwargs = command, kwargs
            self.pid = 12345
            self.bridge = command[1:3] == [str(Path(chat_module.__file__).resolve()), "run"]
            self.terminated = False

        def poll(self) -> int | None:
            return None if self.bridge and not self.terminated else 0

        def terminate(self) -> None:
            self.terminated = True

        def kill(self) -> None:
            self.terminated = True

        def wait(self, timeout: float | None = None) -> int:
            self.terminated = True
            return 0

    processes: list[Process] = []

    def popen(command: list[str], **kwargs: object) -> Process:
        process = Process(command, **kwargs)
        processes.append(process)
        return process

    monkeypatch.setattr("agentctl.chat.subprocess.Popen", popen)
    monkeypatch.setattr("agentctl.chat.os.killpg", lambda pid, signal: processes[0].terminate())

    result = _launch_here(tmp_path / "state", config_path, harness="codex", model="gpt-6-astra",
                          resume=None, harness_args=("--personality", "pragmatic"), agent_label=None,
                          after=None, interval=3, reconcile_interval=300, prog="agentctl chat")

    assert result == 0
    assert initialized == [(tmp_path / "state", config, None)]
    assert processes[0].command == [
        sys.executable, str(Path(chat_module.__file__).resolve()), "run",
        "--state", str((tmp_path / "state").absolute()), "--interval", "3",
        "--reconcile-interval", "300",
    ]
    assert processes[0].kwargs["start_new_session"] is True
    assert processes[0].terminated
    assert processes[1].command == ["codex", "--no-alt-screen", "--model", "gpt-6-astra",
                                    "--personality", "pragmatic"]


def test_launch_help_describes_current_pane_and_every_option(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as result:
        chat_module.run_cli(["launch", "--help"])
    assert result.value.code == 0
    output = capsys.readouterr().out
    for text in ("this Herdr pane", "--config", "--harness", "--model", "--resume",
                 "--harness-arg", "--agent-label", "--after", "--interval",
                 "--reconcile-interval", "--state", "0.1–86400"):
        assert text in output


def test_launch_accepts_one_day_poll_interval_and_rejects_larger(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str],
) -> None:
    intervals: list[float] = []

    def launch(*args: object, **kwargs: object) -> int:
        interval = kwargs["interval"]
        assert isinstance(interval, float)
        intervals.append(interval)
        return 0

    monkeypatch.setattr(chat_module, "_launch_here", launch)
    assert chat_module.run_cli([
        "launch", "--config", str(tmp_path / "chat.json"), "--interval", "86400",
    ]) == 0
    assert intervals == [86400]
    assert chat_module.run_cli([
        "launch", "--config", str(tmp_path / "chat.json"), "--interval", "86400.1",
    ]) == 1
    assert "between 0.1 and 86400 seconds" in capsys.readouterr().err


def test_launch_defaults_to_hourly_polling(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    intervals: list[float] = []

    def launch(*args: object, **kwargs: object) -> int:
        interval = kwargs["interval"]
        assert isinstance(interval, float)
        intervals.append(interval)
        return 0

    monkeypatch.setattr(chat_module, "_launch_here", launch)
    assert chat_module.run_cli([
        "launch", "--config", str(tmp_path / "chat.json"),
    ]) == 0
    assert intervals == [3600.0]
