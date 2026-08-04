"""PURE rendering of a :class:`~pr_landing_planner.model.PlanResult` into the three output formats.

* ``human``   — a readable landing summary (the ``pr_status`` + ``pr_conflict_graph`` views fused).
* ``json``    — the machine-facing schema (deterministic: 2-space indent, sorted keys).
* ``actions`` — tick-hub-style line output: a block of bare ``key=value`` summary counts (so a
  tick-hub reminder's ``capture: true`` gate can lift ``land_now`` / ``stale_gates`` / ``outage``
  into its emitted line), then loud diagnostic ``ERROR:`` / ``NOTE:`` lines, then one
  ``ACTION:`` / ``ERROR:`` / ``NOTE:`` line per PR in the recommended order. A coordinator parses the
  per-PR lines by their leading token; tick-hub captures the summary block. Diagnostics
  (outage / race / stale-gate) map to LOUD lines (No Silent Failure).
"""

from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from typing import Callable

from pr_landing_planner.model import (
    CiState,
    HeldPr,
    MechanismEdge,
    PlanResult,
    PrAction,
    PrActionDecision,
    PrNode,
)

#: A colorizer: ``(style_name, text) -> styled_text``. The default (identity) leaves text plain.
ColorFn = Callable[[str, str], str]


# --------------------------------------------------------------------------- quoting
def _quote(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


# --------------------------------------------------------------------------- json
def _node_obj(node: PrNode, held: bool) -> dict[str, object]:
    return {
        "pr": node.number,
        "title": node.title,
        "author": node.author,
        "head": node.head_sha,
        "base_ref": node.base_ref,
        "ci": node.ci.raw_state.value,
        "ci_detail": node.ci.detail,
        "red_class": node.ci.red_class.value if node.ci.red_class is not None else None,
        "gate_ok": node.ci.gate_ok,
        "freshness_behind": node.commits_behind,
        "size": node.size,
        "held": held,
        "priority": node.priority,
        "labels": list(node.labels),
    }


def _decision_obj(decision: PrActionDecision) -> dict[str, object]:
    return {
        "pr": decision.pr,
        "action": decision.action.value,
        "why": decision.why,
        "group": decision.group,
    }


def render_json(result: PlanResult) -> str:
    """Render the whole plan as canonical, deterministic JSON."""
    graph = result.graph
    held_set = {h.pr for h in result.held}
    obj: dict[str, object] = {
        "repository": graph.repository,
        "base": graph.base,
        "nodes": [_node_obj(n, n.number in held_set) for n in graph.nodes],
        "conflict_edges": [
            {"a": e.a, "b": e.b, "paths": list(e.paths)} for e in graph.conflict_edges
        ],
        "file_overlap_edges": [
            {"a": e.a, "b": e.b, "paths": list(e.paths)} for e in graph.overlap_edges
        ],
        "ordering_edges": [
            {"before": e.before, "after": e.after, "reason": e.reason}
            for e in graph.ordering_edges
        ],
        "mechanism_overlap_edges": [
            {"a": e.a, "b": e.b, "mechanisms": list(e.mechanisms)}
            for e in graph.mechanism_edges
        ],
        "unclassified_mechanism_candidates": [
            {"pr": u.pr, "candidates": list(u.candidates)} for u in graph.unclassified_mechanisms
        ],
        "stacks": [list(stack) for stack in result.stacks],
        "held_prs": [{"pr": h.pr, "reasons": list(h.reasons)} for h in result.held],
        "plan": {
            "parallel_safe_groups": [list(g) for g in result.plan.parallel_safe_groups],
            "land_now": list(result.plan.land_now),
            "order": list(result.plan.order),
            "batch": list(result.plan.batch),
            "per_pr_actions": [_decision_obj(d) for d in result.plan.per_pr_actions],
        },
        "diagnostics": {
            "stale_gates": list(result.diagnostics.stale_gates),
            "flaky_reds": list(result.diagnostics.flaky_reds),
            "real_reds": list(result.diagnostics.real_reds),
            "evaluate_once_race": list(result.diagnostics.evaluate_once_race),
            "outage_prs": list(result.diagnostics.outage_prs),
            "outage_suspected": result.diagnostics.outage_suspected,
        },
    }
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=False)


# --------------------------------------------------------------------------- actions
_ACTION_LINE_KIND: Mapping[PrAction, str] = {
    PrAction.LAND_NOW: "ACTION",
    PrAction.REBASE_THEN_LAND: "ACTION",
    PrAction.REFIRE_STALE_GATE: "ACTION",
    PrAction.REFIRE_CI: "ACTION",
    PrAction.HOLD_FIX: "ACTION",
    PrAction.ESCALATE_RUNNER_OUTAGE: "ERROR",
    PrAction.WAIT: "NOTE",
}


def _summary_counts(result: PlanResult) -> list[tuple[str, int]]:
    counts = {action: 0 for action in PrAction}
    for decision in result.plan.per_pr_actions:
        counts[decision.action] += 1
    diag = result.diagnostics
    return [
        ("open_prs", len(result.graph.nodes)),
        ("land_now", counts[PrAction.LAND_NOW]),
        ("rebase", counts[PrAction.REBASE_THEN_LAND]),
        ("refire_stale_gate", counts[PrAction.REFIRE_STALE_GATE]),
        ("refire_ci", counts[PrAction.REFIRE_CI]),
        ("hold_fix", counts[PrAction.HOLD_FIX]),
        ("escalate_outage", counts[PrAction.ESCALATE_RUNNER_OUTAGE]),
        ("wait", counts[PrAction.WAIT]),
        ("held", len(result.held)),
        ("stale_gates", len(diag.stale_gates)),
        ("flaky_reds", len(diag.flaky_reds)),
        ("real_reds", len(diag.real_reds)),
        ("evaluate_once_race", len(diag.evaluate_once_race)),
        ("mechanism_overlaps", len(result.graph.mechanism_edges)),
        ("unclassified_mechanisms", len(result.graph.unclassified_mechanisms)),
        ("outage", 1 if diag.outage_suspected else 0),
    ]


def _pr_action_line(decision: PrActionDecision, node: PrNode | None) -> str:
    kind = _ACTION_LINE_KIND[decision.action]
    parts = [f"{kind}: {decision.action.value}", f"pr={decision.pr}"]
    if decision.group is not None:
        parts.append(f"group={decision.group}")
    if node is not None:
        if decision.action is PrAction.REBASE_THEN_LAND and node.commits_behind:
            parts.append(f"behind={node.commits_behind}")
        if decision.action is PrAction.LAND_NOW:
            parts.append(f"size={node.size}")
    parts.append(f"why={_quote(decision.why)}")
    return " ".join(parts)


def render_actions(result: PlanResult) -> str:
    """Render tick-hub-style lines: a capturable summary block, diagnostics, then per-PR lines."""
    by_number = {n.number: n for n in result.graph.nodes}
    lines: list[str] = [f"{key}={value}" for key, value in _summary_counts(result)]

    diag = result.diagnostics
    if diag.outage_suspected:
        prs = ",".join(str(n) for n in diag.outage_prs)
        lines.append(
            f"ERROR: ci-hosted-runner-outage-systemic prs={prs} "
            f'detail={_quote(f"merge-gate job never ran on {len(diag.outage_prs)} PR(s) (ds-69ih3r)")}'
        )
    for pr in diag.evaluate_once_race:
        lines.append(
            f"NOTE: evaluate-once-race pr={pr} "
            "(benign gate noise; treat as pending, ds-xdc7m9)"
        )
    for me in result.graph.mechanism_edges:
        lines.append(
            f"NOTE: mechanism-overlap prs={me.a},{me.b} "
            f"mechanisms={_quote(','.join(me.mechanisms))} "
            "(same mechanism — review together; may be opposite intent)"
        )
    for um in result.graph.unclassified_mechanisms:
        lines.append(
            f"NOTE: unclassified-mechanism pr={um.pr} "
            f"candidates={_quote(','.join(um.candidates))} "
            "(not in the enum — recognise as an existing member or add a new one)"
        )

    decisions = {d.pr: d for d in result.plan.per_pr_actions}
    emitted: set[int] = set()
    for pr in result.plan.order:
        decision = decisions.get(pr)
        if decision is not None:
            lines.append(_pr_action_line(decision, by_number.get(pr)))
            emitted.add(pr)
    for decision in result.plan.per_pr_actions:
        if decision.pr not in emitted:
            lines.append(_pr_action_line(decision, by_number.get(decision.pr)))
    return "\n".join(lines)


# --------------------------------------------------------------------------- human
def _held_line(held: HeldPr) -> str:
    return f"  #{held.pr}: {', '.join(held.reasons)}"


def render_human(result: PlanResult, color: ColorFn | None = None) -> str:
    """Render a readable landing summary. ``color`` is an optional (style, text)->text colorizer."""
    c = color if color is not None else (lambda _style, text: text)
    graph = result.graph
    plan = result.plan
    diag = result.diagnostics
    by_number = {n.number: n for n in graph.nodes}
    held_set = {h.pr for h in result.held}

    n_conflict = len(graph.conflict_edges)
    n_overlap = len(graph.overlap_edges)
    lines: list[str] = [
        c("bold", f"Repository: {graph.repository}  base: {graph.base}"),
        (
            f"{len(graph.nodes)} open PR(s), {n_conflict} real conflict(s), "
            f"{n_overlap} file-overlap risk(s), {len(graph.ordering_edges)} ordering edge(s), "
            f"{len(graph.mechanism_edges)} mechanism overlap(s)"
        ),
        "",
        c("bold", "CI health:"),
    ]
    for node in sorted(graph.nodes, key=lambda n: n.number):
        red = node.ci.red_class.value if node.ci.red_class is not None else ""
        red_str = f" [{red}]" if red else ""
        held_str = " HELD" if node.number in held_set else ""
        state = node.ci.raw_state.value
        styled = c("green" if state == "green" else ("yellow" if state == "pending" else "red"), state)
        lines.append(
            f"  #{node.number:<5} ci={styled}{red_str} behind={node.commits_behind} "
            f"size={node.size}{held_str}  {node.title}"
        )

    lines.extend(["", c("bold", "Parallel-safe groups (each group lands in any order):")])
    if plan.parallel_safe_groups:
        for idx, group in enumerate(plan.parallel_safe_groups):
            lines.append(f"  group {idx}: " + ", ".join(f"#{n}" for n in group))
    else:
        lines.append("  (none)")

    lines.extend(
        [
            "",
            "Land now: " + (", ".join(f"#{n}" for n in plan.land_now) or "none"),
            "Recommended order: " + (", ".join(f"#{n}" for n in plan.order) or "none"),
        ]
    )
    if plan.batch:
        lines.append("Batch (green-only, conflict-free): " + ", ".join(f"#{n}" for n in plan.batch))

    lines.extend(["", c("bold", "Per-PR actions:")])
    for decision in plan.per_pr_actions:
        title = by_number[decision.pr].title if decision.pr in by_number else ""
        lines.append(
            f"  #{decision.pr:<5} {c('cyan', decision.action.value):<22} {decision.why}"
            + (f"  ({title})" if title else "")
        )

    lines.extend(
        ["", c("bold", "Mechanism overlaps (same mechanism — review together, may be opposite intent):")]
    )
    if graph.mechanism_edges:
        for me in graph.mechanism_edges:
            lines.append(f"  #{me.a} <-> #{me.b}: {', '.join(me.mechanisms)}")
    else:
        lines.append("  (none)")

    if graph.unclassified_mechanisms:
        lines.extend(
            ["", c("bold", "Unclassified mechanism candidates (recognise, then extend the enum):")]
        )
        for um in graph.unclassified_mechanisms:
            lines.append(f"  #{um.pr}: {', '.join(um.candidates)}")

    lines.extend(["", c("bold", "Diagnostics:")])
    _append_diag(lines, "stale required-check (refire gate)", diag.stale_gates)
    _append_diag(lines, "flaky reds (refire CI)", diag.flaky_reds)
    _append_diag(lines, "real reds (hold + fix)", diag.real_reds)
    _append_diag(lines, "evaluate-once race (benign)", diag.evaluate_once_race)
    _append_diag(lines, "runner-outage", diag.outage_prs)
    if diag.outage_suspected:
        lines.append(c("red", "  SYSTEMIC RUNNER OUTAGE SUSPECTED — escalate CI (ds-69ih3r)"))

    if result.held:
        lines.extend(["", c("bold", "Held PRs:")])
        lines.extend(_held_line(h) for h in result.held)

    lines.append("")
    lines.append(c("dim", "Advisory only: this plan recommends; it never arms or merges anything."))
    return "\n".join(lines)


def _append_diag(lines: list[str], label: str, prs: Sequence[int]) -> None:
    if prs:
        lines.append(f"  {label}: " + ", ".join(f"#{n}" for n in prs))


# --------------------------------------------------------------------------- graph view
def render_graph_json(result: PlanResult) -> str:
    """The conflict/ordering-graph view only (no plan/diagnostics) — the pr_conflict_graph.py shape."""
    graph = result.graph
    held_set = {h.pr for h in result.held}
    obj: dict[str, object] = {
        "repository": graph.repository,
        "base": graph.base,
        "nodes": [_node_obj(n, n.number in held_set) for n in graph.nodes],
        "conflict_edges": [
            {"a": e.a, "b": e.b, "paths": list(e.paths)} for e in graph.conflict_edges
        ],
        "file_overlap_edges": [
            {"a": e.a, "b": e.b, "paths": list(e.paths)} for e in graph.overlap_edges
        ],
        "ordering_edges": [
            {"before": e.before, "after": e.after, "reason": e.reason}
            for e in graph.ordering_edges
        ],
        "mechanism_overlap_edges": [
            {"a": e.a, "b": e.b, "mechanisms": list(e.mechanisms)}
            for e in graph.mechanism_edges
        ],
        "unclassified_mechanism_candidates": [
            {"pr": u.pr, "candidates": list(u.candidates)} for u in graph.unclassified_mechanisms
        ],
        "stacks": [list(stack) for stack in result.stacks],
        "held_prs": [{"pr": h.pr, "reasons": list(h.reasons)} for h in result.held],
    }
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=False)


def render_graph_human(result: PlanResult, color: ColorFn | None = None) -> str:
    """A readable conflict/ordering-graph summary (stacks, real conflicts, overlaps, held)."""
    c = color if color is not None else (lambda _style, text: text)
    graph = result.graph
    lines: list[str] = [
        c("bold", f"Repository: {graph.repository}  base: {graph.base}"),
        (
            f"{len(graph.nodes)} open PR(s), {len(graph.conflict_edges)} real conflict(s), "
            f"{len(graph.overlap_edges)} file-overlap risk(s), "
            f"{len(graph.ordering_edges)} ordering edge(s), "
            f"{len(graph.mechanism_edges)} mechanism overlap(s)"
        ),
        "",
        c("bold", "Stacks:"),
    ]
    if result.stacks:
        lines.extend("  " + " -> ".join(f"#{n}" for n in s) for s in result.stacks)
    else:
        lines.append("  (none)")
    lines.extend(["", c("bold", "Real conflicts (git merge-tree):")])
    if graph.conflict_edges:
        for e in graph.conflict_edges:
            preview = ", ".join(e.paths[:5])
            more = f" (+{len(e.paths) - 5} more)" if len(e.paths) > 5 else ""
            lines.append(f"  #{e.a} <-> #{e.b}: {preview}{more}")
    else:
        lines.append("  (none)")
    lines.extend(["", c("bold", "File-overlap risks (auto-mergeable but shared files):")])
    if graph.overlap_edges:
        for oe in graph.overlap_edges:
            preview = ", ".join(oe.paths[:5])
            more = f" (+{len(oe.paths) - 5} more)" if len(oe.paths) > 5 else ""
            lines.append(f"  #{oe.a} <-> #{oe.b}: {preview}{more}")
    else:
        lines.append("  (none)")
    lines.extend(
        ["", c("bold", "Mechanism overlaps (same mechanism — review together, may be opposite intent):")]
    )
    if graph.mechanism_edges:
        for me in graph.mechanism_edges:
            lines.append(f"  #{me.a} <-> #{me.b}: {', '.join(me.mechanisms)}")
    else:
        lines.append("  (none)")
    lines.extend(
        ["", c("bold", "Unclassified mechanism candidates (recognise, then extend the enum):")]
    )
    if graph.unclassified_mechanisms:
        for um in graph.unclassified_mechanisms:
            lines.append(f"  #{um.pr}: {', '.join(um.candidates)}")
    else:
        lines.append("  (none)")
    lines.extend(["", c("bold", "Held PRs:")])
    lines.extend(_held_line(h) for h in result.held)
    if not result.held:
        lines.append("  (none)")
    return "\n".join(lines)


# --------------------------------------------------------------------------- status view
def render_status_json(result: PlanResult) -> str:
    """Per-PR CI/label health only — the pr_status.py shape."""
    diag = result.diagnostics
    obj: dict[str, object] = {
        "repository": result.graph.repository,
        "base": result.graph.base,
        "prs": [
            {
                "pr": n.number,
                "ci": n.ci.raw_state.value,
                "red_class": n.ci.red_class.value if n.ci.red_class is not None else None,
                "draft": n.is_draft,
                "labels": list(n.labels),
                "title": n.title,
            }
            for n in sorted(result.graph.nodes, key=lambda n: n.number)
        ],
        "summary": {
            "open": len(result.graph.nodes),
            "green": sum(n.ci.raw_state is CiState.GREEN for n in result.graph.nodes),
            "red": sum(n.ci.raw_state is CiState.RED for n in result.graph.nodes),
            "pending": sum(n.ci.raw_state is CiState.PENDING for n in result.graph.nodes),
            "real_reds": len(diag.real_reds),
            "outage_suspected": diag.outage_suspected,
        },
    }
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=False)


def render_status_human(
    result: PlanResult, warn_threshold: int, color: ColorFn | None = None
) -> str:
    """A readable per-PR CI/label health report with an open-PR-count warning."""
    c = color if color is not None else (lambda _style, text: text)
    nodes = sorted(result.graph.nodes, key=lambda n: n.number)
    lines: list[str] = [c("bold", f"Open PR health: {result.graph.repository}"), ""]
    for n in nodes:
        state = n.ci.raw_state.value
        styled = c("green" if state == "green" else ("yellow" if state == "pending" else "red"), state)
        red = f" [{n.ci.red_class.value}]" if n.ci.red_class is not None else ""
        draft = " draft" if n.is_draft else ""
        labels = f"  labels={','.join(n.labels)}" if n.labels else ""
        lines.append(f"  #{n.number:<5} ci={styled}{red}{draft}{labels}  {n.title}")
    reds = sum(n.ci.raw_state is CiState.RED for n in nodes)
    lines.extend(
        [
            "",
            c("bold", "Summary"),
            f"  open:      {len(nodes)}",
            f"  ci-red:    {reds}",
            f"  real reds: {len(result.diagnostics.real_reds)}",
        ]
    )
    if result.diagnostics.outage_suspected:
        lines.append(c("red", "  SYSTEMIC RUNNER OUTAGE SUSPECTED (ds-69ih3r)"))
    if len(nodes) > warn_threshold:
        lines.append(
            c("yellow", f"  WARNING: {len(nodes)} open PRs exceeds the {warn_threshold} threshold; "
              "prioritize landing/CI repair.")
        )
    return "\n".join(lines)


__all__ = [
    "render_json",
    "render_actions",
    "render_human",
    "render_graph_json",
    "render_graph_human",
    "render_status_json",
    "render_status_human",
    "ColorFn",
]
