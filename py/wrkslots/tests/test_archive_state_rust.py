"""Differential: wrkslotsd parse-archive against the Python archive loader.

#562 rust-primary-wrkslots slice 4. Fixture projects carry an
ARCHIVED.<machine>.json shard; the Python loader's output
(`_archive_to_obj` over `_load_archive_snapshot`) and the Rust command
must agree, including refusal messages.
"""

from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64
FINISHED = "2026-10-26T02:00:00Z"


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


def _record(slot: str, agent: str) -> dict[str, object]:
    return {
        "archive_id": f"node-a:{slot}:1:{FINISHED}",
        "slot": slot,
        "agent": agent,
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "finished_at": FINISHED,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["gate ok"],
        "limitations": [],
        "continuation": "none needed",
        "checkouts": [
            {
                "name": "repo",
                "path": f"worktrees/{slot}/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": SHA,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": SHA,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _write_archive(root: Path, archive: dict[str, object]) -> Path:
    path = root / "worktrees" / "ARCHIVED.node-a.json"
    path.write_text(json.dumps(archive))
    return path


def _archive(*records: dict[str, object]) -> dict[str, object]:
    return {"schema": 2, "machine": "node-a", "revision": 3, "records": list(records)}


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_archive_snapshot(config)
    return wrkslots_cli._archive_to_obj(state)


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-archive", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_archive_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_archive(root, _archive(_record("slot-a", "agent-a"), _record("slot-b", "agent-b")))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def _records(archive: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], archive["records"])


def _checkout0(archive: dict[str, object]) -> dict[str, object]:
    return cast(list[dict[str, object]], _records(archive)[0]["checkouts"])[0]


@pytest.mark.parametrize(
    ("mutate", "fragment"),
    [
        (lambda archive: archive.update(schema=3), "unsupported archive state schema in"),
        (lambda archive: _records(archive)[0].update(bogus=1), "has invalid fields"),
        (lambda archive: _records(archive).append(_records(archive)[0]), "duplicate archive_id in"),
        (lambda archive: _records(archive)[0].update(archive_id="node-a:slot-a:9:x"), "does not match its record"),
        (lambda archive: _records(archive)[0].update(mode="keep"), "mode is invalid"),
        (lambda archive: _records(archive)[0].update(actor="agent"), "does not match its operation"),
        (lambda archive: _records(archive)[0].update(validation=[]), "has no validation evidence"),
        (
            lambda archive: _checkout0(archive).update(path="worktrees/slot-a/other"),
            "path escaped its archived slot",
        ),
    ],
)
def test_parse_archive_refusals_name_the_same_problem(
    tmp_path: Path, mutate: Callable[[dict[str, object]], None], fragment: str
) -> None:
    root = _root(tmp_path)
    archive = _archive(_record("slot-a", "agent-a"))
    mutate(archive)
    _write_archive(root, archive)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_archive_missing_shard(tmp_path: Path) -> None:
    root = _root(tmp_path)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert "missing archive state" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "missing archive state" in result.stderr
