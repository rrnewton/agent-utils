"""Human-readable DAG visualizations: Graphviz DOT and quick ASCII art.

Pure functions over a :class:`~safe_ci_dag_runner.model.DagConfig` — no I/O.

* :func:`to_dot` emits Graphviz (pipe to ``dot -Tsvg -o dag.svg``): one cluster per group,
  solid dependency edges, and a dashed edge chain for each cap-1 scarce resource (a visual
  hint that those steps serialize).
* :func:`to_ascii` emits a compact topological-layer view for a glance in the terminal —
  each step on its layer (longest dependency depth), with its class, scarce-resource demand,
  and immediate dependencies.
"""

from __future__ import annotations

from safe_ci_dag_runner.model import DagConfig, Step, step_classification

__all__ = ["to_dot", "to_ascii"]


def _selected_steps(cfg: DagConfig, selected: set[str] | None) -> list[Step]:
    return [s for s in cfg.steps if selected is None or s.tag in selected]


def _kept_deps(steps: list[Step]) -> dict[str, list[str]]:
    """Each step's deps, filtered to steps that are actually present in the selection."""
    tags = {s.tag for s in steps}
    return {s.tag: [d for d in s.deps if d in tags] for s in steps}


def to_dot(cfg: DagConfig, *, name: str = "dag", selected: set[str] | None = None) -> str:
    """Render the DAG as Graphviz DOT."""
    steps = _selected_steps(cfg, selected)
    deps = _kept_deps(steps)
    by_group: dict[str, list[Step]] = {}
    for step in steps:
        by_group.setdefault(step.group, []).append(step)

    out: list[str] = [
        f"digraph {name} {{",
        "  rankdir=LR;",
        "  node [shape=box, style=rounded, fontsize=10];",
        '  labelloc="t";',
        '  label="DAG  (solid = dependency;  dashed = shared cap-1 resource -> serialized)";',
    ]
    for i, group in enumerate(sorted(by_group)):
        out.append(f"  subgraph cluster_{i} {{")
        out.append(f'    label="{group}"; style=dashed; color=gray70;')
        for step in sorted(by_group[group], key=lambda s: s.job):
            out.append(f'    "{step.tag}" [label="{step.tag}\\n[{step_classification(step).value}]"];')
        out.append("  }")

    for step in steps:
        for dep in deps[step.tag]:
            out.append(f'  "{dep}" -> "{step.tag}";')

    # A dashed chain across the users of each cap-1 resource: a hint that they serialize.
    for res, cap in sorted(cfg.resource_caps.items()):
        if cap == 1:
            users = sorted(s.tag for s in steps if s.hint.resources.get(res, 0) > 0)
            for left, right in zip(users, users[1:]):
                out.append(
                    f'  "{left}" -> "{right}" '
                    f'[style=dashed, color=gray60, constraint=false, label="{res}"];'
                )
    out.append("}")
    return "\n".join(out) + "\n"


def _layers(steps: list[Step], deps: dict[str, list[str]]) -> dict[str, int]:
    """Longest-dependency-depth layer for each step tag."""
    depth: dict[str, int] = {}

    def visit(tag: str) -> int:
        if tag in depth:
            return depth[tag]
        parents = deps.get(tag, [])
        depth[tag] = 0 if not parents else 1 + max(visit(p) for p in parents)
        return depth[tag]

    for step in steps:
        visit(step.tag)
    return depth


def to_ascii(cfg: DagConfig, *, selected: set[str] | None = None) -> str:
    """Render the DAG as compact ASCII art, grouped by topological layer."""
    steps = _selected_steps(cfg, selected)
    deps = _kept_deps(steps)
    depth = _layers(steps, deps)
    layers: dict[int, list[Step]] = {}
    for step in steps:
        layers.setdefault(depth[step.tag], []).append(step)

    edge_count = sum(len(d) for d in deps.values())
    width = max((len(s.tag) for s in steps), default=1)
    out: list[str] = [
        f"DAG - {len(steps)} steps, {edge_count} edges, {len(layers)} layer(s)",
        "",
    ]
    for level in sorted(layers):
        out.append(f"layer {level}:")
        for step in sorted(layers[level], key=lambda s: s.tag):
            res = "".join(f" {{{k}:{v}}}" for k, v in sorted(step.hint.resources.items()))
            dep = "  <- " + ", ".join(sorted(deps[step.tag])) if deps[step.tag] else ""
            out.append(f"  {step.tag:<{width}}  [{step_classification(step).value}]{res}{dep}")
    return "\n".join(out) + "\n"
