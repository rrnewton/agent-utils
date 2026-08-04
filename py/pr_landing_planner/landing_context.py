"""Pure parsing and application of caller-supplied landing context.

GitHub exposes checks and labels, but an external coordinator owns three facts the
generic planner cannot infer: an exact-head local validation record, the assigned
landing agent, and whether a change alters gate policy.  ``--landing-context``
supplies those facts without baking one project's ledger or task system into this
package.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace

from pr_landing_planner.model import (
    CiState,
    PolicyClass,
    PrNode,
    ValidationEvidence,
)

AGENT_PREFIX = "agent:"
POLICY_PREFIX = "landing-policy:"
LOCALLY_VALIDATED_LABEL = "locally-validated"


@dataclass(frozen=True)
class LandingContext:
    """Caller-owned landing facts for one PR, optionally guarded by exact head SHA."""

    pr: int
    head_sha: str = ""
    assigned_agent: str = ""
    validation_evidence: ValidationEvidence | None = None
    policy_class: PolicyClass | None = None


def _str_field(obj: Mapping[str, object], key: str) -> str:
    value = obj.get(key)
    return value if isinstance(value, str) else ""


def parse_landing_context(raw: object) -> tuple[LandingContext, ...]:
    """Parse ``{"prs": [...]}``, rejecting ambiguous or unsafe evidence."""
    if not isinstance(raw, dict):
        raise ValueError("landing context must be an object with a 'prs' array")
    prs = raw.get("prs")
    if not isinstance(prs, list):
        raise ValueError("landing context field 'prs' must be an array")

    contexts: list[LandingContext] = []
    seen: set[int] = set()
    for item in prs:
        if not isinstance(item, dict):
            raise ValueError("each landing context PR entry must be an object")
        obj: dict[str, object] = {str(key): value for key, value in item.items()}
        pr = obj.get("pr")
        if not isinstance(pr, int) or isinstance(pr, bool) or pr <= 0:
            raise ValueError("landing context PR entry needs a positive integer 'pr'")
        if pr in seen:
            raise ValueError(f"landing context contains duplicate PR #{pr}")
        seen.add(pr)

        evidence_raw = _str_field(obj, "validation_evidence")
        policy_raw = _str_field(obj, "policy_class")
        try:
            evidence = ValidationEvidence(evidence_raw) if evidence_raw else None
        except ValueError as exc:
            raise ValueError(
                f"PR #{pr} has unknown validation_evidence {evidence_raw!r}"
            ) from exc
        try:
            policy = PolicyClass(policy_raw) if policy_raw else None
        except ValueError as exc:
            raise ValueError(f"PR #{pr} has unknown policy_class {policy_raw!r}") from exc

        head_sha = _str_field(obj, "head_sha")
        if evidence is ValidationEvidence.CLEAN_VALIDATE_RECORD and not head_sha:
            raise ValueError(
                f"PR #{pr} clean-validate-record evidence requires exact 'head_sha'"
            )
        contexts.append(
            LandingContext(
                pr=pr,
                head_sha=head_sha,
                assigned_agent=_str_field(obj, "assigned_agent"),
                validation_evidence=evidence,
                policy_class=policy,
            )
        )
    return tuple(contexts)


def _one_label_value(labels: Sequence[str], prefix: str, field: str, pr: int) -> str:
    values = sorted(
        {
            label[len(prefix) :]
            for label in labels
            if label.startswith(prefix) and label != prefix
        }
    )
    if len(values) > 1:
        raise ValueError(f"PR #{pr} has multiple {field} labels: {', '.join(values)}")
    return values[0] if values else ""


def _label_context(node: PrNode) -> PrNode:
    assigned_agent = _one_label_value(node.labels, AGENT_PREFIX, "agent", node.number)
    policy_raw = _one_label_value(node.labels, POLICY_PREFIX, "landing-policy", node.number)
    try:
        policy = PolicyClass(policy_raw) if policy_raw else PolicyClass.UNCLASSIFIED
    except ValueError as exc:
        raise ValueError(
            f"PR #{node.number} has unknown landing-policy label {policy_raw!r}"
        ) from exc

    if node.ci.raw_state is CiState.GREEN and node.ci.gate_ok:
        evidence = ValidationEvidence.AUTHORITATIVE_CI
    elif LOCALLY_VALIDATED_LABEL in node.labels:
        # The label is an observable cache hint, not evidence.  Only caller-supplied
        # exact-head records and authoritative CI can authorize a landing.
        evidence = ValidationEvidence.LOCALLY_VALIDATED
    else:
        evidence = ValidationEvidence.NONE
    return replace(
        node,
        assigned_agent=assigned_agent,
        validation_evidence=evidence,
        policy_class=policy,
    )


def apply_landing_context(
    nodes: Sequence[PrNode], contexts: Sequence[LandingContext]
) -> tuple[PrNode, ...]:
    """Apply labels, then exact-head caller context; fail closed on drift or unknown PRs."""
    by_context = {context.pr: context for context in contexts}
    node_numbers = {node.number for node in nodes}
    unknown = sorted(set(by_context) - node_numbers)
    if unknown:
        rendered = ", ".join(f"#{number}" for number in unknown)
        raise ValueError(f"landing context names PRs absent from this plan: {rendered}")

    out: list[PrNode] = []
    for raw_node in nodes:
        node = _label_context(raw_node)
        context = by_context.get(node.number)
        if context is None:
            out.append(node)
            continue
        if context.head_sha and context.head_sha != node.head_sha:
            raise ValueError(
                f"PR #{node.number} landing context is stale: "
                f"context={context.head_sha}, current={node.head_sha}"
            )
        out.append(
            replace(
                node,
                assigned_agent=context.assigned_agent or node.assigned_agent,
                validation_evidence=(
                    context.validation_evidence
                    if context.validation_evidence is not None
                    else node.validation_evidence
                ),
                policy_class=(
                    context.policy_class
                    if context.policy_class is not None
                    else node.policy_class
                ),
            )
        )
    return tuple(out)
