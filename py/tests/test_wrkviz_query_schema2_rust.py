"""Differential: Rust `wrkviz query` over schema-2 archives vs the oracle.

The fixture is a real schema-2 generation: the Python schema-1
timeline is published through the retired writer (enabled only
for the fixture call, the way `tests.timeline_legacy_generations`
does for the Python reader), with its content-addressed global,
phase-index, detail and search objects. The schema-3 bootstrap
is removed so both editions must read schema 2; a second copy
also removes the schema-1 monolith, which only the schema-2
reader can open. `inspect` on that monolith-free copy is the one
intentional divergence: the Python CLI's inspect counts still
require the monolith and refuse, while the Rust fallback counts
the assembled schema-2 timeline the query reader opens (agents,
phases and rollups agree with the oracle's `TimelineQuery`).
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from wrkviz.archive import JsonValue

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkvizd"

DAY_MS = 24 * 60 * 60 * 1000
MIDNIGHT = 1777852800000
TEAMS = ("team-a", "team-b")


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _run(binary: Path, args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), *args],
        cwd=AU_ROOT, capture_output=True, text=True, timeout=300,
    )


def _oracle(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(PYTHON_WRKVIZ), *args],
        cwd=AU_ROOT, capture_output=True, text=True, timeout=300,
    )


def _check(
    rust_wrkviz: Path, args: list[str]
) -> subprocess.CompletedProcess[str]:
    oracle = _oracle(args)
    result = _run(rust_wrkviz, args)
    assert result.returncode == oracle.returncode, (args, result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout, args
    assert result.stderr == oracle.stderr, args
    return oracle


def _check_error(rust_wrkviz: Path, args: list[str]) -> None:
    oracle = _oracle(args)
    result = _run(rust_wrkviz, args)
    assert oracle.returncode == 2, oracle.stdout
    assert result.returncode == oracle.returncode, (args, result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout == ""
    assert result.stderr == oracle.stderr, (args, result.stderr, oracle.stderr)


def _agent(team: str, index: int) -> dict[str, JsonValue]:
    return {
        "id": f"{team}::agent-{index:02d}",
        "team": team,
        "parent_id": None if index == 0 else f"{team}::agent-00",
        "path": f"/{team}/agent-{index:02d}",
        "label": f"agent {index}",
        "short_name": f"agent-{index:02d}",
        "official_name": f"/{team}/agent-{index:02d}",
        "nickname": "",
        "naming_rationale": f"named {index}",
        "lifetime_summary": f"deadline summary agent {index} team {team}",
        "summary_available": True,
        "depth": 0 if index == 0 else 1,
        "start_ms": MIDNIGHT,
        "end_ms": MIDNIGHT + 2 * DAY_MS,
        "status": "done",
    }


def _phase(team: str, agent_index: int, day: int, index: int) -> dict[str, JsonValue]:
    start_ms = MIDNIGHT + day * DAY_MS + index * 3_600_000
    return {
        "id": f"{team}::phase-{day}-{index:02d}",
        "agent_id": f"{team}::agent-{agent_index:02d}",
        "team": team,
        "start_ms": start_ms,
        "end_ms": start_ms + 1_800_000,
        "phrase": f"deadline work {index}",
        "paragraph": f"deadline paragraph {index}",
        "summary_available": True,
        "detail_path": f"data/details/{team}/phase-{day}-{index:02d}.json",
        "stats": {"events": 3},
        "states": [],
        "artifact_ids": [],
        "output_artifact_ids": [],
    }


def _rollup(team: str, day: int) -> dict[str, JsonValue]:
    start_ms = MIDNIGHT + day * DAY_MS
    return {
        "kind": "daily",
        "team": team,
        "label": f"{team} day {day}",
        "path": f"teams/{team}/summaries/daily/day-{day}.md",
        "technical_path": f"teams/{team}/summaries/daily/day-{day}.md",
        "plain_language_path": f"teams/{team}/summaries/daily/day-{day}-plain.md",
        "technical_summary_available": True,
        "plain_language_summary_available": True,
        "summary_available": True,
        "start_ms": start_ms,
        "end_ms": start_ms + DAY_MS,
        "stats": {"events": 9},
    }


def _timeline(teams: tuple[str, ...]) -> dict[str, JsonValue]:
    agents: list[JsonValue] = []
    phases: list[JsonValue] = []
    rollups: list[JsonValue] = []
    events: list[JsonValue] = []
    bins: list[JsonValue] = []
    for team in teams:
        for index in range(3):
            agents.append(_agent(team, index))
        for day in range(2):
            rollups.append(_rollup(team, day))
            for index in range(3):
                phase = _phase(team, index % 3, day, index)
                phases.append(phase)
                start = phase["start_ms"]
                assert isinstance(start, int)
                events.append(
                    {
                        "agent_id": phase["agent_id"],
                        "team": team,
                        "at_ms": start + 1000,
                        "kind": "user_prompt",
                    }
                )
        for hour in range(2):
            bins.append(
                {
                    "team": team,
                    "resolution": "hourly",
                    "role": "coordinator",
                    "start_ms": MIDNIGHT + hour * 3_600_000,
                    "end_ms": MIDNIGHT + (hour + 1) * 3_600_000,
                    "peak_concurrency": 2,
                    "avg_active_concurrency": 0.5,
                }
            )
    return {
        "schema_version": 1,
        "generated_at": "2026-05-06T00:00:00Z",
        "source_digest": "0" * 64,
        "display_timezone": "America/New_York",
        "display_timezone_source": "explicit",
        "range": {"start_ms": MIDNIGHT, "end_ms": MIDNIGHT + 2 * DAY_MS},
        "stats": {"events": len(events)},
        "artifact_catalog_path": "data/artifacts.json",
        "glossary_path": "",
        "teams": [
            {
                "slug": team,
                "label": team,
                "provider": "codex",
                "projects": [],
                "hosts": [],
                "stats": {"agents": 3},
            }
            for team in teams
        ],
        "agents": agents,
        "phases": phases,
        "edges": [],
        "events": events,
        "rollups": rollups,
        "projects": [],
        "summary_files": [
            {
                "kind": "daily",
                "team": team,
                "label": f"{team} daily",
                "path": f"teams/{team}/summaries/daily/day-0.md",
                "period": "day-0",
            }
            for team in teams
        ],
        "glossary": [],
        "activity_bins": bins,
        "project_overviews": [
            {
                "team": team,
                "text": "An overview mentioning the deadline.",
                "summary_available": True,
                "evidence_status": "supported",
                "model": "a-model",
                "prompt_version": "v1",
                "input_hash": "1" * 64,
            }
            for team in teams
        ],
    }


def _search_records(teams: tuple[str, ...]) -> list[dict[str, JsonValue]]:
    records: list[dict[str, JsonValue]] = []
    for team in teams:
        records.append(
            {
                "schema_version": 1,
                "ref": f"message:{team}::evt-prompt-0",
                "record_type": "prompt",
                "event_id": "evt-prompt-0",
                "team": team,
                "agent_id": f"{team}::agent-00",
                "agent_ref": f"agent:{team}::agent-00",
                "agent_path": f"/{team}/agent-00",
                "role": "user",
                "text": "please fix the deadline bug",
                "at_ms": MIDNIGHT + 1000,
                "prompt_ref": None,
                "prompt_at_ms": None,
                "prompt_in_scope": None,
                "author_kind": "owner_human",
                "prompt_author_kind": "owner_human",
                "content_fidelity": "verbatim",
            }
        )
        records.append(
            {
                "schema_version": 1,
                "ref": f"message:{team}::evt-resp-0",
                "record_type": "response",
                "event_id": "evt-resp-0",
                "team": team,
                "agent_id": f"{team}::agent-00",
                "agent_ref": f"agent:{team}::agent-00",
                "agent_path": f"/{team}/agent-00",
                "role": "assistant",
                "text": "the deadline fix is ready",
                "at_ms": MIDNIGHT + 2000,
                "prompt_ref": f"message:{team}::evt-prompt-0",
                "prompt_at_ms": MIDNIGHT + 1000,
                "prompt_in_scope": True,
                "author_kind": None,
                "prompt_author_kind": "owner_human",
                "content_fidelity": "verbatim",
            }
        )
    return records


def _publish(root: Path, teams: tuple[str, ...] = TEAMS) -> None:
    sys.path.insert(0, str(AU_ROOT / "py"))
    sys.path.insert(0, str(AU_ROOT / "py" / "tests"))
    from wrkviz.archive import canonical_json
    from wrkviz.timeline_shards import write_timeline_shards
    from wrkviz.timeline_v3 import write_timeline_v3

    from tests.timeline_legacy_generations import schema_2_writer_enabled

    timeline = _timeline(teams)
    (root / "data").mkdir(parents=True, exist_ok=True)
    (root / "data" / "timeline.json").write_text(
        canonical_json(timeline), encoding="utf-8"
    )
    phases = timeline["phases"]
    assert isinstance(phases, list)
    for raw in phases:
        assert isinstance(raw, dict)
        relative = raw["detail_path"]
        assert isinstance(relative, str)
        detail = root / relative
        detail.parent.mkdir(parents=True, exist_ok=True)
        detail.write_text(
            json.dumps(
                {
                    "phase_id": raw["id"],
                    "transcript": [
                        {"at_ms": raw["start_ms"], "role": "user", "text": "deadline?"}
                    ],
                },
                sort_keys=True,
            ),
            encoding="utf-8",
        )
    rollups = timeline["rollups"]
    assert isinstance(rollups, list)
    for raw in rollups:
        assert isinstance(raw, dict)
        for key in ("technical_path", "plain_language_path"):
            relative = raw[key]
            assert isinstance(relative, str)
            markdown = root / relative
            markdown.parent.mkdir(parents=True, exist_ok=True)
            markdown.write_text(
                f"# {relative}\n\nthe deadline moved.\n", encoding="utf-8"
            )
    with schema_2_writer_enabled():
        write_timeline_shards(root, timeline, search_records=_search_records(teams))
    write_timeline_v3(root, timeline)


def _schema2_copy(source: Path, dest: Path, *, monolith: bool) -> Path:
    shutil.copytree(source, dest)
    (dest / "data" / "timeline-v3.json").unlink()
    shutil.rmtree(dest / "data" / "timeline-v3")
    if not monolith:
        (dest / "data" / "timeline.json").unlink()
    return dest


@pytest.fixture(scope="module")
def published(tmp_path_factory: pytest.TempPathFactory) -> Path:
    base = tmp_path_factory.mktemp("wrkviz-schema2")
    root = base / "published"
    root.mkdir()
    _publish(root)
    return root


@pytest.fixture(scope="module")
def archive(published: Path, tmp_path_factory: pytest.TempPathFactory) -> Path:
    base = tmp_path_factory.mktemp("wrkviz-schema2-archive")
    return _schema2_copy(published, base / "archive", monolith=True)


@pytest.fixture(scope="module")
def archive_only(
    published: Path, tmp_path_factory: pytest.TempPathFactory
) -> Path:
    base = tmp_path_factory.mktemp("wrkviz-schema2-only")
    return _schema2_copy(published, base / "archive", monolith=False)


def test_list_schema2_matches_python(
    rust_wrkviz: Path, archive: Path
) -> None:
    for resource in ("teams", "agents", "phases", "rollups"):
        oracle = _check(
            rust_wrkviz, ["query", "--output", str(archive), "list", resource]
        )
        payload = json.loads(oracle.stdout)
        assert payload["command"] == f"list {resource}"
        assert payload["count"] == len(payload["items"])
        assert payload["count"] > 0
    for fmt in ("jsonl", "markdown", "text"):
        _check(
            rust_wrkviz,
            ["query", "--output", str(archive), "--format", fmt, "list", "agents"],
        )
    for extra in (
        ["list", "teams", "--team", "team-a"],
        ["list", "agents", "--agent", "agent:team-a::agent-00"],
        ["list", "phases", "--agent", "agent:team-a::agent-00"],
        ["list", "rollups", "--kind", "daily"],
        ["list", "agents", "--start-time", "2026-05-04T00:00:00Z",
         "--end-time", "2026-05-06T00:00:00Z"],
    ):
        _check(rust_wrkviz, ["query", "--output", str(archive), *extra])


def test_show_schema2_matches_python(
    rust_wrkviz: Path, archive: Path
) -> None:
    for reference in (
        "team:team-a",
        "agent:team-a::agent-00",
        "agent:team-b::agent-01",
        "phase:team-b::phase-1-02",
        f"rollup:team-a::daily::{MIDNIGHT}",
        "message:team-a::evt-prompt-0",
        "message:team-a::evt-resp-0",
    ):
        _check(
            rust_wrkviz, ["query", "--output", str(archive), "show", reference]
        )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive), "show",
         "phase:team-b::phase-1-02", "--transcript"],
    )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive), "--format", "markdown",
         "show", "agent:team-a::agent-00"],
    )


def test_search_schema2_matches_python(
    rust_wrkviz: Path, archive: Path
) -> None:
    for extra in (
        ["--in", "all-transcript"],
        ["--in", "owner-prompts"],
        ["--in", "agent-responses"],
        ["--in", "all-transcript", "--sort", "newest"],
        ["--in", "all-transcript", "--sort", "oldest"],
        ["--in", "all-transcript", "--match", "phrase"],
        ["--in", "all-transcript", "--case-sensitive"],
        ["--in", "all-transcript", "--limit", "1", "--offset", "1"],
        ["--in", "all-transcript", "--prompt-author", "owner"],
        ["--in", "all-transcript", "--linkage", "linked"],
        ["--in", "all-transcript", "--linkage", "unlinked"],
        ["--in", "all-transcript", "--role", "user"],
        ["--in", "all-transcript", "--team", "team-a"],
    ):
        _check(
            rust_wrkviz,
            ["query", "--output", str(archive), "search", *extra, "deadline"],
        )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive), "search", "--in",
         "all-transcript", "zzzznotfoundterm"],
    )
    _check(rust_wrkviz, ["query", "--output", str(archive), "search", "deadline"])
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive), "search", "--scope",
         "transcripts", "deadline", "--limit", "3"],
    )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive), "search", "--scope",
         "all", "deadline", "--limit", "3"],
    )
    for fmt in ("jsonl", "markdown", "text"):
        _check(
            rust_wrkviz,
            ["query", "--output", str(archive), "--format", fmt,
             "search", "--in", "all-transcript", "deadline", "--limit", "2"],
        )


def test_schema2_only_archive_matches_python(
    rust_wrkviz: Path, archive_only: Path
) -> None:
    # No monolith: only the schema-2 reader can answer at all.
    assert not (archive_only / "data" / "timeline.json").exists()
    for resource in ("teams", "agents", "phases", "rollups"):
        _check(
            rust_wrkviz,
            ["query", "--output", str(archive_only), "list", resource],
        )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive_only), "show",
         "agent:team-a::agent-00"],
    )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive_only), "search", "--in",
         "all-transcript", "deadline"],
    )


def test_inspect_schema2_matches_python(
    rust_wrkviz: Path, archive: Path, archive_only: Path
) -> None:
    oracle = _check(rust_wrkviz, ["inspect", "--output", str(archive)])
    payload = json.loads(oracle.stdout)
    assert payload["agents"] == 6
    assert payload["phases"] == 12
    # Without the monolith the Python CLI refuses; the Rust
    # fallback counts the assembled schema-2 timeline instead.
    refused = _oracle(["inspect", "--output", str(archive_only)])
    assert refused.returncode == 2
    result = _run(rust_wrkviz, ["inspect", "--output", str(archive_only)])
    assert result.returncode == 0, result.stderr
    counts = json.loads(result.stdout)
    assert counts["agents"] == 6
    assert counts["phases"] == 12
    assert counts["rollups"] == 4
    assert counts["summary_files"] == 2


def test_generations_answer_the_same(
    rust_wrkviz: Path, published: Path, tmp_path: Path
) -> None:
    three = tmp_path / "schema-3"
    shutil.copytree(published, three)
    two = _schema2_copy(published, tmp_path / "schema-2", monolith=True)
    one = _schema2_copy(published, tmp_path / "schema-1", monolith=True)
    (one / "data" / "timeline-v2.json").unlink()
    shutil.rmtree(one / "data" / "timeline-v2")
    for root in (three, two, one):
        for args in (
            ["list", "teams"],
            ["list", "agents"],
            ["list", "phases"],
            ["list", "rollups"],
            ["show", "agent:team-a::agent-00"],
            ["show", "phase:team-b::phase-1-02"],
            ["search", "--in", "all-transcript", "deadline"],
            ["search", "deadline"],
        ):
            _check(rust_wrkviz, ["query", "--output", str(root), *args])


def test_schema2_loader_refusals_match_python(
    rust_wrkviz: Path, archive: Path, tmp_path: Path
) -> None:
    # Content-addressed object tamper: digest mismatch.
    digest = tmp_path / "digest"
    shutil.copytree(archive, digest)
    bootstrap = json.loads((digest / "data" / "timeline-v2.json").read_text())
    global_ref = bootstrap["global"]
    assert isinstance(global_ref, dict)
    relative = global_ref["url"]
    assert isinstance(relative, str)
    target = digest / relative
    blob = bytearray(target.read_bytes())
    blob[10] ^= 1
    target.write_bytes(bytes(blob))
    _check_error(
        rust_wrkviz, ["query", "--output", str(digest), "list", "agents"]
    )

    # Bootstrap that is not a regular file.
    bootdir = tmp_path / "bootdir"
    shutil.copytree(archive, bootdir)
    (bootdir / "data" / "timeline-v2.json").unlink()
    (bootdir / "data" / "timeline-v2.json").mkdir()
    _check_error(
        rust_wrkviz, ["query", "--output", str(bootdir), "list", "agents"]
    )

    # Unsupported bootstrap version.
    version = tmp_path / "version"
    shutil.copytree(archive, version)
    doc = json.loads((version / "data" / "timeline-v2.json").read_text())
    doc["schema_version"] = 9
    (version / "data" / "timeline-v2.json").write_text(json.dumps(doc))
    _check_error(
        rust_wrkviz, ["query", "--output", str(version), "list", "agents"]
    )

    # A bootstrap predating the phase index falls back to schema 1.
    predates = tmp_path / "predates"
    shutil.copytree(archive, predates)
    doc = json.loads((predates / "data" / "timeline-v2.json").read_text())
    del doc["phase_index"]
    (predates / "data" / "timeline-v2.json").write_text(json.dumps(doc))
    _check(
        rust_wrkviz, ["query", "--output", str(predates), "list", "agents"]
    )
    (predates / "data" / "timeline.json").unlink()
    _check_error(
        rust_wrkviz, ["query", "--output", str(predates), "list", "agents"]
    )


def test_schema2_sole_team_stamping_matches_python(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    root = tmp_path / "single"
    root.mkdir()
    _publish(root, teams=("team-a",))
    archive = _schema2_copy(root, tmp_path / "single-schema2", monolith=True)
    # Strip `team` from the global agents/rollups/overviews: the
    # single-team loader must stamp the sole team back on.
    bootstrap_path = archive / "data" / "timeline-v2.json"
    bootstrap = json.loads(bootstrap_path.read_text())
    global_ref = bootstrap["global"]
    assert isinstance(global_ref, dict)
    relative = global_ref["url"]
    assert isinstance(relative, str)
    global_doc = json.loads((archive / relative).read_text())
    for key in ("agents", "rollups", "project_overviews"):
        for record in global_doc.get(key, []):
            record.pop("team", None)
    encoded = json.dumps(global_doc, sort_keys=True).encode()
    digest = hashlib.sha256(encoded).hexdigest()
    new_relative = f"data/timeline-v2/objects/{digest}.json"
    (archive / new_relative).write_bytes(encoded)
    global_ref["sha256"] = digest
    global_ref["url"] = new_relative
    global_ref["bytes"] = len(encoded)
    bootstrap_path.write_text(json.dumps(bootstrap))
    for resource in ("agents", "rollups"):
        _check(
            rust_wrkviz, ["query", "--output", str(archive), "list", resource]
        )
    _check(
        rust_wrkviz,
        ["query", "--output", str(archive), "show", "agent:team-a::agent-00"],
    )
