from __future__ import annotations

import sys
import contextlib
import io
import signal
import shutil
import threading
import subprocess
import json
import os
import time
from dataclasses import asdict, replace
from pathlib import Path
from typing import Iterator, cast

import pytest

from agentctl import worker_rpc
from agentctl.client import CustomProcessIdentity
from agentctl.errors import AgentDeliveryError
from agentctl.foreign import agent_down, agent_runner, lib


@pytest.mark.parametrize("bypass_permissions", [False, True])
def test_packaged_runner_resumes_two_durable_turns(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch, bypass_permissions: bool,
) -> None:
    """A fresh subprocess imports the installed module and consumes the same state."""
    rec = _install_agy_agent(fake_runner_state, "portable")
    with lib.registry_lock() as agents:
        current = agents[rec.name]
        _replace_test_launch(
            current, harness="codex",
            codex_bypass_permissions=bypass_permissions,
        )
        current.next_seq = 0
        current.session_id = None
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
    current = lib.read_registry()[rec.name]
    assert current.control_generation is not None
    proc = subprocess.Popen(
        [
            sys.executable, str(Path(agent_runner.__file__)), rec.name,
            current.control_generation, current.launch_fingerprint(),
        ],
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

    monkeypatch.setattr(
        agent_runner, "_spawn_harness",
        lambda _name, argv, _cwd, _authority: (FakePopen(argv), object()),
    )
    monkeypatch.setattr(
        agent_runner, "_owned_harness",
        lambda _name, _proc, _authority, _ownership: contextlib.nullcontext(threading.Event()),
    )
    agent_runner._run_muse_turn(
        name, rec,
        lib.Message(seq=0, text='literal $(prompt) "quoted"', model=None, queued_at=lib.now_iso()),
        agent_runner._RunnerAuthority.from_record(rec),
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
    _replace_test_launch(
        rec, harness="muse", model="watermelon-model",
        harness_args=("--reasoning-effort", "ultra", 'literal $(unexpanded) "quotes"'),
    )
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
    _replace_test_launch(rec, harness="muse")
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
    _replace_test_launch(rec, harness="muse")
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
    _replace_test_launch(rec, harness="muse")
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
    _replace_test_launch(rec, harness="muse")
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
    _replace_test_launch(rec, harness="muse")
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
    _replace_test_launch(rec, harness="muse")
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
            control=lib.RuntimeControl.standalone("standalone-test-generation"),
            owner_launch=lib.RuntimeLaunchContract.create(
                cwd=str(cwd), harness="agy", model="Example Model",
                backend="tmux", mode="headless", harness_args=(),
                permission_mode="native", runtime_home=base,
            ),
        )
        return agents[name]


def _outer_controlled(record: lib.AgentRecord, token: str = "a" * 32) -> lib.AgentRecord:
    """Convert a fixture to the exact outer-session authority under the lock."""
    with lib.registry_lock() as agents:
        current = agents[record.name]
        current.control = lib.RuntimeControl.outer_session(token)
    return lib.read_registry()[record.name]


def _replace_test_launch(record: lib.AgentRecord, **fields: object) -> None:
    """Construct a different canonical launch for setup-only fixture mutations."""
    for field, value in fields.items():
        setattr(record, field, value)
    record.owner_launch = None
    record.owner_launch = record.launch_contract()


def _untagged_runtime_row(record: lib.AgentRecord) -> dict[str, object]:
    """Project a current fixture into the exact pre-schema compatibility row."""
    row = record.to_public_dict()
    for field in ("schema", "control", "launch"):
        row.pop(field)
    return row


def _v2_runtime_row(
    record: lib.AgentRecord, token: str = "a" * 32,
) -> dict[str, object]:
    row = record.to_public_dict()
    row["schema"] = lib.LEGACY_RUNTIME_RECORD_SCHEMA
    row["owner_token"] = token
    for field in ("control", "launch", "runner_pid", "runner_started_at"):
        row.pop(field)
    return row


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

    monkeypatch.setattr(
        agent_runner, "_spawn_harness",
        lambda _name, argv, _cwd, _authority: (FakePopen(argv), object()),
    )
    monkeypatch.setattr(
        agent_runner, "_owned_harness",
        lambda _name, _proc, _authority, _ownership: contextlib.nullcontext(threading.Event()),
    )
    msg = lib.Message(seq=0, text="Probe agy.", model=None, queued_at=lib.now_iso())
    agent_runner._run_agy_turn(
        name, rec, msg, agent_runner._RunnerAuthority.from_record(rec),
    )


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
    record = lib.read_registry()[name]
    assert record.control_generation is not None
    return subprocess.Popen([
        sys.executable, str(Path(agent_runner.__file__)), name,
        record.control_generation, record.launch_fingerprint(),
    ], cwd=base,
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
        _replace_test_launch(agents[rec.name], harness=harness)
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
        _replace_test_launch(agents[rec.name], harness="codex")
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
        "def delayed(name,runner,harness,generation,fingerprint):\n"
        f"    Path({str(barrier)!r}).write_text(str(harness.pid))\n"
        f"    while not Path({str(release)!r}).exists(): time.sleep(0.01)\n"
        "    record(name,runner,harness,generation,fingerprint)\n"
        "lib.record_active_harness=delayed\n"
        "raise SystemExit(agent_runner.main())\n"
    )
    env = os.environ.copy()
    env.pop("HERDR_SUBAGENTS_POLICY", None)
    env.pop("SUBAGENTS_MIGRATION_STAGE", None)
    env.update(HERDR_SUBAGENTS_HOME=str(fake_runner_state), CODEX_BIN=str(executable))
    current = lib.read_registry()[rec.name]
    assert current.control_generation is not None
    runner = subprocess.Popen([
        sys.executable, str(launcher), rec.name,
        current.control_generation, current.launch_fingerprint(),
    ], env=env, text=True,
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
        _replace_test_launch(agents[rec.name], harness="codex")
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


def _make_dead_gc_record(base: Path, name: str) -> lib.AgentRecord:
    rec = _install_agy_agent(base, name)
    identity = CustomProcessIdentity(
        version=1,
        boot_id="11111111-2222-3333-4444-555555555555",
        pid=22 if name == "affected" else 44,
        starttime_ticks=100 if name == "affected" else 300,
        executable_device=3,
        executable_inode=4,
    )
    with lib.registry_lock() as agents:
        current = agents[name]
        current.created_at = "2000-01-01T00:00:00+00:00"
        current.runner_pid = identity.pid
        current.runner_started_at = str(identity.starttime_ticks)
        current.runner_identity = identity
    return lib.read_registry()[name]


def test_gc_isolates_malformed_harness_sidecar_and_reaps_other_dead_agent(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    affected = _make_dead_gc_record(fake_runner_state, "affected")
    _make_dead_gc_record(fake_runner_state, "bystander")
    sidecar = lib.agent_dir(affected.name) / "active-harness.json"
    sidecar.write_text(
        '{"schema":"invalid"}\n', encoding="utf-8",
    )
    sidecar.chmod(0o600)
    monkeypatch.setattr(lib, "runner_liveness", lambda _rec: lib.ProcessLiveness.DEAD)
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)

    notes = lib.gc()

    assert "affected" in lib.read_registry()
    assert "bystander" not in lib.read_registry()
    assert any("affected degraded" in note for note in notes)
    assert any("reaped bystander" in note for note in notes)


def test_gc_migrates_only_proved_dead_v1_harness_sidecar(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    affected = _make_dead_gc_record(fake_runner_state, "affected")
    _make_dead_gc_record(fake_runner_state, "bystander")
    sidecar = lib.agent_dir(affected.name) / "active-harness.json"
    sidecar.write_text(json.dumps({
        "runner_pid": affected.runner_pid,
        "runner_started_at": affected.runner_started_at,
        "pid": 33,
        "started_at": "200",
    }), encoding="utf-8")
    sidecar.chmod(0o600)
    monkeypatch.setattr(lib, "runner_liveness", lambda _rec: lib.ProcessLiveness.DEAD)
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)
    monkeypatch.setattr(lib, "pid_start_time", lambda _pid: None)
    monkeypatch.setattr(lib, "_process_group_members", lambda _group: {})

    notes = lib.gc()

    assert not sidecar.exists()
    assert not lib.read_registry()
    assert sum(note.startswith("reaped ") for note in notes) == 2


def test_gc_contains_legacy_sidecar_unlink_failure_per_agent(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    affected = _make_dead_gc_record(fake_runner_state, "affected")
    _make_dead_gc_record(fake_runner_state, "bystander")
    sidecar = lib.agent_dir(affected.name) / "active-harness.json"
    sidecar.write_text(json.dumps({
        "runner_pid": affected.runner_pid,
        "runner_started_at": affected.runner_started_at,
        "pid": 33,
        "started_at": "200",
    }), encoding="utf-8")
    sidecar.chmod(0o600)
    monkeypatch.setattr(lib, "runner_liveness", lambda _rec: lib.ProcessLiveness.DEAD)
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)
    monkeypatch.setattr(lib, "pid_start_time", lambda _pid: None)
    monkeypatch.setattr(lib, "_process_group_members", lambda _group: {})
    original_unlink = Path.unlink

    def refuse_sidecar(path: Path, *args: object, **kwargs: object) -> None:
        if path == sidecar:
            raise OSError("injected unlink failure")
        original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", refuse_sidecar)
    notes = lib.gc()

    registry = lib.read_registry()
    assert "affected" in registry
    assert "bystander" not in registry
    assert any("affected degraded" in note for note in notes)


def test_gc_missing_workspace_contains_harness_failure_per_agent(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    affected = _make_dead_gc_record(fake_runner_state, "affected")
    bystander = _make_dead_gc_record(fake_runner_state, "bystander")
    for record in (affected, bystander):
        with lib.registry_lock() as agents:
            current = agents[record.name]
            current.backend = "herdr"
            current.tmux_target = "w1:t1"
    sidecar = lib.agent_dir(affected.name) / "active-harness.json"
    sidecar.write_text(
        '{"schema":"invalid"}\n', encoding="utf-8",
    )
    sidecar.chmod(0o600)
    monkeypatch.setattr(lib, "runner_liveness", lambda _rec: lib.ProcessLiveness.DEAD)
    monkeypatch.setattr(lib, "_herdr_workspace_exists", lambda _workspace: False)

    notes = lib.gc()

    registry = lib.read_registry()
    assert "affected" in registry
    assert "bystander" not in registry
    assert not (lib.agent_dir("affected") / "WORKSPACE_LOST.json").exists()
    assert any("affected degraded" in note for note in notes)


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
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "lost-stop"))
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
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "stop-collision"))
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


def test_stop_retry_refuses_a_receipt_for_another_immutable_launch(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "stop-launch-mismatch"))
    token = cast(str, rec.owner_token)
    fingerprint = rec.launch_fingerprint()
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)
    stopped = lib.stop_owned_agent(
        rec.name, token, grace=0, launch_fingerprint=fingerprint,
    )
    destination = Path(cast(str, stopped.archived_to))
    receipt_path = destination / "stop-receipt.json"
    receipt = json.loads(receipt_path.read_text())
    receipt["owner_launch_fingerprint"] = "f" * 64
    receipt_path.write_text(json.dumps(receipt) + "\n")

    with pytest.raises(lib.AgentOperationError) as raised:
        lib.stop_owned_agent(
            rec.name, token, grace=0, launch_fingerprint=fingerprint,
        )
    assert raised.value.code == "owner_launch_mismatch"
    assert destination.is_dir()
    assert rec.name not in lib.read_registry()


def test_stop_retry_refuses_archive_directory_replacement_after_receipt_read(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "stop-archive-replaced"))
    token = cast(str, rec.owner_token)
    fingerprint = rec.launch_fingerprint()
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)
    stopped = lib.stop_owned_agent(
        rec.name, token, grace=0, launch_fingerprint=fingerprint,
    )
    destination = Path(cast(str, stopped.archived_to))
    displaced = destination.with_name(destination.name + "-displaced")
    with lib.registry_lock() as agents:
        agents[rec.name] = rec
    real_read = lib._read_bounded_private_json
    swapped = False

    def replace_after_read(
        path: Path | str, limit: int, label: str,
        *, directory_fd: int | None = None,
    ) -> object:
        nonlocal swapped
        value = real_read(
            path, limit, label, directory_fd=directory_fd,
        )
        if label == "stop receipt" and not swapped:
            swapped = True
            destination.rename(displaced)
            destination.mkdir()
            (destination / "replacement").write_text("must remain untouched\n")
        return value

    monkeypatch.setattr(lib, "_read_bounded_private_json", replace_after_read)
    with pytest.raises(lib.AgentOperationError) as raised:
        lib.stop_owned_agent(
            rec.name, token, grace=0, launch_fingerprint=fingerprint,
        )
    assert raised.value.code == "stop_receipt_invalid"
    assert rec.name in lib.read_registry()
    assert (destination / "replacement").read_text() == "must remain untouched\n"
    assert displaced.is_dir()


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


def test_stale_runner_cannot_clear_a_replacement_active_harness_sidecar(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "harness-replaced")
    runner = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=4241, starttime_ticks=9000, executable_device=3,
        executable_inode=4,
    )
    original_harness = replace(
        runner, pid=4242, starttime_ticks=9001, executable_inode=5,
    )
    replacement_harness = replace(
        runner, pid=4243, starttime_ticks=9002, executable_inode=6,
    )
    with lib.registry_lock() as agents:
        current = agents[rec.name]
        current.runner_identity = runner
        current.runner_pid = runner.pid
        current.runner_started_at = str(runner.starttime_ticks)
    rec = lib.read_registry()[rec.name]
    generation = cast(str, rec.control_generation)
    fingerprint = rec.launch_fingerprint()
    sidecar = lib.agent_dir(rec.name) / "active-harness.json"
    sidecar.write_text(json.dumps({
        "schema": "agentctl-active-harness/v3",
        "control_generation": generation,
        "launch_fingerprint": fingerprint,
        "runner": asdict(runner),
        "harness": asdict(replacement_harness),
    }))
    before = sidecar.read_bytes()

    assert not lib.clear_active_harness(
        rec.name, generation, fingerprint, runner, original_harness,
    )
    assert sidecar.read_bytes() == before


def test_stop_marker_preserves_active_turn_grace_period(fake_runner_state: Path) -> None:
    rec = _install_agy_agent(fake_runner_state, "grace")
    with lib.registry_lock() as agents:
        _replace_test_launch(agents[rec.name], harness="codex")
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
    monkeypatch.setattr(lib, "record_active_harness", lambda *_args: None)
    lib.last_message_path(rec.name).write_text("previous turn answer")
    (lib.agent_dir(rec.name) / "codex-answer-000000000001.txt").write_text("older attempt")
    agent_runner._run_codex_turn(
        rec.name, rec, lib.Message(1, "new request", None, lib.now_iso()),
        agent_runner._RunnerAuthority.from_record(rec),
    )
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
    monkeypatch.setattr(lib, "record_active_harness", lambda *_args: None)
    monkeypatch.setattr(lib, "TURN_TIMEOUT_S", 2)
    agent_runner._run_codex_turn(
        rec.name, rec, lib.Message(1, "request", None, lib.now_iso()),
        agent_runner._RunnerAuthority.from_record(rec),
    )
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
    def execute(
        name: str, message: lib.Message,
        _authority: agent_runner._RunnerAuthority,
    ) -> None:
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
        agent_runner._consume(
            rec.name, rec, source, agent_runner._RunnerAuthority.from_record(rec),
        )
    monkeypatch.setattr(os, "replace", original_replace)
    agent_runner._consume(
        rec.name, rec, source, agent_runner._RunnerAuthority.from_record(rec),
    )
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
    agent_runner._consume(
        rec.name, rec, source, agent_runner._RunnerAuthority.from_record(rec),
    )
    assert source.exists()
    assert not list(lib.inflight_dir(rec.name).iterdir())
    assert lib.status_snapshot(rec.name, run_gc=False).agents[0].automation_paused


def test_owner_generation_binds_once_and_rejects_stale_runtime_operations(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "owned")
    first = "a" * 32
    second = "b" * 32
    rec.runner_identity = CustomProcessIdentity(
        version=1,
        boot_id="11111111-2222-3333-4444-555555555555",
        pid=999_991,
        starttime_ticks=88,
        executable_device=3,
        executable_inode=4,
    )
    rec.runner_pid = rec.runner_identity.pid
    rec.runner_started_at = str(rec.runner_identity.starttime_ticks)
    legacy = _untagged_runtime_row(rec)
    lib.REGISTRY.write_text(json.dumps([legacy]), encoding="utf-8")
    monkeypatch.setattr(
        lib, "runner_liveness", lambda _rec: lib.ProcessLiveness.DEAD,
    )
    contract = rec.launch_contract()
    assert lib.bind_owner_launch(rec.name, first, contract).owner_token == first
    with pytest.raises(lib.AgentOperationError, match="another session generation") as raised:
        lib.bind_owner_launch(rec.name, second, contract)
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
            "owner_launch": contract.to_document(),
        })
    assert not called


def test_standalone_mutation_cannot_target_an_outer_runtime(
    fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "outer-only"))
    before_registry = lib.REGISTRY.read_bytes()
    before_inbox = tuple(lib.inbox_dir(rec.name).iterdir())

    with pytest.raises(lib.AgentOperationError) as raised:
        lib.send_message_to_agent(rec.name, "must not cross control planes")

    assert raised.value.code == "owner_launch_required"
    assert lib.REGISTRY.read_bytes() == before_registry
    assert tuple(lib.inbox_dir(rec.name).iterdir()) == before_inbox


def test_current_standalone_dead_runtime_remains_collectable(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "standalone-dead")
    assert rec.control is not None
    assert rec.control.kind == lib.STANDALONE_CONTROL
    with lib.registry_lock() as agents:
        agents[rec.name].created_at = "2020-01-01T00:00:00+00:00"
    monkeypatch.setattr(lib, "runner_liveness", lambda _rec: lib.ProcessLiveness.DEAD)
    monkeypatch.setattr(lib, "window_exists", lambda _rec: False)

    notes = lib.gc()

    assert any("standalone-dead" in note for note in notes)
    assert rec.name not in lib.read_registry()
    assert any(path.name.startswith("standalone-dead-") for path in lib.ARCHIVE.iterdir())


def test_stale_runner_generation_cannot_mutate_reused_runtime(
    fake_runner_state: Path,
) -> None:
    original = _install_agy_agent(fake_runner_state, "reused")
    stale = agent_runner._RunnerAuthority.from_record(original)
    with lib.registry_lock() as agents:
        replacement = agents[original.name]
        replacement.control = lib.RuntimeControl.standalone("replacement-generation")
    before = lib.REGISTRY.read_bytes()

    with pytest.raises(lib.AgentOperationError) as raised:
        agent_runner._set_status(original.name, "busy", stale)

    assert raised.value.code == "runner_authority_mismatch"
    assert lib.REGISTRY.read_bytes() == before


def test_runner_generation_reads_do_not_rewrite_the_current_registry(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "steady-runner")
    authority = agent_runner._RunnerAuthority.from_record(rec)
    before = lib.REGISTRY.read_bytes()
    before_stat = lib.REGISTRY.stat()

    for _ in range(5):
        observed = authority.load(rec.name)
        assert observed.control_generation == authority.control_generation
        assert observed.launch_fingerprint() == authority.launch_fingerprint

    after_stat = lib.REGISTRY.stat()
    assert lib.REGISTRY.read_bytes() == before
    assert after_stat.st_ino == before_stat.st_ino
    assert after_stat.st_mtime_ns == before_stat.st_mtime_ns


def test_stale_runner_refuses_before_recreating_generation_state(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    original = _install_agy_agent(fake_runner_state, "stale-start")
    stale = agent_runner._RunnerAuthority.from_record(original)
    with lib.registry_lock() as agents:
        agents[original.name].control = lib.RuntimeControl.standalone(
            "replacement-generation"
        )
    shutil.rmtree(lib.agent_dir(original.name))
    monkeypatch.setattr(sys, "argv", [
        str(agent_runner.__file__), original.name,
        cast(str, stale.control_generation), cast(str, stale.launch_fingerprint),
    ])

    with pytest.raises(lib.AgentOperationError) as raised:
        agent_runner.main()

    assert raised.value.code == "runner_authority_mismatch"
    assert not lib.agent_dir(original.name).exists()


def test_owner_permission_policy_is_part_of_the_generation_binding(
    fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "permission-bound"))
    with lib.registry_lock() as agents:
        _replace_test_launch(agents[rec.name], harness="codex")
    rec = lib.read_registry()[rec.name]
    token = rec.owner_token
    assert token is not None
    contract = rec.launch_contract()
    native = lib.verify_owner_launch(rec.name, token, contract)
    assert native.codex_bypass_permissions is False

    before = lib.REGISTRY.read_bytes()
    with pytest.raises(lib.AgentOperationError, match="immutable launch") as raised:
        lib.verify_owner_launch(
            rec.name, token, replace(contract, permission_mode="bypass"),
        )
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == before


def test_existing_worker_start_refuses_permission_policy_drift(
    fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "permission-start"))
    with lib.registry_lock() as agents:
        _replace_test_launch(agents[rec.name], harness="codex")
    rec = lib.read_registry()[rec.name]
    before = lib.REGISTRY.read_bytes()
    token = rec.owner_token
    assert token is not None
    contract = replace(rec.launch_contract(), permission_mode="bypass")

    with pytest.raises(lib.AgentOperationError) as raised:
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2",
            "action": "start",
            "name": rec.name,
            "owner_token": token,
            "desired_paused": False,
            "owner_launch": contract.to_document(),
            "brief": None,
        })
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == before


def test_legacy_pid_start_identity_is_never_upgraded_from_current_process(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "legacy-identity")
    legacy = _untagged_runtime_row(rec)
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
        lib.bind_owner_launch(rec.name, "a" * 32, rec.launch_contract())
    assert raised.value.code == "runner_identity_incomplete"
    assert lib.REGISTRY.read_bytes() == raw


def test_binding_one_legacy_row_preserves_unrelated_untrusted_row(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = _install_agy_agent(fake_runner_state, "first")
    first.runner_identity = CustomProcessIdentity(
        version=1,
        boot_id="11111111-2222-3333-4444-555555555555",
        pid=999_992,
        starttime_ticks=89,
        executable_device=3,
        executable_inode=4,
    )
    first.runner_pid = first.runner_identity.pid
    first.runner_started_at = str(first.runner_identity.starttime_ticks)
    second = replace(first, name="second", tmux_target="subagents:second")
    rows = []
    for record in (first, second):
        legacy = _untagged_runtime_row(record)
        if record.name == "second":
            legacy.pop("runner_identity")
            legacy["runner_pid"] = os.getpid()
            legacy["runner_started_at"] = lib.pid_start_time(os.getpid())
        rows.append(legacy)
    lib.REGISTRY.write_text(json.dumps(rows, sort_keys=True), encoding="utf-8")
    sidecar = lib._permission_policy_path("second")
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    sidecar_bytes = b'{"legacy":"must remain untouched"}\n'
    sidecar.write_bytes(sidecar_bytes)
    monkeypatch.setattr(
        lib, "runner_liveness", lambda record: (
            lib.ProcessLiveness.DEAD
            if record.name == "first" else lib.ProcessLiveness.UNKNOWN
        ),
    )

    bound = lib.bind_owner_launch("first", "a" * 32, first.launch_contract())

    assert bound.owner_token == "a" * 32
    stored = json.loads(lib.REGISTRY.read_text(encoding="utf-8"))
    second_stored = next(row for row in stored if row["name"] == "second")
    assert "owner_token" not in second_stored
    assert "schema" not in second_stored
    assert "runner_identity" not in second_stored
    assert sidecar.read_bytes() == sidecar_bytes


@pytest.mark.parametrize(
    "dimension",
    ["cwd", "harness", "model", "backend", "mode", "harness_args", "permission", "runtime_home"],
)
def test_worker_start_launch_mismatch_does_not_claim_or_rewrite_legacy_row(
    dimension: str, fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "mismatch")
    if dimension == "permission":
        with lib.registry_lock() as agents:
            _replace_test_launch(agents[rec.name], harness="codex")
        rec = lib.read_registry()[rec.name]
    legacy = _untagged_runtime_row(rec)
    legacy.pop("runner_identity")
    raw = (json.dumps([legacy], sort_keys=True) + "\n").encode()
    lib.REGISTRY.write_bytes(raw)
    values: dict[str, object] = {
        "cwd": rec.cwd,
        "harness": rec.harness,
        "model": rec.model,
        "backend": rec.backend,
        "mode": rec.mode,
        "harness_args": rec.harness_args,
        "permission_mode": "native",
        "runtime_home": str(lib.BASE),
    }
    values[dimension if dimension != "permission" else "permission_mode"] = {
        "cwd": str(fake_runner_state / "other"),
        "harness": "codex",
        "model": "different-model",
        "backend": "herdr",
        "mode": "tui",
        "harness_args": ("--different",),
        "permission": "bypass",
        "runtime_home": str(fake_runner_state / "other-runtime"),
    }[dimension]

    with pytest.raises(lib.AgentOperationError) as raised:
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2",
            "action": "start",
            "name": rec.name,
            "owner_token": "a" * 32,
            "desired_paused": False,
            "owner_launch": lib.RuntimeLaunchContract.create(
                **values,  # type: ignore[arg-type]
            ).to_document(),
            "brief": None,
        })
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == raw


@pytest.mark.parametrize("state", ["starting", "live", "unknown"])
def test_v2_headless_runner_cannot_be_relabelled_as_generation_bound(
    state: str, fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, f"v2-{state}")
    if state != "starting":
        rec.runner_identity = CustomProcessIdentity(
            version=1,
            boot_id="11111111-2222-3333-4444-555555555555",
            pid=999_993,
            starttime_ticks=90,
            executable_device=3,
            executable_inode=4,
        )
        rec.runner_pid = rec.runner_identity.pid
        rec.runner_started_at = str(rec.runner_identity.starttime_ticks)
    row = _v2_runtime_row(rec)
    raw = (json.dumps([row], sort_keys=True) + "\n").encode()
    lib.REGISTRY.write_bytes(raw)
    if state != "starting":
        monkeypatch.setattr(
            lib, "runner_liveness",
            lambda _rec: (
                lib.ProcessLiveness.LIVE
                if state == "live" else lib.ProcessLiveness.UNKNOWN
            ),
        )

    loaded = lib.read_registry()[rec.name]
    assert loaded.control is not None
    assert loaded.control.kind == lib.LEGACY_UNDETERMINED_CONTROL
    with pytest.raises(lib.AgentOperationError) as raised:
        lib.verify_owner_launch(rec.name, "a" * 32, rec.launch_contract())

    assert raised.value.code == "runner_generation_unbound"
    assert lib.REGISTRY.read_bytes() == raw


def test_v2_tui_route_migrates_only_to_standalone_control(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "v2-migrated")
    with lib.registry_lock() as agents:
        _replace_test_launch(agents[rec.name], harness="codex")
    rec = lib.read_registry()[rec.name]
    owner_launch = rec.launch_contract()
    row = _v2_runtime_row(rec)
    row.update({
        "backend": "herdr", "mode": "tui", "tmux_target": "w1:t1",
        "presentation_pane": "w1:p1", "runner_identity": None,
    })
    raw = (json.dumps([row], sort_keys=True) + "\n").encode()
    lib.REGISTRY.write_bytes(raw)

    loaded = lib.read_registry()[rec.name]
    assert (loaded.backend, loaded.mode) == ("herdr", "tui")
    assert loaded.control == lib.RuntimeControl.standalone("a" * 32)
    assert loaded.owner_launch == loaded.launch_contract()
    stored = json.loads(lib.REGISTRY.read_text())[0]
    assert stored["schema"] == lib.RUNTIME_RECORD_SCHEMA
    assert stored["control"] == loaded.control.to_document()
    assert stored["launch"] == loaded.owner_launch.to_document()

    before = lib.REGISTRY.read_bytes()
    with pytest.raises(lib.AgentOperationError) as raised:
        lib.verify_owner_launch(rec.name, "a" * 32, owner_launch)
    assert raised.value.code == "owner_control_mismatch"
    assert lib.REGISTRY.read_bytes() == before


def test_v2_headless_runtime_has_one_explicit_generation_bound_retirement(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "v2-retire")
    identity = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=999_994, starttime_ticks=91, executable_device=3,
        executable_inode=4,
    )
    rec.runner_identity = identity
    rec.runner_pid = identity.pid
    rec.runner_started_at = str(identity.starttime_ticks)
    generation = "a" * 32
    lib.REGISTRY.write_text(json.dumps([
        _v2_runtime_row(rec, generation),
    ], sort_keys=True) + "\n")
    alive = True
    terminated: list[tuple[lib.AgentRecord, float]] = []

    monkeypatch.setattr(
        lib, "runner_liveness",
        lambda _record: (
            lib.ProcessLiveness.LIVE if alive else lib.ProcessLiveness.DEAD
        ),
    )

    def terminate(record: lib.AgentRecord, grace: float = 2.0) -> bool:
        nonlocal alive
        terminated.append((record, grace))
        alive = False
        return True

    monkeypatch.setattr(lib, "terminate_runner", terminate)
    monkeypatch.setattr(
        lib, "kill_window",
        lambda _record: pytest.fail("legacy retirement must preserve presentation"),
    )
    first = lib.retire_legacy_undetermined_runtime(
        rec.name, generation, grace=0.25,
    )
    second = lib.retire_legacy_undetermined_runtime(
        rec.name, generation, grace=0.25,
    )

    assert [(item.name, grace) for item, grace in terminated] == [
        (rec.name, 0.25),
    ]
    assert first == second
    assert first.killed_window is False
    assert first.forced is True
    assert first.archived_to == str(lib.ARCHIVE / f"{rec.name}-{generation}")
    assert rec.name not in lib.read_registry()
    receipt = json.loads(
        (Path(cast(str, first.archived_to)) / "stop-receipt.json").read_text()
    )
    assert receipt["owner_token"] == generation
    assert receipt["owner_launch_fingerprint"] == rec.launch_fingerprint()


@pytest.mark.parametrize(
    "case,expected_code",
    [
        ("wrong-generation", "legacy_control_mismatch"),
        ("missing-identity", "runner_identity_incomplete"),
        ("unknown", "runner_liveness_unknown"),
        ("current-v3", "legacy_control_mismatch"),
    ],
)
def test_v2_headless_retirement_refuses_without_exact_dead_or_owned_runner(
    case: str, expected_code: str, fake_runner_state: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, f"v2-retire-{case}")
    identity = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=999_995, starttime_ticks=92, executable_device=3,
        executable_inode=4,
    )
    rec.runner_identity = identity
    rec.runner_pid = identity.pid
    rec.runner_started_at = str(identity.starttime_ticks)
    generation = "a" * 32
    if case != "current-v3":
        row = _v2_runtime_row(rec, generation)
        if case == "missing-identity":
            row["runner_identity"] = None
        lib.REGISTRY.write_text(json.dumps([row], sort_keys=True) + "\n")
    if case == "unknown":
        monkeypatch.setattr(
            lib, "runner_liveness", lambda _record: lib.ProcessLiveness.UNKNOWN,
        )
    before_registry = lib.REGISTRY.read_bytes()
    before_state = tuple(sorted(path.name for path in lib.agent_dir(rec.name).iterdir()))
    expected = "b" * 32 if case == "wrong-generation" else generation
    monkeypatch.setattr(
        lib, "terminate_runner",
        lambda *_args, **_kwargs: pytest.fail("unproved runner was signaled"),
    )

    with pytest.raises(lib.AgentOperationError) as raised:
        lib.retire_legacy_undetermined_runtime(rec.name, expected, grace=0)

    assert raised.value.code == expected_code
    assert lib.REGISTRY.read_bytes() == before_registry
    assert tuple(sorted(path.name for path in lib.agent_dir(rec.name).iterdir())) == before_state


def test_v2_headless_retirement_refuses_live_and_archive_collision(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "v2-retire-collision")
    identity = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=999_996, starttime_ticks=93, executable_device=3,
        executable_inode=4,
    )
    rec.runner_identity = identity
    rec.runner_pid = identity.pid
    rec.runner_started_at = str(identity.starttime_ticks)
    generation = "a" * 32
    lib.REGISTRY.write_text(json.dumps([
        _v2_runtime_row(rec, generation),
    ], sort_keys=True) + "\n")
    destination = lib.ARCHIVE / f"{rec.name}-{generation}"
    destination.mkdir(parents=True)
    before_registry = lib.REGISTRY.read_bytes()

    with pytest.raises(lib.AgentOperationError) as raised:
        lib.retire_legacy_undetermined_runtime(rec.name, generation, grace=0)

    assert raised.value.code == "stop_archive_collision"
    assert lib.REGISTRY.read_bytes() == before_registry
    assert destination.is_dir()
    assert lib.agent_dir(rec.name).is_dir()


@pytest.mark.parametrize(
    "argv",
    [
        ["agent_down.py", "worker", "--force", "--recover-v2-generation", "a" * 32],
        ["agent_down.py", "worker", "--all-dead", "--recover-v2-generation", "a" * 32],
    ],
)
def test_v2_recovery_cli_refuses_conflicting_modes(
    argv: list[str], monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(sys, "argv", argv)
    monkeypatch.setattr(
        lib, "retire_legacy_undetermined_runtime",
        lambda *_args, **_kwargs: pytest.fail("conflicting recovery mode executed"),
    )

    with pytest.raises(SystemExit):
        agent_down.main()


def test_untagged_row_cannot_assert_owner_launch_or_trigger_migration(
    fake_runner_state: Path, capsys: pytest.CaptureFixture[str],
) -> None:
    rec = _install_agy_agent(fake_runner_state, "legacy-injection")
    row = _untagged_runtime_row(rec)
    row.pop("runner_identity")
    row["launch"] = rec.launch_contract().to_document()
    raw = (json.dumps([row], sort_keys=True) + "\n").encode()
    lib.REGISTRY.write_bytes(raw)

    with pytest.raises(SystemExit):
        lib.read_registry()

    assert "cannot assert runtime control" in capsys.readouterr().err
    assert lib.REGISTRY.read_bytes() == raw


def test_current_runtime_storage_has_one_launch_authority(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "canonical-launch")
    stored = rec.to_dict()

    assert stored["schema"] == lib.RUNTIME_RECORD_SCHEMA
    assert stored["launch"] == rec.owner_launch.to_document()  # type: ignore[union-attr]
    assert stored["control"] == rec.control.to_document()  # type: ignore[union-attr]
    assert not {
        "harness", "cwd", "model", "harness_args", "codex_bypass_permissions",
    } & stored.keys()
    loaded = lib.AgentRecord.from_dict(stored)
    assert loaded.owner_launch == rec.owner_launch
    assert loaded.harness == rec.harness
    assert loaded.cwd == rec.cwd


@pytest.mark.parametrize(
    "field,value",
    [
        ("cwd", "/different"),
        ("harness", "codex"),
        ("model", "different"),
        ("harness_args", ("--different",)),
        ("codex_bypass_permissions", True),
    ],
)
def test_current_runtime_refuses_mutated_launch_compatibility_projection(
    field: str, value: object, fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, f"mutated-{field.replace('_', '-')}")
    setattr(rec, field, value)

    with pytest.raises(lib.AgentOperationError) as raised:
        rec.to_dict()

    assert raised.value.code == "owner_launch_mismatch"


def test_tokenless_migrated_route_cannot_invent_original_launch(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "unowned-migrated")
    owner_launch = rec.launch_contract()
    row = _untagged_runtime_row(rec)
    row.update({"backend": "herdr", "mode": "headless"})
    raw = (json.dumps([row], sort_keys=True) + "\n").encode()
    lib.REGISTRY.write_bytes(raw)

    with pytest.raises(lib.AgentOperationError) as raised:
        lib.bind_owner_launch(rec.name, "a" * 32, owner_launch)
    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == raw


def test_worker_start_rejects_interactive_owner_contract_without_state(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    cwd = fake_runner_state / "cwd"
    cwd.mkdir(parents=True)
    contract = lib.RuntimeLaunchContract.create(
        cwd=str(cwd), harness="codex", model=None, backend="herdr",
        mode="tui", harness_args=(), permission_mode="native",
        runtime_home=lib.BASE,
    )
    monkeypatch.setattr(lib, "backend_available", lambda _backend: True)
    with pytest.raises(lib.AgentOperationError) as raised:
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2", "action": "start",
            "name": "interactive", "owner_token": "a" * 32,
            "desired_paused": False,
            "owner_launch": contract.to_document(), "brief": None,
        })
    assert raised.value.code == "owner_launch_mismatch"
    assert not lib.REGISTRY.exists()


@pytest.mark.parametrize(
    "action,desired_paused", [("pause", False), ("resume", True)],
)
def test_worker_pause_action_must_match_desired_state(
    action: str, desired_paused: bool, fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, f"bad-{action}"))
    before = lib.REGISTRY.read_bytes()
    with pytest.raises(ValueError, match="contradicts desired_paused"):
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2", "action": action,
            "name": rec.name, "owner_token": rec.owner_token,
            "desired_paused": desired_paused,
            "owner_launch": rec.launch_contract().to_document(),
        })
    assert lib.REGISTRY.read_bytes() == before


@pytest.mark.parametrize(
    "request_bytes",
    [
        b'{"schema":"agentctl-worker-rpc/v2","schema":"duplicate"}',
        ("[" * 70 + "0" + "]" * 70).encode(),
        b"\xff",
    ],
)
def test_worker_main_returns_one_typed_error_for_ambiguous_input(
    request_bytes: bytes, monkeypatch: pytest.MonkeyPatch,
) -> None:
    class BinaryStream:
        def __init__(self, value: bytes = b"") -> None:
            self.buffer = io.BytesIO(value)

    stdin = BinaryStream(request_bytes)
    stdout = BinaryStream()
    monkeypatch.setattr(sys, "stdin", stdin)
    monkeypatch.setattr(sys, "stdout", stdout)

    assert worker_rpc._main() == 1
    response = json.loads(stdout.buffer.getvalue())
    assert response["schema"] == "agentctl-worker-rpc/v2"
    assert response["ok"] is False
    assert response["error"]["code"] == "runtime_failure"


def test_worker_response_and_error_formatting_are_byte_bounded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class BinaryStream:
        def __init__(self) -> None:
            self.buffer = io.BytesIO()

    stdout = BinaryStream()
    monkeypatch.setattr(sys, "stdout", stdout)
    assert worker_rpc._write_response({
        "schema": "agentctl-worker-rpc/v2", "action": "read",
        "owner_token": "a" * 32, "ok": True,
        "payload": {"text": "x" * ((8 << 20) + 1)},
    })
    encoded = stdout.buffer.getvalue()
    assert len(encoded) < 1024
    assert json.loads(encoded)["error"]["code"] == "response_too_large"

    for unit in ("x", "é", "😀"):
        bounded = worker_rpc._bounded_error(ValueError(unit * (1 << 20)))
        assert len(bounded.encode()) <= 32 << 10
        assert bounded.endswith("...[truncated by agentctl worker RPC]")


def test_read_refuses_an_oversized_transcript_before_response_serialization(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "large-transcript")
    path = lib.transcript_path(rec.name)
    path.write_bytes(b"x" * (lib.MAX_READ_OUTPUT_BYTES + 1))
    with pytest.raises(lib.AgentOperationError, match="within") as raised:
        lib.read_agent_output(rec.name, mode="all")
    assert raised.value.code == "control_record_invalid"


def test_non_stop_operation_cannot_succeed_after_runtime_row_disappears(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "lost-row"))

    def remove_row(_name: str, *, run_gc: bool = False) -> list[object]:
        assert not run_gc
        with lib.registry_lock() as agents:
            agents.pop(rec.name)
        return []

    monkeypatch.setattr(lib, "status_snapshot", remove_row)
    with pytest.raises(lib.AgentOperationError) as raised:
        worker_rpc.dispatch({
            "schema": "agentctl-worker-rpc/v2", "action": "status",
            "name": rec.name, "owner_token": rec.owner_token,
            "desired_paused": False,
            "owner_launch": rec.launch_contract().to_document(),
        })
    assert raised.value.code == "owner_launch_lost"


def test_runtime_registry_rejects_duplicate_keys_and_logical_names(
    fake_runner_state: Path,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "duplicate-row")
    original = rec.to_dict()
    assert rec.control_generation is not None
    generation = rec.control_generation
    encoded = json.dumps(original, separators=(",", ":"))
    duplicate_key = encoded.replace(
        f'"generation":"{generation}"',
        f'"generation":"{generation}","generation":"replacement"',
        1,
    )
    lib.REGISTRY.write_text(f"[{duplicate_key}]\n")
    with pytest.raises(SystemExit):
        lib.read_registry()

    lib.REGISTRY.write_text(json.dumps([original, original]) + "\n")
    with pytest.raises(SystemExit):
        lib.read_registry()


def test_generation_sidecars_reject_duplicate_json_keys(
    fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "duplicate-sidecars"))
    duplicate = b'{"schema":"first","schema":"second"}\n'

    active = lib.agent_dir(rec.name) / "active-harness.json"
    active.write_bytes(duplicate)
    with pytest.raises(lib.AgentOperationError) as active_error:
        lib.terminate_active_harness(rec)
    assert active_error.value.code == "harness_identity_unknown"

    pause_ack = lib.migration_pause_ack_path(rec.name)
    pause_ack.write_bytes(duplicate)
    identity = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=1, starttime_ticks=1, executable_device=1, executable_inode=1,
    )
    assert lib.pause_acknowledged(rec.name, identity) is False

    staged = lib.staged_runner_path(rec.name, "stage-token")
    staged.write_bytes(duplicate)
    assert lib.read_staged_runner(rec.name, "stage-token") is None

    queued = lib.inbox_dir(rec.name) / "000000000001.json"
    queued.write_bytes(b'{"seq":1,"seq":2,"text":"x","queued_at":"now"}\n')
    with pytest.raises(lib.AgentOperationError) as queued_error:
        lib.Message.from_path(queued)
    assert queued_error.value.code == "control_record_invalid"

    with pytest.raises(lib.AgentOperationError) as tui_error:
        lib._record_tui_delivery_failure(
            queued, lib.AgentOperationError("delivery_failed", "failed"),
        )
    assert tui_error.value.code == "tui_inbox_corrupt"

    destination = lib.ARCHIVE / f"{rec.name}-{rec.owner_token}"
    destination.mkdir(parents=True)
    (destination / "stop-receipt.json").write_bytes(duplicate)
    with pytest.raises(lib.AgentOperationError) as stop_error:
        lib._owned_stop_receipt(
            destination, rec.name, cast(str, rec.owner_token),
            rec.launch_fingerprint(),
        )
    assert stop_error.value.code == "stop_receipt_invalid"


def test_pause_markers_are_scoped_to_the_owner_generation(
    fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(_install_agy_agent(fake_runner_state, "pause-generation"))
    current_token = rec.owner_token
    assert current_token is not None
    stale_token = "d" * 32
    current = lib.verify_owner_launch(
        rec.name, current_token, rec.launch_contract(),
    )
    legacy = lib.agent_dir(rec.name) / "automation-paused"
    legacy.touch()

    lib.reconcile_automation_pause(current, True)
    assert not legacy.exists()
    assert lib.automation_is_paused(current)
    stale = lib.automation_pause_path(replace(
        current, control=lib.RuntimeControl.outer_session(stale_token),
    ))
    stale.touch()
    lib.reconcile_automation_pause(current, False)
    assert not lib.automation_is_paused(current)
    assert stale.exists()


@pytest.mark.parametrize(
    "action",
    ["status", "send", "read", "stop", "reset", "migrate", "repair", "pause", "resume"],
)
@pytest.mark.parametrize(
    "dimension",
    ["cwd", "harness", "model", "backend", "mode", "harness_args", "permission", "runtime_home"],
)
def test_every_worker_operation_refuses_a_different_owner_launch(
    action: str, dimension: str, fake_runner_state: Path,
) -> None:
    rec = _outer_controlled(
        _install_agy_agent(fake_runner_state, f"owned-{action}-{dimension}")
    )
    if dimension == "permission":
        with lib.registry_lock() as agents:
            _replace_test_launch(agents[rec.name], harness="codex")
        rec = lib.read_registry()[rec.name]
    token = rec.owner_token
    assert token is not None
    contract = rec.launch_contract()
    values: dict[str, object] = {
        "cwd": contract.cwd,
        "harness": contract.harness,
        "model": contract.model,
        "backend": contract.backend,
        "mode": contract.mode,
        "harness_args": contract.harness_args,
        "permission_mode": contract.permission_mode,
        "runtime_home": contract.runtime_home,
    }
    values[dimension if dimension != "permission" else "permission_mode"] = {
        "cwd": str(fake_runner_state / "other"),
        "harness": "codex",
        "model": "different-model",
        "backend": "herdr",
        "mode": "tui",
        "harness_args": ("--different",),
        "permission": "bypass",
        "runtime_home": str(fake_runner_state / "other-runtime"),
    }[dimension]
    mismatched = lib.RuntimeLaunchContract.create(
        **values,  # type: ignore[arg-type]
    )
    request: dict[str, object] = {
        "schema": "agentctl-worker-rpc/v2",
        "action": action,
        "name": rec.name,
        "owner_token": token,
        "desired_paused": action == "pause",
        "owner_launch": mismatched.to_document(),
    }
    request.update({
        "send": {"text": "must not send", "model": None},
        "read": {"mode": "tail", "since_turn": None, "tail": 10},
        "migrate": {"backend": "herdr", "mode": "tui"},
    }.get(action, {}))
    before = lib.REGISTRY.read_bytes()

    with pytest.raises(lib.AgentOperationError) as raised:
        worker_rpc.dispatch(request)

    assert raised.value.code == "owner_launch_mismatch"
    assert lib.REGISTRY.read_bytes() == before


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


def test_staged_activation_receipt_binds_committed_runner_generation(
    fake_runner_state: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rec = _install_agy_agent(fake_runner_state, "staged-identity")
    staged = CustomProcessIdentity(
        version=1, boot_id="11111111-2222-3333-4444-555555555555",
        pid=4242, starttime_ticks=9001, executable_device=3,
        executable_inode=5,
    )
    replacement = replace(staged, pid=4243, starttime_ticks=9002)
    token = "stage-token"
    generation = cast(str, rec.control_generation)
    fingerprint = rec.launch_fingerprint()
    monkeypatch.setattr(
        lib, "process_identity_liveness",
        lambda identity, **_kwargs: (
            lib.ProcessLiveness.LIVE
            if identity in (staged, replacement) else lib.ProcessLiveness.DEAD
        ),
    )
    lib.write_staged_runner(rec.name, token, staged)
    with lib.registry_lock() as agents:
        current = agents[rec.name]
        current.runner_identity = staged
        current.runner_pid = staged.pid
        current.runner_started_at = str(staged.starttime_ticks)

    with pytest.raises(lib.AgentOperationError) as raised:
        lib.acknowledge_staged_runner_activation(
            rec.name, token, replacement, generation, fingerprint,
        )
    assert raised.value.code == "runner_authority_mismatch"
    assert not lib.staged_runner_activated_path(rec.name, token).exists()

    lib.acknowledge_staged_runner_activation(
        rec.name, token, staged, generation, fingerprint,
    )
    assert lib.staged_runner_activation_acknowledged(
        rec.name, token, staged, generation, fingerprint,
    )
    assert not lib.staged_runner_activation_acknowledged(
        rec.name, token, staged, "replacement-generation", fingerprint,
    )
