"""Landing-context evidence, policy, assignment, and mechanism-overlap tests."""

from __future__ import annotations

import json
from dataclasses import replace

import pytest

from pr_landing_planner.emit import render_json
from pr_landing_planner.landing_context import (
    apply_landing_context,
    parse_landing_context,
)
from pr_landing_planner.graph import build_mechanism_edges, held_reasons, review_binding
from pr_landing_planner.model import (
    CiState,
    CiVerdict,
    CollectedGraph,
    PolicyClass,
    PrAction,
    PrNode,
    RedClass,
    ReviewBinding,
    ValidationAuthority,
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


def test_raw_local_label_is_cache_only_but_dereferenced_record_is_evidence() -> None:
    nodes = apply_landing_context(
        [
            _node(
                1,
                labels=(
                    "locally-validated",
                    "agent:widget-a",
                    "landing-policy:ci-hygiene",
                    "mechanism:cancel-in-progress",
                ),
            ),
            _node(2, labels=("mechanism:cancel-in-progress",)),
        ],
        [],
    )
    # Negative bracket: one bare cache label produces zero validation evidence.
    assert nodes[0].validation_evidence is ValidationEvidence.NONE
    assert nodes[0].assigned_agent == "widget-a"
    assert nodes[0].policy_class is PolicyClass.CI_HYGIENE
    assert build_mechanism_edges(nodes)[0].mechanisms == ("cancel-in-progress",)
    plan, _ = compute_plan(nodes, [], [], [])
    actions = {decision.pr: decision.action for decision in plan.per_pr_actions}
    assert actions[1] is PrAction.REFIRE_CI
    assert 1 not in plan.land_now

    context = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "base",
                    "validation_evidence": "locally-validated",
                }
            ]
        }
    )
    # Positive bracket: one caller-dereferenced exact-identity record is accepted.
    dereferenced = apply_landing_context([_node(1)], context)[0]
    assert dereferenced.validation_evidence is ValidationEvidence.LOCALLY_VALIDATED

    with pytest.raises(ValueError, match="locally-validated evidence requires"):
        parse_landing_context(
            {"prs": [{"pr": 1, "validation_evidence": "locally-validated"}]}
        )


def test_clean_record_keeps_exact_head_and_delegates_older_base_to_authority() -> None:
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
                    "validation_authority": "hard-green",
                }
            ]
        }
    )
    with pytest.raises(ValueError, match="landing context is stale"):
        apply_landing_context([_node(1)], context)

    with pytest.raises(ValueError, match="requires explicit validation_authority"):
        parse_landing_context(
            {
                "prs": [
                    {
                        "pr": 1,
                        "head_sha": "sha-1",
                        "base_sha": "base",
                        "validation_evidence": "clean-validate-record",
                    }
                ]
            }
        )

    hard_green = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "base",
                    "validation_evidence": "clean-validate-record",
                    "validation_authority": "hard-green",
                }
            ]
        }
    )
    hard_green_node = apply_landing_context([_node(1)], hard_green)[0]
    assert hard_green_node.validation_authority is ValidationAuthority.HARD_GREEN
    hard_green_plan, _ = compute_plan([hard_green_node], [], [], [])
    assert hard_green_plan.per_pr_actions[0].action is PrAction.LAND_NOW

    earlier_green = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "earlier-green-base",
                    "validation_evidence": "clean-validate-record",
                    "validation_authority": "soft-green",
                }
            ]
        }
    )
    authorized = apply_landing_context(
        [replace(_node(1), commits_behind=5)], earlier_green
    )[0]
    assert authorized.validation_authority is ValidationAuthority.SOFT_GREEN
    plan, _ = compute_plan([authorized], [], [], [])
    decision = plan.per_pr_actions[0]
    assert decision.action is PrAction.REBASE_THEN_LAND
    assert "without pre-landing revalidation" in decision.why

    hard_green_on_other_base = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "divergent-base",
                    "validation_evidence": "clean-validate-record",
                    "validation_authority": "hard-green",
                }
            ]
        }
    )
    with pytest.raises(ValueError, match="supplied no soft-green authority"):
        apply_landing_context([_node(1)], hard_green_on_other_base)


def test_validation_authority_requires_a_clean_record() -> None:
    with pytest.raises(ValueError, match="requires validation_evidence"):
        parse_landing_context(
            {
                "prs": [
                    {
                        "pr": 1,
                        "head_sha": "sha-1",
                        "base_sha": "base",
                        "validation_authority": "soft-green",
                    }
                ]
            }
        )


def test_local_evidence_bypasses_ci_wait_but_gate_policy_escalates() -> None:
    context = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 1,
                    "head_sha": "sha-1",
                    "base_sha": "base",
                    "validation_evidence": "clean-validate-record",
                    "validation_authority": "hard-green",
                    "policy_class": "ci-hygiene",
                    "assigned_agent": "widget-ci",
                },
                {
                    "pr": 2,
                    "head_sha": "sha-2",
                    "base_sha": "base",
                    "validation_evidence": "clean-validate-record",
                    "validation_authority": "hard-green",
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
    assert payload["nodes"][0]["validation_evidence"] == "none"
    assert payload["nodes"][0]["validation_authority"] == "none"
    assert payload["nodes"][0]["review_binding"] == "not-required"
    assert payload["nodes"][0]["review_pass_heads"] == {}
    assert payload["mechanism_overlap_edges"] == [
        {"a": 1, "b": 2, "mechanisms": ["cancel-in-progress"]}
    ]


REVIEWED_HEAD = "92e1e0d0af65e50cd2991d4deaa25f726832fbf4"
REBASED_HEAD = "0fc9f61edc01d6425def2efb0ed82f01410c7fcc"
CHANGED_HEAD = "1111111111111111111111111111111111111111"
REVIEW_LABELS = (
    "post-facto-human-review",
    "passed-review-codex",
    "passed-review-claude",
)


def test_review_passes_bind_exact_head_and_head_moves_fail_closed() -> None:
    exact_context = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 394,
                    "review_pass_heads": {
                        "codex": REBASED_HEAD,
                        "claude": REBASED_HEAD,
                    },
                }
            ]
        }
    )
    exact = apply_landing_context(
        [replace(_node(394, labels=REVIEW_LABELS), head_sha=REBASED_HEAD)],
        exact_context,
    )[0]
    assert review_binding(exact) == (ReviewBinding.EXACT_HEAD, ())
    assert not held_reasons((exact,), ())

    stale_context = parse_landing_context(
        {
            "prs": [
                {
                    "pr": 394,
                    "review_pass_heads": {
                        "codex": REBASED_HEAD,
                        "claude": REVIEWED_HEAD,
                    },
                }
            ]
        }
    )
    stale = apply_landing_context(
        [replace(_node(394, labels=REVIEW_LABELS), head_sha=REBASED_HEAD)],
        stale_context,
    )[0]
    reason = (
        f"review-pass-stale:claude:reviewed={REVIEWED_HEAD}:current={REBASED_HEAD}"
    )
    assert review_binding(stale) == (ReviewBinding.STALE, (reason,))
    assert held_reasons((stale,), ())[0].reasons == (reason,)

    changed = replace(stale, head_sha=CHANGED_HEAD)
    assert review_binding(changed)[0] is ReviewBinding.STALE


def test_review_pass_labels_without_receipts_are_unbound_and_bad_receipts_refuse() -> None:
    unbound = apply_landing_context(
        [replace(_node(394, labels=REVIEW_LABELS), head_sha=REBASED_HEAD)], ()
    )[0]
    assert review_binding(unbound)[0] is ReviewBinding.UNBOUND

    with pytest.raises(ValueError, match="exact 40-character lowercase hex SHA"):
        parse_landing_context(
            {"prs": [{"pr": 394, "review_pass_heads": {"claude": "0fc9f61e"}}]}
        )
    with pytest.raises(ValueError, match="unknown review lane"):
        parse_landing_context(
            {"prs": [{"pr": 394, "review_pass_heads": {"other": REBASED_HEAD}}]}
        )
