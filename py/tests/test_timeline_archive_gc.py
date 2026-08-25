"""Contract tests for retiring a format generation and for reclaiming what it leaves behind.

Two changes are asserted here and they are deliberately in one file, because each is only safe
because of the other. A published build stopped writing ``data/timeline.json``; the reason that
is not a data-loss bug is that nothing deletes the copy an older build already wrote, and the
reason *that* is not a hoarding bug is that `gc` will delete it when an operator says so.

The assertions are organised around the three ways this could go wrong, worst first.

*Something is deleted that a reader still needs.* Every reclaim is gated on a named superseding
artefact being present and complete, and there is one test per gate that damages exactly that
gate and requires `gc` to hold. The strongest of them is the incomplete-generation case: a
rebuild interrupted halfway leaves a bootstrap describing fewer shards than exist, and a `gc`
that trusted it would delete the better half of the rebuild.

*Something is deleted irreversibly by accident.* The first pass is a rename into a trash tree
that mirrors the archive, so the test for it is a *round trip*: sweep, copy the payload back,
and require the archive to read the same as before.

*The accounting silently stops adding up.* The schema-2 manifest's current/retained split is
exact today, and the orphan set is derived as ``on disk - current - retained`` rather than
counted separately, so a test that plants one unreferenced object and one retained object has
to see exactly one of them reclaimed.

Named `test_timeline_*` because the packaged workflow runs `pytest tests/test_timeline_*.py`;
a file outside that glob would run only under `make validate`.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import pytest

from agent_team_timeline.archive_gc import (
    ArchiveGcError,
    TRASH_ROOT,
    collect,
    format_gc_report,
    plan_collection,
)
from agent_team_timeline.cli import _inspect_counts
from agent_team_timeline.cli import main as timeline_main
from agent_team_timeline.multi_team import build_combined_archive
from agent_team_timeline.pipeline import (
    _ensure_bulk_content_ignored,
    build_archive,
    summarize_archive,
)
from agent_team_timeline.query import QueryFilters, TimelineQuery
from agent_team_timeline.render import SCHEMA_1_TIMELINE_PATH
from agent_team_timeline.timeline_shards import SCHEMA_2_BOOTSTRAP_PATH
from agent_team_timeline.timeline_v3 import SCHEMA_3_BOOTSTRAP_PATH, SCHEMA_3_ROOT
from tests.test_timeline_pipeline import _team, _write_team
from tests.timeline_projection import schema_1_timeline_text


_OBJECT_ROOT = "data/timeline-v2/objects"
_V2_MANIFEST = "data/timeline-v2/manifest.json"
_EXPORT_MANIFEST = "data/export.json"


def _built(archive: Path) -> str:
    """One real single-team archive, built the way the pipeline builds one."""

    team = _team()
    _write_team(archive, team)
    summarize_archive(archive, team.team_slug, "heuristic", "test-model")
    build_archive(archive, team.team_slug)
    return team.team_slug


def _categories(archive: Path) -> dict[str, tuple[bool, int]]:
    report = plan_collection(archive)
    return {item.name: (item.reclaimable, item.count) for item in report.categories}


def _answers(archive: Path) -> tuple[object, object]:
    query = TimelineQuery(archive)
    return (
        query.list_records("agents", QueryFilters()),
        query.list_records("phases", QueryFilters()),
    )


# -- what a published build no longer writes -------------------------------------------------


def test_a_published_build_writes_no_schema_one_monolith(tmp_path: Path) -> None:
    """The retirement itself, and the two generations that make it safe.

    Both readers reach schema 1 only as a fallback, and both of the generations in front of it
    are written here, so an archive this tool produces never needs the monolith. The manifest is
    checked as well as the filesystem: a path that vanished from disk but stayed in
    ``generated_files`` would be reported as missing by the next build's stale-file pass.
    """

    _built(tmp_path)

    assert not (tmp_path / SCHEMA_1_TIMELINE_PATH).exists()
    assert not (tmp_path / (SCHEMA_1_TIMELINE_PATH + ".gz")).exists()
    assert (tmp_path / SCHEMA_2_BOOTSTRAP_PATH).is_file()
    assert (tmp_path / SCHEMA_3_BOOTSTRAP_PATH).is_file()

    manifest = json.loads((tmp_path / _EXPORT_MANIFEST).read_text(encoding="utf-8"))
    assert SCHEMA_1_TIMELINE_PATH not in manifest["generated_files"]
    assert manifest["retired_files"] == []
    assert TimelineQuery(tmp_path).schema_3_declined == ""


def test_a_combined_export_writes_no_monolith_and_is_still_readable(tmp_path: Path) -> None:
    """The combined export retires it too, though its per-team intermediate still uses it.

    `multi_team` renders each team into a temporary directory and reads ``data/timeline.json``
    back out of it, so schema 1 is still that render's *output format*. What must not survive is
    a merged monolith in the export an operator keeps.
    """

    archive = tmp_path / "archive"
    output = tmp_path / "export"
    first = _team()
    _write_team(archive, first)
    summarize_archive(archive, first.team_slug, "heuristic", "test-model")
    second = _team("A second team so the combined path is exercised.")
    second = second.__class__(**{**second.__dict__, "team_slug": "second-team"})
    _write_team(archive, second)
    summarize_archive(archive, second.team_slug, "heuristic", "test-model")

    build_combined_archive(
        archive,
        (first.team_slug, second.team_slug),
        output=output,
        display_timezone="UTC",
    )

    assert not (output / SCHEMA_1_TIMELINE_PATH).exists()
    manifest = json.loads((output / _EXPORT_MANIFEST).read_text(encoding="utf-8"))
    assert SCHEMA_1_TIMELINE_PATH not in manifest["generated_files"]
    query = TimelineQuery(output)
    assert query.schema_3_declined == ""
    assert len(query.teams) == 2


def test_a_rebuild_retires_the_older_monolith_instead_of_deleting_it(tmp_path: Path) -> None:
    """The ordering rule: not writing a file and deleting it are separate decisions.

    Left to itself, `_remove_stale_presentation_files` reaps ``previous - current``, and after
    this change the monolith is in every older manifest's ``previous`` and in no new build's
    ``current`` -- so the default behaviour would be a quarter-gigabyte irreversible unlink
    triggered by a build the operator asked for a different reason. The file has to survive the
    rebuild, and it has to be *recorded* as surviving, because a file nobody names is a file
    `gc` cannot classify.
    """

    slug = _built(tmp_path)
    # Stand in for an archive an older tool wrote: the monolith on disk and named by the
    # manifest the next build will read as `previous`.
    monolith = tmp_path / SCHEMA_1_TIMELINE_PATH
    monolith.write_text('{"schema_version": 1}', encoding="utf-8")
    (tmp_path / (SCHEMA_1_TIMELINE_PATH + ".gz")).write_bytes(b"\x1f\x8b old")
    manifest_path = tmp_path / _EXPORT_MANIFEST
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["generated_files"] = sorted(
        {*manifest["generated_files"], SCHEMA_1_TIMELINE_PATH,
         SCHEMA_1_TIMELINE_PATH + ".gz"}
    )
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    build_archive(tmp_path, slug)

    assert monolith.is_file()
    assert monolith.read_text(encoding="utf-8") == '{"schema_version": 1}'
    rebuilt = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert SCHEMA_1_TIMELINE_PATH not in rebuilt["generated_files"]
    assert rebuilt["retired_files"] == [
        SCHEMA_1_TIMELINE_PATH,
        SCHEMA_1_TIMELINE_PATH + ".gz",
    ]


def test_inspect_counts_the_same_records_without_the_monolith(tmp_path: Path) -> None:
    """`inspect` printed six integers by parsing the whole monolith; it now reads the catalogue.

    246,973,399 bytes at 1.44 GiB resident, on the measured archive, to print six integers that
    the 89,298-byte bootstrap already publishes. The differential is the point: the two branches
    have to agree, or the command has quietly started reporting something else, and the natural
    place to get an honest schema-1 input is the combiner's unpublished render -- which is the
    one mode that still writes the monolith and nothing else.
    """

    archive = tmp_path / "archive"
    intermediate = tmp_path / "intermediate"
    slug = _built(archive)
    from_schema_3 = _inspect_counts(archive)

    query = TimelineQuery(archive)
    assert from_schema_3["agents"] == len(query.list_records("agents", QueryFilters()))
    assert from_schema_3["phases"] == len(query.list_records("phases", QueryFilters()))
    assert from_schema_3["rollups"] == len(query.list_records("rollups", QueryFilters()))
    assert from_schema_3["events"] > 0

    build_archive(archive, slug, output=intermediate, _published=False)
    assert (intermediate / SCHEMA_1_TIMELINE_PATH).is_file()
    assert not (intermediate / SCHEMA_3_BOOTSTRAP_PATH).exists()
    assert not (intermediate / SCHEMA_2_BOOTSTRAP_PATH).exists()
    assert _inspect_counts(intermediate) == from_schema_3


def test_the_reconstruction_the_suite_leans_on_is_the_real_monolith(tmp_path: Path) -> None:
    """`tests/timeline_projection.py` now feeds a hundred assertions; here is what backs it.

    Most of this suite still asserts against the schema-1 object, reconstructed from schema 3
    because the file is gone. That reconstruction is only load-bearing if it is *exact*, so it is
    compared here against the genuine article -- the monolith the combiner's unpublished render
    still writes -- collection by collection, as record sets.

    Sets and not sequences: schema 3 sorts a timeline shard by instant and a spine group by the
    kind's identifier, which is not the renderer's insertion order, and the helper says so. What
    must hold is that every record survives the round trip unchanged and none is invented or
    duplicated -- which is also `timeline_v3`'s own losslessness claim, executed.
    """

    archive = tmp_path / "archive"
    intermediate = tmp_path / "intermediate"
    slug = _built(archive)
    build_archive(archive, slug, output=intermediate, _published=False)

    real = json.loads((intermediate / SCHEMA_1_TIMELINE_PATH).read_text(encoding="utf-8"))
    rebuilt = json.loads(schema_1_timeline_text(archive))

    assert sorted(real) == sorted(rebuilt)
    for field, value in sorted(real.items()):
        if isinstance(value, list):
            assert sorted(json.dumps(item, sort_keys=True) for item in value) == sorted(
                json.dumps(item, sort_keys=True) for item in rebuilt[field]
            ), field
        else:
            assert value == rebuilt[field], field


# -- what gc reclaims, and what it refuses to ------------------------------------------------


def test_a_dry_run_touches_nothing_and_explains_every_category(tmp_path: Path) -> None:
    _built(tmp_path)
    before = sorted(path.relative_to(tmp_path).as_posix() for path in tmp_path.rglob("*"))

    report = plan_collection(tmp_path)

    assert report.action == "dry-run"
    assert report.moved == ()
    assert sorted(
        path.relative_to(tmp_path).as_posix() for path in tmp_path.rglob("*")
    ) == before
    assert {item.name for item in report.categories} == {
        "schema-1-monolith",
        "schema-2-orphan-objects",
        "schema-2-retained-objects",
        "schema-3-orphan-shards",
        "ingest-source-snapshots",
        "trash",
    }
    # Every category carries its reason even when it is empty, so a zero is never unexplained.
    assert all(item.reason for item in report.categories)
    text = format_gc_report(report, "text")
    assert "this was a dry run" in text
    assert TRASH_ROOT in text


def test_the_cli_defaults_to_a_dry_run(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    _built(tmp_path)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text("{}", encoding="utf-8")

    assert timeline_main(["gc", "--output", str(tmp_path)]) == 0

    assert (tmp_path / SCHEMA_1_TIMELINE_PATH).is_file()
    assert "dry run" in capsys.readouterr().out


def test_the_monolith_is_held_without_a_complete_schema_three(tmp_path: Path) -> None:
    """Clause one of the gate: the superseding generation must be readable as a whole.

    This is the state the measured archive is in before its migration -- schema 1 and schema 2
    and no schema 3 -- and reclaiming there would delete the only complete presentation
    generation the command-line reader can use.
    """

    _built(tmp_path)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text("{}", encoding="utf-8")
    (tmp_path / SCHEMA_3_BOOTSTRAP_PATH).unlink()

    report = plan_collection(tmp_path)
    monolith = report.category("schema-1-monolith")
    assert monolith.count == 1
    assert not monolith.reclaimable
    assert "no schema-3 bootstrap" in monolith.reason

    collect(tmp_path, delete=True)
    assert (tmp_path / SCHEMA_1_TIMELINE_PATH).is_file()


def test_the_monolith_is_held_when_the_website_has_no_fallback(tmp_path: Path) -> None:
    """Clause two: the *browser* retired schema 1 later than the CLI did.

    `static/app.js` loads schema 2 and falls back to the monolith when that load throws; it has
    no schema-3 mode at all. An archive with schema 3 but no schema 2 is one where reclaiming
    the monolith takes away the thing the fallback falls back to.
    """

    _built(tmp_path)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text("{}", encoding="utf-8")
    (tmp_path / SCHEMA_2_BOOTSTRAP_PATH).unlink()

    monolith = plan_collection(tmp_path).category("schema-1-monolith")
    assert not monolith.reclaimable
    assert "website reads schema 2" in monolith.reason


def test_an_interrupted_rebuild_holds_its_own_shards(tmp_path: Path) -> None:
    """One direction of a half-published generation: a shard shorter than the catalogue says.

    This is the damage the bootstrap's own numbers can see. The direction it *cannot* see is the
    next test, which is the dangerous one.
    """

    _built(tmp_path)
    shard = next((tmp_path / SCHEMA_3_ROOT).rglob("*.jsonl.gz"))
    shard.write_bytes(shard.read_bytes()[:-8])

    shards = plan_collection(tmp_path).category("schema-3-orphan-shards")
    assert not shards.reclaimable
    assert "partly published" in shards.reason
    monolith = plan_collection(tmp_path).category("schema-1-monolith")
    assert not monolith.reclaimable


def test_a_build_that_died_before_its_bootstrap_loses_no_team_and_no_fallback(
    tmp_path: Path,
) -> None:
    """The failure the orphan rule used to cause, reproduced the way it actually happens.

    No hand-edited archive: build a two-team export, keep its bootstrap and its bins shard,
    build a three-team export, and put those two files back. That is exactly the state
    ``write_timeline_v3`` leaves when it raises anywhere between the last spine shard and the
    bootstrap -- ENOSPC, a SIGKILL, a validation error in a later phase of the build -- because
    publication is shards, then the bootstrap, with no atomicity across the set.

    Everything the surviving bootstrap names is present at its declared length, so a rule that
    only checked the catalogue against the disk in one direction called this complete. Two
    consequences followed, and both are asserted against here: the reader answered from the
    older catalogue and silently dropped a team the archive plainly has, and `gc` classified the
    third team's live shards as unreachable *and* declared the schema-1 monolith superseded, so
    one `--delete` plus one `--empty-trash` removed a team from schema 3 and the fallback that
    would have had it.
    """

    archive = tmp_path / "archive"
    output = tmp_path / "export"
    slugs: list[str] = []
    for index, slug in enumerate(("team-one", "team-two", "team-three")):
        team = _team(f"Team {index} narrative.")
        team = team.__class__(**{**team.__dict__, "team_slug": slug})
        _write_team(archive, team)
        summarize_archive(archive, slug, "heuristic", "test-model")
        slugs.append(slug)

    build_combined_archive(archive, tuple(slugs[:2]), output=output, display_timezone="UTC")
    surviving_bootstrap = (output / SCHEMA_3_BOOTSTRAP_PATH).read_bytes()
    bins = output / SCHEMA_3_ROOT / "bins.jsonl.gz"
    surviving_bins = bins.read_bytes()
    surviving_bins_index = bins.with_name(bins.name + ".index.jsonl").read_bytes()
    # An operator's older monolith, so the fallback has something to lose.
    (output / SCHEMA_1_TIMELINE_PATH).write_text(
        json.dumps({"schema_version": 1, "marker": "the fallback"}), encoding="utf-8"
    )

    build_combined_archive(archive, tuple(slugs), output=output, display_timezone="UTC")
    (output / SCHEMA_3_BOOTSTRAP_PATH).write_bytes(surviving_bootstrap)
    bins.write_bytes(surviving_bins)
    bins.with_name(bins.name + ".index.jsonl").write_bytes(surviving_bins_index)

    third_spine = output / SCHEMA_3_ROOT / "spine" / "team-three.jsonl.gz"
    assert third_spine.is_file()

    # The reader refuses the generation rather than answering with two teams out of three.
    query = TimelineQuery(output)
    assert "named by no entry" in query.schema_3_declined
    assert len(query.teams) == 3

    report = plan_collection(output)
    shards = report.category("schema-3-orphan-shards")
    assert not shards.reclaimable
    assert third_spine.relative_to(output).as_posix() in {
        item.relative_path for item in shards.files
    }
    assert not report.category("schema-1-monolith").reclaimable

    collect(output, delete=True)
    assert third_spine.is_file()
    assert (output / SCHEMA_1_TIMELINE_PATH).is_file()

    # And the remedy is the build itself: re-running it clears the residue and the archive is
    # whole again, with no operator decision and nothing in the trash.
    build_combined_archive(archive, tuple(slugs), output=output, display_timezone="UTC")
    assert TimelineQuery(output).schema_3_declined == ""
    assert plan_collection(output).category("schema-3-orphan-shards").count == 0


def test_a_shard_no_bootstrap_names_is_held_and_the_next_build_removes_it(
    tmp_path: Path,
) -> None:
    """Absence from the catalogue is reported, never swept, and cleared by the publisher.

    The planted file stands in for both cases at once, which is the whole argument: from `gc`'s
    position a retired team's leftover shard and a live team's not-yet-named shard are the same
    two bytes on disk. It reports them and holds; the next build -- which is the only thing that
    knows which one it is looking at -- removes them, along with the directory they emptied.
    """

    slug = _built(tmp_path)
    orphan_dir = tmp_path / SCHEMA_3_ROOT / "timeline" / "retired-team"
    orphan_dir.mkdir(parents=True)
    orphan = orphan_dir / "2026-05-04.jsonl.gz"
    orphan.write_bytes(b"\x1f\x8b" + b"0" * 64)
    (orphan_dir / "2026-05-04.jsonl.gz.index.jsonl").write_text("{}\n", encoding="utf-8")

    shards = plan_collection(tmp_path).category("schema-3-orphan-shards")
    assert not shards.reclaimable
    assert {item.relative_path for item in shards.files} == {
        f"{SCHEMA_3_ROOT}/timeline/retired-team/2026-05-04.jsonl.gz",
        f"{SCHEMA_3_ROOT}/timeline/retired-team/2026-05-04.jsonl.gz.index.jsonl",
    }
    # A live team's shards are never in that set.
    assert all(f"/{slug}/" not in item.relative_path for item in shards.files)
    # While it is there the reader falls back rather than trusting a catalogue older than its
    # tree, so the archive is slower but never wrong.
    assert "named by no entry" in TimelineQuery(tmp_path).schema_3_declined

    collect(tmp_path, delete=True)
    assert orphan.is_file()

    build_archive(tmp_path, slug)

    assert not orphan.exists()
    assert not orphan_dir.exists()
    assert TimelineQuery(tmp_path).schema_3_declined == ""
    assert plan_collection(tmp_path).category("schema-3-orphan-shards").count == 0


def test_a_retained_object_is_held_and_an_unreferenced_one_is_reclaimed(
    tmp_path: Path,
) -> None:
    """The schema-2 accounting, extended rather than replaced.

    ``retained_objects`` is the previous generation's objects, kept alive one more generation so
    that a browser which already loaded the previous bootstrap can still fetch what it names.
    `gc` reclaiming them would collapse that grace period to zero at exactly the moment it is
    being used, since the operator most likely to run `gc` has just rebuilt. The orphan set is
    derived as ``on disk - current - retained``, so planting one of each has to reclaim exactly
    one.
    """

    _built(tmp_path)
    manifest_path = tmp_path / _V2_MANIFEST
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    retained = f"{_OBJECT_ROOT}/{'a' * 64}.json"
    orphan = f"{_OBJECT_ROOT}/{'b' * 64}.json"
    (tmp_path / retained).write_text('{"kept": true}', encoding="utf-8")
    (tmp_path / orphan).write_text('{"unreferenced": true}', encoding="utf-8")
    manifest["retained_objects"] = sorted({*manifest["retained_objects"], retained})
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    report = plan_collection(tmp_path)
    assert not report.category("schema-2-retained-objects").reclaimable
    assert retained in {
        item.relative_path for item in report.category("schema-2-retained-objects").files
    }
    orphans = report.category("schema-2-orphan-objects")
    assert orphans.reclaimable
    assert {item.relative_path for item in orphans.files} == {orphan}

    collect(tmp_path, delete=True)
    assert (tmp_path / retained).is_file()
    assert not (tmp_path / orphan).exists()


def test_an_unreadable_v2_manifest_holds_every_object(tmp_path: Path) -> None:
    """An absent reachability record means "an older tool wrote these", not "these are dead".

    `timeline_shards._previous_objects` reads the same absence the same way. A `gc` that read it
    as an empty current set would classify the entire object store as orphaned.
    """

    _built(tmp_path)
    before = sorted((tmp_path / _OBJECT_ROOT).iterdir())
    assert before
    (tmp_path / _V2_MANIFEST).unlink()

    orphans = plan_collection(tmp_path).category("schema-2-orphan-objects")
    assert not orphans.reclaimable
    assert orphans.count == 0
    collect(tmp_path, delete=True)
    assert sorted((tmp_path / _OBJECT_ROOT).iterdir()) == before


def test_source_snapshots_are_measured_and_never_touched(tmp_path: Path) -> None:
    """The largest thing in the archive is reported and refused in the same breath.

    It is 72% of the measured archive's bytes, so omitting it would make the report answer a
    different question than the one asked. It is also vendor input with a purpose-built deletion
    gate of its own, so a second, weaker gate here would be a liability.
    """

    slug = _built(tmp_path)
    snapshots = tmp_path / "teams" / slug / "source_snapshots"
    snapshots.mkdir(parents=True, exist_ok=True)
    (snapshots / "rollout.jsonl").write_text("x" * 4096, encoding="utf-8")

    category = plan_collection(tmp_path).category("ingest-source-snapshots")
    assert not category.reclaimable
    assert category.bytes >= 4096
    assert "audit-losslessness" in category.reason

    collect(tmp_path, delete=True)
    assert (snapshots / "rollout.jsonl").is_file()


# -- reversibility --------------------------------------------------------------------------


def test_a_sweep_is_undone_by_copying_the_trash_back(tmp_path: Path) -> None:
    """The whole reason the first pass renames instead of unlinking, asserted as a round trip.

    The archive costs hours to rebuild, so the test is not "the file is in the trash" but "the
    archive is the same afterwards": sweep, copy the payload tree back over the root, and read
    the same answers out of it.
    """

    _built(tmp_path)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text(
        json.dumps({"schema_version": 1, "marker": "restore me"}), encoding="utf-8"
    )
    before = _answers(tmp_path)

    report = collect(tmp_path, delete=True)

    assert report.action == "swept"
    assert report.trash_generation is not None
    assert not (tmp_path / SCHEMA_1_TIMELINE_PATH).exists()
    payload = tmp_path / report.trash_generation / "files"
    assert (payload / SCHEMA_1_TIMELINE_PATH).is_file()

    receipt = json.loads(
        (tmp_path / report.trash_generation / "receipt.json").read_text(encoding="utf-8")
    )
    assert receipt["kind"] == "archive-gc-trash-receipt"
    assert SCHEMA_1_TIMELINE_PATH in {entry["path"] for entry in receipt["files"]}
    assert "cp -a" in receipt["restore"]

    shutil.copytree(payload, tmp_path, dirs_exist_ok=True)
    restored = json.loads(
        (tmp_path / SCHEMA_1_TIMELINE_PATH).read_text(encoding="utf-8")
    )
    assert restored["marker"] == "restore me"
    assert _answers(tmp_path) == before


def test_emptying_the_trash_is_a_second_command(tmp_path: Path) -> None:
    """Sweeping and emptying in one invocation would reintroduce the irreversibility."""

    _built(tmp_path)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text("{}", encoding="utf-8")

    with pytest.raises(ArchiveGcError, match="separate passes"):
        collect(tmp_path, delete=True, empty_trash=True)
    assert (tmp_path / SCHEMA_1_TIMELINE_PATH).is_file()

    swept = collect(tmp_path, delete=True)
    assert swept.trash_generation is not None
    emptied = collect(tmp_path, empty_trash=True)
    assert emptied.action == "emptied"
    assert emptied.emptied_bytes > 0
    assert not (tmp_path / TRASH_ROOT).exists()


def test_two_sweeps_in_one_second_do_not_share_a_generation(tmp_path: Path) -> None:
    _built(tmp_path)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text("{}", encoding="utf-8")
    first = collect(tmp_path, delete=True)
    (tmp_path / SCHEMA_1_TIMELINE_PATH).write_text("{}", encoding="utf-8")
    second = collect(tmp_path, delete=True)

    assert first.trash_generation != second.trash_generation
    assert (tmp_path / str(first.trash_generation) / "receipt.json").is_file()
    assert (tmp_path / str(second.trash_generation) / "receipt.json").is_file()


def test_the_trash_is_gitignored_before_it_can_exist(tmp_path: Path) -> None:
    """An operator who reclaims 268 MiB must not then be offered it as a commit.

    Asserted against the ingest-time rule rather than against a built archive, because that is
    where the archive's ``.gitignore`` is written and the ordering claim is precisely that the
    entry lands before anything can occupy the directory.
    """

    _ensure_bulk_content_ignored(tmp_path)
    ignored = set((tmp_path / ".gitignore").read_text(encoding="utf-8").splitlines())
    assert f"/{TRASH_ROOT}/" in ignored
    # Idempotent, and it does not disturb what an operator added themselves.
    (tmp_path / ".gitignore").write_text(
        (tmp_path / ".gitignore").read_text(encoding="utf-8") + "/mine\n", encoding="utf-8"
    )
    assert not _ensure_bulk_content_ignored(tmp_path)
    assert "/mine" in (tmp_path / ".gitignore").read_text(encoding="utf-8")


# -- refusals -------------------------------------------------------------------------------


def test_gc_refuses_a_directory_that_is_not_an_archive(tmp_path: Path) -> None:
    with pytest.raises(ArchiveGcError, match="not an agent-team-timeline archive"):
        collect(tmp_path)


def test_gc_waits_for_the_writer_lock(tmp_path: Path) -> None:
    """Even the dry run serializes against a build.

    A report computed beside a running build names files the build is halfway through replacing,
    and the report is the input to a human's decision about deleting them.
    """

    _built(tmp_path)
    ready = tmp_path / "holder-ready"
    holder = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import fcntl, pathlib, sys, time\n"
            "lock = pathlib.Path(sys.argv[1]) / '.agent-team-timeline.lock'\n"
            "handle = open(lock, 'a+b')\n"
            "fcntl.flock(handle.fileno(), fcntl.LOCK_EX)\n"
            "pathlib.Path(sys.argv[2]).write_text('yes')\n"
            "time.sleep(1.5)\n",
            str(tmp_path),
            str(ready),
        ]
    )
    try:
        deadline = time.monotonic() + 20
        while not ready.is_file() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert ready.is_file()
        started = time.monotonic()
        collect(tmp_path)
        assert time.monotonic() - started >= 0.5
    finally:
        holder.wait(timeout=30)


def test_gc_never_follows_a_symlink_out_of_the_archive(tmp_path: Path) -> None:
    """A symlinked shard must not become a way to delete something outside the archive."""

    _built(tmp_path)
    outside = tmp_path.parent / "outside.txt"
    outside.write_text("not ours", encoding="utf-8")
    link = tmp_path / SCHEMA_3_ROOT / "timeline" / "linked.jsonl.gz"
    os.symlink(outside, link)

    report = plan_collection(tmp_path)
    listed = {
        item.relative_path
        for category in report.categories
        for item in category.files
    }
    assert not any("linked.jsonl.gz" in value for value in listed)
    assert outside.is_file()
