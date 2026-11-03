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
const FORMER_TOKENS: &str = "former_tokens";
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
        let edge = match &action {
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
        fn build(
            name: &str,
            records: &[AgentRecord],
            parents: &BTreeMap<String, Option<String>>,
            notes: &BTreeMap<String, &'static str>,
            designated: Option<&str>,
            boot: Option<&str>,
        ) -> Value {
            let record = records.iter().find(|record| record.name == name);
            let children: Vec<Value> = parents
                .iter()
                .filter(|(_, parent)| parent.as_deref() == Some(name))
                .map(|(child, _)| build(child, records, parents, notes, designated, boot))
                .collect();
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
