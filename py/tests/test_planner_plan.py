"""Tests for the pure fusion: per-PR actions, ordering, freshness, batch, diagnostics."""

from __future__ import annotations

from collections.abc import Sequence

from pr_landing_planner.model import (
    CiState,
    CiVerdict,
    ConflictEdge,
    HeldPr,
    OrderingEdge,
    PrAction,
    PrActionDecision,
    PrNode,
    RedClass,
)
from pr_landing_planner.plan import compute_plan


def _verdict(state: CiState, red: RedClass | None = None, gate_ok: bool = True) -> CiVerdict:
    return CiVerdict(state, red, gate_present=True, gate_ok=gate_ok, gate_missing_run=False)


def _node(
    number: int,
    *,
    state: CiState = CiState.GREEN,
    red: RedClass | None = None,
    behind: int = 0,
    files: frozenset[str] = frozenset(),
    priority: int = 0,
    size: int = 0,
) -> PrNode:
    return PrNode(
        number=number,
        head_ref=f"feat-{number}",
        base_ref="integration",
        head_sha=f"sha-{number}",
        base_sha="base",
        files=files,
        commits_behind=behind,
        ci=_verdict(state, red, gate_ok=state is CiState.PASSED),
        priority=priority,
        additions=size,
    )


def _action(decisions: Sequence[PrActionDecision], pr: int) -> PrAction:
    for d in decisions:
        if d.pr == pr:
            return d.action
    raise AssertionError(f"no decision for #{pr}")


def test_fusion_table_actions() -> None:
    nodes = [
        _node(1, state=CiState.GREEN),
        _node(2, state=CiState.GREEN, behind=5),
        _node(3, state=CiState.RED, red=RedClass.STALE_REQUIRED_CHECK),
        _node(4, state=CiState.RED, red=RedClass.FLAKY),
        _node(5, state=CiState.RED, red=RedClass.REAL),
        _node(6, state=CiState.RED, red=RedClass.EVALUATE_ONCE_RACE),
        _node(7, state=CiState.RED, red=RedClass.RUNNER_OUTAGE),
        _node(8, state=CiState.NO_RESULT),
        _node(9, state=CiState.NO_RESULT),
    ]
    plan, _ = compute_plan(nodes, [], [], [])
    acts = plan.per_pr_actions
    assert _action(acts, 1) is PrAction.LAND_NOW
    assert _action(acts, 2) is PrAction.REBASE_THEN_LAND
    assert _action(acts, 3) is PrAction.REFIRE_STALE_GATE
    assert _action(acts, 4) is PrAction.REFIRE_CI
    assert _action(acts, 5) is PrAction.HOLD_FIX
    assert _action(acts, 6) is PrAction.WAIT
    assert _action(acts, 7) is PrAction.ESCALATE_RUNNER_OUTAGE
    assert _action(acts, 8) is PrAction.REFIRE_CI
    assert _action(acts, 9) is PrAction.REFIRE_CI


def test_freshness_threshold() -> None:
    nodes = [_node(1, behind=3)]
    plan_strict, _ = compute_plan(nodes, [], [], [], freshness_max_behind=0)
    assert _action(plan_strict.per_pr_actions, 1) is PrAction.REBASE_THEN_LAND
    plan_loose, _ = compute_plan(nodes, [], [], [], freshness_max_behind=5)
    assert _action(plan_loose.per_pr_actions, 1) is PrAction.LAND_NOW


def test_held_actions() -> None:
    nodes = [_node(1), _node(2), _node(3)]
    held = [
        HeldPr(1, ("draft",)),
        HeldPr(2, ("local-base-conflict",)),
        HeldPr(3, ("depends-on-held:#1",)),
    ]
    plan, _ = compute_plan(nodes, [], [], held)
    assert _action(plan.per_pr_actions, 1) is PrAction.WAIT
    assert _action(plan.per_pr_actions, 2) is PrAction.REBASE_THEN_LAND
    assert _action(plan.per_pr_actions, 3) is PrAction.WAIT
    # Held PRs never appear in land_now or the parallel-safe groups.
    assert plan.land_now == ()
    assert all(1 not in g and 2 not in g and 3 not in g for g in plan.parallel_safe_groups)


def test_land_now_and_order_priority() -> None:
    nodes = [
        _node(1, priority=1, size=5),
        _node(2, priority=0, size=50),
        _node(3, priority=0, size=1),
    ]
    plan, _ = compute_plan(nodes, [], [], [])
    assert set(plan.land_now) == {1, 2, 3}
    # order ranks priority asc, then size asc: #3 (p0,sz1), #2 (p0,sz50), #1 (p1,sz5)
    assert plan.order == (3, 2, 1)


def test_batch_is_conflict_free_green_subset() -> None:
    nodes = [_node(1, files=frozenset({"x"})), _node(2, files=frozenset({"x"})), _node(3)]
    conflicts = [ConflictEdge(1, 2, ("x",))]
    plan, _ = compute_plan(nodes, conflicts, [], [], batch=True)
    # #1 and #2 conflict => the batch keeps only one of them plus #3.
    assert 3 in plan.batch
    assert not (1 in plan.batch and 2 in plan.batch)


def test_diagnostics_and_outage_threshold() -> None:
    nodes = [
        _node(1, state=CiState.RED, red=RedClass.RUNNER_OUTAGE),
        _node(2, state=CiState.RED, red=RedClass.RUNNER_OUTAGE),
        _node(3, state=CiState.RED, red=RedClass.STALE_REQUIRED_CHECK),
        _node(4, state=CiState.RED, red=RedClass.FLAKY),
        _node(5, state=CiState.RED, red=RedClass.REAL),
        _node(6, state=CiState.RED, red=RedClass.EVALUATE_ONCE_RACE),
    ]
    _, diag = compute_plan(nodes, [], [], [], outage_min_prs=2)
    assert diag.outage_prs == (1, 2)
    assert diag.outage_suspected is True
    assert diag.stale_gates == (3,)
    assert diag.flaky_reds == (4,)
    assert diag.real_reds == (5,)
    assert diag.evaluate_once_race == (6,)
    # One outage PR below the threshold is not "systemic".
    _, diag_one = compute_plan(nodes[1:], [], [], [], outage_min_prs=2)
    assert diag_one.outage_suspected is False
