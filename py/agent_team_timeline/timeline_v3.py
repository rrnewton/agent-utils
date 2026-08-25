"""Schema 3: the presentation timeline as chunked, seekable JSONL.

Every number in this document was measured against one archive on one day, and that archive grows
daily. They are recorded to the byte anyway, because a rounded number cannot be re-derived and a
re-derivation that lands two per cent away is the only way to notice that a claim has gone stale.
Where a figure is quoted twice below it is the same measurement, taken on 2026-08-24.

Schema 1 is one JSON value of 246,973,399 bytes over 5,769,917 physical lines, and `inspect`
parses all of it at 1.44 GiB RSS to answer any question at all. Schema 2 broke that into 334
content-addressed objects, which fixed *addressability* and then gave most of the win back: the
objects are pretty-printed at two-space indent (~13 lines per record, 17% of the bytes buying
nothing a browser reads), every one of them large enough to be worth it is written twice -- once
plain and once as a `.gz` sidecar -- and the bootstrap that publishes them is 5,702,530 bytes
because 2,059 pre-aggregated activity bins are inlined into the file the browser must download
before it can draw anything.

Schema 3 keeps schema 2's shape -- a small stable bootstrap naming a set of immutable shards --
and changes what a shard *is*: a multi-member gzip file of minified JSONL, one record per line,
written through :mod:`agent_team_timeline.seekable_jsonl` and accompanied by that module's
sidecar index. The three consequences are the three defects above, each removed at its source:

* **Minified.** One line per record, `sort_keys`, no spaces. Re-encoding the schema-2 objects
  minified measured 83% of pretty; the indent was never read by anything.
* **Compressed only, with no plain twin.** Every `.gz` in the archive currently has an
  uncompressed sibling -- 2.42 GB of `.json` against 0.19 GB of `.gz` -- so compression there is
  duplication, not saving. A schema-3 shard exists once. It is still an entirely ordinary gzip
  file, so `zcat`, `gunzip`, `gzip -t`, `file(1)` and `gzip.open` all read it, and losing the
  sidecar costs speed and nothing else.
* **A bootstrap that fits in one packet's worth of parsing.** Activity bins are a shard, not an
  inline field; see "Where the activity bins went" below.

This module is the write path. What reads it is `query._SchemaThreeArchive`, which every list,
show, search and stats surface now goes through when a *complete* schema-3 generation is present
-- see that class for the completeness rule and for why a partial publication falls back rather
than being read.

**Schema 2 is still written beside it; schema 1 is not.** The two generations were retired at
different times because their readers retired them at different times. `query.py` reaches schema
1 only when neither of the newer bootstraps is present, and `static/app.js` reaches it only when
its schema-2 load throws -- so a published build stopped writing the 280,971,189-byte monolith,
and `render.retired_schema_1_files` says why not writing it and deleting it had to be separate
decisions. Schema 2 stays because the website has no schema-3 mode at all: a browser reading
schema 3 needs multi-member gzip over HTTP Range in JavaScript, which does not exist yet, and
retiring the format the only graphical surface reads would be retiring the surface. Reclaiming
what an older build already wrote belongs to :mod:`agent_team_timeline.archive_gc`, not to a
build.

Schema 3 replaces the presentation *timeline*. It does not replace the transcript search corpus,
which is still schema 2's content-addressed day shards with their trigram blooms, so a transcript
search reads phases and agents from a schema-3 spine and messages from schema 2 -- the one
operation that touches both generations, and the one place the reader checks that they describe
the same source.

The one thing schema 3 publishes that schema 1 does not is the zoom bounds; see
:data:`_ACTIVITY_BOUNDS_KIND` for why they are published rather than recomputed on read, and what
the 324,624 bytes buy.

Sharding axis: (team, UTC day)
------------------------------
The timeline stream is one shard per team per UTC day::

    data/timeline-v3/timeline/<team>/<YYYY-MM-DD>.jsonl.gz

*Not per agent.* There are 2,555 agents in the measured archive against 12 teams and 33 days, and
the dominant query is "the most recent N", which per-agent sharding turns into O(agents) file
opens. Day sharding answers it with one open per team. The compression locality that per-agent
files would have bought -- every line repeating the same `agent_id`, `agent_path` and `team` --
is recovered instead by sorting within the shard, which is the next paragraph.

*Not one file per team, either*, even though :mod:`seekable_jsonl` would make that read just as
cheaply: the sidecar index carries `t0`/`t1` per member, so a day range inside a whole-team file
is a bisect and two member reads, and the file count would drop from 72 to 12. The reason to cut
at the day is **write amplification**, the same reason :mod:`agent_team_timeline.payloads` shards
by digest prefix rather than keeping one `payloads.jsonl`: a rebuild that observes one new day
would otherwise recompress and republish every team's entire history, and `_replace_if_changed`
would correctly report all 12 files as changed. With day shards, yesterday is byte-identical and
the rebuild touches only today.

Measured, by building the archive with its final UTC day removed and then adding that day back:
**387,547 bytes rewritten across 7 files**, of a 38,288,394-byte schema-3 generation -- 1.01%.
The seven are that team's shard for that day, that team's spine, the bins shard, their three
sidecars, and the bootstrap. The same day landing in a one-file-per-team layout would have
rewritten 5,335,449 bytes, 13.8 times as much, and in the schema-1 monolith it rewrites all
280,971,189 (`data/timeline.json` and its `.gz` twin). Read cost is a wash between per-day and
per-team files; incremental cost is not.

The axis is also not new. The schema-2 search corpus already shards by `(team, UTC day)`, so this
reuses a partition the archive has already committed to rather than inventing a second one that
would have to be kept consistent with it.

**UTC, never the display timezone.** The archive's display timezone is a configured value
(`America/New_York` today, `display_timezone_source` records where it came from). Cutting shards
on local midnight would make the on-disk layout a function of that setting, so changing it would
rewrite every shard and invalidate every digest. Schema 2 already uses UTC days for the same
reason; the browser converts for display.

Sorted by `(at_ms, record_kind, line)` within a shard
-----------------------------------------------------
Total, deterministic, and the tiebreaker is the encoded line itself, so the order is defined by
exactly the bytes that land on disk and cannot depend on a field one record kind happens to lack.
Sorting by instant is what recovers per-agent compression locality without per-agent files --
records from one agent cluster in time -- and it is what makes `timestamps_sorted` true in the
sidecar, which is the precondition :func:`seekable_jsonl._bisectable` requires before the reader
may binary-search the member table.

Repeated fields are NOT factored out
------------------------------------
No struct-of-arrays, no per-shard string dictionary, no per-agent symbol table. Minified plus
gzip already reaches 6.3% on the two largest schema-2 objects, because gzip's LZ77
back-references *are* a dictionary coder for the repeated `agent_id`, `agent_path` and `team`
strings, and they cost nothing to decode and nothing to explain. Hand-factoring would buy a
fraction of an already-solved problem in exchange for a format `jq` cannot read -- and `jq` over
`zcat` is the fallback that makes this archive durable rather than merely compact.

**Delta-encoding `at_ms` was measured and rejected**, which is a stronger statement than
declining to try it. On the largest team's full timeline stream, replacing the absolute instant
with a delta from the day's start made the compressed output 2.07% *larger* (18,278,941 bytes to
18,657,655), and on a small team 10.4% larger: absolute 13-digit epoch milliseconds share a long
literal prefix that LZ77 back-references for free, while deltas are variable-width integers with
no such prefix. Isolated to the events stream alone -- where the instant is a tenth of the record
rather than a rounding error beside a prompt body -- delta-from-previous-record did win, 634,973
compressed bytes down to 485,145. It is still refused, for a reason size cannot outvote: the
sidecar index derives each member's `t0`/`t1` by reading `timestamp_key` off the records, so a
delta-encoded sort key leaves the index describing offsets instead of instants and destroys
`read_range`, `tail` and the bisect -- the entire property schema 3 exists to provide. Saving
150 KB by disabling seeking is not a trade, it is the opposite of the change.

The envelope: exactly two added keys
------------------------------------
A schema-3 line is its schema-1 record verbatim, plus:

``record_kind``
    Which collection the record came from, because a timeline-stream shard interleaves events,
    phases and edges in one time-ordered sequence and a reader must be able to tell them apart.
    Three separate files per day would avoid the field and cost three fetches to draw one day.

``at_ms``
    The instant the record enters the timeline: `at_ms` for an event (already present, and
    asserted to agree), `start_ms` for a phase, `min(source_ms, target_ms)` for an edge,
    `start_ms` for an activity bin. This is the sidecar's `timestamp_key`.

Adding a key that a record already carries would silently overwrite schema-1 data, so the writer
*refuses* rather than clobbers -- including the case where an event's own `at_ms` disagrees with
where the projection placed it, which is a disagreement worth a crash rather than a silent
correction. Removing `record_kind`, and removing `at_ms` from every kind but `event` (which
carried it in schema 1), recovers the schema-1 record byte for byte. That is a property
`test_timeline_v3.py` asserts over every collection, not a claim made here.

Records are assigned to one shard, not duplicated into every day they span
--------------------------------------------------------------------------
Schema 2 appends a phase or an edge to every UTC day its interval intersects, so a day shard is
self-sufficient. Schema 3 assigns each record to the single day containing its `at_ms`, and each
shard entry in the bootstrap publishes ``t_end_exclusive``: the smallest instant at which nothing
in the shard is still live. A reader rendering ``[T0, T1)`` selects shards where
``t0 < T1 and t_end_exclusive > T0``, which is one pass over a list of 72 entries it already holds.

**Everything here is half-open, including the records.** That is the whole content of the field
name, and it is stated because getting it half-right is the natural mistake. A phase is
``[start_ms, end_ms)`` -- 8,342 of the archive's 9,984 adjacent same-agent phase pairs have one
phase's ``end_ms`` exactly equal to the next one's ``start_ms``, and none overlap -- and an
activity bin is the same shape, so their reach is ``end_ms`` unadjusted. An event and an edge are
*points*, and a point at ``a`` is live over ``[a, a + 1)`` at millisecond resolution, so their
reach is one past the last instant they occupy. An earlier draft padded the edge and not the
event, which made the published rule right for edges and off by one for events: a shard whose
newest record was an event at exactly ``T0`` failed ``t_end_exclusive > T0`` and was skipped,
losing a record inside the window.

The rule also names ``t0``, which is what a catalog entry publishes. An earlier draft named
``start_ms``, a field the entry does not carry at all, so no reader could have implemented the
rule as written. `test_timeline_v3.py` recomputes the reach per shard from the records and
asserts the published value, so neither half can drift again.

Measured, the choice costs nothing today: of 11,899 phases, the 49,542 non-structural edges and
379,006 events that make up the timeline stream, **0 cross a UTC midnight** -- phases are capped
at 1,800,000 ms and aligned, and the longest edge spans 5,146,972 ms without happening to straddle
one. That zero is a half-open zero, and it is worth saying which: 163 phases *end* exactly on a
UTC midnight, so under a closed reading they would each straddle a boundary by a single instant
they are not live for. It is not structurally impossible, so the
decision is made on principle rather than on the zero: duplication makes the index's
`record_count` stop meaning "records", makes `tail` able to return a record the caller already
has, and makes "schema 3 is a faithful re-encoding of schema 1" a claim needing an asterisk. The
differential in :mod:`agent_team_timeline.losslessness` compares record *sets*; a projection that
emits one record twice is one the differential has to be taught to forgive.

Three streams
-------------
``timeline``  ``data/timeline-v3/timeline/<team>/<YYYY-MM-DD>.jsonl.gz``
    Events, phases and non-structural edges: everything with a position on the time axis and
    enough volume to be worth seeking into. 93% of schema 3's bytes.

``spine``  ``data/timeline-v3/spine/<team>.jsonl.gz``
    The per-team records that have no single instant, or that a reader needs *all* of before it
    can draw anything: the team record, agents, phase cards, structural edges
    (`spawn`/`continuation`/`result` -- the spawn tree is a graph, and fetching it a day at a
    time to draw it whole is the wrong shape), rollups, projects, summary files, glossary terms,
    the project overview, and the derived zoom bounds of :data:`_ACTIVITY_BOUNDS_KIND`.

    **The spine deliberately carries no `at_ms`.** Its shards are addressed by *line range*, not
    by time: records are grouped by kind in :data:`_SPINE_KIND_ORDER` and the bootstrap publishes
    each kind's `[l0, n)`, so "just the agents" is one
    :meth:`~seekable_jsonl.SeekableJsonlReader.read_lines` call reading the members those lines
    fall in. Giving spine records a timestamp would make `read_range` *appear* to work on them
    and then quietly under-return -- an agent alive across the whole window has one `start_ms`,
    which is outside almost every window a reader will ask about. A stream with no time axis
    should say so rather than offer a broken one. Agents and rollups keep their own
    `start_ms`/`end_ms`; there are 2,555 agents in the whole archive, so a reader filters them in
    memory.

    One file per team rather than one file per (team, kind): 10 kinds times 12 teams is 120
    mostly tiny files, each with its own sidecar, and the line-range door already exists.

    This is the stream every command-line question is answered from, and it is 2,412,120 bytes
    of a 38,288,394-byte generation. Its shards are the only ones `query.py` opens.

``bins``  ``data/timeline-v3/bins.jsonl.gz``
    Activity bins, all teams, sorted by `(at_ms, ...)` and therefore bisectable.

Where the activity bins went, and why the bootstrap is small
------------------------------------------------------------
Schema 2's bootstrap is 5,702,530 bytes, and it is that size because `activity_bins` is inlined:
2,059 pre-aggregated bins, 811,708 bytes minified, pretty-printed and then published in the one
file a browser must have in hand before it can render its first frame. Schema 3 moves them to
their own shard.

The objection to answer is "the overview needs the bins immediately". It does -- one round trip
later, from a shard that gzips to a fraction of its 812 KB, while the frame that shows the teams,
the range and the shard catalogue has already painted. The bins also grow as
`days x teams x resolutions x roles`, so inlining them makes the *entry point* the fastest-growing
file in the archive; everything else in the bootstrap grows with the shard count. A bootstrap
whose size is set by the largest pre-aggregation it happens to carry is not a bootstrap.

They are one shard rather than one per team, sorted by instant across all teams, because the
overview draws every team at once: per-team bins would be 12 fetches for the first chart, and the
global sort keeps the shard bisectable so a reader zoomed into a window reads only that window.

The bootstrap itself is written **plain and pretty**, as one JSON object at
``data/timeline-v3.json``, with no `.gz` twin. That is a deliberate exception to "compressed
only": at the measured size the twin would save tens of kilobytes against a shard set measured in
tens of megabytes, the duplication being removed is 2.42 GB of `.json` beside 0.19 GB of `.gz`
and not this, transfer compression is the wire's job, and the entry point is the one file that
must stay readable by `jq`, by a human diffing a rebuild, and by a browser with no decoder wired
up yet. Pretty-printing it costs about 17% of a file that is three orders of magnitude smaller
than the data it describes.

Stale shards, and why there is no reachability manifest
-------------------------------------------------------
Schema 2 needs `data/timeline-v2/manifest.json` because its object names are digests: nothing in
a directory listing says which objects the current generation reaches, so the writer maintains a
second file that does, plus a scope rule so a partial render does not delete another team's
objects. Schema 3 names shards by `(stream, team, day)`, so the bootstrap *is* the reachability
set, and :attr:`TimelineV3Report.generated_files` hands it to the caller's existing stale-file
removal -- `render._remove_stale_presentation_files` and `multi_team._remove_stale_files` -- which
already own that job for every other generated path. One fewer file, one fewer invariant.

Publication order is unchanged and load-bearing: every shard and every sidecar exists before the
bootstrap that names them is written.

**The writer owns the directory, and empties it of everything it did not just write.** After the
bootstrap is published, :func:`write_timeline_v3` removes every shard-shaped file under
``data/timeline-v3/`` outside its own plan, and `rmdir`s what that empties. Handing the job to the
caller's manifest-driven removal alone is not enough, and the gap is not hypothetical: that
removal reaps `previous - current` out of ``data/export.json``, which is written *after* this
function returns, so shards published by a build that died before its own bootstrap are in no
manifest's `previous` and no manifest's `current`. Nothing would ever name them again.

That leftover matters far beyond the disk it occupies, because absence from the catalogue is not
by itself evidence of death: a shard the catalogue does not name is a *retired team's* shard if
the bootstrap beside it is newer, and a *live team's* shard if the bootstrap is older, and the two
are indistinguishable from the outside. A sweeper that guessed would eventually guess "retired"
about the better half of a rebuild. Making the publisher clear its own tree removes the ambiguity
at the only point where it is not a guess -- here, where the plan is known -- and lets both the
reader (rule 5 of :class:`agent_team_timeline.query._SchemaThreeArchive`) and
:mod:`agent_team_timeline.archive_gc` treat any leftover as one unambiguous fact: a build was
interrupted, and the remedy is to run one.

Only files matching the two shapes this module emits are removed, and never through a symlink, so
"owns the directory" means the output it produces and not whatever an operator put beside it.

What it costs, measured
-----------------------
Three write paths over the same schema-1 input -- one team's records sliced out of the archive and
handed to `_write_compressible_json`, `write_timeline_shards` and `write_timeline_v3` in turn --
counting every file each one leaves on disk::

    input                        v1 (json+gz)   v2 (objects+gz)   v3 (shards+index+bootstrap)
    one mid-sized team             13,615,030        14,033,360                     1,002,954
      27,815 records                     2 files          17 files                     15 files
    the largest team              118,048,747       124,230,844                    19,146,119
      178,685 records                    2 files          51 files                     49 files

That is **7.4%** of schema 1 and **7.1%** of schema 2 on the mid-sized team; 16.2% and 15.4% on
the largest, where a third of the bytes are message bodies that were always going to dominate.
Against the two older generations together, schema 3 is 3.6% and 7.9%.

Over the whole archive schema 3 is 38,288,394 bytes in 171 files -- 72 timeline shards totalling
35,655,306 compressed bytes, 12 spine shards totalling 2,412,120, one 65,384-byte bins shard,
their sidecars, and an **89,298-byte bootstrap against schema 2's 5,702,530**. The sidecars are
66,286 bytes of that, 0.17%, which is the number the phrase "a roughly fixed fraction of the shard
set" refers to wherever it appears. Against the generations it sits beside: schema 1 is
280,971,189 bytes in 2 files and schema 2 is 1,503,831,881 in 661, so schema 3 is **13.6% of
schema 1, 2.5% of schema 2, and 2.1% of the two together** -- 1.79 GB against 38 MB.

478,007 records, of which 463,423 are the schema-1 timeline re-encoded and 14,584 are the derived
zoom bounds. Those cost 324,624 bytes, 0.85% of the generation, and they are 0.02% of what schema
1 and schema 2 occupy between them; the recomputation they replace would have read tens of
megabytes of the timeline stream per zoom.

That the timeline stream is 93% of the generation is the number the read path turns on: no
question `query.py` asks opens it. Every list, show, search and stats answer comes out of the
2,412,120-byte spine, and a one-team answer out of one team's share of it.

Determinism
-----------
Two builds over identical input produce identical bytes; two builds in separate processes under
different ``PYTHONHASHSEED`` values over the measured archive produced byte-identical trees across
all 171 files, and a third build in place reported `files_changed == 0`. Records are encoded with
sorted keys and compact separators; shards are iterated in sorted order; members carry `mtime=0` through
:func:`static_assets.deterministic_gzip`; day boundaries are UTC and depend on no configuration;
and every sort is total, tiebroken on the encoded line. :func:`seekable_jsonl.write_seekable_jsonl_lines`
replaces a file only when its bytes differ, so an unchanged rebuild churns nothing.
"""

from __future__ import annotations

import json
import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from types import MappingProxyType

from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    as_string,
    canonical_json,
    narrow_json,
    write_text_if_changed,
)
from agent_team_timeline.seekable_jsonl import (
    DEFAULT_TARGET_CHUNK_BYTES,
    INDEX_SUFFIX,
    WriteReport,
    write_seekable_jsonl_lines,
)
from agent_team_timeline.static_assets import GZIP_COMPRESSION_LEVEL
from agent_team_timeline.timeline_shards import activity_bounds


SCHEMA_3_VERSION = 3
SCHEMA_3_BOOTSTRAP_PATH = "data/timeline-v3.json"
SCHEMA_3_ROOT = "data/timeline-v3"
SCHEMA_3_TIMELINE_ROOT = f"{SCHEMA_3_ROOT}/timeline"
SCHEMA_3_SPINE_ROOT = f"{SCHEMA_3_ROOT}/spine"
SCHEMA_3_BINS_PATH = f"{SCHEMA_3_ROOT}/bins.jsonl.gz"

#: The sidecar's ``timestamp_key`` and the sort key of every time-addressed stream. Named for the
#: field schema-1 events already carry, so an event's line is unchanged by the envelope.
SCHEMA_3_TIMESTAMP_KEY = "at_ms"

#: The one classifying field the envelope adds. Not ``kind``: events and edges already carry a
#: ``kind`` meaning something else entirely (``user_prompt``, ``message``), and reusing the name
#: would collide on exactly the records that most need to be told apart.
SCHEMA_3_RECORD_KIND_KEY = "record_kind"

#: Edge kinds hoisted into the spine. These are the spawn tree, not conversation: a reader draws
#: the whole graph or none of it, so slicing them by day would mean fetching every day to draw
#: the first frame. Schema 2 reached the same conclusion and put them in its ``global`` object.
_STRUCTURAL_EDGE_KINDS = frozenset({"spawn", "continuation", "result"})

#: The subset of a phase a card needs: schema 2's nine card fields, plus ``team`` because a
#: schema-3 spine shard is per-team and the record has to say which one it belongs to.
#:
#: Schema 2 also attaches ``activity_start_ms``/``activity_end_ms`` to every card, and schema 3
#: does *not* put them here. It publishes them, but as their own spine kind -- see
#: :data:`_ACTIVITY_BOUNDS_KIND`.
_PHASE_CARD_FIELDS = frozenset(
    {
        "id",
        "agent_id",
        "start_ms",
        "end_ms",
        "phrase",
        "paragraph",
        "summary_available",
        "detail_path",
        "stats",
        "team",
    }
)

#: The zoom bounds, published rather than derived on read.
#:
#: ``zoomToActivityRange`` wants the smallest interval that actually contains a phase's,
#: an agent's or a rollup's work, so that "zoom to agent lifetime" on an agent that was idle
#: for six of its seven hours frames the hour rather than the seven. Schema 2 attaches those
#: two numbers to 14,584 records -- 2,555 agents, 130 rollups, 11,899 phase cards -- and the
#: first draft of schema 3 carried them nowhere, on the ground that they are *derived* and the
#: envelope is "the schema-1 record plus exactly two keys".
#:
#: **They are published.** The alternative -- recompute them when a reader asks -- was weighed
#: and refused, and the reason is that the recomputation is the exact scan the bounds exist to
#: avoid. :func:`timeline_shards.activity_bounds` derives them from every phase's ``states``
#: array and every event and edge instant belonging to the agent; a reader answering "zoom to
#: this agent's lifetime" would therefore have to read every timeline shard the agent's
#: lifetime intersects -- for a long-lived agent, that is its team's entire stream, which is
#: 93% of schema 3's bytes. Publishing 14,584 small records to avoid re-reading tens of
#: megabytes is not a close call, and it is the trade schema 2 already made.
#:
#: The envelope is nonetheless intact, because these are **not** schema-1 records with two
#: extra keys: they are a derived kind of their own, like ``phase_card``, keyed by the
#: archive's stable reference. An ``agent`` line in a spine shard is still its schema-1 record
#: byte for byte, so the losslessness differential stays a comparison rather than a
#: subtraction, and a reader that does not zoom never reads this kind -- it is last in
#: :data:`_SPINE_KIND_ORDER` precisely so that it falls outside the member a first frame
#: inflates.
_ACTIVITY_BOUNDS_KIND = "activity_bounds"

#: Spine kinds in the order they are laid down, and therefore the order of their line ranges.
#: Declared rather than sorted so the records a first frame needs -- the team, its agents, the
#: phase cards behind the navigation strip -- land in the first member, which is the only member
#: a reader that wants them has to inflate, and so that the kind only a zoom interaction needs
#: lands after them.
_SPINE_KIND_ORDER: tuple[str, ...] = (
    "team",
    "agent",
    "phase_card",
    "structural_edge",
    "rollup",
    "project",
    "summary_file",
    "glossary_term",
    "project_overview",
    _ACTIVITY_BOUNDS_KIND,
)

#: The field each spine kind is ordered by within its group. Every sort below tiebreaks on the
#: encoded line as well, so a key that is missing from a record, or duplicated across two, costs
#: the order its readability and never its determinism -- which is what lets this table name a
#: field per kind rather than demand a universal identifier the schema does not have.
_SPINE_SORT_FIELD: Mapping[str, str] = {
    "team": "slug",
    "agent": "id",
    "phase_card": "id",
    "structural_edge": "id",
    "rollup": "path",
    "project": "project_id",
    "summary_file": "path",
    "glossary_term": "id",
    "project_overview": "team",
    _ACTIVITY_BOUNDS_KIND: "ref",
}

#: Schema-1 top-level keys copied into the bootstrap as-is.
_BOOTSTRAP_FIELDS: tuple[str, ...] = (
    "generated_at",
    "source_digest",
    "display_timezone",
    "display_timezone_source",
    "range",
    "stats",
    "artifact_catalog_path",
    "glossary_path",
)

#: Schema-1 top-level keys this module knows how to project. A key in neither this set nor
#: :data:`_BOOTSTRAP_FIELDS` is a refusal, not a silent drop: the next field added to schema 1
#: must be classified deliberately, the same way `scripts/validate.py` makes an unclassified path
#: select every check rather than none.
_STREAMED_FIELDS: frozenset[str] = frozenset(
    {
        "teams",
        "agents",
        "phases",
        "edges",
        "events",
        "rollups",
        "projects",
        "summary_files",
        "glossary",
        "activity_bins",
        "project_overview",
        "project_overviews",
    }
)

_KNOWN_FIELDS: frozenset[str] = (
    frozenset(_BOOTSTRAP_FIELDS) | _STREAMED_FIELDS | {"schema_version"}
)

_SLUG_CHARACTERS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
)


class TimelineV3Error(ValueError):
    """A schema-1 timeline this module refuses to project.

    Distinct from a bare ``ValueError`` so a caller wiring schema 3 in beside schema 2 can tell
    "the projection rejected the input" from "a narrowing helper found the wrong type", and so a
    build can decide to keep publishing schema 1 and 2 while schema 3 is still being taught about
    a new field.
    """


@dataclass(frozen=True)
class ShardReport:
    """One written shard, in enough detail to publish it in the bootstrap."""

    stream: str
    team: str | None
    day: str | None
    relative_path: str
    write: WriteReport
    #: The largest *inclusive* end instant of any record in the shard: the last instant at which
    #: anything here is still live. One convention across every record kind -- see the module
    #: docstring's "Records are assigned to one shard" section for why the reader rule is ``>=``
    #: and what went wrong when one kind was padded and the others were not.
    t_end_exclusive: int | None
    counts: Mapping[str, int]
    line_ranges: Mapping[str, tuple[int, int]]

    @property
    def index_relative_path(self) -> str:
        """Where the sidecar sits, relative to the archive root."""

        return self.relative_path + INDEX_SUFFIX

    @property
    def generated_files(self) -> tuple[str, ...]:
        """The pair this shard occupies in the caller's generated-file manifest."""

        return (self.relative_path, self.index_relative_path)

    def catalog_obj(self) -> dict[str, JsonValue]:
        """Render this shard's bootstrap entry.

        ``c_sha256`` and ``u_sha256`` are both published. They answer different questions -- the
        first that the bytes served are the bytes written, the second that the records inside
        them are the records projected -- and the archive's existing integrity contract is stated
        over the uncompressed stream, so dropping either would leave one of the two unanswerable
        without a full re-read.
        """

        index = self.write.index
        entry: dict[str, JsonValue] = {
            "stream": self.stream,
            "team": self.team,
            "day": self.day,
            "path": self.relative_path,
            "index_path": self.index_relative_path,
            "records": index.record_count,
            "members": len(index.members),
            "c_bytes": index.c_size,
            "u_bytes": index.u_size,
            "c_sha256": index.c_sha256,
            "u_sha256": index.u_sha256,
            "timestamps_sorted": index.timestamps_sorted,
            "t0": index.members[0].t0 if index.members else None,
            "t1": max((m.t1 for m in index.members if m.t1 is not None), default=None),
            "t_end_exclusive": self.t_end_exclusive,
        }
        if self.line_ranges:
            # A spine shard's line ranges already carry every count, and to a reader they carry
            # it in the form it can act on. Publishing both would be two answers to one question.
            entry["line_ranges"] = {
                kind: [first, count]
                for kind, (first, count) in sorted(self.line_ranges.items())
            }
        elif self.counts:
            entry["counts"] = {kind: count for kind, count in sorted(self.counts.items())}
        return entry


@dataclass(frozen=True)
class TimelineV3Report:
    """What one schema-3 projection wrote, and what it cost."""

    files_changed: int
    generated_files: tuple[str, ...]
    timeline_shards: int
    spine_shards: int
    record_count: int
    bootstrap_bytes: int
    compressed_bytes: int
    uncompressed_bytes: int
    index_bytes: int
    #: Shard-shaped files found under the schema-3 root and outside this build's plan, removed
    #: after the bootstrap was published. Reported separately from ``files_changed`` rather than
    #: only folded into it, because a non-zero value here is a *finding*: on a healthy archive
    #: every build leaves the tree equal to its catalogue, so anything removed is the residue of
    #: a build that did not finish.
    removed_files: tuple[str, ...] = ()

    @property
    def total_bytes(self) -> int:
        """Everything schema 3 occupies on disk: shards, sidecars and the bootstrap."""

        return self.compressed_bytes + self.index_bytes + self.bootstrap_bytes


#: The empty line-range table, named so the keyword default is a shared immutable value rather
#: than a fresh dict evaluated once at import and then shared by accident.
_NO_LINE_RANGES: Mapping[str, tuple[int, int]] = MappingProxyType({})


@dataclass(frozen=True)
class _Line:
    """One encoded record, with everything the shard writer needs to place and order it.

    The encoded bytes are carried rather than the record, and the sort key is carried beside them
    rather than read back out of them. Both choices are about the largest team's stream, which is
    84 MB of lines: keeping the parsed record alongside would roughly double peak memory for a
    value used once, and re-parsing each line inside the sort comparator would run `json.loads`
    O(n log n) times over exactly those bytes.
    """

    at_ms: int | None
    kind: str
    key: str
    line: bytes
    #: The record's **exclusive** reach: the smallest instant at which it is no longer live. Half
    #: open like everything else here, so a phase or a bin contributes its `end_ms` unadjusted and
    #: a point -- an event, an edge -- contributes one past the last instant it occupies. ``None``
    #: for a spine record, which has no position on the time axis at all.
    end_ms: int | None

    def time_order(self) -> tuple[int, str, bytes]:
        """Order within a time-addressed shard: instant, then kind, then the bytes themselves.

        Every record in a time-addressed stream has an instant; the ``0`` is unreachable and is
        here so the key's type is a plain int rather than something the sort has to compare
        against ``None``.
        """

        return (self.at_ms if self.at_ms is not None else 0, self.kind, self.line)

    def kind_order(self) -> tuple[str, bytes]:
        """Order within one kind's group in a spine shard."""

        return (self.key, self.line)


def _local_identifier(team: str, identifier: str) -> str:
    """Strip the ``<team>::`` prefix the combined export puts on every identifier.

    The single-team render leaves identifiers bare and the combined export qualifies them, so
    a reference built from the raw identifier would name the same agent two ways depending on
    which renderer produced the archive. Stripping the prefix makes the reference the same
    string in both, which is the whole reason it is called stable.
    """

    prefix = f"{team}::"
    return identifier[len(prefix) :] if identifier.startswith(prefix) else identifier


def agent_reference(team: str, identifier: str) -> str:
    """The archive's stable reference for one agent."""

    return f"agent:{team}::{_local_identifier(team, identifier)}"


def phase_reference(team: str, identifier: str) -> str:
    """The archive's stable reference for one work phase."""

    return f"phase:{team}::{_local_identifier(team, identifier)}"


def rollup_reference(team: str, kind: str, start_ms: int) -> str:
    """The archive's stable reference for one calendar rollup.

    A rollup has no identifier of its own, so the triple that identifies it is the reference.
    """

    return f"rollup:{team}::{kind}::{start_ms}"


# The three functions above are a second implementation of `query.agent_ref`, `query.phase_ref`
# and `query.rollup_ref`. The duplication is deliberate and cannot be removed by importing:
# `query.py` is copied verbatim into every generated archive as the `./timeline` executable and
# may import nothing outside the standard library, so the reader cannot import the writer. It is
# also not left to a comment to enforce -- `test_timeline_v3.py` builds a timeline, computes both
# forms for every agent, phase and rollup in it, and asserts they are the same string, because a
# comment does not fail.


def utc_day_start(timestamp_ms: int) -> int:
    """The UTC midnight at or before *timestamp_ms*, in epoch milliseconds."""

    instant = datetime.fromtimestamp(timestamp_ms / 1000, tz=timezone.utc)
    midnight = datetime(instant.year, instant.month, instant.day, tzinfo=timezone.utc)
    return int(midnight.timestamp() * 1000)


def utc_day_label(day_start_ms: int) -> str:
    """The ``YYYY-MM-DD`` a shard filename is built from."""

    return datetime.fromtimestamp(day_start_ms / 1000, tz=timezone.utc).date().isoformat()


def _encode(record: Mapping[str, JsonValue]) -> bytes:
    """Encode one schema-3 line: sorted keys, compact separators, no NaN.

    Byte for byte what :func:`seekable_jsonl.write_seekable_jsonl` would have produced from the
    same record, which is why the shards can be written through the pre-encoded-lines door
    without changing what lands on disk. ``allow_nan=False`` for the reason that module gives:
    Python emits a bare ``NaN`` token that Python reads back happily and every other JSON parser
    rejects, and a durable format cannot ship a value only its own writer can read.
    """

    return json.dumps(
        dict(record),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    ).encode("utf-8")


def is_schema_3_path(relative: str) -> bool:
    """Whether an archive-relative path is one this module owns.

    Published because the callers that embed schema 3 in a larger export -- `render` and
    `multi_team` -- both keep a strict allowlist of the paths their manifests may name, and both
    would otherwise grow a second, drifting copy of this shape. The shape is checked rather than
    a listing kept, because the shard set is a function of the data: teams and days are
    discovered at render time and there is nothing to enumerate ahead of them.

    It is deliberately tight enough to do a listing's job -- a fixed root, a fixed depth per
    stream, components restricted to the characters :func:`_safe_slug` admits, and an exact
    suffix -- so that a path this returns True for is a path this module could have written.
    """

    if relative == SCHEMA_3_BOOTSTRAP_PATH:
        return True
    body = relative.removesuffix(INDEX_SUFFIX)
    if not body.endswith(".jsonl.gz"):
        return False
    root = SCHEMA_3_ROOT.split("/")
    parts = body.split("/")
    if parts[: len(root)] != root:
        return False
    tail = parts[len(root) :]
    for part in tail:
        stem = part.removesuffix(".jsonl.gz")
        if not stem or stem[0] == "." or not set(stem) <= _SLUG_CHARACTERS:
            return False
    # One shape per stream, keyed by the component that names the stream. Schema 3 has exactly
    # three, and a fourth must be added here deliberately rather than admitted by a wildcard.
    depths: Mapping[str, int] = {"bins.jsonl.gz": 1, "spine": 2, "timeline": 3}
    return depths.get(tail[0]) == len(tail)


def is_schema_3_shard_name(name: str) -> bool:
    """Whether a bare file name is one of the two shapes this module publishes.

    Deliberately looser than :func:`is_schema_3_path`, and the looseness is the point. This is
    the predicate that decides what :func:`_remove_unplanned` clears out of the schema-3 root and
    -- restated, because that module imports nothing from this package -- what the reader counts
    when it checks that the tree holds nothing the catalogue omits. Those two must agree, and
    they must agree in the safe direction: a file the reader notices and the writer will not
    clear is a generation that declines forever and that rebuilding cannot repair.

    Matching a *name* rather than a *path* is what guarantees the agreement, since it removes
    every question of depth and component grammar on which two implementations could differ. The
    cost is that a ``.jsonl.gz`` an operator parked inside ``data/timeline-v3/`` would be removed
    by the next build. That directory is machine-owned -- its name, its layout and its contents
    are all decided here -- so the trade is a hypothetical against a real cliff.
    """

    return name.endswith(".jsonl.gz") or name.endswith(".jsonl.gz" + INDEX_SUFFIX)


def _safe_slug(slug: str, where: str) -> str:
    """Refuse a team slug that cannot be a single path component.

    Schema 2 sidestepped this by naming every object after its digest. Schema 3 puts the team in
    the path on purpose -- that is what makes the bootstrap the reachability set -- so the slug
    has to be checked here rather than trusted. Everything outside ``[A-Za-z0-9._-]`` is refused
    including the separator and the empty string, and a leading dot is refused so a slug can
    never produce a hidden file, a parent traversal or a name the caller's own path guards would
    have to re-examine.
    """

    if not slug or slug[0] == "." or not set(slug) <= _SLUG_CHARACTERS:
        raise TimelineV3Error(f"{where}: team slug is not a safe path component: {slug!r}")
    return slug


def _envelope(
    record: Mapping[str, JsonValue],
    kind: str,
    at_ms: int | None,
    where: str,
) -> dict[str, JsonValue]:
    """Add the two schema-3 keys, refusing to overwrite anything schema 1 already said.

    An event already carries ``at_ms`` and it must be the instant we computed; anything else
    means the projection and the record disagree about when the record happened, and writing our
    answer over theirs would make the disagreement undetectable.
    """

    if SCHEMA_3_RECORD_KIND_KEY in record:
        raise TimelineV3Error(
            f"{where}: record already carries {SCHEMA_3_RECORD_KIND_KEY!r}; "
            "the schema-3 envelope would overwrite it"
        )
    projected = dict(record)
    projected[SCHEMA_3_RECORD_KIND_KEY] = kind
    if at_ms is None:
        return projected
    existing = record.get(SCHEMA_3_TIMESTAMP_KEY)
    if existing is not None and existing != at_ms:
        raise TimelineV3Error(
            f"{where}: record carries {SCHEMA_3_TIMESTAMP_KEY}={existing!r} but the "
            f"schema-3 projection places it at {at_ms}"
        )
    projected[SCHEMA_3_TIMESTAMP_KEY] = at_ms
    return projected


def _field_int(record: Mapping[str, JsonValue], field: str, where: str) -> int:
    return as_int(record.get(field), f"{where}.{field}")


def _team_of(
    record: Mapping[str, JsonValue], sole_team: str | None, where: str
) -> str:
    """Which shard a record belongs to.

    The single-team render does not stamp ``team`` onto records -- there is only one, and schema 1
    does not carry it -- while the combined export does. Rather than teach the renderer to emit a
    field nothing reads, the projection falls back to the sole team when there is exactly one and
    refuses when there is more than one, which is the only case where the fallback could be wrong.
    """

    raw = record.get("team")
    if raw is not None:
        return as_string(raw, f"{where}.team")
    if sole_team is None:
        raise TimelineV3Error(
            f"{where}: record has no 'team' and the timeline names more than one team"
        )
    return sole_team


def _collection(
    timeline: Mapping[str, JsonValue], field: str
) -> list[dict[str, JsonValue]]:
    raw = timeline.get(field)
    if raw is None:
        return []
    return [
        as_object(value, f"timeline.{field}[{index}]")
        for index, value in enumerate(as_array(raw, f"timeline.{field}"))
    ]


def _output_path(output: Path, relative: str, *, make_parents: bool) -> Path:
    """Resolve *relative* under *output*, refusing anything that leaves the archive.

    Every component of every schema-3 path is either a literal in this module, a team slug
    already through :func:`_safe_slug`, or an ISO date -- so traversal cannot come from the
    timeline. It can still come from the filesystem: a symlinked ``data/timeline-v3`` or a
    symlinked team directory would silently write the archive's shards somewhere else, and
    ``os.replace`` onto a symlink follows it. The combined export already refuses exactly this
    for its own generated paths; schema 3 refuses it here rather than relying on the caller,
    because this module creates directories the caller has never seen.
    """

    root = output.resolve()
    cursor = output
    for part in relative.split("/")[:-1]:
        cursor = cursor / part
        if cursor.is_symlink():
            raise TimelineV3Error(f"schema-3 output path crosses a symlink: {relative!r}")
        if make_parents:
            cursor.mkdir(parents=True, exist_ok=True)
    candidate = output.joinpath(*relative.split("/"))
    if candidate.is_symlink():
        raise TimelineV3Error(f"refusing to write schema-3 output through a symlink: {candidate}")
    try:
        candidate.parent.resolve().relative_to(root)
    except ValueError as error:
        raise TimelineV3Error(
            f"schema-3 output path escapes the archive: {relative!r}"
        ) from error
    return candidate


def _remove_unplanned(output: Path, published: frozenset[str]) -> tuple[str, ...]:
    """Remove shard-shaped files under the schema-3 root that *published* does not name.

    Called only after the bootstrap is on disk, so that an interruption during this pass leaves a
    complete generation with some residue beside it -- the harmless direction -- rather than a
    catalogue that names a file this pass has already unlinked.

    ``unlink`` rather than a move into `gc`'s trash, because this is not a decision an operator
    made and there is nothing here to reverse: the removed file is derived output that this same
    call has just re-derived, and the shard that replaced it is on disk before the first unlink.
    Directories emptied by the pass are removed innermost-first, since a retired team's day
    directory is otherwise a permanent empty marker for a team that no longer exists.
    """

    root = output / SCHEMA_3_ROOT
    if root.is_symlink() or not root.is_dir():
        return ()
    removed: list[str] = []
    for directory, subdirectories, names in os.walk(root, topdown=False, followlinks=False):
        here = Path(directory)
        if here.is_symlink():
            continue
        for name in sorted(names):
            if not is_schema_3_shard_name(name):
                continue
            path = here / name
            relative = path.relative_to(output).as_posix()
            if relative in published or path.is_symlink() or not path.is_file():
                continue
            path.unlink()
            removed.append(relative)
        del subdirectories
        if here != root and not any(here.iterdir()):
            here.rmdir()
    return tuple(sorted(removed))


@dataclass(frozen=True)
class _PlannedShard:
    """One shard decided on but not yet written.

    The plan exists so that publication is a single uninterrupted phase. It carries the encoded
    lines rather than a callback that would produce them, because the lines have to exist before
    the plan can be validated at all -- the team a record names is read off the record.
    """

    relative_path: str
    stream: str
    team: str | None
    day: str | None
    lines: list[_Line]
    line_ranges: Mapping[str, tuple[int, int]]


def _write_shard(
    output: Path,
    relative: str,
    stream: str,
    team: str | None,
    day: str | None,
    lines: Sequence[_Line],
    *,
    line_ranges: Mapping[str, tuple[int, int]] = _NO_LINE_RANGES,
    target_chunk_bytes: int = DEFAULT_TARGET_CHUNK_BYTES,
) -> ShardReport:
    """Write one shard and its sidecar, and describe it for the bootstrap."""

    counts: dict[str, int] = {}
    end_values: list[int] = []
    for item in lines:
        counts[item.kind] = counts.get(item.kind, 0) + 1
        if item.end_ms is not None:
            end_values.append(item.end_ms)
    report = write_seekable_jsonl_lines(
        _output_path(output, relative, make_parents=True),
        (item.line for item in lines),
        target_chunk_bytes=target_chunk_bytes,
        timestamp_key=SCHEMA_3_TIMESTAMP_KEY,
    )
    return ShardReport(
        stream=stream,
        team=team,
        day=day,
        relative_path=relative,
        write=report,
        t_end_exclusive=max(end_values) if end_values else None,
        counts=counts,
        line_ranges=line_ranges,
    )


def _timeline_lines(
    timeline: Mapping[str, JsonValue], sole_team: str | None
) -> dict[tuple[str, int], list[_Line]]:
    """Bucket the time-addressed records by ``(team, UTC day)``."""

    buckets: dict[tuple[str, int], list[_Line]] = {}

    def place(team: str, at_ms: int, item: _Line) -> None:
        buckets.setdefault((team, utc_day_start(at_ms)), []).append(item)

    for index, record in enumerate(_collection(timeline, "events")):
        where = f"timeline.events[{index}]"
        at_ms = _field_int(record, "at_ms", where)
        place(
            _team_of(record, sole_team, where),
            at_ms,
            # `at_ms + 1`, not `at_ms`: an event is a point, and a point's half-open reach ends
            # one millisecond later. Publishing `at_ms` made the shard catalogue's reader rule
            # skip a shard whose newest record was an event exactly at the window's start.
            _Line(
                at_ms,
                "event",
                "",
                _encode(_envelope(record, "event", at_ms, where)),
                at_ms + 1,
            ),
        )
    for index, record in enumerate(_collection(timeline, "phases")):
        where = f"timeline.phases[{index}]"
        start_ms = _field_int(record, "start_ms", where)
        end_ms = _field_int(record, "end_ms", where)
        place(
            _team_of(record, sole_team, where),
            start_ms,
            _Line(
                start_ms,
                "phase",
                "",
                _encode(_envelope(record, "phase", start_ms, where)),
                end_ms,
            ),
        )
    for index, record in enumerate(_collection(timeline, "edges")):
        where = f"timeline.edges[{index}]"
        if record.get("kind") in _STRUCTURAL_EDGE_KINDS:
            continue
        source_ms = _field_int(record, "source_ms", where)
        target_ms = _field_int(record, "target_ms", where)
        at_ms = min(source_ms, target_ms)
        place(
            _team_of(record, sole_team, where),
            at_ms,
            _Line(
                at_ms,
                "edge",
                "",
                _encode(_envelope(record, "edge", at_ms, where)),
                # Both endpoints are points, so the later one reaches one millisecond past
                # itself -- the same rule as an event, applied to the later of the two.
                max(source_ms, target_ms) + 1,
            ),
        )
    return buckets


def _spine_lines(
    timeline: Mapping[str, JsonValue], sole_team: str | None
) -> dict[str, dict[str, list[_Line]]]:
    """Bucket the spine records by team and then by kind, preserving neither's order yet."""

    buckets: dict[str, dict[str, list[_Line]]] = {}

    def place(team: str, kind: str, record: Mapping[str, JsonValue], where: str) -> None:
        raw = record.get(_SPINE_SORT_FIELD[kind])
        by_kind = buckets.setdefault(team, {})
        by_kind.setdefault(kind, []).append(
            _Line(
                None,
                kind,
                raw if isinstance(raw, str) else "",
                _encode(_envelope(record, kind, None, where)),
                None,
            )
        )

    for index, record in enumerate(_collection(timeline, "teams")):
        where = f"timeline.teams[{index}]"
        place(as_string(record.get("slug"), f"{where}.slug"), "team", record, where)
    for index, record in enumerate(_collection(timeline, "agents")):
        where = f"timeline.agents[{index}]"
        place(_team_of(record, sole_team, where), "agent", record, where)
    for index, record in enumerate(_collection(timeline, "phases")):
        where = f"timeline.phases[{index}]"
        card = {key: value for key, value in record.items() if key in _PHASE_CARD_FIELDS}
        place(_team_of(record, sole_team, where), "phase_card", card, where)
    for index, record in enumerate(_collection(timeline, "edges")):
        where = f"timeline.edges[{index}]"
        if record.get("kind") not in _STRUCTURAL_EDGE_KINDS:
            continue
        place(_team_of(record, sole_team, where), "structural_edge", record, where)
    for index, record in enumerate(_collection(timeline, "rollups")):
        where = f"timeline.rollups[{index}]"
        place(_team_of(record, sole_team, where), "rollup", record, where)
    for index, record in enumerate(_collection(timeline, "projects")):
        where = f"timeline.projects[{index}]"
        place(_team_of(record, sole_team, where), "project", record, where)
    for index, record in enumerate(_collection(timeline, "summary_files")):
        where = f"timeline.summary_files[{index}]"
        place(_team_of(record, sole_team, where), "summary_file", record, where)
    for index, record in enumerate(_collection(timeline, "glossary")):
        where = f"timeline.glossary[{index}]"
        place(_team_of(record, sole_team, where), "glossary_term", record, where)
    for index, record in enumerate(_collection(timeline, "project_overviews")):
        where = f"timeline.project_overviews[{index}]"
        place(_team_of(record, sole_team, where), "project_overview", record, where)
    singular = timeline.get("project_overview")
    if singular is not None:
        # The single-team render emits one unlabelled overview object; the combined export emits
        # a labelled list. Both become the same spine kind so a reader never has to know which
        # renderer produced the archive it opened.
        where = "timeline.project_overview"
        record = as_object(singular, where)
        place(_team_of(record, sole_team, where), "project_overview", record, where)

    # The zoom bounds, last, so that the member a first frame inflates does not carry them.
    # Every subject is walked in its own collection's order because that is the order
    # `activity_bounds` keys its rollup answer by; the two loops over `rollups` therefore have
    # to agree, and they do because both enumerate `_collection(timeline, "rollups")`.
    #
    # This is a second pass over the events and edges in the same build, because
    # `write_timeline_shards` already ran one for schema 2. Calling the same function twice is
    # the deliberate choice: caching the result across the two writers would couple them through
    # a shared object whose lifetime neither owns, and the alternative -- deriving schema 3's
    # bounds independently -- is the one thing that must not happen, since two derivations are
    # two answers to "where should this zoom". Measured at 5.1 seconds on the archive's 487,796
    # timeline instants -- 379,006 events and both endpoints of 54,395 edges -- against 16.6 for
    # the projection as a whole.
    phase_bounds, agent_bounds, rollup_bounds = activity_bounds(dict(timeline))
    for index, record in enumerate(_collection(timeline, "agents")):
        where = f"timeline.agents[{index}]"
        team = _team_of(record, sole_team, where)
        identifier = as_string(record.get("id"), f"{where}.id")
        place(
            team,
            _ACTIVITY_BOUNDS_KIND,
            _bounds_record(
                agent_reference(team, identifier),
                agent_bounds.get(identifier, _own_interval(record, where)),
            ),
            where,
        )
    for index, record in enumerate(_collection(timeline, "phases")):
        where = f"timeline.phases[{index}]"
        team = _team_of(record, sole_team, where)
        identifier = as_string(record.get("id"), f"{where}.id")
        place(
            team,
            _ACTIVITY_BOUNDS_KIND,
            _bounds_record(
                phase_reference(team, identifier),
                phase_bounds.get(identifier, _own_interval(record, where)),
            ),
            where,
        )
    rollups = _collection(timeline, "rollups")
    if len(rollup_bounds) != len(rollups):
        raise TimelineV3Error(
            "timeline.rollups: activity bounds do not line up with the rollups they describe"
        )
    for index, record in enumerate(rollups):
        where = f"timeline.rollups[{index}]"
        team = _team_of(record, sole_team, where)
        place(
            team,
            _ACTIVITY_BOUNDS_KIND,
            _bounds_record(
                rollup_reference(
                    team,
                    as_string(record.get("kind"), f"{where}.kind"),
                    _field_int(record, "start_ms", where),
                ),
                rollup_bounds[index],
            ),
            where,
        )
    return buckets


def _own_interval(record: Mapping[str, JsonValue], where: str) -> tuple[int, int]:
    """The record's declared interval, which is what an unbounded subject falls back to.

    Schema 2 uses exactly this fallback -- `_global_object` and `_phase_index_object` both pass
    ``(start_ms, end_ms)`` as the default of their ``bounds.get`` -- so schema 3 must too, or
    the two generations would disagree about the one record for which no activity was found.
    """

    return _field_int(record, "start_ms", where), _field_int(record, "end_ms", where)


def _bounds_record(reference: str, bounds: tuple[int, int]) -> dict[str, JsonValue]:
    """One zoom-bounds line: which record it is about, and the interval to frame.

    No ``team`` field: the shard is per-team and the reference already names the team, so a
    third copy of it would be 14,584 repetitions of something the reader already knows.
    """

    return {
        "ref": reference,
        "activity_start_ms": bounds[0],
        "activity_end_ms": bounds[1],
    }


def _ordered_spine(
    by_kind: Mapping[str, list[_Line]], where: str
) -> tuple[list[_Line], dict[str, tuple[int, int]]]:
    """Lay a team's spine out kind by kind, and report where each kind starts and how long it is."""

    ordered: list[_Line] = []
    ranges: dict[str, tuple[int, int]] = {}
    unknown = sorted(set(by_kind) - set(_SPINE_KIND_ORDER))
    if unknown:
        raise TimelineV3Error(f"{where}: unclassified spine kinds {unknown}")
    for kind in _SPINE_KIND_ORDER:
        group = by_kind.get(kind)
        if not group:
            continue
        group.sort(key=_Line.kind_order)
        ranges[kind] = (len(ordered), len(group))
        ordered.extend(group)
    return ordered, ranges


def _bins_lines(timeline: Mapping[str, JsonValue], sole_team: str | None) -> list[_Line]:
    lines: list[_Line] = []
    for index, record in enumerate(_collection(timeline, "activity_bins")):
        where = f"timeline.activity_bins[{index}]"
        _team_of(record, sole_team, where)
        start_ms = _field_int(record, "start_ms", where)
        end_ms = _field_int(record, "end_ms", where)
        lines.append(
            _Line(
                start_ms,
                "activity_bin",
                "",
                _encode(_envelope(record, "activity_bin", start_ms, where)),
                end_ms,
            )
        )
    lines.sort(key=_Line.time_order)
    return lines


def write_timeline_v3(
    output: Path,
    raw_timeline: Mapping[str, JsonValue],
    *,
    target_chunk_bytes: int = DEFAULT_TARGET_CHUNK_BYTES,
) -> TimelineV3Report:
    """Publish the schema-3 shards, then the bootstrap that names them.

    *output* is the archive root; every path this writes is relative to it. ``data/timeline-v2``
    remains the caller's, and is still published beside this, because the website reads it and
    has no schema-3 mode; ``data/timeline.json`` is no longer written by a published build at
    all. See the module docstring for why those two retirements are not the same event.

    **Every refusal is raised before the first byte is written.** The projection is built whole
    -- buckets, ordering, per-team validation, spine layout -- and only then published. The
    obvious shape, validating each shard as the loop reaches it, leaks: a rollup naming a team
    absent from ``teams[]`` is discovered in the spine loop, which runs after every timeline
    shard has already landed, and the caller then aborts before writing ``data/export.json``.
    Those shards are in no manifest, so the caller's stale-file removal -- which reaps
    ``previous - current`` from the export manifest -- can never see them, and a later build
    that no longer produces that team or day never will either. Orphans that no generation can
    name are worse than a slower failure, and the cost of avoiding them is nothing: the lines
    were already all in memory, because the sort needs them there.
    """

    timeline = as_object(narrow_json(dict(raw_timeline), "timeline"), "timeline")
    if as_int(timeline.get("schema_version"), "timeline.schema_version") != 1:
        raise TimelineV3Error("schema-3 projection requires a schema-1 source timeline")
    unknown = sorted(set(timeline) - _KNOWN_FIELDS)
    if unknown:
        raise TimelineV3Error(
            f"timeline: unclassified schema-1 fields {unknown}; schema 3 would drop them "
            "silently, so classify each one as bootstrap or streamed before publishing"
        )

    teams = _collection(timeline, "teams")
    slugs = [
        _safe_slug(as_string(team.get("slug"), f"timeline.teams[{index}].slug"), "timeline.teams")
        for index, team in enumerate(teams)
    ]
    if len(set(slugs)) != len(slugs):
        raise TimelineV3Error("timeline.teams: duplicate team slug")
    sole_team = slugs[0] if len(slugs) == 1 else None
    known_slugs = frozenset(slugs)

    # ---- plan ---------------------------------------------------------------------------
    # Nothing below this comment touches the filesystem until the plan is complete and every
    # refusal it can raise has been raised.
    planned: list[_PlannedShard] = []

    timeline_buckets = _timeline_lines(timeline, sole_team)
    for (team, day_start) in sorted(timeline_buckets):
        if team not in known_slugs:
            raise TimelineV3Error(f"timeline: record names team {team!r}, absent from teams[]")
        lines = timeline_buckets[(team, day_start)]
        lines.sort(key=_Line.time_order)
        day = utc_day_label(day_start)
        planned.append(
            _PlannedShard(
                relative_path=f"{SCHEMA_3_TIMELINE_ROOT}/{team}/{day}.jsonl.gz",
                stream="timeline",
                team=team,
                day=day,
                lines=lines,
                line_ranges=_NO_LINE_RANGES,
            )
        )

    spine_buckets = _spine_lines(timeline, sole_team)
    for team in sorted(spine_buckets):
        if team not in known_slugs:
            raise TimelineV3Error(f"timeline: record names team {team!r}, absent from teams[]")
        ordered, ranges = _ordered_spine(spine_buckets[team], f"spine {team}")
        planned.append(
            _PlannedShard(
                relative_path=f"{SCHEMA_3_SPINE_ROOT}/{team}.jsonl.gz",
                stream="spine",
                team=team,
                day=None,
                lines=ordered,
                line_ranges=ranges,
            )
        )

    planned.append(
        _PlannedShard(
            relative_path=SCHEMA_3_BINS_PATH,
            stream="bins",
            team=None,
            day=None,
            lines=_bins_lines(timeline, sole_team),
            line_ranges=_NO_LINE_RANGES,
        )
    )

    # Path safety is filesystem state rather than projection state, but it fails in exactly
    # the same way -- halfway through publication, with orphans behind it -- so it is checked
    # in the same pre-pass. `make_parents=False` because a refusal must not leave directories
    # it created behind either.
    for item in planned:
        _output_path(output, item.relative_path, make_parents=False)
    _output_path(output, SCHEMA_3_BOOTSTRAP_PATH, make_parents=False)

    # ---- publish ------------------------------------------------------------------------
    shards: list[ShardReport] = []
    changed = 0
    record_count = 0
    for item in planned:
        shard = _write_shard(
            output,
            item.relative_path,
            item.stream,
            item.team,
            item.day,
            item.lines,
            line_ranges=item.line_ranges,
            target_chunk_bytes=target_chunk_bytes,
        )
        shards.append(shard)
        changed += int(shard.write.data_changed) + int(shard.write.index_changed)
        record_count += shard.write.record_count

    bootstrap = _bootstrap(timeline, teams, shards, target_chunk_bytes)
    bootstrap_text = canonical_json(bootstrap)
    # Publication point: every shard and every sidecar above is on disk before the one stable
    # name that refers to them changes.
    changed += int(
        write_text_if_changed(
            _output_path(output, SCHEMA_3_BOOTSTRAP_PATH, make_parents=True), bootstrap_text
        )
    )

    generated: list[str] = [SCHEMA_3_BOOTSTRAP_PATH]
    for shard in shards:
        generated.extend(shard.generated_files)
    removed = _remove_unplanned(
        output,
        frozenset(
            relative for relative in generated if relative != SCHEMA_3_BOOTSTRAP_PATH
        ),
    )
    changed += len(removed)
    index_bytes = sum(
        _output_path(output, shard.index_relative_path, make_parents=False).stat().st_size
        for shard in shards
    )
    return TimelineV3Report(
        files_changed=changed,
        generated_files=tuple(sorted(generated)),
        timeline_shards=sum(1 for shard in shards if shard.stream == "timeline"),
        spine_shards=sum(1 for shard in shards if shard.stream == "spine"),
        record_count=record_count,
        bootstrap_bytes=len(bootstrap_text.encode("utf-8")),
        compressed_bytes=sum(shard.write.index.c_size for shard in shards),
        uncompressed_bytes=sum(shard.write.index.u_size for shard in shards),
        index_bytes=index_bytes,
        removed_files=removed,
    )


def _bootstrap(
    timeline: Mapping[str, JsonValue],
    teams: Sequence[Mapping[str, JsonValue]],
    shards: Sequence[ShardReport],
    target_chunk_bytes: int,
) -> dict[str, JsonValue]:
    """The entry point: everything needed to draw a first frame and locate every shard.

    ``teams`` is inlined -- 12 records and about 5 KB on the measured archive -- because the frame
    cannot be drawn without it and a second round trip to learn the names of the teams would be a
    round trip spent on nothing. ``activity_bins`` is not, for the reason in the module docstring:
    it is 811,708 bytes and grows with days times teams times resolutions.
    """

    root: dict[str, JsonValue] = {
        "schema_version": SCHEMA_3_VERSION,
        "kind": "timeline-v3-bootstrap",
    }
    for field in _BOOTSTRAP_FIELDS:
        if field in timeline:
            root[field] = timeline[field]
    root["codec"] = {
        "container": "multi-member-gzip",
        "level": GZIP_COMPRESSION_LEVEL,
        "target_chunk_bytes": target_chunk_bytes,
        "timestamp_key": SCHEMA_3_TIMESTAMP_KEY,
        "record_kind_key": SCHEMA_3_RECORD_KIND_KEY,
        "index_suffix": INDEX_SUFFIX,
    }
    root["teams"] = [dict(team) for team in teams]
    by_stream: dict[str, list[JsonValue]] = {"timeline": [], "spine": [], "bins": []}
    for shard in shards:
        by_stream[shard.stream].append(shard.catalog_obj())
    root["streams"] = {
        "timeline": {
            "addressing": "per-team-utc-day",
            "sort": "at_ms, record_kind, encoded line",
            "record_kinds": ["edge", "event", "phase"],
            "shards": by_stream["timeline"],
        },
        "spine": {
            "addressing": "per-team-line-range",
            "sort": "record_kind in declared order, then the kind's identifier",
            "record_kinds": list(_SPINE_KIND_ORDER),
            "shards": by_stream["spine"],
        },
        "bins": {
            "addressing": "single-shard-time-range",
            "sort": "at_ms, record_kind, encoded line",
            "record_kinds": ["activity_bin"],
            "shards": by_stream["bins"],
        },
    }
    return root


__all__ = [
    "SCHEMA_3_BINS_PATH",
    "SCHEMA_3_BOOTSTRAP_PATH",
    "SCHEMA_3_RECORD_KIND_KEY",
    "SCHEMA_3_ROOT",
    "SCHEMA_3_SPINE_ROOT",
    "SCHEMA_3_TIMELINE_ROOT",
    "SCHEMA_3_TIMESTAMP_KEY",
    "SCHEMA_3_VERSION",
    "ShardReport",
    "is_schema_3_path",
    "is_schema_3_shard_name",
    "TimelineV3Error",
    "TimelineV3Report",
    "write_timeline_v3",
    "utc_day_label",
    "utc_day_start",
]
