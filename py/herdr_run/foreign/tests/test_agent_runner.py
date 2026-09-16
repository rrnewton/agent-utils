from __future__ import annotations

import sys
import subprocess
import json
import os
import time
from pathlib import Path
from typing import Iterator

import pytest

from herdr_run.foreign import agent_runner, lib


def test_packaged_runner_resumes_two_durable_turns(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A fresh subprocess imports the installed module and consumes the same state."""
    rec = _install_agy_agent(fake_runner_state, "portable")
    with lib.registry_lock() as agents:
        agents[rec.name].harness = "codex"
        agents[rec.name].next_seq = 0
        agents[rec.name].session_id = None
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

        def communicate(self, timeout: int) -> tuple[str, str]:
            return stdout_text, stderr_text

    monkeypatch.setattr(subprocess, "Popen", FakePopen)
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
