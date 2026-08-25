"""Reclaim what a built archive no longer produces, reversibly and under the writer's lock.

An archive is append-heavy by construction. Every build publishes a new generation of the
presentation timeline beside the last, every format generation that was ever written stays
written until something removes it, and the two mechanisms that already remove things --
`render._remove_stale_presentation_files` and `timeline_shards._finish_object_generation` --
only ever reap what the *current* build's own manifest stopped naming. Neither can see a whole
format generation that a newer tool stopped emitting, because a file nobody writes is a file no
manifest lists, and "absent from the manifest" is exactly the signal those two use to mean
"delete". A retired generation therefore falls into the one gap between them: too dead to be
rewritten, too unnamed to be reaped.

This module is that gap, made explicit. It is a **third** user of the same mark-and-sweep idiom
rather than a second idiom:

* :func:`timeline_shards._finish_object_generation` marks with `current_objects`, sweeps
  `previous - current - retained`, and holds a `retained_objects` set: the objects the *previous*
  generation named and this one no longer does, kept alive one more generation so that a browser
  which already loaded the previous bootstrap can still fetch what it points at. That grace is
  gated by `_scope_allows_retention`, which grants it only when the new build covered the same
  teams and a window no narrower -- that is, only when the new generation genuinely supersedes
  the old one. A narrowed re-export gets no grace and takes the older objects with it, which is
  consistent rather than surprising: the archive it leaves behind describes exactly what was
  re-exported. The precedent to carry across is the shape, not the scope rule: **something is
  released only once a named successor has demonstrably taken over its job, and never merely
  because the last build did not mention it.**
* :func:`render.prune_retired_query_artifacts` removes one specific retired output -- the
  generated `query.py` launcher -- and only when it can prove the file is the one the tool
  wrote, either because a manifest claimed it or because its bytes still match the bundled
  source.

`gc` generalises both across *format generations*, and inherits both of their refusals. Nothing
is reclaimed on the strength of "the current build did not write it". Something is reclaimed only
when a **named superseding artefact is present and complete**, checked by the reader's own
acceptance rule, or when a manifest that is authoritative over a directory says the file is in
neither its current nor its retained set.

The rejected design: sweeping everything ``data/export.json`` does not name
-------------------------------------------------------------------------
The export manifest is a complete positive inventory of one build's presentation output, so
"delete everything under the archive that is not in it" looks like the whole feature in one
line. It is refused, for two reasons that are different failures.

The first is scope. The manifest covers the *presentation* tree and nothing else, while the
archive it lives in also holds `teams/`, `runs/`, `extracted/`, `qa/`, the marker, the lock, and
whatever an operator put there -- none of which any manifest names and all of which that rule
would propose deleting. The negation of a partial inventory is not a garbage set.

The second is that it is the very inference this module exists to *avoid*. "The last build did
not write it" is what `_remove_stale_presentation_files` already acts on, correctly, within the
one directory its manifest is authoritative over -- and it is precisely why the schema-1 monolith
had to be given a `retired_files` seat rather than being left to that sweep. Rebuilding the same
inference at archive scope, where nothing is authoritative, would generalise the hazard instead
of the mechanism. So `gc` reasons only forwards: a file goes when something *else*, named and
verified, has taken over its job.

Why the first pass moves rather than unlinks
--------------------------------------------
The measured archive costs hours of model time to rebuild and holds 9.1 GB across roughly forty
thousand files; the single largest thing `gc` can reclaim is a 246,973,399-byte JSON document.
A wrong `unlink` there is not a slow afternoon, it is a re-ingest. So the first destructive
operation is `os.replace` into ``.agent-team-timeline-trash/<generation>/files/<original path>``,
which preserves the archive-relative path exactly, so undoing a sweep is copying one directory
tree back over the archive. The generation directory carries a ``receipt.json`` naming every
file, its category, its size and the reason it was judged dead -- written *before* the first move
as a statement of intent and rewritten after the last one as a record, so that a sweep which dies
partway still leaves the operator a list rather than a traceback. See :func:`_receipt`.

Emptying the trash is a **separate invocation with a separate flag**. That is the whole design:
`--delete` is reversible and `--empty-trash` is not, and no single command does both, because a
command that swept and emptied in one pass would have exactly the failure mode the trash exists
to prevent.

What `gc` will not touch, and why
---------------------------------
* **`teams/*/source_snapshots/`** -- 6,524,876,923 bytes across 995 files on the measured
  archive, 72% of it, and genuinely not output: it is the vendor material ingest reads. It is
  already gitignored and already excluded from every manifest. `gc` reports it, at its full size,
  because an operator asking "what is in my output that is not output" deserves that number as
  the first line of the answer -- and then declines to reclaim it, because the archive has a
  purpose-built gate for exactly that deletion (`audit-losslessness --require-lossless`, whose
  own help says "run this before deleting source snapshots"). A second, weaker gate beside a
  strong one is not a convenience.
* **`retained_objects`** -- the schema-2 objects the last build deliberately kept alive for one
  more generation, so that a reader holding the previous bootstrap can still fetch what it
  names. Reclaiming them would collapse that grace period to zero at exactly the moment it is
  being used, since the operator most likely to run `gc` is the one who has just rebuilt and is
  looking at what the rebuild left behind -- which is when a browser tab from before the rebuild
  is still open. The next build supersedes them and they go on their own.
* **Schema-3 shards the bootstrap does not name.** Reported, never reclaimed. Absence from the
  catalogue does not say whether the shard is a retired team's or a live team's: the answer turns
  entirely on whether the bootstrap beside it is newer or older than the shard, which nothing on
  disk states. So the publisher settles it instead -- `timeline_v3.write_timeline_v3` clears its
  own root of everything outside its plan once its bootstrap is published -- and any leftover
  `gc` can see is therefore residue from a build that did not finish. The remedy is to run one,
  not to sweep. See :func:`_schema_3_category`.
* **Anything at all, when the artefact that would supersede it is absent or incomplete.** The
  measured archive today has no schema-3 generation, so a `gc` run against it reclaims nothing
  from the schema-1 monolith and says so in a sentence. That is the intended behaviour on a
  first run and the reason the report distinguishes "reclaimable" from "held (<reason>)" instead
  of printing one total.

Concurrency
-----------
Both the dry run and the sweep take :func:`pipeline.archive_writer_lock`. The dry run takes it
too -- despite reading nothing -- because its output is the input to a human's decision, and a
listing computed beside a running build names files the build is halfway through replacing.

A reader mid-flight is a different question, and the answer is POSIX rather than a lock: readers
of a built archive take no lock (that is the point of the format), so `gc` can move a file out
from under one. A reader that already has the file open keeps reading it, because the rename
does not disturb the open description; a reader that has not opened it yet gets `ENOENT` on a
file it learned about from a manifest that `gc` has, by construction, already established does
not name it. The residual case -- a reader that read a stale manifest before the sweep and opens
the path after it -- degrades to a missing file rather than a wrong answer, and unlike the
existing stale-file removal it is recoverable from the trash.
"""

from __future__ import annotations

import errno
import os
import re
import stat as stat_module
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

from agent_team_timeline.archive import (
    ARCHIVE_MARKER_FILE,
    JsonValue,
    as_array,
    as_object,
    as_string,
    canonical_json,
    read_json,
    write_text_if_changed,
)
from agent_team_timeline.pipeline import ARCHIVE_TRASH_ROOT, archive_writer_lock
from agent_team_timeline.query import schema_3_completeness
from agent_team_timeline.timeline_shards import SCHEMA_2_BOOTSTRAP_PATH
from agent_team_timeline.timeline_v3 import SCHEMA_3_BOOTSTRAP_PATH, SCHEMA_3_ROOT

#: Where a swept file goes. Dot-prefixed and at the archive root, beside the marker and the lock,
#: because it is control state rather than content; :func:`pipeline._ensure_bulk_content_ignored`
#: adds it to the archive's own ``.gitignore`` for the same reason it adds the lock, and the name
#: is declared there so that the ignore rule and the directory cannot drift apart.
TRASH_ROOT = ARCHIVE_TRASH_ROOT

#: The receipt's own name, kept inside a ``files/`` sibling rather than beside the reclaimed
#: tree, so that reclaiming an archive path that happens to be called ``receipt.json`` can never
#: overwrite the record of why it was reclaimed.
_RECEIPT_NAME = "receipt.json"
_TRASH_PAYLOAD = "files"

_SCHEMA_1_TIMELINE = "data/timeline.json"
_SCHEMA_2_OBJECT_ROOT = "data/timeline-v2/objects"
_SCHEMA_2_MANIFEST = "data/timeline-v2/manifest.json"
_SOURCE_SNAPSHOT_DIRECTORY = "source_snapshots"
_OBJECT_NAME = re.compile(r"[0-9a-f]{64}\.json(?:\.gz)?")
_GENERATION_STAMP = "%Y%m%dT%H%M%SZ"


class ArchiveGcError(ValueError):
    """A collection this module refuses to plan or perform.

    Distinct from a bare ``ValueError`` so the CLI can tell "the archive is not in a state this
    can reason about" from "a narrowing helper found the wrong type", and so a refusal is never
    mistaken for an empty report -- reclaiming nothing and *failing* to decide are opposite
    outcomes that would otherwise print the same zero.
    """


@dataclass(frozen=True)
class GcFile:
    """One classified file: where it is, and how much reclaiming it would return."""

    relative_path: str
    bytes: int

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Render this file for the JSON report and for the trash receipt."""

        return {"path": self.relative_path, "bytes": self.bytes}


@dataclass(frozen=True)
class GcCategory:
    """One reason a set of files is dead, or one reason it only looks dead.

    ``reclaimable`` and ``reason`` are both always populated, and that is deliberate: a category
    with no files still carries its reason, so a dry run that reclaims nothing explains itself
    rather than printing a bare zero the operator has to interpret.
    """

    name: str
    reclaimable: bool
    reason: str
    files: tuple[GcFile, ...]

    @property
    def bytes(self) -> int:
        """How much disk this category holds."""

        return sum(item.bytes for item in self.files)

    @property
    def count(self) -> int:
        """How many files it holds."""

        return len(self.files)

    def to_json_obj(self, *, include_files: bool) -> dict[str, JsonValue]:
        """Render this category, with its paths only when the caller wants the long form."""

        result: dict[str, JsonValue] = {
            "category": self.name,
            "reclaimable": self.reclaimable,
            "reason": self.reason,
            "files": self.count,
            "bytes": self.bytes,
        }
        if include_files:
            listed: list[JsonValue] = [item.to_json_obj() for item in self.files]
            result["paths"] = listed
        return result


@dataclass(frozen=True)
class GcReport:
    """What one ``gc`` invocation found, and what -- if anything -- it did about it."""

    root: str
    action: str
    total_bytes: int
    total_files: int
    categories: tuple[GcCategory, ...]
    trash_relative_path: str
    trash_generation: str | None
    moved: tuple[GcFile, ...]
    removed_directories: tuple[str, ...]
    emptied_bytes: int

    @property
    def reclaimable_bytes(self) -> int:
        """How much this pass would return to the filesystem."""

        return sum(item.bytes for item in self.categories if item.reclaimable)

    @property
    def reclaimable_files(self) -> int:
        """How many files that is."""

        return sum(item.count for item in self.categories if item.reclaimable)

    @property
    def held_bytes(self) -> int:
        """How much is classified but deliberately not reclaimed, with a reason per category."""

        return sum(item.bytes for item in self.categories if not item.reclaimable)

    def category(self, name: str) -> GcCategory:
        """The named category, which always exists -- an absent one would be an unread reason."""

        for item in self.categories:
            if item.name == name:
                return item
        raise KeyError(name)

    def to_json_obj(self, *, include_files: bool = False) -> dict[str, JsonValue]:
        """Render the whole report as the JSON the `--format json` caller parses."""

        categories: list[JsonValue] = [
            item.to_json_obj(include_files=include_files) for item in self.categories
        ]
        moved: list[JsonValue] = [item.to_json_obj() for item in self.moved]
        directories: list[JsonValue] = [value for value in self.removed_directories]
        return {
            "schema_version": 1,
            "kind": "archive-gc-report",
            "archive": self.root,
            "action": self.action,
            "archive_bytes": self.total_bytes,
            "archive_files": self.total_files,
            "reclaimable_bytes": self.reclaimable_bytes,
            "reclaimable_files": self.reclaimable_files,
            "held_bytes": self.held_bytes,
            "trash": self.trash_relative_path,
            "trash_generation": self.trash_generation,
            "moved": moved,
            "removed_directories": directories,
            "emptied_bytes": self.emptied_bytes,
            "categories": categories,
        }


def _archive_path(root: Path, relative: str) -> Path:
    """Resolve one archive-relative path, refusing to leave the archive or cross a symlink.

    The same shape as `render._generated_path` and `timeline_shards._safe_output_path`, repeated
    here for the reason those two are repeated in each other: this module's whole job is to
    *remove* things, so it is the one place where borrowing a path helper whose guarantees are
    stated for a writer would be a mistake worth making explicit rather than importing.
    """

    parts = PurePosixPath(relative)
    if parts.is_absolute() or not parts.parts or any(
        part in ("", ".", "..") for part in parts.parts
    ):
        raise ArchiveGcError(f"unsafe archive path: {relative!r}")
    resolved_root = root.resolve()
    cursor = root
    for part in parts.parts[:-1]:
        cursor = cursor / part
        if cursor.is_symlink():
            raise ArchiveGcError(f"archive path crosses a symlink: {relative!r}")
    candidate = root.joinpath(*parts.parts)
    if candidate.is_symlink():
        raise ArchiveGcError(f"refusing to collect through a symlink: {relative!r}")
    try:
        candidate.parent.resolve().relative_to(resolved_root)
    except ValueError as error:
        raise ArchiveGcError(f"archive path escapes the archive: {relative!r}") from error
    return candidate


def _size_of(root: Path, relative: str) -> int | None:
    """The size of a regular, non-symlinked file, or ``None`` if it is not one."""

    path = root / relative
    try:
        stat_result = path.lstat()
    except OSError:
        return None
    if not stat_module.S_ISREG(stat_result.st_mode):
        return None
    return stat_result.st_size


def _walk_files(root: Path, relative_root: str) -> Iterator[tuple[str, int]]:
    """Every regular file under *relative_root*, as ``(archive-relative path, bytes)``.

    Symlinked directories are not descended into and symlinked files are not reported, so a
    listing can never grow a path that leaves the archive -- which is the property that lets
    everything downstream treat a reported path as collectable.
    """

    start = root / relative_root if relative_root else root
    if not start.is_dir() or start.is_symlink():
        return
    for directory, subdirectories, names in os.walk(start, followlinks=False):
        subdirectories[:] = sorted(
            name
            for name in subdirectories
            if not Path(directory, name).is_symlink()
        )
        for name in sorted(names):
            path = Path(directory, name)
            try:
                stat_result = path.lstat()
            except OSError:
                continue
            if not stat_module.S_ISREG(stat_result.st_mode):
                continue
            yield path.relative_to(root).as_posix(), stat_result.st_size


def _files_with_sizes(root: Path, relatives: Iterable[str]) -> tuple[GcFile, ...]:
    found: list[GcFile] = []
    for relative in sorted(set(relatives)):
        size = _size_of(root, relative)
        if size is not None:
            found.append(GcFile(relative, size))
    return tuple(found)


def _schema_1_category(root: Path) -> GcCategory:
    """The schema-1 monolith, reclaimable only once both later generations can answer for it.

    Two conditions, not one, because the archive has two readers and they retired schema 1 at
    different times. ``./timeline`` reads schema 3 first and schema 2 second and reaches schema 1
    only when neither is present, so schema 3 alone would satisfy it. The **website** has no
    schema-3 mode at all: `static/app.js` loads `data/timeline-v2.json` and falls back to
    `data/timeline.json` when that load throws. Reclaiming the monolith while schema 2 was
    missing would therefore leave the browser with nothing to fall back *to*, which is precisely
    the failure the fallback exists to absorb. So both must be present, and schema 3's presence
    is judged by :func:`query.schema_3_completeness` -- the reader's own five-clause rule, not a
    looser restatement of it that could say yes where the reader says no.
    """

    files = _files_with_sizes(root, (_SCHEMA_1_TIMELINE, _SCHEMA_1_TIMELINE + ".gz"))
    complete, declined = schema_3_completeness(root)
    if not complete:
        return GcCategory(
            "schema-1-monolith",
            False,
            f"held: no complete schema-3 generation supersedes it ({declined})",
            files,
        )
    if _size_of(root, SCHEMA_2_BOOTSTRAP_PATH) is None:
        return GcCategory(
            "schema-1-monolith",
            False,
            "held: the website reads schema 2 and falls back to schema 1, and "
            f"{SCHEMA_2_BOOTSTRAP_PATH} is absent",
            files,
        )
    return GcCategory(
        "schema-1-monolith",
        True,
        f"superseded by a complete {SCHEMA_3_BOOTSTRAP_PATH} beside "
        f"{SCHEMA_2_BOOTSTRAP_PATH}; no build writes it any more",
        files,
    )


def _schema_2_manifest_sets(root: Path) -> tuple[frozenset[str], frozenset[str]] | None:
    """The v2 manifest's ``(current, retained)`` sets, or ``None`` when it cannot be trusted.

    ``None`` is returned rather than an empty pair for an absent or unrecognised manifest,
    because an empty pair would make every object on disk look like an orphan. That is exactly
    the misreading `timeline_shards._previous_objects` refuses on the same file, where an absent
    manifest means "an older tool wrote these and recorded no reachability", and the safe
    interpretation is that everything is live.
    """

    path = root / _SCHEMA_2_MANIFEST
    if path.is_symlink() or not path.is_file():
        return None
    try:
        manifest = as_object(read_json(path), str(path))
        if (
            manifest.get("schema_version") != 1
            or manifest.get("kind") != "timeline-shard-files"
        ):
            return None
        current = frozenset(
            as_string(value, f"{path}.current_objects[{index}]")
            for index, value in enumerate(
                as_array(manifest.get("current_objects"), f"{path}.current_objects")
            )
        )
        retained = frozenset(
            as_string(value, f"{path}.retained_objects[{index}]")
            for index, value in enumerate(
                as_array(manifest.get("retained_objects"), f"{path}.retained_objects")
            )
        )
    except (OSError, ValueError):
        return None
    return current, retained


def _schema_2_categories(root: Path) -> tuple[GcCategory, GcCategory]:
    """Split the object store into what the manifest disowns and what it deliberately holds.

    The manifest's accounting is exact today -- on the measured archive 353 current plus 305
    retained is 658, which is every one of the 334 ``.json`` and 324 ``.gz`` files on disk, with
    no orphan and nothing missing -- and this function is written so that reading it that way
    keeps the property rather than replacing it. The orphan set is *derived* as
    ``on disk - current - retained``, so if it is ever non-empty that is a real finding about a
    crashed build, not an artefact of `gc` counting differently.
    """

    on_disk = {
        relative: size
        for relative, size in _walk_files(root, _SCHEMA_2_OBJECT_ROOT)
        if _OBJECT_NAME.fullmatch(PurePosixPath(relative).name) is not None
    }
    sets = _schema_2_manifest_sets(root)
    if sets is None:
        return (
            GcCategory(
                "schema-2-orphan-objects",
                False,
                f"held: {_SCHEMA_2_MANIFEST} is absent or unrecognised, so nothing on disk can "
                "be shown to be unreachable",
                (),
            ),
            GcCategory(
                "schema-2-retained-objects",
                False,
                f"held: {_SCHEMA_2_MANIFEST} is absent or unrecognised",
                (),
            ),
        )
    current, retained = sets
    orphans = sorted(set(on_disk) - current - retained)
    return (
        GcCategory(
            "schema-2-orphan-objects",
            True,
            f"named by neither current_objects nor retained_objects in {_SCHEMA_2_MANIFEST}",
            tuple(GcFile(relative, on_disk[relative]) for relative in orphans),
        ),
        GcCategory(
            "schema-2-retained-objects",
            False,
            "held: the previous generation's objects, kept alive one more generation so a "
            "browser that already loaded the previous bootstrap can still fetch what it names; "
            "the next build that supersedes them reclaims them",
            tuple(
                GcFile(relative, on_disk[relative])
                for relative in sorted(retained)
                if relative in on_disk
            ),
        ),
    )


def _schema_3_named_paths(root: Path) -> frozenset[str] | None:
    """Every path the schema-3 bootstrap names, or ``None`` if it cannot be read.

    Deliberately parsed here rather than obtained from `query._SchemaThreeArchive`: the reader
    keeps the catalogue for the questions it answers and exposes it as shard *entries*, while the
    only question here is the flat set of names, and reaching into a private structure to
    reconstruct it would couple a sweep to a reader's field layout.
    """

    path = root / SCHEMA_3_BOOTSTRAP_PATH
    if path.is_symlink() or not path.is_file():
        return None
    try:
        bootstrap = as_object(read_json(path), str(path))
        if bootstrap.get("schema_version") != 3 or (
            bootstrap.get("kind") != "timeline-v3-bootstrap"
        ):
            return None
        named: set[str] = {SCHEMA_3_BOOTSTRAP_PATH}
        streams = as_object(bootstrap.get("streams"), "timeline-v3.streams")
        for stream, raw_section in sorted(streams.items()):
            where = f"timeline-v3.streams.{stream}"
            section = as_object(raw_section, where)
            for index, raw in enumerate(as_array(section.get("shards"), where + ".shards")):
                entry = as_object(raw, f"{where}.shards[{index}]")
                named.add(as_string(entry.get("path"), f"{where}.shards[{index}].path"))
                named.add(
                    as_string(
                        entry.get("index_path"), f"{where}.shards[{index}].index_path"
                    )
                )
    except (OSError, ValueError):
        return None
    return frozenset(named)


def _schema_3_category(root: Path) -> GcCategory:
    """Shards under the schema-3 root that the current bootstrap does not name. Never swept.

    This category was written to be reclaimable, on the reasoning that schema 3 needs no
    reachability manifest because the bootstrap *is* the reachable set, so a file under
    ``data/timeline-v3/`` and absent from the catalogue must have been reachable from a
    generation that no longer exists. **That reasoning is wrong, and the counterexample costs a
    team.** Publication is shards, then the bootstrap, with no atomicity across the set. A build
    that adds a team and dies after writing its shards and before writing the bootstrap leaves
    the *previous* bootstrap in place -- complete, self-consistent, naming files that are all
    present at their declared lengths -- with the new team's shards beside it, named by nobody.
    Absence from the catalogue therefore means "retired" if the bootstrap is newer than the
    shard and "live, and the newest thing here" if it is older, and nothing on disk says which.
    A rule that guessed would eventually sweep the better half of a rebuild, and would in the
    same pass declare the schema-1 monolith superseded and reclaim the archive's fallback.

    Two changes retire the question rather than answer it. The reader now checks the converse of
    its presence rule -- clause 5 of :class:`query._SchemaThreeArchive`, "nothing on disk is
    unnamed" -- so the interrupted generation above is declined rather than read as complete,
    which is what already keeps this category, and the monolith beside it, held. And the
    publisher clears its own root of everything outside its plan once its bootstrap is on disk,
    so a completed build always leaves the tree equal to its catalogue. Between them, a non-empty
    orphan set has exactly one meaning left: a build was interrupted. That is a finding to report
    and a build to re-run, not garbage to collect -- and `gc` is not the tool that should be
    deciding it, because it is the one tool here with no way to tell the two cases apart.

    So the files are still listed, at their full size, with the reason saying what they are.
    Reporting a thing and reclaiming it are different jobs, and this module already separates
    them for the largest thing in the archive.
    """

    named = _schema_3_named_paths(root)
    on_disk = tuple(_walk_files(root, SCHEMA_3_ROOT))
    if named is None:
        return GcCategory(
            "schema-3-orphan-shards",
            False,
            f"held: {SCHEMA_3_BOOTSTRAP_PATH} is absent or unreadable, so no shard can be shown "
            "to be unreachable",
            (),
        )
    orphans = tuple(
        GcFile(relative, size) for relative, size in on_disk if relative not in named
    )
    if not orphans:
        complete, declined = schema_3_completeness(root)
        return GcCategory(
            "schema-3-orphan-shards",
            False,
            f"nothing under {SCHEMA_3_ROOT}/ is unaccounted for; the tree matches "
            f"{SCHEMA_3_BOOTSTRAP_PATH}"
            + ("" if complete else f", though the generation is incomplete ({declined})"),
            (),
        )
    return GcCategory(
        "schema-3-orphan-shards",
        False,
        f"held: under {SCHEMA_3_ROOT}/ and named by no entry in {SCHEMA_3_BOOTSTRAP_PATH}. A "
        "completed build leaves this set empty -- it removes its own strays -- so these are the "
        "residue of a build that was interrupted before it published its bootstrap, and there "
        "is no way from here to tell a retired team's shard from a live team's. Until they are "
        "gone the reader declines schema 3 entirely and this archive is being read through the "
        "slower generation behind it. Re-run the build; it will clear them and this category "
        "goes back to empty",
        orphans,
    )


def _source_snapshot_category(root: Path) -> GcCategory:
    """The ingest inputs still inside the archive, measured and left alone.

    This is the largest single thing in the archive and it is reported for that reason alone: the
    question `gc` exists to answer is "what is in the output that is not output", and an answer
    that omitted 72% of the bytes because they are out of scope would be a technically true
    answer to a different question.

    On an archive whose snapshots have been relocated by `migrate-snapshots` this category is
    empty, and that is the correct report rather than a blind spot: `gc` classifies the files in
    the directory it was given, and after a migration there are none of these there. The store
    outside the archive is deliberately not walked -- reaching outside `--output` to enumerate,
    let alone reclaim, is exactly the authority a garbage collector should not have.
    """

    teams = root / "teams"
    relatives: list[str] = []
    if teams.is_dir() and not teams.is_symlink():
        for team in sorted(teams.iterdir()):
            if not team.is_dir() or team.is_symlink():
                continue
            relatives.append(f"teams/{team.name}/{_SOURCE_SNAPSHOT_DIRECTORY}")
    found: list[GcFile] = []
    for relative in relatives:
        found.extend(GcFile(path, size) for path, size in _walk_files(root, relative))
    return GcCategory(
        "ingest-source-snapshots",
        False,
        "held: vendor input rather than published output, and its deletion has a purpose-built "
        "gate -- run `audit-losslessness --require-lossless` and act on that, not on this. To "
        "get these bytes out of the published tree without deleting anything, run "
        "`migrate-snapshots`",
        tuple(found),
    )


def _trash_category(root: Path) -> GcCategory:
    return GcCategory(
        "trash",
        False,
        f"held: already swept and reversible; `gc --empty-trash` deletes {TRASH_ROOT}/ for good",
        tuple(GcFile(path, size) for path, size in _walk_files(root, TRASH_ROOT)),
    )


def _empty_directories(root: Path, pending: frozenset[str]) -> tuple[str, ...]:
    """Directories under ``data/`` that hold nothing, innermost first.

    *pending* is the set of files this pass is about to reclaim, counted as already gone. That
    is what makes the dry run's number honest: the directory a retired team leaves behind still
    contains its shards at the moment the plan is computed, and a plan that only reported
    already-empty directories would report zero and then remove two.

    The known source is a retired team: stale-file removal unlinks a team's shards and details
    and never `rmdir`s what held them, so `data/timeline-v3/timeline/<team>/` and
    `data/details/<team>/` survive their contents. They cost no bytes, which is why they are
    reported as a count rather than a size and removed by `rmdir` rather than moved to the trash
    -- an empty directory is restored by `mkdir`, so a trash copy of one would be ceremony.

    Innermost first so that removing a leaf makes its parent collectable in the same pass without
    a second walk.
    """

    data = root / "data"
    if not data.is_dir() or data.is_symlink():
        return ()
    collected: set[str] = set()
    empty: list[str] = []
    for directory, subdirectories, names in os.walk(data, topdown=False, followlinks=False):
        path = Path(directory)
        if path == data or path.is_symlink():
            continue
        if any(
            Path(directory, name).relative_to(root).as_posix() not in pending
            for name in names
        ):
            continue
        if any(
            Path(directory, name).as_posix() not in collected for name in subdirectories
        ):
            continue
        collected.add(path.as_posix())
        empty.append(path.relative_to(root).as_posix())
    return tuple(empty)


def plan_collection(root: Path) -> GcReport:
    """Classify the archive without touching it. The caller holds the lock."""

    total_bytes = 0
    total_files = 0
    for _relative, size in _walk_files(root, ""):
        total_bytes += size
        total_files += 1
    orphan_objects, retained_objects = _schema_2_categories(root)
    categories = (
        _schema_1_category(root),
        orphan_objects,
        _schema_3_category(root),
        retained_objects,
        _source_snapshot_category(root),
        _trash_category(root),
    )
    pending = frozenset(
        item.relative_path
        for category in categories
        if category.reclaimable
        for item in category.files
    )
    return GcReport(
        root=str(root.resolve()),
        action="dry-run",
        total_bytes=total_bytes,
        total_files=total_files,
        categories=categories,
        trash_relative_path=TRASH_ROOT,
        trash_generation=None,
        moved=(),
        removed_directories=_empty_directories(root, pending),
        emptied_bytes=0,
    )


def _generation_directory(root: Path, now: datetime) -> str:
    """A fresh trash generation, named for the instant it was opened.

    The suffix loop exists because two sweeps in the same second are ordinary in a test and
    possible in a script, and a generation that silently merged with an earlier one would make
    its receipt describe files it did not move.
    """

    stamp = now.strftime(_GENERATION_STAMP)
    for attempt in range(1, 1000):
        candidate = stamp if attempt == 1 else f"{stamp}-{attempt}"
        relative = f"{TRASH_ROOT}/{candidate}"
        if not (root / relative).exists():
            return relative
    raise ArchiveGcError(f"cannot open a trash generation under {TRASH_ROOT}/")


def _move_to_trash(root: Path, generation: str, relative: str) -> None:
    source = _archive_path(root, relative)
    target = _archive_path(root, f"{generation}/{_TRASH_PAYLOAD}/{relative}")
    target.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.replace(source, target)
    except OSError as error:
        if error.errno == errno.EXDEV:
            raise ArchiveGcError(
                f"cannot move {relative} into {TRASH_ROOT}/ because they are on different "
                "filesystems; the trash has to sit on the archive's own device for the first "
                "pass to be both fast and reversible"
            ) from error
        raise ArchiveGcError(f"cannot reclaim {relative}: {error}") from error


def _receipt(
    root: Path,
    generation: str,
    report: GcReport,
    files: Sequence[tuple[str, GcFile]],
    *,
    complete: bool,
) -> None:
    """Write the generation's receipt, before the sweep as intent and after it as record.

    Written **twice**, and the first write is the one that matters. A receipt produced only after
    every move succeeded is absent in precisely the case it was invented for: `os.replace` raises
    on the fifth of ten files -- EACCES under a read-only subtree, ENOSPC or EDQUOT creating the
    trash subdirectory, EPERM on a sticky directory -- and the operator is left with a traceback
    and a generation directory holding four files and no statement of what they are or how to put
    them back. The payload tree still mirrors the archive, so the restore would have worked; it
    just was not written down anywhere, which was the receipt's entire job.

    So the intent is recorded first, listing everything the sweep is about to move, and rewritten
    afterwards with what it actually moved. ``complete`` distinguishes the two, so a reader can
    tell "these files are in the trash" from "each of these is in the trash *or* still in the
    archive" -- and the restore command is correct either way, because copying the payload tree
    back over the archive is idempotent and a file that never moved simply is not in it.
    """

    entries: list[JsonValue] = [
        {
            "path": item.relative_path,
            "bytes": item.bytes,
            "category": category,
            "restore_to": item.relative_path,
        }
        for category, item in files
    ]
    reasons: list[JsonValue] = [
        {"category": item.name, "reason": item.reason}
        for item in report.categories
        if item.reclaimable
    ]
    receipt: dict[str, JsonValue] = {
        "schema_version": 1,
        "kind": "archive-gc-trash-receipt",
        "generation": generation,
        "archive": report.root,
        "complete": complete,
        "reasons": reasons,
        "files": entries,
        "restore": (
            f"from the archive root, run: cp -a {generation}/{_TRASH_PAYLOAD}/. ./ -- the "
            "payload tree mirrors the archive, so every file lands back where it was taken from"
        ),
    }
    if not complete:
        receipt["note"] = (
            "written before the sweep started. Every file listed is either in this generation's "
            "payload tree or still at its original path in the archive; the restore command "
            "above is correct in both cases. If the sweep finished, this file was replaced by "
            "one with complete=true."
        )
    write_text_if_changed(
        _archive_path(root, f"{generation}/{_RECEIPT_NAME}"), canonical_json(receipt)
    )


def _remove_tree(path: Path) -> int:
    """Delete a directory tree that this module created, returning the bytes freed."""

    freed = 0
    for directory, subdirectories, names in os.walk(path, topdown=False, followlinks=False):
        for name in names:
            entry = Path(directory, name)
            try:
                freed += entry.lstat().st_size
            except OSError:
                pass
            entry.unlink()
        for name in subdirectories:
            candidate = Path(directory, name)
            if candidate.is_symlink():
                candidate.unlink()
            else:
                candidate.rmdir()
    path.rmdir()
    return freed


def collect(
    root: Path, *, delete: bool = False, empty_trash: bool = False
) -> GcReport:
    """Report what is dead, and -- only when asked -- move it or destroy the trash.

    ``delete`` and ``empty_trash`` are refused together rather than sequenced. Sweeping and then
    emptying in one command would reintroduce exactly the irreversibility the trash exists to
    remove, and an operator who wants both has lost nothing by typing two commands: the second
    one is the one they should have had to think about.
    """

    if delete and empty_trash:
        raise ArchiveGcError(
            "--delete and --empty-trash are separate passes on purpose: the first is "
            "reversible and the second is not, so no single invocation does both"
        )
    if not (root / ARCHIVE_MARKER_FILE).is_file():
        raise ArchiveGcError(f"not an agent-team-timeline archive: {root}")
    with archive_writer_lock(root):
        report = plan_collection(root)
        if empty_trash:
            trash = root / TRASH_ROOT
            freed = 0
            if trash.is_dir() and not trash.is_symlink():
                freed = _remove_tree(trash)
            return GcReport(
                root=report.root,
                action="emptied",
                total_bytes=report.total_bytes - freed,
                total_files=report.total_files - report.category("trash").count,
                categories=tuple(
                    item if item.name != "trash" else GcCategory(
                        item.name, False, "emptied", ()
                    )
                    for item in report.categories
                ),
                trash_relative_path=TRASH_ROOT,
                trash_generation=None,
                moved=(),
                removed_directories=(),
                emptied_bytes=freed,
            )
        if not delete:
            return report
        moved: list[tuple[str, GcFile]] = []
        planned: list[tuple[str, GcFile]] = [
            (category.name, item)
            for category in report.categories
            if category.reclaimable
            for item in category.files
        ]
        generation = _generation_directory(root, datetime.now(tz=timezone.utc))
        if planned:
            _receipt(root, generation, report, planned, complete=False)
        for category_name, item in planned:
            _move_to_trash(root, generation, item.relative_path)
            moved.append((category_name, item))
        removed: list[str] = []
        for relative in report.removed_directories:
            candidate = _archive_path(root, relative)
            try:
                candidate.rmdir()
            except OSError:
                continue
            removed.append(relative)
        swept = GcReport(
            root=report.root,
            action="swept",
            total_bytes=report.total_bytes,
            total_files=report.total_files,
            categories=tuple(
                GcCategory(item.name, item.reclaimable, item.reason, ())
                if item.reclaimable
                else item
                for item in report.categories
            ),
            trash_relative_path=TRASH_ROOT,
            trash_generation=generation,
            moved=tuple(item for _category, item in moved),
            removed_directories=tuple(removed),
            emptied_bytes=0,
        )
        if moved:
            _receipt(root, generation, report, moved, complete=True)
        return swept


def _mib(value: int) -> str:
    return f"{value / (1024 * 1024):,.1f} MiB"


def format_gc_report(report: GcReport, output_format: str) -> str:
    """Render a report for a human or for a machine.

    The text form leads with the archive total and the reclaimable fraction, because "how much
    of this 9.1 GB is dead" is the question, and a table of categories that never adds up to
    anything is how a report about size fails to be about size.
    """

    if output_format == "json":
        return canonical_json(report.to_json_obj(include_files=True)) + "\n"
    if output_format != "text":
        raise ArchiveGcError(f"unsupported gc report format {output_format!r}")
    lines = [
        f"archive: {report.root}",
        f"  {report.total_files:,} files, {_mib(report.total_bytes)}",
        "",
    ]
    for category in report.categories:
        state = "reclaimable" if category.reclaimable else "held"
        lines.append(
            f"{category.name}: {category.count:,} files, {_mib(category.bytes)} [{state}]"
        )
        lines.append(f"  {category.reason}")
    lines.append("")
    lines.append(
        f"reclaimable now: {report.reclaimable_files:,} files, "
        f"{_mib(report.reclaimable_bytes)}"
    )
    if report.action == "dry-run":
        if report.removed_directories:
            lines.append(
                f"empty directories to remove: {len(report.removed_directories):,}"
            )
        lines.append("")
        lines.append(
            "this was a dry run; nothing was moved or deleted. To reclaim, add --delete: "
            f"every file above is moved into {report.trash_relative_path}/<generation>/ with "
            "its archive path preserved, and a receipt beside it says how to put it back. "
            "Emptying that trash for good is a separate command, `gc --empty-trash`."
        )
    elif report.action == "swept":
        lines.append(
            f"moved {len(report.moved):,} file(s) into {report.trash_generation}/"
            f"{_TRASH_PAYLOAD}/"
        )
        if report.removed_directories:
            lines.append(f"removed {len(report.removed_directories):,} empty director(ies)")
        lines.append(
            f"to undo, from the archive root: "
            f"cp -a {report.trash_generation}/{_TRASH_PAYLOAD}/. ./"
        )
        lines.append("to make it permanent: gc --empty-trash")
    else:
        lines.append(f"emptied the trash: {_mib(report.emptied_bytes)} freed permanently")
    return "\n".join(lines) + "\n"


__all__ = [
    "ArchiveGcError",
    "GcCategory",
    "GcFile",
    "GcReport",
    "TRASH_ROOT",
    "collect",
    "format_gc_report",
    "plan_collection",
]
