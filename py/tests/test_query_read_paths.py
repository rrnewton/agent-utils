"""Contract tests for the slicing read paths in :mod:`wrkviz.query`.

The read path was rewritten to stop materializing the transcript projections, and a rewrite
whose entire point is "it costs less" needs its cost asserted rather than described. Three
instruments do that here, and none of them is a clock:

*``TranscriptQuery.bytes_read``.* A byte count is a proof; a timing assertion is a flake on a
loaded build host. Every laziness claim below is a bound on this counter, including the one
that matters most -- that constructing the object reads nothing at all.

*A full-scan reference.* Every seek, bisect and backwards read is checked against the obvious
implementation over the same records. The seek is only interesting if it returns the same
answer, so the answer is computed both ways and compared, over a sweep of window bounds that
deliberately lands on, one before, and one after instants shared by several records.

*A differential against the package reader.* ``_ChunkedJsonlReader`` is a standalone-library
copy of :class:`wrkviz.seekable_jsonl.SeekableJsonlReader`, kept separate because
this file is copied verbatim into every generated archive as the ``./timeline`` executable and
may import nothing but the standard library. The duplication is pinned here rather than by a
comment, because a comment does not fail.
"""

from __future__ import annotations

import base64
import hashlib
import json
import random
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

import pytest

from wrkviz.archive import JsonValue
from wrkviz.query import (
    ArchiveReadError,
    OrdinalRange,
    QueryFilters,
    TranscriptQuery,
    _ChunkedJsonlReader,
    _TRANSCRIPT_CONTAINER,
    _TRANSCRIPT_CONTAINER_SUFFIX,
)
from wrkviz.seekable_jsonl import (
    SeekableJsonlReader,
    write_seekable_jsonl,
)
from wrkviz.transcript_export import (
    _CONTAINER_SUFFIX as _WRITER_CONTAINER_SUFFIX,
    _TRANSCRIPT_CONTAINER as _WRITER_CONTAINER,
)


@dataclass(frozen=True)
class _Window:
    """The half-open window the query filters accept, spelled out for the fixtures."""

    start_ms: int | None
    end_ms: int | None

    def contains(self, timestamp_ms: int) -> bool:
        if self.start_ms is not None and timestamp_ms < self.start_ms:
            return False
        return self.end_ms is None or timestamp_ms < self.end_ms

    def overlaps(self, start_ms: int, end_ms: int | None) -> bool:
        upper = start_ms if end_ms is None else end_ms
        if self.start_ms is not None and upper < self.start_ms:
            return False
        return self.end_ms is None or start_ms < self.end_ms


# ---------------------------------------------------------------------------------------
# A synthetic transcript export
# ---------------------------------------------------------------------------------------
#
# Built here rather than borrowed from `transcript_export` on purpose. What is under test is
# the reader's behaviour at boundaries -- duplicate instants, a window bound landing exactly
# on one, a selection that ends early in the file -- and those are conditions a real export
# happens to contain or happens not to. A fixture that states them cannot stop containing
# them.


def _prompt(
    ordinal: int, timestamp_ms: int, team: str, author_kind: str, text: str
) -> dict[str, object]:
    return {
        "record_id": f"logical-prompt:{ordinal:04d}",
        "record_type": "prompt",
        "ordinal": ordinal,
        "timestamp_ms": timestamp_ms,
        "team_slug": team,
        "author_kind": author_kind,
        "text": text,
    }


def _response(
    prompt_id: str | None, timestamp_ms: int, team: str, index: int, text: str
) -> dict[str, object]:
    return {
        "record_id": f"response:{index:04d}",
        "record_type": "response",
        "in_reply_to_prompt_id": prompt_id,
        "timestamp_ms": timestamp_ms,
        "team_slug": team,
        "text": text,
    }


def _sort_key(record: dict[str, object]) -> tuple[int, str]:
    timestamp = record["timestamp_ms"]
    assert isinstance(timestamp, int)
    record_id = record["record_id"]
    assert isinstance(record_id, str)
    return (timestamp, record_id)


#: Deliberately far below the exporter's 1 MiB. The fixtures here are under a megabyte, and a
#: target that put all of one in a single member would make every "reads less than the
#: projection" assertion below vacuously true while the same query against the real corpus --
#: 129.7 MiB across 127 members -- touched under one percent of it. A small target reproduces the
#: *shape* the assertions are about at a size a test can afford.
_FIXTURE_CHUNK_BYTES = 1 << 14

#: A vocabulary for fixture bodies, and the reason there is one. ``"x" * 400`` is what these
#: fixtures used to carry, which was free while the projections were plain text and is actively
#: misleading now: it compresses about 200:1, so a 767 KB projection becomes a 4 KB container
#: whose sidecar is a third of it, and a bound stated as a fraction of the file measures the
#: index rather than the read. Word soup compresses about 4:1, which is what the real corpus
#: does, so the fractions these tests assert are the fractions a reader actually sees.
_FIXTURE_WORDS = tuple(f"w{number:04d}" for number in range(600))


def _body(rng: random.Random, words: int) -> str:
    return " ".join(rng.choice(_FIXTURE_WORDS) for _ in range(words))


def _write_export(
    root: Path,
    prompts: Sequence[dict[str, object]],
    messages: Sequence[dict[str, object]],
    *,
    declare_compressed_bytes: bool = True,
    keep_index: bool = True,
) -> Path:
    """Lay out an ``extracted/transcripts`` tree the reader will accept."""

    transcripts = root / "extracted" / "transcripts"
    transcripts.mkdir(parents=True, exist_ok=True)
    payload = {
        "prompts.jsonl": prompts,
        "messages.jsonl": messages,
        # Declared, checked for presence and size, and opened by no query. It is here
        # because the constructor's contract covers every projection the archive carries,
        # which is what `test_a_truncated_unconsulted_projection_is_still_refused_at_open`
        # pins. The monotonic baseline is not among them: it is rerun state and lives in the
        # build store, so an archive that does not carry it is correct rather than damaged.
        "system-inputs.jsonl": [{"record_id": "system-input:1", "timestamp_ms": 1}],
    }
    files: dict[str, object] = {}
    for name, records in payload.items():
        report = write_seekable_jsonl(
            transcripts / (name + ".gz"),
            [dict(record) for record in records],  # type: ignore[arg-type]
            timestamp_key="timestamp_ms",
            target_chunk_bytes=_FIXTURE_CHUNK_BYTES,
        )
        entry: dict[str, object] = {
            "sha256": report.index.u_sha256,
            "bytes": report.index.u_size,
            "records": report.index.record_count,
            "container": "multi-member-gzip",
            "path": name + ".gz",
            "index_path": name + ".gz.index.jsonl",
            "c_sha256": report.index.c_sha256,
        }
        if declare_compressed_bytes:
            entry["c_bytes"] = report.index.c_size
        if not keep_index:
            report.index_path.unlink()
        files[name] = entry
    (transcripts / "manifest.json").write_text(
        json.dumps({"schema_version": 1, "files": files}, indent=1), encoding="utf-8"
    )
    return root


def _container(root: Path, name: str) -> Path:
    return root / "extracted" / "transcripts" / (name + ".gz")


def _corpus_bytes(root: Path, name: str) -> int:
    """Everything one projection occupies on disk: the container and its sidecar.

    The "at most one pass" ceilings below are stated against this rather than against the
    container alone, because the sidecar is now part of the projection and a reader that opens
    the projection reads it. On the measured corpus it is 0.08% of the pair; on a fixture built
    to have dozens of tiny members it is a few percent, which is why it is counted rather than
    waved at.
    """

    container = _container(root, name)
    sidecar = container.with_name(container.name + ".index.jsonl")
    return container.stat().st_size + sidecar.stat().st_size


#: Instants deliberately repeated 2, 3 and 4 times, so a window bound landing on a shared
#: instant is exercised rather than hoped for.
_INSTANTS = (100, 100, 100, 200, 200, 300, 300, 300, 300, 400, 500, 500)
_TEAMS = ("team-a", "team-b")
_AUTHORS = ("owner_human", "agent", "someone_else")


@pytest.fixture
def export(tmp_path: Path) -> Path:
    prompts = [
        _prompt(
            ordinal,
            instant,
            _TEAMS[ordinal % len(_TEAMS)],
            _AUTHORS[ordinal % len(_AUTHORS)],
            f"prompt body {ordinal}",
        )
        for ordinal, instant in enumerate(_INSTANTS, start=1)
    ]
    messages: list[dict[str, object]] = list(prompts)
    for index, prompt in enumerate(prompts, start=1):
        prompt_id = prompt["record_id"]
        assert isinstance(prompt_id, str)
        timestamp = prompt["timestamp_ms"]
        assert isinstance(timestamp, int)
        team = prompt["team_slug"]
        assert isinstance(team, str)
        messages.append(
            _response(prompt_id, timestamp + 1, team, index, f"reply to {index}")
        )
    # One response linked to nothing, to prove `_message_ordinal` drops it rather than
    # tripping over a null link.
    messages.append(_response(None, 601, "team-a", 999, "orphaned reply"))
    return _write_export(tmp_path, prompts, sorted(messages, key=_sort_key))


def _reference_prompts(
    export_root: Path, filters: QueryFilters, which: str
) -> list[dict[str, JsonValue]]:
    """The obvious implementation: read everything, filter, return it.

    Through the *package* reader rather than the one under test, so that "the seek agrees with a
    full scan" does not reduce to "the seek agrees with itself".
    """

    human = {"owner_human", "other_human"}
    bot = {"agent", "system"}
    kept: list[dict[str, JsonValue]] = []
    with SeekableJsonlReader(_container(export_root, "prompts.jsonl")) as reader:
        every = list(reader.iter_records())
    for record in every:
        author = record["author_kind"]
        klass = "human" if author in human else "bot" if author in bot else "unclassified"
        if which != "all" and klass != which:
            continue
        if filters.teams and record["team_slug"] not in filters.teams:
            continue
        if filters.window is not None and not filters.window.contains(
            int(str(record["timestamp_ms"]))
        ):
            continue
        kept.append(record)
    return kept


# ---------------------------------------------------------------------------------------
# Laziness
# ---------------------------------------------------------------------------------------


def test_constructing_a_transcript_query_reads_no_projection_bytes(
    export: Path,
) -> None:
    """The constructor's digest loop is gone, and nothing quietly replaced it.

    This is the headline of the rewrite -- it used to hash 220,026,263 bytes before answering
    anything -- so it is asserted as an exact zero rather than a bound.
    """

    assert TranscriptQuery(export).bytes_read == 0


def test_a_small_page_reads_far_less_than_the_projection(tmp_path: Path) -> None:
    """``--limit 1`` costs the sidecar and one member, not the file.

    The projection has to hold many members for the claim to mean anything, so this one builds
    its own rather than using the boundary fixture, which is deliberately small enough to fit in
    a single member.
    """

    rng = random.Random(20260826)
    prompts = [
        _prompt(ordinal, ordinal * 10, "team-a", "owner_human", _body(rng, 60))
        for ordinal in range(1, 1_501)
    ]
    root = _write_export(tmp_path, prompts, prompts)
    with SeekableJsonlReader(_container(root, "prompts.jsonl")) as reader:
        assert len(reader.index.members) > 20
    query = TranscriptQuery(root)
    records = query.list_prompts(QueryFilters(), None, "all", limit=1)
    assert len(records) == 1
    assert 0 < query.bytes_read < _corpus_bytes(root, "prompts.jsonl") // 10


def test_a_tail_reads_backwards_rather_than_forwards(export: Path) -> None:
    query = TranscriptQuery(export)
    records = query.list_prompts(QueryFilters(), None, "all", tail=2)
    assert [record["ordinal"] for record in records] == [11, 12]
    assert query.bytes_read <= _corpus_bytes(export, "prompts.jsonl")


def test_a_filtered_tail_reads_the_projection_at_most_once(export: Path) -> None:
    """The widening rounds must extend the buffer, not restart the read.

    A ``--tail`` whose filter rejects most of what it sees used to restart the tail from the end
    of the file on every round, so round *k* re-read everything rounds 1..k-1 read -- measured at
    1.85 times the message projection to return twenty records. The ceiling for a backwards read
    that keeps its buffer is 1.0, and the fixture is built to force several rounds: the accepted
    team's records stop well before the end of the file, and the container holds enough members
    that reaching them takes many widenings.
    """

    rng = random.Random(20260826)
    prompts = [
        _prompt(
            ordinal,
            ordinal * 10,
            "team-a" if ordinal <= 5 else "team-b",
            "owner_human",
            _body(rng, 30),
        )
        for ordinal in range(1, 1_201)
    ]
    root = export / "early-selection"
    _write_export(root, prompts, prompts)
    with SeekableJsonlReader(_container(root, "prompts.jsonl")) as reader:
        assert len(reader.index.members) > 10
    query = TranscriptQuery(root)
    records = query.list_prompts(
        QueryFilters(teams=("team-a",)), None, "all", tail=3
    )
    assert [record["ordinal"] for record in records] == [3, 4, 5]
    size = _corpus_bytes(root, "prompts.jsonl")
    assert query.bytes_read <= size, (
        f"read {query.bytes_read} of a {size}-byte projection; the backwards read is "
        "re-reading what it already has"
    )


# ---------------------------------------------------------------------------------------
# Correctness of the seek, against a full scan
# ---------------------------------------------------------------------------------------


@pytest.mark.parametrize("which", ["human", "bot", "all"])
@pytest.mark.parametrize("teams", [(), ("team-a",), ("team-a", "team-b")])
def test_a_window_sweep_agrees_with_a_full_scan(
    export: Path, which: str, teams: tuple[str, ...]
) -> None:
    """Every window bound the fixture can produce, crossed with every selection.

    The bounds range over one before, exactly on, and one after each distinct instant, which
    is where a half-open contract goes wrong if it goes wrong at all -- and where the ties
    matter, because four records share instant 300.
    """

    probes = [None] + [
        instant + delta for instant in sorted(set(_INSTANTS)) for delta in (-1, 0, 1)
    ]
    query = TranscriptQuery(export)
    for start in probes:
        for end in probes:
            if start is not None and end is not None and end < start:
                continue
            filters = QueryFilters(teams=teams, window=_Window(start, end))
            expected = _reference_prompts(export, filters, which)
            actual = query.list_prompts(filters, None, which)
            assert [record["ordinal"] for record in actual] == [
                record["ordinal"] for record in expected
            ], f"start={start} end={end} which={which} teams={teams}"


def test_limit_and_tail_take_opposite_ends_of_the_same_selection(
    export: Path,
) -> None:
    query = TranscriptQuery(export)
    filters = QueryFilters()
    everything = query.list_prompts(filters, None, "all")
    head = query.list_prompts(filters, None, "all", limit=3)
    tail = query.list_prompts(filters, None, "all", tail=3)
    assert head == everything[:3]
    assert tail == everything[-3:]


def test_limit_and_tail_together_are_refused(export: Path) -> None:
    query = TranscriptQuery(export)
    with pytest.raises(ValueError, match="opposite ends"):
        query.list_prompts(QueryFilters(), None, "all", limit=1, tail=1)
    for bad in (0, -1):
        with pytest.raises(ValueError, match="at least 1"):
            query.list_prompts(QueryFilters(), None, "all", limit=bad)
        with pytest.raises(ValueError, match="at least 1"):
            query.list_prompts(QueryFilters(), None, "all", tail=bad)


def test_an_ordinal_range_returns_exactly_that_range(export: Path) -> None:
    query = TranscriptQuery(export)
    records = query.list_prompts(QueryFilters(), OrdinalRange(4, 6), "all")
    assert [record["ordinal"] for record in records] == [4, 5, 6]


def test_messages_return_the_selected_prompts_and_their_replies(
    export: Path,
) -> None:
    query = TranscriptQuery(export)
    records = query.list_messages(QueryFilters(), OrdinalRange(2, 3), "all")
    kinds = [record["record_type"] for record in records]
    assert kinds.count("prompt") == 2
    assert kinds.count("response") == 2
    assert all(record["record_id"] != "response:0999" for record in records)


def test_a_seeked_message_read_agrees_with_the_exhaustive_one(export: Path) -> None:
    """``verify=True`` abandons the linkage lower bound; both paths must agree.

    The lower bound rests on a writer invariant rather than on a measurement, so the two
    readings are only equivalent while that invariant holds. Comparing them is the cheapest
    way to notice that it has stopped.
    """

    for ordinals in (OrdinalRange(1, 3), OrdinalRange(5, 8), OrdinalRange(10, 12)):
        seeked = TranscriptQuery(export).list_messages(QueryFilters(), ordinals, "all")
        exhaustive = TranscriptQuery(export, verify=True).list_messages(
            QueryFilters(), ordinals, "all"
        )
        assert seeked == exhaustive


# ---------------------------------------------------------------------------------------
# The integrity contract
# ---------------------------------------------------------------------------------------


def test_a_truncated_unconsulted_projection_is_still_refused_at_open(
    tmp_path: Path,
) -> None:
    """A damaged file no query reads is still a damaged generation.

    The size check used to live in the reader, which meant it ran only for the two
    projections a query opens -- so a truncated ``system-inputs.jsonl`` was accepted in
    silence by an open that had already been told how long the file should be.
    """

    root = _write_export(tmp_path, [_prompt(1, 100, "team-a", "owner_human", "x")], [])
    for name in ("system-inputs.jsonl", "prompts.jsonl"):
        path = _container(root, name)
        original = path.read_bytes()
        path.write_bytes(original[: len(original) // 2])
        with pytest.raises(ValueError, match="generation is incomplete"):
            TranscriptQuery(root)
        path.write_bytes(original)
    TranscriptQuery(root)


def test_the_manifest_digest_is_now_enforced_at_open(tmp_path: Path) -> None:
    """What the container bought: the content digest is an O(1) check, not a full read.

    While the projections were plain text, ``sha256`` could only be confirmed by hashing the
    whole file, so an archive whose corpus had been replaced under an unchanged manifest opened
    clean and answered from the new bytes. The sidecar publishes ``u_sha256`` over the same
    stream the manifest describes, so the disagreement is now three numbers apart at open.
    """

    root = _write_export(
        tmp_path,
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
    )
    _write_export(
        root,
        [_prompt(1, 100, "team-a", "owner_human", "xyz")],
        [_prompt(1, 100, "team-a", "owner_human", "xyz")],
    )
    manifest = root / "extracted" / "transcripts" / "manifest.json"
    stale = json.loads(manifest.read_text(encoding="utf-8"))
    stale["files"]["prompts.jsonl"]["sha256"] = hashlib.sha256(b"not this").hexdigest()
    manifest.write_text(json.dumps(stale, indent=1), encoding="utf-8")
    with pytest.raises(ValueError, match="generation is incomplete"):
        TranscriptQuery(root).list_prompts(QueryFilters(), OrdinalRange(1, 1), "all")


def test_a_container_that_disagrees_with_its_sidecar_needs_verify(tmp_path: Path) -> None:
    """The honest limit of the O(1) checks, asserted so the docstring cannot overstate it.

    Everything cheap agrees here: the container is the declared compressed length, the sidecar
    describes that container, and the manifest's ``sha256`` equals the sidecar's ``u_sha256``.
    What no cheap check can see is that the container does not actually inflate to a stream with
    that digest -- the sidecar has been told the wrong answer. Only inflating the whole thing
    finds it, which is exactly what ``--verify`` is for.
    """

    root = _write_export(
        tmp_path,
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
    )
    elsewhere = _write_export(
        tmp_path / "other",
        [_prompt(1, 100, "team-a", "owner_human", "xyz")],
        [_prompt(1, 100, "team-a", "owner_human", "xyz")],
    )
    other = json.loads(
        (elsewhere / "extracted" / "transcripts" / "manifest.json").read_text("utf-8")
    )
    forged = other["files"]["prompts.jsonl"]["sha256"]

    sidecar = _container(root, "prompts.jsonl").with_name(
        "prompts.jsonl.gz" + ".index.jsonl"
    )
    lines = sidecar.read_text(encoding="utf-8").split("\n")
    header = json.loads(lines[0])
    header["u_sha256"] = forged
    lines[0] = json.dumps(header, ensure_ascii=False, separators=(",", ":"))
    sidecar.write_text("\n".join(lines), encoding="utf-8")
    manifest = root / "extracted" / "transcripts" / "manifest.json"
    described = json.loads(manifest.read_text(encoding="utf-8"))
    described["files"]["prompts.jsonl"]["sha256"] = forged
    manifest.write_text(json.dumps(described, indent=1), encoding="utf-8")

    # Every O(1) check passes, and the slice read returns what the container really holds.
    assert TranscriptQuery(root).list_prompts(
        QueryFilters(), OrdinalRange(1, 1), "all"
    )[0]["text"] == "abc"
    with pytest.raises(ValueError, match="generation is incomplete"):
        TranscriptQuery(root, verify=True)


def test_verify_reproduces_the_compressed_digest_too(tmp_path: Path) -> None:
    """A wholesale swap of container and sidecar is caught by ``c_sha256`` and nothing else.

    Replace both with a consistent pair for different content and update the manifest's stream
    digest to match, and every check that reads the sidecar agrees, and inflating agrees. The
    only claim left disagreeing is the one about the bytes on disk.
    """

    root = _write_export(
        tmp_path,
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
    )
    stale = json.loads(
        (root / "extracted" / "transcripts" / "manifest.json").read_text("utf-8")
    )
    _write_export(
        root,
        [_prompt(1, 100, "team-a", "owner_human", "xyz")],
        [_prompt(1, 100, "team-a", "owner_human", "xyz")],
    )
    manifest = root / "extracted" / "transcripts" / "manifest.json"
    swapped = json.loads(manifest.read_text(encoding="utf-8"))
    swapped["files"]["prompts.jsonl"]["c_sha256"] = stale["files"]["prompts.jsonl"][
        "c_sha256"
    ]
    manifest.write_text(json.dumps(swapped, indent=1), encoding="utf-8")
    assert TranscriptQuery(root).list_prompts(
        QueryFilters(), OrdinalRange(1, 1), "all"
    )[0]["text"] == "xyz"
    with pytest.raises(ValueError, match="compressed digest"):
        TranscriptQuery(root, verify=True)


def test_a_projection_without_a_sidecar_still_gets_the_digest_check(
    tmp_path: Path,
) -> None:
    """Losing the index must cost speed, not the guarantee.

    Without a sidecar there is nothing to compare the manifest's digest against in O(1), and
    every read is a full inflate regardless -- so the digest is folded into the first one rather
    than left unchecked. Degrading to "no check" is the one direction this must not go.
    """

    root = _write_export(
        tmp_path,
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
        declare_compressed_bytes=False,
        keep_index=False,
    )
    manifest = root / "extracted" / "transcripts" / "manifest.json"
    stale = json.loads(manifest.read_text(encoding="utf-8"))
    stale["files"]["prompts.jsonl"]["sha256"] = hashlib.sha256(b"not this").hexdigest()
    manifest.write_text(json.dumps(stale, indent=1), encoding="utf-8")
    with pytest.raises(ValueError, match="generation is incomplete"):
        TranscriptQuery(root).list_prompts(QueryFilters(), OrdinalRange(1, 1), "all")


def test_the_container_name_the_reader_expects_is_the_one_the_writer_writes() -> None:
    """The reader may not import the exporter, so the shared vocabulary is pinned here.

    ``query.py`` is copied verbatim into every archive and can import nothing but the standard
    library, so ``multi-member-gzip`` is spelled once in the writer and once in the reader. That
    is the same deliberate duplication as the sidecar's format string, and it gets the same
    treatment: a test, because a comment saying "keep these in sync" does not fail.
    """

    assert _TRANSCRIPT_CONTAINER == _WRITER_CONTAINER
    assert _TRANSCRIPT_CONTAINER_SUFFIX == _WRITER_CONTAINER_SUFFIX


def test_an_uncompressed_corpus_is_refused_by_name(tmp_path: Path) -> None:
    """A pre-container archive gets the remedy, not a missing-file error.

    The reader carries one slicing implementation, so a manifest describing plain text is a
    generation it declines rather than half-reads. What it owes that operator is the sentence
    that fixes it.
    """

    root = _write_export(
        tmp_path,
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
        [_prompt(1, 100, "team-a", "owner_human", "abc")],
    )
    manifest = root / "extracted" / "transcripts" / "manifest.json"
    described = json.loads(manifest.read_text(encoding="utf-8"))
    for entry in described["files"].values():
        entry.pop("container")
    manifest.write_text(json.dumps(described, indent=1), encoding="utf-8")
    with pytest.raises(ValueError, match="re-run extract-transcripts"):
        TranscriptQuery(root)


def test_an_unsorted_projection_is_refused_rather_than_under_returned(
    tmp_path: Path,
) -> None:
    """A seek over an unsorted file silently omits records; this makes it fail loudly."""

    prompts = [
        _prompt(1, 100, "team-a", "owner_human", "a"),
        _prompt(2, 500, "team-a", "owner_human", "b"),
        _prompt(3, 200, "team-a", "owner_human", "c"),
    ]
    root = _write_export(tmp_path, prompts, prompts)
    query = TranscriptQuery(root)
    with pytest.raises(ValueError, match="not ordered by timestamp_ms"):
        query.list_prompts(QueryFilters(window=_Window(50, None)), None, "all")


def test_an_ordinal_that_is_not_its_line_number_is_refused(tmp_path: Path) -> None:
    """The invariant an ordinal range is now *located* by, asserted where it is used.

    While the projection was plain text the ordinal was bisected on, so a column that stopped
    being dense produced a slow answer or a wrong window. Now it is arithmetic -- ordinal *k* is
    line *k-1* -- so a projection that ever stopped honouring it would be read from the wrong
    place and return the wrong records silently. It is checked at every record instead.
    """

    prompts = [
        _prompt(1, 100, "team-a", "owner_human", "a"),
        _prompt(3, 200, "team-a", "owner_human", "b"),
        _prompt(4, 300, "team-a", "owner_human", "c"),
    ]
    root = _write_export(tmp_path, prompts, prompts)
    query = TranscriptQuery(root)
    with pytest.raises(ValueError, match="prompt ordinal is 3, not 2"):
        query.list_prompts(QueryFilters(), OrdinalRange(1, 4), "all")


def test_a_corpus_without_its_sidecar_answers_the_same_questions(tmp_path: Path) -> None:
    """Losing the index costs speed and nothing else -- on the transcript paths too.

    The container's fallbacks are exercised through the query layer rather than through the
    reader, because it is the query layer that has three separate index-dependent paths: the
    timestamp seek, the line-addressed ordinal read, and the backwards tail. Each has a
    correct-but-linear sibling, and the only way to know they agree is to ask both.
    """

    rng = random.Random(4)
    prompts = [
        _prompt(ordinal, ordinal * 10, _TEAMS[ordinal % 2], "owner_human", _body(rng, 8))
        for ordinal in range(1, 301)
    ]
    indexed = _write_export(tmp_path / "indexed", prompts, prompts)
    bare = _write_export(tmp_path / "bare", prompts, prompts, keep_index=False)
    assert not _ChunkedJsonlReader(_container(bare, "prompts.jsonl")).has_index

    questions: tuple[tuple[QueryFilters, OrdinalRange | None, int | None], ...] = (
        (QueryFilters(), OrdinalRange(40, 44), None),
        (QueryFilters(window=_Window(1_000, 1_400)), None, None),
        (QueryFilters(teams=("team-a",)), None, 3),
        (QueryFilters(teams=("team-b",), window=_Window(None, 2_000)), None, 4),
    )
    for filters, ordinals, tail in questions:
        assert TranscriptQuery(bare).list_prompts(
            filters, ordinals, "all", tail=tail
        ) == TranscriptQuery(indexed).list_prompts(filters, ordinals, "all", tail=tail)
        assert TranscriptQuery(bare).list_messages(
            filters, ordinals, "all", tail=tail
        ) == TranscriptQuery(indexed).list_messages(filters, ordinals, "all", tail=tail)


# ---------------------------------------------------------------------------------------
# The bundled schema-3 reader, against the package one
# ---------------------------------------------------------------------------------------


def _shard(path: Path, records: Sequence[dict[str, JsonValue]], target: int) -> None:
    write_seekable_jsonl(
        path, list(records), timestamp_key="at_ms", target_chunk_bytes=target
    )


def _plain_records(count: int) -> list[dict[str, JsonValue]]:
    return [
        {"at_ms": 1_000 + index * 10, "index": index, "text": f"record {index} " * 12}
        for index in range(count)
    ]


def test_query_chunk_reader_matches_the_package_reader(tmp_path: Path) -> None:
    """The differential the section comment promises: same records, same bytes.

    ``_ChunkedJsonlReader`` is a standalone-library copy of ``SeekableJsonlReader`` and the
    copy is pinned here rather than by a comment. Bytes read are compared as well as records,
    because a copy that answers correctly by reading the whole shard has lost the only
    property the duplication was paid for.
    """

    path = tmp_path / "shard.jsonl.gz"
    records = _plain_records(1_200)
    _shard(path, records, 1 << 14)
    mine = _ChunkedJsonlReader(path)
    assert mine.has_index
    assert mine.record_count == len(records)

    with SeekableJsonlReader(path) as theirs:
        assert list(mine.iter_records()) == list(theirs.iter_records())

    random.seed(20260824)
    for _ in range(40):
        low = random.randrange(900, 14_000)
        high = low + random.randrange(1, 3_000)
        mine = _ChunkedJsonlReader(path)
        with SeekableJsonlReader(path) as theirs:
            assert list(mine.read_range(low, high)) == list(theirs.read_range(low, high))
            assert mine.data_bytes_read == theirs.data_bytes_read
    for count in (1, 3, 20, 500):
        mine = _ChunkedJsonlReader(path)
        with SeekableJsonlReader(path) as theirs:
            assert mine.tail(count) == theirs.tail(count)
            assert mine.data_bytes_read == theirs.data_bytes_read
    for first, last in ((0, 1), (5, 5), (17, 400), (0, len(records))):
        mine = _ChunkedJsonlReader(path)
        with SeekableJsonlReader(path) as theirs:
            assert list(mine.read_lines(first, last)) == list(
                theirs.read_lines(first, last)
            )
            assert mine.data_bytes_read == theirs.data_bytes_read


def test_seek_first_line_is_the_bisect_a_full_scan_would_have_found(
    tmp_path: Path,
) -> None:
    """The member-table bisect, swept against the obvious linear answer.

    Every probe lands one before, exactly on, and one after a real instant, and the records are
    deliberately duplicated at each instant so that "the *first* line at or above the target" is
    a claim with teeth rather than a coincidence of uniqueness. The unindexed reader answers the
    same question by scanning, so the two are compared as well: that path is the fallback the
    format's whole "the sidecar is optional" promise rests on.
    """

    records: list[dict[str, JsonValue]] = [
        {"at_ms": 1_000 + (index // 3) * 10, "index": index, "text": f"body {index} " * 20}
        for index in range(900)
    ]
    path = tmp_path / "shard.jsonl.gz"
    _shard(path, records, 1 << 12)
    bare = tmp_path / "bare.jsonl.gz"
    bare.write_bytes(path.read_bytes())
    reader = _ChunkedJsonlReader(path)
    assert len(reader.index.members) > 10
    scanner = _ChunkedJsonlReader(bare)
    assert not scanner.has_index

    instants = sorted({int(str(record["at_ms"])) for record in records})
    for instant in instants:
        for target in (instant - 1, instant, instant + 1):
            expected = next(
                (
                    number
                    for number, record in enumerate(records)
                    if int(str(record["at_ms"])) >= target
                ),
                len(records),
            )
            assert reader.seek_first_line("at_ms", target) == expected, target
            assert scanner.seek_first_line("at_ms", target) == expected, target
    assert reader.seek_first_line("at_ms", instants[-1] + 1_000) == len(records)
    assert scanner.seek_first_line("at_ms", instants[-1] + 1_000) == len(records)


def test_iter_records_from_numbers_the_lines_it_yields(tmp_path: Path) -> None:
    """The open-ended read, with and without an index, including past the end."""

    records = _plain_records(500)
    path = tmp_path / "shard.jsonl.gz"
    _shard(path, records, 1 << 12)
    bare = tmp_path / "bare.jsonl.gz"
    bare.write_bytes(path.read_bytes())
    for reader in (_ChunkedJsonlReader(path), _ChunkedJsonlReader(bare)):
        for first in (0, 1, 137, 499):
            assert list(reader.iter_records_from(first)) == list(
                enumerate(records[first:], first)
            )
        assert list(reader.iter_records_from(len(records))) == []
        assert list(reader.iter_records_from(len(records) + 5)) == []


def test_the_uncompressed_digest_is_reproduced_by_reading_the_records(
    tmp_path: Path,
) -> None:
    """``iter_records_verified`` must fold the digest the sidecar declares, index or not."""

    records = _plain_records(400)
    path = tmp_path / "shard.jsonl.gz"
    _shard(path, records, 1 << 12)
    with SeekableJsonlReader(path) as packaged:
        digest = packaged.index.u_sha256
    bare = tmp_path / "bare.jsonl.gz"
    bare.write_bytes(path.read_bytes())
    for reader in (_ChunkedJsonlReader(path), _ChunkedJsonlReader(bare)):
        assert list(reader.iter_records_verified(digest)) == records
        with pytest.raises(ArchiveReadError, match="declared digest"):
            list(reader.iter_records_verified(hashlib.sha256(b"no").hexdigest()))


def test_a_windowed_read_touches_less_than_the_shard(tmp_path: Path) -> None:
    path = tmp_path / "shard.jsonl.gz"
    _shard(path, _plain_records(4_000), 1 << 14)
    reader = _ChunkedJsonlReader(path)
    records = list(reader.read_range(1_000, 1_500))
    assert records
    assert 0 < reader.data_bytes_read < path.stat().st_size


def test_losing_the_sidecar_costs_speed_and_nothing_else(tmp_path: Path) -> None:
    """The scan path must read what the index path reads, including a many-window member.

    The member size is what broke this: the scan reads a fixed number of *compressed* bytes
    per iteration, so a member that compresses to more than one window hands the splitter a
    buffer ending mid-record. Incompressible payloads at the default target reach 796,663
    compressed bytes in one member -- three windows -- so the shard here is built from random
    bytes rather than from the well-compressing fixture above, which would never have found
    it.
    """

    random.seed(99)
    records: list[dict[str, JsonValue]] = [
        {
            "at_ms": 1_000 + index,
            "index": index,
            "text": base64.b64encode(random.randbytes(9_000)).decode("ascii"),
        }
        for index in range(200)
    ]
    indexed = tmp_path / "indexed.jsonl.gz"
    _shard(indexed, records, 1 << 20)
    with SeekableJsonlReader(indexed) as packaged:
        assert max(member.c_len for member in packaged.index.members) > (1 << 18)

    bare = tmp_path / "bare.jsonl.gz"
    bare.write_bytes(indexed.read_bytes())
    scanning = _ChunkedJsonlReader(bare)
    assert not scanning.has_index
    assert [record["index"] for record in scanning.iter_records()] == list(range(200))
    assert _ChunkedJsonlReader(bare).tail(3) == _ChunkedJsonlReader(indexed).tail(3)
    assert list(_ChunkedJsonlReader(bare).read_range(1_010, 1_015)) == list(
        _ChunkedJsonlReader(indexed).read_range(1_010, 1_015)
    )
    assert list(_ChunkedJsonlReader(bare).read_lines(5, 9)) == list(
        _ChunkedJsonlReader(indexed).read_lines(5, 9)
    )


def test_a_stale_sidecar_is_refused_rather_than_believed(tmp_path: Path) -> None:
    path = tmp_path / "shard.jsonl.gz"
    _shard(path, _plain_records(400), 1 << 13)
    sidecar = path.with_name(path.name + ".index.jsonl")
    lines = sidecar.read_text(encoding="utf-8").split("\n")
    header = json.loads(lines[0])
    header["c_size"] = header["c_size"] + 1
    lines[0] = json.dumps(header, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    sidecar.write_text("\n".join(lines), encoding="utf-8")
    with pytest.raises(ArchiveReadError):
        _ChunkedJsonlReader(path)
