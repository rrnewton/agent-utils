"""PURE graph algorithms over pre-collected PRs and edges — no network, no git, no I/O.

The collection layer (:mod:`pr_landing_planner.collect`, which drives a
:class:`pr_landing_planner.host.VcsHost`) produces the nodes and the raw conflict / overlap /
ordering edges. Everything in THIS module is a deterministic function of that data, so it is directly
unit-testable and is the byte-stable target a future Rust port would cross-check:

* file-overlap edges + the file-overlap conflict FALLBACK (used when merge-tree is disabled),
* base-ref (explicit stacking) ordering edges,
* transitive reduction of the ordering DAG + stack extraction,
* held-PR reasoning (draft / base-conflict / depends-on-held, propagated transitively),
* the parallel-safe partition: a greedy independent-set layering over the real conflict graph that
  respects ordering edges (our affordable, static-analysis version of bors batching / Zuul parallel
  states).
"""

from __future__ import annotations

from collections.abc import Sequence

from pr_landing_planner.model import (
    ConflictEdge,
    HeldPr,
    OrderingEdge,
    OverlapEdge,
    PrNode,
)


# --------------------------------------------------------------------------- file-based edges
def build_overlap_edges(nodes: Sequence[PrNode]) -> tuple[OverlapEdge, ...]:
    """Undirected file-overlap edges: two PRs whose changed-file sets intersect (semantic risk)."""
    edges: list[OverlapEdge] = []
    for i, a in enumerate(nodes):
        for b in nodes[i + 1 :]:
            shared = tuple(sorted(a.files & b.files))
            if shared:
                edges.append(OverlapEdge(a.number, b.number, shared))
    return tuple(edges)


def build_conflict_edges_file_overlap(nodes: Sequence[PrNode]) -> tuple[ConflictEdge, ...]:
    """Fast, conservative conflict FALLBACK: treat any shared-file pair as a conflict.

    Used when ``--conflict-detector file-overlap`` is selected (or merge-tree is unavailable). This
    over-serializes PRs that share a file but do not truly collide — which is exactly why the default
    detector is the real ``git merge-tree`` probe in :mod:`pr_landing_planner.collect`."""
    edges: list[ConflictEdge] = []
    for i, a in enumerate(nodes):
        for b in nodes[i + 1 :]:
            shared = tuple(sorted(a.files & b.files))
            if shared:
                edges.append(ConflictEdge(a.number, b.number, shared))
    return tuple(edges)


def build_ordering_edges_base_ref(nodes: Sequence[PrNode]) -> tuple[OrderingEdge, ...]:
    """Explicit-stacking ordering edges: PR B's base branch IS PR A's head branch => A before B."""
    by_head_ref = {n.head_ref: n for n in nodes}
    edges: list[OrderingEdge] = []
    for node in nodes:
        predecessor = by_head_ref.get(node.base_ref)
        if predecessor is not None and predecessor.number != node.number:
            edges.append(OrderingEdge(predecessor.number, node.number, "base-ref"))
    return tuple(edges)


def dedupe_ordering(edges: Sequence[OrderingEdge]) -> tuple[OrderingEdge, ...]:
    """De-duplicate ordering edges by (before, after), keeping the first reason seen, sorted."""
    seen: dict[tuple[int, int], OrderingEdge] = {}
    for edge in edges:
        seen.setdefault((edge.before, edge.after), edge)
    return tuple(sorted(seen.values(), key=lambda e: (e.before, e.after)))


# --------------------------------------------------------------------------- transitive reduction
def _has_path(
    adjacency: dict[int, set[int]], start: int, target: int, skip: tuple[int, int]
) -> bool:
    pending = [start]
    seen: set[int] = set()
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        for child in adjacency.get(current, set()):
            if (current, child) == skip:
                continue
            if child == target:
                return True
            pending.append(child)
    return False


def transitive_reduce(edges: Sequence[OrderingEdge]) -> tuple[OrderingEdge, ...]:
    """Drop any ordering edge that is implied by a longer path (transitive reduction of the DAG)."""
    adjacency: dict[int, set[int]] = {}
    for edge in edges:
        adjacency.setdefault(edge.before, set()).add(edge.after)
    return tuple(
        edge
        for edge in edges
        if not _has_path(adjacency, edge.before, edge.after, (edge.before, edge.after))
    )


def build_stacks(edges: Sequence[OrderingEdge]) -> tuple[tuple[int, ...], ...]:
    """Root-to-leaf dependency chains (length >= 2) over the transitively-reduced ordering DAG."""
    reduced = transitive_reduce(edges)
    children: dict[int, set[int]] = {}
    parents: dict[int, set[int]] = {}
    involved: set[int] = set()
    for edge in reduced:
        children.setdefault(edge.before, set()).add(edge.after)
        parents.setdefault(edge.after, set()).add(edge.before)
        involved.update((edge.before, edge.after))
    roots = sorted(node for node in involved if not parents.get(node))
    stacks: list[tuple[int, ...]] = []

    def visit(node: int, path: tuple[int, ...]) -> None:
        next_nodes = sorted(children.get(node, set()))
        if not next_nodes:
            if len(path) > 1:
                stacks.append(path)
            return
        for child in next_nodes:
            if child in path:
                continue
            visit(child, (*path, child))

    for root in roots:
        visit(root, (root,))
    return tuple(stacks)


# --------------------------------------------------------------------------- held reasoning
def held_reasons(
    nodes: Sequence[PrNode], ordering_edges: Sequence[OrderingEdge]
) -> tuple[HeldPr, ...]:
    """Compute per-PR hold reasons, propagating ``depends-on-held`` transitively up the ordering DAG.

    Base reasons: ``draft`` (WIP), ``local-base-conflict`` (a real merge-tree conflict with the base),
    ``github-base-conflicting`` (GitHub's own ``mergeable == CONFLICTING``). Then any PR ordered after
    a held PR inherits ``depends-on-held:#N`` until fixpoint."""
    reasons: dict[int, list[str]] = {}
    for node in nodes:
        node_reasons: list[str] = []
        if node.is_draft:
            node_reasons.append("draft")
        if node.base_conflict_paths:
            node_reasons.append("local-base-conflict")
        if node.mergeable == "CONFLICTING":
            node_reasons.append("github-base-conflicting")
        if node_reasons:
            reasons[node.number] = node_reasons

    changed = True
    while changed:
        changed = False
        for edge in ordering_edges:
            if edge.before in reasons and edge.after not in reasons:
                reasons[edge.after] = [f"depends-on-held:#{edge.before}"]
                changed = True

    return tuple(
        HeldPr(pr=number, reasons=tuple(reasons[number])) for number in sorted(reasons)
    )


# --------------------------------------------------------------------------- parallel-safe partition
def partition_parallel_safe(
    nodes: Sequence[PrNode],
    conflict_edges: Sequence[ConflictEdge],
    ordering_edges: Sequence[OrderingEdge],
    exclude: frozenset[int] = frozenset(),
) -> tuple[tuple[int, ...], ...]:
    """Greedy layering into parallel-safe groups (independent sets in the conflict graph).

    A PR joins the earliest group where (a) every ordering-predecessor is already placed in a
    strictly-earlier group and (b) it does not conflict with any PR already placed in that group.
    Within a layer, PRs are considered in the deterministic land priority: priority (lower first),
    then diff size, then age (``created_at``), then PR number — so the emitted order is stable and
    matches the fusion table's "priority -> size -> age" ranking. ``exclude`` drops held PRs.
    """
    active = [n for n in nodes if n.number not in exclude]
    numbers = {n.number for n in active}
    by_number = {n.number: n for n in active}

    conflicts: dict[int, set[int]] = {n: set() for n in numbers}
    for edge in conflict_edges:
        if edge.a in numbers and edge.b in numbers:
            conflicts[edge.a].add(edge.b)
            conflicts[edge.b].add(edge.a)

    predecessors: dict[int, set[int]] = {n: set() for n in numbers}
    for oedge in ordering_edges:
        if oedge.before in numbers and oedge.after in numbers:
            predecessors[oedge.after].add(oedge.before)

    def rank(number: int) -> tuple[int, int, str, int]:
        node = by_number[number]
        return (node.priority, node.size, node.created_at, node.number)

    remaining = set(numbers)
    placed: set[int] = set()
    groups: list[tuple[int, ...]] = []
    while remaining:
        ready = sorted(
            (n for n in remaining if predecessors[n].issubset(placed)), key=rank
        )
        if not ready:
            # An ordering cycle among the rest: emit each remaining PR as its own singleton group
            # (deterministically), so we never loop forever.
            for number in sorted(remaining, key=rank):
                groups.append((number,))
            break
        group: list[int] = []
        for number in ready:
            if all(peer not in conflicts[number] for peer in group):
                group.append(number)
        groups.append(tuple(group))
        remaining.difference_update(group)
        placed.update(group)
    return tuple(groups)


__all__ = [
    "build_overlap_edges",
    "build_conflict_edges_file_overlap",
    "build_ordering_edges_base_ref",
    "dedupe_ordering",
    "transitive_reduce",
    "build_stacks",
    "held_reasons",
    "partition_parallel_safe",
]
