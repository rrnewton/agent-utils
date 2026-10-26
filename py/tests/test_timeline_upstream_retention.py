"""The archive is the record and upstream is a feed: retention and compression upstream are expected.

Four situations a months-long archive meets, one test group each:

* a rollout or transcript upstream has rotated away -- the archive keeps it and says so;
* an upstream file compressed in place (``X`` -> ``X.zst``) -- read as if it were not;
* upstream content changed in place for records already imported -- still a refusal;
* new records appended after the last import -- appended, and kept after upstream retires them.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

from wrkviz.build_store import team_build_root
from wrkviz.claude import ClaudeParseError, snapshot_claude_lineage
from wrkviz.codex import CodexParseError
from wrkviz.compressed_sources import (
    compressed_sibling,
    iter_logical_glob,
    logical_path,
    materialize_source,
    read_first_line,
    read_source_bytes,
    resolve_source,
)
from wrkviz.model import TeamData
from wrkviz.orc import OrcParseError, snapshot_orc_lineage
from wrkviz.pipeline import IngestReport, ingest_codex, ingest_orc
from tests.test_timeline_source_snapshots import (
    CHILD,
    ROOT,
    _child_bytes,
    _line,
    _manifest,
    _origin,
    _root_bytes,
    _snapshot,
)
from tests.test_timeline_claude import FIXTURE_ROOT, _session
from tests.test_timeline_orc import ROOT as ORC_ROOT
from tests.test_timeline_orc import _fixture as _orc_fixture
from tests.timeline_snapshots import snapshot_root

ZSTD = shutil.which("zstd")
needs_zstd = pytest.mark.skipif(ZSTD is None, reason="the zstd command is not installed")
TEAM = "codex-test"


def _compress_in_place(path: Path) -> Path:
    """Do what the operator did: replace ``X`` with ``X.zst``."""

    assert ZSTD is not None
    subprocess.run([ZSTD, "--quiet", "--rm", str(path)], check=True)
    assert not path.exists()
    return compressed_sibling(path)


def _manifest_sources(archive: Path) -> dict[str, dict[str, object]]:
    raw = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    return {str(source["source_path"]): source for source in raw["sources"]}


def _ingest(sessions: Path, archive: Path) -> tuple[TeamData, IngestReport]:
    return ingest_codex(archive, sessions, ROOT, TEAM, "UTC")


def _lineage(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    root = _origin(sessions)
    root.parent.mkdir(parents=True)
    root.write_bytes(_root_bytes())
    child = root.parent / "rollout-child.jsonl"
    child.write_bytes(_child_bytes())
    _ingest(sessions, archive)
    return sessions, archive, root, child


# --- rotated away ----------------------------------------------------------------------------


def test_rotated_away_rollout_is_retired_not_refused(tmp_path: Path) -> None:
    sessions, archive, root, child = _lineage(tmp_path)
    snapshot_before = _snapshot(archive).read_bytes()
    record_before = _manifest_sources(archive)[root.relative_to(sessions).as_posix()]
    root.unlink()

    team, report = _ingest(sessions, archive)

    relative = root.relative_to(sessions).as_posix()
    assert report.upstream_retired_sources == (relative,)
    assert {agent.thread_id for agent in team.agents} == {ROOT, CHILD}
    assert _snapshot(archive).read_bytes() == snapshot_before
    record = _manifest_sources(archive)[relative]
    retired_at = record.pop("upstream_retired_at")
    assert isinstance(retired_at, str) and retired_at
    assert record == record_before
    assert "upstream_retired_at" not in _manifest_sources(archive)[
        child.relative_to(sessions).as_posix()
    ]

    # Sticky: the run that noticed is the date recorded, not every run since.
    _, again = _ingest(sessions, archive)
    assert again.upstream_retired_sources == (relative,)
    assert _manifest_sources(archive)[relative]["upstream_retired_at"] == retired_at


def test_whole_lineage_retired_upstream_still_ingests(tmp_path: Path) -> None:
    sessions, archive, root, child = _lineage(tmp_path)
    root.unlink()
    child.unlink()

    team, report = _ingest(sessions, archive)

    assert len(report.upstream_retired_sources) == 2
    assert {agent.thread_id for agent in team.agents} == {ROOT, CHILD}


def test_retired_rollout_whose_archive_copy_is_gone_refuses(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    manifest_before = _manifest(archive).read_bytes()
    root.unlink()
    _snapshot(archive).unlink()

    with pytest.raises(CodexParseError, match="archive's own snapshot is missing"):
        _ingest(sessions, archive)
    assert _manifest(archive).read_bytes() == manifest_before


def test_retired_rollout_whose_archive_copy_was_altered_refuses(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    root.unlink()
    snapshot = _snapshot(archive)
    snapshot.write_bytes(snapshot.read_bytes().replace(b"Root done", b"Root gone"))

    with pytest.raises(CodexParseError, match="does not match its manifest record"):
        _ingest(sessions, archive)


def test_retired_claude_transcript_keeps_the_archived_lineage(tmp_path: Path) -> None:
    live = tmp_path / "live"
    shutil.copytree(FIXTURE_ROOT, live)
    snapshot = tmp_path / "snapshot"
    first = snapshot_claude_lineage(_session(live), snapshot, (), "2026-01-03T00:00:00Z")

    shutil.rmtree(live)
    live.mkdir()
    retired = snapshot_claude_lineage(
        _session(live), snapshot, first.sources, "2026-01-04T00:00:00Z"
    )

    assert retired.files_changed == 0
    assert set(retired.upstream_retired) == {source.source_path for source in first.sources}
    assert [
        (source.source_path, source.sha256, source.copied_bytes) for source in retired.sources
    ] == [(source.source_path, source.sha256, source.copied_bytes) for source in first.sources]
    assert {source.upstream_retired_at for source in retired.sources} == {
        "2026-01-04T00:00:00Z"
    }

    (snapshot / _session(live).name).unlink()
    with pytest.raises(ClaudeParseError, match="archive's own snapshot is missing"):
        snapshot_claude_lineage(_session(live), snapshot, retired.sources, "2026-01-05T00:00:00Z")


def test_never_imported_claude_session_without_upstream_is_still_an_error(
    tmp_path: Path,
) -> None:
    with pytest.raises(ClaudeParseError, match="not a regular file"):
        snapshot_claude_lineage(
            _session(tmp_path / "empty"), tmp_path / "snapshot", (), "2026-01-03T00:00:00Z"
        )


# --- compressed in place ---------------------------------------------------------------------


def test_resolution_prefers_the_plain_spelling(tmp_path: Path) -> None:
    plain = tmp_path / "a.jsonl"
    assert resolve_source(plain) is None
    compressed_sibling(plain).write_bytes(b"x")
    assert resolve_source(plain) == compressed_sibling(plain)
    plain.write_bytes(b"y")
    assert resolve_source(plain) == plain
    assert logical_path(compressed_sibling(plain)) == plain
    assert iter_logical_glob(tmp_path, "*.jsonl") == [plain]
    with pytest.raises(FileNotFoundError):
        read_source_bytes(tmp_path / "missing.jsonl")


@needs_zstd
def test_compressed_file_reads_as_its_logical_bytes(tmp_path: Path) -> None:
    plain = tmp_path / "rollout.jsonl"
    data = b'{"a":1}\n{"b":2}\n'
    plain.write_bytes(data)
    _compress_in_place(plain)

    assert read_source_bytes(plain) == data
    assert read_first_line(plain) == b'{"a":1}\n'
    copy = materialize_source(plain, tmp_path / "scratch")
    assert copy.read_bytes() == data and copy.parent == tmp_path / "scratch"


@needs_zstd
def test_compressed_upstream_rollout_is_not_a_change(tmp_path: Path) -> None:
    sessions, archive, root, child = _lineage(tmp_path)
    manifest_before = _manifest(archive).read_bytes()
    _compress_in_place(root)
    _compress_in_place(child)

    _, report = _ingest(sessions, archive)

    assert report.upstream_retired_sources == ()
    assert report.files_changed == 0
    assert _manifest(archive).read_bytes() == manifest_before


@needs_zstd
def test_first_ingest_can_start_from_compressed_upstream(tmp_path: Path) -> None:
    sessions = tmp_path / "sessions"
    root = _origin(sessions)
    root.parent.mkdir(parents=True)
    root.write_bytes(_root_bytes())
    _compress_in_place(root)

    team, report = _ingest(sessions, tmp_path / "archive")

    assert {agent.thread_id for agent in team.agents} == {ROOT}
    assert _snapshot(tmp_path / "archive").read_bytes() == _root_bytes()
    record = _manifest_sources(tmp_path / "archive")[root.relative_to(sessions).as_posix()]
    assert record["original_path"] == str(root.resolve())
    assert report.upstream_retired_sources == ()


@needs_zstd
def test_compressed_upstream_rewritten_in_place_still_refuses(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    snapshot_before = _snapshot(archive).read_bytes()
    root.write_bytes(root.read_bytes().replace(b"Root done", b"Root gone"))
    _compress_in_place(root)

    with pytest.raises(CodexParseError, match="existing prefix was rewritten"):
        _ingest(sessions, archive)
    assert _snapshot(archive).read_bytes() == snapshot_before


# --- changed in place ------------------------------------------------------------------------


def test_upstream_changed_in_place_refuses_and_keeps_the_archive(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    snapshot_before = _snapshot(archive).read_bytes()
    manifest_before = _manifest(archive).read_bytes()
    root.write_bytes(root.read_bytes().replace(b"Root done", b"Root gone"))

    with pytest.raises(CodexParseError, match="existing prefix was rewritten"):
        _ingest(sessions, archive)
    assert _snapshot(archive).read_bytes() == snapshot_before
    assert _manifest(archive).read_bytes() == manifest_before


# --- incremental append after a checkpoint ---------------------------------------------------


def test_append_after_checkpoint_is_kept_once_upstream_retires_it(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    relative = root.relative_to(sessions).as_posix()
    checkpoint = _manifest_sources(archive)[relative]
    appended = _line(
        1_020,
        "response_item",
        {
            "type": "message",
            "id": "root-late",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "Appended after the checkpoint"}],
        },
    )
    root.write_bytes(root.read_bytes() + appended)

    team, report = _ingest(sessions, archive)
    advanced = _manifest_sources(archive)[relative]
    assert report.files_changed > 0
    assert advanced["copied_bytes"] == int(str(checkpoint["copied_bytes"])) + len(appended)
    assert _snapshot(archive).read_bytes().endswith(appended)
    assert any("Appended after the checkpoint" in (event.text or "") for event in team.events)

    root.unlink()
    retired_team, retired = _ingest(sessions, archive)
    assert retired.upstream_retired_sources == (relative,)
    assert _manifest_sources(archive)[relative]["copied_bytes"] == advanced["copied_bytes"]
    assert any(
        "Appended after the checkpoint" in (event.text or "") for event in retired_team.events
    )
    assert (team_build_root(archive, TEAM) / "raw" / "team.json").is_file()


# --- Orc -------------------------------------------------------------------------------------


@needs_zstd
def test_compressed_orc_databases_snapshot_identically(tmp_path: Path) -> None:
    source, root_db, task_db = _orc_fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ORC_ROOT, snapshot, (), "first")
    for database in (root_db, task_db, source / ".orc" / "index.db"):
        _compress_in_place(database)

    second = snapshot_orc_lineage(source, ORC_ROOT, snapshot, first.sources, "second")

    assert second.files_changed == 0
    assert [(s.source_path, s.sha256) for s in second.sources] == [
        (s.source_path, s.sha256) for s in first.sources
    ]


def test_orc_lineage_retired_upstream_keeps_its_archive(tmp_path: Path) -> None:
    source, _, _ = _orc_fixture(tmp_path)
    archive = tmp_path / "archive"
    team, _ = ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")
    manifest = team_build_root(archive, "orc-test") / "raw" / "source-manifest.json"
    manifest_before = manifest.read_bytes()
    shutil.rmtree(source)

    retired_team, report = ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")

    assert set(report.upstream_retired_sources) == {
        str(raw["source_path"]) for raw in json.loads(manifest_before)["sources"]
    }
    assert manifest.read_bytes() == manifest_before
    assert retired_team.events == team.events
    assert retired_team.agents == team.agents


def test_orc_lineage_retired_with_damaged_archive_copy_refuses(tmp_path: Path) -> None:
    source, _, _ = _orc_fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")
    shutil.rmtree(source)
    objects = sorted((snapshot_root(archive, "orc-test") / ".objects").rglob("*.db"))
    objects[0].write_bytes(b"not the database the manifest recorded")

    with pytest.raises(OrcParseError, match="does not match its manifest"):
        ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")
