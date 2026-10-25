//! The per-host coordinator index, read side: discovery of every
//! designated coordinator on this host. `agentctl coordinator set/clear`
//! maintains it (schema `agentctl-coordinator-index/v1`, one file per
//! registry under the host state home); this module only reads it. The
//! index is a discovery cache - the registry's own designation stays the
//! authority, so a stale or missing entry degrades to "not listed", never
//! to a wrong answer.

use crate::proof::identity_alive;
use agent_queue::identity::ProcessIdentity;
use serde_json::Value;
use std::path::PathBuf;

/// One designated coordinator as the host index records it.
#[derive(Debug, Clone)]
pub struct HostCoordinator {
    /// Absolute registry path.
    pub registry: PathBuf,
    /// Designated coordinator's current name in that registry.
    pub name: String,
    /// The agent-teams team whose declaration names this registry, if any.
    pub team: Option<String>,
    /// The harness working directory (the registry's parent).
    pub harness_cwd: PathBuf,
    /// Git origin URLs of the harness's checkout(s), when found.
    pub repo_origins: Vec<String>,
    /// Recorded harness identity, when the coordinator had one.
    pub identity: Option<ProcessIdentity>,
    /// Harness identity JSON as stored (for display).
    pub identity_json: Option<Value>,
}

/// The index directory, mirroring the paths `agentctl` writes.
pub fn index_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENTCTL_COORDINATOR_INDEX_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state).join("agentctl/coordinators");
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("~"), PathBuf::from);
    home.join(".local/state/agentctl/coordinators")
}

/// Every readable index entry, ordered by team then name. Unreadable or
/// malformed files are skipped: discovery must never fail a command.
pub fn load() -> Vec<HostCoordinator> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(index_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        out.push(parse(&document));
    }
    out.sort_by(|a, b| {
        a.team
            .as_deref()
            .unwrap_or("")
            .cmp(b.team.as_deref().unwrap_or(""))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn parse(document: &Value) -> HostCoordinator {
    let text = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let identity_json = document.get("harness_identity").cloned();
    let identity = identity_json
        .as_ref()
        .and_then(|value| serde_json::from_value::<ProcessIdentity>(value.clone()).ok());
    HostCoordinator {
        registry: PathBuf::from(text("registry")),
        name: text("name"),
        team: document
            .get("team")
            .and_then(Value::as_str)
            .filter(|team| !team.is_empty())
            .map(str::to_owned),
        harness_cwd: PathBuf::from(text("harness_cwd")),
        repo_origins: document
            .get("repo_origins")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        identity,
        identity_json,
    }
}

/// Whether the recorded harness is still the same live process.
pub fn live(entry: &HostCoordinator) -> bool {
    entry.identity.as_ref().is_some_and(identity_alive)
}

/// The live designated coordinator for `team` on this host, if the index
/// names one.
pub fn live_coordinator_for_team(team: &str) -> Option<HostCoordinator> {
    load()
        .into_iter()
        .find(|entry| entry.team.as_deref() == Some(team) && live(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_keeps_discovery_fields() {
        let entry = parse(&json!({
            "schema": "agentctl-coordinator-index/v1",
            "registry": "/work/harness/.agentctl",
            "name": "coordinator",
            "token": "abc",
            "harness_cwd": "/work/harness",
            "repo_origins": ["https://example.invalid/org/repo"],
            "team": "tools",
            "harness_identity": null,
        }));
        assert_eq!(entry.team.as_deref(), Some("tools"));
        assert_eq!(entry.name, "coordinator");
        assert_eq!(entry.repo_origins.len(), 1);
        assert!(!live(&entry), "no identity is never live");
    }
}
