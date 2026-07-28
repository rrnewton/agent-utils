"""Tests for the pure graph algorithms: edges, transitive reduction, stacks, held, partition."""

from __future__ import annotations

from pr_landing_planner.graph import (
    build_conflict_edges_file_overlap,
    build_ordering_edges_base_ref,
    build_overlap_edges,
    build_stacks,
    dedupe_ordering,
    held_reasons,
    partition_parallel_safe,
    transitive_reduce,
)
from pr_landing_planner.model import ConflictEdge, OrderingEdge, PrNode


def _node(
    number: int,
    *,
    head_ref: str = "",
    base_ref: str = "integration",
    files: frozenset[str] = frozenset(),
    draft: bool = False,
    base_conflict: tuple[str, ...] = (),
    mergeable: str = "",
    priority: int = 0,
    size: int = 0,
    created_at: str = "",
) -> PrNode:
    return PrNode(
        number=number,
        head_ref=head_ref or f"feat-{number}",
        base_ref=base_ref,
        head_sha=f"sha-{number}",
        base_sha="base",
        files=files,
        is_draft=draft,
        base_conflict_paths=base_conflict,
        mergeable=mergeable,
        priority=priority,
        additions=size,
        created_at=created_at,
    )


def test_overlap_and_file_overlap_conflict_edges() -> None:
    a = _node(1, files=frozenset({"x.rs", "y.rs"}))
    b = _node(2, files=frozenset({"y.rs"}))
    c = _node(3, files=frozenset({"z.rs"}))
    overlaps = build_overlap_edges([a, b, c])
    assert overlaps == (type(overlaps[0])(1, 2, ("y.rs",)),)
    conflicts = build_conflict_edges_file_overlap([a, b, c])
    assert [(e.a, e.b) for e in conflicts] == [(1, 2)]


def test_base_ref_ordering_edges() -> None:
    root = _node(1, head_ref="feat-root")
    stacked = _node(2, head_ref="feat-child", base_ref="feat-root")
    edges = build_ordering_edges_base_ref([root, stacked])
    assert edges == (OrderingEdge(1, 2, "base-ref"),)


def test_transitive_reduce_drops_implied_edge() -> None:
    edges = [OrderingEdge(1, 2, "x"), OrderingEdge(2, 3, "x"), OrderingEdge(1, 3, "x")]
    reduced = transitive_reduce(edges)
    assert OrderingEdge(1, 3, "x") not in reduced
    assert OrderingEdge(1, 2, "x") in reduced and OrderingEdge(2, 3, "x") in reduced


def test_build_stacks() -> None:
    edges = [OrderingEdge(1, 2, "x"), OrderingEdge(2, 3, "x")]
    assert build_stacks(edges) == ((1, 2, 3),)


def test_dedupe_ordering_keeps_first_reason_sorted() -> None:
    edges = [OrderingEdge(2, 3, "ancestry"), OrderingEdge(1, 2, "base-ref"), OrderingEdge(2, 3, "base-ref")]
    deduped = dedupe_ordering(edges)
    assert [(e.before, e.after) for e in deduped] == [(1, 2), (2, 3)]
    assert deduped[1].reason == "ancestry"  # first seen wins


def test_held_reasons_base_draft_and_transitive() -> None:
    draft = _node(1, draft=True)
    conflicted = _node(2, base_conflict=("f.rs",))
    gh_conflict = _node(3, mergeable="CONFLICTING")
    dependent = _node(4, head_ref="feat-4", base_ref="feat-1")  # stacked on the draft
    ordering = build_ordering_edges_base_ref([draft, conflicted, gh_conflict, dependent])
    held = held_reasons([draft, conflicted, gh_conflict, dependent], ordering)
    by = {h.pr: h.reasons for h in held}
    assert "draft" in by[1]
    assert "local-base-conflict" in by[2]
    assert "github-base-conflicting" in by[3]
    assert by[4] == ("depends-on-held:#1",)


def test_partition_respects_conflicts_and_ordering() -> None:
    a = _node(1)
    b = _node(2)
    c = _node(3)
    conflicts = [ConflictEdge(1, 2, ("x.rs",))]
    ordering = [OrderingEdge(1, 3, "ancestry")]  # 3 must follow 1
    groups = partition_parallel_safe([a, b, c], conflicts, ordering)
    # 1 and 2 conflict => different groups; 3 follows 1 => not in group 0.
    assert 1 in groups[0]
    assert 2 not in groups[0]
    g_of = {n: i for i, g in enumerate(groups) for n in g}
    assert g_of[3] > g_of[1]
    assert g_of[2] > 0 or g_of[2] != g_of[1]


def test_partition_ranks_by_priority_then_size() -> None:
    # Two non-conflicting PRs land in one group; order within is by (priority, size, age, number).
    hi = _node(5, priority=0, size=100)
    lo = _node(6, priority=1, size=1)
    groups = partition_parallel_safe([lo, hi], [], [])
    assert groups == ((5, 6),)  # priority 0 (#5) ranked before priority 1 (#6)


def test_partition_excludes_held() -> None:
    a = _node(1)
    b = _node(2)
    groups = partition_parallel_safe([a, b], [], [], exclude=frozenset({2}))
    assert groups == ((1,),)
