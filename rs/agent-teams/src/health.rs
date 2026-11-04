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
//!
//! Designation re-roots the tree: once an agent is the designated
//! coordinator it answers to nobody for these checks, whatever `managed_by`
//! its record still shows (the edge stays as audit history; `coordinator
//! set` does not rewrite another agent's record). Only a non-designated
//! delivery target can be a child here. An agentcloud coordinator has no
//! pinned process identity, so its liveness comes from the orchestrator
//! (see `proof::CloudLiveness`), never from a local pid.

use crate::model::{Declaration, Designation};
use crate::proof::{
    agent_record, identity_alive, managed_by_chain, registry_coordinator_name, CloudLiveness,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One health finding about a declaration's delivery target.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// Stable flag kind, for example `target-missing`.
    pub kind: &'static str,
    /// `error` when messages to or from the team fail until it is fixed;
    /// `warning` for a standing risk (a pin that goes stale later, a tree
    /// shape) that does not lose anything today.
    pub severity: &'static str,
    /// What is wrong, in one line.
    pub detail: String,
    /// The command that fixes it.
    pub fix: String,
}

impl Flag {
    /// Whether this flag is a present failure rather than a standing risk.
    pub fn is_error(&self) -> bool {
        self.severity == "error"
    }
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

/// What a registry says about one agent name.
#[derive(Debug, Clone, PartialEq)]
pub enum Presence {
    /// The agent has a record (`agent.json`) in the registry, either
    /// layout; or a directory without one (a queue created ahead of
    /// registration) and no archived generation.
    Present,
    /// No record, and the archive holds a retired generation of exactly
    /// this name. A leftover queue directory (an older receiver, or a
    /// router writing by path, recreates `agents/NAME/queue/` after the
    /// stop) does not make a retired agent present.
    Archived,
    /// No directory and no archived generation.
    Missing,
    /// The registry could not be read; absence is not established.
    Unknown(String),
}

/// Where `agent` stands in `registry`. A record wins over any archived
/// generation of the same name (a stop and restart, or a re-adoption,
/// leaves the predecessor in the archive); the archive is matched by the
/// entry's recorded name, never a file-name prefix, and it decides for a
/// directory that holds no record.
pub fn presence(registry: &Path, agent: &str) -> Presence {
    let listed = match agent_queue::registry::names(registry) {
        Ok(names) => names.iter().any(|name| name == agent),
        Err(error) => return Presence::Unknown(error.to_string()),
    };
    if listed {
        match agent_queue::registry::record_directories(registry, agent) {
            Ok(found) if !found.is_empty() => return Presence::Present,
            Ok(_) => {}
            Err(error) => return Presence::Unknown(error.to_string()),
        }
    }
    match agent_queue::registry::archived(registry, agent) {
        Ok(true) => Presence::Archived,
        Ok(false) if listed => Presence::Present,
        Ok(false) => Presence::Missing,
        Err(error) => Presence::Unknown(error.to_string()),
    }
}

/// Whether `registry` is a real agentctl registry, where a missing agent is
/// evidence of a dead target: it has a designation or at least one agent
/// record. A scratch queue root (queue directories created ahead of
/// registration, or an empty `agents/` tree) is not, and neither is a
/// registry that cannot be read.
fn populated(registry: &Path) -> bool {
    if registry.join("coordinator.json").exists() {
        return true;
    }
    agent_queue::registry::names(registry).is_ok_and(|names| {
        names.iter().any(|name| {
            agent_queue::registry::record_directories(registry, name)
                .is_ok_and(|found| !found.is_empty())
        })
    })
}

/// Why handing a message to `agent` in `registry` can never reach anyone:
/// the agent has no record and its name is archived, or it has no
/// directory at all in a registry that otherwise has agents or a
/// designation. An agent with a record is deliverable; so is any agent of
/// an unreadable or scratch registry, where absence proves nothing. Shared
/// by `accept`'s deliverer and the sender-side pre-flight (#721).
pub fn undeliverable_reason(registry: &Path, agent: &str) -> Option<String> {
    match presence(registry, agent) {
        Presence::Present | Presence::Unknown(_) => None,
        Presence::Archived => Some(format!(
            "delivery target {agent:?} is archived in registry {}",
            registry.display()
        )),
        Presence::Missing if populated(registry) => Some(format!(
            "delivery target {agent:?} has no agent record in registry {}",
            registry.display()
        )),
        Presence::Missing => None,
    }
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
    let severity = match kind {
        "target-missing"
        | "target-archived"
        | "target-not-live"
        | "target-cloud-unreachable"
        | "designation-not-live" => "error",
        _ => "warning",
    };
    Flag {
        kind,
        severity,
        detail,
        fix,
    }
}

/// Whether a pinned designation's harness process is live.
fn pinned_designation_live(declaration: &Declaration, registry: Option<&Path>) -> bool {
    crate::proof::coordinator_identity(&declaration.designation, registry)
        .is_some_and(|identity| identity_alive(&identity))
}

/// Evaluate one declaration's delivery target against its registry's
/// control tree. Non-queue deliverers have no agent target; only a pinned
/// designation's liveness is checked for them.
pub fn evaluate(declaration: &Declaration) -> TargetHealth {
    evaluate_with_cloud_probe(declaration, &crate::proof::cloud_liveness)
}

/// [`evaluate`] with the agentcloud liveness probe injected, so tests can
/// answer for the orchestrator without one.
pub fn evaluate_with_cloud_probe(
    declaration: &Declaration,
    cloud_probe: &dyn Fn(&Path, &str) -> Option<CloudLiveness>,
) -> TargetHealth {
    let mut flags = Vec::new();
    let team = &declaration.team;
    if declaration.deliverer_kind != "agentctl-queue" {
        if let Designation::Pinned { .. } = declaration.designation {
            let registry = registry_of(declaration);
            if pinned_designation_live(declaration, registry.as_deref()) {
                flags.push(flag(
                    "pinned-designation",
                    format!("team {team}: declaration pins the coordinator's harness process; when that session is retired its successor cannot send for the team (`agentctl coordinator set NEW` moves a pin to exactly the old harness)"),
                    format!("re-run `agent-teams init {team} --registry <registry> --deliver {}` so sender standing follows the registry's designated coordinator", declaration.deliverer_kind),
                ));
            } else {
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
    // A pinned designation decides who may send for the team (sender
    // standing), whatever the deliverer: after a handover it still names
    // the old harness process, so the new coordinator's sends are refused.
    if let Designation::Pinned { .. } = declaration.designation {
        if pinned_designation_live(declaration, Some(&registry)) {
            flags.push(flag(
                "pinned-designation",
                format!("team {team}: declaration pins the coordinator's harness process; when that session is retired its successor cannot send for the team"),
                format!("re-run `agent-teams init {team} --registry {}` so sender standing follows the registry's designated coordinator", registry.display()),
            ));
        } else {
            flags.push(flag(
                "designation-not-live",
                format!("team {team}: declaration pins a coordinator harness process that is not live; every send for the team is refused as sender-not-coordinator"),
                format!("re-run `agent-teams init {team} --registry {}` (or `agentctl coordinator set NEW`, which rewrites it) so sender standing follows the registry's designated coordinator", registry.display()),
            ));
        }
    }
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
    match presence(&registry, &target_name) {
        Presence::Archived => flags.push(flag(
            "target-archived",
            format!("team {team}: delivery target {target_name:?} is archived (stopped and retired) in {}", registry.display()),
            format!("run `agentctl coordinator set NEW` to designate a live coordinator, then re-run `agent-teams init {team} --registry {}` if this declaration pinned the old name", registry.display()),
        )),
        Presence::Missing => flags.push(flag(
            "target-missing",
            format!("team {team}: delivery target {target_name:?} has no agent record in {}", registry.display()),
            "run `agentctl coordinator set NEW` to designate a live coordinator, then `agent-teams check`".to_string(),
        )),
        Presence::Unknown(error) => flags.push(flag(
            "target-unknown",
            format!("team {team}: could not read registry {} to find delivery target {target_name:?} ({error}); delivery is attempted", registry.display()),
            format!("make {} readable to this user, then `agent-teams check`", registry.display()),
        )),
        Presence::Present => {
            if let Some((identity, _)) = agent_record(&registry, &target_name) {
                if !identity_alive(&identity) {
                    flags.push(flag(
                        "target-not-live",
                        format!("team {team}: delivery target {target_name:?} is not live (its pinned harness process is gone)"),
                        format!("revive {target_name} (`agentctl revive {target_name}`) or designate its successor: `agentctl coordinator set NEW`"),
                    ));
                }
            } else if let Some(liveness) = cloud_probe(&registry, &target_name) {
                // No pinned process identity: an agentcloud target's
                // liveness is the orchestrator's word, not a local pid.
                match liveness {
                    CloudLiveness::Live => {}
                    CloudLiveness::NotLive => flags.push(flag(
                        "target-not-live",
                        format!("team {team}: delivery target {target_name:?} is not live (its agentcloud session is not active or attached, as the orchestrator reports it)"),
                        format!("revive {target_name} (`agentctl revive {target_name}`) or designate its successor: `agentctl coordinator set NEW`"),
                    )),
                    CloudLiveness::Unreachable(reason) => flags.push(flag(
                        "target-cloud-unreachable",
                        format!("team {team}: delivery target {target_name:?} is an agentcloud session whose liveness is unverified ({reason}); delivery is still attempted"),
                        "check the agentcloud orchestrator is reachable (`agentcloudctl list`), then `agent-teams check`".to_string(),
                    )),
                }
            }
        }
    }
    // Control-tree shape: a non-designated target must be the root, and the
    // tree's root must be the designated coordinator. The designated
    // coordinator itself is the root by designation (see the module docs):
    // its recorded `managed_by` edge names whoever started it, which stops
    // counting the moment it is designated, so neither `target-is-child`
    // nor a root mismatch can fire for it.
    if designated.as_deref() != Some(target_name.as_str()) {
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

    /// An archive entry `entry`, with an `agent.json` naming `recorded`.
    fn archive(dir: &Path, entry: &str, recorded: Option<&str>) {
        let path = dir.join("archive").join(entry);
        std::fs::create_dir_all(&path).expect("archive entry");
        if let Some(name) = recorded {
            std::fs::write(path.join("agent.json"), json!({"name": name}).to_string())
                .expect("archived record");
        }
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
        archive(&dir, "old-1792931619150276422-1347278", Some("old"));
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
    fn designated_child_is_the_root() {
        // Designation re-roots: a designated coordinator managed by its
        // starter is the root, so no child or not-root flag fires for it.
        let dir = fixture(
            &[("root", None, true), ("child", Some("root"), true)],
            Some("child"),
        );
        let health = evaluate(&declaration(&dir, None));
        assert!(health.flags.is_empty(), "flags: {:?}", health.flags);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_designated_child_target_still_flags() {
        let dir = fixture(
            &[
                ("coord", None, true),
                ("root", None, true),
                ("child", Some("root"), true),
            ],
            Some("coord"),
        );
        let health = evaluate(&declaration(&dir, Some("child")));
        let found = kinds(&health);
        assert!(found.contains(&"target-is-child"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_root_mismatch_flags_live_other_root() {
        // A non-designated target whose tree roots at a live agent that is
        // not the designated coordinator.
        let dir = fixture(
            &[
                ("coord", None, true),
                ("root", None, true),
                ("coord2", Some("root"), true),
            ],
            Some("coord"),
        );
        let health = evaluate(&declaration(&dir, Some("coord2")));
        let found = kinds(&health);
        assert!(
            found.contains(&"tree-root-mismatch"),
            "{found:?} {:?}",
            health.flags
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Registry holding one agentcloud coordinator (`coord7`, started by
    /// `coord6`) with no pinned process identity, as agentctl records it.
    fn cloud_fixture() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10_000);
        let dir = std::env::temp_dir().join(format!(
            "agent-teams-health-cloud-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let agent_dir = dir.join("agents").join("coord7");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let record = json!({
            "name": "coord7", "harness": "agentcloud", "adapter": "agentcloud",
            "harness_identity": null,
            "session_value": "2bed1f68-4b34-4880-b933-4913caecf0e6",
            "agentcloud": {"endpoint": "wss://orchestrator.example/ws"},
            "managed_by": {"agent": "coord6", "registry": dir.to_string_lossy(), "token": "t"},
        });
        std::fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string(&record).expect("record json"),
        )
        .expect("record file");
        std::fs::write(dir.join("coordinator.json"), "{\"name\": \"coord7\"}")
            .expect("designation");
        dir
    }

    #[test]
    fn live_agentcloud_coordinator_is_healthy_and_root() {
        let dir = cloud_fixture();
        let health =
            evaluate_with_cloud_probe(&declaration(&dir, None), &|_, _| Some(CloudLiveness::Live));
        assert_eq!(health.target.as_deref(), Some("coord7"));
        assert!(health.flags.is_empty(), "flags: {:?}", health.flags);
        assert!(health.undeliverable().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inactive_agentcloud_coordinator_is_not_live() {
        let dir = cloud_fixture();
        let health = evaluate_with_cloud_probe(&declaration(&dir, None), &|_, _| {
            Some(CloudLiveness::NotLive)
        });
        let flag = health
            .flags
            .iter()
            .find(|flag| flag.kind == "target-not-live")
            .expect("not-live flagged");
        assert!(flag.is_error());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreachable_orchestrator_is_distinct_and_still_deliverable() {
        let dir = cloud_fixture();
        let health = evaluate_with_cloud_probe(&declaration(&dir, None), &|_, _| {
            Some(CloudLiveness::Unreachable("probe timed out".to_string()))
        });
        let flag = health
            .flags
            .iter()
            .find(|flag| flag.kind == "target-cloud-unreachable")
            .expect("unreachable flagged distinctly");
        assert!(flag.is_error());
        assert!(flag.detail.contains("unverified"), "{}", flag.detail);
        assert!(health.undeliverable().is_none(), "{:?}", health.flags);
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

    /// Review finding 1: a live agent stays deliverable beside an archived
    /// predecessor of the same name and an archived agent whose name it
    /// prefixes, in both archive token formats.
    #[test]
    fn live_agent_beside_same_name_and_prefix_archives_is_deliverable() {
        let dir = fixture(&[("coordinator", None, true)], Some("coordinator"));
        archive(
            &dir,
            "coordinator-1792931619150276422-1347278",
            Some("coordinator"),
        );
        archive(&dir, &format!("coordinator-stale-{}", "c".repeat(64)), None);
        archive(
            &dir,
            "coordinator-tail-1792931619150276999-1347279",
            Some("coordinator-tail"),
        );
        assert_eq!(presence(&dir, "coordinator"), Presence::Present);
        assert_eq!(undeliverable_reason(&dir, "coordinator"), None);
        let health = evaluate(&declaration(&dir, None));
        assert!(health.undeliverable().is_none(), "{:?}", health.flags);
        assert!(health.flags.is_empty(), "flags: {:?}", health.flags);
        // A missing agent whose name only prefixes an archived one is
        // missing, not archived.
        let missing = fixture(&[("other", None, true)], Some("other"));
        archive(
            &missing,
            "coordinator-tail-1792931619150276999-1347279",
            Some("coordinator-tail"),
        );
        assert_eq!(presence(&missing, "coordinator"), Presence::Missing);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&missing);
    }

    /// An unreadable registry proves nothing: unknown, deliverable.
    #[test]
    fn unreadable_registry_is_unknown_and_deliverable() {
        let dir = fixture(&[], None);
        std::fs::create_dir_all(&dir).expect("dir");
        let not_a_directory = dir.join("registry-file");
        std::fs::write(&not_a_directory, "x").expect("file");
        assert!(matches!(
            presence(&not_a_directory, "coord"),
            Presence::Unknown(_)
        ));
        assert_eq!(undeliverable_reason(&not_a_directory, "coord"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A queue created before registration never causes a refusal: the
    /// target's own queue directory makes it present, and other agents'
    /// queues (or an empty prepared `agents/` tree) do not make a registry
    /// populated.
    #[test]
    fn queue_directories_before_registration_are_deliverable() {
        let dir = fixture(&[], None);
        std::fs::create_dir_all(dir.join("agents/coord/queue")).expect("own queue");
        assert_eq!(undeliverable_reason(&dir, "coord"), None);
        std::fs::create_dir_all(dir.join("agents/other/queue")).expect("other queue");
        std::fs::write(dir.join("agents/agent.json"), "\"tripwire\"").expect("tripwire");
        assert_eq!(undeliverable_reason(&dir, "late"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Standing risks warn; only present failures are errors.
    #[test]
    fn pinned_agent_is_a_warning_and_missing_target_an_error() {
        let dir = fixture(&[("coord", None, true)], Some("coord"));
        let health = evaluate(&declaration(&dir, Some("coord")));
        assert_eq!(kinds(&health), vec!["pinned-agent"]);
        assert!(health.flags.iter().all(|flag| !flag.is_error()));
        let health = evaluate(&declaration(&dir, Some("ghost")));
        assert!(health
            .flags
            .iter()
            .any(|flag| flag.kind == "target-missing" && flag.is_error()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review finding 3: a pinned designation is flagged for queue
    /// deliverers too; a dead one is an error (every send is refused).
    #[test]
    fn pinned_designation_is_flagged_for_queue_deliverers() {
        let dir = fixture(&[("coord", None, true)], Some("coord"));
        let me = crate::proof::identity_of(u64::from(std::process::id())).expect("self");
        let mut raw = declaration(&dir, None).raw;
        raw["coordinator"] = json!({"designation": "pinned", "harness_identity": me});
        let health = evaluate(&parse_declaration(raw.clone()).expect("declaration"));
        assert_eq!(
            kinds(&health),
            vec!["pinned-designation"],
            "{:?}",
            health.flags
        );
        raw["coordinator"]["harness_identity"]["starttime_ticks"] =
            json!(me.starttime_ticks + 999_999);
        let health = evaluate(&parse_declaration(raw).expect("declaration"));
        let dead = health
            .flags
            .iter()
            .find(|flag| flag.kind == "designation-not-live")
            .expect("dead pin flagged");
        assert!(dead.is_error());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review of the fix, finding A: a queue directory recreated after the
    /// stop (by an older receiver) does not make an archived agent
    /// deliverable again.
    #[test]
    fn leftover_queue_of_an_archived_agent_is_undeliverable() {
        let dir = fixture(&[("coord", None, true)], Some("coord"));
        archive(&dir, "old-1792931619150276422-1347278", Some("old"));
        std::fs::create_dir_all(dir.join("agents/old/queue")).expect("leftover queue");
        assert_eq!(presence(&dir, "old"), Presence::Archived);
        assert!(undeliverable_reason(&dir, "old").is_some());
        let health = evaluate(&declaration(&dir, Some("old")));
        assert!(
            kinds(&health).contains(&"target-archived"),
            "{:?}",
            health.flags
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
