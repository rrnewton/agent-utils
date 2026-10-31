//! Transcript search records at parity with `py/wrkviz/search_index.py`.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::team_data::{Event, TeamData};

/// Search record schema version.
pub const SEARCH_RECORD_SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
struct PromptLink {
    at_ms: i64,
    source_line: i64,
    event_id: String,
    author_kind: Option<String>,
}

struct AgentRoutes {
    aliases_by_thread: HashMap<String, BTreeSet<String>>,
    threads_by_alias: HashMap<String, BTreeSet<String>>,
    parent_by_thread: HashMap<String, Option<String>>,
    lifetime_by_thread: HashMap<String, (i64, Option<i64>)>,
}

fn message_ref(team_slug: &str, event_id: &str) -> String {
    format!("message:{team_slug}::{event_id}")
}

fn tool_ref(team_slug: &str, call_id: &str) -> String {
    format!("tool:{team_slug}::{call_id}")
}

fn agent_ref(team_slug: &str, thread_id: &str) -> String {
    format!("agent:{team_slug}::{thread_id}")
}

fn agent_id(team_slug: &str, thread_id: &str, namespace_agents: bool) -> String {
    if namespace_agents {
        format!("{team_slug}::{thread_id}")
    } else {
        thread_id.to_string()
    }
}

/// Transcript text with the encrypted placeholder (shared shape).
pub fn event_text(event: &Event) -> String {
    if event.content_availability == "encrypted" {
        let mut message_type = "message".to_string();
        for line in event.text.as_deref().unwrap_or_default().lines() {
            if let Some(rest) = line.strip_prefix("Message Type:") {
                let candidate = rest.trim().to_lowercase().replace('_', " ");
                if !candidate.is_empty() {
                    message_type = candidate;
                }
                break;
            }
        }
        let route = if event.author.as_deref().is_some_and(|item| !item.is_empty())
            || event
                .recipient
                .as_deref()
                .is_some_and(|item| !item.is_empty())
        {
            format!(
                " from {} to {}",
                event
                    .author
                    .as_deref()
                    .filter(|item| !item.is_empty())
                    .unwrap_or("unknown"),
                event
                    .recipient
                    .as_deref()
                    .filter(|item| !item.is_empty())
                    .unwrap_or("unknown")
            )
        } else {
            String::new()
        };
        return format!(
            "[Encrypted Codex collaboration {message_type}{route}; message body unavailable offline.]"
        );
    }
    event
        .text
        .as_deref()
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn tool_text(tool: &crate::team_data::ToolCall) -> String {
    let (parts, count) = if !tool.nested_tools.is_empty() {
        let mut nested = tool.nested_tools.clone();
        nested.sort();
        let count: i64 = nested.iter().map(|(_, value)| *value).sum();
        (
            nested
                .iter()
                .map(|(name, value)| format!("{value} {name}"))
                .collect::<Vec<_>>(),
            count,
        )
    } else {
        let qualified = match &tool.namespace {
            Some(namespace) if !namespace.is_empty() => format!("{namespace}.{}", tool.name),
            _ => tool.name.clone(),
        };
        (vec![format!("1 {qualified}")], 1)
    };
    let noun = if count == 1 { "tool" } else { "tools" };
    format!("{count} {noun} used: {}", parts.join(", "))
}

fn agent_routes(team: &TeamData) -> AgentRoutes {
    let continuation_prefixes: Vec<String> = team
        .agents
        .iter()
        .filter(|agent| {
            team.provider == "codex"
                && agent.parent_thread_id.is_some()
                && agent.role.as_deref() == Some("coordinator")
                && agent.agent_path.starts_with("/root/continuation-")
        })
        .map(|agent| agent.agent_path.trim_end_matches('/').to_string())
        .collect();
    let mut aliases_by_thread: HashMap<String, BTreeSet<String>> = HashMap::new();
    for agent in &team.agents {
        let mut aliases: BTreeSet<String> = [agent.thread_id.clone(), agent.agent_path.clone()]
            .into_iter()
            .collect();
        for prefix in &continuation_prefixes {
            if agent.agent_path == *prefix {
                aliases.insert("/root".to_string());
            } else if let Some(rest) = agent.agent_path.strip_prefix(&format!("{prefix}/")) {
                aliases.insert(format!("/root/{rest}"));
            }
        }
        aliases_by_thread.insert(agent.thread_id.clone(), aliases);
    }
    let mut threads_by_alias: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (thread_id, aliases) in &aliases_by_thread {
        for alias in aliases {
            threads_by_alias
                .entry(alias.clone())
                .or_default()
                .insert(thread_id.clone());
        }
    }
    AgentRoutes {
        aliases_by_thread,
        threads_by_alias,
        parent_by_thread: team
            .agents
            .iter()
            .map(|agent| (agent.thread_id.clone(), agent.parent_thread_id.clone()))
            .collect(),
        lifetime_by_thread: team
            .agents
            .iter()
            .map(|agent| {
                (
                    agent.thread_id.clone(),
                    (agent.started_at_ms, agent.ended_at_ms),
                )
            })
            .collect(),
    }
}

fn resolved_route(routes: &AgentRoutes, event: &Event) -> (Option<String>, Option<String>) {
    let empty: BTreeSet<String> = BTreeSet::new();
    let authors = routes
        .threads_by_alias
        .get(event.author.as_deref().unwrap_or_default())
        .unwrap_or(&empty);
    let recipients = routes
        .threads_by_alias
        .get(event.recipient.as_deref().unwrap_or_default())
        .unwrap_or(&empty);
    let mut direct: Vec<(String, String)> = Vec::new();
    for author in authors {
        for recipient in recipients {
            let parent_of = |thread: &str| {
                routes
                    .parent_by_thread
                    .get(thread)
                    .cloned()
                    .unwrap_or_default()
            };
            if parent_of(author).as_deref() == Some(recipient.as_str())
                || parent_of(recipient).as_deref() == Some(author.as_str())
            {
                direct.push((author.clone(), recipient.clone()));
            }
        }
    }
    let on_event_thread: Vec<(String, String)> = direct
        .iter()
        .filter(|pair| pair.0 == event.thread_id || pair.1 == event.thread_id)
        .cloned()
        .collect();
    let candidates = if on_event_thread.is_empty() {
        direct
    } else {
        on_event_thread
    };
    if !candidates.is_empty() {
        let score = |pair: &(String, String)| -> (i64, i64, String, String) {
            let mut starts = Vec::new();
            let mut active = 0i64;
            for thread_id in [&pair.0, &pair.1] {
                let (started, ended) = routes.lifetime_by_thread[thread_id.as_str()];
                starts.push(started);
                if started <= event.timestamp_ms
                    && (ended.is_none() || event.timestamp_ms <= ended.unwrap())
                {
                    active += 1;
                }
            }
            (
                active,
                *starts.iter().max().unwrap(),
                pair.0.clone(),
                pair.1.clone(),
            )
        };
        let best = candidates
            .iter()
            .max_by(|a, b| score(a).cmp(&score(b)))
            .unwrap();
        return (Some(best.0.clone()), Some(best.1.clone()));
    }
    (
        if authors.len() == 1 {
            authors.iter().next().cloned()
        } else {
            None
        },
        if recipients.len() == 1 {
            recipients.iter().next().cloned()
        } else {
            None
        },
    )
}

fn message_type(event: &Event) -> Option<String> {
    for line in event.text.as_deref().unwrap_or_default().lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (label, value) = line.split_once(':')?;
        if label.trim().to_lowercase() == "message type" {
            return Some(value.trim().to_lowercase().replace(['-', ' '], "_"));
        }
        return None;
    }
    None
}

fn inter_agent_record_type(routes: &AgentRoutes, event: &Event) -> String {
    let (author_thread, recipient_thread) = resolved_route(routes, event);
    let parent_of = |thread: &str| {
        routes
            .parent_by_thread
            .get(thread)
            .cloned()
            .unwrap_or_default()
    };
    if let (Some(author), Some(recipient)) = (&author_thread, &recipient_thread) {
        if parent_of(recipient).as_deref() == Some(author.as_str()) {
            return "inter_agent_prompt".to_string();
        }
        if parent_of(author).as_deref() == Some(recipient.as_str()) {
            return "inter_agent_response".to_string();
        }
    }
    let kind = message_type(event);
    if event.phase.as_deref() == Some("instruction")
        || matches!(kind.as_deref(), Some("new_task" | "followup_task"))
    {
        return "inter_agent_prompt".to_string();
    }
    if event.phase.as_deref() == Some("final_answer") || kind.as_deref() == Some("final_answer") {
        return "inter_agent_response".to_string();
    }
    let default_aliases: BTreeSet<String> = [event.thread_id.clone()].into_iter().collect();
    let aliases = routes
        .aliases_by_thread
        .get(&event.thread_id)
        .unwrap_or(&default_aliases);
    let author_is_thread = event
        .author
        .as_ref()
        .is_some_and(|item| aliases.contains(item));
    let recipient_is_thread = event
        .recipient
        .as_ref()
        .is_some_and(|item| aliases.contains(item));
    if recipient_is_thread && !author_is_thread {
        return "inter_agent_prompt".to_string();
    }
    if author_is_thread && !recipient_is_thread {
        return "inter_agent_response".to_string();
    }
    "inter_agent".to_string()
}

fn record_type(routes: &AgentRoutes, event: &Event) -> String {
    if event.kind == "inter_agent_message" {
        return inter_agent_record_type(routes, event);
    }
    match event.kind.as_str() {
        "user_prompt" => "prompt".into(),
        "assistant_message" => "response".into(),
        "external_message" => "external".into(),
        "goal_updated" => "goal".into(),
        "system_input" => "system".into(),
        other => other.to_string(),
    }
}

fn record_author_kind(event: &Event, record_type: &str) -> Option<String> {
    if matches!(
        record_type,
        "inter_agent" | "inter_agent_prompt" | "inter_agent_response"
    ) {
        return Some(
            event
                .author_kind
                .clone()
                .unwrap_or_else(|| "agent".to_string()),
        );
    }
    event.author_kind.clone()
}

fn event_role(event: &Event) -> String {
    match event.kind.as_str() {
        "user_prompt" => return "user".into(),
        "assistant_message" => return "assistant".into(),
        "inter_agent_message" => return "agent".into(),
        "external_message" => return "external".into(),
        "goal_updated" => return "goal".into(),
        "system_input" => return "system".into(),
        _ => {}
    }
    if event.kind.starts_with("subagent_") {
        return "agent".into();
    }
    match event.role.as_deref() {
        Some(role @ ("user" | "assistant" | "agent" | "system" | "external" | "goal")) => {
            role.to_string()
        }
        _ => "event".into(),
    }
}

type PromptLists = (
    HashMap<(String, String), Vec<PromptLink>>,
    HashMap<String, Vec<PromptLink>>,
    HashMap<(String, String), Vec<PromptLink>>,
);

fn prompt_lists(team: &TeamData, routes: &AgentRoutes) -> PromptLists {
    let mut by_turn: HashMap<(String, String), Vec<PromptLink>> = HashMap::new();
    let mut by_thread: HashMap<String, Vec<PromptLink>> = HashMap::new();
    let mut by_route: HashMap<(String, String), Vec<PromptLink>> = HashMap::new();
    let mut events: Vec<&Event> = team.events.iter().collect();
    events.sort_by(|a, b| {
        a.timestamp_ms
            .cmp(&b.timestamp_ms)
            .then(a.source_line.cmp(&b.source_line))
            .then(a.event_id.cmp(&b.event_id))
    });
    for event in events {
        let kind = record_type(routes, event);
        if !matches!(kind.as_str(), "prompt" | "inter_agent_prompt") {
            continue;
        }
        let link = PromptLink {
            at_ms: event.timestamp_ms,
            source_line: event.source_line,
            event_id: event.event_id.clone(),
            author_kind: record_author_kind(event, &kind),
        };
        by_thread
            .entry(event.thread_id.clone())
            .or_default()
            .push(link.clone());
        if let Some(turn_id) = &event.turn_id {
            by_turn
                .entry((event.thread_id.clone(), turn_id.clone()))
                .or_default()
                .push(link.clone());
        }
        if let Some(route) = prompt_route(routes, event, &kind) {
            by_route.entry(route).or_default().push(link);
        }
    }
    for value in by_turn.values_mut() {
        value.sort_by(|a, b| {
            a.at_ms
                .cmp(&b.at_ms)
                .then(a.source_line.cmp(&b.source_line))
                .then(a.event_id.cmp(&b.event_id))
        });
    }
    for value in by_thread.values_mut() {
        value.sort_by(|a, b| {
            a.at_ms
                .cmp(&b.at_ms)
                .then(a.source_line.cmp(&b.source_line))
                .then(a.event_id.cmp(&b.event_id))
        });
    }
    for value in by_route.values_mut() {
        value.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then(a.event_id.cmp(&b.event_id)));
    }
    (by_turn, by_thread, by_route)
}

fn prompt_route(
    routes: &AgentRoutes,
    event: &Event,
    record_type: &str,
) -> Option<(String, String)> {
    let (author_thread, recipient_thread) = resolved_route(routes, event);
    let (author, recipient) = (author_thread?, recipient_thread?);
    let parent_of = |thread: &str| {
        routes
            .parent_by_thread
            .get(thread)
            .cloned()
            .unwrap_or_default()
    };
    if record_type == "inter_agent_prompt"
        && parent_of(&recipient).as_deref() == Some(author.as_str())
    {
        return Some((author, recipient));
    }
    if record_type == "inter_agent_response"
        && parent_of(&author).as_deref() == Some(recipient.as_str())
    {
        return Some((recipient, author));
    }
    None
}

fn record_agent_thread(
    routes: &AgentRoutes,
    event: &Event,
    record_type: &str,
    visible_agent_ids: &BTreeSet<String>,
) -> String {
    let (author_thread, recipient_thread) = resolved_route(routes, event);
    let candidate = match record_type {
        "inter_agent_prompt" => recipient_thread,
        "inter_agent_response" => author_thread,
        _ => None,
    };
    if let Some(candidate) = candidate {
        if visible_agent_ids.contains(&candidate) {
            return candidate;
        }
    }
    event.thread_id.clone()
}

#[allow(clippy::too_many_arguments)]
fn linked_prompt(
    thread_id: &str,
    turn_id: Option<&str>,
    at_ms: i64,
    source_line: i64,
    stable_id: &str,
    by_turn: &HashMap<(String, String), Vec<PromptLink>>,
    by_thread: &HashMap<String, Vec<PromptLink>>,
    route: Option<&(String, String)>,
    by_route: Option<&HashMap<(String, String), Vec<PromptLink>>>,
) -> Option<PromptLink> {
    let empty: Vec<PromptLink> = Vec::new();
    let candidates: &Vec<PromptLink> = if let (Some(route), Some(by_route)) = (route, by_route) {
        by_route.get(route).unwrap_or(&empty)
    } else if let Some(turn_id) = turn_id {
        by_turn
            .get(&(thread_id.to_string(), turn_id.to_string()))
            .unwrap_or(&empty)
    } else {
        by_thread.get(thread_id).unwrap_or(&empty)
    };
    for prompt in candidates.iter().rev() {
        if route.is_some() {
            if prompt.at_ms < at_ms {
                return Some(prompt.clone());
            }
            continue;
        }
        if (prompt.at_ms, prompt.source_line, prompt.event_id.as_str())
            <= (at_ms, source_line, stable_id)
        {
            return Some(prompt.clone());
        }
    }
    None
}

fn in_scope(at_ms: i64, start_ms: Option<i64>, end_ms: Option<i64>) -> bool {
    (start_ms.is_none() || at_ms >= start_ms.unwrap())
        && (end_ms.is_none() || at_ms < end_ms.unwrap())
}

/// Build unique searchable transcript records.
pub fn build_search_records(
    team: &TeamData,
    visible_agent_ids: &BTreeSet<String>,
    namespace_agents: bool,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> Result<Vec<Value>, String> {
    if let (Some(start), Some(end)) = (start_ms, end_ms) {
        if start >= end {
            return Err("search record start must be earlier than end".to_string());
        }
    }
    let agents: HashMap<&str, &crate::team_data::Agent> = team
        .agents
        .iter()
        .map(|agent| (agent.thread_id.as_str(), agent))
        .collect();
    let routes = agent_routes(team);
    let (by_turn, by_thread, by_route) = prompt_lists(team, &routes);
    let mut sortable: Vec<(i64, i64, String, Value)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for event in &team.events {
        let role = event_role(event);
        if !visible_agent_ids.contains(&event.thread_id)
            || !in_scope(event.timestamp_ms, start_ms, end_ms)
        {
            continue;
        }
        let text = event_text(event);
        if text.is_empty() {
            continue;
        }
        let reference = message_ref(&team.team_slug, &event.event_id);
        if !seen.insert(reference.clone()) {
            return Err(format!("duplicate transcript search record {reference}"));
        }
        let event_record_type = record_type(&routes, event);
        let event_author_kind = record_author_kind(event, &event_record_type);
        let route = prompt_route(&routes, event, &event_record_type);
        let record_thread_id =
            record_agent_thread(&routes, event, &event_record_type, visible_agent_ids);
        let prompt = if matches!(event_record_type.as_str(), "prompt" | "inter_agent_prompt") {
            Some(PromptLink {
                at_ms: event.timestamp_ms,
                source_line: event.source_line,
                event_id: event.event_id.clone(),
                author_kind: event_author_kind.clone(),
            })
        } else {
            linked_prompt(
                &event.thread_id,
                event.turn_id.as_deref(),
                event.timestamp_ms,
                event.source_line,
                &event.event_id,
                &by_turn,
                &by_thread,
                route.as_ref(),
                Some(&by_route),
            )
        };
        let agent = agents.get(record_thread_id.as_str());
        let mut record = serde_json::Map::new();
        let opt = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        record.insert("schema_version".into(), SEARCH_RECORD_SCHEMA_VERSION.into());
        record.insert("ref".into(), Value::String(reference.clone()));
        record.insert(
            "record_type".into(),
            Value::String(event_record_type.clone()),
        );
        record.insert("role".into(), Value::String(role));
        record.insert("team".into(), Value::String(team.team_slug.clone()));
        record.insert(
            "agent_id".into(),
            Value::String(agent_id(
                &team.team_slug,
                &record_thread_id,
                namespace_agents,
            )),
        );
        record.insert(
            "agent_ref".into(),
            Value::String(agent_ref(&team.team_slug, &record_thread_id)),
        );
        record.insert(
            "agent_path".into(),
            Value::String(
                agent
                    .map(|item| item.agent_path.clone())
                    .unwrap_or_else(|| record_thread_id.clone()),
            ),
        );
        record.insert("event_id".into(), Value::String(event.event_id.clone()));
        record.insert("turn_id".into(), opt(&event.turn_id));
        record.insert("at_ms".into(), event.timestamp_ms.into());
        record.insert("text".into(), Value::String(text));
        record.insert("author_kind".into(), opt(&event_author_kind));
        record.insert("ingress_kind".into(), opt(&event.ingress_kind));
        record.insert("author".into(), opt(&event.author));
        record.insert("recipient".into(), opt(&event.recipient));
        record.insert("phase".into(), opt(&event.phase));
        record.insert(
            "prompt_ref".into(),
            prompt.as_ref().map_or(Value::Null, |item| {
                Value::String(message_ref(&team.team_slug, &item.event_id))
            }),
        );
        record.insert(
            "prompt_author_kind".into(),
            prompt.as_ref().map_or(Value::Null, |item| {
                item.author_kind.clone().map_or(Value::Null, Value::String)
            }),
        );
        record.insert(
            "prompt_at_ms".into(),
            prompt
                .as_ref()
                .map_or(Value::Null, |item| item.at_ms.into()),
        );
        record.insert(
            "prompt_in_scope".into(),
            Value::Bool(
                prompt
                    .as_ref()
                    .is_some_and(|item| in_scope(item.at_ms, start_ms, end_ms)),
            ),
        );
        record.insert(
            "content_fidelity".into(),
            Value::String(
                if event.content_availability == "encrypted" {
                    "encrypted-placeholder"
                } else {
                    "verbatim"
                }
                .into(),
            ),
        );
        sortable.push((event.timestamp_ms, 1, reference, Value::Object(record)));
    }

    for tool in &team.tool_calls {
        if !visible_agent_ids.contains(&tool.thread_id)
            || !in_scope(tool.started_at_ms, start_ms, end_ms)
        {
            continue;
        }
        let reference = tool_ref(&team.team_slug, &tool.call_id);
        if !seen.insert(reference.clone()) {
            return Err(format!("duplicate transcript search record {reference}"));
        }
        let prompt = linked_prompt(
            &tool.thread_id,
            tool.turn_id.as_deref(),
            tool.started_at_ms,
            tool.source_line,
            &tool.call_id,
            &by_turn,
            &by_thread,
            None,
            None,
        );
        let agent = agents.get(tool.thread_id.as_str());
        let mut record = serde_json::Map::new();
        record.insert("schema_version".into(), SEARCH_RECORD_SCHEMA_VERSION.into());
        record.insert("ref".into(), Value::String(reference.clone()));
        record.insert("record_type".into(), Value::String("tool".into()));
        record.insert("role".into(), Value::String("tool".into()));
        record.insert("team".into(), Value::String(team.team_slug.clone()));
        record.insert(
            "agent_id".into(),
            Value::String(agent_id(&team.team_slug, &tool.thread_id, namespace_agents)),
        );
        record.insert(
            "agent_ref".into(),
            Value::String(agent_ref(&team.team_slug, &tool.thread_id)),
        );
        record.insert(
            "agent_path".into(),
            Value::String(
                agent
                    .map(|item| item.agent_path.clone())
                    .unwrap_or_else(|| tool.thread_id.clone()),
            ),
        );
        record.insert("event_id".into(), Value::String(tool.call_id.clone()));
        record.insert(
            "turn_id".into(),
            tool.turn_id.clone().map_or(Value::Null, Value::String),
        );
        record.insert("at_ms".into(), tool.started_at_ms.into());
        record.insert("text".into(), Value::String(tool_text(tool)));
        record.insert("author_kind".into(), Value::String("agent".into()));
        record.insert("ingress_kind".into(), Value::String("tool".into()));
        record.insert(
            "prompt_ref".into(),
            prompt.as_ref().map_or(Value::Null, |item| {
                Value::String(message_ref(&team.team_slug, &item.event_id))
            }),
        );
        record.insert(
            "prompt_author_kind".into(),
            prompt.as_ref().map_or(Value::Null, |item| {
                item.author_kind.clone().map_or(Value::Null, Value::String)
            }),
        );
        record.insert(
            "prompt_at_ms".into(),
            prompt
                .as_ref()
                .map_or(Value::Null, |item| item.at_ms.into()),
        );
        record.insert(
            "prompt_in_scope".into(),
            Value::Bool(
                prompt
                    .as_ref()
                    .is_some_and(|item| in_scope(item.at_ms, start_ms, end_ms)),
            ),
        );
        record.insert("content_fidelity".into(), Value::String("condensed".into()));
        sortable.push((tool.started_at_ms, 0, reference, Value::Object(record)));
    }

    sortable.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    Ok(sortable.into_iter().map(|item| item.3).collect())
}
