"""Unified registry and adapter boundaries, independent of terminal-server availability."""
from __future__ import annotations

import fcntl
import json
import os
import subprocess
import sys
from dataclasses import replace
from pathlib import Path
from subprocess import CompletedProcess
from typing import cast

import pytest

from agentctl import cli, mcp
from agentctl.client import HerdrClient, Pane
from agentctl.errors import AgentDeliveryError
from agentctl.profiles import validate_muse_headless_arguments
from agentctl.sessions import Sessions
from agentctl.subagents import AgentRecord
from .test_herdr_subagents import FakeManagedClient


def setup(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Sessions, FakeManagedClient, list[str]]:
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    sessions = Sessions(cast(HerdrClient, fake), tmp_path / "registry")
    calls: list[str] = []

    def worker(record: AgentRecord, action: str, **options: object) -> dict[str, object]:
        calls.append(action)
        return {"record": {"mode": "headless", "backend": record.backend,
                "session_id": "native-thread", "tmux_target": "workers:worker",
                "presentation_pane": "w1:headless"},
            "result": {"agents": [{"name": record.name, "status": "idle", "pending": 0, "runner_alive": True}]}}

    monkeypatch.setattr(sessions, "_worker", worker)
    return sessions, fake, calls


@pytest.mark.parametrize("first,second", [("interactive", "headless"), ("headless", "interactive"), ("headless", "headless")])
def test_modes_share_names_and_preserve_the_original_generation(first: str, second: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    first_record = sessions.start_session("worker", cwd=str(tmp_path), mode=first)
    launches = len(fake.launched) + calls.count("start")
    with pytest.raises(AgentDeliveryError, match="already registered"):
        sessions.start_session("worker", cwd=str(tmp_path), mode=second)
    assert sessions.get("worker").token == first_record["token"]
    assert len(fake.launched) + calls.count("start") == launches
    assert len(sessions.list()) == 1


@pytest.mark.parametrize("mode", ["interactive", "headless"])
def test_retirement_reuses_a_name_only_with_a_new_generation_and_stable_lock(mode: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    original = sessions.start_session("worker", cwd=str(tmp_path), mode=mode)
    lock = tmp_path / "registry/.worker.lock"
    inode = lock.stat().st_ino
    stopped = sessions.stop("worker")
    archive = Path(str(stopped["archive"]))
    assert json.loads((archive / "agent.json").read_text())["token"] == original["token"]
    replacement = sessions.start_session("worker", cwd=str(tmp_path), mode=mode)
    assert replacement["token"] != original["token"]
    assert lock.stat().st_ino == inode


@pytest.mark.parametrize("mode", ["interactive", "headless"])
@pytest.mark.parametrize("operation", ["send", "stop", "attach", "wait"])
def test_dispatch_never_follows_a_name_into_a_replacement_generation(mode: str, operation: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode=mode)
    original_load = sessions._load
    replaced = False

    def load(name: str) -> AgentRecord:
        nonlocal replaced
        record = original_load(name)
        if not replaced:
            replaced = True
            # Model a retirement/relaunch after routing inspected the old record,
            # before it acquired this name's lifecycle lock.
            sessions._save(replace(record, token="replacement-generation"))
        return record

    monkeypatch.setattr(sessions, "_load", load)
    calls.clear()
    with pytest.raises(AgentDeliveryError, match="replaced"):
        if operation == "send":
            sessions.send_session("worker", "old generation task")
        elif operation == "stop":
            sessions.stop("worker")
        elif operation == "attach":
            sessions.attach("worker")
        else:
            sessions.wait("worker", timeout=0)
    assert not fake.submitted and not fake.closed
    assert not calls
    assert original_load("worker").token == "replacement-generation"


@pytest.mark.parametrize("mode", ["interactive", "headless"])
def test_pausing_blocks_input_but_preserves_readiness_and_the_running_session(mode: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    record = sessions.start_session("worker", cwd=str(tmp_path), mode=mode)
    sessions.pause("worker")
    calls.clear()
    with pytest.raises(AgentDeliveryError, match="paused"):
        sessions.send_session("worker", "should wait")
    if mode == "interactive":
        with pytest.raises(AgentDeliveryError, match="paused"):
            sessions.drain("worker")
        with pytest.raises(AgentDeliveryError, match="paused"):
            sessions.goal("worker", "do work")
    else:
        with pytest.raises(AgentDeliveryError, match="paused"):
            sessions.runtime_operation("worker", "reset")
    assert not fake.submitted and not calls
    assert sessions.wait("worker", timeout=0)["token"] == record["token"]
    assert sessions.get("worker").paused
    sessions.pause("worker", paused=False)
    sessions.send_session("worker", "continue")
    assert fake.submitted == ["continue"] if mode == "interactive" else "send" in calls


def test_native_goal_and_transcript_boundaries_are_honest(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path))
    capabilities = cast(list[str], sessions.status("worker")["capabilities"])
    assert "terminal-snapshot" in capabilities
    assert "final-answer" not in capabilities
    with pytest.raises(AgentDeliveryError, match="snapshots"):
        sessions.read_session("worker", output="last")


def test_headless_start_rejects_interactive_tab_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="headless start does not accept.*environment"):
        sessions.start_session(
            "worker",
            cwd=str(tmp_path),
            mode="headless",
            environment=("META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",),
        )
    assert fake.environments == []
    assert calls == []
    assert not (tmp_path / "registry").exists()


@pytest.mark.parametrize(
    "arguments",
    [
        ("--session-id=foreign",),
        ("--json",),
        ("--prompt-file=/tmp/prompt",),
        ("resume",),
        ("--",),
        ("--api-key-stdin",),
        ("--provider", "meta"),
    ],
)
def test_headless_muse_reserves_session_prompt_and_precedence_options(
    arguments: tuple[str, ...], tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError):
        sessions.start_session(
            "worker", cwd=str(tmp_path), mode="headless", harness="muse",
            harness_args=arguments,
        )
    assert fake.environments == []
    assert calls == []
    assert not (tmp_path / "registry").exists()


def test_headless_muse_accepts_compiled_effort_and_repeatable_literal_options() -> None:
    validate_muse_headless_arguments([
        "--reasoning-effort", "ultra", "--image=first.png", "--image=second.png",
    ])


def test_headless_muse_refuses_raw_duplicate_structured_options_before_allocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="model field"):
        sessions.start_session(
            "worker", cwd=str(tmp_path), mode="headless", harness="muse",
            model="structured", harness_args=("--model=raw",),
        )
    with pytest.raises(AgentDeliveryError, match="reasoning effort"):
        sessions.start_session(
            "worker", cwd=str(tmp_path), mode="headless", harness="muse",
            reasoning_effort="ultra", harness_args=("--effort", "low"),
        )
    assert not (tmp_path / "registry").exists()
    assert fake.environments == []
    assert calls == []


def test_headless_muse_rejects_string_harness_arguments_before_allocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, calls = setup(tmp_path, monkeypatch)
    with pytest.raises(AgentDeliveryError, match="headless harness arguments"):
        sessions.start_session(
            "worker", cwd=str(tmp_path), mode="headless", harness="muse",
            harness_args="--image=not-a-sequence-of-arguments",
        )
    assert not (tmp_path / "registry").exists()
    assert fake.environments == []
    assert calls == []


def test_cli_headless_muse_profile_reaches_worker_with_exact_structured_arguments(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    subprocess.run(["/usr/bin/git", "init", "-q", str(tmp_path)], check=True)
    (tmp_path / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
    directory = tmp_path / ".agentctl"
    directory.mkdir(mode=0o700)
    config = directory / "profiles.json"
    config.write_text(json.dumps({
        "schema": "agentctl-profiles/v1",
        "profiles": {
            "watermelon-exec": {
                "harness": "muse",
                "mode": "headless",
                "model": "kiki_gb300_mxfp8_6p2_840_nwr",
                "reasoning_effort": "ultra",
                "argv": ["--image=first.png", "--image=second.png"],
                "env": {},
            },
        },
    }), encoding="utf-8")
    config.chmod(0o600)
    fake = FakeManagedClient()
    captured: list[dict[str, object]] = []

    def worker(
        _sessions: Sessions, record: AgentRecord, action: str, **options: object,
    ) -> dict[str, object]:
        if action == "start":
            captured.append(dict(options))
        return {
            "record": {
                "mode": "headless", "backend": record.backend,
                "session_id": "native-thread", "tmux_target": "workers:worker",
                "presentation_pane": "w1:headless",
            },
            "result": {
                "agents": [{
                    "name": record.name, "status": "idle", "pending": 0,
                    "runner_alive": True,
                }],
            },
        }

    monkeypatch.setattr(cli, "HerdrClient", lambda **_kwargs: cast(HerdrClient, fake))
    monkeypatch.setattr(Sessions, "_worker", worker)
    assert cli.main([
        "start", "worker", "--cwd", str(tmp_path),
        "--registry", str(tmp_path / "registry"),
        "--profile", "watermelon-exec", "--brief", "return PROFILE_OK",
    ]) == 0
    status = json.loads(capsys.readouterr().out)
    assert status["name"] == "worker"
    assert status["adapter"] == "turn-runner"
    assert len(captured) == 1
    assert captured[0]["harness"] == "muse"
    assert captured[0]["model"] == "kiki_gb300_mxfp8_6p2_840_nwr"
    assert captured[0]["harness_args"] == [
        "--reasoning-effort", "ultra", "--image=first.png", "--image=second.png",
    ]
    assert captured[0]["brief"] == "return PROFILE_OK"
    assert fake.launched == []


def test_mcp_routes_into_the_cli_registry_and_honors_its_pause(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, fake, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path))
    monkeypatch.setattr(cli, "HerdrClient", lambda **_: fake)
    registry = str(tmp_path / "registry")
    assert mcp.call_tool("agent_pause", {"name": "worker"}, registry, "unused")["isError"] is False
    assert sessions.get("worker").paused
    result = mcp.call_tool("agent_send", {"name": "worker", "text": "do not send"}, registry, "unused")
    assert result["isError"] is True
    assert not fake.submitted
    assert mcp.call_tool("agent_resume", {"name": "worker"}, registry, "unused")["isError"] is False
    assert mcp.call_tool("agent_send", {"name": "worker", "text": "--literal prompt"}, registry, "unused")["isError"] is False
    assert fake.submitted == ["--literal prompt"]


def test_headless_herdr_attach_checks_pane_ownership_and_focuses_its_tab(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, fake, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    fake.presentations.append(Pane("w1:headless", "workers:worker", "w1"))
    focused: list[str] = []
    monkeypatch.setattr(fake, "focus_tab", focused.append, raising=False)
    assert sessions.attach("worker")["focused"] == "tab"
    assert focused == ["workers:worker"]
    fake.presentations[0] = Pane("w1:headless", "different-tab", "w1")
    with pytest.raises(AgentDeliveryError, match="no longer belongs"):
        sessions.attach("worker")
    assert focused == ["workers:worker"]


@pytest.mark.parametrize("inside", [True, False])
def test_tmux_attach_uses_exact_window_identity_and_releases_lifecycle_lock(inside: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import agentctl.sessions as module
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless", backend="tmux")
    if inside:
        monkeypatch.setenv("TMUX", "/tmp/socket,123,0")
    else:
        monkeypatch.delenv("TMUX", raising=False)
    commands: list[list[str]] = []

    def control(command: list[str]) -> CompletedProcess[str]:
        commands.append(command)
        return CompletedProcess(command, 0, "@123\n", "")

    def attach(command: list[str], **_: object) -> CompletedProcess[str]:
        descriptor = os.open(tmp_path / "registry/.worker.lock", os.O_RDONLY)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)
        commands.append(command)
        return CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(module, "_bounded_control_command", control)
    monkeypatch.setattr(subprocess, "run", attach)
    monkeypatch.setattr(sys.stdin, "isatty", lambda: True)
    monkeypatch.setattr(sys.stdout, "isatty", lambda: True)
    result = sessions.attach("worker")
    assert result["focused"] == "window"
    assert commands[0] == ["tmux", "display-message", "-p", "-t", "=workers:=worker", "#{window_id}"]
    assert commands[1] == ["tmux", "select-window" if inside else "attach-session", "-t", "@123"]


def test_nested_stop_receipt_paths_follow_the_canonical_archive(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")

    def worker(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "stop" and record.runtime_home
        archived = Path(record.runtime_home) / "state/_archive/worker"
        archived.mkdir(parents=True)
        (archived / "turn.log").write_text("completed turn")
        return {"result": {"archived_to": str(archived), "state_path": str(archived / "turn.log")}}

    monkeypatch.setattr(sessions, "_worker", worker)
    result = sessions.stop("worker")
    nested = cast(dict[str, object], cast(dict[str, object], result["runtime"])["result"])
    assert Path(str(nested["archived_to"])).is_dir()
    assert Path(str(nested["state_path"])).read_text() == "completed turn"
    assert Path(str(nested["archived_to"])).is_relative_to(Path(str(result["archive"])))


@pytest.mark.parametrize("liveness", [False, None, "absent"])
def test_saved_idle_state_cannot_make_a_dead_or_unverifiable_runner_ready(liveness: object, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    original = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        row: dict[str, object] = {"name": record.name, "status": "idle", "pending": 0}
        if liveness != "absent":
            row["runner_alive"] = liveness
        return {"result": {"agents": [row]}, "record": {"session_id": "native-thread"}}

    monkeypatch.setattr(sessions, "_worker", status)
    with pytest.raises(AgentDeliveryError, match="not alive|cannot confirm"):
        sessions.wait("worker", timeout=0)
    assert sessions.get("worker").token == original["token"]
    assert sessions.get("worker").lifecycle == "running"  # A failed probe never reaps state.


def test_live_idle_runner_remains_ready_when_only_its_presentation_is_lost(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    original = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status" and record.token == original["token"]
        descriptor = os.open(tmp_path / "registry/.worker.lock", os.O_RDONLY)
        try:
            with pytest.raises(BlockingIOError):
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)
        return {"result": {"agents": [{"name": record.name, "status": "idle", "pending": 0,
            "runner_alive": True, "window_alive": False, "presentation_degraded": True}]},
            "record": {"session_id": "native-thread"}}

    monkeypatch.setattr(sessions, "_worker", status)
    assert sessions.wait("worker", timeout=0)["token"] == original["token"]


@pytest.mark.parametrize("timeout", [float("nan"), float("inf"), -1.0, 31_536_001.0])
def test_invalid_wait_deadlines_are_refused_before_registry_access(timeout: float, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)

    def no_load(_: str) -> AgentRecord:
        raise AssertionError("invalid deadline accessed the registry")

    monkeypatch.setattr(sessions, "_load", no_load)
    with pytest.raises(AgentDeliveryError, match="finite"):
        sessions.wait("worker", timeout=timeout)
    assert not (tmp_path / "registry").exists()


@pytest.mark.parametrize("output,since", [("tail", 9), ("all", 9), ("last", 9), ("since_turn", None)])
def test_invalid_turn_boundaries_are_refused_before_worker_dispatch(output: str, since: int | None, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    calls.clear()
    with pytest.raises(AgentDeliveryError, match="requires"):
        sessions.read_session("worker", output=output, since_turn=since)
    assert not calls
