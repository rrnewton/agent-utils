"""Differential: Rust `agentctl mcp` speaks the same protocol as Python.

#545 rust-primary-agentctl-chat-readers. Both stdio servers get the same
JSON-RPC conversation against an empty registry; replies must agree after
parsing (the wire encodings may differ in whitespace).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]

REQUESTS = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}},
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "agent_list", "arguments": {}}},
    {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "agent_status", "arguments": {"name": True}}},
    {"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "agent_nope", "arguments": {}}},
    {"jsonrpc": "2.0", "id": 6, "method": "bogus"},
]


def _serve(launcher: Path, registry: Path) -> list[dict[str, object]]:
    result = subprocess.run(
        [str(launcher), "--registry", str(registry), "--herdr-bin", "/unavailable-herdr", "mcp"],
        input="".join(json.dumps(item) + "\n" for item in REQUESTS),
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
    )
    assert result.returncode == 0, result.stderr
    return [json.loads(line) for line in result.stdout.splitlines()]


def test_rust_mcp_matches_python(tmp_path: Path) -> None:
    python_replies = _serve(AU_ROOT / "py" / "bin" / "agentctl", tmp_path / "py-registry")
    rust_replies = _serve(AU_ROOT / "rs" / "bin" / "agentctl", tmp_path / "rs-registry")
    assert rust_replies == python_replies
