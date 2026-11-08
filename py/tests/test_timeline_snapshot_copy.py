"""Snapshot writes share the upstream file's blocks where they can, and are exact where they can't."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

import pytest

import wrkviz.snapshot_copy as snapshot_copy_module
from wrkviz.compressed_sources import compressed_sibling
from wrkviz.pipeline import ingest_codex, ingest_orc
from wrkviz.snapshot_copy import (
    COPY_REASONS,
    SnapshotCopyTally,
    counting_snapshot_copies,
    write_snapshot_if_changed,
)
from tests.test_timeline_orc import ROOT as ORC_ROOT
from tests.test_timeline_orc import _fixture as _orc_fixture
from tests.test_timeline_source_snapshots import (
    ROOT,
    _first_ingest,
    _root_bytes,
    _snapshot,
)

BLOCK = 4096


def _reflink_supported(directory: Path) -> bool:
    """Ask `cp`, not the code under test, whether this filesystem can share blocks."""

    source = directory / "probe-source"
    target = directory / "probe-target"
    source.write_bytes(b"x" * BLOCK)
    try:
        completed = subprocess.run(
            ["cp", "--reflink=always", str(source), str(target)],
            capture_output=True,
            check=False,
        )
        return completed.returncode == 0
    finally:
        source.unlink()
        target.unlink(missing_ok=True)


def _lines(count: int) -> bytes:
    return b"".join(b'{"n":%06d,"pad":"%s"}\n' % (index, b"p" * 40) for index in range(count))


def _write(tmp_path: Path, data: bytes, origin: Path | None) -> tuple[Path, SnapshotCopyTally]:
    target = tmp_path / "store" / "snap.jsonl"
    with counting_snapshot_copies() as tally:
        assert write_snapshot_if_changed(target, data, origin)
    return target, tally


def test_snapshot_holds_exactly_the_validated_prefix(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(400)
    upstream.write_bytes(complete + b'{"partial":')

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.files == 1
    assert tally.reflinked_bytes + tally.copied_bytes == len(complete)
    if _reflink_supported(tmp_path):
        aligned = len(complete) // BLOCK * BLOCK
        assert tally.reflinked_bytes == aligned
        assert tally.copied_bytes_by_reason == {"tail": len(complete) - aligned}
        assert tally.refused_files == 0


def test_whole_file_is_shared_including_its_unaligned_end(tmp_path: Path) -> None:
    if not _reflink_supported(tmp_path):
        pytest.skip("filesystem under tmp_path cannot share blocks")
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(123)
    upstream.write_bytes(complete)

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.reflinked_bytes == len(complete)
    assert tally.copied_bytes == 0


def test_later_upstream_change_does_not_reach_the_snapshot(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(400)
    upstream.write_bytes(complete)
    target, _ = _write(tmp_path, complete, upstream)

    # Rewrite the first block in place, then append, on the very inode the snapshot was cloned
    # from: copy-on-write must give those writes new blocks and leave the shared ones alone.
    with upstream.open("r+b") as handle:
        handle.write(b"X" * BLOCK)
        handle.seek(0, os.SEEK_END)
        handle.write(_lines(50))
        handle.flush()
        os.fsync(handle.fileno())

    assert upstream.read_bytes()[:BLOCK] == b"X" * BLOCK
    assert target.read_bytes() == complete


def test_upstream_rewritten_before_the_clone_keeps_the_validated_bytes(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    validated = _lines(400)
    # Same length, different first block: what the importer validated is no longer what is there.
    upstream.write_bytes(b"Y" * BLOCK + validated[BLOCK:])

    target, tally = _write(tmp_path, validated, upstream)

    assert target.read_bytes() == validated
    assert tally.reflinked_bytes == 0
    assert tally.copied_bytes == len(validated)
    if _reflink_supported(tmp_path):
        assert tally.copied_bytes_by_reason["source-changed"] == len(validated)


def test_refused_clone_is_written_out_and_counted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(snapshot_copy_module, "_clone_file", lambda *_args: False)
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(400)
    upstream.write_bytes(complete + b"{")

    target, tally = _write(tmp_path, complete, upstream)

    aligned = len(complete) // BLOCK * BLOCK
    assert target.read_bytes() == complete
    assert tally.reflinked_bytes == 0
    assert tally.refused_files == 1
    assert tally.copied_bytes_by_reason == {
        "refused": aligned,
        "tail": len(complete) - aligned,
    }


def test_compressed_upstream_is_written_out_and_counted(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(200)
    upstream.write_bytes(complete)
    completed = subprocess.run(
        ["zstd", "-q", "--rm", str(upstream)], capture_output=True, check=False
    )
    if completed.returncode != 0:
        pytest.skip("zstd command unavailable")
    assert compressed_sibling(upstream).is_file()

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.copied_bytes_by_reason == {"compressed-source": len(complete)}


def test_unchanged_snapshot_is_not_rewritten_or_counted(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(10)
    upstream.write_bytes(complete)
    target, _ = _write(tmp_path, complete, upstream)
    inode = target.stat().st_ino

    with counting_snapshot_copies() as tally:
        assert not write_snapshot_if_changed(target, complete, upstream)

    assert target.stat().st_ino == inode
    assert tally == SnapshotCopyTally()


def test_nested_tallies_each_see_the_write(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    upstream.write_bytes(_lines(10))
    with counting_snapshot_copies() as outer:
        with counting_snapshot_copies() as inner:
            write_snapshot_if_changed(tmp_path / "a", _lines(10), upstream)
        write_snapshot_if_changed(tmp_path / "b", _lines(10), None)

    assert inner.files == 1
    assert outer.files == 2
    assert outer.copied_bytes_by_reason.get("no-source") == len(_lines(10))


def test_ingest_receipt_records_reflinked_and_copied_bytes(tmp_path: Path) -> None:
    # `_first_ingest` does not return its report, so the measured ingest is a second archive.
    sessions, archive, origin = _first_ingest(tmp_path)

    _, report = ingest_codex(archive / "second", sessions, ROOT, "codex-test", "UTC")

    receipt = report.to_json_obj()["snapshot_copy"]
    assert isinstance(receipt, dict)
    assert set(receipt) == {
        "files",
        "reflinked_bytes",
        "copied_bytes",
        "copied_bytes_by_reason",
        "refused_files",
    }
    reflinked, copied = receipt["reflinked_bytes"], receipt["copied_bytes"]
    assert isinstance(reflinked, int) and isinstance(copied, int)
    assert receipt["files"] == 1
    assert reflinked + copied == len(_root_bytes())
    reasons = receipt["copied_bytes_by_reason"]
    assert isinstance(reasons, dict) and tuple(reasons) == COPY_REASONS
    assert _snapshot(archive / "second").read_bytes() == origin.read_bytes()
    if _reflink_supported(tmp_path):
        assert reflinked == len(_root_bytes())

    # A re-ingest with nothing new writes nothing, and the receipt says zero rather than omitting.
    _, repeat = ingest_codex(archive / "second", sessions, ROOT, "codex-test", "UTC")
    assert repeat.to_json_obj()["snapshot_copy"] == json.loads(
        json.dumps(SnapshotCopyTally().to_json_obj())
    )


def _require_reflink(tmp_path: Path) -> None:
    if not _reflink_supported(tmp_path):
        pytest.skip("filesystem under tmp_path cannot share blocks; content was still checked")


def _changed_at(data: bytes, offset: int) -> bytes:
    flipped = b"#" if data[offset : offset + 1] != b"#" else b"%"
    return data[:offset] + flipped + data[offset + 1 :]


def test_a_tail_that_differs_upstream_is_written_from_the_validated_bytes(tmp_path: Path) -> None:
    complete = _lines(400)
    aligned = len(complete) // BLOCK * BLOCK
    upstream = tmp_path / "upstream.jsonl"
    # Only the unaligned tail differs, and a partial line follows, so the prefix is not the whole
    # file: the shared blocks are right and the tail must come from memory, not from the clone.
    upstream.write_bytes(_changed_at(complete, aligned + 10) + b'{"partial":')

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    _require_reflink(tmp_path)
    assert tally.reflinked_bytes == aligned
    assert tally.copied_bytes_by_reason == {"tail": len(complete) - aligned}


def test_a_later_shared_block_that_differs_is_caught_by_the_read_back(tmp_path: Path) -> None:
    complete = _lines(400)
    aligned = len(complete) // BLOCK * BLOCK
    upstream = tmp_path / "upstream.jsonl"
    upstream.write_bytes(_changed_at(complete, 2 * BLOCK + 100) + b'{"partial":')

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.reflinked_bytes == 0
    _require_reflink(tmp_path)
    assert tally.copied_bytes_by_reason == {
        "source-changed": aligned,
        "tail": len(complete) - aligned,
    }


def test_a_difference_past_the_first_read_back_chunk_is_caught(tmp_path: Path) -> None:
    complete = _lines(150_000)
    assert len(complete) > snapshot_copy_module._VERIFY_CHUNK + BLOCK
    aligned = len(complete) // BLOCK * BLOCK
    upstream = tmp_path / "upstream.jsonl"
    upstream.write_bytes(
        _changed_at(complete, snapshot_copy_module._VERIFY_CHUNK + 2 * BLOCK) + b'{"partial":'
    )

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.reflinked_bytes == 0
    _require_reflink(tmp_path)
    assert tally.copied_bytes_by_reason == {
        "source-changed": aligned,
        "tail": len(complete) - aligned,
    }


def test_a_store_on_another_filesystem_is_refused_and_written_out(tmp_path: Path) -> None:
    shm = Path("/dev/shm")
    if not shm.is_dir() or shm.stat().st_dev == tmp_path.stat().st_dev:
        pytest.skip("no second filesystem at /dev/shm")
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(400)
    upstream.write_bytes(complete + b"{")
    aligned = len(complete) // BLOCK * BLOCK
    store = Path(tempfile.mkdtemp(prefix="wrkviz-snapshot-copy-", dir=shm))
    try:
        # A real cross-device clone: the kernel answers EXDEV, which must be a counted refusal.
        with counting_snapshot_copies() as tally:
            assert write_snapshot_if_changed(store / "snap.jsonl", complete, upstream)
        assert (store / "snap.jsonl").read_bytes() == complete
    finally:
        shutil.rmtree(store)
    assert tally.reflinked_bytes == 0
    assert tally.refused_files == 1
    assert tally.copied_bytes_by_reason == {"refused": aligned, "tail": len(complete) - aligned}


def test_an_upstream_gone_since_it_was_resolved_is_written_out_as_no_source(
    tmp_path: Path,
) -> None:
    upstream = tmp_path / "upstream.jsonl"
    # A dangling symlink passes the existence check that resolves the source and then fails to
    # open, which is exactly the window of a source removed between the two.
    upstream.symlink_to(tmp_path / "gone.jsonl")
    complete = _lines(50)

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.copied_bytes_by_reason == {"no-source": len(complete)}
    assert tally.refused_files == 0


def test_an_upstream_shorter_than_the_validated_bytes_is_source_changed(tmp_path: Path) -> None:
    upstream = tmp_path / "upstream.jsonl"
    complete = _lines(400)
    upstream.write_bytes(complete[:10])

    target, tally = _write(tmp_path, complete, upstream)

    assert target.read_bytes() == complete
    assert tally.copied_bytes_by_reason == {"source-changed": len(complete)}


def test_orc_receipts_say_their_objects_are_not_tallied(tmp_path: Path) -> None:
    source, _, _ = _orc_fixture(tmp_path)
    _, report = ingest_orc(tmp_path / "archive", source, ORC_ROOT, "orc-test", "UTC")

    assert report.snapshot_copy is None
    assert report.to_json_obj()["snapshot_copy"] is None
