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

from agentctl import cli, mcp, worker_rpc
from agentctl.client import CustomProcessIdentity, HerdrClient, Pane
from agentctl.errors import AgentDeliveryError
from agentctl.foreign import lib as worker_lib
from agentctl.profiles import validate_muse_headless_arguments
from agentctl.sessions import Sessions, WorkerRpcError
from agentctl.subagents import AgentRecord
from .test_herdr_subagents import FakeManagedClient


RUNNER_IDENTITY = CustomProcessIdentity(
    version=1,
    boot_id="11111111-2222-3333-4444-555555555555",
    pid=4242,
    starttime_ticks=9001,
    executable_device=3,
    executable_inode=4,
)
RUNNER_IDENTITY_JSON = {
    "version": RUNNER_IDENTITY.version,
    "boot_id": RUNNER_IDENTITY.boot_id,
    "pid": RUNNER_IDENTITY.pid,
    "starttime_ticks": RUNNER_IDENTITY.starttime_ticks,
    "executable_device": RUNNER_IDENTITY.executable_device,
    "executable_inode": RUNNER_IDENTITY.executable_inode,
}


def setup(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Sessions, FakeManagedClient, list[str]]:
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    sessions = Sessions(cast(HerdrClient, fake), tmp_path / "registry")
    calls: list[str] = []

    def worker(record: AgentRecord, action: str, **options: object) -> dict[str, object]:
        calls.append(action)
        # Starting the detached runner is asynchronous: its first legitimate
        # response can precede publication of the child PID/starttime.
        identity: dict[str, object] = {} if action == "start" else {
            "runner_pid": 4242, "runner_started_at": "9001",
            "runner_identity": RUNNER_IDENTITY_JSON,
        }
        if action == "status":
            descriptor = os.open(sessions.registry / f".{record.name}.lock", os.O_RDONLY)
            try:
                with pytest.raises(BlockingIOError):
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            finally:
                os.close(descriptor)
        return {"record": {"name": record.name, "owner_token": record.token,
                "mode": "headless", "backend": record.backend,
                "session_id": "native-thread", "tmux_target": "workers:worker",
                "presentation_pane": "w1:headless", **identity},
            "result": {"agents": [{"name": record.name, "status": "idle", "pending": 0,
                **identity, "runner_alive": True if identity else None}]}}

    monkeypatch.setattr(sessions, "_worker", worker)
    return sessions, fake, calls


def test_headless_start_binds_first_later_exact_runner_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, calls = setup(tmp_path, monkeypatch)
    status = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    saved = sessions.get("worker")
    assert calls[:2] == ["start", "status"]
    assert saved.lifecycle == "running"
    assert (saved.runner_pid, saved.runner_started_at) == (4242, "9001")
    assert cast(dict[str, object], status["runtime_liveness"])["runner_alive"] is True


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
        "schema": "agentctl-profiles/v2",
        "default_workspace": {"id": "wK"},
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
                "owner_token": record.token,
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


def test_cli_v2_config_selects_default_workspace_and_exact_opus_profile(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    subprocess.run(["/usr/bin/git", "init", "-q", str(tmp_path)], check=True)
    (tmp_path / ".gitignore").write_text(".agentctl/\n", encoding="utf-8")
    directory = tmp_path / ".agentctl"
    directory.mkdir(mode=0o700)
    config = directory / "profiles.json"
    config.write_text(json.dumps({
        "schema": "agentctl-profiles/v2",
        "default_workspace": {"id": "wK"},
        "profiles": {
            "claude-opus-55": {
                "harness": "claude", "mode": "interactive", "model": "opus",
                "argv": [], "env": {},
            },
        },
    }), encoding="utf-8")
    config.chmod(0o600)
    fake = FakeManagedClient()
    monkeypatch.setattr(cli, "HerdrClient", lambda **_kwargs: cast(HerdrClient, fake))

    assert cli.main(["profiles", "--cwd", str(tmp_path)]) == 0
    listed = json.loads(capsys.readouterr().out)
    assert listed["default_workspace"] == {"id": "wK"}
    assert listed["profiles"] == [{
        "argv_count": 0, "environment": [], "harness": "claude",
        "mode": "interactive", "model": "opus", "name": "claude-opus-55",
        "reasoning_effort": None,
    }]

    monkeypatch.setenv("HERDR_WORKSPACE_ID", "w-env")
    assert cli.main([
        "start", "worker", "--cwd", str(tmp_path),
        "--registry", str(tmp_path / "registry"), "--profile", "claude-opus-55",
    ]) == 0
    status = json.loads(capsys.readouterr().out)
    assert status["workspace_id"] == "wK"
    assert status["launch_profile"] == "claude-opus-55"
    assert fake.launched == [("worker", "claude", "w1:p1", ("--model", "opus"))]

    assert cli.main([
        "start", "override", "--cwd", str(tmp_path),
        "--registry", str(tmp_path / "registry"), "--profile", "claude-opus-55",
        "--workspace-id", "w2",
    ]) == 0
    overridden = json.loads(capsys.readouterr().out)
    assert overridden["workspace_id"] == "w2"


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
        return {"result": {"agents": [row]}, "record": {
            "session_id": "native-thread", "owner_token": record.token,
        }}

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
            "record": {"session_id": "native-thread", "owner_token": record.token}}

    monkeypatch.setattr(sessions, "_worker", status)
    assert sessions.wait("worker", timeout=0)["token"] == original["token"]


@pytest.mark.parametrize("message", [
    "transport stopped responding before a liveness receipt",
    "remote side says runner is not alive but supplied no identity",
    "decoder saw dead bytes in an incomplete response",
])
def test_headless_health_never_infers_death_from_error_text(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, message: str,
) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")

    def failed_status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert record.name == "worker" and action == "status"
        raise AgentDeliveryError(message)

    monkeypatch.setattr(sessions, "_worker", failed_status)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == "unknown"
    assert row["runtime_state"] == "unknown"
    assert row["reason_code"] == "runtime-probe-failed"


@pytest.mark.parametrize("alive", [False, True])
def test_headless_health_requires_identity_bound_typed_liveness(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, alive: bool,
) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    saved = replace(
        sessions.get("worker"), runner_pid=4242, runner_started_at="9001",
        runner_identity=RUNNER_IDENTITY,
    )
    sessions._save(saved)

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert record.token == saved.token and action == "status"
        identity = {
            "name": record.name, "runner_pid": 4242, "runner_started_at": "9001",
            "runner_identity": RUNNER_IDENTITY_JSON,
        }
        return {
            "record": {
                **identity, "session_id": "native-thread", "owner_token": record.token,
            },
            "result": {"agents": [{
                **identity, "status": "dead", "pending": 0,
                "runner_alive": alive,
            }]},
        }

    monkeypatch.setattr(sessions, "_worker", status)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == ("healthy" if alive else "unhealthy")
    assert row["runtime_state"] == ("live" if alive else "dead")
    assert row["reason_code"] == ("ok" if alive else "runner-not-live")


@pytest.mark.parametrize(
    "process_observation, expected_health, expected_reason",
    [
        (("R", "9001"), "healthy", "ok"),
        (("Z", "9001"), "unhealthy", "runner-not-live"),
        (PermissionError("procfs denied"), "unknown", "runtime-liveness-unconfirmed"),
    ],
)
def test_worker_rpc_process_liveness_reaches_health_as_typed_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    process_observation: tuple[str, str] | BaseException,
    expected_health: str, expected_reason: str,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    outer = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    inner = worker_lib.AgentRecord(
        name="worker", harness="codex", backend="tmux",
        tmux_target="workers:worker", cwd=str(tmp_path), model=None,
        session_id="native-thread", status="idle", runner_pid=4242,
        runner_started_at="9001", runner_identity=RUNNER_IDENTITY, next_seq=0,
        created_at="2026-09-24T00:00:00+00:00", last_turn_at=None,
        owner_token=cast(str, outer["token"]),
    )
    monkeypatch.setattr(worker_lib, "read_registry", lambda: {"worker": inner})
    monkeypatch.setattr(
        worker_lib, "reconcile_automation_pause",
        lambda name, owner_token, paused: (
            None if (name, owner_token, paused) == ("worker", outer["token"], False)
            else pytest.fail("unexpected pause reconciliation")
        ),
    )
    monkeypatch.setattr(worker_lib, "window_exists", lambda _record: True)
    monkeypatch.setattr(worker_lib, "pending_count", lambda _name: 0)
    monkeypatch.setattr(worker_lib, "transcript_path", lambda _name: tmp_path / "turn.log")
    monkeypatch.setattr(worker_lib, "last_message_preview", lambda _name: "")

    def read_process(_pid: int) -> tuple[str, str]:
        if isinstance(process_observation, BaseException):
            raise process_observation
        return process_observation

    monkeypatch.setattr(worker_lib, "_read_process_state_start", read_process)
    if not isinstance(process_observation, BaseException) and process_observation[0] not in ("Z", "X", "x"):
        monkeypatch.setattr(
            HerdrClient,
            "_process_identity",
            staticmethod(lambda _pid: (RUNNER_IDENTITY, RUNNER_IDENTITY.pid)),
        )

    def dispatch(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert record.name == "worker" and action == "status"
        return worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2",
            "action": action,
            "name": record.name,
            "owner_token": record.token,
            "desired_paused": record.paused,
        })

    monkeypatch.setattr(sessions, "_worker", dispatch)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == expected_health
    assert row["reason_code"] == expected_reason
    assert row["runtime_state"] == (
        "live" if expected_health == "healthy"
        else "dead" if expected_health == "unhealthy" else "unknown"
    )


def test_headless_dead_receipt_for_another_runner_is_unknown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    saved = replace(
        sessions.get("worker"), runner_pid=4242, runner_started_at="9001",
    )
    sessions._save(saved)

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        return {
            "record": {
                "name": record.name, "runner_pid": 4242,
                "runner_started_at": "9002", "session_id": "native-thread",
                "owner_token": record.token,
            },
            "result": {"agents": [{
                "name": record.name, "runner_pid": 4242,
                "runner_started_at": "9002", "runner_alive": False,
            }]},
        }

    monkeypatch.setattr(sessions, "_worker", status)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == "unknown"
    assert row["runtime_state"] == "unknown"
    assert row["reason_code"] == "runtime-liveness-unconfirmed"


def test_headless_boolean_pid_cannot_match_saved_runner_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    saved_identity = replace(RUNNER_IDENTITY, pid=1)
    saved = replace(
        sessions.get("worker"), runner_pid=1, runner_started_at="9001",
        runner_identity=saved_identity,
    )
    sessions._save(saved)

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        identity = {
            "name": record.name, "runner_pid": True, "runner_started_at": "9001",
        }
        return {
            "record": {**identity, "owner_token": record.token},
            "result": {"agents": [{**identity, "runner_alive": False}]},
        }

    monkeypatch.setattr(sessions, "_worker", status)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == "unknown"
    assert row["runtime_state"] == "unknown"
    assert row["reason_code"] == "runtime-probe-failed"


def test_worker_rpc_preserves_typed_remote_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation", "codex", str(tmp_path), 1.0,
        adapter="turn-runner", mode="headless", runtime_home=str(tmp_path / "runtime"),
    )
    response = {
        "schema": "agentctl-worker-rpc/v2", "action": "status",
        "owner_token": record.token, "ok": False,
        "error": {"code": "unknown_agent", "message": "runner is not alive"},
    }
    monkeypatch.setattr(
        subprocess, "run",
        lambda *args, **kwargs: CompletedProcess(args[0], 1, json.dumps(response), ""),
    )
    with pytest.raises(WorkerRpcError) as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "runtime-error"
    assert raised.value.remote_code == "unknown_agent"
    assert str(raised.value) == "runner is not alive"


def test_worker_rpc_rejects_a_receipt_for_another_owner_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation", "codex", str(tmp_path), 1.0,
        adapter="turn-runner", mode="headless", runtime_home=str(tmp_path / "runtime"),
    )
    response = {
        "schema": "agentctl-worker-rpc/v2", "action": "status",
        "owner_token": "replacement-generation", "ok": True,
        "payload": {"record": None, "result": {}},
    }
    monkeypatch.setattr(
        subprocess, "run",
        lambda *args, **kwargs: CompletedProcess(args[0], 0, json.dumps(response), ""),
    )
    with pytest.raises(WorkerRpcError, match="invalid typed envelope") as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "invalid-receipt"


def test_lost_start_receipt_is_reconciled_without_a_second_launch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    calls: list[str] = []

    def lost_start(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        calls.append(action)
        raise WorkerRpcError("timeout", "receipt was lost")

    monkeypatch.setattr(sessions, "_worker", lost_start)
    with pytest.raises(WorkerRpcError, match="receipt was lost"):
        sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    saved = sessions.get("worker")
    assert saved.lifecycle == "starting"
    assert calls == ["start"]

    def recovered(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        calls.append(action)
        assert action == "status"
        identity = {
            "name": record.name, "runner_pid": 4242, "runner_started_at": "9001",
            "runner_identity": RUNNER_IDENTITY_JSON,
        }
        return {
            "record": {
                **identity, "owner_token": record.token, "mode": "headless",
                "backend": record.backend, "session_id": "native-thread",
            },
            "result": {"agents": [{**identity, "runner_alive": True}]},
        }

    monkeypatch.setattr(sessions, "_worker", recovered)
    status = sessions.status("worker")
    assert status["lifecycle"] == "running"
    assert sessions.get("worker").error is None
    assert calls == ["start", "status"]


def test_lost_pause_receipt_leaves_durable_intent_for_reconciliation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")

    def lost_pause(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "pause" and record.paused
        raise WorkerRpcError("timeout", "pause receipt was lost")

    monkeypatch.setattr(sessions, "_worker", lost_pause)
    with pytest.raises(WorkerRpcError, match="pause receipt was lost"):
        sessions.pause("worker")
    assert sessions.get("worker").paused

    reconciled: list[bool] = []

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        reconciled.append(record.paused)
        identity = {
            "name": record.name, "runner_pid": 4242, "runner_started_at": "9001",
            "runner_identity": RUNNER_IDENTITY_JSON,
        }
        return {
            "record": {**identity, "owner_token": record.token},
            "result": {"agents": [{**identity, "runner_alive": True}]},
        }

    monkeypatch.setattr(sessions, "_worker", status)
    sessions.status("worker")
    assert reconciled == [True]


def test_lost_stop_receipt_reconciles_inner_and_outer_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    started = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    outer = sessions.get("worker")
    assert outer.runtime_home is not None
    runtime = Path(outer.runtime_home)
    monkeypatch.setattr(worker_lib, "BASE", runtime)
    monkeypatch.setattr(worker_lib, "STATE", runtime / "state")
    monkeypatch.setattr(worker_lib, "ARCHIVE", runtime / "state/_archive")
    monkeypatch.setattr(worker_lib, "REGISTRY", runtime / "registry.json")
    monkeypatch.setattr(worker_lib, "LOCKFILE", runtime / ".registry.lock")
    monkeypatch.setattr(worker_lib, "EVENT_LOG", runtime / "state/events.jsonl")
    monkeypatch.setattr(worker_lib, "EVENT_LOCKFILE", runtime / "state/.events.lock")
    worker_lib.ensure_agent_dirs("worker")
    with worker_lib.registry_lock() as agents:
        agents["worker"] = worker_lib.AgentRecord(
            name="worker", harness="agy", backend="tmux",
            tmux_target="subagents:worker", cwd=str(tmp_path), model=None,
            session_id=None, status="idle", runner_pid=None,
            runner_started_at=None, next_seq=0, created_at=worker_lib.now_iso(),
            last_turn_at=None, owner_token=str(started["token"]),
        )
    monkeypatch.setattr(worker_lib, "window_exists", lambda _rec: False)
    attempts = 0

    def worker(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        nonlocal attempts
        assert action == "stop"
        attempts += 1
        payload = worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2", "action": "stop",
            "name": record.name, "owner_token": record.token,
            "desired_paused": record.paused,
        })
        if attempts == 1:
            raise WorkerRpcError("timeout", "committed stop receipt was lost")
        return payload

    monkeypatch.setattr(sessions, "_worker", worker)
    with pytest.raises(WorkerRpcError, match="receipt was lost"):
        sessions.stop("worker", expected_token=str(started["token"]))
    assert sessions.get("worker").lifecycle == "stopping"

    stopped = sessions.stop("worker", expected_token=str(started["token"]))
    assert attempts == 2
    assert not (sessions.registry / "worker").exists()
    assert Path(str(stopped["archive"])).is_dir()
    runtime_result = cast(
        dict[str, object], cast(dict[str, object], stopped["runtime"])["result"],
    )
    assert str(runtime_result["archived_to"]).startswith(
        str(stopped["archive"])
    )


def test_lost_outer_stop_receipt_reconciles_exact_archived_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    started = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    token = str(started["token"])
    publish = sessions._publish_pinned_directory
    published = False

    def lose_receipt(*args: object, **kwargs: object) -> None:
        nonlocal published
        publish(*args, **kwargs)  # type: ignore[arg-type]
        published = True
        raise OSError("simulated caller loss after outer archive publication")

    monkeypatch.setattr(sessions, "_publish_pinned_directory", lose_receipt)
    with pytest.raises(OSError, match="caller loss"):
        sessions.stop("worker", expected_token=token)
    assert published
    assert not (sessions.registry / "worker").exists()

    monkeypatch.setattr(sessions, "_publish_pinned_directory", publish)
    reconciled = sessions.stop("worker", expected_token=token)
    destination = sessions.registry / "archive" / f"worker-{token}"
    assert reconciled["archive"] == str(destination)
    assert destination.is_dir()
    assert json.loads((destination / "agent.json").read_text())["lifecycle"] == "stopped"


def test_outer_stop_reconciliation_requires_exact_token_and_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    started = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    token = str(started["token"])
    stopped = sessions.stop("worker", expected_token=token)
    destination = Path(str(stopped["archive"]))

    with pytest.raises(AgentDeliveryError, match="unknown agent"):
        sessions.stop("worker")
    with pytest.raises(AgentDeliveryError, match="inspect archived agent generation"):
        sessions.stop("worker", expected_token="b" * 32)

    receipt = json.loads((destination / "stop-result.json").read_text())
    receipt["token"] = "c" * 32
    (destination / "stop-result.json").write_text(json.dumps(receipt), encoding="utf-8")
    with pytest.raises(AgentDeliveryError, match="does not match"):
        sessions.stop("worker", expected_token=token)


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
