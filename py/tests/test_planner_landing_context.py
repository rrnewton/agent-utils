"""Landing-context evidence, policy, assignment, and mechanism-overlap tests."""

from __future__ import annotations

import json

import pytest

from pr_landing_planner.emit import render_json
from pr_landing_planner.landing_context import (
    apply_landing_context,
    parse_landing_context,
)
from pr_landing_planner.graph import build_mechanism_edges
from pr_landing_planner.model import (
    CiState,
    CiVerdict,
    CollectedGraph,
    PolicyClass,
    PrAction,
    PrNode,
    RedClass,
    ValidationEvidence,
)
from pr_landing_planner.plan import assemble_result, compute_plan


def _node(
    number: int,
    *,
    labels: tuple[str, ...] = (),
    state: CiState = CiState.PENDING,
    red: RedClass | None = None,
) -> PrNode:
    return PrNode(
        number=number,
        head_ref=f"feature-{number}",
        base_ref="main",
        head_sha=f"sha-{number}",
        base_sha="base",
        labels=labels,
        ci=CiVerdict(
            raw_state=state,
            red_class=red,
            gate_present=True,
            gate_ok=state is CiState.GREEN,
            gate_missing_run=False,
        ),
    )


def test_raw_local_label_is_observed_but_does_not_authorize_landing() -> None:
    nodes = apply_landing_context(
        [
            _node(
                1,
                labels=(
                    "locally-validated",
                    "agent:hermit-a",
                    "landing-policy:ci-hygiene",
                    "mechanism:cancel-in-progress",
                ),
            ),
            _node(2, labels=("mechanism:cancel-in-progress",)),
        ],
        [],
    )
    assert nodes[0].validation_evidence is ValidationEvidence.LOCALLY_VALIDATED
    assert nodes[0].assigned_agent == "hermit-a"
    assert nodes[0].policy_class is PolicyClass.CI_HYGIENE
    assert build_mechanism_edges(nodes)[0].mechanisms == ("cancel-in-progress",)
    plan, _ = compute_plan(nodes, [], [], [])
    actions = {decision.pr: decision.action for decision in plan.per_pr_actions}
    assert actions[1] is PrAction.REFIRE_CI
    assert 1 not in plan.land_now


def test_clean_record_requires_and_checks_exact_head_and_base() -> None:
    with pytest.raises(ValueError, match="requires exact 'head_sha'.*'base_sha'"):
        parse_landing_context(
            {
                "prs": [
                    {"pr": 1, "validation_evidence": "clean-validate-record"}
                ]
            }
        )

    context = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "stale-sha",
                    "base_sha": "base",
                    "validation_evidence": "clean-validate-record",
                }
            ]
        }
    )
    with pytest.raises(ValueError, match="landing context is stale"):
        apply_landing_context([_node(1)], context)

    stale_base = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "stale-base",
                    "validation_evidence": "clean-validate-record",
                }
            ]
        }
    )
    with pytest.raises(ValueError, match="context base is stale.*revalidate"):
        apply_landing_context([_node(1)], stale_base)


def test_local_evidence_bypasses_ci_wait_but_gate_policy_escalates() -> None:
    context = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "base",
                    "validation_evidence": "clean-validate-record",
                    "policy_class": "ci-hygiene",
                    "assigned_agent": "hermit-ci",
                },
                {
                    "pr": 2,
                    "head_sha": "sha-2",
                    "base_sha": "base",
                    "validation_evidence": "clean-validate-record",
                    "policy_class": "gate-policy",
                },
            ]
        }
    )
    nodes = apply_landing_context(
        [
            _node(1, state=CiState.RED, red=RedClass.STALE_REQUIRED_CHECK),
            _node(2, state=CiState.GREEN),
        ],
        context,
    )
    plan, _ = compute_plan(nodes, [], [], [])
    actions = {decision.pr: decision for decision in plan.per_pr_actions}
    assert actions[1].action is PrAction.LAND_NOW
    assert "no merge-gate wait" in actions[1].why
    assert actions[2].action is PrAction.ESCALATE_GATE_POLICY
    assert 2 not in plan.land_now


def test_authoritative_ci_is_reported_as_evidence() -> None:
    node = apply_landing_context(
        [_node(1, labels=("locally-validated",), state=CiState.GREEN)], []
    )[0]
    assert node.validation_evidence is ValidationEvidence.AUTHORITATIVE_CI


def test_json_schema_exposes_context_and_mechanism_overlap() -> None:
    # Uses a RECOGNISED mechanism: under the enum redesign, clustering keys on the normalised
    # Mechanism enum value, not the raw slug, so an unknown slug ("shared") would be UNCLASSIFIED
    # rather than an edge. `cancel-in-progress` is a seeded enum member, so the pair still clusters.
    nodes = apply_landing_context(
        [
            _node(1, labels=("locally-validated", "mechanism:cancel-in-progress")),
            _node(2, labels=("mechanism:cancel-in-progress",)),
        ],
        [],
    )
    mechanism_edges = build_mechanism_edges(nodes)
    result = assemble_result(
        CollectedGraph(
            repository="owner/repo",
            base="main",
            nodes=nodes,
            conflict_edges=(),
            overlap_edges=(),
            ordering_edges=(),
            mechanism_edges=mechanism_edges,
        )
    )
    payload = json.loads(render_json(result))
    assert payload["nodes"][0]["validation_evidence"] == "locally-validated"
    assert payload["mechanism_overlap_edges"] == [
        {"a": 1, "b": 2, "mechanisms": ["cancel-in-progress"]}
    ]
