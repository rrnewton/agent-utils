"""Differential: wrkslotsd ensure-states against the Python bootstrap.

#562 rust-primary-wrkslots slice 26, the first writer slice. The
Python oracle is `_ensure_state_shard` run on a copy of the fixture
registry; the Rust writer runs on the original. Resulting shard
bytes, event names, and event payloads must match; the only tolerated
differences are the event's `recorded_at` (and therefore its digest,
which is re-verified against the Python digest of the Rust event) and
lock-file presence. Temporary fixture registries only: writers are
never pointed at a live registry before the edition cutover.
"""

from __future__ import annotations

import fcntl
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
EMPTY_ACTIVE = {"schema": 2, "machine": "node-a", "revision": 0, "slots": []}
EMPTY_ARCHIVE = {"schema": 2, "machine": "node-a", "revision": 0, "records": []}


def _root(base: Path, name: str) -> Path:
    root = base / name
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


def _write(root: Path, payload: dict[str, object], *, archive: bool) -> None:
    name = "ARCHIVED.node-a.json" if archive else "ACTIVE.node-a.json"
    (root / "worktrees" / name).write_text(json.dumps(payload))


def _rust(root: Path, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "ensure-states", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _event_names(root: Path) -> list[str]:
    events = root / "worktrees" / "EVENTS.node-a"
    return sorted(path.name for path in events.iterdir()) if events.is_dir() else []


def _assert_same_registry(rust_root: Path, oracle_root: Path) -> None:
    for name in ("ACTIVE.node-a.json", "ARCHIVED.node-a.json"):
        rust_path = rust_root / "worktrees" / name
        oracle_path = oracle_root / "worktrees" / name
        assert rust_path.exists() == oracle_path.exists(), name
        if rust_path.exists():
            assert rust_path.read_bytes() == oracle_path.read_bytes(), name
    assert _event_names(rust_root) == _event_names(oracle_root)
    for name in _event_names(rust_root):
        rust_event = json.loads((rust_root / "worktrees" / "EVENTS.node-a" / name).read_text())
        oracle_event = json.loads(
            (oracle_root / "worktrees" / "EVENTS.node-a" / name).read_text()
        )
        assert rust_event["sha256"] == wrkslots_cli._event_digest(
            {k: v for k, v in rust_event.items() if k != "sha256"}
        )
        for event in (rust_event, oracle_event):
            event.pop("recorded_at")
            event.pop("sha256")
        assert rust_event == oracle_event


def _run_pair(tmp_path: Path, setup: str) -> tuple[Path, Path]:
    rust_root = _root(tmp_path, "rust")
    oracle_root = _root(tmp_path, "oracle")
    for root in (rust_root, oracle_root):
        if setup == "lone-active":
            _write(root, EMPTY_ACTIVE, archive=False)
        elif setup == "lone-archive":
            _write(root, EMPTY_ARCHIVE, archive=True)
        elif setup == "both":
            _write(root, EMPTY_ACTIVE, archive=False)
            _write(root, EMPTY_ARCHIVE, archive=True)
    return rust_root, oracle_root


def test_ensure_states_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    root = _root(tmp_path, "plain")
    result = _rust(root, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert not (root / "worktrees" / "ACTIVE.node-a.json").exists()


def test_ensure_states_matches_python_when_lock_held(tmp_path: Path) -> None:
    root = _root(tmp_path, "locked")
    control = root / "worktrees"
    control.mkdir(parents=True, exist_ok=True)
    with (control / "ACTIVE.global.lock").open("w") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = _rust(root)
    assert result.returncode == 1
    assert "state lock is busy" in result.stderr
    assert not (control / "ACTIVE.node-a.json").exists()


def _write_event(
    root: Path, sequence: int, previous: str, payload: dict[str, object]
) -> dict[str, object]:
    event: dict[str, object] = {
        "schema": 1,
        "machine": "node-a",
        "sequence": sequence,
        "previous_sha256": previous,
        "recorded_at": "2026-01-01T00:00:00+00:00",
        "kind": "state-imported",
        "payload": payload,
    }
    event["sha256"] = wrkslots_cli._event_digest(event)
    directory = root / "worktrees" / "EVENTS.node-a"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{sequence:020d}.json").write_text(json.dumps(event) + "\n")
    return event


def test_ensure_states_refuses_multi_import_derived_log(tmp_path: Path) -> None:
    root = _root(tmp_path, "multi")
    payload: dict[str, object] = {"active": EMPTY_ACTIVE, "archive": EMPTY_ARCHIVE, "holds": []}
    first = _write_event(root, 1, "0" * 64, payload)
    _write_event(root, 2, str(first["sha256"]), payload)
    result = _rust(root)
    assert result.returncode == 1
    assert "imports state more than once" in result.stderr
    assert not (root / "worktrees" / "ACTIVE.node-a.json").exists()


def test_ensure_states_refuses_junk_event_entry_without_writing(tmp_path: Path) -> None:
    root = _root(tmp_path, "junk")
    directory = root / "worktrees" / "EVENTS.node-a"
    directory.mkdir(parents=True)
    (directory / "junk").write_text("x")
    result = _rust(root)
    assert result.returncode == 1
    assert "unexpected entry" in result.stderr
    assert not (root / "worktrees" / "ACTIVE.node-a.json").exists()


@pytest.mark.parametrize("setup", ["neither", "lone-active", "lone-archive", "both"])
def test_ensure_states_matches_python(tmp_path: Path, setup: str) -> None:
    rust_root, oracle_root = _run_pair(tmp_path, setup)
    config = wrkslots_cli._load_config(str(oracle_root), None)
    wrkslots_cli._ensure_state_shard(config)
    result = _rust(rust_root)
    assert result.returncode == 0, result.stderr
    _assert_same_registry(rust_root, oracle_root)


def test_ensure_states_derived_import_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _run_pair(tmp_path, "neither")
    for root in (rust_root, oracle_root):
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
        events = root / "worktrees" / "EVENTS.node-a"
        events.mkdir()
        (events / "00000000000000000001.json").write_text(json.dumps(event))
    config = wrkslots_cli._load_config(str(oracle_root), None)
    wrkslots_cli._ensure_state_shard(config)
    result = _rust(rust_root)
    assert result.returncode == 0, result.stderr
    _assert_same_registry(rust_root, oracle_root)


def test_ensure_states_partial_refusal_writes_nothing(tmp_path: Path) -> None:
    rust_root, oracle_root = _run_pair(tmp_path, "neither")
    payload = {"schema": 2, "machine": "node-a", "revision": 3, "slots": []}
    for root in (rust_root, oracle_root):
        _write(root, payload, archive=False)
    config = wrkslots_cli._load_config(str(oracle_root), None)
    with pytest.raises(Exception) as caught:
        wrkslots_cli._ensure_state_shard(config)
    assert "partial machine state for node-a" in str(caught.value)
    result = _rust(rust_root)
    assert result.returncode == 1
    assert "partial machine state for node-a" in result.stderr
    for root in (rust_root, oracle_root):
        names = sorted(
            path.name
            for path in (root / "worktrees").iterdir()
            if not path.name.endswith(".lock")
        )
        assert names == ["ACTIVE.node-a.json"]


def test_ensure_states_is_idempotent(tmp_path: Path) -> None:
    rust_root, oracle_root = _run_pair(tmp_path, "neither")
    first = _rust(rust_root)
    assert first.returncode == 0, first.stderr
    before = {
        path.name: path.read_bytes()
        for path in (rust_root / "worktrees" / "EVENTS.node-a").iterdir()
    }
    second = _rust(rust_root)
    assert second.returncode == 0, second.stderr
    after = {
        path.name: path.read_bytes()
        for path in (rust_root / "worktrees" / "EVENTS.node-a").iterdir()
    }
    assert before == after
    config = wrkslots_cli._load_config(str(oracle_root), None)
    wrkslots_cli._ensure_state_shard(config)
    _assert_same_registry(rust_root, oracle_root)
