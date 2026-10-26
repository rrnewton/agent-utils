"""Differential: Rust `agentctl chat history` reads Python bridge state.

#545 rust-primary-agentctl-chat-readers. The fixture state is built by the
Python polling bridge itself (its reply_storage 2 artifacts), then both
editions read it: the Python `read_reply_history` and the Rust binary through
its tracked launcher. The parsed documents must agree exactly, including a
non-ASCII reply body (the Python CLI escapes it; the Rust reader must too).
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from agentctl.chat import _read, read_reply_history
from agentctl.chat_output import PaneOutputSnapshot
from agentctl.jsonx import as_mapping, as_sequence
from tests.test_herdr_chat import setup


AU_ROOT = Path(__file__).resolve().parents[2]
RUST_LAUNCHER = AU_ROOT / "rs" / "bin" / "agentctl"


def _records(state: Path) -> dict[str, dict[str, object]]:
    records = [_read(path) for path in (state / "requests").glob("*.json")]
    return {str(as_mapping(record["message"], "message")["id"]).rsplit("/", 1)[-1]: record
            for record in records}


def _rust_history(state: Path, request: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(RUST_LAUNCHER), "chat", "history", "--bridge-state", str(state),
         "--request", request],
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
    )


def test_rust_history_matches_python_on_bridge_state(tmp_path: Path) -> None:
    bridge, _harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    nonce = str(record["reply_nonce"])
    block = f"<CHAT_REPLY_{nonce}_1>\nretained body — café\n</CHAT_REPLY_{nonce}_1>"
    bridge.capture_output(PaneOutputSnapshot("w1:p1", block, 0))
    key = str(record["key"])

    expected = read_reply_history(tmp_path, key[:12])
    assert [as_mapping(item, "history item")["text"]
            for item in as_sequence(expected["items"], "history")] == ["retained body — café"]

    result = _rust_history(tmp_path, key[:12])
    assert result.returncode == 0, result.stderr
    import json

    actual = json.loads(result.stdout)
    assert actual == expected


def test_rust_history_refuses_like_python(tmp_path: Path) -> None:
    bridge, _harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    key = str(record["key"])

    result = _rust_history(tmp_path, "zzzzzzzzzzzz")
    assert result.returncode == 1
    assert "hexadecimal key or unique prefix" in result.stderr

    result = _rust_history(tmp_path, key[:12] + "0" * 52)
    assert result.returncode == 1
    assert "found 0" in result.stderr
