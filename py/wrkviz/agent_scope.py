"""Which of a team's agents a summarize run reads: all of them, or the coordinator alone.

A team is a coordinator plus the children it starts inside its own session -- Claude Agent and
workflow subagents, Codex-spawned threads. ``all`` (the default) summarizes every one of them.
``coordinator`` summarizes only the team's root session and the sessions that explicitly continue
it (``continuation`` edges, which Codex and Orc ingest record from the manifest's
``continuation_sessions``). The coordinator's own transcript still shows what it dispatched and
what came back; what drops out is each child's own work, phase by phase.

A coordinator-only summary is a different artifact from a full-team one, and the two must never
be mistaken for each other or share a cache. So a non-default scope writes its whole summary tree
-- caches, phase and rollup projections, artifact catalog, run state, and Markdown -- under
``teams/<slug>/summary_scopes/<scope>/`` instead of ``summary_data/``, and its rollup Markdown
says in its heading and first line that child agents are excluded. ``build`` and ``export`` read
only ``summary_data/``, so the website stays full-team coverage.
"""

from __future__ import annotations

from dataclasses import replace

from wrkviz.model import TeamData

ALL_AGENTS = "all"
COORDINATOR_ONLY = "coordinator"
AGENT_SCOPES: tuple[str, ...] = (ALL_AGENTS, COORDINATOR_ONLY)

#: The line every coordinator-only rollup carries under its heading.
COORDINATOR_SCOPE_NOTE = (
    "**Scope: coordinator only.** Built from the team's root coordinator session and its "
    "declared continuation sessions. In-session child agents (subagents, spawned threads) are "
    "excluded except as the coordinator saw them, so this is not full-team coverage."
)


def validate_agent_scope(agent_scope: str) -> str:
    """Return *agent_scope* when it names a known scope; raise ``ValueError`` otherwise."""

    if agent_scope not in AGENT_SCOPES:
        raise ValueError(
            f"unknown agent scope {agent_scope!r}; expected one of {', '.join(AGENT_SCOPES)}"
        )
    return agent_scope


def coordinator_thread_ids(team: TeamData) -> frozenset[str]:
    """Return the root thread plus every session reachable from it by continuation edges."""

    successors: dict[str, set[str]] = {}
    for edge in team.edges:
        if edge.kind == "continuation":
            successors.setdefault(edge.from_thread_id, set()).add(edge.to_thread_id)
    selected = {team.root_thread_id}
    pending = [team.root_thread_id]
    while pending:
        for successor in successors.get(pending.pop(), ()):
            if successor not in selected:
                selected.add(successor)
                pending.append(successor)
    return frozenset(selected)


def restrict_to_agent_scope(team: TeamData, agent_scope: str) -> TeamData:
    """Return *team* unchanged for ``all``; otherwise only its coordinator threads' records."""

    validate_agent_scope(agent_scope)
    if agent_scope == ALL_AGENTS:
        return team
    keep = coordinator_thread_ids(team)
    return replace(
        team,
        agents=tuple(agent for agent in team.agents if agent.thread_id in keep),
        turns=tuple(turn for turn in team.turns if turn.thread_id in keep),
        events=tuple(event for event in team.events if event.thread_id in keep),
        tool_calls=tuple(tool for tool in team.tool_calls if tool.thread_id in keep),
        edges=tuple(
            edge
            for edge in team.edges
            if edge.from_thread_id in keep and edge.to_thread_id in keep
        ),
    )
