"""run-agent-prompt fallback tests over fake harness executables only."""

from __future__ import annotations

import json
import os
import sys
import time
from collections.abc import Mapping
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run_agent_prompt import __version__
from run_agent_prompt.cli import main

PROMPT = "judge this fact set"


def write_fake(directory: Path, harness: str, mode: str, answer: str = "") -> Path:
    """Write one fake harness executable recording argv/env/prompt beside it."""

    path = directory / f"fake-{harness}"
    body = f'''#!/usr/bin/env python3
import json
import os
import subprocess
import sys
import time
from pathlib import Path

HARNESS = {harness!r}
MODE = {mode!r}
ANSWER = {answer!r}
here = Path(__file__).resolve().parent
log = here / f"fake-{{HARNESS}}.log"
prompt_seen = ""
if HARNESS == "muse":
    prompt_seen = Path(sys.argv[sys.argv.index("--prompt-file") + 1]).read_text(encoding="utf-8")
else:
    prompt_seen = sys.stdin.read()
with log.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"argv": sys.argv, "prompt": prompt_seen, "env_keys": sorted(os.environ)}}) + "\\n")
if MODE == "fail":
    print(f"fake failure while handling {{prompt_seen}}", file=sys.stderr)
    raise SystemExit(9)
if MODE == "timeout":
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
    (here / "grandchild.pid").write_text(str(child.pid), encoding="utf-8")
    time.sleep(30)
    raise SystemExit(0)
if HARNESS == "muse":
    text = "" if MODE == "empty" else ANSWER
    print(json.dumps({{"payload": {{"kind": "run_usage", "models": [{{"model": "fake-muse-model", "input_tokens": 1, "cached_tokens": 0, "output_tokens": 1, "reasoning_tokens": 0}}]}}}}))
    print(json.dumps({{"payload": {{"kind": "run_terminal", "terminal": "completed", "reason": None, "text": text}}}}))
elif HARNESS == "claude":
    print(json.dumps({{"type": "result", "subtype": "success", "is_error": False, "result": "" if MODE == "empty" else ANSWER, "model": "fake-claude-model"}}))
else:
    out = Path(sys.argv[sys.argv.index("--output-last-message") + 1])
    out.write_text("" if MODE == "empty" else ANSWER, encoding="utf-8")
    print(json.dumps({{"type": "turn.completed", "model": "fake-codex-model"}}))
'''
    path.write_text(body, encoding="utf-8")
    path.chmod(0o700)
    return path


def run_cli(tmp_path: Path, commands: Mapping[str, Path | str], *extra: str) -> int:
    args = ["--timeout", "5", "--total-timeout", "30"]
    for harness, command in commands.items():
        args += [f"--{harness}-command", str(command)]
    args += list(extra)
    prompt_path = tmp_path / "prompt.txt"
    if not prompt_path.exists():
        prompt_path.write_text(PROMPT, encoding="utf-8")
    return main([*args, "--prompt-file", str(prompt_path)])


def invocations(directory: Path, harness: str) -> list[dict[str, object]]:
    log = directory / f"fake-{harness}.log"
    if not log.exists():
        return []
    return [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines() if line]


def string_list(value: object) -> list[str]:
    assert isinstance(value, list)
    return [str(part) for part in value]


def test_embedded_docs_and_version(capsys: pytest.CaptureFixture[str]) -> None:
    package = Path(__file__).resolve().parents[1] / "run_agent_prompt"
    assert main(["--userguide"]) == 0
    captured = capsys.readouterr()
    assert captured.out == (package / "USER_GUIDE.md").read_text(encoding="utf-8")
    assert captured.err == ""

    assert main(["quickstart"]) == 0
    captured = capsys.readouterr()
    assert captured.out == (package / "QUICKSTART.md").read_text(encoding="utf-8")
    assert captured.err == ""

    with pytest.raises(SystemExit) as raised:
        main(["--version"])
    assert raised.value.code == 0
    captured = capsys.readouterr()
    assert __version__ in captured.out
    assert captured.err == ""


def test_muse_wins_and_others_never_run(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    commands = {
        harness: write_fake(tmp_path, harness, "ok", f"{harness} answer")
        for harness in ("muse", "claude", "codex")
    }
    assert run_cli(tmp_path, commands) == 0
    captured = capsys.readouterr()
    assert captured.out == "muse answer\n"
    assert "harness=muse status=answered" in captured.err
    assert "model=fake-muse-model" in captured.err
    assert invocations(tmp_path, "claude") == []
    assert invocations(tmp_path, "codex") == []


def test_fallback_reaches_codex_last_message_file(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    commands = {
        "muse": write_fake(tmp_path, "muse", "fail"),
        "claude": write_fake(tmp_path, "claude", "empty"),
        "codex": write_fake(tmp_path, "codex", "ok", "codex answer"),
    }
    cwd = tmp_path / "caller-cwd"
    cwd.mkdir()
    assert run_cli(tmp_path, commands, "--cwd", str(cwd)) == 0
    assert not (cwd / "codex-last-message.txt").exists()
    captured = capsys.readouterr()
    assert captured.out == "codex answer\n"
    assert "harness=muse status=failed" in captured.err
    assert 'detail="fake failure while handling [prompt redacted]"' in captured.err
    assert PROMPT not in captured.err
    assert "harness=claude status=empty" in captured.err
    assert "harness=codex status=answered" in captured.err


def test_all_fail_is_69_with_empty_stdout(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    commands: dict[str, Path | str] = {
        "muse": write_fake(tmp_path, "muse", "fail"),
        "claude": write_fake(tmp_path, "claude", "fail"),
        "codex": "/nonexistent/codex",
    }
    assert run_cli(tmp_path, commands) == 69
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "harness=codex status=spawn-error" in captured.err
    assert 'detail="executable not found"' in captured.err
    assert "every harness failed" in captured.err


def test_timeout_kills_the_whole_group_and_next_harness_wins(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    commands = {
        "muse": write_fake(tmp_path, "muse", "timeout"),
        "claude": write_fake(tmp_path, "claude", "ok", "after timeout"),
        "codex": write_fake(tmp_path, "codex", "ok", "unused"),
    }
    started = time.monotonic()
    assert run_cli(tmp_path, commands, "--timeout", "0.2") == 0
    assert time.monotonic() - started < 10
    assert capsys.readouterr().out == "after timeout\n"
    grandchild = int((tmp_path / "grandchild.pid").read_text(encoding="utf-8"))
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(grandchild, 0)
        except ProcessLookupError:
            break
        time.sleep(0.05)
    else:
        raise AssertionError(f"grandchild {grandchild} survived the group kill")


def test_prompt_and_environment_never_leak(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SECRET_SHOULD_NOT_LEAK", "x")
    commands = {
        harness: write_fake(tmp_path, harness, "ok", "safe answer")
        for harness in ("muse", "claude", "codex")
    }
    log_file = tmp_path / "attempts.jsonl"
    assert run_cli(tmp_path, commands, "--log-file", str(log_file)) == 0
    captured = capsys.readouterr()
    seen = invocations(tmp_path, "muse")[0]
    assert seen["prompt"] == PROMPT
    assert PROMPT not in " ".join(string_list(seen["argv"]))
    assert "SECRET_SHOULD_NOT_LEAK" not in string_list(seen["env_keys"])
    assert PROMPT not in captured.err
    records = [
        json.loads(line) for line in log_file.read_text(encoding="utf-8").splitlines()
    ]
    assert records[0]["harness"] == "muse"
    assert records[0]["detail"] is None
    assert PROMPT not in log_file.read_text(encoding="utf-8")


def test_model_and_effort_translation_per_harness(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    commands: dict[str, Path | str] = {
        "muse": write_fake(tmp_path, "muse", "fail"),
        "claude": write_fake(tmp_path, "claude", "fail"),
        "codex": write_fake(tmp_path, "codex", "fail"),
    }
    config = tmp_path / "config.toml"
    config.write_text(
        'effort = "high"\n[claude]\nmodel = "cfg-claude"\n[codex]\nmodel = "cfg-codex"\n',
        encoding="utf-8",
    )
    assert (
        run_cli(
            tmp_path,
            commands,
            "--config",
            str(config),
            "--muse-model",
            "flag-muse",
            "--muse-effort",
            "ultra",
        )
        == 69
    )
    capsys.readouterr()
    muse_argv = string_list(invocations(tmp_path, "muse")[0]["argv"])
    assert muse_argv[muse_argv.index("--model") + 1] == "flag-muse"
    assert muse_argv[muse_argv.index("--reasoning-effort") + 1] == "ultra"
    claude_argv = string_list(invocations(tmp_path, "claude")[0]["argv"])
    assert claude_argv[claude_argv.index("--model") + 1] == "cfg-claude"
    assert claude_argv[claude_argv.index("--effort") + 1] == "high"
    codex_argv = string_list(invocations(tmp_path, "codex")[0]["argv"])
    assert codex_argv[codex_argv.index("--model") + 1] == "cfg-codex"
    assert 'model_reasoning_effort="high"' in codex_argv
