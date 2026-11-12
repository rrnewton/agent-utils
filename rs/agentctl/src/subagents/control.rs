//! Who controls whom in one registry: the registry's coordinator designation, and each agent's
//! `created_by` and `managed_by` edges.
//!
//! Control is a tree. The designated coordinator is its root; an agent whose `managed_by` names
//! another live agent of the registry hangs under it, at any depth, so a lieutenant that manages
//! its own workers is an ordinary interior node. An agent with no usable `managed_by` hangs
//! under the root. `created_by` is provenance only and never decides control.
//!
//! Edges name an agent generation by token, not by name: a renamed agent keeps its token, and a
//! revived one lists its earlier tokens in `former_tokens`, so an edge survives both. A token no
//! record answers to (the agent was stopped, or its name reused) is a stale edge, which control
//! decisions skip.

use agent_queue::identity::Ancestry;

use super::*;

/// The registry file holding the coordinator designation.
pub(crate) const DESIGNATION_FILE: &str = "coordinator.json";
/// Schema of [`DESIGNATION_FILE`].
pub(crate) const DESIGNATION_SCHEMA: &str = "agentctl-coordinator/v1";
const DESIGNATION_LOCK: &str = ".coordinator.lock";
pub(crate) const CREATED_BY: &str = "created_by";
pub(crate) const MANAGED_BY: &str = "managed_by";
/// Record key for the explicit pinned (persistent lane) flag (#917 chat-bang-commands).
pub(crate) const PINNED: &str = "pinned";
const FORMER_TOKENS: &str = "former_tokens";
/// A standing agent rests idle between requests by design, so `watch-agents` reports no
/// idle-without-verdict episode for it (#901 watch-standing-agents).
pub(crate) const STANDING: &str = "standing";
/// The Herdr tab viewing this agent, when it has no pane of its own: a
/// headless agent's `tail --follow` or console tab (#1225
/// workspace-goal-reconciler, Phase 1). Hygiene ranks that tab with the
/// agent; it is never the agent's pane.
pub(crate) const VIEWER_TAB: &str = "viewer_tab_id";
/// A one-line description of what the agent is for (#1342 agent-purpose-line):
/// display-only metadata recorded at start. Unlike `goal` it is never sent to
/// the harness as an instruction.
pub(crate) const PURPOSE: &str = "purpose";
/// Longest purpose line, in characters.
pub(crate) const PURPOSE_MAX_CHARS: usize = 200;

/// Validate a purpose the caller typed: nonempty, single line, bounded.
pub(crate) fn validated_purpose(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() || text.contains(['\n', '\r']) {
        return Err(fail("purpose must be a nonempty single line"));
    }
    if text.chars().count() > PURPOSE_MAX_CHARS {
        return Err(fail(format!(
            "purpose must be at most {PURPOSE_MAX_CHARS} characters"
        )));
    }
    Ok(text.to_owned())
}

/// A purpose taken from a stored source (a slot record, a brief): cleaned the
/// same way, but an unusable value yields `None` instead of an error, so the
/// next source is tried and a start never fails over a purpose.
fn cleaned_purpose(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    validated_purpose(&collapsed).ok().or_else(|| {
        let truncated: String = collapsed.chars().take(PURPOSE_MAX_CHARS).collect();
        validated_purpose(&truncated).ok()
    })
}

/// The first meaningful line of a brief: the first non-empty line that is not
/// a sender header or a bare fence, with heading/bullet markers stripped.
pub(crate) fn purpose_from_brief(brief: &str) -> Option<String> {
    for line in brief.lines() {
        let candidate = line.trim().trim_start_matches(['#', '-', '*', '>', ' ']);
        if candidate.is_empty() || candidate.starts_with("```") {
            continue;
        }
        if candidate.starts_with("[from ") {
            continue;
        }
        if let Some(purpose) = cleaned_purpose(candidate) {
            return Some(purpose);
        }
    }
    None
}

/// Choose the purpose recorded at start: an explicit `--purpose` wins, then
/// the slot's recorded purpose, then the brief's first meaningful line.
pub(crate) fn resolve_purpose(
    explicit: Option<&str>,
    slot: Option<&str>,
    brief: Option<&str>,
) -> Option<String> {
    if let Some(purpose) = explicit {
        return Some(purpose.to_owned());
    }
    if let Some(purpose) = slot.and_then(cleaned_purpose) {
        return Some(purpose);
    }
    brief.and_then(purpose_from_brief)
}
/// Earlier tokens a record keeps; older generations' edges are treated as stale.
const FORMER_TOKENS_MAX: usize = 32;
/// The edge was found by walking the caller's process ancestry to a pinned harness.
pub(crate) const PROOF_ANCESTRY: &str = "harness-ancestry";
/// The edge was named explicitly with `agentctl manage NAME --by MANAGER`.
pub(crate) const PROOF_ASSIGNED: &str = "assigned";
/// Deepest `managed_by` chain followed before a chain is treated as a cycle.
const MAX_DEPTH: usize = 64;

/// One edge from an agent record to the agent that created or manages it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Edge {
    /// The other agent's name when the edge was written.
    pub(crate) agent: String,
    /// Absolute path of the registry that agent is registered in.
    pub(crate) registry: String,
    /// The other agent's record token when the edge was written.
    pub(crate) token: String,
    /// How the edge was established: [`PROOF_ANCESTRY`] or [`PROOF_ASSIGNED`].
    pub(crate) proof: String,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    pub(crate) unknown: crate::forward_compat::KeptKeys,
}

/// The registry's coordinator designation, `REGISTRY/coordinator.json`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Designation {
    schema: String,
    pub(crate) name: String,
    pub(crate) token: String,
    set_at: f64,
    set_by: Option<Edge>,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    pub(crate) unknown: crate::forward_compat::KeptKeys,
}

/// The per-host coordinator index (schema `agentctl-coordinator-index/v1`):
/// directory holding one index file per designated registry, under the
/// host state home (`$XDG_STATE_HOME/agentctl/coordinators`, else
/// `~/.local/state/agentctl/coordinators`; `$AGENTCTL_COORDINATOR_INDEX_DIR`
/// overrides). Per-host state, never under a synced config directory: it
/// lets other tools on this machine (`agent-teams coordinators`) discover
/// who coordinates what, where, without scanning every harness.
pub(crate) fn host_index_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENTCTL_COORDINATOR_INDEX_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state).join("agentctl/coordinators");
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("~"), PathBuf::from);
    home.join(".local/state/agentctl/coordinators")
}

/// The index file for one registry: the SHA-256 of its absolute path, so
/// two harnesses never collide and no path needs escaping.
pub(crate) fn host_index_path(directory: &Path, registry: &str) -> PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(registry.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    directory.join(format!("{hex}.json"))
}

/// The `origin` URL from a git config file's first `[remote "origin"]`
/// section, or `None`. A plain text read: no git subprocess, no network.
fn git_origin_url(config: &Path) -> Option<String> {
    let text = fs::read_to_string(config).ok()?;
    let mut in_origin = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == "[remote \"origin\"]";
            continue;
        }
        if in_origin {
            if let Some(url) = trimmed.strip_prefix("url =") {
                return Some(url.trim().to_owned());
            }
        }
    }
    None
}

/// Origin URLs of the git checkouts at `harness_cwd` itself and its
/// immediate children (sorted, at most eight): where the harness's
/// primary checkout lives is a layout fact this cheap scan covers.
pub(crate) fn harness_repo_origins(harness_cwd: &Path) -> Vec<String> {
    let mut candidates = vec![harness_cwd.to_path_buf()];
    if let Ok(entries) = fs::read_dir(harness_cwd) {
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        children.sort();
        candidates.extend(children);
    }
    let mut origins = Vec::new();
    for candidate in candidates.into_iter().take(9) {
        let dotgit = candidate.join(".git");
        let config = if dotgit.is_dir() {
            dotgit.join("config")
        } else if let Ok(pointer) = fs::read_to_string(&dotgit) {
            // A worktree's `.git` file points at its gitdir; its config is
            // the gitdir's own or the shared one its commondir names.
            let Some(gitdir) = pointer.trim().strip_prefix("gitdir:") else {
                continue;
            };
            let gitdir = PathBuf::from(gitdir.trim());
            if gitdir.join("config").exists() {
                gitdir.join("config")
            } else if let Ok(common) = fs::read_to_string(gitdir.join("commondir")) {
                let common = common.trim();
                let base = if Path::new(common).is_absolute() {
                    PathBuf::from(common)
                } else {
                    gitdir.join(common)
                };
                base.join("config")
            } else {
                continue;
            }
        } else {
            continue;
        };
        if let Some(url) = git_origin_url(&config) {
            if !origins.contains(&url) {
                origins.push(url);
            }
        }
    }
    origins
}

/// The agent-teams state directory declarations live under
/// (`$AGENT_TEAMS_STATE_DIR`, `$XDG_STATE_HOME/agent-teams`, or
/// `~/.local/state/agent-teams`).
pub(crate) fn agent_teams_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENT_TEAMS_STATE_DIR") {
        PathBuf::from(dir)
    } else if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        PathBuf::from(xdg).join("agent-teams")
    } else {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("~"), PathBuf::from);
        home.join(".local/state/agent-teams")
    }
}

/// Whether a declaration's `deliverer.registry` text names `registry`: `~/` expanded, then both
/// sides canonicalized (symlinks, `..`, a trailing `/`), so a differently spelled path to the
/// same registry still matches. When either side does not resolve, the spelled paths are
/// compared component by component.
fn names_registry(declared: &str, registry: &Path) -> bool {
    if declared.is_empty() {
        return false;
    }
    let expanded = match (declared.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(declared),
    };
    match (fs::canonicalize(&expanded), fs::canonicalize(registry)) {
        (Ok(left), Ok(right)) => left == right,
        _ => expanded.as_path() == registry,
    }
}

/// Every agent-teams declaration under `teams_state` (`teams.d/<team>/declaration.json`), as
/// `(declaration path, document)` sorted by path. Best-effort: an unreadable declaration is
/// skipped.
fn declarations_all(teams_state: &Path) -> Vec<(PathBuf, Value)> {
    let Ok(entries) = fs::read_dir(teams_state.join("teams.d")) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join("declaration.json"))
        .filter(|path| path.exists())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let document: Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
            Some((path, document))
        })
        .collect()
}

/// Whether a declaration's deliverer names `registry`.
fn declaration_names_registry(document: &Value, registry: &Path) -> bool {
    document
        .get("deliverer")
        .and_then(|deliverer| deliverer.get("registry"))
        .and_then(Value::as_str)
        .is_some_and(|declared| names_registry(declared, registry))
}

/// Every agent-teams declaration under `teams_state` whose deliverer names `registry`, as
/// `(declaration path, document)` sorted by path.
fn declarations_naming(teams_state: &Path, registry: &Path) -> Vec<(PathBuf, Value)> {
    declarations_all(teams_state)
        .into_iter()
        .filter(|(_, document)| declaration_names_registry(document, registry))
        .collect()
}

/// The team whose `agent-teams` declaration names `registry` as its
/// queue deliverer, if any. Reads the agent-teams state layout
/// (`teams.d/<team>/declaration.json` under [`agent_teams_state_dir`])
/// best-effort: a discovery hint, never an authority.
pub(crate) fn declared_team_for_registry(registry: &str) -> Option<String> {
    declarations_naming(&agent_teams_state_dir(), Path::new(registry))
        .into_iter()
        .find_map(|(_, document)| {
            document
                .get("team")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

/// A declaration's pinned coordinator process (`coordinator.designation: "pinned"`), when it
/// pins one: `Some(None)` for a pin whose identity does not parse.
fn pinned_designation(document: &Value) -> Option<Option<CustomProcessIdentity>> {
    let coordinator = document.get("coordinator")?;
    if coordinator.get("designation").and_then(Value::as_str) != Some("pinned") {
        return None;
    }
    Some(
        coordinator
            .get("harness_identity")
            .cloned()
            .and_then(|identity| serde_json::from_value(identity).ok()),
    )
}

/// The designation a handover writes into a declaration in place of a pinned process.
fn registry_designation() -> Value {
    json!({"designation": "registry", "comment": "the registry's own coordinator designation; agentctl keeps it current"})
}

/// Red flags for every agent-teams declaration naming `registry`, in the
/// same vocabulary as `agent-teams check` (#721): the delivery target a
/// declaration resolves to (its pinned agent, else the designated
/// coordinator) against this registry's records and control tree, and a
/// pinned coordinator process. Each flag names its fix and a `severity`:
/// `error` when messages to or from the team fail today, `warning` for a
/// standing risk. `watch-agents` surfaces these each pass, reading
/// declarations from `teams_state`.
pub(crate) fn team_declaration_flags(registry: &Path, teams_state: &Path) -> Vec<Value> {
    let registry_text = registry.display().to_string();
    let designated = fs::read_to_string(registry.join(DESIGNATION_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("name").and_then(Value::as_str).map(str::to_owned));
    // Every listed agent directory (a queue made ahead of registration counts), and for those
    // with a readable record: name -> (lifecycle, managed_by agent).
    let listed = agent_queue::registry::names(registry).map_err(|error| error.to_string());
    let mut records: std::collections::BTreeMap<String, (String, Option<String>)> =
        std::collections::BTreeMap::new();
    for name in listed.as_deref().unwrap_or_default() {
        let Ok(directory) = agent_queue::registry::directory(registry, name) else {
            continue;
        };
        let Ok(text) = fs::read_to_string(directory.join("agent.json")) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        records.insert(
            name.clone(),
            (
                document
                    .get("lifecycle")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                document
                    .get(MANAGED_BY)
                    .and_then(|edge| edge.get("agent"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
        );
    }
    let root_of = |agent: &str| {
        let mut current = agent.to_owned();
        for _ in 0..MAX_DEPTH {
            match records.get(&current).and_then(|(_, parent)| parent.clone()) {
                Some(parent) if parent != current => current = parent,
                _ => break,
            }
        }
        current
    };
    let mut flags = Vec::new();
    for (_, document) in declarations_naming(teams_state, registry) {
        let team = document
            .get("team")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned();
        let mut push = |kind: &str, detail: String, fix: String| {
            let severity = match kind {
                "target-missing"
                | "target-archived"
                | "target-not-live"
                | "designation-not-live" => "error",
                _ => "warning",
            };
            flags.push(json!({"team": team, "kind": kind, "severity": severity, "detail": detail, "fix": fix}));
        };
        // A pinned coordinator process decides who may send for the team, whatever the
        // deliverer; after a handover it names the old harness.
        if let Some(identity) = pinned_designation(&document) {
            // As `agent-teams check` judges it: a pin it cannot show alive refuses every send.
            let live = identity.is_some_and(|identity| {
                agent_queue::identity::liveness(&identity) == ProcessLiveness::Alive
            });
            if !live {
                push(
                    "designation-not-live",
                    format!("team {team}: declaration pins a coordinator harness process that is not live; every send for the team is refused as sender-not-coordinator"),
                    format!("run `agentctl coordinator set NEW` (it rewrites the pin), or re-run `agent-teams init {team} --registry {registry_text}`"),
                );
            } else {
                push(
                    "pinned-designation",
                    format!("team {team}: declaration pins the coordinator's harness process; when that session is retired its successor cannot send for the team"),
                    format!("re-run `agent-teams init {team} --registry {registry_text}` so sender standing follows the designated coordinator"),
                );
            }
        }
        if document
            .get("deliverer")
            .and_then(|deliverer| deliverer.get("kind"))
            .and_then(Value::as_str)
            != Some("agentctl-queue")
        {
            continue;
        }
        let pinned = document
            .get("deliverer")
            .and_then(|deliverer| deliverer.get("agent"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(agent) = &pinned {
            push(
                "pinned-agent",
                format!("team {team}: declaration pins deliverer agent {agent:?}; a pinned name goes stale when that agent is retired"),
                format!("re-run `agent-teams init {team} --registry {registry_text}` so delivery resolves the designated coordinator at send time"),
            );
        }
        let target = pinned.clone().or_else(|| designated.clone());
        let Some(target) = target else {
            push(
                "target-missing",
                format!("team {team}: registry has no designated coordinator to deliver to"),
                "run `agentctl coordinator set NEW`, then `agent-teams check`".to_owned(),
            );
            continue;
        };
        if let (Some(agent), Some(root)) = (&pinned, &designated) {
            if agent != root {
                push(
                    "target-not-designated",
                    format!("team {team}: declaration delivers to {agent:?} but the designated coordinator is {root:?}"),
                    format!("re-run `agent-teams init {team} --registry {registry_text}` to drop the pin, or `agentctl coordinator set {agent}`"),
                );
            }
        }
        // A record wins over any archived generation of the same name; the archive is matched by
        // recorded name, never a file-name prefix, and decides for a directory without a record
        // (a queue recreated after the stop); a registry that cannot be read proves nothing.
        let listed_target = match &listed {
            Ok(names) => Ok(names.contains(&target)),
            Err(error) => Err(error.clone()),
        };
        let recorded = listed_target.as_ref().is_ok_and(|listed| {
            *listed
                && agent_queue::registry::record_directories(registry, &target)
                    .is_ok_and(|found| !found.is_empty())
        });
        match listed_target {
            Err(error) => push(
                "target-unknown",
                format!("team {team}: could not list this registry to find delivery target {target:?} ({error})"),
                "make the registry readable to this user, then `agent-teams check`".to_owned(),
            ),
            Ok(_) if recorded => match records.get(&target) {
                Some((lifecycle, _)) if !matches!(lifecycle.as_str(), "running" | "starting") => {
                    push(
                        "target-not-live",
                        format!("team {team}: delivery target {target:?} is not live (lifecycle {lifecycle})"),
                        format!("revive it (`agentctl revive {target}`) or designate its successor: `agentctl coordinator set NEW`"),
                    );
                }
                _ => {}
            },
            Ok(listed) => match agent_queue::registry::archived(registry, &target) {
                Ok(true) => push(
                    "target-archived",
                    format!("team {team}: delivery target {target:?} is archived in this registry"),
                    "run `agentctl coordinator set NEW` to designate a live coordinator".to_owned(),
                ),
                // A queue made ahead of registration.
                Ok(false) if listed => {}
                Ok(false) => push(
                    "target-missing",
                    format!("team {team}: delivery target {target:?} has no agent record in this registry"),
                    "run `agentctl coordinator set NEW` to designate a live coordinator, then `agent-teams check`".to_owned(),
                ),
                Err(error) => push(
                    "target-unknown",
                    format!("team {team}: could not read this registry's archive for delivery target {target:?} ({error})"),
                    "make the registry readable to this user, then `agent-teams check`".to_owned(),
                ),
            },
        }
        if let Some(parent) = records.get(&target).and_then(|(_, parent)| parent.clone()) {
            push(
                "target-is-child",
                format!("team {team}: delivery target {target:?} is managed by {parent:?}; it is a child tab, not the root coordinator"),
                format!("designate the root instead: `agentctl coordinator set {}`", root_of(&target)),
            );
        }
        let root = root_of(&target);
        if designated.as_deref() != Some(root.as_str()) && records.contains_key(&root) {
            push(
                "tree-root-mismatch",
                format!("team {team}: the root of {target:?}'s managed-by tree is {root:?}, not the designated coordinator"),
                format!("run `agentctl coordinator set {root}` if that root should coordinate"),
            );
        }
        if let Some(root) = &designated {
            if let Some(parent) = records.get(root).and_then(|(_, parent)| parent.clone()) {
                push(
                    "designated-not-root",
                    format!("team {team}: designated coordinator {root:?} is managed by {parent:?}; a coordinator must be the root of its control tree"),
                    format!("run `agentctl manage {root} --clear`, or designate its root: `agentctl coordinator set {parent}`"),
                );
            }
        }
    }
    flags
}

/// The receiving-host notices agent-teams leaves when it refuses a message as undeliverable
/// (`teams.d/<team>/notices/<id>.json`), for every team whose declaration names `registry`,
/// as `(notice path, notice)` sorted by path. A notice for a message already told about (one
/// in `notices-told/`) carries `"repeat": true`.
pub(crate) fn team_undeliverable_notices(
    registry: &Path,
    teams_state: &Path,
) -> Vec<(PathBuf, Value)> {
    let mut found = Vec::new();
    for (path, _) in declarations_naming(teams_state, registry) {
        let Some(team_dir) = path.parent() else {
            continue;
        };
        let Ok(entries) = fs::read_dir(team_dir.join("notices")) else {
            continue;
        };
        for entry in entries.flatten() {
            let notice_path = entry.path();
            if notice_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(mut notice) = fs::read_to_string(&notice_path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            else {
                continue;
            };
            // A sender's retry refused again rewrites the same notice; the coordinator has been
            // told about that message once already.
            if let Some(file_name) = notice_path.file_name() {
                if team_dir.join("notices-told").join(file_name).exists() {
                    notice["repeat"] = json!(true);
                }
            }
            found.push((notice_path, notice));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

/// Move a notice the coordinator has been told about out of `notices/` into its sibling
/// `notices-told/`, so it is told once and the record stays for a person to read.
pub(crate) fn acknowledge_team_notice(path: &Path) -> std::io::Result<()> {
    let (Some(notices), Some(file_name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let Some(team_dir) = notices.parent() else {
        return Ok(());
    };
    let told = team_dir.join("notices-told");
    fs::create_dir_all(&told)?;
    fs::rename(path, told.join(file_name))
}

/// How `agentctl manage` changes an agent's controller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManageAction {
    /// Make the calling agent, proved by its process ancestry, the controller.
    Caller,
    /// Make this registered agent the controller.
    By(String),
    /// Remove the controller; the agent then answers to the designated coordinator.
    Clear,
    /// Record the agent as pinned, a persistent lane (#917 chat-bang-commands).
    Pin,
    /// Remove the explicit pinned flag; the derived classification applies again.
    Unpin,
}

/// Whether `record` counts as pinned for `!who` and `tree` (#917): the explicit flag wins;
/// otherwise the designated coordinator, an agent in `restore.json` `agents[]`, or an agent
/// that manages another agent counts as pinned. Returns the value and its source.
pub(crate) fn pinned_classification(
    record: &AgentRecord,
    designated: Option<&str>,
    restore_agents: &BTreeSet<String>,
    has_children: bool,
) -> (bool, &'static str) {
    if let Some(value) = record.pinned_explicit() {
        return (value, "explicit");
    }
    if Some(record.name.as_str()) == designated {
        return (true, "coordinator");
    }
    if restore_agents.contains(&record.name) {
        return (true, "restore");
    }
    if has_children {
        return (true, "manager");
    }
    (false, "none")
}

/// The agent names `restore.json` lists as desired state, or none when it cannot be read.
pub(crate) fn restore_agent_names(registry: &Path) -> BTreeSet<String> {
    let Ok(text) = fs::read_to_string(registry.join("restore.json")) else {
        return BTreeSet::new();
    };
    let Ok(document) = serde_json::from_str::<Value>(&text) else {
        return BTreeSet::new();
    };
    document
        .get("agents")
        .and_then(Value::as_array)
        .map(|agents| {
            agents
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

impl AgentRecord {
    /// The edge stored under `key`, from the flat record or a nested row's extensions. A value
    /// that is not a well-formed edge reads as absent.
    pub(super) fn edge(&self, key: &str) -> Option<Edge> {
        let value = match &self.nested {
            Some(nested) => nested.extensions().get(key),
            None => self.extra.get(key),
        }?;
        serde_json::from_value::<Edge>(value.clone())
            .ok()
            .filter(|edge| name(&edge.agent).is_ok() && !edge.token.is_empty())
    }

    /// Store `edge` under `key`, or remove it.
    pub(super) fn set_edge(&mut self, key: &str, edge: Option<&Edge>) {
        let map = match &mut self.nested {
            Some(nested) => nested.extensions_mut(),
            None => {
                match edge {
                    Some(edge) => {
                        self.extra
                            .insert(key.to_owned(), serde_json::to_value(edge).expect("edge"));
                    }
                    None => {
                        self.extra.remove(key);
                    }
                }
                return;
            }
        };
        match edge {
            Some(edge) => {
                map.insert(key.to_owned(), serde_json::to_value(edge).expect("edge"));
            }
            None => {
                map.remove(key);
            }
        }
    }

    /// The explicit pinned flag (#917 chat-bang-commands): `Some` when the record carries a
    /// `pinned` key, from the flat record or a nested row's extensions.
    pub(super) fn pinned_explicit(&self) -> Option<bool> {
        let value = match &self.nested {
            Some(nested) => nested.extensions().get(PINNED),
            None => self.extra.get(PINNED),
        }?;
        value.as_bool()
    }

    /// Store or clear the explicit pinned flag.
    pub(super) fn set_pinned(&mut self, pinned: Option<bool>) {
        match &mut self.nested {
            Some(nested) => {
                let map = nested.extensions_mut();
                match pinned {
                    Some(value) => {
                        map.insert(PINNED.to_owned(), json!(value));
                    }
                    None => {
                        map.remove(PINNED);
                    }
                }
            }
            None => match pinned {
                Some(value) => {
                    self.extra.insert(PINNED.to_owned(), json!(value));
                }
                None => {
                    self.extra.remove(PINNED);
                }
            },
        }
    }

    /// Whether this agent is standing ([`STANDING`]).
    pub(super) fn standing(&self) -> bool {
        let value = match &self.nested {
            Some(nested) => nested.extensions().get(STANDING),
            None => self.extra.get(STANDING),
        };
        value == Some(&Value::Bool(true))
    }

    /// The Herdr tab viewing this agent ([`VIEWER_TAB`]), when one is named.
    pub(super) fn viewer_tab(&self) -> Option<String> {
        let value = match &self.nested {
            Some(nested) => nested.extensions().get(VIEWER_TAB),
            None => self.extra.get(VIEWER_TAB),
        }?;
        value.as_str().map(str::to_owned)
    }

    /// Name the Herdr tab viewing this agent, or clear it; a record with no
    /// viewer carries no field.
    pub(super) fn set_viewer_tab(&mut self, tab: Option<&str>) {
        match &mut self.nested {
            Some(nested) => {
                let map = nested.extensions_mut();
                match tab {
                    Some(tab) => {
                        map.insert(VIEWER_TAB.to_owned(), json!(tab));
                    }
                    None => {
                        map.remove(VIEWER_TAB);
                    }
                }
            }
            None => match tab {
                Some(tab) => {
                    self.extra.insert(VIEWER_TAB.to_owned(), json!(tab));
                }
                None => {
                    self.extra.remove(VIEWER_TAB);
                }
            },
        }
    }

    /// What this agent is for ([`PURPOSE`]), when a purpose is recorded.
    pub(super) fn purpose(&self) -> Option<String> {
        let value = match &self.nested {
            Some(nested) => nested.extensions().get(PURPOSE),
            None => self.extra.get(PURPOSE),
        }?;
        value.as_str().map(str::to_owned)
    }

    /// Record what this agent is for, or clear it; a record with no purpose
    /// carries no field. Display-only: nothing here prompts the agent.
    pub(super) fn set_purpose(&mut self, purpose: Option<&str>) {
        match &mut self.nested {
            Some(nested) => {
                let map = nested.extensions_mut();
                match purpose {
                    Some(purpose) => {
                        map.insert(PURPOSE.to_owned(), json!(purpose));
                    }
                    None => {
                        map.remove(PURPOSE);
                    }
                }
            }
            None => match purpose {
                Some(purpose) => {
                    self.extra.insert(PURPOSE.to_owned(), json!(purpose));
                }
                None => {
                    self.extra.remove(PURPOSE);
                }
            },
        }
    }

    /// Mark this agent standing, or not; a record that is not standing carries no field.
    pub(super) fn set_standing(&mut self, standing: bool) {
        match (&mut self.nested, standing) {
            (Some(nested), true) => {
                nested
                    .extensions_mut()
                    .insert(STANDING.to_owned(), Value::Bool(true));
            }
            (Some(nested), false) => {
                nested.extensions_mut().remove(STANDING);
            }
            (None, true) => {
                self.extra.insert(STANDING.to_owned(), Value::Bool(true));
            }
            (None, false) => {
                self.extra.remove(STANDING);
            }
        }
    }

    /// The pinned process ID to report as this agent's, only when it belongs to the current
    /// boot: after a restart the recorded PID may be an unrelated process.
    fn current_harness_pid(&self, boot_id: Option<&str>) -> Option<u64> {
        self.acting_process()
            .filter(|identity| Some(identity.boot_id.as_str()) == boot_id)
            .map(|identity| identity.pid)
    }

    /// Tokens of this agent's earlier generations, newest last.
    pub(super) fn former_tokens(&self) -> Vec<String> {
        self.extra
            .get(FORMER_TOKENS)
            .and_then(Value::as_array)
            .map(|tokens| {
                tokens
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Record that this generation replaces one whose token was `previous`, when `referenced`
    /// says an edge or the designation names that token (or earlier ones were already kept), and
    /// carry the edges a nested row kept in its extensions into the flat record a revive writes.
    /// A registry that uses no edges and no designation gets no new field.
    pub(super) fn succeed(&mut self, previous: &AgentRecord, referenced: bool) {
        let mut tokens = previous.former_tokens();
        if referenced || !tokens.is_empty() {
            tokens.push(previous.token.clone());
            let skip = tokens.len().saturating_sub(FORMER_TOKENS_MAX);
            let tokens: Vec<String> = tokens.into_iter().skip(skip).collect();
            self.extra.insert(FORMER_TOKENS.to_owned(), json!(tokens));
        }
        if self.nested.is_none() {
            for key in [CREATED_BY, MANAGED_BY] {
                if let Some(edge) = previous.edge(key) {
                    self.extra
                        .insert(key.to_owned(), serde_json::to_value(edge).expect("edge"));
                }
            }
        }
        if previous.standing() {
            self.set_standing(true);
        }
    }

    /// Whether this record is the generation `token` names, or a revival of it.
    pub(super) fn answers_to(&self, token: &str) -> bool {
        self.token == token || self.former_tokens().iter().any(|former| former == token)
    }

    /// The pinned process whose descendants act for this agent: its harness, else the runtime
    /// process a pane adapter pins.
    fn acting_process(&self) -> Option<&CustomProcessIdentity> {
        self.harness_identity
            .as_ref()
            .or(self.custom_process_identity.as_ref())
            .filter(|identity| identity.valid())
    }
}

/// One agent as control decisions see it.
#[derive(Clone, Debug)]
pub(crate) struct ControlNode {
    /// Registered name.
    pub(crate) name: String,
    /// Current token.
    pub(crate) token: String,
    /// Earlier tokens, from revives.
    pub(crate) former_tokens: Vec<String>,
    /// The agent's controller edge, if any.
    pub(crate) managed_by: Option<Edge>,
    /// Whether the agent can take a notice: its record says running or starting and its
    /// harness was reachable when last probed.
    pub(crate) reachable: bool,
}

impl ControlNode {
    fn answers_to(&self, token: &str) -> bool {
        self.token == token || self.former_tokens.iter().any(|former| former == token)
    }
}

/// The node `edge` names among `nodes`, when the edge belongs to `registry`.
fn resolve<'n>(nodes: &'n [ControlNode], registry: &Path, edge: &Edge) -> Option<&'n ControlNode> {
    if Path::new(&edge.registry) != registry {
        return None;
    }
    nodes
        .iter()
        .find(|node| node.name == edge.agent && node.answers_to(&edge.token))
        .or_else(|| nodes.iter().find(|node| node.answers_to(&edge.token)))
}

/// Whom to tell about `agent`: the nearest reachable agent up its `managed_by` chain, else the
/// designated coordinator, else `None`. Never the agent itself, and a cycle ends the walk.
pub(crate) fn controller_of(
    nodes: &[ControlNode],
    registry: &Path,
    designated: Option<&str>,
    agent: &str,
) -> Option<String> {
    let mut seen = BTreeSet::from([agent.to_owned()]);
    let mut current = nodes.iter().find(|node| node.name == agent)?;
    for _ in 0..MAX_DEPTH {
        let Some(manager) = current
            .managed_by
            .as_ref()
            .and_then(|edge| resolve(nodes, registry, edge))
        else {
            break;
        };
        if !seen.insert(manager.name.clone()) {
            break;
        }
        if manager.reachable {
            return Some(manager.name.clone());
        }
        current = manager;
    }
    designated
        .filter(|designated| *designated != agent)
        .map(str::to_owned)
}

/// The parent of `node` in the tree, by its `managed_by` edge only.
fn tree_parent<'n>(
    nodes: &'n [ControlNode],
    registry: &Path,
    node: &ControlNode,
) -> Option<&'n ControlNode> {
    node.managed_by
        .as_ref()
        .and_then(|edge| resolve(nodes, registry, edge))
        .filter(|parent| parent.name != node.name)
}

/// Order `names` so every agent comes after the agents above it in the control tree: the
/// designated coordinator first, then by depth, then by name.
pub(crate) fn tree_order(
    nodes: &[ControlNode],
    registry: &Path,
    designated: Option<&str>,
    names: &[String],
) -> Vec<String> {
    let depth = |name: &str| -> usize {
        if Some(name) == designated {
            return 0;
        }
        let mut seen = BTreeSet::new();
        let mut depth = 1;
        let mut current = nodes.iter().find(|node| node.name == name);
        while let Some(node) = current {
            if !seen.insert(node.name.clone()) || depth > MAX_DEPTH {
                break;
            }
            match tree_parent(nodes, registry, node) {
                Some(parent) if Some(parent.name.as_str()) != designated => {
                    depth += 1;
                    current = Some(parent);
                }
                _ => break,
            }
        }
        depth
    };
    let mut ordered: Vec<(usize, String)> = names
        .iter()
        .map(|name| (depth(name), name.clone()))
        .collect();
    ordered.sort();
    ordered.into_iter().map(|(_, name)| name).collect()
}

/// Every agent managed, at any depth, by `caller`, workers first (deepest
/// first, then name). The caller itself is never included (R2 team restart).
pub(crate) fn descendants(nodes: &[ControlNode], registry: &Path, caller: &str) -> Vec<String> {
    let depth_below = |name: &str| -> Option<usize> {
        let mut depth = 0;
        let mut seen = BTreeSet::new();
        let mut current = nodes.iter().find(|node| node.name == name)?;
        while current.name != caller {
            if !seen.insert(current.name.clone()) || depth > MAX_DEPTH {
                return None;
            }
            depth += 1;
            current = tree_parent(nodes, registry, current)?;
        }
        (name != caller).then_some(depth)
    };
    let mut found: Vec<(usize, String)> = nodes
        .iter()
        .filter_map(|node| depth_below(&node.name).map(|depth| (depth, node.name.clone())))
        .collect();
    found.sort_by(|(left_depth, left), (right_depth, right)| {
        right_depth.cmp(left_depth).then_with(|| left.cmp(right))
    });
    found.into_iter().map(|(_, name)| name).collect()
}

fn read_designation(registry: &Path) -> Result<Option<Designation>> {
    let path = registry.join(DESIGNATION_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(format!("cannot inspect {}: {error}", path.display()))),
    }
    let value = agent::read_private_json(&path)?;
    let designation: Designation = serde_json::from_value(value)
        .map_err(|error| fail(format!("invalid {}: {error}", path.display())))?;
    if designation.schema != DESIGNATION_SCHEMA || name(&designation.name).is_err() {
        return Err(fail(format!(
            "invalid {}: schema must be {DESIGNATION_SCHEMA} and name an agent",
            path.display()
        )));
    }
    Ok(Some(designation))
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// Every record of the registry this build can read, by name.
    fn readable_records(&self) -> Result<Vec<AgentRecord>> {
        if !self.registry.exists() {
            return Ok(Vec::new());
        }
        let names = self.registered_names()?;
        Ok(names
            .iter()
            .filter_map(|name| self.read_record(name).ok())
            .collect())
    }

    /// The registry's canonical path, as edges record it.
    fn registry_text(&self) -> String {
        fs::canonicalize(&self.registry)
            .unwrap_or_else(|_| self.registry.clone())
            .display()
            .to_string()
    }

    fn caller_ancestry(&self) -> Option<Ancestry> {
        #[cfg(test)]
        if let Some(ancestry) = &self.caller_ancestry {
            return ancestry.clone();
        }
        Ancestry::of_self().ok()
    }

    /// The registered agent this process acts for: the one whose pinned harness is the nearest
    /// ancestor of this process. `None` for a human at a shell, a timer, or a process whose
    /// ancestry stops at a box relay.
    pub(crate) fn caller_edge(&self) -> Option<Edge> {
        let ancestry = self.caller_ancestry()?;
        let records = self.readable_records().ok()?;
        let anchored: Vec<(&AgentRecord, &CustomProcessIdentity)> = records
            .iter()
            .filter_map(|record| record.acting_process().map(|identity| (record, identity)))
            .collect();
        let (index, _) = ancestry.nearest(anchored.iter().map(|(_, identity)| *identity))?;
        let record = anchored[index].0;
        Some(Edge {
            unknown: Default::default(),
            agent: record.name.clone(),
            registry: self.registry_text(),
            token: record.token.clone(),
            proof: PROOF_ANCESTRY.to_owned(),
        })
    }

    /// Whether the designation or any agent's edge names `token`, so a revive must keep it.
    pub(super) fn token_referenced(&self, token: &str) -> bool {
        let designated = read_designation(&self.registry)
            .ok()
            .flatten()
            .is_some_and(|designation| designation.token == token);
        designated
            || self.readable_records().is_ok_and(|records| {
                records.iter().any(|record| {
                    [CREATED_BY, MANAGED_BY]
                        .iter()
                        .filter_map(|key| record.edge(key))
                        .any(|edge| edge.token == token)
                })
            })
    }

    /// The edges a new record gets from its creator, unless `record_edges` is off.
    pub(super) fn creation_edges(&self, record: &mut AgentRecord, record_edges: bool) {
        if !record_edges {
            return;
        }
        if let Some(edge) = self.caller_edge() {
            record.set_edge(CREATED_BY, Some(&edge));
            record.set_edge(MANAGED_BY, Some(&edge));
        }
    }

    fn control_nodes(records: &[AgentRecord]) -> Vec<ControlNode> {
        records
            .iter()
            .map(|record| ControlNode {
                name: record.name.clone(),
                token: record.token.clone(),
                former_tokens: record.former_tokens(),
                managed_by: record.edge(MANAGED_BY),
                reachable: matches!(record.lifecycle.as_str(), "running" | "starting"),
            })
            .collect()
    }

    /// The designation and the record it names now, found by token so a rename or revive keeps
    /// it.
    fn designated_record(
        &self,
        records: &[AgentRecord],
    ) -> Result<Option<(Designation, Option<AgentRecord>)>> {
        let Some(designation) = read_designation(&self.registry)? else {
            return Ok(None);
        };
        let current = records
            .iter()
            .find(|record| record.name == designation.name && record.answers_to(&designation.token))
            .or_else(|| {
                records
                    .iter()
                    .find(|record| record.answers_to(&designation.token))
            })
            .cloned();
        Ok(Some((designation, current)))
    }

    /// The name of the designated coordinator's current record, if one answers to it.
    pub(crate) fn designated_coordinator(&self) -> Result<Option<String>> {
        let records = self.readable_records()?;
        Ok(self
            .designated_record(&records)?
            .and_then(|(_, record)| record.map(|record| record.name)))
    }

    /// What control decisions read: every readable record as a node, the designated
    /// coordinator's current name, and the registry path edges must name to count.
    pub(crate) fn control_view(&self) -> Result<(Vec<ControlNode>, Option<String>, PathBuf)> {
        let records = self.readable_records()?;
        let designated = self
            .designated_record(&records)?
            .and_then(|(_, record)| record.map(|record| record.name));
        Ok((
            Self::control_nodes(&records),
            designated,
            PathBuf::from(self.registry_text()),
        ))
    }

    fn designation_lock(&self) -> Result<File> {
        agent::create_private_directory(&self.registry, "agent registry", true, true)?;
        let path = self.registry.join(DESIGNATION_LOCK);
        let file = agent::open_private_lock(&path, "coordinator designation lock")?;
        file.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        Ok(file)
    }

    /// Check every agent-teams declaration under `teams_state` after a coordinator handover
    /// (#721), and fix what the handover made stale:
    ///
    /// - a deliverer naming this registry and pinned to the old coordinator's name loses the
    ///   pin, so delivery resolves this registry's designated coordinator at send time; a pin to
    ///   any other name (not the new coordinator) is flagged for a human;
    /// - a pinned coordinator process (`coordinator.designation: "pinned"`), which decides who
    ///   may send for the team: in a declaration naming this registry it becomes
    ///   `{"designation": "registry"}` when it is the old coordinator's harness, a process that
    ///   is gone, or unreadable (a pin to some other live process is flagged); in a declaration
    ///   naming no registry (what `agent-teams init --coordinator-self` writes) a pin to exactly
    ///   the old coordinator's harness is moved to the new coordinator's harness. Without this
    ///   the new coordinator's sends are refused as sender-not-coordinator.
    ///
    /// Returns the JSON report embedded in `coordinator set`'s output.
    fn team_declarations_handover(
        &self,
        teams_state: &Path,
        old: Option<&str>,
        old_identity: Option<CustomProcessIdentity>,
        new: &str,
    ) -> Value {
        let registry = self.registry_text();
        let new_identity = self
            .load(new)
            .ok()
            .and_then(|record| record.acting_process().cloned());
        let mut fixed = Vec::new();
        let mut flagged = Vec::new();
        let mut checked = 0_usize;
        for (path, mut document) in declarations_all(teams_state) {
            let naming = declaration_names_registry(&document, &self.registry);
            let team = document
                .get("team")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned();
            let pin = pinned_designation(&document);
            if !naming {
                // Only a pin to exactly the old harness is this registry's business.
                let Some(Some(identity)) = pin else { continue };
                if old_identity.as_ref() != Some(&identity) {
                    continue;
                }
                checked += 1;
                let Some(successor) = &new_identity else {
                    flagged.push(json!({
                        "team": team, "pinned_designation": true,
                        "reason": "declaration pins the old coordinator's harness and the new coordinator has no pinned harness process to move it to",
                        "fix": format!("re-run `agent-teams init {team} --coordinator-self` from the new coordinator, or `agent-teams init {team} --registry {registry}`"),
                    }));
                    continue;
                };
                document["coordinator"]["harness_identity"] = json!(successor);
                document["updated_at"] = json!(now_seconds());
                document["updated_by"] = json!("agentctl coordinator set (handover)");
                match agent::atomic_json(&path, &document) {
                    Ok(()) => fixed.push(json!({
                        "team": team, "repinned_designation": true,
                        "declaration": path.to_string_lossy(),
                    })),
                    Err(error) => flagged.push(json!({
                        "team": team, "pinned_designation": true,
                        "reason": format!("could not rewrite the declaration: {error}"),
                        "fix": format!("re-run `agent-teams init {team} --coordinator-self` from the new coordinator"),
                    })),
                }
                continue;
            }
            checked += 1;
            let mut unpinned_designation = false;
            if let Some(identity) = pin {
                let stale = match &identity {
                    None => true,
                    Some(identity) => {
                        old_identity.as_ref() == Some(identity)
                            || self.client.process_liveness(identity) == ProcessLiveness::Dead
                    }
                };
                if stale {
                    document["coordinator"] = registry_designation();
                    unpinned_designation = true;
                } else {
                    flagged.push(json!({
                        "team": team, "pinned_designation": true,
                        "reason": "declaration pins a live coordinator process that is not the old coordinator's harness",
                        "fix": format!("re-run `agent-teams init {team} --registry {registry}` so sender standing follows the designated coordinator"),
                    }));
                }
            }
            let pinned = document
                .get("deliverer")
                .and_then(|deliverer| deliverer.get("agent"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let mut was_pinned = None;
            match &pinned {
                Some(agent) if Some(agent.as_str()) == old && agent != new => {
                    if let Some(deliverer) =
                        document.get_mut("deliverer").and_then(Value::as_object_mut)
                    {
                        deliverer.remove("agent");
                    }
                    was_pinned = Some(agent.clone());
                }
                Some(agent) if agent != new => flagged.push(json!({
                    "team": team, "pinned": agent,
                    "reason": "declaration pins an agent that is neither the old nor the new coordinator",
                    "fix": format!("re-run `agent-teams init {team} --registry {registry}` to drop the pin"),
                })),
                _ => {}
            }
            if !unpinned_designation && was_pinned.is_none() {
                continue;
            }
            document["updated_at"] = json!(now_seconds());
            document["updated_by"] = json!("agentctl coordinator set (handover)");
            match agent::atomic_json(&path, &document) {
                Ok(()) => fixed.push(json!({
                    "team": team, "was_pinned": was_pinned,
                    "unpinned_designation": unpinned_designation,
                    "declaration": path.to_string_lossy(),
                })),
                Err(error) => flagged.push(json!({
                    "team": team, "pinned": was_pinned,
                    "reason": format!("could not rewrite the declaration: {error}"),
                    "fix": format!("re-run `agent-teams init {team} --registry {registry}`"),
                })),
            }
        }
        json!({"checked": checked, "fixed": fixed, "flagged": flagged})
    }

    /// The acting process of the generation `designation` (the one being replaced) names, which
    /// `old` (its current name) identifies: its live record when that record still is the designated
    /// generation (its token or a former one), else its archived record (`archive/*/agent.json`
    /// with the designated token), so a coordinator stopped before the handover is still known,
    /// and a different agent registered since under the same name is never taken for it.
    fn designated_generation_process(
        &self,
        old: &str,
        designation: &Designation,
    ) -> Option<CustomProcessIdentity> {
        if let Ok(record) = self.load(old) {
            if record.token == designation.token
                || record.former_tokens().contains(&designation.token)
            {
                return record.acting_process().cloned();
            }
        }
        let entries = fs::read_dir(self.registry.join("archive")).ok()?;
        for entry in entries.flatten() {
            let Some(document) = fs::read(entry.path().join("agent.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            else {
                continue;
            };
            if document.get("token").and_then(Value::as_str) != Some(designation.token.as_str()) {
                continue;
            }
            return ["harness_identity", "custom_process_identity"]
                .iter()
                .filter_map(|key| document.get(*key).cloned())
                .filter_map(|value| serde_json::from_value::<CustomProcessIdentity>(value).ok())
                .find(CustomProcessIdentity::valid);
        }
        None
    }

    /// Designate the registered agent `agent_name` as this registry's coordinator.
    pub fn coordinator_set(&self, agent_name: &str) -> Result<Value> {
        self.coordinator_set_in(agent_name, &agent_teams_state_dir())
    }

    /// [`Self::coordinator_set`], fixing the agent-teams declarations under `teams_state`.
    pub(crate) fn coordinator_set_in(&self, agent_name: &str, teams_state: &Path) -> Result<Value> {
        let _lock = self.designation_lock()?;
        // The designated name even when its record is gone (stopped first, or retired dead),
        // so a deliverer pinned to it is still recognised as the old coordinator's.
        let previous = read_designation(&self.registry).ok().flatten();
        let old = self.designated_coordinator().ok().flatten().or_else(|| {
            previous
                .as_ref()
                .map(|designation| designation.name.clone())
        });
        let old_identity = old
            .as_deref()
            .zip(previous.as_ref())
            .and_then(|(name, designation)| self.designated_generation_process(name, designation));
        let record = self.load(agent_name)?;
        // Never designate a target that never started (#747): a half-failed handover
        // would strand inter-team delivery on a launch_failed record. A running record
        // whose harness died stays designatable: the designation follows its revive.
        if record.lifecycle == "launch_failed" {
            return Err(fail(format!(
                "coordinator target {agent_name:?} is not alive (lifecycle {}); start or revive it first",
                record.lifecycle
            )));
        }
        let designation = Designation {
            unknown: Default::default(),
            schema: DESIGNATION_SCHEMA.to_owned(),
            name: record.name.clone(),
            token: record.token.clone(),
            set_at: now_seconds(),
            set_by: self.caller_edge(),
        };
        agent::atomic_json(
            &self.registry.join(DESIGNATION_FILE),
            &serde_json::to_value(&designation).map_err(|error| fail(error.to_string()))?,
        )?;
        let mut report = self.coordinator_show()?;
        report["team_declarations"] = self.team_declarations_handover(
            teams_state,
            old.as_deref(),
            old_identity,
            &record.name,
        );
        report["host_index"] = match self.host_index_refresh() {
            Ok(path) => json!({"registered": true, "path": path.to_string_lossy()}),
            Err(error) => json!({"registered": false, "error": error.to_string()}),
        };
        Ok(report)
    }

    /// Move `agent_name`'s Herdr tab to the front of its workspace when it is a coordinator
    /// (#849 coordinator-tab-first): the designated coordinator, or an agent launched with a
    /// coordinator profile ([`crate::tab_order::is_coordinator_profile`]). Coordinators already at
    /// the front keep their order. Best-effort: with no Herdr socket nothing happens, and a failed
    /// move is reported on standard error and never fails the command that asked for it.
    /// `socket` is the Herdr socket to use ([`crate::tab_order::socket_for`]); `None` does nothing.
    pub fn bring_coordinator_tab_forward(&self, agent_name: &str, socket: Option<&Path>) {
        let Some(socket) = socket else {
            return;
        };
        if let Err(error) = self.bring_coordinator_tab_forward_through(agent_name, socket) {
            eprintln!(
                "agentctl: could not move coordinator {agent_name}'s tab to the front of its \
                 workspace: {error}"
            );
        }
    }

    /// [`Self::bring_coordinator_tab_forward`] for every registered agent (`revive --all`).
    pub fn bring_every_coordinator_tab_forward(&self, socket: Option<&Path>) {
        if socket.is_none() {
            return;
        }
        for name in self.registered_names().unwrap_or_default() {
            self.bring_coordinator_tab_forward(&name, socket);
        }
    }

    /// [`Self::bring_coordinator_tab_forward`] through the Herdr socket at `socket`: the index the
    /// tab moved to, or `None` when it is no coordinator, has no tab, or was already in place.
    pub(crate) fn bring_coordinator_tab_forward_through(
        &self,
        agent_name: &str,
        socket: &Path,
    ) -> std::result::Result<Option<usize>, String> {
        let records = self.readable_records().map_err(|error| error.to_string())?;
        let designated = self.designated_coordinator().ok().flatten();
        let is_coordinator = |record: &AgentRecord| {
            designated.as_deref() == Some(record.name.as_str())
                || crate::tab_order::is_coordinator_profile(record.profile.as_deref())
        };
        let Some(record) = records.iter().find(|record| record.name == agent_name) else {
            return Ok(None);
        };
        if !is_coordinator(record) || record.is_cloud() {
            return Ok(None);
        }
        // A headless coordinator has no pane or tab of its own: its viewer
        // tab is the tab to bring forward (#1225 Phase 1). Its workspace is
        // the viewer tab's own, when the record names none.
        let viewer = record.viewer_tab();
        let tab = record.tab_id.as_deref().or(viewer.as_deref());
        let workspace = record.workspace_id.as_deref().or_else(|| {
            viewer
                .as_deref()
                .and_then(|tab| tab.split(':').next())
                .filter(|workspace| !workspace.is_empty())
        });
        let (Some(workspace), Some(tab)) = (workspace, tab) else {
            return Ok(None);
        };
        let others = records
            .iter()
            .filter(|other| other.name != agent_name && is_coordinator(other))
            .filter(|other| other.workspace_id.as_deref() == Some(workspace))
            .filter_map(|other| other.tab_id.clone().or_else(|| other.viewer_tab()))
            .collect();
        crate::tab_order::bring_forward(socket, workspace, tab, &others)
    }

    /// Atomic coordinator handover (#747): verify the new coordinator is alive (reviving
    /// its record first when it is dead), designate it, and only then stop the old one.
    /// Any failure before designation leaves the old coordinator running and designated.
    pub fn handover(&self, old_name: &str, new_name: &str) -> Result<Value> {
        if old_name == new_name {
            return Err(fail("handover needs two different agent names"));
        }
        let record = self.load(new_name)?;
        let alive = record.lifecycle != "launch_failed"
            && record
                .harness_anchor()
                .or(record.custom_process_identity.as_ref())
                .is_none_or(|identity| {
                    self.client.process_liveness(identity) != ProcessLiveness::Dead
                });
        let revived = if alive {
            false
        } else {
            self.revive(new_name, false, None, std::time::Duration::from_secs(30))?;
            true
        };
        let designation = self.coordinator_set(new_name)?;
        let stopped = self.stop(old_name)?;
        Ok(json!({
            "old": old_name,
            "new": new_name,
            "revived_new": revived,
            "designation": designation,
            "stopped": stopped,
        }))
    }

    /// Whether `agent_name`'s recorded harness process is positively gone (kernel evidence of a
    /// changed generation or exit); a record without a pinned process, or a probe that cannot
    /// tell, is not dead.
    pub fn harness_dead(&self, agent_name: &str) -> bool {
        self.load(agent_name)
            .ok()
            .and_then(|record| record.acting_process().cloned())
            .is_some_and(|identity| {
                self.client.process_liveness(&identity) == ProcessLiveness::Dead
            })
    }

    /// Report the designation and the state of the agent it names.
    pub fn coordinator_show(&self) -> Result<Value> {
        let records = self.readable_records()?;
        let Some((designation, current)) = self.designated_record(&records)? else {
            return Ok(json!({"designated": false, "registry": self.registry_text()}));
        };
        let boot = agent_queue::identity::current_boot_id().ok();
        let state = match &current {
            Some(record) => json!(record.lifecycle),
            None if self.archived_token(&designation.token) => json!("coordinator-stopped"),
            None if records.iter().any(|record| record.name == designation.name) => {
                json!("replaced")
            }
            None => json!("missing"),
        };
        Ok(json!({
            "designated": true,
            "registry": self.registry_text(),
            "name": current.as_ref().map_or(designation.name.clone(), |record| record.name.clone()),
            "designated_name": designation.name,
            "token": current.as_ref().map_or(designation.token.clone(), |record| record.token.clone()),
            "designated_token": designation.token,
            "set_at": designation.set_at,
            "set_by": designation.set_by,
            "state": state,
            "harness_pid": current.as_ref().and_then(|record| record.current_harness_pid(boot.as_deref())),
        }))
    }

    /// Whether a stopped record with `token` is in the registry's archive, whose entries are
    /// named `NAME-TOKEN`.
    fn archived_token(&self, token: &str) -> bool {
        let suffix = format!("-{token}");
        fs::read_dir(self.registry.join("archive")).is_ok_and(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
        })
    }

    /// Remove the designation.
    pub fn coordinator_clear(&self) -> Result<Value> {
        let _lock = self.designation_lock()?;
        let previous = read_designation(&self.registry)?;
        if previous.is_some() {
            let path = self.registry.join(DESIGNATION_FILE);
            fs::remove_file(&path)
                .map_err(|error| fail(format!("cannot remove {}: {error}", path.display())))?;
            agent::sync_directory(&self.registry)?;
        }
        self.host_index_remove();
        Ok(json!({"cleared": previous.is_some(), "previous": previous}))
    }

    /// Refresh this registry's host-index entry from its current
    /// designated record. Best-effort by design: the designation file is
    /// the authority, the index only a discovery cache, so a failure
    /// here is reported to the caller but never fails a designation.
    pub(crate) fn host_index_refresh(&self) -> std::result::Result<PathBuf, String> {
        let records = self.readable_records().map_err(|error| error.to_string())?;
        let Some((designation, current)) = self
            .designated_record(&records)
            .map_err(|error| error.to_string())?
        else {
            return Err("no coordinator designated".to_owned());
        };
        let Some(record) = current else {
            return Err("designated coordinator has no live record".to_owned());
        };
        let registry = self.registry_text();
        let harness_cwd = PathBuf::from(&registry)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = json!({
            "schema": "agentctl-coordinator-index/v1",
            "registry": registry,
            "name": record.name,
            "token": designation.token,
            "harness_cwd": harness_cwd,
            "harness_identity": record.acting_process().map(|identity| {
                serde_json::to_value(identity).unwrap_or(Value::Null)
            }),
            "repo_origins": harness_repo_origins(Path::new(&harness_cwd)),
            "team": declared_team_for_registry(&registry),
            "set_at": designation.set_at,
            "updated_at": now_seconds(),
        });
        let directory = host_index_dir();
        agent::create_private_directory(&directory, "coordinator index", true, true)
            .map_err(|error| error.to_string())?;
        let path = host_index_path(&directory, &registry);
        agent::atomic_json(&path, &entry).map_err(|error| error.to_string())?;
        Ok(path)
    }

    /// Drop this registry's host-index entry, if any.
    pub(crate) fn host_index_remove(&self) {
        let path = host_index_path(&host_index_dir(), &self.registry_text());
        let _ = fs::remove_file(path);
    }

    /// Designate `agent_name` when `as_coordinator` asks for it, after an adopt or attach-self.
    pub(crate) fn designate_after(
        &self,
        result: &mut Value,
        agent_name: &str,
        as_coordinator: bool,
    ) {
        if !as_coordinator {
            return;
        }
        result["coordinator"] = self
            .coordinator_set(agent_name)
            .unwrap_or_else(|error| json!({"designated": false, "error": error.to_string()}));
    }

    /// Keep the designation's recorded name current after `old` was renamed `new`.
    pub(super) fn follow_rename(&self, new: &str) -> Result<()> {
        let _lock = self.designation_lock()?;
        let Some(mut designation) = read_designation(&self.registry)? else {
            return Ok(());
        };
        let record = self.read_record(new)?;
        if designation.name == new || !record.answers_to(&designation.token) {
            return Ok(());
        }
        designation.name = new.to_owned();
        agent::atomic_json(
            &self.registry.join(DESIGNATION_FILE),
            &serde_json::to_value(&designation).map_err(|error| fail(error.to_string()))?,
        )
    }

    /// Change who controls `agent_name`.
    pub fn manage(&self, agent_name: &str, action: ManageAction) -> Result<Value> {
        if matches!(action, ManageAction::Pin | ManageAction::Unpin) {
            let _lock = self.lock(agent_name)?;
            let mut record = self.load(agent_name)?;
            let previous = record.pinned_explicit();
            record.set_pinned(match action {
                ManageAction::Pin => Some(true),
                _ => None,
            });
            self.save(&record)?;
            return Ok(json!({
                "name": record.name,
                "token": record.token,
                "pinned": record.pinned_explicit(),
                "previous": previous,
            }));
        }
        let edge = match &action {
            ManageAction::Pin | ManageAction::Unpin => unreachable!("pin handled above"),
            ManageAction::Clear => None,
            ManageAction::Caller => Some(self.caller_edge().ok_or_else(|| {
                fail(
                    "cannot prove this process belongs to a registered agent (no pinned harness \
                     of this registry is among its ancestors); name the manager with --by NAME, \
                     or remove the manager with --clear",
                )
            })?),
            ManageAction::By(manager) => {
                let record = self.load(manager)?;
                Some(Edge {
                    unknown: Default::default(),
                    agent: record.name.clone(),
                    registry: self.registry_text(),
                    token: record.token,
                    proof: PROOF_ASSIGNED.to_owned(),
                })
            }
        };
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        if let Some(edge) = &edge {
            if edge.agent == record.name || record.answers_to(&edge.token) {
                return Err(fail(format!("agent {agent_name:?} cannot manage itself")));
            }
            let registry = self.registry_text();
            let mut nodes = Self::control_nodes(&self.readable_records()?);
            if let Some(node) = nodes.iter_mut().find(|node| node.name == record.name) {
                node.managed_by = Some(edge.clone());
            }
            let mut current = nodes.iter().find(|node| node.name == record.name);
            let mut seen = BTreeSet::new();
            while let Some(node) = current {
                if !seen.insert(node.name.clone()) {
                    return Err(fail(format!(
                        "making {:?} the manager of {agent_name:?} would make a cycle",
                        edge.agent
                    )));
                }
                current = tree_parent(&nodes, Path::new(&registry), node);
            }
        }
        let previous = record.edge(MANAGED_BY);
        record.set_edge(MANAGED_BY, edge.as_ref());
        self.save(&record)?;
        Ok(json!({
            "name": record.name,
            "token": record.token,
            "managed_by": edge,
            "previous": previous,
        }))
    }

    /// Name the Herdr viewer tab of `agent_name`, or clear it (#1225
    /// workspace-goal-reconciler, Phase 1): tab hygiene then ranks that tab
    /// with the agent, and `bring_forward` can keep a headless coordinator's
    /// viewer first. The tab is a view only; no pane id is read or changed.
    pub fn set_viewer_tab(&self, agent_name: &str, tab: Option<String>) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        let previous = record.viewer_tab();
        record.set_viewer_tab(tab.as_deref());
        if previous != tab {
            self.save(&record)?;
        }
        Ok(json!({
            "name": record.name,
            "viewer_tab_id": tab,
            "previous": previous,
        }))
    }

    /// Read, set, or clear what `agent_name` is for (#1342 agent-purpose-line).
    /// With neither `text` nor `clear` this only reads. Setting or clearing
    /// writes the record once and never prompts the agent; `goal` is untouched.
    pub fn purpose(&self, agent_name: &str, text: Option<&str>, clear: bool) -> Result<Value> {
        if !clear && text.is_none() {
            let record = self.load(agent_name)?;
            let purpose = record.purpose();
            return Ok(json!({
                "name": record.name,
                "purpose": purpose,
                "previous": purpose,
            }));
        }
        let new = if clear {
            None
        } else {
            Some(validated_purpose(text.unwrap_or_default())?)
        };
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        let previous = record.purpose();
        if previous != new {
            record.set_purpose(new.as_deref());
            self.save(&record)?;
        }
        Ok(json!({
            "name": record.name,
            "purpose": new,
            "previous": previous,
        }))
    }

    /// The recorded purpose of `agent_name`, for display surfaces that hold a
    /// manager (the headless console banner); `None` when unset or unreadable.
    pub(crate) fn purpose_of(&self, agent_name: &str) -> Option<String> {
        self.load(agent_name).ok()?.purpose()
    }

    /// Mark `agent_name` standing or not (#901 watch-standing-agents): `watch-agents` then
    /// reports no idle-without-verdict episode for it, and every other notice stays. The flag is
    /// part of the record, so it follows the agent through rename, restore and revive.
    pub fn set_standing(&self, agent_name: &str, standing: bool) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        let previous = record.standing();
        record.set_standing(standing);
        if previous != standing {
            self.save(&record)?;
        }
        Ok(json!({
            "name": record.name,
            "standing": standing,
            "previous": previous,
        }))
    }

    /// The control tree: the designated coordinator at the root, every agent under its nearest
    /// `managed_by` that is registered here, and the rest under the root.
    pub fn tree(&self) -> Result<Value> {
        let records = self.readable_records()?;
        let designation = self.designated_record(&records)?;
        let designated = designation
            .as_ref()
            .and_then(|(_, record)| record.as_ref().map(|record| record.name.clone()));
        let nodes = Self::control_nodes(&records);
        let registry = self.registry_text();
        let boot = agent_queue::identity::current_boot_id().ok();
        // Each agent's parent, or `None` for a root. An edge into a cycle is cut at the agent
        // where the cycle was found.
        let mut parents: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut notes: BTreeMap<String, &'static str> = BTreeMap::new();
        for node in &nodes {
            let parent =
                tree_parent(&nodes, Path::new(&registry), node).map(|parent| parent.name.clone());
            let note = match (&node.managed_by, &parent) {
                (None, _) => "unmanaged",
                (Some(_), None) => "manager-not-here",
                (Some(_), Some(_)) => "managed",
            };
            notes.insert(node.name.clone(), note);
            parents.insert(node.name.clone(), parent);
        }
        for node in &nodes {
            let mut seen = BTreeSet::from([node.name.clone()]);
            let mut current = parents.get(&node.name).cloned().flatten();
            while let Some(name) = current {
                if !seen.insert(name.clone()) {
                    // Cut the cycle at the agent the walk started from only when that agent is
                    // on it; one that merely leads into a cycle keeps its parent.
                    if name == node.name {
                        parents.insert(node.name.clone(), None);
                        notes.insert(node.name.clone(), "cycle");
                    }
                    break;
                }
                current = parents.get(&name).cloned().flatten();
            }
        }
        // Agents with no parent other than the coordinator itself hang under the coordinator.
        if let Some(root) = designated.as_deref() {
            parents.insert(root.to_owned(), None);
            for (name, parent) in parents.iter_mut() {
                if parent.is_none() && name != root {
                    *parent = Some(root.to_owned());
                }
            }
        }
        let restore_agents = restore_agent_names(&self.registry);
        fn build(
            name: &str,
            records: &[AgentRecord],
            parents: &BTreeMap<String, Option<String>>,
            notes: &BTreeMap<String, &'static str>,
            designated: Option<&str>,
            boot: Option<&str>,
            restore_agents: &BTreeSet<String>,
        ) -> Value {
            let record = records.iter().find(|record| record.name == name);
            let children: Vec<Value> = parents
                .iter()
                .filter(|(_, parent)| parent.as_deref() == Some(name))
                .map(|(child, _)| {
                    build(
                        child,
                        records,
                        parents,
                        notes,
                        designated,
                        boot,
                        restore_agents,
                    )
                })
                .collect();
            let (pinned, pinned_source) = record.map_or((false, "none"), |record| {
                pinned_classification(record, designated, restore_agents, !children.is_empty())
            });
            json!({
                "name": name,
                "token": record.map(|record| record.token.clone()),
                "lifecycle": record.map(|record| record.lifecycle.clone()),
                "harness": record.map(|record| record.harness.clone()),
                "coordinator": Some(name) == designated,
                "control": if Some(name) == designated { "designated-coordinator" } else { notes.get(name).copied().unwrap_or("unmanaged") },
                "created_by": record.and_then(|record| record.edge(CREATED_BY)),
                "managed_by": record.and_then(|record| record.edge(MANAGED_BY)),
                "harness_pid": record.and_then(|record| record.current_harness_pid(boot)),
                "pinned": pinned,
                "pinned_source": pinned_source,
                "children": children,
            })
        }
        let roots: Vec<Value> = parents
            .iter()
            .filter(|(_, parent)| parent.is_none())
            .map(|(name, _)| {
                build(
                    name,
                    &records,
                    &parents,
                    &notes,
                    designated.as_deref(),
                    boot.as_deref(),
                    &restore_agents,
                )
            })
            .collect();
        Ok(json!({
            "registry": self.registry_text(),
            "coordinator": designated,
            "designation": designation.map(|(designation, current)| json!({
                "name": designation.name,
                "token": designation.token,
                "resolved": current.is_some(),
            })),
            "agents": records.len(),
            "roots": roots,
        }))
    }
}

/// Render [`ManagedAgents::tree`] as indented text, one agent per line.
pub fn render_tree(tree: &Value) -> String {
    fn line(node: &Value, prefix: &str, last: bool, top: bool, out: &mut String) {
        let mut facts = vec![node["lifecycle"].as_str().unwrap_or("unknown").to_owned()];
        if node["coordinator"] == true {
            facts.insert(0, "designated coordinator".to_owned());
        }
        if node["pinned"] == true {
            facts.push("pinned".to_owned());
        }
        match node["control"].as_str() {
            Some("unmanaged") if !top => facts.push("no manager recorded".to_owned()),
            Some("manager-not-here") => facts.push(format!(
                "managed by {} outside this registry or stopped",
                node["managed_by"]["agent"].as_str().unwrap_or("?")
            )),
            Some("cycle") => facts.push("managed_by forms a cycle; cut here".to_owned()),
            _ => {}
        }
        let created = node["created_by"]["agent"].as_str();
        let managed = node["managed_by"]["agent"].as_str();
        if let Some(creator) = created.filter(|creator| Some(*creator) != managed) {
            facts.push(format!("created by {creator}"));
        }
        let branch = if top {
            ""
        } else if last {
            "└── "
        } else {
            "├── "
        };
        out.push_str(&format!(
            "{prefix}{branch}{} ({})\n",
            node["name"].as_str().unwrap_or("?"),
            facts.join("; ")
        ));
        let children = node["children"].as_array().cloned().unwrap_or_default();
        let next = if top {
            prefix.to_owned()
        } else if last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };
        for (index, child) in children.iter().enumerate() {
            line(child, &next, index + 1 == children.len(), false, out);
        }
    }
    let mut out = String::new();
    if tree["coordinator"].is_null() {
        out.push_str("(no coordinator designated; run `agentctl coordinator set NAME`)\n");
    }
    for root in tree["roots"].as_array().into_iter().flatten() {
        line(root, "", true, true, &mut out);
    }
    if tree["roots"].as_array().is_none_or(Vec::is_empty) {
        out.push_str("(no agents registered)\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(agent: &str, token: &str) -> Option<Edge> {
        Some(Edge {
            unknown: Default::default(),
            agent: agent.to_owned(),
            registry: "/r".to_owned(),
            token: token.to_owned(),
            proof: PROOF_ANCESTRY.to_owned(),
        })
    }

    fn node(name: &str, managed_by: Option<Edge>, reachable: bool) -> ControlNode {
        ControlNode {
            name: name.to_owned(),
            token: format!("t-{name}"),
            former_tokens: Vec::new(),
            managed_by,
            reachable,
        }
    }

    #[test]
    fn a_notice_goes_to_the_nearest_reachable_manager_then_the_coordinator() {
        let registry = Path::new("/r");
        let mut nodes = vec![
            node("coord", None, true),
            node("lieutenant", edge("coord", "t-coord"), true),
            node("worker", edge("lieutenant", "t-lieutenant"), true),
            node("deep", edge("worker", "t-worker"), true),
            node("orphan", None, true),
            node("stale", edge("lieutenant", "t-gone"), true),
            node(
                "foreign",
                Some(Edge {
                    registry: "/other".to_owned(),
                    ..edge("lieutenant", "t-lieutenant").unwrap()
                }),
                true,
            ),
        ];
        let to = |nodes: &[ControlNode], agent: &str| {
            controller_of(nodes, registry, Some("coord"), agent)
        };
        assert_eq!(to(&nodes, "worker").as_deref(), Some("lieutenant"));
        assert_eq!(to(&nodes, "deep").as_deref(), Some("worker"));
        assert_eq!(to(&nodes, "lieutenant").as_deref(), Some("coord"));
        assert_eq!(to(&nodes, "orphan").as_deref(), Some("coord"));
        assert_eq!(to(&nodes, "stale").as_deref(), Some("coord"));
        assert_eq!(to(&nodes, "foreign").as_deref(), Some("coord"));
        assert_eq!(
            to(&nodes, "coord"),
            None,
            "the coordinator is never told about itself"
        );
        // A dead lieutenant passes its workers' notices up to its own manager.
        nodes[1].reachable = false;
        assert_eq!(to(&nodes, "worker").as_deref(), Some("coord"));
        assert_eq!(to(&nodes, "deep").as_deref(), Some("worker"));
        // A revived manager keeps its workers through its former tokens.
        nodes[1].reachable = true;
        nodes[1].token = "t-lieutenant-2".to_owned();
        nodes[1].former_tokens = vec!["t-lieutenant".to_owned()];
        assert_eq!(to(&nodes, "worker").as_deref(), Some("lieutenant"));
        // A cycle ends at the designated coordinator.
        nodes[0].managed_by = edge("deep", "t-deep");
        assert_eq!(to(&nodes, "lieutenant").as_deref(), Some("coord"));
        let undesignated = controller_of(&nodes, registry, None, "orphan");
        assert_eq!(undesignated, None);
    }

    #[test]
    fn purpose_validation_and_brief_lines() {
        assert_eq!(
            validated_purpose("  fix the parser  ").unwrap(),
            "fix the parser"
        );
        assert!(validated_purpose("").is_err());
        assert!(validated_purpose("two\nlines").is_err());
        assert!(validated_purpose(&"x".repeat(PURPOSE_MAX_CHARS + 1)).is_err());
        assert_eq!(
            purpose_from_brief("\n# Fix the parser\n\nDetails follow.").as_deref(),
            Some("Fix the parser")
        );
        assert_eq!(
            purpose_from_brief("- [from coord via agentctl] queued\n- Review the diff\n")
                .as_deref(),
            Some("Review the diff")
        );
        assert_eq!(purpose_from_brief("  \n```\n"), None);
        let long = format!("word {}", "x".repeat(PURPOSE_MAX_CHARS));
        assert_eq!(
            purpose_from_brief(&long).map(|line| line.chars().count()),
            Some(PURPOSE_MAX_CHARS)
        );
    }

    #[test]
    fn purpose_sources_resolve_in_order() {
        assert_eq!(
            resolve_purpose(Some("explicit"), Some("slot"), Some("brief line")).as_deref(),
            Some("explicit")
        );
        assert_eq!(
            resolve_purpose(None, Some("slot purpose"), Some("brief line")).as_deref(),
            Some("slot purpose")
        );
        assert_eq!(
            resolve_purpose(None, Some("  "), Some("brief line")).as_deref(),
            Some("brief line")
        );
        assert_eq!(resolve_purpose(None, None, Some("  \n")), None);
        assert_eq!(resolve_purpose(None, None, None), None);
    }

    #[test]
    fn revive_order_puts_managers_before_their_workers() {
        let registry = Path::new("/r");
        let nodes = vec![
            node("coord", None, true),
            node("a-worker", edge("lieutenant", "t-lieutenant"), true),
            node("lieutenant", edge("coord", "t-coord"), true),
            node("b-unmanaged", None, true),
            node("z-deep", edge("a-worker", "t-a-worker"), true),
        ];
        let names: Vec<String> = ["a-worker", "b-unmanaged", "coord", "lieutenant", "z-deep"]
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(
            tree_order(&nodes, registry, Some("coord"), &names),
            ["coord", "b-unmanaged", "lieutenant", "a-worker", "z-deep"]
        );
    }
}
