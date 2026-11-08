//! Which of a team's agents a summarize run reads: all of them, or the coordinator alone.
//!
//! A team is a coordinator plus the children it starts inside its own session -- Claude Agent and
//! workflow subagents, Codex-spawned threads. ``all`` (the default) summarizes every one of them.
//! ``coordinator`` summarizes only the team's root session and the sessions that explicitly
//! continue it (``continuation`` edges, which Codex and Orc ingest record from the manifest's
//! ``continuation_sessions``). The coordinator's own transcript still shows what it dispatched
//! and what came back; what drops out is each child's own work, phase by phase.
//!
//! A coordinator-only summary is a different artifact from a full-team one, and the two must
//! never be mistaken for each other or share a cache. So a non-default scope writes its whole
//! summary tree -- caches, phase and rollup projections, artifact catalog, run state, and
//! Markdown -- under ``teams/<slug>/summary_scopes/<scope>/`` instead of ``summary_data/``, and
//! its rollup Markdown says in its heading and first line that child agents are excluded.
//! ``build`` and ``export`` read only ``summary_data/``, so the website stays full-team coverage.
//!
//! A narrowed run says what it kept ("kept N of M agents" and the kept thread ids, on the
//! console, in the run receipt and in ``summarize_state.json``) so a wrong pick cannot pass
//! silently. It refuses a team whose root thread is not among its agents, and it refuses Claude
//! prompt-history teams outright: every session there is its own parentless root with no
//! continuation edges, so nothing identifies a coordinator. A coordinator with no activity in
//! the window is not refused; that is a legitimate answer.

use std::collections::HashSet;

use serde_json::Value;

use crate::json::{as_array, as_int, as_object, as_string};
use crate::team_data::TeamData;

/// Every agent in the team.
pub const ALL_AGENTS: &str = "all";
/// Only the root coordinator session and its declared continuations.
pub const COORDINATOR_ONLY: &str = "coordinator";
/// The known agent scopes.
pub const AGENT_SCOPES: [&str; 2] = [ALL_AGENTS, COORDINATOR_ONLY];

/// The line every coordinator-only rollup carries under its heading.
pub const COORDINATOR_SCOPE_NOTE: &str = "**Scope: coordinator only.** Built from the team's root \
coordinator session and its declared continuation sessions. In-session child agents \
(subagents, spawned threads) are excluded except as the coordinator saw them, so this is \
not full-team coverage.";

/// The provider whose sessions are all parentless roots with no continuation edges.
const UNSCOPABLE_PROVIDERS: [&str; 1] = ["claude-history"];

/// Which of a team's agents a narrowed scope kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeSelection {
    /// The narrowed scope.
    pub agent_scope: String,
    /// Kept thread ids, in team order.
    pub kept_thread_ids: Vec<String>,
    /// Agents in the full team.
    pub total_agents: i64,
}

impl ScopeSelection {
    /// One console line: the scope, the counts, and the kept thread ids.
    pub fn describe(&self) -> String {
        format!(
            "agent scope {}: kept {} of {} agents ({})",
            self.agent_scope,
            self.kept_thread_ids.len(),
            self.total_agents,
            self.kept_thread_ids.join(", ")
        )
    }

    /// The record a receipt and ``summarize_state.json`` carry.
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert(
            "agent_scope".into(),
            Value::String(self.agent_scope.clone()),
        );
        out.insert(
            "kept_agents".into(),
            (self.kept_thread_ids.len() as i64).into(),
        );
        out.insert("total_agents".into(), self.total_agents.into());
        out.insert(
            "kept_thread_ids".into(),
            Value::Array(
                self.kept_thread_ids
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

/// The console line for a recorded selection (see [`ScopeSelection::to_json_obj`]).
pub fn describe_selection(value: &Value) -> Result<String, String> {
    let obj = as_object(value, "agent_scope_selection")?;
    let kept = as_array(
        obj.get("kept_thread_ids").unwrap_or(&Value::Null),
        "agent_scope_selection.kept_thread_ids",
    )?;
    let mut kept_thread_ids = Vec::new();
    for item in kept {
        kept_thread_ids.push(as_string(item, "kept thread id")?.to_string());
    }
    Ok(ScopeSelection {
        agent_scope: as_string(
            obj.get("agent_scope").unwrap_or(&Value::Null),
            "agent_scope_selection.agent_scope",
        )?
        .to_string(),
        kept_thread_ids,
        total_agents: as_int(
            obj.get("total_agents").unwrap_or(&Value::Null),
            "agent_scope_selection.total_agents",
        )?,
    }
    .describe())
}

/// Return *agent_scope* when it names a known scope; error otherwise.
pub fn validate_agent_scope(agent_scope: &str) -> Result<&str, String> {
    if AGENT_SCOPES.contains(&agent_scope) {
        Ok(agent_scope)
    } else {
        Err(format!(
            "unknown agent scope {}; expected one of {}",
            crate::json::py_repr_str(agent_scope),
            AGENT_SCOPES.join(", ")
        ))
    }
}

/// Return the root thread plus every session reachable from it by continuation edges.
pub fn coordinator_thread_ids(team: &TeamData) -> HashSet<String> {
    let mut successors: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for edge in &team.edges {
        if edge.kind == "continuation" {
            successors
                .entry(edge.from_thread_id.as_str())
                .or_default()
                .push(edge.to_thread_id.as_str());
        }
    }
    let mut selected: HashSet<String> = HashSet::from([team.root_thread_id.clone()]);
    let mut pending = vec![team.root_thread_id.as_str()];
    while let Some(current) = pending.pop() {
        for successor in successors.get(current).into_iter().flatten() {
            if selected.insert((*successor).to_string()) {
                pending.push(successor);
            }
        }
    }
    selected
}

/// Return *team* narrowed to *agent_scope* and what was kept (`None` for ``all``).
///
/// Errors for a provider with no identifiable coordinator and for a team whose
/// root thread is not among its agents.
pub fn select_agent_scope(
    team: TeamData,
    agent_scope: &str,
) -> Result<(TeamData, Option<ScopeSelection>), String> {
    validate_agent_scope(agent_scope)?;
    if agent_scope == ALL_AGENTS {
        return Ok((team, None));
    }
    if UNSCOPABLE_PROVIDERS.contains(&team.provider.as_str()) {
        return Err(format!(
            "team {}: --agent-scope {agent_scope} cannot select a coordinator in a {} team: \
             every session is its own parentless root with no continuation edges. \
             Summarize it with --agent-scope all.",
            team.team_slug, team.provider
        ));
    }
    let agent_ids: Vec<&str> = team
        .agents
        .iter()
        .map(|agent| agent.thread_id.as_str())
        .collect();
    if !agent_ids.contains(&team.root_thread_id.as_str()) {
        return Err(format!(
            "team {}: root thread {} is not among its {} agents; refusing to guess which \
             agent is the coordinator",
            team.team_slug,
            team.root_thread_id,
            agent_ids.len()
        ));
    }
    let keep = coordinator_thread_ids(&team);
    let kept_thread_ids: Vec<String> = agent_ids
        .iter()
        .filter(|thread_id| keep.contains(**thread_id))
        .map(|thread_id| (*thread_id).to_string())
        .collect();
    let selection = ScopeSelection {
        agent_scope: agent_scope.to_string(),
        total_agents: agent_ids.len() as i64,
        kept_thread_ids,
    };
    let narrowed = TeamData {
        agents: team
            .agents
            .into_iter()
            .filter(|agent| keep.contains(&agent.thread_id))
            .collect(),
        turns: team
            .turns
            .into_iter()
            .filter(|turn| keep.contains(&turn.thread_id))
            .collect(),
        events: team
            .events
            .into_iter()
            .filter(|event| keep.contains(&event.thread_id))
            .collect(),
        tool_calls: team
            .tool_calls
            .into_iter()
            .filter(|tool| keep.contains(&tool.thread_id))
            .collect(),
        edges: team
            .edges
            .into_iter()
            .filter(|edge| keep.contains(&edge.from_thread_id) && keep.contains(&edge.to_thread_id))
            .collect(),
        ..team
    };
    Ok((narrowed, Some(selection)))
}

/// Return *team* unchanged for ``all``; otherwise only its coordinator threads' records.
pub fn restrict_to_agent_scope(team: TeamData, agent_scope: &str) -> Result<TeamData, String> {
    Ok(select_agent_scope(team, agent_scope)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_data::{Agent, Edge, Event, ToolCall, Turn};

    fn team() -> TeamData {
        let agent = |thread_id: &str| Agent {
            thread_id: thread_id.to_string(),
            ..Agent::default()
        };
        TeamData {
            team_slug: "widget".to_string(),
            provider: "codex".to_string(),
            root_thread_id: "root".to_string(),
            agents: vec![agent("root"), agent("child"), agent("next"), agent("later")],
            turns: ["root", "child", "next"]
                .iter()
                .map(|thread_id| Turn {
                    thread_id: (*thread_id).to_string(),
                    ..Turn::default()
                })
                .collect(),
            events: vec![Event {
                thread_id: "child".to_string(),
                ..Event::default()
            }],
            tool_calls: vec![ToolCall {
                thread_id: "next".to_string(),
                ..ToolCall::default()
            }],
            edges: vec![
                Edge {
                    from_thread_id: "root".to_string(),
                    to_thread_id: "child".to_string(),
                    kind: "spawn".to_string(),
                    ..Edge::default()
                },
                Edge {
                    from_thread_id: "root".to_string(),
                    to_thread_id: "next".to_string(),
                    kind: "continuation".to_string(),
                    ..Edge::default()
                },
                Edge {
                    from_thread_id: "next".to_string(),
                    to_thread_id: "later".to_string(),
                    kind: "continuation".to_string(),
                    ..Edge::default()
                },
            ],
            ..TeamData::default()
        }
    }

    #[test]
    fn coordinator_keeps_root_and_continuations_only() {
        let (narrowed, selection) = select_agent_scope(team(), COORDINATOR_ONLY).unwrap();
        let kept: Vec<&str> = narrowed
            .agents
            .iter()
            .map(|agent| agent.thread_id.as_str())
            .collect();
        assert_eq!(kept, ["root", "next", "later"]);
        assert_eq!(narrowed.turns.len(), 2);
        assert!(narrowed.events.is_empty());
        assert_eq!(narrowed.tool_calls.len(), 1);
        assert_eq!(narrowed.edges.len(), 2);
        let selection = selection.unwrap();
        assert_eq!(
            selection.describe(),
            "agent scope coordinator: kept 3 of 4 agents (root, next, later)"
        );
        assert_eq!(selection.to_json_obj()["kept_agents"], 3);
    }

    #[test]
    fn all_keeps_everything_and_refusals_are_exact() {
        let (same, none) = select_agent_scope(team(), ALL_AGENTS).unwrap();
        assert_eq!(same.agents.len(), 4);
        assert!(none.is_none());

        let mut history = team();
        history.provider = "claude-history".to_string();
        let err = select_agent_scope(history, COORDINATOR_ONLY).unwrap_err();
        assert!(err.contains("cannot select a coordinator"), "{err}");

        let mut rootless = team();
        rootless.root_thread_id = "missing".to_string();
        let err = select_agent_scope(rootless, COORDINATOR_ONLY).unwrap_err();
        assert!(
            err.contains("refusing to guess which agent is the coordinator"),
            "{err}"
        );

        assert!(validate_agent_scope("everyone").is_err());
    }
}
