//! Phase windows at parity with `py/wrkviz/phases.py`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;
use sha2::Digest as _;

use crate::team_data::{Agent, Event, TeamData, ToolCall, Turn};

/// Activity counts for one or more phases.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhaseStats {
    /// User prompts.
    pub user_prompts: i64,
    /// Agent responses.
    pub agent_responses: i64,
    /// Inter-agent messages.
    pub inter_agent_messages: i64,
    /// Tool calls.
    pub tool_calls: i64,
    /// External messages.
    pub external_messages: i64,
}

impl PhaseStats {
    /// Stable archive field names.
    pub fn to_mapping(&self) -> BTreeMap<String, i64> {
        let mut out = BTreeMap::new();
        out.insert("user_prompts".into(), self.user_prompts);
        out.insert("agent_responses".into(), self.agent_responses);
        out.insert(
            "inter_agent_messages".into(),
            self.inter_agent_messages,
        );
        out.insert("tool_calls".into(), self.tool_calls);
        if self.external_messages != 0 {
            out.insert("external_messages".into(), self.external_messages);
        }
        out
    }
}

/// One contiguous activity-state interval.
#[derive(Clone, Debug, PartialEq)]
pub struct StateSegment {
    /// Start ms.
    pub start_ms: i64,
    /// End ms.
    pub end_ms: i64,
    /// Kind.
    pub kind: String,
}

impl StateSegment {
    /// Persisted form.
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("start_ms".into(), self.start_ms.into());
        out.insert("end_ms".into(), self.end_ms.into());
        out.insert("kind".into(), Value::String(self.kind.clone()));
        Value::Object(out)
    }
}

/// One collapsed tool count in a transcript entry.
#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptTool {
    /// Name.
    pub name: String,
    /// Count.
    pub count: i64,
}

/// One timeline transcript item.
#[derive(Clone, Debug)]
pub struct TranscriptEntry {
    /// Timestamp ms.
    pub at_ms: i64,
    /// Role.
    pub role: String,
    /// Text.
    pub text: String,
    /// Tools.
    pub tools: Vec<TranscriptTool>,
}

/// A bounded agent-work interval and its summary material.
#[derive(Clone, Debug)]
pub struct PhaseWindow {
    /// Phase id.
    pub phase_id: String,
    /// Summary key.
    pub summary_key: String,
    /// Agent id.
    pub agent_id: String,
    /// Agent label.
    pub agent_label: String,
    /// Start ms.
    pub start_ms: i64,
    /// End ms.
    pub end_ms: i64,
    /// Stats.
    pub stats: PhaseStats,
    /// States.
    pub states: Vec<StateSegment>,
    /// Transcript text.
    pub transcript_text: String,
    /// Prior context.
    pub prior_context: String,
    /// Transcript entries.
    pub transcript: Vec<TranscriptEntry>,
}

const TEXT_KINDS: [&str; 5] = [
    "user_prompt",
    "assistant_message",
    "inter_agent_message",
    "external_message",
    "goal_updated",
];
const WAIT_TOOLS: [&str; 2] = ["wait", "wait_agent"];

/// Stable human label for an agent.
pub fn agent_label(agent: &Agent) -> String {
    if agent.agent_path == "/root" {
        return "Coordinator".to_string();
    }
    let slug = agent
        .agent_path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    let readable = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    match &agent.nickname {
        Some(nickname) => format!("{readable} · {nickname}"),
        None => readable,
    }
}

fn iso(at_ms: i64) -> String {
    use chrono::TimeZone as _;
    let dt = chrono::Utc.timestamp_millis_opt(at_ms).single().unwrap();
    if dt.timestamp_subsec_micros() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    }
}

fn text_of(event: &Event) -> String {
    if event.content_availability == "encrypted" {
        let mut message_type = "message".to_string();
        for line in event.text.as_deref().unwrap_or_default().lines() {
            if let Some(rest) = line.strip_prefix("Message Type:") {
                let candidate = rest
                    .trim()
                    .to_lowercase()
                    .replace('_', " ");
                if !candidate.is_empty() {
                    message_type = candidate;
                }
                break;
            }
        }
        let route = if event.author.is_some() || event.recipient.is_some() {
            format!(
                " from {} to {}",
                event.author.as_deref().unwrap_or("unknown"),
                event.recipient.as_deref().unwrap_or("unknown")
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

fn tool_count(tool: &ToolCall) -> i64 {
    let nested: i64 = tool.nested_tools.iter().map(|(_, count)| *count).sum();
    if nested != 0 {
        nested
    } else {
        1
    }
}

fn tool_names(tool: &ToolCall) -> Vec<TranscriptTool> {
    if !tool.nested_tools.is_empty() {
        return tool
            .nested_tools
            .iter()
            .map(|(name, count)| TranscriptTool {
                name: name.clone(),
                count: *count,
            })
            .collect();
    }
    vec![TranscriptTool {
        name: tool.name.clone(),
        count: 1,
    }]
}

fn thread_end(
    agent: &Agent,
    events: &[Event],
    tools: &[ToolCall],
    turns: &[Turn],
) -> i64 {
    let mut candidates = vec![agent.started_at_ms + 1000];
    if let Some(end) = agent.ended_at_ms {
        candidates.push(end);
    }
    candidates.extend(events.iter().map(|event| event.timestamp_ms + 1));
    candidates.extend(
        tools
            .iter()
            .map(|tool| (tool.ended_at_ms.unwrap_or(tool.started_at_ms)) + 1),
    );
    candidates.extend(
        turns
            .iter()
            .map(|turn| turn.ended_at_ms.unwrap_or(turn.started_at_ms)),
    );
    candidates.into_iter().max().unwrap()
}

fn blocked_ranges(events: &[Event], end_ms: i64) -> Vec<(i64, i64)> {
    let mut ordered: Vec<&Event> = events.iter().collect();
    ordered.sort_by(|a, b| {
        a.timestamp_ms
            .cmp(&b.timestamp_ms)
            .then(a.event_id.cmp(&b.event_id))
    });
    let mut result = Vec::new();
    for (index, event) in ordered.iter().enumerate() {
        if event.kind != "goal_updated"
            || !event
                .text
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .starts_with("blocked")
        {
            continue;
        }
        let mut stop = end_ms;
        for later in &ordered[index + 1..] {
            if matches!(
                later.kind.as_str(),
                "user_prompt" | "assistant_message" | "goal_updated"
            ) {
                stop = later.timestamp_ms;
                break;
            }
        }
        if stop > event.timestamp_ms {
            result.push((event.timestamp_ms, stop));
        }
    }
    result
}

fn contains(start: i64, end: i64, midpoint: f64) -> bool {
    (start as f64) <= midpoint && midpoint < end as f64
}

fn states(
    start_ms: i64,
    end_ms: i64,
    turns: &[Turn],
    tools: &[ToolCall],
    blocked: &[(i64, i64)],
) -> Vec<StateSegment> {
    let mut points: BTreeSet<i64> = [start_ms, end_ms].into_iter().collect();
    for turn in turns {
        points.insert(turn.started_at_ms.max(start_ms));
        points.insert(turn.ended_at_ms.unwrap_or(end_ms).min(end_ms));
    }
    for tool in tools {
        points.insert(tool.started_at_ms.max(start_ms));
        points.insert(tool.ended_at_ms.unwrap_or(end_ms).min(end_ms));
    }
    for (left, right) in blocked {
        points.insert((*left).max(start_ms));
        points.insert((*right).min(end_ms));
    }
    let boundaries: Vec<i64> = points
        .into_iter()
        .filter(|point| start_ms <= *point && *point <= end_ms)
        .collect();
    let mut raw: Vec<StateSegment> = Vec::new();
    for pair in boundaries.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        if right <= left {
            continue;
        }
        let midpoint = (left as f64 + right as f64) / 2.0;
        let mut kind = "idle";
        if blocked
            .iter()
            .any(|(bl, br)| contains(*bl, *br, midpoint))
        {
            kind = "blocked";
        }
        let matching: Vec<&ToolCall> = tools
            .iter()
            .filter(|tool| {
                contains(
                    tool.started_at_ms,
                    tool.ended_at_ms.unwrap_or(end_ms),
                    midpoint,
                )
            })
            .collect();
        if !matching.is_empty() {
            kind = if matching
                .iter()
                .any(|tool| WAIT_TOOLS.contains(&tool.name.as_str()))
            {
                "waiting"
            } else {
                "tool"
            };
        } else if turns.iter().any(|turn| {
            contains(
                turn.started_at_ms,
                turn.ended_at_ms.unwrap_or(end_ms),
                midpoint,
            )
        }) {
            kind = "active";
        }
        if let Some(previous) = raw.last_mut() {
            if previous.kind == kind && previous.end_ms == left {
                previous.end_ms = right;
                continue;
            }
        }
        raw.push(StateSegment {
            start_ms: left,
            end_ms: right,
            kind: kind.to_string(),
        });
    }
    raw
}

fn transcript_entries(
    events: &[&Event],
    tools: &[&ToolCall],
) -> Vec<TranscriptEntry> {
    enum Record<'a> {
        Event(&'a Event),
        Tool(&'a ToolCall),
    }
    let mut records: Vec<(i64, i64, Record<'_>)> = Vec::new();
    for event in events {
        if TEXT_KINDS.contains(&event.kind.as_str()) {
            records.push((event.timestamp_ms, 1, Record::Event(event)));
        }
    }
    for tool in tools {
        records.push((tool.started_at_ms, 0, Record::Tool(tool)));
    }
    records.sort_by_key(|item| (item.0, item.1));
    let mut result: Vec<TranscriptEntry> = Vec::new();
    let mut pending_at: Option<i64> = None;
    let mut pending: BTreeMap<String, i64> = BTreeMap::new();
    let flush = |pending_at: &mut Option<i64>,
                 pending: &mut BTreeMap<String, i64>,
                 result: &mut Vec<TranscriptEntry>| {
        let Some(at_ms) = *pending_at else { return };
        let tools: Vec<TranscriptTool> = pending
            .iter()
            .map(|(name, count)| TranscriptTool {
                name: name.clone(),
                count: *count,
            })
            .collect();
        let count: i64 = pending.values().sum();
        let noun = if count == 1 { "tool" } else { "tools" };
        result.push(TranscriptEntry {
            at_ms,
            role: "tool".into(),
            text: format!("{count} {noun} used"),
            tools,
        });
        pending.clear();
        *pending_at = None;
    };
    for (at_ms, _, record) in records {
        match record {
            Record::Tool(tool) => {
                if pending_at.is_none() {
                    pending_at = Some(at_ms);
                }
                for tool_name in tool_names(tool) {
                    *pending.entry(tool_name.name).or_default() += tool_name.count;
                }
            }
            Record::Event(event) => {
                flush(&mut pending_at, &mut pending, &mut result);
                let text = text_of(event);
                if text.is_empty() {
                    continue;
                }
                let role = match event.kind.as_str() {
                    "user_prompt" => "user",
                    "assistant_message" => "assistant",
                    "inter_agent_message" => "agent",
                    "external_message" => "external",
                    "goal_updated" => "goal",
                    _ => "event",
                };
                result.push(TranscriptEntry {
                    at_ms,
                    role: role.into(),
                    text,
                    tools: Vec::new(),
                });
            }
        }
    }
    flush(&mut pending_at, &mut pending, &mut result);
    result
}

fn summary_text(entries: &[TranscriptEntry], max_chars: i64) -> String {
    let mut lines = Vec::new();
    for entry in entries {
        if entry.role == "tool" {
            let detail = entry
                .tools
                .iter()
                .map(|tool| format!("{} {}", tool.count, tool.name))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("[{}] TOOLS: {detail}", iso(entry.at_ms)));
        } else {
            lines.push(format!(
                "[{}] {}: {}",
                iso(entry.at_ms),
                entry.role.to_uppercase(),
                entry.text
            ));
        }
    }
    let text = lines.join("\n\n");
    let max = max_chars as usize;
    if crate::summarize_core::char_len(&text) <= max {
        return text;
    }
    let front = max / 3;
    let back = max - front;
    format!(
        "{}\n\n[...middle omitted for summary input...]\n\n{}",
        crate::summarize_core::char_take(&text, front),
        crate::summarize_core::char_tail(&text, back)
    )
}

fn ancestor_ids(
    root_thread_id: &str,
    by_id: &HashMap<String, &Agent>,
    agent: &Agent,
) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = [agent.thread_id.clone()].into_iter().collect();
    let mut parent = agent.parent_thread_id.clone();
    while let Some(parent_id) = parent {
        if !by_id.contains_key(&parent_id) {
            break;
        }
        ids.insert(parent_id.clone());
        parent = by_id[&parent_id].parent_thread_id.clone();
    }
    ids.insert(root_thread_id.to_string());
    ids
}

fn prior_context(
    ancestor_ids: &BTreeSet<String>,
    context_by_thread: &HashMap<String, Vec<(i64, String, String)>>,
    start_ms: i64,
    max_chars: i64,
) -> String {
    type Candidate<'a> = (&'a Vec<(i64, String, String)>, usize);
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for thread_id in ancestor_ids {
        let Some(entries) = context_by_thread.get(thread_id) else {
            continue;
        };
        let cutoff = entries.partition_point(|item| (item.0, item.1.as_str()) < (start_ms, ""));
        if cutoff > 0 {
            candidates.push((entries, cutoff - 1));
        }
    }
    let mut reverse_lines: Vec<String> = Vec::new();
    let mut retained_chars = 0usize;
    while !candidates.is_empty() && retained_chars < max_chars as usize {
        let selected_index = candidates
            .iter()
            .enumerate()
            .max_by(|a, b| {
                let key_a = (
                    a.1 .0[a.1 .1].0,
                    a.1 .0[a.1 .1].1.clone(),
                );
                let key_b = (
                    b.1 .0[b.1 .1].0,
                    b.1 .0[b.1 .1].1.clone(),
                );
                key_a.cmp(&key_b).then(b.0.cmp(&a.0))
            })
            .map(|item| item.0)
            .unwrap();
        let (entries, entry_index) = candidates[selected_index];
        reverse_lines.push(entries[entry_index].2.clone());
        retained_chars += crate::summarize_core::char_len(reverse_lines.last().unwrap());
        if reverse_lines.len() > 1 {
            retained_chars += 2;
        }
        if entry_index == 0 {
            candidates.remove(selected_index);
        } else {
            candidates[selected_index] = (entries, entry_index - 1);
        }
    }
    let text = reverse_lines
        .iter()
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    crate::summarize_core::char_tail(&text, max_chars as usize)
}

fn in_window(team: &TeamData, timestamp_ms: i64) -> bool {
    if let Some(start) = team.window_start_ms {
        if timestamp_ms < start {
            return false;
        }
    }
    match team.window_end_ms {
        None => true,
        Some(end) => timestamp_ms < end,
    }
}

fn agent_overlaps_window(team: &TeamData, agent: &Agent) -> bool {
    if let Some(end) = team.window_end_ms {
        if agent.started_at_ms >= end {
            return false;
        }
    }
    if team.window_start_ms.is_none() || agent.ended_at_ms.is_none() {
        return true;
    }
    let effective_end = agent
        .ended_at_ms
        .unwrap()
        .max(agent.started_at_ms + 1);
    effective_end > team.window_start_ms.unwrap()
}

/// Agents alive in the window plus their lineage ancestors.
pub fn phase_agent_ids(
    team: &TeamData,
    phases: &[PhaseWindow],
) -> BTreeSet<String> {
    let by_id: HashMap<String, &Agent> = team
        .agents
        .iter()
        .map(|agent| (agent.thread_id.clone(), agent))
        .collect();
    let mut selected: BTreeSet<String> = team
        .agents
        .iter()
        .filter(|agent| agent_overlaps_window(team, agent))
        .map(|agent| agent.thread_id.clone())
        .collect();
    selected.extend(phases.iter().map(|phase| phase.agent_id.clone()));
    let mut pending: Vec<String> = selected.iter().cloned().collect();
    while let Some(current) = pending.pop() {
        let Some(agent) = by_id.get(&current) else {
            continue;
        };
        if let Some(parent) = &agent.parent_thread_id {
            if by_id.contains_key(parent) && selected.insert(parent.clone()) {
                pending.push(parent.clone());
            }
        }
    }
    selected
}

/// Partition activity into fixed, append-stable UTC windows.
pub fn build_phases(
    team: &TeamData,
    phase_minutes: i64,
    context_chars: i64,
    transcript_chars: i64,
) -> Result<Vec<PhaseWindow>, String> {
    if phase_minutes <= 0 {
        return Err("phase_minutes must be positive".to_string());
    }
    if context_chars <= 0 {
        return Err("context_chars must be positive".to_string());
    }
    if transcript_chars <= 0 {
        return Err("transcript_chars must be positive".to_string());
    }
    let window_ms = phase_minutes * 60 * 1000;
    let mut events_by_thread: HashMap<String, Vec<Event>> = HashMap::new();
    let mut tools_by_thread: HashMap<String, Vec<ToolCall>> = HashMap::new();
    let mut turns_by_thread: HashMap<String, Vec<Turn>> = HashMap::new();
    for event in &team.events {
        events_by_thread
            .entry(event.thread_id.clone())
            .or_default()
            .push(event.clone());
    }
    for tool in &team.tool_calls {
        tools_by_thread
            .entry(tool.thread_id.clone())
            .or_default()
            .push(tool.clone());
    }
    for turn in &team.turns {
        turns_by_thread
            .entry(turn.thread_id.clone())
            .or_default()
            .push(turn.clone());
    }
    for events in events_by_thread.values_mut() {
        events.sort_by(|a, b| {
            a.timestamp_ms
                .cmp(&b.timestamp_ms)
                .then(a.event_id.cmp(&b.event_id))
        });
    }
    for tools in tools_by_thread.values_mut() {
        tools.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then(a.call_id.cmp(&b.call_id))
        });
    }
    for turns in turns_by_thread.values_mut() {
        turns.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then(a.turn_id.cmp(&b.turn_id))
        });
    }

    let mut context_by_thread: HashMap<String, Vec<(i64, String, String)>> = HashMap::new();
    for event in &team.events {
        let rendered = text_of(event);
        if !TEXT_KINDS.contains(&event.kind.as_str()) || rendered.is_empty() {
            continue;
        }
        context_by_thread
            .entry(event.thread_id.clone())
            .or_default()
            .push((
                event.timestamp_ms,
                event.event_id.clone(),
                format!("[{}] {}: {}", iso(event.timestamp_ms), event.kind, rendered),
            ));
    }
    for entries in context_by_thread.values_mut() {
        entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    let agents_by_id: HashMap<String, &Agent> = team
        .agents
        .iter()
        .map(|agent| (agent.thread_id.clone(), agent))
        .collect();

    let mut result: Vec<PhaseWindow> = Vec::new();
    for agent in &team.agents {
        let empty_events: Vec<Event> = Vec::new();
        let empty_tools: Vec<ToolCall> = Vec::new();
        let empty_turns: Vec<Turn> = Vec::new();
        let own_events = events_by_thread.get(&agent.thread_id).unwrap_or(&empty_events);
        let own_tools = tools_by_thread.get(&agent.thread_id).unwrap_or(&empty_tools);
        let own_turns = turns_by_thread.get(&agent.thread_id).unwrap_or(&empty_turns);
        let mut end_ms = thread_end(agent, own_events, own_tools, own_turns);
        if let Some(window_end) = team.window_end_ms {
            end_ms = end_ms.min(window_end);
        }
        let ancestors = ancestor_ids(&team.root_thread_id, &agents_by_id, agent);
        let mut activity_times: Vec<i64> = own_events
            .iter()
            .filter(|event| {
                TEXT_KINDS.contains(&event.kind.as_str()) && in_window(team, event.timestamp_ms)
            })
            .map(|event| event.timestamp_ms)
            .collect();
        activity_times.extend(
            own_tools
                .iter()
                .filter(|tool| in_window(team, tool.started_at_ms))
                .map(|tool| tool.started_at_ms),
        );
        if activity_times.is_empty() {
            continue;
        }
        let mut bucket_starts: Vec<i64> = activity_times
            .iter()
            .map(|at_ms| at_ms.div_euclid(window_ms) * window_ms)
            .collect();
        bucket_starts.sort_unstable();
        bucket_starts.dedup();
        let blocked = blocked_ranges(own_events, end_ms);
        for bucket in bucket_starts {
            let start_ms = agent
                .started_at_ms
                .max(bucket)
                .max(team.window_start_ms.unwrap_or(agent.started_at_ms));
            let mut phase_end = end_ms.min(bucket + window_ms);
            if phase_end <= start_ms {
                phase_end = start_ms + 1000;
            }
            let phase_events: Vec<&Event> = own_events
                .iter()
                .filter(|event| start_ms <= event.timestamp_ms && event.timestamp_ms < phase_end)
                .collect();
            let phase_tools: Vec<&ToolCall> = own_tools
                .iter()
                .filter(|tool| {
                    (start_ms <= tool.started_at_ms && tool.started_at_ms < phase_end)
                        || (tool.started_at_ms < start_ms
                            && tool.ended_at_ms.unwrap_or(phase_end) > start_ms)
                })
                .collect();
            let phase_turns: Vec<Turn> = own_turns
                .iter()
                .filter(|turn| {
                    turn.started_at_ms < phase_end
                        && turn.ended_at_ms.unwrap_or(phase_end) > start_ms
                })
                .cloned()
                .collect();
            let started_phase_tools: Vec<&ToolCall> = phase_tools
                .iter()
                .copied()
                .filter(|tool| start_ms <= tool.started_at_ms && tool.started_at_ms < phase_end)
                .collect();
            let entries = transcript_entries(&phase_events, &started_phase_tools);
            let mut counts: HashMap<&str, i64> = HashMap::new();
            for event in &phase_events {
                *counts.entry(event.kind.as_str()).or_default() += 1;
            }
            let stats = PhaseStats {
                user_prompts: *counts.get("user_prompt").unwrap_or(&0),
                agent_responses: *counts.get("assistant_message").unwrap_or(&0),
                inter_agent_messages: *counts.get("inter_agent_message").unwrap_or(&0),
                tool_calls: started_phase_tools.iter().map(|tool| tool_count(tool)).sum(),
                external_messages: *counts.get("external_message").unwrap_or(&0),
            };
            let stable = format!(
                "{}\0{}\0{}\0{}",
                team.team_slug, agent.thread_id, bucket, phase_minutes
            );
            let suffix: String = sha2::Sha256::digest(stable.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .chars()
                .take(16)
                .collect();
            let phase_id = format!("phase-{suffix}");
            result.push(PhaseWindow {
                phase_id: phase_id.clone(),
                summary_key: phase_id,
                agent_id: agent.thread_id.clone(),
                agent_label: agent_label(agent),
                start_ms,
                end_ms: phase_end,
                stats,
                states: states(
                    start_ms,
                    phase_end,
                    &phase_turns,
                    &phase_tools.iter().copied().cloned().collect::<Vec<_>>(),
                    &blocked,
                ),
                transcript_text: summary_text(&entries, transcript_chars),
                prior_context: prior_context(
                    &ancestors,
                    &context_by_thread,
                    start_ms,
                    context_chars,
                ),
                transcript: entries,
            });
        }
    }
    result.sort_by(|a, b| {
        a.start_ms
            .cmp(&b.start_ms)
            .then(a.agent_id.cmp(&b.agent_id))
            .then(a.phase_id.cmp(&b.phase_id))
    });
    Ok(result)
}

/// Sum activity counters across phases.
pub fn aggregate_stats(phases: &[PhaseWindow]) -> PhaseStats {
    let mut out = PhaseStats::default();
    for phase in phases {
        out.user_prompts += phase.stats.user_prompts;
        out.agent_responses += phase.stats.agent_responses;
        out.inter_agent_messages += phase.stats.inter_agent_messages;
        out.tool_calls += phase.stats.tool_calls;
        out.external_messages += phase.stats.external_messages;
    }
    out
}
