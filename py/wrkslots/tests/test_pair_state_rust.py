"""Differential: wrkslotsd parse-states against `_ensure_state_shard`.

#562 rust-primary-wrkslots slice 17. The Python oracle runs the state
bootstrap on a copy of the fixture project (it writes the missing
counterpart there) and reads back both shards; the Rust command must
report the same pair read-only, including partial-shard refusals.
Fixtures carry no EVENTS log and empty shards, so the bootstrap's
repository checks do not engage; shard documents themselves are
validated in reference mode like parse-state/parse-archive. Slice 20
classifies the log by its entries: an in-progress write of the first
event file refuses with the bootstrap's partial-update message on
both sides (a later sequence's in-progress write does not block the
bootstrap's first append and stays in snapshot mode); a complete
event file refuses on the Rust side toward `rebuild`, and any other
entry refuses with the event loader's message.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
EMPTY_ACTIVE: dict[str, object] = {"schema": 2, "machine": "node-a", "revision": 0, "slots": []}
EMPTY_ARCHIVE: dict[str, object] = {"schema": 2, "machine": "node-a", "revision": 0, "records": []}


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


def _write(root: Path, payload: dict[str, object], archive: bool) -> None:
    name = "ARCHIVED.node-a.json" if archive else "ACTIVE.node-a.json"
    (root / "worktrees" / name).write_text(json.dumps(payload))


def _expected(root: Path) -> dict[str, object]:
    oracle = root.parent / f"{root.name}-oracle"
    shutil.copytree(root, oracle, dirs_exist_ok=True)
    config = wrkslots_cli._load_config(str(oracle), None)
    wrkslots_cli._ensure_state_shard(config)
    control = oracle / "worktrees"
    return {
        "machine": "node-a",
        "active": json.loads((control / "ACTIVE.node-a.json").read_text()),
        "archive": json.loads((control / "ARCHIVED.node-a.json").read_text()),
    }


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-states", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


@pytest.mark.parametrize(
    "setup",
    ["both-absent", "both-present", "active-only", "archive-only"],
)
def test_parse_states_matches_python(tmp_path: Path, setup: str) -> None:
    root = _root(tmp_path)
    if setup in {"both-present", "active-only"}:
        _write(root, EMPTY_ACTIVE, archive=False)
    if setup in {"both-present", "archive-only"}:
        _write(root, EMPTY_ARCHIVE, archive=True)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


@pytest.mark.parametrize(
    ("payload", "archive", "fragment"),
    [
        (
            {"schema": 2, "machine": "node-a", "revision": 3, "slots": []},
            False,
            "partial machine state for node-a: non-empty ACTIVE has no ARCHIVED",
        ),
        (
            {"schema": 2, "machine": "node-a", "revision": 2, "records": []},
            True,
            "partial machine state for node-a: non-empty ARCHIVED has no ACTIVE",
        ),
    ],
)
def test_parse_states_partial_refusals(
    tmp_path: Path, payload: dict[str, object], archive: bool, fragment: str
) -> None:
    root = _root(tmp_path)
    _write(root, payload, archive=archive)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_states_partial_event_write_refuses_like_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(root, EMPTY_ACTIVE, archive=False)
    _write(root, EMPTY_ARCHIVE, archive=True)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json.tmp.abc").write_text("{}")
    fragment = "partial state update exists for 00000000000000000001.json"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_states_later_partial_event_write_stays_snapshot(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    _write(root, EMPTY_ACTIVE, archive=False)
    _write(root, EMPTY_ARCHIVE, archive=True)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000002.json.tmp.abc").write_text("{}")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_states_event_log_refusals(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(root, EMPTY_ACTIVE, archive=False)
    _write(root, EMPTY_ARCHIVE, archive=True)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "notes.txt").write_text("{}")
    result = _rust(root)
    assert result.returncode == 1
    assert "append-only event log contains an unexpected entry" in result.stderr


def test_parse_states_valid_event_log_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(root, EMPTY_ACTIVE, archive=False)
    _write(root, EMPTY_ARCHIVE, archive=True)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    core: dict[str, object] = {
        "schema": 1,
        "machine": "node-a",
        "sequence": 1,
        "previous_sha256": "0" * 64,
        "recorded_at": "2026-10-26T03:00:00Z",
        "kind": "state-imported",
        "payload": {"active": EMPTY_ACTIVE, "archive": EMPTY_ARCHIVE, "holds": []},
    }
    event = {**core, "sha256": wrkslots_cli._event_digest(core)}
    (events / "00000000000000000001.json").write_text(json.dumps(event))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


@pytest.mark.parametrize(
    ("name", "fragment"),
    [
        ("00000000000000000001.json", "append-only event has invalid fields"),
        ("00000000000000000002.json", "append-only event log has a sequence gap at"),
    ],
)
def test_parse_states_corrupt_event_log_refuses_like_python(
    tmp_path: Path, name: str, fragment: str
) -> None:
    root = _root(tmp_path)
    _write(root, EMPTY_ACTIVE, archive=False)
    _write(root, EMPTY_ARCHIVE, archive=True)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / name).write_text("{}")
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_states_shard_error_precedes_partial_refusal(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(
        root,
        {"schema": 2, "machine": "node-a", "revision": 3, "slots": []},
        archive=False,
    )
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json.tmp.abc").write_text("{}")
    fragment = "partial machine state for node-a: non-empty ACTIVE has no ARCHIVED"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr
