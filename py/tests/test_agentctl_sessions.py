"""Unified registry and adapter boundaries, independent of terminal-server availability."""
from __future__ import annotations

import fcntl
import errno
import hashlib
import json
import os
import subprocess
import sys
import threading
from dataclasses import replace
from pathlib import Path
from subprocess import CompletedProcess
from typing import cast

import pytest

from agentctl import cli, mcp, worker_rpc
from agentctl.client import (
    CustomProcessIdentity, HerdrClient, Pane, _bounded_control_command,
)
from agentctl.errors import AgentDeliveryError
from agentctl.foreign import lib as worker_lib
from agentctl.launch_contract import RuntimeControl, RuntimeLaunchContract
from agentctl.profiles import validate_muse_headless_arguments
from agentctl.sessions import Sessions, WorkerRpcError
from agentctl.subagents import AgentRecord, LaunchSpec
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
MIGRATED_RUNNER_IDENTITY = replace(
    RUNNER_IDENTITY, pid=4343, starttime_ticks=9101, executable_inode=5,
)
MIGRATED_RUNNER_IDENTITY_JSON = {
    "version": MIGRATED_RUNNER_IDENTITY.version,
    "boot_id": MIGRATED_RUNNER_IDENTITY.boot_id,
    "pid": MIGRATED_RUNNER_IDENTITY.pid,
    "starttime_ticks": MIGRATED_RUNNER_IDENTITY.starttime_ticks,
    "executable_device": MIGRATED_RUNNER_IDENTITY.executable_device,
    "executable_inode": MIGRATED_RUNNER_IDENTITY.executable_inode,
}


def runtime_receipt(record: AgentRecord, **values: object) -> dict[str, object]:
    """Build the worker projection from the outer launch authority."""
    result: dict[str, object] = {
        "name": record.name,
        "control": RuntimeControl.outer_session(record.token).to_document(),
        "harness": record.launch.harness,
        "cwd": record.launch.cwd,
        "model": record.launch.model,
        "mode": "headless",
        "backend": record.launch.backend,
        "tmux_target": "workers:worker",
        "harness_args": record.arguments,
        "codex_bypass_permissions": record.launch.permission_mode == "bypass",
        "launch": launch_contract(record).to_document(),
    }
    result.update(values)
    return result


def launch_contract(record: AgentRecord) -> RuntimeLaunchContract:
    return RuntimeLaunchContract.create(
        cwd=record.launch.cwd,
        harness=record.launch.harness,
        model=record.launch.model,
        backend=record.launch.backend,
        mode=record.launch.mode,
        harness_args=record.arguments,
        permission_mode=record.launch.permission_mode or "native",
        runtime_home=record.launch.runtime_home or "",
    )


def setup(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Sessions, FakeManagedClient, list[str]]:
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    fake = FakeManagedClient()
    sessions = Sessions(cast(HerdrClient, fake), tmp_path / "registry")
    calls: list[str] = []

    def worker(record: AgentRecord, action: str, **options: object) -> dict[str, object]:
        calls.append(action)
        if action == "stop":
            assert record.launch.runtime_home is not None
            return {
                "record": None,
                "result": {
                    "name": record.name,
                    "killed_window": False,
                    "archived_to": str(
                        Path(record.launch.runtime_home)
                        / "state/_archive" / f"{record.name}-{record.token}"
                    ),
                    "state_path": None,
                    "was_registered": True,
                    "forced": False,
                    "unverified_presentation": None,
                },
            }
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
        return {"record": {**runtime_receipt(record),
                "session_id": "native-thread", "tmux_target": "workers:worker",
                "presentation_pane": None, **identity},
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


def test_sessions_status_and_list_derive_harness_from_v4_launch_only_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session(
        "worker", cwd=str(tmp_path), mode="interactive", harness="claude",
    )
    path = sessions.registry / "worker/agent.json"
    stored = json.loads(path.read_text(encoding="utf-8"))
    assert stored["schema"] == "agentctl-session/v4"
    assert stored["launch"]["harness"] == "claude"
    assert "harness" not in stored

    status, status_healthy = sessions.status_with_health("worker")
    rows, list_healthy = sessions.list_with_health()

    assert status_healthy is True
    assert status["harness"] == "claude"
    assert list_healthy is True
    assert [(row["name"], row["harness"]) for row in rows] == [
        ("worker", "claude"),
    ]


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
            captured.append({
                "harness": record.launch.harness,
                "model": record.launch.model,
                "harness_args": record.arguments,
                **options,
            })
        return {
                "record": {
                    **runtime_receipt(record),
                    "session_id": "native-thread", "tmux_target": "workers:worker",
                    "presentation_pane": None,
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


def test_headless_herdr_attach_focuses_the_runtime_owned_tab_not_a_tui_pane(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, fake, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    fake.presentations.append(Pane("w1:headless", "workers:worker", "w1"))
    focused: list[str] = []
    monkeypatch.setattr(fake, "focus_tab", focused.append, raising=False)
    assert sessions.attach("worker")["focused"] == "tab"
    assert focused == ["workers:worker"]
    fake.presentations[0] = Pane("w1:headless", "different-tab", "w1")
    # The headless runtime owns the tab target; no TUI pane id is authoritative.
    assert sessions.attach("worker")["focused"] == "tab"
    assert focused == ["workers:worker", "workers:worker"]


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


def test_lost_migration_receipt_reconciles_from_inner_runtime_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    migrated = False

    def worker(
        record: AgentRecord, action: str, **_: object,
    ) -> dict[str, object]:
        nonlocal migrated
        if action == "migrate":
            migrated = True
            raise WorkerRpcError("timeout", "migration receipt was lost")
        assert action == "status"
        identity = (
            MIGRATED_RUNNER_IDENTITY_JSON if migrated else RUNNER_IDENTITY_JSON
        )
        return {
            "record": runtime_receipt(
                record,
                backend="tmux" if migrated else "herdr",
                tmux_target="workers:migrated" if migrated else "workers:worker",
                session_id="native-thread",
                presentation_pane=None,
                runner_pid=identity["pid"],
                runner_started_at=str(identity["starttime_ticks"]),
                runner_identity=identity,
            ),
            "result": {"agents": [{
                "name": record.name,
                "status": "idle",
                "pending": 0,
                "runner_alive": True,
                "runner_identity": identity,
            }]},
        }

    monkeypatch.setattr(sessions, "_worker", worker)
    with pytest.raises(WorkerRpcError, match="receipt was lost"):
        sessions.runtime_operation("worker", "migrate", backend="tmux")

    status = sessions.status("worker")
    saved = sessions.get("worker")
    assert saved.launch.backend == "herdr"
    assert saved.runner_identity == MIGRATED_RUNNER_IDENTITY
    assert status["runtime_evidence"] == {
        "schema": "agentctl-worker-runtime-evidence/v1",
        "backend": "tmux",
        "mode": "headless",
        "target": "workers:migrated",
        "session_id": "native-thread",
        "pane_id": None,
        "runner_identity": MIGRATED_RUNNER_IDENTITY_JSON,
    }


@pytest.mark.parametrize("reconciliation", ["status", "migrate"])
def test_worker_receipt_cannot_duplicate_another_native_session(
    reconciliation: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("first", cwd=str(tmp_path), mode="headless")
    second_dir = sessions.registry / "second"
    second_dir.mkdir(mode=0o700)
    second = AgentRecord(
        "second", "second-generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(second_dir / "runtime"),
            "owned", None, permission_mode="native",
        ),
        2.0, lifecycle="starting",
    )
    sessions._save(second)
    before = (second_dir / "agent.json").read_bytes()
    response = {
        "record": runtime_receipt(
            second, session_id="native-thread", runner_pid=4242,
            runner_started_at="9001", runner_identity=RUNNER_IDENTITY_JSON,
        ),
        "result": {"agents": [{
            "name": "second", "status": "idle", "pending": 0,
            "runner_pid": 4242, "runner_started_at": "9001",
            "runner_identity": RUNNER_IDENTITY_JSON, "runner_alive": True,
        }]},
    }

    if reconciliation == "status":
        with pytest.raises(AgentDeliveryError, match="already registered"):
            sessions._sync_worker_record(second, response, promote_running=True)
    else:
        monkeypatch.setattr(sessions, "_worker", lambda *_args, **_kwargs: response)
        with pytest.raises(AgentDeliveryError, match="already registered"):
            sessions.runtime_operation("second", "migrate", backend="tmux")

    assert (second_dir / "agent.json").read_bytes() == before
    assert sessions.get("first").session_value == "native-thread"
    assert sessions.get("second").session_value is None


def test_concurrent_worker_receipts_allow_one_native_session_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    registry = tmp_path / "registry"
    first = Sessions(registry=registry)
    second = Sessions(registry=registry)
    records: list[AgentRecord] = []
    for index, sessions in enumerate((first, second), start=1):
        name = f"worker-{index}"
        directory = registry / name
        directory.mkdir(parents=True, mode=0o700)
        record = AgentRecord(
            name, f"generation-{index}",
            LaunchSpec(
                "codex", str(tmp_path), "turn-runner", "headless", "herdr",
                None, None, None, ("codex",), (), str(directory / "runtime"),
                "owned", None, permission_mode="native",
            ),
            float(index), lifecycle="starting",
        )
        sessions._save(record)
        records.append(record)

    rendezvous = threading.Barrier(2)
    original_save = Sessions._save

    def synchronize_candidate_save(self: Sessions, record: AgentRecord) -> None:
        if record.session_value == "shared-native-session":
            try:
                rendezvous.wait(timeout=0.2)
            except threading.BrokenBarrierError:
                pass
        original_save(self, record)

    monkeypatch.setattr(Sessions, "_save", synchronize_candidate_save)
    outcomes: list[bool] = []
    outcome_lock = threading.Lock()

    def reconcile(sessions: Sessions, record: AgentRecord) -> None:
        response = {
            "record": runtime_receipt(
                record, session_id="shared-native-session",
                runner_pid=4242, runner_started_at="9001",
                runner_identity=RUNNER_IDENTITY_JSON,
            ),
        }
        try:
            sessions._sync_worker_record(record, response, promote_running=True)
        except AgentDeliveryError:
            success = False
        else:
            success = True
        with outcome_lock:
            outcomes.append(success)

    threads = [
        threading.Thread(target=reconcile, args=(sessions, record))
        for sessions, record in zip((first, second), records, strict=True)
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=3)
        assert not thread.is_alive()

    assert sorted(outcomes) == [False, True]
    claims = [first.get(record.name).session_value for record in records]
    assert claims.count("shared-native-session") == 1


def test_tui_migration_receipt_replaces_runner_cache_and_drives_attach(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, _calls = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    fake.presentations.append(Pane("w1:tui-pane", "w1:tui-tab", "w1"))
    focused: list[str] = []
    monkeypatch.setattr(fake, "focus_tab", focused.append, raising=False)

    def worker(
        record: AgentRecord, action: str, **_: object,
    ) -> dict[str, object]:
        assert action == "status"
        return {
            "record": runtime_receipt(
                record,
                backend="herdr",
                mode="tui",
                tmux_target="w1:tui-tab",
                session_id="native-thread",
                presentation_pane="w1:tui-pane",
            ),
            "result": {"agents": [{
                "name": record.name,
                "backend": "herdr",
                "mode": "tui",
                "presentation_pane": "w1:tui-pane",
                "status": "idle",
                "pending": 0,
                "runner_alive": True,
                "window_alive": True,
            }]},
        }

    monkeypatch.setattr(sessions, "_worker", worker)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == "healthy"
    status = sessions.status("worker")
    assert "migrate" not in cast(list[str], status["capabilities"])
    saved = sessions.get("worker")
    assert saved.launch.mode == "headless"
    assert saved.runner_identity is None
    assert saved.pane_id == "w1:tui-pane"
    assert sessions.attach("worker")["backend"] == "herdr"
    assert focused == ["w1:tui-tab"]


def test_nested_stop_receipt_path_is_derived_from_canonical_archive(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    started = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    token = str(started["token"])

    def worker(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "stop" and record.launch.runtime_home
        archived = Path(record.launch.runtime_home) / f"state/_archive/worker-{token}"
        archived.mkdir(parents=True)
        return {
            "record": None,
            "result": {
                "name": "worker", "killed_window": False,
                "archived_to": str(archived), "state_path": None,
                "was_registered": True, "forced": False,
                "unverified_presentation": None,
            },
        }

    monkeypatch.setattr(sessions, "_worker", worker)
    result = sessions.stop("worker")
    nested = cast(dict[str, object], cast(dict[str, object], result["runtime"])["result"])
    assert Path(str(nested["archived_to"])).is_dir()
    assert nested["state_path"] is None
    assert Path(str(nested["archived_to"])).is_relative_to(Path(str(result["archive"])))
    receipt = json.loads(
        (Path(str(result["archive"])) / "terminal-retirement.json").read_text()
    )
    assert set(receipt) == {"schema", "record_sha256"}
    stopped = json.loads(
        (Path(str(result["archive"])) / "agent.json").read_text()
    )
    assert stopped["terminal"]["outcome"] == "turn-runner-stopped"
    assert stopped["terminal"]["evidence"] == {"killed_window": False}


@pytest.mark.parametrize("liveness", [False, None, "absent"])
def test_saved_idle_state_cannot_make_a_dead_or_unverifiable_runner_ready(liveness: object, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    original = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        row: dict[str, object] = {"name": record.name, "status": "idle", "pending": 0}
        if liveness != "absent":
            row["runner_alive"] = liveness
        return {"result": {"agents": [row]}, "record": runtime_receipt(
            record, session_id="native-thread",
        )}

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
            "record": runtime_receipt(record, session_id="native-thread")}

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
        sessions.get("worker"), runner_identity=RUNNER_IDENTITY,
    )
    sessions._save(saved)

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert record.token == saved.token and action == "status"
        identity = {
            "name": record.name, "runner_pid": 4242, "runner_started_at": "9001",
            "runner_identity": RUNNER_IDENTITY_JSON,
        }
        return {
            "record": runtime_receipt(
                record, **identity, session_id="native-thread",
            ),
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
    outer = sessions.start_session(
        "worker", cwd=str(tmp_path), mode="headless", backend="tmux",
    )
    assert sessions.get("worker").launch.runtime_home is not None
    monkeypatch.setattr(
        worker_lib, "BASE", Path(cast(str, sessions.get("worker").launch.runtime_home)),
    )
    inner = worker_lib.AgentRecord(
        name="worker", harness="codex", backend="tmux",
        tmux_target="workers:worker", cwd=str(tmp_path), model=None,
        session_id="native-thread", status="idle", runner_pid=4242,
        runner_started_at="9001", runner_identity=RUNNER_IDENTITY, next_seq=0,
        created_at="2026-09-24T00:00:00+00:00", last_turn_at=None,
        control=RuntimeControl.outer_session(cast(str, outer["token"])),
        owner_launch=launch_contract(sessions.get("worker")),
    )
    monkeypatch.setattr(worker_lib, "read_registry", lambda: {"worker": inner})
    monkeypatch.setattr(
        worker_lib, "verify_owner_launch",
        lambda name, owner_token, owner_launch: (
            inner if (name, owner_token, owner_launch) == (
                "worker", outer["token"],
                launch_contract(sessions.get("worker")),
            ) else pytest.fail("unexpected permission verification")
        ),
    )
    monkeypatch.setattr(
        worker_lib, "reconcile_automation_pause",
        lambda record, paused: (
            None if (record.name, record.owner_token, paused)
            == ("worker", outer["token"], False)
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
            "owner_launch": launch_contract(record).to_document(),
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
    saved_identity = replace(RUNNER_IDENTITY, pid=4242, starttime_ticks=9001)
    saved = replace(
        sessions.get("worker"), runner_identity=saved_identity,
    )
    sessions._save(saved)
    record_path = sessions.registry / "worker" / "agent.json"
    before = record_path.read_bytes()

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        replacement = {**RUNNER_IDENTITY_JSON, "starttime_ticks": 9002}
        return {
            "record": runtime_receipt(
                record, runner_pid=4242, runner_started_at="9002",
                runner_identity=replacement,
                session_id="native-thread",
            ),
            "result": {"agents": [{
                "name": record.name, "runner_pid": 4242,
                "runner_started_at": "9002", "runner_identity": replacement,
                "runner_alive": False,
            }]},
        }

    monkeypatch.setattr(sessions, "_worker", status)
    health = sessions.health(["worker"], checked_at=123.0)
    row = cast(list[dict[str, object]], health["sessions"])[0]
    assert row["health"] == "unknown"
    assert row["runtime_state"] == "unknown"
    assert row["reason_code"] == "runtime-liveness-unconfirmed"
    assert record_path.read_bytes() == before


def test_headless_boolean_pid_cannot_match_saved_runner_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _, _ = setup(tmp_path, monkeypatch)
    sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    saved_identity = replace(RUNNER_IDENTITY, pid=1)
    saved = replace(
        sessions.get("worker"), runner_identity=saved_identity,
    )
    sessions._save(saved)

    def status(record: AgentRecord, action: str, **_: object) -> dict[str, object]:
        assert action == "status"
        identity = {
            "name": record.name, "runner_pid": True, "runner_started_at": "9001",
        }
        return {
            "record": runtime_receipt(record, **identity),
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
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None,
        ),
        1.0,
    )
    response = {
        "schema": "agentctl-worker-rpc/v2", "action": "status",
        "owner_token": record.token, "ok": False,
        "error": {"code": "unknown_agent", "message": "runner is not alive"},
    }
    monkeypatch.setattr(
        "agentctl.sessions._bounded_control_command",
        lambda *args, **kwargs: CompletedProcess(args[0], 1, json.dumps(response), ""),
    )
    with pytest.raises(WorkerRpcError) as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "runtime-error"
    assert raised.value.remote_code == "unknown_agent"
    assert str(raised.value) == "runner is not alive"


def test_worker_start_rpc_contains_one_nested_launch_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "muse", str(tmp_path), "turn-runner", "headless", "tmux",
            "watermelon", None, "watermelon-profile",
            ("muse", "--reasoning-effort", "xhigh"), (),
            str(tmp_path / "runtime"), "owned", None,
            permission_mode="native",
        ),
        1.0,
    )
    captured: dict[str, object] = {}

    def control(command: list[str], **kwargs: object) -> CompletedProcess[str]:
        captured.update(kwargs)
        response = {
            "schema": "agentctl-worker-rpc/v2", "action": "start",
            "owner_token": record.token, "ok": True,
            "payload": {"record": runtime_receipt(record), "result": {}},
        }
        return CompletedProcess(command, 0, json.dumps(response), "")

    monkeypatch.setattr("agentctl.sessions._bounded_control_command", control)
    sessions._worker(record, "start", brief="first task")
    request = json.loads(cast(str, captured["input_text"]))
    assert set(request) == {
        "schema", "action", "name", "owner_token", "desired_paused",
        "owner_launch", "brief",
    }
    assert request["owner_launch"] == launch_contract(record).to_document()
    assert request["brief"] == "first task"
    assert captured["stdout_limit"] == 8 << 20
    assert captured["stderr_limit"] == 64 << 10
    assert captured["strict_utf8"] is True


def test_worker_control_preserves_invalid_utf8_as_a_protocol_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    with pytest.raises(UnicodeDecodeError):
        _bounded_control_command(
            [sys.executable, "-c", "import os; os.write(1, b'{\\\"x\\\":\\\"\\xff\\\"}')"],
            strict_utf8=True,
        )

    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None, permission_mode="native",
        ),
        1.0,
    )
    monkeypatch.setattr(
        "agentctl.sessions._bounded_control_command",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid")
        ),
    )
    with pytest.raises(WorkerRpcError) as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "invalid-receipt"


@pytest.mark.parametrize(
    "stdout",
    [
        '{"schema":"agentctl-worker-rpc/v2","schema":"duplicate"}\n',
        "[" * 70 + "0" + "]" * 70,
    ],
)
def test_committed_start_with_ambiguous_worker_receipt_stays_reconcilable(
    stdout: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    monkeypatch.setattr(
        "agentctl.sessions._bounded_control_command",
        lambda command, **_kwargs: CompletedProcess(command, 0, stdout, ""),
    )
    with pytest.raises(WorkerRpcError) as raised:
        sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    assert raised.value.kind == "invalid-receipt"
    saved = sessions.get("worker")
    assert saved.lifecycle == "starting"
    assert saved.launch.adapter == "turn-runner"


def test_committed_start_with_oversized_worker_output_stays_reconcilable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    monkeypatch.setattr(
        "agentctl.sessions._bounded_control_command",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            OSError(errno.EFBIG, "worker stdout exceeds bound")
        ),
    )
    with pytest.raises(WorkerRpcError) as raised:
        sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    assert raised.value.kind == "invalid-receipt"
    assert sessions.get("worker").lifecycle == "starting"


def test_oversized_worker_request_refuses_before_spawning(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "muse", str(tmp_path), "turn-runner", "headless", "tmux",
            None, None, None, ("muse", "x" * (1 << 20)), (),
            str(tmp_path / "runtime"), "owned", None,
            permission_mode="native",
        ),
        1.0,
    )
    called = False

    def control(*_args: object, **_kwargs: object) -> CompletedProcess[str]:
        nonlocal called
        called = True
        return CompletedProcess([], 0, "", "")

    monkeypatch.setattr("agentctl.sessions._bounded_control_command", control)
    with pytest.raises(WorkerRpcError, match="request exceeds") as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "request-too-large"
    assert not called


@pytest.mark.parametrize("channel", ["stdout", "stderr"])
def test_non_start_worker_operation_bounds_all_diagnostic_channels(
    channel: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None, permission_mode="native",
        ),
        1.0,
    )

    def oversized(_command: object, **options: object) -> CompletedProcess[str]:
        assert options["stdout_limit"] == 8 << 20
        assert options["stderr_limit"] == 64 << 10
        raise OSError(errno.EFBIG, f"worker {channel} exceeds bound")

    monkeypatch.setattr("agentctl.sessions._bounded_control_command", oversized)
    with pytest.raises(WorkerRpcError, match="bounded diagnostic") as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "invalid-receipt"


def test_worker_rpc_rejects_a_receipt_for_another_owner_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None,
        ),
        1.0,
    )
    response = {
        "schema": "agentctl-worker-rpc/v2", "action": "status",
        "owner_token": "replacement-generation", "ok": True,
        "payload": {"record": None, "result": {}},
    }
    monkeypatch.setattr(
        "agentctl.sessions._bounded_control_command",
        lambda *args, **kwargs: CompletedProcess(args[0], 0, json.dumps(response), ""),
    )
    with pytest.raises(WorkerRpcError, match="invalid typed envelope") as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "invalid-receipt"


@pytest.mark.parametrize(
    "returncode,envelope",
    [
        (0, {"ok": True, "payload": {}, "error": {"code": "x", "message": "x"}}),
        (0, {"ok": True, "payload": {}, "extra": True}),
        (1, {"ok": False, "error": {"code": "x", "message": "x"}, "payload": {}}),
        (1, {"ok": False, "error": {"code": "x", "message": "x", "extra": True}}),
    ],
)
def test_worker_rpc_rejects_contradictory_or_extended_envelopes(
    returncode: int, envelope: dict[str, object], tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None, permission_mode="native",
        ),
        1.0,
    )
    document = {
        "schema": "agentctl-worker-rpc/v2", "action": "status",
        "owner_token": record.token, **envelope,
    }
    monkeypatch.setattr(
        "agentctl.sessions._bounded_control_command",
        lambda command, **_kwargs: CompletedProcess(
            command, returncode, json.dumps(document), "",
        ),
    )
    with pytest.raises(WorkerRpcError) as raised:
        sessions._worker(record, "status")
    assert raised.value.kind == "invalid-receipt"


@pytest.mark.parametrize("mutation", ["session", "runner", "permission"])
def test_worker_receipt_validation_is_atomic_before_outer_state_mutation(
    tmp_path: Path, mutation: str,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None, permission_mode="native",
        ),
        1.0,
        session_value="prior-session",
        pane_id="prior-pane",
        runner_identity=RUNNER_IDENTITY,
    )
    runtime = runtime_receipt(
        record,
        session_id="replacement-session",
        presentation_pane=None,
        runner_pid=RUNNER_IDENTITY.pid,
        runner_started_at=str(RUNNER_IDENTITY.starttime_ticks),
        runner_identity=RUNNER_IDENTITY_JSON,
    )
    if mutation == "session":
        runtime["session_id"] = 7
    elif mutation == "runner":
        runtime["runner_started_at"] = "replacement"
    else:
        runtime["launch"] = replace(
            launch_contract(record), permission_mode="bypass",
        ).to_document()
    before = record.to_document()

    with pytest.raises(AgentDeliveryError, match="invalid|contradictory|disagrees"):
        sessions._sync_worker_record(record, {"record": runtime})

    assert record.to_document() == before


def test_incomplete_worker_receipt_cannot_erase_saved_runner_identity(
    tmp_path: Path,
) -> None:
    sessions = Sessions(registry=tmp_path / "registry")
    record = AgentRecord(
        "worker", "generation",
        LaunchSpec(
            "codex", str(tmp_path), "turn-runner", "headless", "herdr",
            None, None, None, ("codex",), (), str(tmp_path / "runtime"),
            "owned", None,
        ),
        1.0,
        runner_identity=RUNNER_IDENTITY,
    )

    sessions._sync_worker_record(record, {
        "record": runtime_receipt(record, session_id=None, presentation_pane=None),
    }, save=False)

    assert record.runner_identity == RUNNER_IDENTITY
    assert record.runner_pid == RUNNER_IDENTITY.pid
    assert record.runner_started_at == str(RUNNER_IDENTITY.starttime_ticks)


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
            "record": runtime_receipt(
                record, **identity, session_id="native-thread",
            ),
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
            "record": runtime_receipt(record, **identity),
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
    assert outer.launch.runtime_home is not None
    runtime = Path(outer.launch.runtime_home)
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
            name="worker", harness=outer.launch.harness,
            backend=outer.launch.backend,
            tmux_target="subagents:worker", cwd=str(tmp_path),
            model=outer.launch.model,
            session_id=None, status="idle", runner_pid=None,
            runner_started_at=None, next_seq=0, created_at=worker_lib.now_iso(),
            last_turn_at=None, mode="headless",
            codex_bypass_permissions=False,
            harness_args=tuple(outer.arguments),
            control=RuntimeControl.outer_session(str(started["token"])),
            owner_launch=launch_contract(outer),
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
            "owner_launch": launch_contract(record).to_document(),
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


def test_stopped_turn_runner_receipt_completes_publication_without_second_rpc(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, calls = setup(tmp_path, monkeypatch)
    started = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    token = str(started["token"])
    publish = sessions._publish_pinned_directory

    def fail_before_publish(*_args: object, **_kwargs: object) -> None:
        raise AgentDeliveryError("injected pre-publication failure")

    monkeypatch.setattr(sessions, "_publish_pinned_directory", fail_before_publish)
    with pytest.raises(AgentDeliveryError, match="pre-publication"):
        sessions.stop("worker", expected_token=token)
    active = sessions.registry / "worker"
    assert json.loads((active / "agent.json").read_text())["lifecycle"] == "stopped"
    assert (active / "terminal-retirement.json").is_file()
    assert calls.count("stop") == 1

    def no_second_rpc(*_args: object, **_kwargs: object) -> dict[str, object]:
        raise AssertionError("terminal receipt recovery re-ran the worker stop")

    monkeypatch.setattr(sessions, "_publish_pinned_directory", publish)
    monkeypatch.setattr(sessions, "_worker", no_second_rpc)
    recovered = sessions.stop("worker", expected_token=token)

    assert Path(str(recovered["archive"])).is_dir()
    assert not active.exists()
    assert calls.count("stop") == 1


def test_active_legacy_turn_runner_receipt_migrates_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)
    started = sessions.start_session("worker", cwd=str(tmp_path), mode="headless")
    token = str(started["token"])
    active = sessions.registry / "worker"
    stored = json.loads((active / "agent.json").read_text(encoding="utf-8"))
    assert stored.pop("terminal") is None
    stored["schema"] = "agentctl-session/v3"
    stored["lifecycle"] = "stopped"
    (active / "agent.json").write_text(json.dumps(stored), encoding="utf-8")
    destination = sessions.registry / "archive" / f"worker-{token}"
    runtime_archive = destination / "runtime/state/_archive" / f"worker-{token}"
    runtime = {
        "record": None,
        "result": {
            "name": "worker", "killed_window": False,
            "archived_to": str(runtime_archive), "state_path": None,
            "was_registered": True, "forced": False,
            "unverified_presentation": None,
        },
    }
    legacy = {
        "schema": "agentctl-session-stop/v1",
        "name": "worker", "token": token,
        "record_sha256": hashlib.sha256((active / "agent.json").read_bytes()).hexdigest(),
        "result": {"name": "worker", "archive": str(destination), "runtime": runtime},
    }
    (active / "stop-result.json").write_text(
        json.dumps(legacy), encoding="utf-8",
    )
    (active / "stop-result.json").chmod(0o600)

    monkeypatch.setattr(
        sessions, "_worker",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("legacy terminal receipt re-ran the worker stop")
        ),
    )
    result = sessions.stop("worker", expected_token=token)

    assert result["archive"] == str(destination)
    assert (destination / "terminal-retirement.json").is_file()
    assert not active.exists()


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

    receipt = json.loads((destination / "terminal-retirement.json").read_text())
    receipt["record_sha256"] = "c" * 64
    (destination / "terminal-retirement.json").write_text(
        json.dumps(receipt), encoding="utf-8",
    )
    with pytest.raises(AgentDeliveryError, match="disagrees"):
        sessions.stop("worker", expected_token=token)


def test_python_sessions_reconciles_managed_dead_retirement_after_lost_response(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    sessions, fake, _calls = setup(tmp_path, monkeypatch)
    started = sessions.start_session(
        "worker", cwd=str(tmp_path), harness="muse", mode="interactive",
        workspace_id="w1",
    )
    fake.custom_running = False
    token = str(started["token"])

    first = sessions.stop("worker", expected_token=token)
    retained = list(fake.presentations)
    second = sessions.stop("worker", expected_token=token)

    assert second == first
    assert second["managed_dead"] is True
    assert second["runtime_preserved"] is True
    assert fake.presentations == retained
    assert fake.closed == []


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
