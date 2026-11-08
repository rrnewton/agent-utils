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

A narrowed run says what it kept ("kept N of M agents" and the kept thread ids, on the console,
in the run receipt and in ``summarize_state.json``) so a wrong pick cannot pass silently. It
refuses a team whose root thread is not among its agents, and it refuses Claude prompt-history
teams outright: every session there is its own parentless root with no continuation edges, so
nothing identifies a coordinator. A coordinator with no activity in the window is not refused;
that is a legitimate answer.
"""

from __future__ import annotations

from dataclasses import dataclass, replace

from wrkviz.archive import JsonValue, as_array, as_int, as_object, as_string
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


#: The provider whose sessions are all parentless roots with no continuation edges.
_UNSCOPABLE_PROVIDERS = frozenset({"claude-history"})


@dataclass(frozen=True)
class ScopeSelection:
    """Which of a team's agents a narrowed scope kept."""

    agent_scope: str
    kept_thread_ids: tuple[str, ...]
    total_agents: int

    def describe(self) -> str:
        """One console line: the scope, the counts, and the kept thread ids."""

        return (
            f"agent scope {self.agent_scope}: kept {len(self.kept_thread_ids)} of "
            f"{self.total_agents} agents ({', '.join(self.kept_thread_ids)})"
        )

    def to_json_obj(self) -> dict[str, JsonValue]:
        """The record a receipt and ``summarize_state.json`` carry."""

        return {
            "agent_scope": self.agent_scope,
            "kept_agents": len(self.kept_thread_ids),
            "total_agents": self.total_agents,
            "kept_thread_ids": list(self.kept_thread_ids),
        }


def describe_selection(value: JsonValue) -> str:
    """The console line for a recorded selection (see :meth:`ScopeSelection.to_json_obj`)."""

    obj = as_object(value, "agent_scope_selection")
    kept = as_array(obj.get("kept_thread_ids"), "agent_scope_selection.kept_thread_ids")
    return ScopeSelection(
        agent_scope=as_string(obj.get("agent_scope"), "agent_scope_selection.agent_scope"),
        kept_thread_ids=tuple(as_string(item, "kept thread id") for item in kept),
        total_agents=as_int(obj.get("total_agents"), "agent_scope_selection.total_agents"),
    ).describe()


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


def select_agent_scope(
    team: TeamData, agent_scope: str
) -> tuple[TeamData, ScopeSelection | None]:
    """Return *team* narrowed to *agent_scope* and what was kept (``None`` for ``all``).

    Raises ``ValueError`` for a provider with no identifiable coordinator and for a team whose
    root thread is not among its agents.
    """

    validate_agent_scope(agent_scope)
    if agent_scope == ALL_AGENTS:
        return team, None
    if team.provider in _UNSCOPABLE_PROVIDERS:
        raise ValueError(
            f"team {team.team_slug}: --agent-scope {agent_scope} cannot select a coordinator "
            f"in a {team.provider} team: every session is its own parentless root with no "
            "continuation edges. Summarize it with --agent-scope all."
        )
    agent_ids = [agent.thread_id for agent in team.agents]
    if team.root_thread_id not in agent_ids:
        raise ValueError(
            f"team {team.team_slug}: root thread {team.root_thread_id} is not among its "
            f"{len(agent_ids)} agents; refusing to guess which agent is the coordinator"
        )
    keep = coordinator_thread_ids(team)
    narrowed = _restricted(team, keep)
    return narrowed, ScopeSelection(
        agent_scope=agent_scope,
        kept_thread_ids=tuple(thread for thread in agent_ids if thread in keep),
        total_agents=len(agent_ids),
    )


def restrict_to_agent_scope(team: TeamData, agent_scope: str) -> TeamData:
    """Return *team* unchanged for ``all``; otherwise only its coordinator threads' records."""

    return select_agent_scope(team, agent_scope)[0]


def _restricted(team: TeamData, keep: frozenset[str]) -> TeamData:
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
