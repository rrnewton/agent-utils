"""Differential: wrkslotsd parse-hold against the Python hold loader.

#562 rust-primary-wrkslots slice 6. Fixture projects may carry a
HOLD.<machine>.<slot>.json record; `_load_hold_snapshot` and the Rust
command must agree, including refusal messages.
"""

from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    return root


def _hold() -> dict[str, object]:
    return {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-a",
        "held_at": "2026-10-26T02:00:00Z",
        "reason": "owner asked to pause",
    }


def _write_hold(root: Path, payload: dict[str, object]) -> Path:
    path = root / "worktrees" / "HOLD.6.node-a.slot-a.json"
    path.write_text(json.dumps(payload))
    return path


def _expected(root: Path) -> dict[str, object] | None:
    config = wrkslots_cli._load_config(str(root), None)
    return wrkslots_cli._load_hold_snapshot(config, "slot-a")


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-hold", "--root", str(root), "--slot", "slot-a"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_hold_absent_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) is None
    assert _expected(root) is None


def test_parse_hold_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_hold(root, _hold())
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


@pytest.mark.parametrize(
    ("mutate", "fragment"),
    [
        (lambda hold: hold.update(schema=2), "unsupported slot hold schema in"),
        (lambda hold: hold.update(bogus=1), "has invalid fields"),
        (lambda hold: hold.update(machine="node-b"), "machine does not match its filename"),
        (lambda hold: hold.update(slot="slot-b"), "name does not match its filename"),
        (lambda hold: hold.update(reason="  "), "has an empty reason"),
        (lambda hold: hold.update(held_at="soon"), "invalid timestamp in"),
    ],
)
def test_parse_hold_refusals_name_the_same_problem(
    tmp_path: Path, mutate: Callable[[dict[str, object]], None], fragment: str
) -> None:
    root = _root(tmp_path)
    hold = _hold()
    mutate(hold)
    _write_hold(root, hold)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr
