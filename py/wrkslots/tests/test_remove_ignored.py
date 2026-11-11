"""Removal must not silently delete gitignored files (#1302).

A fixture slot whose checkout holds a gitignored evidence file refuses
removal by default, naming the file; ``--archive-ignored`` keeps the
files in a verified tar beside the salvage bundle and lists each with
its size and sha256 in the salvage receipt; ``--discard-ignored``
deletes them explicitly and still lists them. Ignored content inside
the known regenerable caches (the cache globs clean-caches shares, and
a checkout's own cargo ``target``) never blocks removal.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import tarfile
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_remove_write_rust import _fixture, _git


def _remove(root: Path, *, archive_ignored: bool = False, discard_ignored: bool = False) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a",
        coordinator_authorized=False, validate_complete=False,
        coordinator_pid=os.getpid(), expected_generation=1,
        salvage_archive_root=None, validation_proof_manifest=None,
        completed_record=None, attest_agent_stopped=None,
        attest_agent_never_started=None, attest_note=None,
        archive_drifted_checkouts=False, adopt_legacy_change=False, wait_lock=0.0,
        no_lock_budget=False,
        archive_ignored=archive_ignored, discard_ignored=discard_ignored,
    )
    production = wrkslots_cli._AGENT_REMOVE_BATCH_ITEM_SECONDS
    wrkslots_cli._AGENT_REMOVE_BATCH_ITEM_SECONDS = 3600.0
    try:
        return wrkslots_cli._cmd_remove(args)
    finally:
        wrkslots_cli._AGENT_REMOVE_BATCH_ITEM_SECONDS = production


def _evidence_fixture(tmp_path: Path) -> tuple[Path, Path]:
    """A fixture slot with ``ignored/evidence/note.bin`` gitignored."""

    root = _fixture(tmp_path)
    checkout = root / "worktrees" / "slot-a" / "repo"
    (checkout / ".gitignore").write_text("ignored/\n")
    _git(checkout, "add", ".gitignore")
    _git(checkout, "commit", "-q", "-m", "ignore evidence")
    _git(checkout, "push", "-q", "origin", "HEAD:suite/work")
    evidence = checkout / "ignored" / "evidence" / "note.bin"
    evidence.parent.mkdir(parents=True)
    evidence.write_bytes(b"evidence-bytes\n")
    return root, evidence


def _archived_salvage(root: Path) -> list[dict[str, object]]:
    archived = json.loads((root / "worktrees" / "ARCHIVED.node-a.json").read_text())
    records = archived["records"]
    assert len(records) == 1
    salvage: list[dict[str, object]] = records[0]["salvage"]
    return salvage


def test_remove_refuses_ignored_evidence_by_default(tmp_path: Path) -> None:
    root, evidence = _evidence_fixture(tmp_path / "case")
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        _remove(root)
    message = str(caught.value)
    assert "ignored/evidence/note.bin" in message
    assert "--archive-ignored" in message
    assert "--discard-ignored" in message
    assert evidence.exists()
    assert (root / "worktrees" / "slot-a").exists()


def test_remove_archives_ignored_evidence_when_asked(tmp_path: Path) -> None:
    root, evidence = _evidence_fixture(tmp_path / "case")
    expected_sha256 = hashlib.sha256(b"evidence-bytes\n").hexdigest()
    assert _remove(root, archive_ignored=True) == 0
    assert not (root / "worktrees" / "slot-a").exists()

    salvage = _archived_salvage(root)
    assert len(salvage) == 1
    assert salvage[0]["ignored_files"] == [
        {
            "path": "ignored/evidence/note.bin",
            "bytes": len(b"evidence-bytes\n"),
            "sha256": expected_sha256,
            "disposition": "archived",
        }
    ]
    archive = salvage[0]["ignored_archive"]
    assert isinstance(archive, dict)
    tar_path = root / "worktrees" / str(archive["path"])
    assert tar_path.is_file()
    observed = tar_path.read_bytes()
    assert hashlib.sha256(observed).hexdigest() == archive["sha256"]
    assert len(observed) == archive["bytes"]
    with tarfile.open(fileobj=io.BytesIO(observed), mode="r:") as bundle:
        members = bundle.getmembers()
        assert [member.name for member in members] == ["repo/ignored/evidence/note.bin"]
        extracted = bundle.extractfile(members[0])
        assert extracted is not None
        assert extracted.read() == b"evidence-bytes\n"


def test_remove_discards_ignored_evidence_when_asked(tmp_path: Path) -> None:
    root, _evidence = _evidence_fixture(tmp_path / "case")
    expected_sha256 = hashlib.sha256(b"evidence-bytes\n").hexdigest()
    assert _remove(root, discard_ignored=True) == 0
    assert not (root / "worktrees" / "slot-a").exists()

    salvage = _archived_salvage(root)
    assert len(salvage) == 1
    assert salvage[0]["ignored_files"] == [
        {
            "path": "ignored/evidence/note.bin",
            "bytes": len(b"evidence-bytes\n"),
            "sha256": expected_sha256,
            "disposition": "discarded",
        }
    ]
    assert "ignored_archive" not in salvage[0]
    assert not list(root.rglob("ignored-*.tar"))


def test_remove_never_blocks_on_cache_only_ignored_content(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    checkout = root / "worktrees" / "slot-a" / "repo"
    (checkout / ".gitignore").write_text("target/\nnode_modules/\n")
    _git(checkout, "add", ".gitignore")
    _git(checkout, "commit", "-q", "-m", "ignore caches")
    _git(checkout, "push", "-q", "origin", "HEAD:suite/work")
    (checkout / "target" / "debug").mkdir(parents=True)
    (checkout / "target" / "debug" / "build.bin").write_bytes(b"cache\n")
    (checkout / "node_modules" / "pkg").mkdir(parents=True)
    (checkout / "node_modules" / "pkg" / "index.js").write_text("cache\n")
    # node_modules is a cache only once the project configures it, the
    # same definition clean-caches uses; target is covered by default.
    config_path = root / ".wrkslots.yml"
    config_path.write_text(
        config_path.read_text() + "cache_globs:\n  - node_modules\n"
    )
    assert _remove(root) == 0
    assert not (root / "worktrees" / "slot-a").exists()
    salvage = _archived_salvage(root)
    assert "ignored_files" not in salvage[0]
