from __future__ import annotations

import sys
import contextlib
import io
import signal
import threading
import subprocess
import json
import os
import time
from dataclasses import asdict, replace
from pathlib import Path
from typing import Iterator

import pytest

from agentctl import worker_rpc
from agentctl.client import CustomProcessIdentity
from agentctl.errors import AgentDeliveryError
from agentctl.foreign import agent_runner, lib


@pytest.mark.parametrize("bypass_permissions", [False, True])
def test_packaged_runner_resumes_two_durable_turns(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch, bypass_permissions: bool,
) -> None:
    """A fresh subprocess imports the installed module and consumes the same state."""
    rec = _install_agy_agent(fake_runner_state, "portable")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = "codex"
        agents[rec.name].next_seq = 0
        agents[rec.name].session_id = None
        agents[rec.name].codex_bypass_permissions = bypass_permissions
    executable = fake_runner_state / "fake-codex"
    executable.write_text(
        f"#!{sys.executable}\n"
        "import json, sys\nfrom pathlib import Path\n"
        "prompt = sys.stdin.read()\n"
        "with Path(sys.argv[0] + '.calls').open('a') as f:\n"
        "    f.write(json.dumps({'argv': sys.argv[1:], 'prompt': prompt}) + '\\n')\n"
        "Path(sys.argv[sys.argv.index('-o') + 1]).write_text('answer: ' + prompt)\n"
        "print(json.dumps({'type': 'thread.started', 'thread_id': 'session-portable'}))\n"
    )
    executable.chmod(0o755)
    monkeypatch.setenv("SUBAGENT_EFFORT", "high")
    assert lib.enqueue_message(rec.name, "first\nline", model="model-a") == 0
    monkeypatch.setenv("SUBAGENT_EFFORT", "low")
    assert lib.enqueue_message(rec.name, "second", model=None) == 1
    env = os.environ.copy()
    env.pop("HERDR_SUBAGENTS_POLICY", None)
    env.pop("PYTHONPATH", None)
    env["HERDR_SUBAGENTS_HOME"] = str(fake_runner_state)
    env["CODEX_BIN"] = str(executable)
    env["SUBAGENTS_CODEX_BYPASS_PERMISSIONS"] = "0" if bypass_permissions else "1"
    proc = subprocess.Popen(
        [sys.executable, str(Path(agent_runner.__file__)), rec.name],
        cwd=fake_runner_state, env=env, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 10
        second = lib.processed_dir(rec.name) / "000000000001.json"
        while not second.exists() and proc.poll() is None and time.monotonic() < deadline:
            time.sleep(0.05)
        assert second.exists(), f"runner failed to consume turns, return code {proc.poll()}"
    finally:
        proc.terminate()
        stdout, stderr = proc.communicate(timeout=5)
    assert proc.returncode == 0, (stdout, stderr)
    assert lib.read_registry()[rec.name].session_id == "session-portable"
    calls = [json.loads(line) for line in Path(str(executable) + ".calls").read_text().splitlines()]
    assert calls[0]["prompt"] == "first\nline"
    assert calls[1]["argv"][:3] == ["exec", "resume", "session-portable"]
    assert 'model_reasoning_effort="high"' in calls[0]["argv"]
    assert 'model_reasoning_effort="low"' in calls[1]["argv"]
    assert all(("--dangerously-bypass-approvals-and-sandbox" in call["argv"]) is bypass_permissions for call in calls)
    transcript = lib.transcript_path(rec.name).read_text()
    assert "===TURN-DONE 0 rc=0" in transcript
    assert "===TURN-DONE 1 rc=0" in transcript


def test_harness_defaults_are_not_overridden(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    rec = _install_agy_agent(fake_runner_state, "defaults")
    rec.model = None
    monkeypatch.delenv("SUBAGENT_EFFORT", raising=False)
    argv = agent_runner._build_codex_argv(
        rec, lib.Message(0, "test", None, lib.now_iso())
    )
    assert "-m" not in argv
    assert "-c" not in argv


def _run_fake_muse_turn(
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    rec: lib.AgentRecord,
    events: list[dict[str, object]],
    *,
    returncode: int = 0,
    stdout: object | None = None,
) -> list[str]:
    launched: list[str] = []

    class FakePopen:
        def __init__(self, argv: list[str]) -> None:
            launched.extend(argv)
            self.stdout = (
                stdout
                if stdout is not None
                else io.StringIO("".join(json.dumps(event) + "\n" for event in events))
            )
            self.stderr = _EmptyReader()
            self.returncode = returncode
            self.pid = os.getpid()

        def wait(self) -> int:
            return self.returncode

        def poll(self) -> int | None:
            return self.returncode

    class _EmptyReader:
        def read(self, _size: int) -> str:
            return ""

    monkeypatch.setattr(agent_runner, "_spawn_harness", lambda _name, argv, _cwd: FakePopen(argv))
    monkeypatch.setattr(agent_runner, "_owned_harness", lambda _name, _proc: contextlib.nullcontext(threading.Event()))
    agent_runner._run_muse_turn(
        name, rec, lib.Message(seq=0, text='literal $(prompt) "quoted"', model=None, queued_at=lib.now_iso())
    )
    return launched


def _muse_event(session: str, sequence: int, payload_type: str, payload: dict[str, object]) -> dict[str, object]:
    return {
        "schema_version": 1,
        "stream": {"kind": "session", "id": session},
        "sequence": sequence,
        "payload_type": payload_type,
        "payload": payload,
    }


def _muse_workspace_epilogue(
    session: str,
    sequence: int,
    command_id: str,
    *,
    extra_payload: dict[str, object] | None = None,
) -> dict[str, object]:
    payload: dict[str, object] = {
        "command_id": command_id,
        "kind": "workspace_branch_observed",
        "record": {
            "command_id": command_id,
            "commit": "feffed69e553",
            "dirty": True,
            "reference": {"kind": "branch", "name": "main"},
            "vcs": "git",
            "workspace_root": "/work/project",
        },
    }
    if extra_payload:
        payload.update(extra_payload)
    event = _muse_event(
        session,
        sequence,
        "session.workspace_branch.observed",
        payload,
    )
    event.update(
        {
            "id": "018f0000-0000-7000-8000-00000000c350",
            "recorded_at": 1_790_114_640_260_776,
            "record_type": "event",
            "durability": "durable",
            "causation_id": command_id,
            "payload_schema_version": 1,
        }
    )
    return event


def test_muse_headless_uses_stable_session_and_requires_terminal_receipt(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "muse-worker"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    rec.model = "watermelon-model"
    rec.harness_args = ("--reasoning-effort", "ultra", 'literal $(unexpanded) "quotes"')
    with lib.registry_lock() as agents:
        agents[name] = rec
    session = rec.session_id or ""
    argv = _run_fake_muse_turn(monkeypatch, name, rec, [
        _muse_event(session, 1, "runtime.command.accepted", {
            "command_kind": "turn.submit", "command_id": "command-1",
        }),
        _muse_event(session, 2, "run.terminal.completed", {
            "command_id": "command-1", "terminal": "completed", "text": "muse answer",
        }),
    ])
    assert argv == [
        lib.MUSE_BIN, "exec", "--reasoning-effort", "ultra", 'literal $(unexpanded) "quotes"',
        "--model", "watermelon-model", "--json", "--session-id", session,
        "--", 'literal $(prompt) "quoted"',
    ]
    later = agent_runner._build_muse_argv(
        rec, lib.Message(seq=1, text="second turn", model=None, queued_at=lib.now_iso())
    )
    assert later == [
        lib.MUSE_BIN, "exec", "--reasoning-effort", "ultra", 'literal $(unexpanded) "quotes"',
        "--model", "watermelon-model", "--json", "--session-id", session, "--", "second turn",
    ]
    assert "resume" not in later
    assert lib.last_message_path(name).read_text() == "muse answer"
    assert "===TURN-DONE 0 rc=0 " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "idle"


@pytest.mark.parametrize("prompt", ["--help", "--disable-shell", "-mattacker-model"])
def test_muse_headless_terminates_options_before_literal_prompt(
    fake_runner_state: Path, prompt: str,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "muse-literal-prompt")
    rec.harness = "muse"

    argv = agent_runner._build_muse_argv(
        rec, lib.Message(seq=0, text=prompt, model=None, queued_at=lib.now_iso())
    )

    assert argv[-2:] == ["--", prompt]


def test_muse_headless_rejects_cross_session_or_missing_terminal_events(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "muse-invalid"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    with lib.registry_lock() as agents:
        agents[name] = rec
    _run_fake_muse_turn(monkeypatch, name, rec, [
        _muse_event("different-session", 1, "runtime.command.accepted", {"command_kind": "turn.submit"}),
    ])
    assert "Muse event session identity changed" in lib.last_message_path(name).read_text()
    assert "===TURN-DONE 0 rc=protocol_error " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "error"


def test_muse_headless_binds_terminal_to_exact_accepted_command(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "muse-command-mismatch"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    with lib.registry_lock() as agents:
        agents[name] = rec
    session = rec.session_id or ""
    _run_fake_muse_turn(monkeypatch, name, rec, [
        _muse_event(session, 1, "runtime.command.accepted", {
            "command_kind": "turn.submit", "command_id": "accepted-command",
        }),
        _muse_event(session, 2, "run.terminal.completed", {
            "command_id": "other-command", "terminal": "completed", "text": "wrong answer",
        }),
    ])
    assert "terminal outcome does not match" in lib.last_message_path(name).read_text()
    assert "===TURN-DONE 0 rc=protocol_error " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "error"


def test_muse_headless_allows_one_bound_workspace_observation_after_terminal(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "muse-workspace-epilogue"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    with lib.registry_lock() as agents:
        agents[name] = rec
    session = rec.session_id or ""
    command_id = "accepted-command"

    _run_fake_muse_turn(monkeypatch, name, rec, [
        _muse_event(session, 34, "runtime.command.accepted", {
            "command_kind": "turn.submit", "command_id": command_id,
        }),
        _muse_event(session, 35, "run.terminal.completed", {
            "command_id": command_id, "terminal": "completed", "text": "verified answer",
        }),
        _muse_workspace_epilogue(session, 36, command_id),
    ])

    assert lib.last_message_path(name).read_text() == "verified answer"
    assert "===TURN-DONE 0 rc=0 " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "idle"


@pytest.mark.parametrize("post_terminal", ["second-terminal", "mutated-epilogue"])
def test_muse_headless_rejects_semantic_events_after_terminal_and_labels_output_unverified(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
    post_terminal: str,
) -> None:
    name = f"muse-post-terminal-{post_terminal}"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    with lib.registry_lock() as agents:
        agents[name] = rec
    session = rec.session_id or ""
    command_id = "accepted-command"
    extra = (
        _muse_event(session, 3, "run.terminal.completed", {
            "command_id": command_id, "terminal": "completed", "text": "second answer",
        })
        if post_terminal == "second-terminal"
        else _muse_workspace_epilogue(
            session, 3, command_id, extra_payload={"text": "semantic mutation"}
        )
    )

    _run_fake_muse_turn(monkeypatch, name, rec, [
        _muse_event(session, 1, "runtime.command.accepted", {
            "command_kind": "turn.submit", "command_id": command_id,
        }),
        _muse_event(session, 2, "run.terminal.completed", {
            "command_id": command_id, "terminal": "completed", "text": "first answer",
        }),
        extra,
    ])

    last_message = lib.last_message_path(name).read_text()
    assert last_message.startswith("[MUSE ERROR] Muse emitted an unexpected event")
    assert "[MUSE UNVERIFIED OUTPUT]\nfirst answer" in last_message
    assert "second answer" not in last_message
    assert "semantic mutation" not in last_message
    assert "===TURN-DONE 0 rc=protocol_error " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "error"


@pytest.mark.parametrize(
    "harness_args",
    [
        ("--json",),
        ("--json=true",),
        ("--model=attacker-selected",),
        ("--prompt-file=/tmp/attacker-prompt",),
        ("--session-id=00000000-0000-0000-0000-000000000000",),
        ("exec",),
        ("resume",),
    ],
)
def test_muse_headless_defensively_rejects_runner_owned_arguments(
    fake_runner_state: Path,
    harness_args: tuple[str, ...],
) -> None:
    rec = _install_agy_agent(fake_runner_state, "muse-owned-arguments")
    rec.harness = "muse"
    rec.harness_args = harness_args

    with pytest.raises(ValueError, match="cannot override runner-owned argument"):
        agent_runner._build_muse_argv(
            rec,
            lib.Message(seq=0, text="trusted prompt", model=None, queued_at=lib.now_iso()),
        )


def test_muse_headless_turn_reports_unicode_decode_failure_as_protocol_error(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    class InvalidUtf8Reader:
        def readline(self, _size: int = -1) -> str:
            raise UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid start byte")

    name = "muse-invalid-utf8"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    with lib.registry_lock() as agents:
        agents[name] = rec

    _run_fake_muse_turn(
        monkeypatch,
        name,
        rec,
        [],
        stdout=InvalidUtf8Reader(),
    )

    assert "Muse JSONL Unicode decode failure" in lib.last_message_path(name).read_text()
    assert "===TURN-DONE 0 rc=protocol_error " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "error"
    assert "Traceback" not in capsys.readouterr().err


def test_muse_headless_turn_reports_pathologically_deep_json_as_protocol_error(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "muse-deep-json"
    rec = _install_agy_agent(fake_runner_state, name)
    rec.harness = "muse"
    with lib.registry_lock() as agents:
        agents[name] = rec
    deeply_nested_json = "[" * 10_000 + "0" + "]" * 10_000 + "\n"

    _run_fake_muse_turn(
        monkeypatch,
        name,
        rec,
        [],
        stdout=io.StringIO(deeply_nested_json),
    )

    assert "Muse emitted invalid JSONL" in lib.last_message_path(name).read_text()
    assert "maximum recursion depth" in lib.last_message_path(name).read_text()
    assert "===TURN-DONE 0 rc=protocol_error " in lib.transcript_path(name).read_text()
    assert lib.read_registry()[name].status == "error"


QUOTA_EXHAUSTED_LOG = (
    "E0708 06:27:57.131317 1365693 log.go:398] agent executor error: "
    "model unreachable: RESOURCE_EXHAUSTED (code 429): Individual quota reached. "
    "Please upgrade your subscription to increase your limits. Resets in 120h44m10s.: "
    "RESOURCE_EXHAUSTED (code 429): Individual quota reached. Please upgrade your "
    "subscription to increase your limits. Resets in 120h44m10s.\n"
)

AUTH_ERROR_LOG = (
    "E0706 10:23:00.480595 103808 server.go:645] Failed to get OAuth token: "
    "error getting token source from auth provider: You are not logged into Antigravity.\n"
)

NORMAL_AGY_LOG = (
    'I0708 06:27:52.910049 1365693 model.go:42] Propagating selected model override '
    'to backend: label="Example Model"\n'
)


@pytest.fixture()
def fake_runner_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    base = tmp_path / "subagents"
    state = base / "state"
    monkeypatch.setattr(lib, "BASE", base)
    monkeypatch.setattr(lib, "STATE", state)
    monkeypatch.setattr(lib, "ARCHIVE", state / "_archive")
    monkeypatch.setattr(lib, "REGISTRY", base / "registry.json")
    monkeypatch.setattr(lib, "LOCKFILE", base / ".registry.lock")
    monkeypatch.setattr(lib, "EVENT_LOG", state / "events.jsonl")
    monkeypatch.setattr(lib, "EVENT_LOCKFILE", state / ".events.lock")
    monkeypatch.setattr(lib, "RUNNER", base / "agent_runner.py")
    monkeypatch.setattr(lib, "window_exists", lambda name: True)
    yield base


def _install_agy_agent(base: Path, name: str) -> lib.AgentRecord:
    cwd = base / "cwd"
    cwd.mkdir(parents=True, exist_ok=True)
    lib.ensure_agent_dirs(name)
    with lib.registry_lock() as agents:
        agents[name] = lib.AgentRecord(
            name=name,
            harness="agy",
            backend="tmux",
            tmux_target=f"subagents:{name}",
            cwd=str(cwd),
            model="Example Model",
            session_id="11111111-2222-3333-4444-555555555555",
            status="idle",
            runner_pid=None,
            runner_started_at=None,
            next_seq=1,
            created_at=lib.now_iso(),
            last_turn_at=None,
        )
        return agents[name]


def _run_fake_agy_turn(
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    rec: lib.AgentRecord,
    *,
    log_text: str,
    stdout_text: str = "",
    stderr_text: str = "",
    returncode: int = 0,
) -> None:
    class FakePopen:
        returncode: int

        def __init__(self, argv: list[str], **_kwargs: object) -> None:
            self.returncode = returncode
            log_path = Path(argv[argv.index("--log-file") + 1])
            log_path.write_text(log_text)

        def communicate(self) -> tuple[str, str]:
            return stdout_text, stderr_text

    monkeypatch.setattr(agent_runner, "_spawn_harness", lambda _name, argv, _cwd: FakePopen(argv))
    monkeypatch.setattr(agent_runner, "_owned_harness", lambda _name, _proc: contextlib.nullcontext(threading.Event()))
    msg = lib.Message(seq=0, text="Probe agy.", model=None, queued_at=lib.now_iso())
    agent_runner._run_agy_turn(name, rec, msg)


def test_agy_quota_log_classifies_turn_and_status(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "quota"
    rec = _install_agy_agent(fake_runner_state, name)

    _run_fake_agy_turn(monkeypatch, name, rec, log_text=QUOTA_EXHAUSTED_LOG)

    transcript = lib.transcript_path(name).read_text()
    assert "===TURN-DONE 0 rc=quota_exhausted reset_in=120h44m10s " in transcript
    assert lib.read_registry()[name].status == "quota_exhausted"
    assert "RESOURCE_EXHAUSTED (code 429)" in lib.last_message_path(name).read_text()


def test_agy_auth_log_classifies_turn_and_status(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "auth"
    rec = _install_agy_agent(fake_runner_state, name)

    _run_fake_agy_turn(monkeypatch, name, rec, log_text=AUTH_ERROR_LOG)

    transcript = lib.transcript_path(name).read_text()
    assert "===TURN-DONE 0 rc=auth_error " in transcript
    assert lib.read_registry()[name].status == "auth_error"
    assert "human re-login required" in lib.last_message_path(name).read_text()


def test_agy_success_stays_rc_zero_and_idle(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "normal"
    rec = _install_agy_agent(fake_runner_state, name)

    _run_fake_agy_turn(
        monkeypatch,
        name,
        rec,
        log_text=NORMAL_AGY_LOG,
        stdout_text="PILOT-READY\n",
    )

    transcript = lib.transcript_path(name).read_text()
    assert "===TURN-DONE 0 rc=0 " in transcript
    assert lib.read_registry()[name].status == "idle"
    assert lib.last_message_path(name).read_text() == "PILOT-READY\n"


def test_headless_completion_advances_last_turn_at(
    fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "completion-time"
    rec = _install_agy_agent(fake_runner_state, name)
    submitted_at = "2026-08-18T00:00:00+00:00"
    completed_at = "2026-08-18T00:01:00+00:00"
    with lib.registry_lock() as agents:
        agents[name].last_turn_at = submitted_at
    monkeypatch.setattr(lib, "now_iso", lambda: completed_at)

    _run_fake_agy_turn(
        monkeypatch,
        name,
        rec,
        log_text=NORMAL_AGY_LOG,
        stdout_text="PILOT-READY\n",
    )

    refreshed = lib.read_registry()[name]
    assert refreshed.status == "idle"
    assert refreshed.last_turn_at == completed_at
    assert refreshed.last_turn_at != submitted_at


def test_context_reset_clears_session_before_the_next_agy_turn(fake_runner_state: Path) -> None:
    name = "reset"
    rec = _install_agy_agent(fake_runner_state, name)

    result = lib.reset_agent_context(name)

    reset_rec = lib.read_registry()[name]
    assert result.previous_session_id == rec.session_id
    assert reset_rec.session_id is None
    argv = agent_runner._build_agy_argv(
        reset_rec,
        lib.Message(seq=1, text="Unrelated task.", model=None, queued_at=lib.now_iso()),
        fake_runner_state / "agy.log",
    )
    assert argv[0] == lib.AGY_BIN
    assert "--conversation" not in argv
    assert "=== CONTEXT RESET ===" in lib.transcript_path(name).read_text()


def test_context_reset_clears_session_before_the_next_codex_turn(fake_runner_state: Path) -> None:
    name = "reset"
    rec = _install_agy_agent(fake_runner_state, name)

    result = lib.reset_agent_context(name)

    reset_rec = lib.read_registry()[name]
    assert result.previous_session_id == rec.session_id
    assert reset_rec.session_id is None
    argv = agent_runner._build_codex_argv(
        reset_rec,
        lib.Message(seq=1, text="Unrelated task.", model=None, queued_at=lib.now_iso()),
    )
    assert argv[:2] == [lib.CODEX_BIN, "exec"]
    assert "resume" not in argv
    assert "=== CONTEXT RESET ===" in lib.transcript_path(name).read_text()


def _wait_for(path: Path, process: subprocess.Popen[str], timeout: float = 8) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists() and process.poll() is None and time.monotonic() < deadline:
        time.sleep(0.02)
    assert path.exists(), f"missing {path}; runner exit={process.poll()}"


def _runner_process(base: Path, name: str, executable: Path, *, stage: str | None = None) -> subprocess.Popen[str]:
    env = os.environ.copy()
    env.pop("HERDR_SUBAGENTS_POLICY", None)
    env.pop("SUBAGENTS_MIGRATION_STAGE", None)
    env.update(HERDR_SUBAGENTS_HOME=str(base), CODEX_BIN=str(executable), AGY_BIN=str(executable))
    if stage is not None:
        env["SUBAGENTS_MIGRATION_STAGE"] = stage
    return subprocess.Popen([sys.executable, str(Path(agent_runner.__file__)), name], cwd=base,
                            env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def _process_running(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except (FileNotFoundError, ProcessLookupError):
        return False


@pytest.mark.parametrize("harness", ["codex", "agy"])
@pytest.mark.parametrize("stop", ["signal", "force"])
def test_stop_busy_runner_terminates_owned_harness_group(fake_runner_state: Path, harness: str, stop: str) -> None:
    rec = _install_agy_agent(fake_runner_state, "busy")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = harness
    executable = fake_runner_state / "sleeping-harness"
    executable.write_text(f"#!{sys.executable}\n" +
        "import json, os, subprocess, sys, time\nfrom pathlib import Path\n"
        "if 'exec' in sys.argv: sys.stdin.read()\n"
        "child=subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\n"
        "Path(sys.argv[0]+'.ready').write_text(json.dumps([os.getpid(),child.pid]))\n"
        "time.sleep(60)\n")
    executable.chmod(0o755)
    lib.enqueue_message(rec.name, "bounded owned-process test", model=None)
    runner = _runner_process(fake_runner_state, rec.name, executable)
    pids: list[int] = []
    try:
        ready = Path(str(executable) + ".ready")
        _wait_for(ready, runner)
        pids = json.loads(ready.read_text())
        _wait_for(lib.agent_dir(rec.name) / "active-harness.json", runner)
        if stop == "signal":
            runner.terminate()
        else:
            assert lib.terminate_runner(lib.read_registry()[rec.name], grace=0)
        runner.communicate(timeout=5)
        deadline = time.monotonic() + 3
        while any(_process_running(pid) for pid in pids) and time.monotonic() < deadline:
            time.sleep(0.02)
        assert not any(_process_running(pid) for pid in pids)
    finally:
        if runner.poll() is None:
            runner.kill()
        runner.communicate(timeout=5)
        if pids:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(pids[0], signal.SIGKILL)


def test_stop_waits_for_harness_identity_publication(fake_runner_state: Path) -> None:
    rec = _install_agy_agent(fake_runner_state, "spawn")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = "codex"
    executable = fake_runner_state / "sleeping-harness"
    executable.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(60)\n")
    executable.chmod(0o755)
    lib.enqueue_message(rec.name, "owned spawn race", model=None)
    barrier, release = fake_runner_state / "barrier", fake_runner_state / "release"
    launcher = fake_runner_state / "launch.py"
    launcher.write_text(
        "import sys,time\nfrom pathlib import Path\n"
        f"sys.path.insert(0,{str(Path(agent_runner.__file__).resolve().parents[2])!r})\n"
        "from agentctl.foreign import agent_runner,lib\n"
        "record=lib.record_active_harness\n"
        "def delayed(name,runner,harness):\n"
        f"    Path({str(barrier)!r}).write_text(str(harness.pid))\n"
        f"    while not Path({str(release)!r}).exists(): time.sleep(0.01)\n"
        "    record(name,runner,harness)\n"
        "lib.record_active_harness=delayed\n"
        "raise SystemExit(agent_runner.main())\n"
    )
    env = os.environ.copy()
    env.pop("HERDR_SUBAGENTS_POLICY", None)
    env.pop("SUBAGENTS_MIGRATION_STAGE", None)
    env.update(HERDR_SUBAGENTS_HOME=str(fake_runner_state), CODEX_BIN=str(executable))
    runner = subprocess.Popen([sys.executable, str(launcher), rec.name], env=env, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child_pid: int | None = None
    stopper: threading.Thread | None = None
    stopped: list[bool] = []
    try:
        _wait_for(barrier, runner)
        child_pid = int(barrier.read_text())
        current = lib.read_registry()[rec.name]
        stopper = threading.Thread(target=lambda: stopped.append(lib.terminate_runner(current, grace=0)))
        stopper.start()
        time.sleep(0.1)
        assert stopper.is_alive(), "retirement crossed an unpublished harness identity"
        assert runner.poll() is None and _process_running(child_pid)
        release.touch()
        stopper.join(timeout=5)
        assert stopped == [True]
        runner.communicate(timeout=5)
        assert not _process_running(child_pid)
    finally:
        release.touch()
        if stopper is not None:
            stopper.join(timeout=5)
        if runner.poll() is None:
            runner.kill()
        runner.communicate(timeout=5)
        if child_pid is not None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child_pid, signal.SIGKILL)


@pytest.mark.parametrize("cleanup", ["down", "gc", "restart"])
def test_crashed_runner_owned_harness_is_cleaned_before_state_reuse(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch, cleanup: str,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "orphan")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = "codex"
    executable = fake_runner_state / "orphan-harness"
    executable.write_text(f"#!{sys.executable}\n" +
        "import os,sys,time\nfrom pathlib import Path\nsys.stdin.read()\n"
        "Path(sys.argv[0]+'.ready').write_text(str(os.getpid()))\ntime.sleep(60)\n")
    executable.chmod(0o755)
    seq = lib.enqueue_message(rec.name, "crash recovery", model=None)
    runner = _runner_process(fake_runner_state, rec.name, executable)
    replacement: subprocess.Popen[str] | None = None
    child_pid: int | None = None
    try:
        ready = Path(str(executable) + ".ready")
        _wait_for(ready, runner)
        child_pid = int(ready.read_text())
        runner.kill()
        runner.communicate(timeout=5)
        assert _process_running(child_pid)
        if cleanup == "down":
            lib.bring_down_agent(rec.name, grace=0, force=True)
            assert rec.name not in lib.read_registry()
        elif cleanup == "gc":
            monkeypatch.setattr(lib, "kill_window", lambda _rec: None)
            assert lib.gc()
            assert rec.name not in lib.read_registry()
        else:
            replacement = _runner_process(fake_runner_state, rec.name, executable)
            _wait_for(lib.failed_dir(rec.name) / f"{seq:012d}.json", replacement)
            assert lib.read_registry()[rec.name].runner_pid == replacement.pid
        deadline = time.monotonic() + 3
        while _process_running(child_pid) and time.monotonic() < deadline:
            time.sleep(0.02)
        assert not _process_running(child_pid)
    finally:
        if replacement is not None:
            replacement.terminate()
            replacement.communicate(timeout=5)
        if runner.poll() is None:
            runner.kill()
        runner.communicate(timeout=5)
        if child_pid is not None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child_pid, signal.SIGKILL)


@pytest.mark.parametrize("changed", ["runner_pid", "runner_started_at", "started_at", "process_group"])
def test_harness_cleanup_refuses_changed_ownership(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch, changed: str,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "ownership")
    runner = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=22, starttime_ticks=100, executable_device=3, executable_inode=4,
    )
    harness = replace(runner, pid=33, starttime_ticks=200, executable_inode=5)
    rec.runner_pid, rec.runner_started_at = runner.pid, str(runner.starttime_ticks)
    rec.runner_identity = runner
    ownership: dict[str, object] = {
        "schema": "agentctl-active-harness/v2",
        "runner": asdict(runner),
        "harness": asdict(harness),
    }
    if changed == "runner_pid":
        ownership["runner"] = asdict(replace(runner, pid=23))
    elif changed == "runner_started_at":
        ownership["runner"] = asdict(replace(runner, starttime_ticks=101))
    elif changed == "started_at":
        ownership["harness"] = asdict(replace(harness, starttime_ticks=201))
    (lib.agent_dir(rec.name) / "active-harness.json").write_text(json.dumps(ownership))
    monkeypatch.setattr(
        lib,
        "process_identity_liveness",
        lambda identity, **_kwargs: (
            lib.ProcessLiveness.LIVE
            if identity in (runner, harness)
            else lib.ProcessLiveness.DEAD
        ),
    )
    @contextlib.contextmanager
    def pinned(_identity: lib.RunnerIdentity, _operation: str) -> Iterator[int | None]:
        yield 91
    monkeypatch.setattr(lib, "_verified_pidfd", pinned)
    monkeypatch.setattr(lib, "_pidfd_signal", lambda *_args: None)
    monkeypatch.setattr(lib, "_pidfd_live", lambda _descriptor: True)
    monkeypatch.setattr(
        lib, "_read_process_state_group_start",
        lambda _pid: ("T", harness.pid, str(harness.starttime_ticks)),
    )
    monkeypatch.setattr(
        lib, "_process_group_members",
        lambda _group: (
            {harness.pid + 1: str(harness.starttime_ticks)}
            if changed == "process_group"
            else {harness.pid: str(harness.starttime_ticks)}
        ),
    )
    monkeypatch.setattr(os, "killpg", lambda *_args: pytest.fail("numeric group was signaled"))
    if changed in ("started_at", "process_group"):
        with pytest.raises(lib.AgentOperationError, match="leader"):
            lib.terminate_active_harness(rec)
    else:
        assert not lib.terminate_active_harness(rec)


def test_process_stat_parser_ignores_non_utf8_process_names(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    raw = (
        b"123 (arbitrary\xff) S 7 8 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 42\n"
    )
    monkeypatch.setattr(Path, "read_bytes", lambda _path: raw)
    assert lib._read_process_state_group_start(123) == ("S", 8, "42")
    with pytest.raises(ValueError, match="identity"):
        lib._read_process_state_group_start(124)


def test_runner_teardown_keeps_pidfd_authority_through_signal(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    identity = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=4242, starttime_ticks=9001, executable_device=3, executable_inode=4,
    )
    events: list[tuple[str, int]] = []
    live = True
    monkeypatch.setattr(os, "pidfd_open", lambda pid, flags: 77)
    monkeypatch.setattr(
        lib, "process_identity_liveness",
        lambda observed: lib.ProcessLiveness.LIVE if observed == identity else pytest.fail(
            "wrong process generation was verified"
        ),
    )
    monkeypatch.setattr(lib, "_pidfd_live", lambda descriptor: live)

    def pinned_signal(descriptor: int, signum: int) -> None:
        nonlocal live
        assert descriptor == 77
        events.append(("signal", signum))
        live = False

    monkeypatch.setattr(lib, "_pidfd_signal", pinned_signal)
    monkeypatch.setattr(
        os, "kill", lambda *_args: pytest.fail("numeric PID signaling reintroduced"),
    )
    monkeypatch.setattr(os, "close", lambda descriptor: events.append(("close", descriptor)))
    assert lib.terminate_runner_identity(identity, grace=0.0)
    assert events == [("signal", signal.SIGTERM), ("close", 77)]


def test_token_bound_stop_reconciles_archive_after_lost_receipt(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "lost-stop")
    token = lib.read_registry()[rec.name].owner_token
    assert token is not None
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)
    real_sync = lib._sync_directory
    interrupted = False

    def lose_receipt_after_archive(path: Path) -> None:
        nonlocal interrupted
        if path == lib.ARCHIVE and not interrupted:
            interrupted = True
            raise OSError("simulated process loss after archive publication")
        real_sync(path)

    monkeypatch.setattr(lib, "_sync_directory", lose_receipt_after_archive)
    with pytest.raises(OSError, match="simulated process loss"):
        lib.stop_owned_agent(rec.name, token, grace=0)

    destination = lib.ARCHIVE / f"{rec.name}-{token}"
    assert destination.is_dir()
    assert not lib.agent_dir(rec.name).exists()
    assert lib.read_registry()[rec.name].owner_token == token

    reconciled = lib.stop_owned_agent(rec.name, token, grace=0)
    assert reconciled.archived_to == str(destination)
    assert reconciled.was_registered
    assert rec.name not in lib.read_registry()


def test_token_bound_stop_never_replaces_a_racing_archive(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "stop-collision")
    token = lib.read_registry()[rec.name].owner_token
    assert token is not None
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)
    real_rename = lib.shared_rename_directory_noreplace_at

    def collide(
        source_parent: int, source_name: str,
        destination_parent: int, destination_name: str,
    ) -> None:
        os.mkdir(destination_name, mode=0o700, dir_fd=destination_parent)
        marker = os.open(
            f"{destination_name}/winner", os.O_CREAT | os.O_WRONLY, 0o600,
            dir_fd=destination_parent,
        )
        os.close(marker)
        real_rename(source_parent, source_name, destination_parent, destination_name)

    monkeypatch.setattr(lib, "shared_rename_directory_noreplace_at", collide)
    with pytest.raises(AgentDeliveryError, match="replace existing agent archive"):
        lib.stop_owned_agent(rec.name, token, grace=0)

    destination = lib.ARCHIVE / f"{rec.name}-{token}"
    assert (destination / "winner").is_file()
    assert lib.agent_dir(rec.name).is_dir()
    assert lib.read_registry()[rec.name].owner_token == token


def test_harness_group_teardown_signals_only_pinned_generations_after_final_proof(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "pinned-group")
    runner = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=4241, starttime_ticks=9000, executable_device=3, executable_inode=4,
    )
    harness = replace(runner, pid=4242, starttime_ticks=9001, executable_inode=5)
    rec.runner_pid = runner.pid
    rec.runner_started_at = str(runner.starttime_ticks)
    rec.runner_identity = runner
    (lib.agent_dir(rec.name) / "active-harness.json").write_text(json.dumps({
        "schema": "agentctl-active-harness/v2",
        "runner": asdict(runner),
        "harness": asdict(harness),
    }))
    events: list[tuple[str, int]] = []
    closed = False
    monkeypatch.setattr(
        lib, "process_identity_liveness", lambda _identity, **_kwargs: lib.ProcessLiveness.LIVE,
    )
    monkeypatch.setattr(os, "pidfd_open", lambda pid, flags: 77 if pid == harness.pid else 78)
    monkeypatch.setattr(lib, "_pidfd_live", lambda descriptor: True)
    monkeypatch.setattr(
        lib, "_read_process_state_group_start",
        lambda pid: (
            "T", harness.pid,
            str(harness.starttime_ticks if pid == harness.pid else harness.starttime_ticks + 1),
        ),
    )
    scans = 0

    def members(_group: int) -> dict[int, str]:
        nonlocal scans
        scans += 1
        return {
            harness.pid: str(harness.starttime_ticks),
            harness.pid + 1: str(harness.starttime_ticks + 1),
        }

    monkeypatch.setattr(lib, "_process_group_members", members)
    monkeypatch.setattr(
        os, "killpg",
        lambda *_args: pytest.fail("numeric PGID signaling reintroduced after final proof"),
    )
    monkeypatch.setattr(
        lib, "_pidfd_signal", lambda descriptor, signum: events.append(("pidfd", signum)),
    )

    real_close = os.close

    def close(descriptor: int) -> None:
        nonlocal closed
        if descriptor not in (77, 78):
            real_close(descriptor)
            return
        if descriptor == 77:
            closed = True
        events.append(("close", descriptor))

    monkeypatch.setattr(os, "close", close)
    assert lib.terminate_active_harness(rec)
    assert scans == 2
    assert events == [
        ("pidfd", signal.SIGSTOP),
        ("pidfd", signal.SIGSTOP),
        ("pidfd", signal.SIGKILL),
        ("pidfd", signal.SIGKILL),
        ("close", 78),
        ("close", 77),
    ]


def test_stop_marker_preserves_active_turn_grace_period(fake_runner_state: Path) -> None:
    rec = _install_agy_agent(fake_runner_state, "grace")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = "codex"
    executable = fake_runner_state / "finishing-harness"
    executable.write_text(f"#!{sys.executable}\n" +
        "import sys,time\nfrom pathlib import Path\nsys.stdin.read()\n"
        "Path(sys.argv[0]+'.ready').touch()\n"
        "while not Path(sys.argv[0]+'.finish').exists(): time.sleep(0.01)\n"
        "Path(sys.argv[sys.argv.index('-o')+1]).write_text('finished within grace')\n")
    executable.chmod(0o755)
    lib.enqueue_message(rec.name, "finish current turn", model=None)
    runner = _runner_process(fake_runner_state, rec.name, executable)
    try:
        _wait_for(Path(str(executable) + ".ready"), runner)
        lib.stop_path(rec.name).touch()
        time.sleep(0.15)
        assert runner.poll() is None
        Path(str(executable) + ".finish").touch()
        stdout, stderr = runner.communicate(timeout=5)
        assert runner.returncode == 0, (stdout, stderr)
        assert lib.last_message_path(rec.name).read_text() == "finished within grace"
    finally:
        if runner.poll() is None:
            lib.terminate_runner(lib.read_registry()[rec.name], grace=0)
        runner.communicate(timeout=5)


def test_failed_codex_turn_does_not_reuse_previous_answer(fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    rec = _install_agy_agent(fake_runner_state, "answer")
    executable = fake_runner_state / "failed-codex"
    executable.write_text(f"#!{sys.executable}\n" +
        "import json,sys\nsys.stdin.read()\n"
        "print(json.dumps({'type':'error','message':'current request failed'}))\n"
        "raise SystemExit(1)\n")
    executable.chmod(0o755)
    monkeypatch.setattr(lib, "CODEX_BIN", str(executable))
    lib.last_message_path(rec.name).write_text("previous turn answer")
    (lib.agent_dir(rec.name) / "codex-answer-000000000001.txt").write_text("older attempt")
    agent_runner._run_codex_turn(rec.name, rec, lib.Message(1, "new request", None, lib.now_iso()))
    answer = lib.last_message_path(rec.name).read_text()
    assert "current request failed" in answer
    assert "previous" not in answer and "older attempt" not in answer
    assert "===TURN-DONE 1 rc=1" in lib.transcript_path(rec.name).read_text()


def test_codex_drains_full_stderr_while_streaming_stdout(fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    rec = _install_agy_agent(fake_runner_state, "pipes")
    executable = fake_runner_state / "loud-codex"
    executable.write_text(f"#!{sys.executable}\n" +
        "import sys\nfrom pathlib import Path\nsys.stdin.read()\n"
        "sys.stderr.write('x'*1048576)\nsys.stderr.flush()\n"
        "Path(sys.argv[sys.argv.index('-o')+1]).write_text('current answer')\n")
    executable.chmod(0o755)
    monkeypatch.setattr(lib, "CODEX_BIN", str(executable))
    monkeypatch.setattr(lib, "TURN_TIMEOUT_S", 2)
    agent_runner._run_codex_turn(rec.name, rec, lib.Message(1, "request", None, lib.now_iso()))
    assert lib.read_registry()[rec.name].status == "idle"
    assert lib.last_message_path(rec.name).read_text() == "current answer"


@pytest.mark.parametrize("crash", ["before_execution", "after_execution"])
def test_headless_claim_is_quarantined_after_crash_without_replay(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch, crash: str,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "claim")
    seq = lib.enqueue_message(rec.name, "one request", model=None)
    source = lib.inbox_dir(rec.name) / f"{seq:012d}.json"
    calls: list[int] = []
    def execute(name: str, message: lib.Message) -> None:
        assert not source.exists()
        assert (lib.inflight_dir(name) / source.name).exists()
        if crash == "before_execution":
            raise RuntimeError("crash after durable claim")
        calls.append(message.seq)
    original_replace = os.replace
    def replace(source_path: str | Path, destination: str | Path) -> None:
        if Path(destination).parent == lib.processed_dir(rec.name):
            raise RuntimeError("crash after execution")
        original_replace(source_path, destination)
    monkeypatch.setattr(agent_runner, "_run_turn", execute)
    monkeypatch.setattr(os, "replace", replace)
    with pytest.raises(RuntimeError, match="crash"):
        agent_runner._consume(rec.name, rec, source)
    monkeypatch.setattr(os, "replace", original_replace)
    agent_runner._consume(rec.name, rec, source)
    assert calls == ([] if crash == "before_execution" else [seq])
    assert (lib.failed_dir(rec.name) / source.name).exists()
    error = json.loads((lib.failed_dir(rec.name) / f"{source.name}.error").read_text())
    assert error["outcome"] == "possibly_submitted"


def test_automation_pause_keeps_request_pending(fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    rec = _install_agy_agent(fake_runner_state, "paused")
    seq = lib.enqueue_message(rec.name, "wait for resume", model=None)
    source = lib.inbox_dir(rec.name) / f"{seq:012d}.json"
    lib.automation_pause_path(rec).touch()
    monkeypatch.setattr(agent_runner, "_run_turn", lambda *_args: pytest.fail("paused intake executed"))
    agent_runner._consume(rec.name, rec, source)
    assert source.exists()
    assert not list(lib.inflight_dir(rec.name).iterdir())
    assert lib.status_snapshot(rec.name, run_gc=False).agents[0].automation_paused


def test_owner_generation_binds_once_and_rejects_stale_runtime_operations(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "owned")
    first = "a" * 32
    second = "b" * 32
    legacy = rec.to_public_dict()
    legacy.pop("owner_token")
    lib.REGISTRY.write_text(json.dumps([legacy]), encoding="utf-8")
    assert lib.bind_owner_token(rec.name, first).owner_token == first
    with pytest.raises(lib.AgentOperationError, match="another session generation") as raised:
        lib.bind_owner_token(rec.name, second)
    assert raised.value.code == "owner_token_mismatch"
    assert lib.read_registry()[rec.name].owner_token == first

    called = False

    def status(*_args: object, **_kwargs: object) -> object:
        nonlocal called
        called = True
        return object()

    monkeypatch.setattr(lib, "status_snapshot", status)
    with pytest.raises(lib.AgentOperationError, match="another session generation"):
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2", "action": "status",
            "name": rec.name, "owner_token": second, "desired_paused": False,
        })
    assert not called


def test_owner_permission_policy_is_part_of_the_generation_binding(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "permission-bound")
    token = rec.owner_token
    assert token is not None
    native = lib.verify_owner_permission(rec.name, token, "native")
    assert native.codex_bypass_permissions is False

    before = lib.REGISTRY.read_bytes()
    with pytest.raises(lib.AgentOperationError, match="different permission") as raised:
        lib.verify_owner_permission(rec.name, token, "bypass")
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == before


def test_existing_worker_start_refuses_permission_policy_drift(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "permission-start")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = "codex"
        agents[rec.name].model = None
        agents[rec.name].harness_args = ()
    token = lib.read_registry()[rec.name].owner_token
    assert token is not None

    with pytest.raises(lib.AgentOperationError, match="different launch") as raised:
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2",
            "action": "start",
            "name": rec.name,
            "owner_token": token,
            "desired_paused": False,
            "permission_mode": "bypass",
            "cwd": rec.cwd,
            "harness": "codex",
            "model": None,
            "backend": rec.backend,
            "harness_args": [],
            "brief": None,
        })
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.read_registry()[rec.name].codex_bypass_permissions is False


def test_legacy_pid_start_identity_is_never_upgraded_from_current_process(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "legacy-identity")
    legacy = rec.to_public_dict()
    legacy.pop("owner_token")
    legacy.pop("runner_identity")
    legacy["runner_pid"] = os.getpid()
    legacy["runner_started_at"] = lib.pid_start_time(os.getpid())
    raw = json.dumps([legacy], sort_keys=True).encode()
    lib.REGISTRY.write_bytes(raw)
    calls = 0

    def forbidden_capture(_pid: int) -> CustomProcessIdentity:
        nonlocal calls
        calls += 1
        raise AssertionError("legacy identity must not sample the current PID occupant")

    monkeypatch.setattr(lib, "capture_process_identity", forbidden_capture)
    loaded = lib.read_registry()[rec.name]
    assert loaded.runner_identity is None
    assert lib.runner_liveness(loaded) is lib.ProcessLiveness.UNKNOWN
    assert calls == 0
    with pytest.raises(lib.AgentOperationError, match="explicit recovery") as raised:
        lib.bind_owner_token(rec.name, "a" * 32)
    assert raised.value.code == "runner_identity_incomplete"
    assert lib.REGISTRY.read_bytes() == raw


def test_binding_one_legacy_row_preserves_unrelated_untrusted_row(
    fake_runner_state: Path,
) -> None:
    first = _install_agy_agent(fake_runner_state, "first")
    second = replace(first, name="second", tmux_target="subagents:second")
    rows = []
    for record in (first, second):
        legacy = record.to_public_dict()
        legacy.pop("owner_token")
        legacy.pop("runner_identity")
        if record.name == "second":
            legacy["runner_pid"] = os.getpid()
            legacy["runner_started_at"] = lib.pid_start_time(os.getpid())
        rows.append(legacy)
    lib.REGISTRY.write_text(json.dumps(rows, sort_keys=True), encoding="utf-8")
    sidecar = lib._permission_policy_path("second")
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    sidecar_bytes = b'{"legacy":"must remain untouched"}\n'
    sidecar.write_bytes(sidecar_bytes)

    bound = lib.bind_owner_token("first", "a" * 32)

    assert bound.owner_token == "a" * 32
    stored = json.loads(lib.REGISTRY.read_text(encoding="utf-8"))
    second_stored = next(row for row in stored if row["name"] == "second")
    assert "owner_token" not in second_stored
    assert "schema" not in second_stored
    assert "runner_identity" not in second_stored
    assert sidecar.read_bytes() == sidecar_bytes


def test_worker_start_launch_mismatch_does_not_claim_or_rewrite_legacy_row(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "mismatch")
    legacy = rec.to_public_dict()
    legacy.pop("owner_token")
    legacy.pop("runner_identity")
    raw = (json.dumps([legacy], sort_keys=True) + "\n").encode()
    lib.REGISTRY.write_bytes(raw)

    with pytest.raises(lib.AgentOperationError) as raised:
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2",
            "action": "start",
            "name": rec.name,
            "owner_token": "a" * 32,
            "desired_paused": False,
            "cwd": rec.cwd,
            "harness": "codex",
            "model": rec.model,
            "backend": rec.backend,
            "harness_args": [],
            "brief": None,
        })
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == raw


def test_pause_markers_are_scoped_to_the_owner_generation(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "pause-generation")
    current_token = rec.owner_token
    assert current_token is not None
    stale_token = "d" * 32
    current = lib.bind_owner_token(rec.name, current_token)
    legacy = lib.agent_dir(rec.name) / "automation-paused"
    legacy.touch()

    lib.reconcile_automation_pause(rec.name, current_token, True)
    assert not legacy.exists()
    assert lib.automation_is_paused(current)
    stale = lib.automation_pause_path(replace(current, owner_token=stale_token))
    stale.touch()
    lib.reconcile_automation_pause(rec.name, current_token, False)
    assert not lib.automation_is_paused(current)
    assert stale.exists()


def test_activated_runner_can_acknowledge_successive_migrations(fake_runner_state: Path) -> None:
    rec = _install_agy_agent(fake_runner_state, "staged")
    token = "first-stage"
    runner = _runner_process(fake_runner_state, rec.name, fake_runner_state / "unused", stage=token)
    try:
        _wait_for(lib.staged_runner_path(rec.name, token), runner)
        identity = lib.read_staged_runner(rec.name, token)
        assert identity is not None
        with lib.registry_lock() as agents:
            agents[rec.name].runner_pid = identity.pid
            agents[rec.name].runner_started_at = str(identity.starttime_ticks)
            agents[rec.name].runner_identity = identity
        lib.staged_runner_activate_path(rec.name, token).touch()
        _wait_for(lib.staged_runner_activated_path(rec.name, token), runner)
        lib.clear_migration_markers(rec.name, token)
        for _ in range(2):
            lib.write_migration_pause(rec.name, identity)
            assert lib.wait_for_pause_ack(rec.name, identity)
            lib.clear_migration_markers(rec.name, "")
    finally:
        runner.terminate()
        stdout, stderr = runner.communicate(timeout=5)
    assert runner.returncode == 0, (stdout, stderr)
