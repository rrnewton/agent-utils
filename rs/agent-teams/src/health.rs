//! Delivery-target health (#721 agent-teams-coordinator-handover): is the
//! agent a declaration delivers to actually the live root coordinator that
//! can receive the message?
//!
//! A declaration that pins an agent name goes stale the day that agent is
//! retired: hand-overs keep succeeding into its unread queue while every
//! listing shows the new coordinator as live. These checks compare the
//! declaration's delivery target against the registry's own control tree
//! (the designated coordinator, and `managed_by` up to its root) so `check`,
//! `accept` and `send` can refuse loudly instead.

use crate::model::{Declaration, Designation};
use crate::proof::{agent_record, identity_alive, managed_by_chain, registry_coordinator_name};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One health finding about a declaration's delivery target.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// Stable flag kind, for example `target-missing`.
    pub kind: &'static str,
    /// What is wrong, in one line.
    pub detail: String,
    /// The command that fixes it.
    pub fix: String,
}

/// Health of one declaration's delivery target.
#[derive(Debug, Clone)]
pub struct TargetHealth {
    /// Resolved delivery agent name, when one could be named.
    pub target: Option<String>,
    /// The registry's designated coordinator, when designated.
    pub designated: Option<String>,
    /// Every red flag found, in check order.
    pub flags: Vec<Flag>,
    /// Why a hand-over to the target can never reach anyone, when refusing.
    pub undeliverable_reason: Option<String>,
}

impl TargetHealth {
    /// The flag that makes a hand-over undeliverable (missing or archived
    /// target in a populated registry), if any. Other flags warn; these refuse.
    pub fn undeliverable(&self) -> Option<&Flag> {
        self.undeliverable_reason.as_ref()?;
        self.flags
            .iter()
            .find(|flag| matches!(flag.kind, "target-missing" | "target-archived"))
    }
}

/// Why handing a message to `agent` in `registry` can never reach anyone:
/// the agent is archived, or it has no record in a registry that otherwise
/// has agents or a designation. An empty registry is not refused: a queue
/// directory is legitimately created ahead of the agent's registration.
/// Shared by `accept`'s deliverer and the sender-side pre-flight (#721).
pub fn undeliverable_reason(registry: &Path, agent: &str) -> Option<String> {
    if agent_archived(registry, agent) {
        return Some(format!(
            "delivery target {agent:?} is archived in registry {}",
            registry.display()
        ));
    }
    if agent_exists(registry, agent) {
        return None;
    }
    // A real agentctl registry: it has a designation, an `agents/` tree, or
    // an archive. Anything else is a scratch queue root created ahead of
    // registration, where absence is not evidence of a dead target.
    let populated = registry.join("coordinator.json").exists()
        || agent_queue::registry::agents_directory(registry).is_dir()
        || registry.join("archive").is_dir();
    if populated {
        return Some(format!(
            "delivery target {agent:?} has no agent record in registry {}",
            registry.display()
        ));
    }
    None
}

/// The registry an `agentctl-queue` declaration delivers through.
pub fn registry_of(declaration: &Declaration) -> Option<PathBuf> {
    declaration
        .deliverer
        .get("registry")
        .and_then(Value::as_str)
        .map(crate::jsonfile::expand_home)
}

/// The agent name a declaration pins in its deliverer, if any.
pub fn pinned_agent(declaration: &Declaration) -> Option<String> {
    declaration
        .deliverer
        .get("agent")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// Whether `agent` has a record in `registry` (either layout).
pub fn agent_exists(registry: &Path, agent: &str) -> bool {
    agent_queue::registry::names(registry)
        .map(|names| names.iter().any(|name| name == agent))
        .unwrap_or(false)
}

/// Whether a stopped record for `agent` sits in the registry archive
/// (entries are named `NAME-TOKEN`, so the name is a prefix).
pub fn agent_archived(registry: &Path, agent: &str) -> bool {
    let prefix = format!("{agent}-");
    std::fs::read_dir(registry.join("archive")).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
    })
}

/// The registry's designated coordinator name, read from the designation
/// file (`REGISTRY/coordinator.json`, kept current by `coordinator set` and
/// rename) with `agentctl coordinator show` as fallback. File-first keeps
/// `check` subprocess-free per declaration.
pub fn designated_name(registry: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(registry.join("coordinator.json")) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                return Some(name.to_string());
            }
        }
    }
    registry_coordinator_name(registry)
}

/// The `managed_by` agent recorded for `agent`, read from its record even
/// when the record carries no pinned process identity.
pub fn managed_by_of(registry: &Path, agent: &str) -> Option<String> {
    if let Some((_, parent)) = agent_record(registry, agent) {
        return parent;
    }
    let directory = agent_queue::registry::directory(registry, agent).ok()?;
    let text = std::fs::read_to_string(directory.join("agent.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("managed_by")
        .and_then(|edge| edge.get("agent"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn flag(kind: &'static str, detail: String, fix: String) -> Flag {
    Flag { kind, detail, fix }
}

/// Evaluate one declaration's delivery target against its registry's
/// control tree. Non-queue deliverers have no agent target; only a pinned
/// designation's liveness is checked for them.
pub fn evaluate(declaration: &Declaration) -> TargetHealth {
    let mut flags = Vec::new();
    let team = &declaration.team;
    if declaration.deliverer_kind != "agentctl-queue" {
        if let Designation::Pinned { .. } = declaration.designation {
            let registry = registry_of(declaration);
            let live =
                crate::proof::coordinator_identity(&declaration.designation, registry.as_deref())
                    .is_some_and(|identity| identity_alive(&identity));
            if !live {
                flags.push(flag(
                    "target-not-live",
                    format!("team {team}: the pinned coordinator process is not live"),
                    format!("re-run `agent-teams init {team} --registry <registry>` from the new coordinator, so the declaration names the registry instead of a pinned process"),
                ));
            }
        }
        return TargetHealth {
            target: None,
            designated: None,
            flags,
            undeliverable_reason: None,
        };
    }
    let Some(registry) = registry_of(declaration) else {
        flags.push(flag(
            "target-missing",
            format!("team {team}: agentctl-queue declaration names no registry"),
            format!("re-run `agent-teams init {team} --registry <registry>`"),
        ));
        return TargetHealth {
            target: None,
            designated: None,
            flags,
            undeliverable_reason: None,
        };
    };
    let designated = designated_name(&registry);
    let pinned = pinned_agent(declaration);
    if let Some(agent) = &pinned {
        flags.push(flag(
            "pinned-agent",
            format!("team {team}: declaration pins deliverer agent {agent:?}; a pinned name goes stale when that agent is retired"),
            format!("re-run `agent-teams init {team} --registry {}` so delivery resolves the registry's designated coordinator at send time", registry.display()),
        ));
    }
    let target = pinned.clone().or_else(|| designated.clone());
    let Some(target_name) = target.clone() else {
        flags.push(flag(
            "target-missing",
            format!(
                "team {team}: registry {} has no designated coordinator to deliver to",
                registry.display()
            ),
            "run `agentctl coordinator set NEW` in that registry, then `agent-teams check`"
                .to_string(),
        ));
        return TargetHealth {
            target: None,
            designated,
            flags,
            undeliverable_reason: Some(format!(
                "registry {} has no designated coordinator to deliver to",
                registry.display()
            )),
        };
    };
    // Designation agreement.
    if let (Some(agent), Some(root)) = (&pinned, &designated) {
        if agent != root {
            flags.push(flag(
                "target-not-designated",
                format!("team {team}: declaration delivers to {agent:?} but the registry's designated coordinator is {root:?}"),
                format!("re-run `agent-teams init {team} --registry {}` to drop the pinned name, or run `agentctl coordinator set {agent}` if {agent} should coordinate", registry.display()),
            ));
        }
    }
    // Existence, archive, liveness.
    let exists = agent_exists(&registry, &target_name);
    if !exists && agent_archived(&registry, &target_name) {
        flags.push(flag(
            "target-archived",
            format!("team {team}: delivery target {target_name:?} is archived (stopped and retired) in {}", registry.display()),
            format!("run `agentctl coordinator set NEW` to designate a live coordinator, then re-run `agent-teams init {team} --registry {}` if this declaration pinned the old name", registry.display()),
        ));
    } else if !exists {
        flags.push(flag(
            "target-missing",
            format!("team {team}: delivery target {target_name:?} has no agent record in {}", registry.display()),
            "run `agentctl coordinator set NEW` to designate a live coordinator, then `agent-teams check`".to_string(),
        ));
    } else if let Some((identity, _)) = agent_record(&registry, &target_name) {
        if !identity_alive(&identity) {
            flags.push(flag(
                "target-not-live",
                format!("team {team}: delivery target {target_name:?} is not live (its pinned harness process is gone)"),
                format!("revive {target_name} (`agentctl revive {target_name}`) or designate its successor: `agentctl coordinator set NEW`"),
            ));
        }
    }
    // Control-tree shape: the target must be the root, and the tree's root
    // must be the designated coordinator.
    if let Some(parent) = managed_by_of(&registry, &target_name) {
        flags.push(flag(
            "target-is-child",
            format!("team {team}: delivery target {target_name:?} is managed by {parent:?}; it is a child tab, not the root coordinator"),
            format!("designate the root instead: `agentctl coordinator set {}` and re-run `agent-teams init {team} --registry {}`", managed_by_chain(&registry, &target_name).last().cloned().unwrap_or_default(), registry.display()),
        ));
    }
    let chain = managed_by_chain(&registry, &target_name);
    if let Some(root) = chain.last() {
        if designated.as_deref() != Some(root.as_str()) {
            let root_live = agent_record(&registry, root)
                .is_some_and(|(identity, _)| identity_alive(&identity));
            if root_live {
                flags.push(flag(
                    "tree-root-mismatch",
                    format!("team {team}: the root of {target_name:?}'s managed-by tree is {root:?}, a live agent that is not the designated coordinator"),
                    format!("run `agentctl coordinator set {root}` if that root should coordinate, or re-point the declaration: `agent-teams init {team} --registry {}`", registry.display()),
                ));
            }
        }
    }
    if let Some(root) = &designated {
        if target_name != *root || pinned.is_none() {
            if let Some(parent) = managed_by_of(&registry, root) {
                flags.push(flag(
                    "designated-not-root",
                    format!("team {team}: designated coordinator {root:?} is managed by {parent:?}; a coordinator must be the root of its control tree"),
                    format!("run `agentctl manage {root} --clear` so it answers to nobody, or designate its root: `agentctl coordinator set {parent}`"),
                ));
            }
        }
    }
    let reason = undeliverable_reason(&registry, &target_name);
    TargetHealth {
        target: Some(target_name),
        designated,
        flags,
        undeliverable_reason: reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_declaration;
    use serde_json::json;
    use std::io::Write;

    fn fixture(agents: &[(&str, Option<&str>, bool)], designated: Option<&str>) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agent-teams-health-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let me = crate::proof::identity_of(u64::from(std::process::id())).expect("self identity");
        for (name, parent, live) in agents {
            let agent_dir = dir.join("agents").join(name);
            std::fs::create_dir_all(&agent_dir).expect("agent dir");
            let mut identity = serde_json::to_value(&me).expect("identity json");
            if !live {
                identity["starttime_ticks"] = json!(me.starttime_ticks + 999_999);
            }
            let record = json!({
                "name": name,
                "harness_identity": identity,
                "managed_by": parent.map(|p| json!({"agent": p, "registry": dir.to_string_lossy(), "token": "t"})),
            });
            let mut file = std::fs::File::create(agent_dir.join("agent.json")).expect("record");
            file.write_all(record.to_string().as_bytes())
                .expect("record body");
        }
        if let Some(name) = designated {
            std::fs::write(
                dir.join("coordinator.json"),
                format!("{{\"name\": \"{name}\"}}"),
            )
            .expect("designation");
        }
        dir
    }

    fn declaration(registry: &Path, pinned: Option<&str>) -> Declaration {
        let mut deliverer =
            json!({"kind": "agentctl-queue", "registry": registry.to_string_lossy()});
        if let Some(agent) = pinned {
            deliverer["agent"] = json!(agent);
        }
        parse_declaration(json!({
            "schema": "agent-teams-declaration/v1", "team": "tools",
            "deliverer": deliverer,
            "coordinator": {"designation": "registry"},
            "accepts_from": [], "limits": {},
        }))
        .expect("declaration")
    }

    fn kinds(health: &TargetHealth) -> Vec<&'static str> {
        health.flags.iter().map(|flag| flag.kind).collect()
    }

    #[test]
    fn healthy_root_coordinator_has_no_flags() {
        let dir = fixture(&[("coord", None, true)], Some("coord"));
        let health = evaluate(&declaration(&dir, None));
        assert_eq!(health.target.as_deref(), Some("coord"));
        assert!(health.flags.is_empty(), "flags: {:?}", health.flags);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinned_dead_agent_flags_pinned_not_designated_and_not_live() {
        let dir = fixture(
            &[("coord", None, true), ("old", None, false)],
            Some("coord"),
        );
        let health = evaluate(&declaration(&dir, Some("old")));
        let found = kinds(&health);
        assert!(found.contains(&"pinned-agent"), "{found:?}");
        assert!(found.contains(&"target-not-designated"), "{found:?}");
        assert!(found.contains(&"target-not-live"), "{found:?}");
        assert!(health.undeliverable().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_target_is_undeliverable() {
        let dir = fixture(&[("coord", None, true)], Some("coord"));
        let health = evaluate(&declaration(&dir, Some("ghost")));
        assert!(
            kinds(&health).contains(&"target-missing"),
            "{:?}",
            health.flags
        );
        assert!(health.undeliverable().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archived_target_is_undeliverable() {
        let dir = fixture(&[("coord", None, true)], Some("coord"));
        let archive = dir.join("archive");
        std::fs::create_dir_all(&archive).expect("archive");
        std::fs::write(archive.join("old-tok123"), "{}").expect("archived record");
        let health = evaluate(&declaration(&dir, Some("old")));
        assert!(
            kinds(&health).contains(&"target-archived"),
            "{:?}",
            health.flags
        );
        assert!(health.undeliverable().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn child_target_and_designated_not_root_flag() {
        let dir = fixture(
            &[("root", None, true), ("child", Some("root"), true)],
            Some("child"),
        );
        let health = evaluate(&declaration(&dir, None));
        let found = kinds(&health);
        assert!(found.contains(&"target-is-child"), "{found:?}");
        assert!(found.contains(&"designated-not-root"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_root_mismatch_flags_live_other_root() {
        // Designated coord is a child of live root; target resolves to coord.
        let dir = fixture(
            &[("root", None, true), ("coord", Some("root"), true)],
            Some("coord"),
        );
        let health = evaluate(&declaration(&dir, None));
        let found = kinds(&health);
        assert!(
            found.contains(&"tree-root-mismatch"),
            "{found:?} {:?}",
            health.flags
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_designated_coordinator_flags_missing() {
        let dir = fixture(&[("coord", None, true)], None);
        let health = evaluate(&declaration(&dir, None));
        assert!(
            kinds(&health).contains(&"target-missing"),
            "{:?}",
            health.flags
        );
        assert!(health.undeliverable().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
