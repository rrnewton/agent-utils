//! Owner-only `!` commands the chat bridge answers itself (#917 chat-bang-commands).
//!
//! A message whose text starts with `!` from a permanent owner is never typed into an
//! agent's pane. The bridge answers `!help` and `!who` itself, and `!@NAME` opens a
//! Slack thread bound to that agent: later owner messages in the thread are admitted
//! as requests for that agent, not for the bridge's own target. The registry is read
//! as plain `agent.json` documents, the same evidence `agentctl tree` renders, so the
//! bridge needs no Herdr client to answer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The help text every `!help` (and every unrecognised `!` command) posts.
pub(crate) const HELP_TEXT: &str = "Owner commands (handled by the bridge, never sent to an agent):\n\
!help — this list\n\
!who — the sub-agents of this coordinator: the managed-by tree, Pinned (persistent lanes) first, then Ephemeral\n\
!who NAME — one agent in detail\n\
!@NAME <message> — open a new thread bound to NAME and send it <message>; later messages in that thread go to NAME\n\
!end — in a bound thread, end the binding; later messages there go to the coordinator\n\
Talk to the coordinator by writing normally.";

/// The outbound operation ID for one bridge-posted `!` answer: deterministic from the
/// command message, so a provider replay reuses it and the helper dedupes the post.
pub(crate) fn bang_operation_id(channel_id: &str, message_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"chat-bang\0");
    hasher.update(channel_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(message_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// One parsed `!` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Bang {
    /// `!help`, a bare `!`, or an unrecognised `!` command: post [`HELP_TEXT`].
    Help,
    /// `!who` or `!who NAME`.
    Who(Option<String>),
    /// `!@NAME` or `!@NAME <message>`: open a thread bound to that agent.
    At(String, Option<String>),
    /// `!end`: end this thread's binding.
    End,
}

/// One Slack thread bound to an agent (#917). Stored in `thread-bindings.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThreadBinding {
    /// Provider channel the thread lives in.
    pub channel_id: String,
    /// Provider thread ID (the `!@` message's own thread).
    pub thread_id: String,
    /// Bound agent's registered name when bound.
    pub agent_name: String,
    /// Bound agent's generation token when bound.
    pub agent_token: String,
    /// The `!@` message that opened the thread.
    pub root_message_id: String,
    /// When the binding opened, Unix milliseconds.
    pub opened_at_millis: u64,
    /// `active` or `ended`.
    pub state: String,
    /// Why it ended, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_reason: Option<String>,
    /// Deterministic agentctl queue ID of the parent notice.
    pub parent_notice_id: String,
    /// Whether the parent notice has been queued.
    pub parent_notified: bool,
}

impl ThreadBinding {
    /// Whether owner messages in this thread route to the bound agent.
    pub fn is_active(&self) -> bool {
        self.state == "active"
    }
}

/// The parent notice for one new bound thread (#917): sent once, by queue ID.
pub(crate) fn parent_notice_body(binding: &ThreadBinding, bridge_state_dir: &Path) -> String {
    format!(
        "[chat-bridge] The owner is talking directly to {} in a bound Slack thread.\n\
         You are not in that loop. Catch up any time:\n  agentctl read {}\n  \
         agentctl chat thread --bridge-state {} --thread {}\n\
         This notice is sent once per bound thread.",
        binding.agent_name,
        binding.agent_name,
        bridge_state_dir.display(),
        binding.thread_id,
    )
}

/// The deterministic queue ID of `binding`'s parent notice.
pub(crate) fn parent_notice_id(thread_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(thread_id.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("chat-bind-{hex}")
}

/// Parse `text` as a `!` command, or `None` when it is an ordinary message. Only a `!` as
/// the very first character counts, so quoted or forwarded text never triggers.
pub(crate) fn parse_bang(text: &str) -> Option<Bang> {
    let trimmed = text.trim_end();
    if !trimmed.starts_with('!') {
        return None;
    }
    let mut words = trimmed.split_whitespace();
    let first = words.next();
    if let Some(at) = first.and_then(|word| word.strip_prefix("!@")) {
        if agent_queue::registry::is_agent_name(at) {
            let message = trimmed[first.map_or(0, str::len)..].trim_start().to_owned();
            return Some(Bang::At(
                at.to_owned(),
                (!message.is_empty()).then_some(message),
            ));
        }
        return Some(Bang::Help);
    }
    match first {
        Some("!end") => {
            if words.next().is_some() {
                return Some(Bang::Help);
            }
            Some(Bang::End)
        }
        Some("!who") => {
            let name = words.next().map(str::to_owned);
            if words.next().is_some() {
                return Some(Bang::Help);
            }
            match name {
                Some(name) if agent_queue::registry::is_agent_name(&name) => {
                    Some(Bang::Who(Some(name)))
                }
                Some(_) => Some(Bang::Help),
                None => Some(Bang::Who(None)),
            }
        }
        _ => Some(Bang::Help),
    }
}

/// One agent as `!who` renders it, read from its `agent.json`.
#[derive(Clone, Debug)]
pub(crate) struct BangAgent {
    pub name: String,
    pub token: String,
    pub lifecycle: String,
    pub harness: String,
    pub model: String,
    pub profile: Option<String>,
    pub cwd: Option<String>,
    pub pane_id: Option<String>,
    pub created_at: Option<f64>,
    pub managed_by: Option<(String, String)>,
    pub created_by: Option<String>,
    pub former_tokens: Vec<String>,
    pub pinned: bool,
    pub pinned_source: String,
    pub pinned_explicit: Option<bool>,
    pub parent: Option<String>,
}

/// The registry as one `!who` answer sees it.
#[derive(Clone, Debug)]
pub(crate) struct BangSnapshot {
    pub coordinator: Option<String>,
    pub host: String,
    pub agents: Vec<BangAgent>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn edge_agent_token(value: &Value, key: &str) -> Option<(String, String)> {
    let edge = value.get(key)?;
    Some((text(edge, "agent")?, text(edge, "token")?))
}

/// Read the registry at `registry` for a `!who` answer. Unreadable records are skipped,
/// as `agentctl tree` skips them.
pub(crate) fn snapshot(registry: &Path) -> BangSnapshot {
    let host = crate::subagents::local_hostname()
        .map(|name| name.split('.').next().unwrap_or(&name).to_owned())
        .unwrap_or_else(|| "this host".to_owned());
    let coordinator = std::fs::read_to_string(registry.join("coordinator.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|document| text(&document, "name"));
    let profiles: BTreeMap<String, Option<String>> =
        std::fs::read_to_string(registry.join("profiles.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|document| {
                document.get("profiles")?.as_object().map(|profiles| {
                    profiles
                        .iter()
                        .map(|(name, profile)| (name.clone(), text(profile, "model")))
                        .collect()
                })
            })
            .unwrap_or_default();
    let restore_agents = crate::subagents::restore_agent_names(registry);
    let names = agent_queue::registry::names(registry).unwrap_or_default();
    let mut agents: Vec<BangAgent> = Vec::new();
    for name in &names {
        let Ok(directory) = agent_queue::registry::directory(registry, name) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(directory.join("agent.json")) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let profile = text(&record, "profile");
        let model = text(&record, "model")
            .or_else(|| {
                profile
                    .as_ref()
                    .and_then(|name| profiles.get(name).cloned().flatten())
            })
            .unwrap_or_else(|| "unknown".to_owned());
        agents.push(BangAgent {
            name: name.clone(),
            token: text(&record, "token").unwrap_or_default(),
            lifecycle: text(&record, "lifecycle").unwrap_or_else(|| "unknown".to_owned()),
            harness: text(&record, "harness").unwrap_or_else(|| "unknown".to_owned()),
            model,
            profile,
            cwd: text(&record, "cwd"),
            pane_id: text(&record, "pane_id"),
            created_at: record.get("created_at").and_then(Value::as_f64),
            managed_by: edge_agent_token(&record, "managed_by"),
            created_by: edge_agent_token(&record, "created_by").map(|(agent, _)| agent),
            former_tokens: record
                .get("former_tokens")
                .and_then(Value::as_array)
                .map(|tokens| {
                    tokens
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            pinned: false,
            pinned_source: "none".to_owned(),
            pinned_explicit: record.get("pinned").and_then(Value::as_bool),
            parent: None,
        });
    }
    // Parents resolve by token, so a rename or revive keeps the edge (as in control::tree).
    let by_token: BTreeMap<String, String> = agents
        .iter()
        .flat_map(|agent| {
            std::iter::once((agent.token.clone(), agent.name.clone())).chain(
                agent
                    .former_tokens
                    .iter()
                    .map(|token| (token.clone(), agent.name.clone())),
            )
        })
        .filter(|(token, _)| !token.is_empty())
        .collect();
    for agent in &mut agents {
        let parent = agent
            .managed_by
            .as_ref()
            .and_then(|(_, token)| by_token.get(token).cloned())
            .filter(|parent| parent != &agent.name);
        agent.parent = parent;
    }
    if let Some(root) = coordinator.clone() {
        for agent in &mut agents {
            if agent.parent.is_none() && agent.name != root {
                agent.parent = Some(root.clone());
            }
        }
    }
    let has_children: BTreeSet<String> = agents
        .iter()
        .filter_map(|agent| agent.parent.clone())
        .collect();
    for agent in &mut agents {
        let (pinned, source) = if let Some(value) = agent.pinned_explicit {
            (value, "explicit")
        } else if coordinator.as_deref() == Some(agent.name.as_str()) {
            (true, "coordinator")
        } else if restore_agents.contains(&agent.name) {
            (true, "restore")
        } else if has_children.contains(&agent.name) {
            (true, "manager")
        } else {
            (false, "none")
        };
        agent.pinned = pinned;
        agent.pinned_source = source.to_owned();
    }
    BangSnapshot {
        coordinator,
        host,
        agents,
    }
}

const MAX_WHO_AGENTS: usize = 50;

/// Render `!who`: the managed-by tree under the designated coordinator, Pinned first.
pub(crate) fn render_who(snapshot: &BangSnapshot) -> String {
    let mut out = String::new();
    let root_name = snapshot
        .coordinator
        .clone()
        .unwrap_or_else(|| "(no coordinator designated)".to_owned());
    out.push_str(&format!(
        "Sub-agents of {root_name} (host {}) — {} agent(s)\n",
        snapshot.host,
        snapshot.agents.len()
    ));
    if snapshot.agents.is_empty() {
        out.push_str("(no agents registered)\n");
        return out;
    }
    let children: BTreeMap<String, Vec<String>> = {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for agent in &snapshot.agents {
            if let Some(parent) = &agent.parent {
                map.entry(parent.clone())
                    .or_default()
                    .push(agent.name.clone());
            }
        }
        for list in map.values_mut() {
            list.sort();
        }
        map
    };
    let by_name: BTreeMap<&str, &BangAgent> = snapshot
        .agents
        .iter()
        .map(|agent| (agent.name.as_str(), agent))
        .collect();
    let mut shown = 0_usize;
    #[allow(clippy::too_many_arguments)]
    fn walk(
        name: &str,
        depth: usize,
        snapshot: &BangSnapshot,
        by_name: &BTreeMap<&str, &BangAgent>,
        children: &BTreeMap<String, Vec<String>>,
        seen: &mut BTreeSet<String>,
        lines: &mut Vec<String>,
        shown: &mut usize,
    ) {
        if *shown >= MAX_WHO_AGENTS || !seen.insert(name.to_owned()) {
            return;
        }
        *shown += 1;
        if let Some(agent) = by_name.get(name) {
            let manager = match &agent.parent {
                Some(parent)
                    if depth == 0 || snapshot.coordinator.as_deref() != Some(parent.as_str()) =>
                {
                    format!(" — managed by {parent}")
                }
                _ => String::new(),
            };
            let _ = snapshot;
            lines.push(format!(
                "{}{} — {}/{} — {}{}",
                "  ".repeat(depth),
                agent.name,
                agent.harness,
                agent.model,
                agent.lifecycle,
                manager
            ));
        }
        if let Some(kids) = children.get(name) {
            for kid in kids {
                walk(
                    kid,
                    depth + 1,
                    snapshot,
                    by_name,
                    children,
                    seen,
                    lines,
                    shown,
                );
            }
        }
    }
    // Section order follows the tree: pinned subtrees first, then ephemeral ones.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut ordered_roots: Vec<&BangAgent> = snapshot
        .agents
        .iter()
        .filter(|agent| {
            agent.parent.is_none() || snapshot.coordinator.as_deref() == Some(agent.name.as_str())
        })
        .collect();
    ordered_roots.sort_by_key(|agent| agent.name.clone());
    if let Some(root) = &snapshot.coordinator {
        ordered_roots.sort_by_key(|agent| agent.name != *root);
    }
    let mut pinned_lines: Vec<String> = Vec::new();
    let mut ephemeral_lines: Vec<String> = Vec::new();
    for root in &ordered_roots {
        let mut local: Vec<String> = Vec::new();
        let mut local_shown = 0_usize;
        walk(
            &root.name,
            0,
            snapshot,
            &by_name,
            &children,
            &mut seen,
            &mut local,
            &mut local_shown,
        );
        if root.pinned {
            pinned_lines.extend(local);
        } else {
            ephemeral_lines.extend(local);
        }
    }
    // Agents whose parent chain never reached a root (a cycle cut) still appear.
    let mut rest: Vec<&BangAgent> = snapshot
        .agents
        .iter()
        .filter(|agent| !seen.contains(&agent.name))
        .collect();
    rest.sort_by_key(|agent| agent.name.clone());
    for agent in rest {
        if shown >= MAX_WHO_AGENTS {
            break;
        }
        shown += 1;
        let line = format!(
            "{} — {}/{} — {} — managed by {}",
            agent.name,
            agent.harness,
            agent.model,
            agent.lifecycle,
            agent.parent.as_deref().unwrap_or("?")
        );
        if agent.pinned {
            pinned_lines.push(line);
        } else {
            ephemeral_lines.push(line);
        }
    }
    out.push_str("Pinned (persistent lanes):\n");
    if pinned_lines.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for line in &pinned_lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("Ephemeral:\n");
    if ephemeral_lines.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for line in &ephemeral_lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    if snapshot.agents.len() > MAX_WHO_AGENTS {
        out.push_str(&format!(
            "… and {} more — !who NAME for one agent\n",
            snapshot.agents.len() - MAX_WHO_AGENTS
        ));
    }
    out.push_str("Details: !who NAME · Help: !help");
    out
}

/// Render `!who NAME`, or the unknown-name answer with near matches.
pub(crate) fn render_who_one(snapshot: &BangSnapshot, name: &str) -> String {
    let Some(agent) = snapshot.agents.iter().find(|agent| agent.name == name) else {
        let names: Vec<&str> = snapshot
            .agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect();
        let near = near_matches(&names, name);
        let mut out = format!("No agent named {name:?} is registered here.\n");
        if !near.is_empty() {
            out.push_str(&format!("Near matches: {}\n", near.join(", ")));
        }
        out.push_str(&render_who(snapshot));
        return out;
    };
    let mut out = format!(
        "{} — {}/{} — {}\n",
        agent.name, agent.harness, agent.model, agent.lifecycle
    );
    out.push_str(&format!(
        "pinned: {} ({})\n",
        if agent.pinned { "yes" } else { "no" },
        agent.pinned_source
    ));
    if let Some((manager, _)) = &agent.managed_by {
        out.push_str(&format!("managed by: {manager}\n"));
    } else if let Some(parent) = &agent.parent {
        out.push_str(&format!("managed by: {parent} (designated coordinator)\n"));
    } else {
        out.push_str("managed by: (none recorded)\n");
    }
    if let Some(creator) = &agent.created_by {
        out.push_str(&format!("created by: {creator}\n"));
    }
    if let Some(profile) = &agent.profile {
        out.push_str(&format!("profile: {profile}\n"));
    }
    if let Some(cwd) = &agent.cwd {
        out.push_str(&format!("cwd: {cwd}\n"));
    }
    if let Some(pane) = &agent.pane_id {
        out.push_str(&format!("pane: {pane}\n"));
    }
    if let Some(created) = agent.created_at {
        out.push_str(&format!("created_at: {created}\n"));
    }
    out.push_str(&format!("host: {}\n", snapshot.host));
    out.push_str(&format!(
        "Inspect: agentctl read {} · agentctl status {}",
        agent.name, agent.name
    ));
    out
}

/// Names close to `query`: prefix matches, then substring, then edit distance at most 2.
pub(crate) fn near_matches(names: &[&str], query: &str) -> Vec<String> {
    let mut scored: Vec<(u8, String)> = Vec::new();
    for name in names {
        let rank = if name.starts_with(query) {
            0
        } else if name.contains(query) {
            1
        } else if levenshtein(name, query) <= 2 {
            2
        } else {
            continue;
        };
        scored.push((rank, (*name).to_owned()));
    }
    scored.sort();
    scored.into_iter().take(5).map(|(_, name)| name).collect()
}

fn levenshtein(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, b) in right.iter().enumerate() {
            current.push(std::cmp::min(
                std::cmp::min(current[j] + 1, previous[j + 1] + 1),
                previous[j] + usize::from(a != b),
            ));
        }
        previous = current;
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_leading_bang_parses() {
        assert_eq!(parse_bang("!help"), Some(Bang::Help));
        assert_eq!(parse_bang("!who"), Some(Bang::Who(None)));
        assert_eq!(
            parse_bang("!who mu-bang"),
            Some(Bang::Who(Some("mu-bang".to_owned())))
        );
        assert_eq!(parse_bang("!who Bad Name"), Some(Bang::Help));
        assert_eq!(parse_bang("hello !who"), None);
        assert_eq!(parse_bang(" !who"), None);
        assert_eq!(
            parse_bang("!@worker hello\nthere"),
            Some(Bang::At(
                "worker".to_owned(),
                Some("hello\nthere".to_owned())
            ))
        );
        assert_eq!(
            parse_bang("!@worker"),
            Some(Bang::At("worker".to_owned(), None))
        );
        assert_eq!(parse_bang("!@Bad Name hi"), Some(Bang::Help));
        assert_eq!(parse_bang("!end"), Some(Bang::End));
        assert_eq!(parse_bang("!end now"), Some(Bang::Help));
    }

    #[test]
    fn near_matches_prefers_prefixes() {
        let names = ["coordb", "mu-bang", "mu-sq", "hands"];
        assert_eq!(near_matches(&names, "mu-"), vec!["mu-bang", "mu-sq"]);
        assert_eq!(near_matches(&names, "hand"), vec!["hands"]);
        assert!(near_matches(&names, "zzzz").is_empty());
    }
}
