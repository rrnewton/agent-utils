"""Tests for the collection layer against a FakeHost: guard, selection, detectors, freshness."""

from __future__ import annotations

import pytest

from pr_landing_planner.collect import (
    CONFLICT_DETECTOR_FILE_OVERLAP,
    CONFLICT_DETECTOR_MERGE_TREE,
    CollectionError,
    collect_graph,
)
from pr_landing_planner.fakehost import FakeHost

_FIXTURE: dict[str, object] = {
    "repo": "OWNER/NAME",
    "base": "integration",
    "prs": [
        {
            "number": 1,
            "head_ref": "feat-1",
            "changed_files": ["src/shared.rs", "src/a.rs"],
            "commits_behind": 4,
            "checks": [{"name": "merge-gate", "conclusion": "SUCCESS"}],
        },
        {
            "number": 2,
            "head_ref": "feat-2",
            "changed_files": ["src/shared.rs", "src/b.rs"],
            "checks": [{"name": "merge-gate", "conclusion": "SUCCESS"}],
        },
        {
            "number": 3,
            "head_ref": "feat-3",
            "changed_files": ["src/c.rs"],
            "checks": [{"name": "merge-gate", "conclusion": "SUCCESS"}],
        },
    ],
    # #1 and #2 share src/shared.rs, but Git only truly CONFLICTS them here:
    "conflicts": [{"a": 1, "b": 2, "paths": ["src/shared.rs"]}],
}


def _host() -> FakeHost:
    host, _, _ = FakeHost.from_fixture(_FIXTURE)
    return host


def test_merge_tree_detector_finds_only_real_conflicts() -> None:
    graph = collect_graph(_host(), repo="OWNER/NAME", base="integration")
    assert [(e.a, e.b) for e in graph.conflict_edges] == [(1, 2)]
    # #1 and #2 also file-overlap; #3 overlaps nobody.
    assert [(e.a, e.b) for e in graph.overlap_edges] == [(1, 2)]
    behind = {n.number: n.commits_behind for n in graph.nodes}
    assert behind[1] == 4 and behind[2] == 0


def test_file_overlap_detector_is_more_conservative() -> None:
    graph = collect_graph(
        _host(), repo="OWNER/NAME", base="integration",
        conflict_detector=CONFLICT_DETECTOR_FILE_OVERLAP,
    )
    # Same set here because the only shared-file pair is also the only real conflict.
    assert [(e.a, e.b) for e in graph.conflict_edges] == [(1, 2)]


def test_conflict_detector_diverges_when_overlap_is_automergeable() -> None:
    fixture: dict[str, object] = {
        "repo": "R",
        "base": "integration",
        "prs": [
            {"number": 1, "head_ref": "a", "changed_files": ["shared.rs"]},
            {"number": 2, "head_ref": "b", "changed_files": ["shared.rs"]},
        ],
        # No `conflicts` entry => git merge-tree reports them auto-mergeable.
    }
    host, _, _ = FakeHost.from_fixture(fixture)
    mt = collect_graph(host, repo="R", base="integration", conflict_detector=CONFLICT_DETECTOR_MERGE_TREE)
    fo_host, _, _ = FakeHost.from_fixture(fixture)
    fo = collect_graph(fo_host, repo="R", base="integration", conflict_detector=CONFLICT_DETECTOR_FILE_OVERLAP)
    assert mt.conflict_edges == ()  # merge-tree: no real conflict
    assert [(e.a, e.b) for e in fo.conflict_edges] == [(1, 2)]  # file-overlap over-serializes


def test_content_identity_guard_aborts_on_drift() -> None:
    fixture: dict[str, object] = {
        "repo": "R",
        "base": "integration",
        "prs": [
            {
                "number": 1,
                "head_ref": "a",
                "api_head_sha": "AAA",
                "fetched_head_sha": "BBB",  # simulate the PR moving mid-collection
                "changed_files": ["a.rs"],
            }
        ],
    }
    host, _, _ = FakeHost.from_fixture(fixture)
    with pytest.raises(CollectionError, match="changed during collection"):
        collect_graph(host, repo="R", base="integration")


def test_only_numbers_restricts_selection() -> None:
    graph = collect_graph(
        _host(), repo="OWNER/NAME", base="integration", only=frozenset({1, 3})
    )
    assert {n.number for n in graph.nodes} == {1, 3}


def test_transitive_stack_selection_across_base() -> None:
    fixture: dict[str, object] = {
        "repo": "R",
        "base": "integration",
        "prs": [
            {"number": 10, "head_ref": "root", "base_ref": "integration"},
            {"number": 11, "head_ref": "child", "base_ref": "root"},  # stacked, targets root not base
        ],
    }
    host, _, _ = FakeHost.from_fixture(fixture)
    graph = collect_graph(host, repo="R", base="integration")
    # #11 is pulled in as part of #10's transitive stack even though it targets `root`.
    assert {n.number for n in graph.nodes} == {10, 11}
    assert any(e.before == 10 and e.after == 11 for e in graph.ordering_edges)


def test_unknown_detector_raises() -> None:
    with pytest.raises(ValueError, match="unknown conflict detector"):
        collect_graph(_host(), repo="R", base="integration", conflict_detector="bogus")
